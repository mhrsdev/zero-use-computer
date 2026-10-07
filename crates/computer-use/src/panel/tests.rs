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

fn schema_of(host: &str, token: &str) -> Value {
    json_of(&get(host, token, "schema.json"))
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

    // Every setting is on the page, with its default; the saved values
    // that differ from them are all the state carries.
    let schema = schema_of(host, token);
    assert_eq!(
        schema["entries"].as_array().unwrap().len(),
        crate::config::known_keys().len()
    );
    let max = schema["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "screenshot.max_dimension")
        .unwrap();
    assert_eq!(max["default"], 1280);
    let state = json_of(&post(host, token, "state", "{}"));
    // (the test's own port is the one thing set)
    assert_eq!(state["values"].as_object().unwrap().len(), 1, "{state}");

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

    let state = json_of(&post(host, token, "state", "{}"));
    assert_eq!(state["values"]["screenshot.max_dimension"], 1024);
    assert_eq!(state["values"]["overlay.cursor_style"], "jelly");

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
        r#"[{"key":"clipboard","value":null}]"#,
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
    assert_eq!(r["confirm"], json!(["control.pause_on_user_input"]));
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
        ("app.js", include_str!("app.js")),
        ("schema.rs", include_str!("schema.rs")),
        ("mod.rs", include_str!("mod.rs")),
        ("settings.rs", include_str!("settings.rs")),
        ("profiles.rs", include_str!("profiles.rs")),
        ("raw.rs", include_str!("raw.rs")),
        ("status.rs", include_str!("status.rs")),
        ("updates.rs", include_str!("updates.rs")),
        ("connecting.rs", include_str!("connecting.rs")),
        ("help.rs", include_str!("help.rs")),
        ("help/start.html", include_str!("help/start.html")),
        ("help/pointers.html", include_str!("help/pointers.html")),
        ("help/tokens.html", include_str!("help/tokens.html")),
        ("help/agents.html", include_str!("help/agents.html")),
        ("help/safety.html", include_str!("help/safety.html")),
        ("help/updates.html", include_str!("help/updates.html")),
        ("help/decision.html", include_str!("help/decision.html")),
        ("help/files.html", include_str!("help/files.html")),
        ("help/trouble.html", include_str!("help/trouble.html")),
        ("tests.rs", include_str!("tests.rs")),
    ] {
        if let Some(c) = text.chars().find(|&c| arabic_script(c)) {
            panic!("panel/{name} has text in another script: {c}");
        }
    }
}

