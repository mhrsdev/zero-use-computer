use super::*;
use serde_json::json;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cu-connect-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn env_in(root: &Path, os: Os) -> Env {
    Env {
        home: root.join("home"),
        appdata: Some(root.join("appdata")),
        os,
        path: vec![root.join("bin")],
        codex_home: None,
        config_home: None,
        claude_dir: None,
        zero_home: root.join("home/.computer-use"),
    }
}

fn state(env: &Env, c: Client, program: &Path) -> State {
    info(env, c, program).state
}

#[test]
fn the_files_are_where_each_client_keeps_them() {
    let root = temp("where");
    let (mac, win, lin) = (
        env_in(&root, Os::Mac),
        env_in(&root, Os::Windows),
        env_in(&root, Os::Linux),
    );
    assert!(
        mac.config_file(Client::ClaudeDesktop)
            .unwrap()
            .ends_with("Library/Application Support/Claude/claude_desktop_config.json")
    );
    assert!(
        win.config_file(Client::ClaudeDesktop)
            .unwrap()
            .starts_with(root.join("appdata"))
    );
    assert!(lin.config_file(Client::ClaudeDesktop).is_none());
    assert!(
        lin.config_file(Client::VsCode)
            .unwrap()
            .ends_with(".config/Code/User/mcp.json")
    );
    assert!(
        mac.config_file(Client::VsCode)
            .unwrap()
            .ends_with("Library/Application Support/Code/User/mcp.json")
    );
    assert!(
        lin.config_file(Client::Cursor)
            .unwrap()
            .ends_with(".cursor/mcp.json")
    );
    assert!(
        lin.config_file(Client::Codex)
            .unwrap()
            .ends_with(".codex/config.toml")
    );
    let mut custom = env_in(&root, Os::Linux);
    custom.codex_home = Some(root.join("elsewhere"));
    assert_eq!(
        custom.config_file(Client::Codex).unwrap(),
        root.join("elsewhere/config.toml")
    );
    assert!(lin.config_file(Client::ClaudeCode).is_none());
    assert_eq!(Client::parse("Claude-Code"), Some(Client::ClaudeCode));
    assert_eq!(Client::parse("vscode"), Some(Client::VsCode));
    assert_eq!(Client::parse("emacs"), None);
}

