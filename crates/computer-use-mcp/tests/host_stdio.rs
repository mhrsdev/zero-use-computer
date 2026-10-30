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
    /// The notifications seen while waiting for answers.
    notes: Vec<Value>,
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
            notes: Vec::new(),
        };
        let hello = session.call(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
        );
        assert_eq!(hello["result"]["serverInfo"]["name"], "computer-use");
        session
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.answer(id)
    }

    /// Sends a request without waiting for its answer; its id.
    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.input, "{line}").unwrap();
        self.input.flush().unwrap();
        id
    }

    /// The answer to request `id`, skipping every other message.
    fn answer(&mut self, id: u64) -> Value {
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
            if message.get("id").is_none() {
                self.notes.push(message);
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

/// The marker a host-started program keeps while the user's stop is held.
const STOP_HELD: &str = "stop-held";

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("never happened: {what}");
}

/// The raw conversation of the request document (section 10.2), without Zero.
#[test]
fn the_state_and_resume_answer_the_shapes_a_host_waits_for() {
    let dir = home("raw-state", "");
    let mut s = Session::start(&dir);
    let state = s.call("computer_use/state", json!({}));
    let state = &state["result"];
    assert_eq!(state["protocol"], 2);
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
    // Asking changed nothing, and wrote nothing.
    assert_eq!(
        s.call("computer_use/state", json!({}))["result"]["stopped"],
        false
    );
    assert!(!dir.join(STOP_HELD).exists());
}

#[test]
fn a_stop_that_was_held_is_kept_over_a_restart_and_only_resume_lets_it_go() {
    let dir = home("held", "");
    std::fs::write(dir.join(STOP_HELD), "the user stopped the agent\n").unwrap();
    // Started again and again: still held, nothing let it go.
    for _ in 0..2 {
        let mut s = Session::start(&dir);
        assert_eq!(
            s.call("computer_use/state", json!({}))["result"]["stopped"],
            true
        );
    }
    assert!(dir.join(STOP_HELD).exists());

    let mut s = Session::start(&dir);
    // The agent can neither act nor look.
    for (tool, arguments) in [
        ("click", json!({"app": "TextEdit", "element_index": 1})),
        ("list_apps", json!({})),
    ] {
        let refused = s.call("tools/call", json!({"name": tool, "arguments": arguments}));
        assert_eq!(refused["result"]["isError"], true, "{tool}: {refused}");
        let text = refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(text.to_lowercase().contains("stop"), "{tool}: {text}");
    }
    // Only the host's Resume (the user's click) lets it go.
    assert_eq!(
        s.call("computer_use/resume", json!({}))["result"],
        json!({"stopped": false})
    );
    assert_eq!(
        s.call("computer_use/state", json!({}))["result"]["stopped"],
        false
    );
    wait_until("the marker is gone", || !dir.join(STOP_HELD).exists());
    let looked = s.call("tools/call", json!({"name": "list_apps", "arguments": {}}));
    assert!(looked["result"]["isError"] != true, "{looked}");
    // Letting go twice is nothing.
    assert_eq!(
        s.call("computer_use/resume", json!({}))["result"],
        json!({"stopped": false})
    );
    drop(s);
    // After a Resume a restart is not held.
    let mut s = Session::start(&dir);
    assert_eq!(
        s.call("computer_use/state", json!({}))["result"]["stopped"],
        false
    );
}

#[test]
fn without_a_host_the_state_is_answered_resume_is_not_and_an_old_marker_is_not_looked_at() {
    let dir = home("no-host", "");
    std::fs::write(dir.join(STOP_HELD), "x").unwrap();
    let mut s = Session::start_with(&dir, &[]);
    let state = s.call("computer_use/state", json!({}));
    assert_eq!(state["result"]["stopped"], false);
    let resume = s.call("computer_use/resume", json!({}));
    assert_eq!(resume["error"]["code"], -32601, "{resume}");
    // Nothing of this program's own, outside a host, is kept or let go.
    assert!(dir.join(STOP_HELD).exists());
}

/// A host's stop holds in a program that never joined a hub (the scripted
/// desktop has none): the agent is refused, the stop is kept over a
/// restart, and only Resume lets it go. (While a call runs: the server's
/// own tests.)
#[test]
fn the_hosts_stop_holds_and_needs_no_hub() {
    let dir = home("host-stop", "");
    let mut s = Session::start(&dir);
    let stop = s.call("computer_use/stop", json!({}));
    assert_eq!(
        stop["result"],
        json!({"stopped": true, "wasStopped": false}),
        "{stop}"
    );
    let refused = s.call("tools/call", json!({"name": "list_apps", "arguments": {}}));
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    // Asked again: it says it was stopped already.
    assert_eq!(
        s.call("computer_use/stop", json!({}))["result"],
        json!({"stopped": true, "wasStopped": true})
    );
    wait_until("the stop is kept", || dir.join(STOP_HELD).exists());
    drop(s);
    let mut s = Session::start(&dir);
    assert_eq!(
        s.call("computer_use/state", json!({}))["result"]["stopped"],
        true
    );
    s.call("computer_use/resume", json!({}));
    assert_eq!(
        s.call("computer_use/state", json!({}))["result"]["stopped"],
        false
    );
}

#[test]
fn stop_is_a_hosts_request() {
    let dir = home("no-host-stop", "");
    let mut s = Session::start_with(&dir, &[]);
    let stop = s.call("computer_use/stop", json!({}));
    assert_eq!(stop["error"]["code"], -32601, "{stop}");
    assert_eq!(
        s.call("computer_use/state", json!({}))["result"]["stopped"],
        false
    );
}

/// Programs a host started for two agents: each with its own folder, both
/// with one hub folder and one settings folder.
fn pair(name: &str) -> [std::path::PathBuf; 4] {
    let shared = home(name, "[host]\nuser_keys = [\"tools.preset\"]\n");
    let [a, b, hub] = ["a", "b", "hub"].map(|d| {
        let dir = shared.join(d);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    });
    [a, b, hub, shared]
}

fn start_in(own: &std::path::Path, hub: &std::path::Path, settings: &std::path::Path) -> Session {
    let (hub, settings) = (hub.display().to_string(), settings.display().to_string());
    Session::start_with(
        own,
        &[
            "--host",
            "zero",
            "--hub-home",
            &hub,
            "--settings-home",
            &settings,
        ],
    )
}

/// Sharing a hub used to mean sharing the server's folder, and with it the
/// user's stop: one agent's stop then held the other after a restart, and
/// one's Resume let the other's go.
#[test]
fn two_sessions_with_their_own_homes_and_one_hub_home_never_share_a_stop() {
    let [a, b, hub, settings] = pair("two");
    let mut one = start_in(&a, &hub, &settings);
    let mut two = start_in(&b, &hub, &settings);
    assert_eq!(
        one.call("computer_use/stop", json!({}))["result"]["stopped"],
        true
    );
    wait_until("the stop is kept", || a.join(STOP_HELD).exists());
    for elsewhere in [&b, &hub, &settings] {
        assert!(
            !elsewhere.join(STOP_HELD).exists(),
            "{}",
            elsewhere.display()
        );
    }
    assert_eq!(
        two.call("computer_use/state", json!({}))["result"]["stopped"],
        false
    );
    drop(two);
    let mut two = start_in(&b, &hub, &settings);
    assert_eq!(
        two.call("computer_use/state", json!({}))["result"]["stopped"],
        false,
        "the other's stop came back with a restart"
    );
    drop(one);
    let mut one = start_in(&a, &hub, &settings);
    assert_eq!(
        one.call("computer_use/state", json!({}))["result"]["stopped"],
        true
    );
}

/// The user's settings are one for all the agents: a change made through
/// one program is in the shared file and reaches the other, running.
#[test]
fn the_settings_are_read_from_the_settings_home() {
    let [a, b, hub, settings] = pair("settings");
    let mut one = start_in(&a, &hub, &settings);
    let mut two = start_in(&b, &hub, &settings);
    let count = |v: &Value| v["result"]["tools"].as_array().unwrap().len();
    let before = count(&two.call("tools/list", json!({})));
    let set = one.call(
        "computer_use/settings/set",
        json!({"key": "tools.preset", "value": "small"}),
    );
    assert!(set.get("result").is_some(), "{set}");
    assert!(
        std::fs::read_to_string(settings.join("user.toml"))
            .unwrap()
            .contains("small")
    );
    assert!(!a.join("user.toml").exists() && !b.join("user.toml").exists());
    wait_until("the other follows the change", || {
        count(&two.call("tools/list", json!({}))) != before
    });
}

/// A host is told when the stop is held and let go, once per change, in
/// the contract's words; the state says the protocol that has it.
#[test]
fn a_host_is_told_when_the_stop_is_held_and_let_go() {
    let dir = home("events", "");
    let mut s = Session::start(&dir);
    let state = s.call("computer_use/state", json!({}));
    assert_eq!(state["result"]["protocol"], 2);
    assert!(state["result"]["agent"].is_null());
    let events = |s: &mut Session| -> Vec<Value> {
        std::mem::take(&mut s.notes)
            .into_iter()
            .filter(|m| m["method"] == "notifications/computer_use/event")
            .map(|m| m["params"].clone())
            .collect()
    };
    assert!(events(&mut s).is_empty());
    s.call("computer_use/stop", json!({}));
    assert_eq!(
        events(&mut s),
        [json!({"type": "stopped", "source": "host", "all": false})]
    );
    s.call("computer_use/stop", json!({}));
    assert!(events(&mut s).is_empty(), "a stop already held is no news");
    s.call("computer_use/resume", json!({}));
    assert_eq!(
        events(&mut s),
        [json!({"type": "resumed", "source": "host", "all": false})]
    );
    s.call("computer_use/resume", json!({}));
    assert!(events(&mut s).is_empty());
}