#[test]
fn the_state_is_small_and_the_schema_is_sent_once() {
    let dir = temp("small");
    let path = config_with_port(&dir, "");
    let up = start(Some(path), &dir);
    let state = post(&up.host, &up.token, "state", "{}");
    let schema = get(&up.host, &up.token, "schema.json");
    assert!(state.len() < 2_000, "state is {} bytes", state.len());
    assert!(schema.len() > 20_000, "schema is {} bytes", schema.len());
    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn profiles_apply_undo_and_are_kept() {
    let dir = temp("profiles");
    let path = config_with_port(&dir, "");
    let up = start(Some(path.clone()), &dir);
    let (host, token) = (&up.host, &up.token);
    let load = || ConfigStore::load(Some(&path)).unwrap().config;
    let call = |route: &str, body: &str| json_of(&post(host, token, route, body));

    let list = call("profiles", "{}");
    let ids: Vec<&str> = list["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["balanced", "low-tokens", "best-quality", "showcase"]);

    let r = call("profile_apply", r#"{"id":"low-tokens"}"#);
    assert_eq!(r["ok"], true, "{r}");
    let c = load();
    assert_eq!((c.screenshot.max_dimension, c.tree.max_nodes), (1024, 600));
    // Another profile leaves nothing of the first one behind.
    assert_eq!(
        call("profile_apply", r#"{"id":"best-quality"}"#)["ok"],
        true
    );
    let c = load();
    assert_eq!((c.screenshot.max_dimension, c.tree.max_nodes), (2048, 2400));
    assert_eq!(c.screenshot.overview_max_dimension, 0);
    assert_eq!(call("profile_apply", r#"{"id":"balanced"}"#)["ok"], true);
    let c = load();
    assert_eq!((c.screenshot.max_dimension, c.tree.max_nodes), (1280, 1200));
    assert_eq!(call("profile_apply", r#"{"id":"nothing"}"#)["ok"], false);

    // The user's own: what they changed, without protected settings.
    call(
        "set",
        r#"{"changes":[{"key":"overlay.cursor_style","value":"ice"},{"key":"tree.indent","value":3}]}"#,
    );
    call(
        "set",
        r#"{"confirmed":true,"changes":[{"key":"control.max_pause_secs","value":30}]}"#,
    );
    let r = call("profile_save", r#"{"label":"My setup"}"#);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["id"], "my-setup");
    assert_eq!(r["settings"], 2);
    assert!(r["left_out"].as_u64().unwrap() >= 2, "{r}"); // max_pause_secs and the port
    assert_eq!(call("profile_save", r#"{"label":"Balanced"}"#)["ok"], false);
    assert_eq!(call("profile_save", r#"{"label":"  "}"#)["ok"], false);
    call(
        "reset",
        r#"{"keys":["overlay.cursor_style","tree.indent"]}"#,
    );
    assert_eq!(load().tree.indent, 1);
    let list = call("profiles", "{}");
    let mine = list["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "my-setup")
        .unwrap();
    assert_eq!(mine["differs"], 2);
    assert_eq!(call("profile_apply", r#"{"id":"my-setup"}"#)["ok"], true);
    assert_eq!(load().overlay.cursor_style, "ice");
    assert_eq!(load().tree.indent, 3);
    assert_eq!(call("profile_delete", r#"{"id":"my-setup"}"#)["ok"], true);
    assert_eq!(call("profile_apply", r#"{"id":"my-setup"}"#)["ok"], false);
    assert_eq!(call("profile_delete", r#"{"id":"../x"}"#)["ok"], false);

    // A file edited by hand to hold a protected setting still asks.
    std::fs::create_dir_all(dir.join("profiles")).unwrap();
    std::fs::write(
        dir.join("profiles/sneaky.json"),
        r#"{"label":"Sneaky","values":{"control.pause_on_user_input":false}}"#,
    )
    .unwrap();
    let r = call("profile_apply", r#"{"id":"sneaky"}"#);
    assert_eq!(r["ok"], false);
    assert_eq!(r["confirm"], json!(["control.pause_on_user_input"]));
    assert!(load().control.pause_on_user_input);

    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_settings_file_is_edited_as_text_through_the_page() {
    let dir = temp("raw");
    let path = config_with_port(&dir, "[decision]\napi_key = \"sk-hidden-0000\"\n");
    let up = start(Some(path.clone()), &dir);
    let (host, token) = (&up.host, &up.token);
    let call = |route: &str, body: Value| json_of(&post(host, token, route, &body.to_string()));

    let v = call("raw_get", json!({}));
    let text = v["text"].as_str().unwrap().to_string();
    assert!(!text.contains("sk-hidden"), "{text}");
    let edited = format!("{text}\n[tree]\nmax_nodes = 77\n");
    let r = call("raw_check", json!({"text": edited}));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["changed"], json!(["tree.max_nodes"]));
    assert_eq!(call("raw_save", json!({"text": edited}))["ok"], true);
    let c = ConfigStore::load(Some(&path)).unwrap().config;
    assert_eq!(
        (c.tree.max_nodes, c.decision.api_key.as_str()),
        (77, "sk-hidden-0000")
    );
    let r = call("raw_save", json!({"text": "tree = [1"}));
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().len() > 3);
    assert_eq!(
        ConfigStore::load(Some(&path))
            .unwrap()
            .config
            .tree
            .max_nodes,
        77
    );

    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_go_out_and_come_back_in_as_toml() {
    let dir = temp("export");
    let path = config_with_port(&dir, "");
    let up = start(Some(path.clone()), &dir);
    let (host, token) = (&up.host, &up.token);
    let call = |route: &str, body: Value| json_of(&post(host, token, route, &body.to_string()));

    call(
        "set",
        json!({"changes":[{"key":"overlay.cursor_style","value":"orbit"},{"key":"tools.disabled","value":["drag"]}]}),
    );
    call(
        "set",
        json!({"confirmed":true,"changes":[{"key":"server.http_token","value":"secret-token-0001"}]}),
    );
    let out = call("export", json!({}));
    let text = out["text"].as_str().unwrap();
    assert!(
        text.contains("cursor_style = \"orbit\"") && text.contains("disabled"),
        "{text}"
    );
    assert!(
        !text.contains("secret-token") && !text.contains("http_token"),
        "{text}"
    );
    assert_eq!(out["skipped_secrets"], 1);

    // Into another file, through its own panel.
    let dir2 = temp("import");
    let path2 = config_with_port(&dir2, "");
    let up2 = start(Some(path2.clone()), &dir2);
    let r = json_of(&post(
        &up2.host,
        &up2.token,
        "import",
        &json!({"text": text}).to_string(),
    ));
    assert_eq!(r["ok"], true, "{r}");
    let c = ConfigStore::load(Some(&path2)).unwrap().config;
    assert_eq!(c.overlay.cursor_style, "orbit");
    assert_eq!(c.tools.disabled, vec!["drag".to_string()]);
    // The port in it came along too (it is a setting like another): fine,
    // but a secret in a file someone hands over is refused.
    let r = json_of(&post(
        &up2.host,
        &up2.token,
        "import",
        &json!({"text": "[decision]\napi_key = \"x\"\n"}).to_string(),
    ));
    assert_eq!(r["ok"], false);
    let r = json_of(&post(
        &up2.host,
        &up2.token,
        "import",
        &json!({"text": "[control]\nstop_hotkey = \"\"\n"}).to_string(),
    ));
    assert_eq!(r["ok"], false);
    assert_eq!(r["confirm"], json!(["control.stop_hotkey"]));
    assert_eq!(
        ConfigStore::load(Some(&path2))
            .unwrap()
            .config
            .control
            .stop_hotkey,
        "ctrl+alt+escape"
    );
    let r = json_of(&post(
        &up2.host,
        &up2.token,
        "import",
        &json!({"text": "nonsense = 1"}).to_string(),
    ));
    assert_eq!(r["ok"], false);

    for u in [&up, &up2] {
        u.alive.store(false, Ordering::SeqCst);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[test]
fn the_reports_answer() {
    let dir = temp("reports");
    let path = config_with_port(&dir, "");
    let up = start(Some(path), &dir);
    let (host, token) = (&up.host, &up.token);
    for (route, key) in [
        ("overview", "version"),
        ("tools", "tools"),
        ("apps", "apps"),
        ("audit", "lines"),
        ("path", "paths"),
    ] {
        let r = json_of(&post(host, token, route, "{}"));
        assert_eq!(r["ok"], true, "{route}: {r}");
        assert!(r.get(key).is_some(), "{route}: {r}");
    }
    let o = json_of(&post(host, token, "overview", "{}"));
    assert_eq!(
        o["stop_key"],
        crate::overlay::helper::pretty_key("ctrl+alt+escape")
    );
    assert!(post(host, token, "nothing", "{}").starts_with("HTTP/1.1 404"));
    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_updates_card_reports_what_is_known() {
    let dir = temp("upd-status");
    let path = config_with_port(
        &dir,
        "[update]\ncheck_every_mins = 10\nchannel = \"prerelease\"\nskip_version = \"9.9.9\"\n",
    );
    let up = start(Some(path), &dir);
    let (host, token) = (&up.host, &up.token);
    let s = json_of(&post(host, token, "update_status", "{}"));
    assert_eq!(s["ok"], true, "{s}");
    assert_eq!(s["every_secs"], 600);
    assert_eq!(s["channel"], "prerelease");
    assert_eq!(s["skip_version"], "9.9.9");
    assert!(s["pending"].is_null() && s["previous"].is_null());
    assert_eq!(s["current"], env!("CARGO_PKG_VERSION"));
    // Turned off: no next look.
    post(
        host,
        token,
        "set",
        r#"{"confirmed":true,"changes":[{"key":"update.enabled","value":false}]}"#,
    );
    let s = json_of(&post(host, token, "update_status", "{}"));
    assert!(s["next_in_secs"].is_null());
    // Nothing waits, nothing is kept.
    assert_eq!(
        json_of(&post(host, token, "update_install", "{}"))["ok"],
        false
    );
    assert_eq!(
        json_of(&post(host, token, "update_rollback", "{}"))["ok"],
        false
    );
    // A bad value is refused as a setting.
    let r = json_of(&post(
        host,
        token,
        "set",
        r#"{"confirmed":true,"changes":[{"key":"update.check_every_mins","value":2}]}"#,
    ));
    assert_eq!(r["ok"], false, "{r}");
    let r = json_of(&post(
        host,
        token,
        "set",
        r#"{"confirmed":true,"changes":[{"key":"update.pin","value":"latest"}]}"#,
    ));
    assert_eq!(r["ok"], false, "{r}");
    let r = json_of(&post(
        host,
        token,
        "set",
        r#"{"confirmed":true,"changes":[{"key":"update.channel","value":"nightly"}]}"#,
    ));
    assert_eq!(r["ok"], false, "{r}");
    up.alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn an_update_goes_in_and_comes_out_only_when_the_user_says_so() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = temp("upd-install");
    let path = config_with_port(&dir, "");
    let exe = dir.join("the-program");
    let script = |v: &str| format!("#!/bin/sh\necho computer-use-mcp {v}\n");
    let put = |p: &Path, v: &str| {
        std::fs::write(p, script(v)).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    put(&exe, env!("CARGO_PKG_VERSION"));
    // A downloaded update, as the updater leaves it.
    let updates = dir.join("updates");
    let waiting = updates.join("v99.0.0");
    std::fs::create_dir_all(&waiting).unwrap();
    put(&waiting.join(crate::update::BIN_NAME), "99.0.0");
    std::fs::write(
        updates.join("pending.json"),
        serde_json::to_vec(&crate::update::Pending {
            version: "99.0.0".into(),
            dir: waiting,
            downloaded: 1,
            sha256: String::new(),
        })
        .unwrap(),
    )
    .unwrap();
    let Bound::Mine(server) = Server::bind_with(
        Some(path.clone()),
        &dir,
        exe.clone(),
        crate::connect::Env::real(),
    )
    .unwrap() else {
        panic!("another panel answered");
    };
    let (host, token, alive) = (
        server.page.host.clone(),
        server.page.token.clone(),
        server.page.alive.clone(),
    );
    std::thread::spawn(move || server.run());
    let call = |route: &str, body: &str| json_of(&post(&host, &token, route, body));

    let s = call("update_status", "{}");
    assert_eq!(s["pending"]["version"], "99.0.0");
    // Without the user's yes, the program is left alone.
    let r = call("update_install", "{}");
    assert_eq!(r["ok"], false);
    assert!(
        r["confirm_text"].as_str().unwrap().contains("99.0.0"),
        "{r}"
    );
    assert!(
        std::fs::read_to_string(&exe)
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION"))
    );
    // With it, it goes in, and the old one is kept.
    let r = call("update_install", r#"{"confirmed":true}"#);
    assert_eq!(r["ok"], true, "{r}");
    assert!(std::fs::read_to_string(&exe).unwrap().contains("99.0.0"));
    let s = call("update_status", "{}");
    assert_eq!(s["previous"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(s["pending"].is_null());
    // Going back asks too, and doesn't take that update again.
    let r = call("update_rollback", "{}");
    assert_eq!(r["ok"], false);
    assert!(r["confirm_text"].as_str().is_some());
    assert!(std::fs::read_to_string(&exe).unwrap().contains("99.0.0"));
    let r = call("update_rollback", r#"{"confirmed":true}"#);
    assert_eq!(r["ok"], true, "{r}");
    assert!(
        std::fs::read_to_string(&exe)
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION"))
    );
    let c = ConfigStore::load(Some(&path)).unwrap().config;
    assert_eq!(c.update.skip_version, "99.0.0");
    assert!(call("update_status", "{}")["previous"].is_null());
    alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_agent_is_added_and_removed_from_the_connect_page_only_with_a_yes() {
    let dir = temp("connect");
    let path = config_with_port(&dir, "");
    let exe = dir.join("Downloads").join("computer-use-mcp");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "#!/bin/sh\necho hi\n").unwrap();
    let env = crate::connect::Env {
        home: dir.join("home"),
        appdata: None,
        os: crate::connect::Os::Linux,
        path: vec![],
        codex_home: None,
        zero_home: dir.join("home/.computer-use"),
    };
    std::fs::create_dir_all(env.home.join(".cursor")).unwrap();
    let cursor_file = env.home.join(".cursor/mcp.json");
    let Bound::Mine(server) =
        Server::bind_with(Some(path), &dir, exe.clone(), env.clone()).unwrap()
    else {
        panic!("another panel answered");
    };
    let (host, token, alive) = (
        server.page.host.clone(),
        server.page.token.clone(),
        server.page.alive.clone(),
    );
    std::thread::spawn(move || server.run());
    let call = |route: &str, body: Value| json_of(&post(&host, &token, route, &body.to_string()));
    let find = |l: &Value, id: &str| {
        l["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()
            .clone()
    };

    let l = call("connect_list", json!({}));
    assert_eq!(l["ok"], true, "{l}");
    assert_eq!(find(&l, "cursor")["state"]["kind"], "not_installed");
    assert_eq!(find(&l, "claude-code")["state"]["kind"], "not_found");
    assert_eq!(l["unstable"], true);
    assert_eq!(l["will_copy"], true);
    assert!(
        find(&l, "cursor")["entry"]
            .as_str()
            .unwrap()
            .contains("mcpServers")
    );

    // No yes, no change; the question names the file and the entry.
    let r = call(
        "connect_do",
        json!({"client": "cursor", "action": "install"}),
    );
    assert_eq!(r["ok"], false);
    let q = r["confirm_text"].as_str().unwrap();
    assert!(
        q.contains("mcp.json") && q.contains("computer-use") && q.contains("copy of this program"),
        "{q}"
    );
    assert!(!cursor_file.exists());
    // Yes: it is in, and the program is kept where it stays.
    let r = call(
        "connect_do",
        json!({"client": "cursor", "action": "install", "confirmed": true}),
    );
    assert_eq!(r["ok"], true, "{r}");
    assert!(r["note"].as_str().unwrap().contains("copy"), "{r}");
    let text = std::fs::read_to_string(&cursor_file).unwrap();
    let stable = crate::connect::stable_program_path(&env);
    assert!(text.contains(&stable.display().to_string()), "{text}");
    assert!(stable.is_file());
    assert_eq!(
        find(&call("connect_list", json!({})), "cursor")["state"]["kind"],
        "installed"
    );
    // Taking it out asks too.
    let r = call(
        "connect_do",
        json!({"client": "cursor", "action": "remove"}),
    );
    assert_eq!(r["ok"], false);
    assert!(
        cursor_file.exists()
            && std::fs::read_to_string(&cursor_file)
                .unwrap()
                .contains("computer-use")
    );
    assert_eq!(
        call(
            "connect_do",
            json!({"client": "cursor", "action": "remove", "confirmed": true})
        )["ok"],
        true
    );
    assert_eq!(
        find(&call("connect_list", json!({})), "cursor")["state"]["kind"],
        "not_installed"
    );
    // Not an agent, no action, one that isn't here, a settings file we won't touch.
    assert_eq!(
        call(
            "connect_do",
            json!({"client": "emacs", "action": "install", "confirmed": true})
        )["ok"],
        false
    );
    assert_eq!(
        call(
            "connect_do",
            json!({"client": "cursor", "action": "frobnicate", "confirmed": true})
        )["ok"],
        false
    );
    assert_eq!(
        call(
            "connect_do",
            json!({"client": "claude-code", "action": "install", "confirmed": true})
        )["ok"],
        false
    );
    std::fs::write(&cursor_file, "{ // mine\n}").unwrap();
    let r = call(
        "connect_do",
        json!({"client": "cursor", "action": "install", "confirmed": true}),
    );
    assert_eq!(r["ok"], false);
    assert_eq!(
        std::fs::read_to_string(&cursor_file).unwrap(),
        "{ // mine\n}"
    );
    alive.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}
