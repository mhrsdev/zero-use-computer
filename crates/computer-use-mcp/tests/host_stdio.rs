//! The program as a host starts it (`--host zero`), over its real stdio and
//! on the scripted in-memory desktop (`--backend mock`): nothing on the
//! computer running the test is looked at or touched, and nothing listens
//! on a port.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Session {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Session {
    fn start(home: &std::path::Path) -> Self {
        Self::start_with(home, true, &[])
    }

    /// With or without a host, and with these variables of the program's environment.
    fn start_with(home: &std::path::Path, host: bool, env: &[(&str, &std::path::Path)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_computer-use-mcp"));
        if host {
            command.args(["--host", "zero"]);
        }
        let mut child = command
            .args(["--backend", "mock", "serve"])
            .env("COMPUTER_USE_HOME", home)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the program starts");
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child,
            input,
            output,
            next: 1,
        };
        let hello = session.call(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
        );
        assert_eq!(hello["result"]["serverInfo"]["name"], "computer-use");
        session
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.input, "{line}").unwrap();
        self.input.flush().unwrap();
        loop {
            let mut text = String::new();
            assert!(
                self.output.read_line(&mut text).unwrap() > 0,
                "the program ended"
            );
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn home(name: &str, managed: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cu-stdio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("managed.toml"), managed).unwrap();
    dir
}

#[test]
fn the_settings_are_read_and_written_over_the_programs_own_stdio() {
    let dir = home("rw", "[host]\nuser_keys = [\"mouse_speed\"]\n");
    let mut s = Session::start(&dir);

    let schema = s.call("computer_use/settings/schema", json!({}));
    assert_eq!(schema["result"]["schemaVersion"], 1);
    assert!(schema["result"]["entries"].as_array().unwrap().len() > 150);

    let set = s.call(
        "computer_use/settings/set",
        json!({"key": "mouse_speed", "value": 1.5}),
    );
    assert_eq!(set["result"]["value"], 1.5, "{set}");
    let values = s.call("computer_use/settings/values", json!({}));
    assert_eq!(values["result"]["values"]["mouse_speed"], 1.5);
    assert!(
        std::fs::read_to_string(dir.join("user.toml"))
            .unwrap()
            .contains("mouse_speed = 1.5")
    );
}

#[test]
fn a_locked_setting_is_refused_with_a_code_and_the_file_stays_as_it_was() {
    let dir = home("locked", "[host]\nuser_keys = [\"mouse_speed\"]\n");
    let mut s = Session::start(&dir);
    let refused = s.call(
        "computer_use/settings/set",
        json!({"key": "clipboard", "value": true}),
    );
    assert_eq!(refused["error"]["code"], -32010);
    assert_eq!(refused["error"]["data"]["code"], "managedByHost");
    assert_eq!(refused["error"]["data"]["key"], "clipboard");
    assert!(!dir.join("user.toml").exists());
}

#[test]
fn a_change_reaches_the_running_server_without_a_restart() {
    // `text_only` is the host's; `mouse_speed` the user's. After the user's change the running
    // engine has the new value: a mock desktop's tool list shows what the settings say.
    let dir = home("live", "[host]\nuser_keys = [\"tools.preset\"]\n");
    let mut s = Session::start(&dir);
    let before = s.call("tools/list", json!({}));
    let count = |v: &Value| v["result"]["tools"].as_array().unwrap().len();
    let before_count = count(&before);
    let set = s.call(
        "computer_use/settings/set",
        json!({"key": "tools.preset", "value": "small"}),
    );
    assert!(set.get("result").is_some(), "{set}");
    let after = s.call("tools/list", json!({}));
    assert_ne!(
        count(&after),
        before_count,
        "the tool list followed the setting"
    );
}

