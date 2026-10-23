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
        Self::start_with(home, &["--host", "zero"])
    }

    /// The program as `args` start it (a host's `--host zero`, or none), on the scripted desktop.
    fn start_with(home: &std::path::Path, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_computer-use-mcp"))
            .args(args)
            .args(["--backend", "mock", "serve"])
            .env("COMPUTER_USE_HOME", home)
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

/// The raw conversation of the request document (section 10.2), without Zero.
#[test]
fn the_state_and_resume_answer_the_shapes_a_host_waits_for() {
    let dir = home("raw-state", "");
    let mut s = Session::start(&dir);
    let state = s.call("computer_use/state", json!({}));
    let state = &state["result"];
    assert_eq!(state["protocol"], 1);
    assert_eq!(state["stopped"], false);
    // The scripted desktop has no helper that listens for keys: the key is not
    // registered, and the answer says so instead of leaving it out.
    assert_eq!(state["hotkey"]["registered"], false);
    assert_eq!(state["hotkey"]["key"], "ctrl+alt+escape");
    // The component says what it controls: nothing, here.
    assert!(state.as_object().unwrap().contains_key("controlling"));
    assert!(state["controlling"].is_null());
    assert_eq!(
        s.call("computer_use/resume", json!({}))["result"],
        json!({"stopped": false})
    );
    assert_eq!(
        s.call("computer_use/state", json!({}))["result"]["stopped"],
        false
    );
}