#[test]
fn a_json_client_is_added_to_without_disturbing_the_rest() {
    let root = temp("json");
    let env = env_in(&root, Os::Linux);
    let program = Path::new("/opt/zero/computer-use-mcp");
    let file = env.config_file(Client::Cursor).unwrap();

    // The client is there, the file isn't.
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    assert_eq!(state(&env, Client::Cursor, program), State::NotInstalled);
    install(&env, Client::Cursor, program).unwrap();
    assert_eq!(state(&env, Client::Cursor, program), State::Installed);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        v["mcpServers"]["computer-use"]["command"],
        "/opt/zero/computer-use-mcp"
    );
    assert_eq!(v["mcpServers"]["computer-use"]["args"], json!(["serve"]));

    // A file with other servers and settings, in their own order.
    let mine = "{\n  \"zeta\": 1,\n  \"mcpServers\": {\n    \"other\": {\"command\": \"x\", \"args\": [\"b\", \"a\"], \"env\": {\"Z\": \"1\", \"A\": \"2\"}}\n  },\n  \"alpha\": [3, 2, 1]\n}\n";
    std::fs::write(&file, mine).unwrap();
    assert_eq!(state(&env, Client::Cursor, program), State::NotInstalled);
    install(&env, Client::Cursor, program).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.find("zeta").unwrap() < text.find("mcpServers").unwrap());
    assert!(text.find("mcpServers").unwrap() < text.find("alpha").unwrap());
    assert!(
        text.contains(
            "{\"command\": \"x\", \"args\": [\"b\", \"a\"], \"env\": {\"Z\": \"1\", \"A\": \"2\"}}"
        ),
        "{text}"
    );
    assert!(text.find("\"other\"").unwrap() < text.find("computer-use").unwrap());
    assert_eq!(
        std::fs::read_to_string(format!("{}.bak", file.display())).unwrap(),
        mine
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["alpha"], json!([3, 2, 1]));
    assert_eq!(v["mcpServers"]["other"]["command"], "x");

    // Again: replaced, not doubled. Another path: noticed.
    install(&env, Client::Cursor, program).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file)
            .unwrap()
            .matches("computer-use\"")
            .count(),
        1
    );
    assert_eq!(
        state(
            &env,
            Client::Cursor,
            Path::new("/elsewhere/computer-use-mcp")
        ),
        State::Different("/opt/zero/computer-use-mcp".into())
    );
    install(
        &env,
        Client::Cursor,
        Path::new("/elsewhere/computer-use-mcp"),
    )
    .unwrap();
    assert_eq!(
        state(
            &env,
            Client::Cursor,
            Path::new("/elsewhere/computer-use-mcp")
        ),
        State::Installed
    );

    // Taken out: the rest stays.
    remove(&env, Client::Cursor).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(v["mcpServers"].get("computer-use").is_none());
    assert_eq!(v["mcpServers"]["other"]["command"], "x");
    assert_eq!(state(&env, Client::Cursor, program), State::NotInstalled);
    assert!(remove(&env, Client::Cursor).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn vs_code_uses_servers_and_a_type() {
    let root = temp("vscode");
    let env = env_in(&root, Os::Linux);
    let file = env.config_file(Client::VsCode).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    install(&env, Client::VsCode, Path::new("/p/computer-use-mcp")).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(v["servers"]["computer-use"]["type"], "stdio");
    assert!(v.get("mcpServers").is_none());
    assert_eq!(
        state(&env, Client::VsCode, Path::new("/p/computer-use-mcp")),
        State::Installed
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_file_that_isnt_plain_json_is_left_alone() {
    let root = temp("comments");
    let env = env_in(&root, Os::Linux);
    let file = env.config_file(Client::VsCode).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let text = "{\n  // my servers\n  \"servers\": {}\n}\n";
    std::fs::write(&file, text).unwrap();
    assert!(matches!(
        state(&env, Client::VsCode, Path::new("/p/x")),
        State::Unreadable(_)
    ));
    let err = install(&env, Client::VsCode, Path::new("/p/x"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("plain JSON"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    assert!(!PathBuf::from(format!("{}.bak", file.display())).exists());
    // Not an object where an object belongs.
    std::fs::write(&file, "{\"servers\": [1]}").unwrap();
    assert!(install(&env, Client::VsCode, Path::new("/p/x")).is_err());
    std::fs::write(&file, "[1]").unwrap();
    assert!(install(&env, Client::VsCode, Path::new("/p/x")).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn codex_keeps_its_comments() {
    let root = temp("codex");
    let env = env_in(&root, Os::Linux);
    let file = env.config_file(Client::Codex).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mine = "# my codex\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\" # keep\n";
    std::fs::write(&file, mine).unwrap();
    assert_eq!(
        state(&env, Client::Codex, Path::new("/p/computer-use-mcp")),
        State::NotInstalled
    );
    install(&env, Client::Codex, Path::new("/p/computer-use-mcp")).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("# my codex") && text.contains("# keep") && text.contains("model = \"o3\""),
        "{text}"
    );
    assert!(
        text.contains("[mcp_servers.computer-use]")
            && text.contains("command = \"/p/computer-use-mcp\""),
        "{text}"
    );
    assert_eq!(
        state(&env, Client::Codex, Path::new("/p/computer-use-mcp")),
        State::Installed
    );
    install(&env, Client::Codex, Path::new("/p/computer-use-mcp")).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file)
            .unwrap()
            .matches("[mcp_servers.computer-use]")
            .count(),
        1
    );
    remove(&env, Client::Codex).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        !text.contains("computer-use")
            && text.contains("[mcp_servers.other]")
            && text.contains("# keep"),
        "{text}"
    );
    // A file that isn't TOML is left alone.
    std::fs::write(&file, "model = = 3").unwrap();
    assert!(matches!(
        state(&env, Client::Codex, Path::new("/p/x")),
        State::Unreadable(_)
    ));
    assert!(install(&env, Client::Codex, Path::new("/p/x")).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "model = = 3");
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn claude_code_is_added_through_its_own_command() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = temp("claude");
    let env = env_in(&root, Os::Linux);
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir_all(&env.home).unwrap();
    // Not there: not found, and no way to add.
    assert_eq!(
        state(&env, Client::ClaudeCode, Path::new("/p/x")),
        State::NotFound
    );
    assert!(install(&env, Client::ClaudeCode, Path::new("/p/x")).is_err());
    // A stand-in that writes down what it was asked.
    let log = root.join("calls.log");
    let fake = root.join("bin/claude");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        state(&env, Client::ClaudeCode, Path::new("/p/x")),
        State::NotInstalled
    );
    install(&env, Client::ClaudeCode, Path::new("/p/computer-use-mcp")).unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("mcp remove computer-use --scope user"),
        "{calls}"
    );
    assert!(
        calls.contains("mcp add --scope user computer-use -- /p/computer-use-mcp serve"),
        "{calls}"
    );
    remove(&env, Client::ClaudeCode).unwrap();
    // What it keeps is read from its own file.
    std::fs::write(
        env.home.join(".claude.json"),
        r#"{"numStartups": 3, "mcpServers": {"computer-use": {"type": "stdio", "command": "/p/computer-use-mcp", "args": ["serve"]}}}"#,
    )
    .unwrap();
    assert_eq!(
        state(&env, Client::ClaudeCode, Path::new("/p/computer-use-mcp")),
        State::Installed
    );
    assert_eq!(
        state(&env, Client::ClaudeCode, Path::new("/q/x")),
        State::Different("/p/computer-use-mcp".into())
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn the_program_is_kept_where_it_stays() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = temp("stable");
    let env = env_in(&root, Os::Linux);
    let exe = root.join("Downloads/computer-use-mcp");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "#!/bin/sh\necho hi\n").unwrap();
    assert!(unstable_place(&exe));
    let (kept, note) = program_for_clients(&env, &exe);
    assert_eq!(kept, stable_program_path(&env));
    assert!(note.unwrap().contains("copy"));
    assert_eq!(
        std::fs::read_to_string(&kept).unwrap(),
        "#!/bin/sh\necho hi\n"
    );
    assert_eq!(
        std::fs::metadata(&kept).unwrap().permissions().mode() & 0o111,
        0o111
    );
    // Already there: itself, no copy.
    let (again, note) = program_for_clients(&env, &kept);
    assert_eq!((again, note), (kept.clone(), None));
    assert!(!unstable_place(Path::new(
        "/home/me/.computer-use/bin/computer-use-mcp"
    )));
    assert!(unstable_place(Path::new(
        "C:\\Users\\me\\Downloads\\computer-use-mcp.exe"
    )));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_entry_is_shown_as_it_will_be_written() {
    let p = Path::new("/p/computer-use-mcp");
    assert!(entry_text(Client::Cursor, p).contains("\"mcpServers\""));
    assert!(
        entry_text(Client::VsCode, p).contains("\"servers\"")
            && entry_text(Client::VsCode, p).contains("\"stdio\"")
    );
    assert!(entry_text(Client::Codex, p).contains("[mcp_servers.computer-use]"));
    assert!(
        entry_text(Client::ClaudeCode, p)
            .starts_with("claude mcp add --scope user computer-use --")
    );
    // A path with a backslash (Windows) survives as TOML and JSON.
    let w = Path::new("C:\\Tools\\computer-use-mcp.exe");
    let t: toml::Table = entry_text(Client::Codex, w).parse().unwrap();
    assert_eq!(
        t["mcp_servers"]["computer-use"]["command"]
            .as_str()
            .unwrap(),
        "C:\\Tools\\computer-use-mcp.exe"
    );
    let j: Value = serde_json::from_str(&entry_text(Client::ClaudeDesktop, w)).unwrap();
    assert_eq!(
        j["mcpServers"]["computer-use"]["command"],
        "C:\\Tools\\computer-use-mcp.exe"
    );
}

#[cfg(unix)]
#[test]
fn claude_code_gets_the_other_name_when_it_keeps_this_one() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = temp("reserved");
    let env = env_in(&root, Os::Linux);
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let log = root.join("calls.log");
    let fake = root.join("bin/claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho \"$@\" >> {}\ncase \"$*\" in *\"add --scope user computer-use \"*) echo 'Cannot add MCP server \"computer-use\": this name is reserved.' >&2; exit 1;; esac\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let said = install(&env, Client::ClaudeCode, Path::new("/p/computer-use-mcp")).unwrap();
    assert!(said.contains("zero-use-computer"), "{said}");
    let calls = std::fs::read_to_string(&log).unwrap();
    // Both old names are cleared first, then the other name is used.
    assert!(
        calls.contains("mcp remove computer-use --scope user"),
        "{calls}"
    );
    assert!(
        calls.contains("mcp remove zero-use-computer --scope user"),
        "{calls}"
    );
    assert!(
        calls.contains("mcp add --scope user zero-use-computer -- /p/computer-use-mcp serve"),
        "{calls}"
    );
    // Another failure is reported, not hidden behind the other name.
    std::fs::write(&fake, "#!/bin/sh\necho 'no network' >&2\nexit 1\n").unwrap();
    let err = install(&env, Client::ClaudeCode, Path::new("/p/x"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("no network"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn what_the_user_added_to_the_entry_stays_and_odd_files_are_handled() {
    let root = temp("merge");
    let env = env_in(&root, Os::Linux);
    let file = env.config_file(Client::Cursor).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    // The user's env on our entry survives a reinstall.
    std::fs::write(&file, r#"{"mcpServers": {"computer-use": {"command": "/old", "args": ["serve", "--log", "debug"], "env": {"COMPUTER_USE_HOME": "/z"}}}}"#).unwrap();
    install(&env, Client::Cursor, Path::new("/new/computer-use-mcp")).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let e = &v["mcpServers"]["computer-use"];
    assert_eq!(e["command"], "/new/computer-use-mcp");
    assert_eq!(e["args"], json!(["serve"]));
    assert_eq!(e["env"]["COMPUTER_USE_HOME"], "/z");
    // `null` where the servers go is an empty list.
    std::fs::write(&file, r#"{"mcpServers": null}"#).unwrap();
    install(&env, Client::Cursor, Path::new("/p")).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["computer-use"]["command"], "/p");
    // A member named twice is refused, the file untouched.
    let twice = r#"{"mcpServers": {}, "mcpServers": {"other": {}}}"#;
    std::fs::write(&file, twice).unwrap();
    let err = install(&env, Client::Cursor, Path::new("/p"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("twice"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), twice);
    // Taking out what isn't there changes nothing, not even a backup.
    let _ = std::fs::remove_file(format!("{}.bak", file.display()));
    let plain = r#"{"theme": "dark"}"#;
    std::fs::write(&file, plain).unwrap();
    assert!(
        remove(&env, Client::Cursor)
            .unwrap()
            .contains("didn't have it")
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), plain);
    assert!(!PathBuf::from(format!("{}.bak", file.display())).exists());
    // Codex: the user's own keys stay, and a new file has no empty header.
    let codex = env.config_file(Client::Codex).unwrap();
    std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
    std::fs::write(
        &codex,
        "[mcp_servers.computer-use]\ncommand = \"/old\"\nenv = { A = \"1\" }\n",
    )
    .unwrap();
    install(&env, Client::Codex, Path::new("/new")).unwrap();
    let t = std::fs::read_to_string(&codex).unwrap();
    assert!(
        t.contains("command = \"/new\"") && t.contains("env = { A = \"1\" }"),
        "{t}"
    );
    std::fs::remove_file(&codex).unwrap();
    install(&env, Client::Codex, Path::new("/new")).unwrap();
    let t = std::fs::read_to_string(&codex).unwrap();
    assert!(!t.contains("[mcp_servers]\n"), "{t}");
    assert!(t.contains("[mcp_servers.computer-use]"), "{t}");
    std::fs::write(&codex, "model = \"o3\"\n").unwrap();
    assert!(
        remove(&env, Client::Codex)
            .unwrap()
            .contains("didn't have it")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_places_people_move_their_settings_to_are_followed() {
    let root = temp("moved");
    let mut env = env_in(&root, Os::Linux);
    env.config_home = Some(root.join("xdg"));
    assert_eq!(
        env.config_file(Client::VsCode).unwrap(),
        root.join("xdg/Code/User/mcp.json")
    );
    env.claude_dir = Some(root.join("claude-home"));
    std::fs::create_dir_all(root.join("claude-home")).unwrap();
    std::fs::write(
        root.join("claude-home/.claude.json"),
        r#"{"mcpServers": {"computer-use": {"command": "/p/x"}}}"#,
    )
    .unwrap();
    assert_eq!(
        state(&env, Client::ClaudeCode, Path::new("/p/x")),
        State::Installed
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn a_newer_kept_copy_is_not_replaced_by_an_older_program() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = temp("newer");
    let env = env_in(&root, Os::Linux);
    let stable = stable_program_path(&env);
    std::fs::create_dir_all(stable.parent().unwrap()).unwrap();
    std::fs::write(&stable, "#!/bin/sh\necho computer-use-mcp 99.0.0\n").unwrap();
    std::fs::set_permissions(&stable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let exe = root.join("Downloads/computer-use-mcp");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "older").unwrap();
    let (given, note) = program_for_clients(&env, &exe);
    assert_eq!(given, stable);
    assert!(note.unwrap().contains("99.0.0"));
    assert!(std::fs::read_to_string(&stable).unwrap().contains("99.0.0"));
    let _ = std::fs::remove_dir_all(&root);
}