#[test]
fn the_settings_requests_are_unknown_without_a_host() {
    let dir = home("nohost", "");
    let mut child = Command::new(env!("CARGO_BIN_EXE_computer-use-mcp"))
        .args(["--backend", "mock", "serve"])
        .env("COMPUTER_USE_HOME", &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        input,
        "{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": "computer_use/settings/schema", "params": {}})
    )
    .unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(answer["error"]["code"], -32601);
    let _ = child.kill();
    let _ = child.wait();
}

/// A picture of one point (GIF), as the overlay reads it.
const GIF_1X1: &str = "R0lGODlhAQABAIAAAAUEBAAAACwAAAAAAQABAAACAkQBADs=";
const PICTURE_KEYS: &str =
    "[host]\nuser_keys = [\"overlay.cursor_image\", \"overlay.cursor_style\", \"mouse_speed\"]\n";

fn error_code(answer: &Value) -> &str {
    answer["error"]["data"]["code"].as_str().unwrap_or_default()
}

/// No folder of this computer is in anything the program said.
fn assert_names_no_folder(said: &[Value], dir: &std::path::Path) {
    let needle = dir.display().to_string();
    for answer in said {
        assert!(!answer.to_string().contains(&needle), "{answer}");
    }
}

#[test]
fn a_pointer_picture_is_checked_kept_shown_and_taken_away() {
    let dir = home("picture", PICTURE_KEYS);
    let mut s = Session::start(&dir);
    let mut said = Vec::new();
    let none = s.call("computer_use/settings/cursor", json!({}));
    assert_eq!(none["result"]["set"], false);
    assert_eq!(none["result"]["limits"]["fileMb"], 4);
    said.push(none);

    let bad = s.call(
        "computer_use/settings/cursor_set",
        json!({"data": "bm90IGEgcGljdHVyZQ=="}),
    );
    assert_eq!(error_code(&bad), "notAPicture", "{bad}");
    assert!(bad["error"]["data"]["detail"]["reason"].is_string());
    said.push(bad);
    let missing = s.call("computer_use/settings/cursor_set", json!({}));
    assert_eq!(error_code(&missing), "badParams", "{missing}");

    let set = s.call("computer_use/settings/cursor_set", json!({"data": GIF_1X1}));
    assert_eq!(set["result"], json!({"ok": true}), "{set}");
    said.push(set);
    let now = s.call("computer_use/settings/cursor", json!({}));
    for (key, want) in [
        ("set", json!(true)),
        ("kept", json!(true)),
        ("usable", json!(true)),
        ("format", json!("gif")),
        ("width", json!(1)),
        ("height", json!(1)),
        ("frames", json!(1)),
    ] {
        assert_eq!(now["result"][key], want, "{key}: {now}");
    }
    said.push(now);

    let cleared = s.call("computer_use/settings/cursor_clear", json!({}));
    assert_eq!(cleared["result"], json!({"ok": true}), "{cleared}");
    said.push(cleared);
    let gone = s.call("computer_use/settings/cursor", json!({}));
    assert_eq!(gone["result"]["set"], false);
    said.push(gone);
    assert_names_no_folder(&said, &dir);
}

#[test]
fn a_picture_cannot_be_set_when_the_host_did_not_give_the_pointer_to_the_user() {
    let dir = home("picture-locked", "[host]\nuser_keys = [\"mouse_speed\"]\n");
    let mut s = Session::start(&dir);
    let set = s.call("computer_use/settings/cursor_set", json!({"data": GIF_1X1}));
    assert_eq!(error_code(&set), "managedByHost", "{set}");
    let cleared = s.call("computer_use/settings/cursor_clear", json!({}));
    assert_eq!(error_code(&cleared), "managedByHost");
    assert!(!dir.join("cursors").exists() && !dir.join("user.toml").exists());
}

/// A player that only writes down that it was started. Unix only (a script).
#[cfg(unix)]
struct FakePlayer {
    bin: std::path::PathBuf,
    cache: std::path::PathBuf,
    log: std::path::PathBuf,
}

