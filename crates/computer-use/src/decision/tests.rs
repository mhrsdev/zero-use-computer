use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use super::*;

/// A one-route HTTP server on 127.0.0.1: answers every request with
/// `reply(body)` (status, JSON) and keeps what it was sent.
/// A request the fake server got: path, headers, body.
pub(crate) type Seen = (String, Vec<(String, String)>, String);

pub(crate) struct Fake {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

pub(crate) fn fake(reply: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let reply = Arc::new(reply);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { break };
            let s = s.clone();
            let reply = reply.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut headers = Vec::new();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    if k.eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap();
                    }
                    headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let (code, text) = reply(&body);
                s.lock().unwrap().push((path, headers, body));
                let _ = write!(
                    conn,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            });
        }
    });
    Fake { url, seen }
}

fn decider(provider: &str, url: &str, key: &str) -> Decider {
    Decider::from_config(&DecisionConfig {
        provider: provider.into(),
        base_url: url.into(),
        model: if provider == "openai" {
            "fast-1".into()
        } else {
            String::new()
        },
        api_key: key.into(),
        ..DecisionConfig::default()
    })
    .unwrap()
    .unwrap()
}

fn never() -> bool {
    false
}

#[test]
fn no_provider_is_no_decider() {
    assert!(
        Decider::from_config(&DecisionConfig::default())
            .unwrap()
            .is_none()
    );
    let e = Decider::from_config(&DecisionConfig {
        provider: "openai".into(),
        ..DecisionConfig::default()
    })
    .unwrap_err();
    assert!(e.to_string().contains("model"), "{e}");
}

#[test]
fn endpoints_are_completed() {
    assert_eq!(
        endpoint(Provider::Jev, "https://api.typesafe.ai"),
        "https://api.typesafe.ai/v1/systemone"
    );
    assert_eq!(
        endpoint(Provider::Jev, "http://127.0.0.1:8765/v1/"),
        "http://127.0.0.1:8765/v1/systemone"
    );
    assert_eq!(
        endpoint(Provider::Jev, "http://h/v1/systemone"),
        "http://h/v1/systemone"
    );
    assert_eq!(
        endpoint(Provider::OpenAi, "https://api.groq.com/openai/v1"),
        "https://api.groq.com/openai/v1/chat/completions"
    );
}

#[test]
fn jev_questions_go_out_and_answers_come_back() {
    let f = fake(|_| {
        (
            200,
            r#"{"model":"jev-1.13.0","answers":{
                "refund":{"type":"noul","noul":0.91},
                "team":{"type":"choice","choice":"billing","confidence":0.81,
                        "probabilities":{"technical":0.12,"billing":0.88}},
                "severity":{"type":"score","score":1.2,"confidence":0.55,
                        "legend":{"0":"Minor","1":"Real","2":"Blocking"},
                        "probabilities":{"0":0.1,"1":0.6,"2":0.3}}},
                "usage":{"input_tokens":612,"output_tokens":3}}"#
                .into(),
        )
    });
    let d = decider("jev", &f.url, "sk-secret-1234");
    let qs = [
        Question::yes_no("refund", "The customer asks for money back"),
        Question::choice("team", "Which team handles it", &["billing", "technical"]),
        Question::score("severity", "How serious", &["Minor", "Real", "Blocking"]),
    ];
    let (answers, _) = d
        .ask("You charged my card twice \"again\"\nفوری", &qs, &never)
        .unwrap();
    assert_eq!(answers[0].1, Answer::YesNo { yes: 0.91 });
    assert_eq!(answers[0].1.brief(), "yes (0.91)");
    assert!(
        matches!(&answers[1].1, Answer::Choice { choice, .. } if choice == "billing"),
        "{answers:?}"
    );
    assert_eq!(answers[1].1.brief(), "billing (0.88)");
    assert!(
        matches!(&answers[2].1, Answer::Score { level, .. } if level == "Real"),
        "{answers:?}"
    );

    let seen = f.seen.lock().unwrap();
    let (path, headers, body) = &seen[0];
    assert_eq!(path, "/v1/systemone");
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer sk-secret-1234"),
        "{headers:?}"
    );
    let sent: Value = serde_json::from_str(body).unwrap();
    assert_eq!(sent["model"], "jev-latest");
    // The state arrives exactly as given (quotes, newline, Persian).
    assert_eq!(sent["state"], "You charged my card twice \"again\"\nفوری");
    assert_eq!(sent["questions"]["refund"]["type"], "noul");
    assert_eq!(sent["questions"]["team"]["criteria"]["billing"], "billing");
    assert_eq!(sent["questions"]["severity"]["criteria"][2], "Blocking");
}

