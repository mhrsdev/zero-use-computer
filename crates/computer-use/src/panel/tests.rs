use super::*;

fn request(host: &str, raw: &str) -> String {
    let mut s = TcpStream::connect(host).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    // Pictures are not text: read bytes.
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn get(host: &str, token: &str, route: &str) -> String {
    request(
        host,
        &format!("GET /{token}/{route} HTTP/1.1\r\nHost: {host}\r\n\r\n"),
    )
}

fn post(host: &str, token: &str, route: &str, body: &str) -> String {
    request(
        host,
        &format!(
            "POST /{token}/{route} HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

/// The JSON of a reply.
fn json_of(reply: &str) -> Value {
    let body = reply.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {reply}"))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cu-panel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Up {
    host: String,
    token: String,
    alive: Arc<AtomicBool>,
}

fn start(path: Option<PathBuf>, home: &Path) -> Up {
    let Bound::Mine(server) = Server::bind(path, home).unwrap() else {
        panic!("another panel answered");
    };
    let up = Up {
        host: server.page.host.clone(),
        token: server.page.token.clone(),
        alive: server.page.alive.clone(),
    };
    std::thread::spawn(move || server.run());
    up
}

/// A settings file whose panel uses a free port of its own.
fn config_with_port(dir: &Path, extra: &str) -> PathBuf {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let path = dir.join("config.toml");
    std::fs::write(&path, format!("[panel]\nport = {port}\n{extra}")).unwrap();
    path
}

fn entry<'a>(state: &'a Value, key: &str) -> &'a Value {
    state["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == key)
        .unwrap_or_else(|| panic!("no entry {key}"))
}

#[test]
fn only_the_page_itself_gets_in() {
    let dir = temp("gate");
    let path = config_with_port(
        &dir,
        "[decision]\nprovider = \"jev\"\napi_key = \"sk-saved-abcd1234\"\n",
    );
    let up = start(Some(path.clone()), &dir);
    let (host, token) = (&up.host, &up.token);

    let page = get(host, token, "");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("<title>Zero panel [private]</title>"));
    assert!(!page.contains("__THEME__") && !page.contains("__ACCENT__"));

    // The key never reaches the page, only its end.
    let state = post(host, token, "state", "{}");
    assert!(
        state.contains("••••1234") && !state.contains("sk-saved"),
        "{state}"
    );

    // No token, another host name, or another site: no.
    let no_token = request(host, &format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n"));
    assert!(no_token.starts_with("HTTP/1.1 404"), "{no_token}");
    let rebound = request(
        host,
        &format!("GET /{token}/ HTTP/1.1\r\nHost: evil.example:80\r\n\r\n"),
    );
    assert!(rebound.starts_with("HTTP/1.1 421"), "{rebound}");
    let body = r#"{"changes":[{"key":"hot_reload","value":false}]}"#;
    let cross = request(
        host,
        &format!(
            "POST /{token}/set HTTP/1.1\r\nHost: {host}\r\nOrigin: https://evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(cross.starts_with("HTTP/1.1 403"), "{cross}");
    let form = request(
        host,
        &format!(
            "POST /{token}/set HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(form.starts_with("HTTP/1.1 403"), "{form}");
    let fetch_site = request(
        host,
        &format!(
            "POST /{token}/set HTTP/1.1\r\nHost: {host}\r\nSec-Fetch-Site: cross-site\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(fetch_site.starts_with("HTTP/1.1 403"), "{fetch_site}");
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("hot_reload")
    );
    let too_big = request(
        host,
        &format!(
            "POST /{token}/set HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 99999999\r\n\r\n"
        ),
    );
    assert!(too_big.starts_with("HTTP/1.1 413"), "{too_big}");
    let png = get(host, token, "cursor/jelly.png");
    assert!(png.starts_with("HTTP/1.1 200") && png.contains("image/png"));
    assert!(get(host, token, "cursor/nothing.png").starts_with("HTTP/1.1 404"));
    assert!(get(host, token, "cursor/..%2f..%2fconfig.toml").starts_with("HTTP/1.1 404"));

    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_are_changed_checked_and_reset() {
    let dir = temp("set");
    let path = config_with_port(&dir, "");
    let up = start(Some(path.clone()), &dir);
    let (host, token) = (&up.host, &up.token);
    let set = |changes: &str, confirmed: bool| {
        json_of(&post(
            host,
            token,
            "set",
            &format!(r#"{{"confirmed":{confirmed},"changes":{changes}}}"#),
        ))
    };
    let load = || ConfigStore::load(Some(&path)).unwrap().config;

    // Every setting is on the page, with its default.
    let state = json_of(&post(host, token, "state", "{}"));
    assert_eq!(state["ok"], true);
    assert_eq!(
        state["entries"].as_array().unwrap().len(),
        crate::config::known_keys().len()
    );
    assert_eq!(entry(&state, "screenshot.max_dimension")["value"], 1280);
    assert_eq!(entry(&state, "screenshot.max_dimension")["changed"], false);

    // Each kind of value.
    let r = set(
        r##"[{"key":"screenshot.max_dimension","value":1024},
             {"key":"overlay.cursor_style","value":"jelly"},
             {"key":"overlay.color_working","value":"#112233"},
             {"key":"tree.diff_full_ratio","value":0.5},
             {"key":"tools.disabled","value":["drag","batch"]},
             {"key":"overlay.cursor_tag","value":""},
             {"key":"natural_mouse","value":false}]"##,
        false,
    );
    assert_eq!(r["ok"], true, "{r}");
    let c = load();
    assert_eq!(c.screenshot.max_dimension, 1024);
    assert_eq!(c.overlay.cursor_style, "jelly");
    assert_eq!(c.overlay.color_working, "#112233");
    assert_eq!(c.tree.diff_full_ratio, 0.5);
    assert_eq!(c.tools.disabled, vec!["drag".to_string(), "batch".into()]);
    assert_eq!(c.overlay.cursor_tag, "");
    assert!(!c.natural_mouse);
    assert_eq!(r["values"]["screenshot.max_dimension"], 1024);

    // Wrong types, values off the choices, out of range, several lines.
    for bad in [
        r#"[{"key":"natural_mouse","value":"yes"}]"#,
        r#"[{"key":"screenshot.max_dimension","value":"big"}]"#,
        r#"[{"key":"screenshot.max_dimension","value":-5}]"#,
        r#"[{"key":"screenshot.max_dimension","value":1.5}]"#,
        r#"[{"key":"screenshot.max_dimension","value":99999}]"#,
        r#"[{"key":"screenshot.attach","value":"sometimes"}]"#,
        r#"[{"key":"overlay.cursor_tag","value":"a\nb"}]"#,
        r#"[{"key":"tools.disabled","value":"drag"}]"#,
        r#"[{"key":"nonsense","value":1}]"#,
        r#"[{"key":"decision.api_key","value":"x"}]"#,
        r#"[{"key":"overlay.color_working","value":"blue-ish"}]"#,
        r#"[{"key":"panel.idle_minutes","value":0}]"#,
    ] {
        let r = set(bad, true);
        assert_eq!(r["ok"], false, "{bad} was accepted: {r}");
    }
    // One bad change in a group leaves the others unsaved.
    let r = set(
        r#"[{"key":"clipboard","value":false},{"key":"screenshot.attach","value":"nope"}]"#,
        false,
    );
    assert_eq!(r["ok"], false);
    assert!(load().clipboard);

    // Protected settings need the user's confirmation, on the server too.
    let prot = r#"[{"key":"control.pause_on_user_input","value":false}]"#;
    let r = set(prot, false);
    assert_eq!(r["ok"], false, "{r}");
    assert!(load().control.pause_on_user_input);
    assert_eq!(set(prot, true)["ok"], true);
    assert!(!load().control.pause_on_user_input);

    // A reset removes the key; the default is back.
    let r = json_of(&post(
        host,
        token,
        "reset",
        r#"{"confirmed":false,"keys":["screenshot.max_dimension","overlay.cursor_style"]}"#,
    ));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(load().screenshot.max_dimension, 1280);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("max_dimension"), "{text}");
    let r = json_of(&post(
        host,
        token,
        "reset",
        r#"{"confirmed":false,"keys":["control.pause_on_user_input"]}"#,
    ));
    assert_eq!(r["ok"], false);

    // The theme comes back with a panel change.
    let r = set(r#"[{"key":"panel.theme","value":"dark"}]"#, false);
    assert_eq!(r["theme"], "dark");
    assert!(get(host, token, "").contains("data-theme=\"dark\""));

    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn secrets_are_write_only() {
    let dir = temp("secret");
    let path = config_with_port(&dir, "");
    let up = start(Some(path.clone()), &dir);
    let (host, token) = (&up.host, &up.token);

    let r = json_of(&post(
        host,
        token,
        "set",
        r#"{"confirmed":true,"changes":[{"key":"server.http_token","value":"123456789-secret"}]}"#,
    ));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["values"]["server.http_token"], "••••cret");
    assert!(!r.to_string().contains("123456789"));
    assert_eq!(
        ConfigStore::load(Some(&path))
            .unwrap()
            .config
            .server
            .http_token,
        "123456789-secret"
    );
    let state = post(host, token, "state", "{}");
    assert!(
        !state.contains("123456789-secret") && state.contains("••••cret"),
        "{state}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
    }

    // An empty box leaves the saved one alone.
    let r = json_of(&post(
        host,
        token,
        "set",
        r#"{"confirmed":true,"changes":[{"key":"server.http_token","value":""}]}"#,
    ));
    assert_eq!(r["ok"], true);
    assert_eq!(
        ConfigStore::load(Some(&path))
            .unwrap()
            .config
            .server
            .http_token,
        "123456789-secret"
    );

    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_decision_model_is_saved_here_and_never_echoed() {
    let dir = temp("decision");
    let path = config_with_port(
        &dir,
        "[decision]\nprovider = \"jev\"\napi_key = \"sk-saved-abcd1234\"\n",
    );
    let up = start(Some(path.clone()), &dir);
    let (host, token) = (&up.host, &up.token);
    let load = || ConfigStore::load(Some(&path)).unwrap().config.decision;

    let saved = post(
        host,
        token,
        "save",
        r#"{"provider":"openai","model":"m","api_key":"sk-new-key-5678"}"#,
    );
    assert!(
        saved.contains("\"ok\":true") && !saved.contains("sk-new-key"),
        "{saved}"
    );
    let d = load();
    assert_eq!(
        (d.provider.as_str(), d.model.as_str(), d.api_key.as_str()),
        ("openai", "m", "sk-new-key-5678")
    );

    // An empty key keeps the saved one; another provider doesn't take it.
    post(
        host,
        token,
        "save",
        r#"{"provider":"openai","model":"m2","api_key":""}"#,
    );
    let d = load();
    assert_eq!(
        (d.model.as_str(), d.api_key.as_str()),
        ("m2", "sk-new-key-5678")
    );
    post(host, token, "save", r#"{"provider":"jev","api_key":""}"#);
    let d = load();
    assert_eq!((d.provider.as_str(), d.api_key.as_str()), ("jev", ""));

    // A bad address, a bad kind of model.
    let r = post(
        host,
        token,
        "save",
        r#"{"provider":"jev","base_url":"ftp://x"}"#,
    );
    assert!(r.contains("\"ok\":false"), "{r}");
    let r = post(host, token, "save", r#"{"provider":"other"}"#);
    assert!(r.contains("\"ok\":false"), "{r}");

    let removed = post(host, token, "remove", "{}");
    assert!(removed.contains("\"ok\":true"), "{removed}");
    assert_eq!(load(), DecisionConfig::default());

    // Close.
    post(host, token, "close", "{}");
    let deadline = Instant::now() + Duration::from_secs(3);
    while up.alive.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!up.alive.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn memory_only_servers_show_but_do_not_save() {
    let dir = temp("memory");
    let up = start(None, &dir);
    let (host, token) = (&up.host, &up.token);
    let state = json_of(&post(host, token, "state", "{}"));
    assert_eq!(state["ok"], true);
    assert!(state["path"].is_null());
    let r = json_of(&post(
        host,
        token,
        "set",
        r#"{"changes":[{"key":"clipboard","value":false}]}"#,
    ));
    assert_eq!(r["ok"], false);
    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_silent_connection_doesnt_hold_up_the_page() {
    let dir = temp("silent");
    let up = start(None, &dir);
    // A browser's spare connection: open, and nothing sent on it.
    let _spare = TcpStream::connect(&up.host).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    let page = get(&up.host, &up.token, "");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_address_stays_the_same_and_one_panel_serves_every_process() {
    let dir = temp("same");
    let path = config_with_port(&dir, "");
    let port = panel_settings(Some(&path)).port;

    let first = start(Some(path.clone()), &dir);
    assert_eq!(first.host, format!("127.0.0.1:{port}"));
    assert_eq!(first.token, persistent_token(&dir, port));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(token_path(&dir, port))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "token readable by others: {mode:o}");
    }

    // A second process (the same home) is pointed at the first.
    match Server::bind(Some(path.clone()), &dir).unwrap() {
        Bound::Elsewhere(url) => assert_eq!(url, format!("http://{}/{}/", first.host, first.token)),
        Bound::Mine(_) => panic!("took a second panel"),
    }

    // Once it is gone, the same address comes back.
    first.alive.store(false, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        std::thread::sleep(Duration::from_millis(50));
        if TcpStream::connect(&first.host).is_err() || Instant::now() > deadline {
            break;
        }
    }
    let again = start(Some(path.clone()), &dir);
    assert_eq!((&again.host, &again.token), (&first.host, &first.token));
    again.alive.store(false, Ordering::SeqCst);

    // Another program holds the port: a free one, and a token of its own.
    let held = TcpListener::bind(("127.0.0.1", port));
    if held.is_ok() {
        let other = start(Some(path), &dir);
        assert_ne!(other.host, format!("127.0.0.1:{port}"));
        assert_ne!(other.token, first.token);
        other.alive.store(false, Ordering::SeqCst);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_token_file_that_is_wrong_is_replaced() {
    let dir = temp("token");
    std::fs::write(token_path(&dir, 4000), "not a token").unwrap();
    let t = persistent_token(&dir, 4000);
    assert_eq!(t.len(), 32);
    assert_eq!(persistent_token(&dir, 4000), t);
    assert_ne!(persistent_token(&dir, 4001), t);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tokens_differ() {
    let (a, b) = (token(), token());
    assert_eq!(a.len(), 32);
    assert_ne!(a, b);
}

#[test]
fn the_panel_window_is_recognised_by_its_title() {
    assert!(is_panel_window("Zero panel [private]"));
    assert!(is_panel_window("Zero panel [private] - Google Chrome"));
    assert!(is_panel_window("zero PANEL [Private] — Mozilla Firefox"));
    assert!(!is_panel_window("Zero Use Computer - GitHub"));
    assert!(!is_panel_window("Control panel"));
    // The page really carries the mark.
    assert!(PAGE.contains("<title>Zero panel [private]</title>"));
}

#[test]
fn the_panel_is_english_only() {
    let arabic_script = |c: char| matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF);
    for (name, text) in [
        ("index.html", include_str!("index.html")),
        ("schema.rs", include_str!("schema.rs")),
        ("mod.rs", include_str!("mod.rs")),
        ("tests.rs", include_str!("tests.rs")),
    ] {
        if let Some(c) = text.chars().find(|&c| arabic_script(c)) {
            panic!("panel/{name} has text in another script: {c}");
        }
    }
}