#[cfg(unix)]
impl FakePlayer {
    fn new(name: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("cu-stdio-player-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bin, cache, log) = (root.join("bin"), root.join("cache"), root.join("played"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        // The program looks for afplay on macOS and for pw-play, paplay or aplay elsewhere.
        let player = bin.join(Self::NAME);
        let script = format!("#!/bin/sh\necho started >> '{}'\n", log.display());
        std::fs::write(&player, script).unwrap();
        std::fs::set_permissions(&player, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { bin, cache, log }
    }

    const NAME: &str = if cfg!(target_os = "macos") {
        "afplay"
    } else {
        "paplay"
    };

    fn start(&self, dir: &std::path::Path) -> Session {
        // (The sounds it writes go to the cache folder, which is the test's own.)
        Session::start_with(
            dir,
            true,
            &[
                ("PATH", &self.bin),
                ("XDG_CACHE_HOME", &self.cache),
                ("HOME", &self.cache),
            ],
        )
    }

    /// How many times the player was started, after it had its time to.
    fn starts(&self) -> usize {
        std::thread::sleep(std::time::Duration::from_millis(700));
        std::fs::read_to_string(&self.log).map_or(0, |t| t.lines().count())
    }
}

#[cfg(unix)]
#[test]
fn a_sound_plays_only_when_asked_for_and_a_check_starts_no_player() {
    let player = FakePlayer::new("play");
    let dir = home("sound", "");
    let mut s = player.start(&dir);

    // A check says whether a sound could play, and starts nothing.
    let check = s.call(
        "computer_use/settings/sound_test",
        json!({"event": "done", "dryRun": true}),
    );
    assert_eq!(
        check["result"],
        json!({"ready": true, "player": FakePlayer::NAME}),
        "{check}"
    );
    // Nothing that is not a clear yes or no is taken for "play".
    for bad in [json!("true"), json!(0), json!(null)] {
        let unclear = s.call(
            "computer_use/settings/sound_test",
            json!({"event": "done", "dryRun": bad}),
        );
        assert_eq!(error_code(&unclear), "badParams", "{unclear}");
    }
    let unknown = s.call("computer_use/settings/sound_test", json!({"event": "boom"}));
    assert_eq!(error_code(&unknown), "badParams");
    assert_eq!(player.starts(), 0, "no player was started by a check");

    // An ask to play plays once; the same sound again at once is refused.
    let played = s.call("computer_use/settings/sound_test", json!({"event": "done"}));
    assert_eq!(
        played["result"],
        json!({"played": true, "player": FakePlayer::NAME}),
        "{played}"
    );
    let again = s.call(
        "computer_use/settings/sound_test",
        json!({"event": "done", "dryRun": false}),
    );
    assert_eq!(error_code(&again), "tooSoon", "{again}");
    assert_eq!(player.starts(), 1);
    assert_names_no_folder(&[check, played, again], &dir);
}

#[cfg(unix)]
#[test]
fn without_a_player_the_sound_test_says_so_for_a_check_and_for_a_play() {
    let player = FakePlayer::new("none");
    std::fs::remove_file(player.bin.join(FakePlayer::NAME)).unwrap();
    let dir = home("sound-none", "");
    let mut s = player.start(&dir);
    for dry in [true, false] {
        let answer = s.call(
            "computer_use/settings/sound_test",
            json!({"event": "click", "dryRun": dry}),
        );
        assert_eq!(error_code(&answer), "noPlayer", "dryRun {dry}: {answer}");
    }
}

#[test]
fn a_profile_lists_the_values_it_sets_and_only_the_users_own_can_be_kept_and_deleted() {
    let dir = home("profiles", "[host]\nuser_keys = [\"mouse_speed\"]\n");
    let mut s = Session::start(&dir);
    let list = s.call("computer_use/settings/profiles", json!({}));
    let profiles = list["result"]["profiles"].as_array().unwrap();
    assert!(profiles.len() <= 50 && profiles.iter().any(|p| p["builtin"] == true));
    for p in profiles {
        assert!(p["id"].as_str().is_some_and(|id| !id.is_empty()), "{p}");
        assert!(
            p["changes"].as_array().is_some_and(|c| c.len() <= 400),
            "{p}"
        );
    }
    // Nothing differs from the defaults yet, so nothing can be kept.
    let empty = s.call(
        "computer_use/settings/profile_save",
        json!({"label": "Mine"}),
    );
    assert_eq!(error_code(&empty), "saveFailed");
    for label in ["   ", "Balanced"] {
        let bad = s.call(
            "computer_use/settings/profile_save",
            json!({"label": label}),
        );
        assert_eq!(error_code(&bad), "badName", "{label}");
    }

    let set = s.call(
        "computer_use/settings/set",
        json!({"key": "mouse_speed", "value": 1.5}),
    );
    assert!(set.get("result").is_some(), "{set}");
    let saved = s.call(
        "computer_use/settings/profile_save",
        json!({"label": "My look"}),
    );
    assert_eq!(saved["result"], json!({"id": "my-look"}), "{saved}");
    let list = s.call("computer_use/settings/profiles", json!({}));
    let mine = list["result"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "my-look")
        .expect("kept");
    assert_eq!(mine["builtin"], false);
    assert_eq!(
        mine["changes"],
        json!([{"key": "mouse_speed", "value": 1.5}])
    );

    let refused = s.call(
        "computer_use/settings/profile_delete",
        json!({"id": "balanced"}),
    );
    assert_eq!(error_code(&refused), "notAllowed");
    let deleted = s.call(
        "computer_use/settings/profile_delete",
        json!({"id": "my-look"}),
    );
    assert_eq!(deleted["result"], json!({}), "{deleted}");
    let again = s.call(
        "computer_use/settings/profile_delete",
        json!({"id": "my-look"}),
    );
    assert_eq!(error_code(&again), "notAllowed");
}

#[test]
fn a_profile_file_written_by_hand_tells_the_host_no_place_and_changes_no_setting() {
    // Only a setting a profile may hold is told to the host, and listing it changes nothing.
    let dir = home("profiles-hand", "[host]\nuser_keys = [\"mouse_speed\"]\n");
    std::fs::create_dir_all(dir.join("profiles")).unwrap();
    std::fs::write(
        dir.join("profiles/hand.json"),
        json!({"label": "Hand", "values": {
            "mouse_speed": 2.0, "ocr.tesseract_path": "/tmp/a-program", "script.files": "all",
            "overlay.cursor_image": "/tmp/picture.png"}})
        .to_string(),
    )
    .unwrap();
    let mut s = Session::start(&dir);
    let list = s.call("computer_use/settings/profiles", json!({}));
    let hand = list["result"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "hand")
        .expect("listed");
    assert_eq!(
        hand["changes"],
        json!([{"key": "mouse_speed", "value": 2.0}])
    );
    assert_names_no_folder(&[list], &dir);
    assert!(
        !dir.join("user.toml").exists(),
        "listing changed no setting"
    );
}

#[test]
fn the_pointer_the_sound_and_the_profiles_are_a_hosts_requests_and_no_tool_of_the_agent() {
    let dir = home("hosts-only", PICTURE_KEYS);
    let methods = [
        "cursor",
        "cursor_set",
        "cursor_clear",
        "sound_test",
        "profiles",
        "profile_save",
        "profile_delete",
    ];
    // Without a host: unknown.
    let mut plain = Session::start_with(&dir, false, &[]);
    for method in methods {
        let answer = plain.call(&format!("computer_use/settings/{method}"), json!({}));
        assert_eq!(answer["error"]["code"], -32601, "{method}: {answer}");
    }
    drop(plain);
    // With one: none of them is a tool, and a tool call by that name is refused.
    let mut s = Session::start(&dir);
    let tools = s.call("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for method in methods {
        assert!(!names.contains(&method), "{method} is offered as a tool");
        let called = s.call(
            "tools/call",
            json!({"name": method, "arguments": {"event": "click"}}),
        );
        assert!(called.get("result").is_none(), "{method}: {called}");
    }
}