#[test]
fn the_key_is_never_printed_and_config_values_are_escaped() {
    let d = decider("jev", "http://127.0.0.1:9", "sk-very-secret");
    assert!(!format!("{d:?}").contains("sk-very-secret"));
    assert_eq!(quote("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
}

#[test]
fn http_errors_say_what_to_do() {
    let f = fake(|_| (401, r#"{"detail":"invalid api key"}"#.into()));
    let d = decider("jev", &f.url, "bad");
    let e = d
        .ask("x", &[Question::yes_no("a", "is it?")], &never)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("refused the API key") && e.contains("invalid api key"),
        "{e}"
    );
    assert!(e.contains(SETTINGS_KEY), "{e}");

    let f = fake(|_| (422, r#"{"detail":[{"msg":"too many labels"}]}"#.into()));
    let e = decider("jev", &f.url, "k")
        .ask("x", &[Question::yes_no("a", "is it?")], &never)
        .unwrap_err()
        .to_string();
    assert!(e.contains("422") && e.contains("too many labels"), "{e}");
}

#[test]
fn an_unreachable_model_is_reported() {
    // Nothing listens on this port.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let d = decider("jev", &format!("http://127.0.0.1:{port}"), "");
    let e = d
        .ask("x", &[Question::yes_no("a", "is it?")], &never)
        .unwrap_err()
        .to_string();
    assert!(e.contains("couldn't reach the decision model"), "{e}");
}

#[test]
fn chat_models_answer_in_json() {
    let f = fake(|body| {
        let v: Value = serde_json::from_str(body).unwrap();
        // Reasoning models refuse a temperature: the request comes again.
        if v.get("temperature").is_some() {
            return (
                400,
                r#"{"error":{"message":"Unsupported value: 'temperature'"}}"#.into(),
            );
        }
        let content = "Sure!\n```json\n{\"done\": \"yes\", \"Team\": \"Technical.\", \"mood\": \"Angry\"}\n```";
        (
            200,
            json!({"choices": [{"message": {"role": "assistant", "content": content}}]})
                .to_string(),
        )
    });
    let d = decider("openai", &f.url, "sk-x");
    let qs = [
        Question::yes_no("done", "Is the page loaded?"),
        Question::choice("Team", "Which team", &["billing", "technical"]),
        Question::score("mood", "How upset", &["Calm", "Annoyed", "Angry"]),
    ];
    let (answers, _) = d.ask("state", &qs, &never).unwrap();
    assert!(answers[0].1.is_yes());
    assert!(matches!(&answers[1].1, Answer::Choice { choice, .. } if choice == "technical"));
    assert!(matches!(&answers[2].1, Answer::Score { score, .. } if *score == 2.0));
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].0, "/chat/completions");
    let sent: Value = serde_json::from_str(&seen[1].2).unwrap();
    assert_eq!(sent["model"], "fast-1");
    assert!(
        sent["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("\"Team\" (choice)")
    );
}

#[test]
fn many_states_are_judged_at_once() {
    let f = fake(|body| {
        let v: Value = serde_json::from_str(body).unwrap();
        let yes = if v["state"].as_str().unwrap().contains("good") {
            0.9
        } else {
            0.1
        };
        // Slow enough that one at a time would show.
        std::thread::sleep(std::time::Duration::from_millis(150));
        (
            200,
            json!({"answers": {"answer": {"type": "noul", "noul": yes}}}).to_string(),
        )
    });
    let d = decider("jev", &f.url, "k");
    let states: Vec<String> = (0..8)
        .map(|i| {
            if i % 2 == 0 {
                format!("good {i}")
            } else {
                format!("bad {i}")
            }
        })
        .collect();
    let start = std::time::Instant::now();
    let out = d.ask_each(
        &states,
        &[Question::yes_no("answer", "Is it good?")],
        &never,
    );
    assert!(
        start.elapsed() < std::time::Duration::from_millis(900),
        "{:?}",
        start.elapsed()
    );
    for (i, r) in out.iter().enumerate() {
        let (a, _) = r.as_ref().unwrap();
        assert_eq!(a[0].1.is_yes(), i % 2 == 0);
    }
}

#[test]
fn questions_are_read_from_a_tool_call() {
    let qs = questions_from(&json!({
        "loaded": {"question": "Has it loaded?"},
        "kind": {"type": "choice", "question": "What is it?", "options": {"bug": "Something broken", "idea": "A wish"}},
        "mood": {"question": "How upset?", "scale": ["calm", "upset", "furious"]},
        "urgent": {"type": "yes-no", "instructions": "Urgent?", "criteria": {"true": "now", "false": "later"}}
    }))
    .unwrap();
    let by = |n: &str| qs.iter().find(|q| q.name == n).unwrap().clone();
    assert_eq!(by("loaded").kind, Kind::YesNo);
    assert_eq!(by("kind").kind, Kind::Choice);
    assert_eq!(
        by("kind").options[0],
        ("bug".into(), "Something broken".into())
    );
    assert_eq!(by("mood").kind, Kind::Score);
    assert_eq!(by("urgent").options.len(), 2);
    assert!(
        questions_from(&json!({"x": {"type": "choice", "question": "?", "options": ["only"]}}))
            .is_err()
    );
    assert!(questions_from(&json!({"x": {"type": "guess", "question": "?"}})).is_err());
    assert!(
        questions_from(&json!({"x": {"type": "choice", "question": "?", "options": ["a", "A"]}}))
            .is_err()
    );
}

#[test]
fn long_states_keep_start_and_end() {
    let s: String = (0..1000)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    let c = clip(&s, 300);
    assert!(c.chars().count() <= 300);
    assert!(c.starts_with("abc") && c.ends_with(&s[s.len() - 10..]));
}

#[test]
fn a_number_answer_is_never_another_element_when_the_options_are_numbers() {
    // Picking an element: the options are element indices.
    let q = [Question::choice("element", "Which one", &["3", "8", "57"])];
    let reply = |content: &str| json!({"choices": [{"message": {"content": content}}]});
    let chosen = |content: &str| match parse_chat(&reply(content), &q).unwrap().pop() {
        Some((_, Answer::Choice { choice, .. })) => choice,
        other => panic!("{other:?}"),
    };
    assert_eq!(chosen(r#"{"element": 57}"#), "57");
    assert_eq!(chosen(r#"{"element": 8}"#), "8");
    // Another element's index is none of them, never the one in that place.
    assert!(parse_chat(&reply(r#"{"element": 1}"#), &q).is_err());
    assert!(parse_chat(&reply(r#"{"element": 99}"#), &q).is_err());
}

#[test]
fn a_number_answer_is_the_option_in_that_place_when_the_options_are_words() {
    let q = [Question::choice("kind", "What is it", &["bug", "idea"])];
    let reply = json!({"choices": [{"message": {"content": r#"{"kind": 1}"#}}]});
    match parse_chat(&reply, &q).unwrap().pop() {
        Some((_, Answer::Choice { choice, .. })) => assert_eq!(choice, "idea"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_label_never_shows_a_name_and_password_from_the_address() {
    let d = decider("openai", "https://ann:hunter2@models.example/v1", "k");
    assert_eq!(d.label(), "fast-1 at models.example");
    let d = decider("openai", "http://127.0.0.1:8080/v1", "k");
    assert_eq!(d.label(), "fast-1 at 127.0.0.1:8080");
}

#[test]
fn the_label_never_shows_a_query_from_the_address() {
    let d = decider("openai", "https://proxy.example?key=SECRET", "k");
    assert_eq!(d.label(), "fast-1 at proxy.example");
    let d = decider("openai", "https://proxy.example#key=SECRET", "k");
    assert_eq!(d.label(), "fast-1 at proxy.example");
}

#[test]
fn errors_name_the_settings_key_the_user_has() {
    let f = fake(|_| (401, r#"{"detail":"invalid api key"}"#.into()));
    let ask = |d: Decider| {
        d.ask("x", &[Question::yes_no("a", "is it?")], &never)
            .unwrap_err()
            .to_string()
    };
    let e = ask(decider("jev", &f.url, "bad").with_settings_key(Some("Ctrl+Shift+F9".into())));
    assert!(
        e.contains("(Ctrl+Shift+F9)") && !e.contains(SETTINGS_KEY),
        "{e}"
    );
    let e = ask(decider("jev", &f.url, "bad").with_settings_key(None));
    assert!(
        e.contains("setup=\"open\"") && !e.contains(SETTINGS_KEY),
        "{e}"
    );
    assert!(not_set_up(Some("F9")).contains("press F9:"));
    assert!(!not_set_up(None).contains("press"));
}

#[test]
fn the_setup_test_stops_when_halted() {
    let f = fake(|_| {
        std::thread::sleep(std::time::Duration::from_secs(10));
        (500, "{}".into())
    });
    let d = decider("openai", &f.url, "k");
    let start = std::time::Instant::now();
    assert!(page::try_decider(&d, &|| true).is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}
