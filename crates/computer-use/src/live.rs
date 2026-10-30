//! What a host may read at any time, even while a tool call runs
//! (`computer_use/state`): whether the stop key works right now, and which
//! app the agent is working on. The stop flag itself is the engine's
//! ([`crate::engine::Engine::stop_handle`]); this holds what the engine learns
//! while it works and the server's reader thread must read without waiting
//! for the engine. It also tells a host what happened as it happens
//! (`notifications/computer_use/event`): the stop held or let go, the stop
//! key lost, a message sent to the other agents.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::overlay::StopKeyProbe;

/// The version of what a host may ask and is told: 2 is a stop key that
/// only stops, `computer_use/stop`, and the events.
pub const PROTOCOL: u32 = 2;

/// The notification a host is told what happened with.
pub const EVENT_METHOD: &str = "notifications/computer_use/event";

/// Who held the stop or let it go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBy {
    /// The stop key: it only stops, and stops every agent.
    Key,
    /// The overlay's button for every agent (Stop all, Continue all).
    All,
    /// The overlay's button for this one agent (or an older helper or hub,
    /// which does not say).
    One,
    /// The host (`computer_use/stop`, `computer_use/resume`).
    Host,
}

/// What a host is told as it happens. Only a change is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    Stopped {
        by: StopBy,
    },
    Resumed {
        by: StopBy,
    },
    /// The stop key had been registered and no longer is.
    HotkeyLost,
    /// This agent sent a message to the others (`to`: one of them, or all).
    Message {
        to: Option<u32>,
        text: String,
    },
}

/// Where the events go: the host's output.
pub type EventSink = std::sync::Arc<dyn Fn(Value) + Send + Sync>;

/// How long an app still counts as the one being worked on after the last
/// step on it, so a host that asks every second or two sees the agent at work
/// between two quick steps.
pub const LINGER: Duration = Duration::from_secs(4);

#[derive(Debug)]
struct Working {
    id: String,
    name: String,
    /// A call that acts on it is running now.
    running: bool,
    last: Instant,
}

#[derive(Default)]
pub struct Live {
    /// `control.stop_hotkey` as set (empty: the user chose none).
    key: Mutex<String>,
    /// Asked on every answer, so a helper that has gone is seen at once and
    /// not at the end of the next call.
    probe: Mutex<Option<StopKeyProbe>>,
    working: Mutex<Option<Working>>,
    /// The host the events go to (none: no host started this server).
    sink: Mutex<Option<EventSink>>,
    /// This engine's number in the hub, while it is in one.
    agent: Mutex<Option<u32>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Live {
    pub fn new(key: &str) -> Self {
        let live = Self::default();
        live.set_key(key);
        live
    }

    pub fn set_key(&self, key: &str) {
        *lock(&self.key) = key.trim().to_string();
    }

    /// The helper that listens for the stop key (`None`: there is none).
    pub fn set_probe(&self, probe: Option<StopKeyProbe>) {
        *lock(&self.probe) = probe;
    }

    /// Tell `sink` what happens from now on (`None`: no one).
    pub fn set_sink(&self, sink: Option<EventSink>) {
        *lock(&self.sink) = sink;
    }

    /// This engine's number in the hub (`None`: it is in none).
    pub fn set_agent(&self, agent: Option<u32>) {
        *lock(&self.agent) = agent;
    }

    /// Tell the host, if there is one, what just happened.
    pub fn emit(&self, event: HostEvent) {
        let Some(sink) = lock(&self.sink).clone() else {
            return;
        };
        let source = |by: StopBy| match by {
            StopBy::Key => "hotkey",
            StopBy::All | StopBy::One => "overlayButton",
            StopBy::Host => "host",
        };
        let mut params = match event {
            HostEvent::Stopped { by } => json!({
                "type": "stopped",
                "source": source(by),
                "all": matches!(by, StopBy::Key | StopBy::All),
            }),
            HostEvent::Resumed { by } => json!({
                "type": "resumed",
                "source": source(by),
                "all": by == StopBy::All,
            }),
            HostEvent::HotkeyLost => json!({ "type": "hotkeyLost" }),
            HostEvent::Message { to, text } => json!({
                "type": "message",
                "to": to,
                "text": text,
            }),
        };
        if let Some(agent) = *lock(&self.agent) {
            params["agent"] = json!(agent);
        }
        sink(json!({ "jsonrpc": "2.0", "method": EVENT_METHOD, "params": params }));
    }

    /// An action starts on an app.
    pub fn begin(&self, id: &str, name: &str, now: Instant) {
        *lock(&self.working) = Some(Working {
            id: if id.is_empty() { name } else { id }.to_string(),
            name: name.to_string(),
            running: true,
            last: now,
        });
    }

    /// The call ended; the app stays named for [`LINGER`].
    pub fn end(&self, now: Instant) {
        if let Some(w) = lock(&self.working).as_mut() {
            w.running = false;
            w.last = now;
        }
    }

    /// The answer to `computer_use/state`. `registered` is true only when the
    /// system accepted the key and its helper runs: not known, refused, no key
    /// at all and a helper that has gone are all `false`.
    pub fn answer(&self, stopped: bool, now: Instant) -> Value {
        let key = lock(&self.key).clone();
        let registered =
            !key.is_empty() && lock(&self.probe).as_ref().is_some_and(|p| p.works(&key));
        let controlling = lock(&self.working)
            .as_ref()
            .filter(|w| w.running || now.saturating_duration_since(w.last) < LINGER)
            .map(|w| json!({ "app": { "id": w.id, "name": w.name } }));
        let mut hotkey = json!({ "registered": registered });
        if !key.is_empty() {
            hotkey["key"] = json!(key);
        }
        json!({
            "protocol": PROTOCOL,
            "stopped": stopped,
            "hotkey": hotkey,
            "controlling": controlling,
            "agent": *lock(&self.agent),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn probe(alive: bool, report: Option<(&str, bool)>) -> (StopKeyProbe, Arc<AtomicBool>) {
        let alive = Arc::new(AtomicBool::new(alive));
        let report = report.map(|(k, ok)| (k.to_string(), ok));
        (
            StopKeyProbe::new(alive.clone(), Arc::new(Mutex::new(report))),
            alive,
        )
    }

    fn registered(live: &Live) -> bool {
        live.answer(false, Instant::now())["hotkey"]["registered"] == true
    }

    #[test]
    fn the_key_is_registered_only_when_the_system_said_so_and_its_helper_runs() {
        let live = Live::new("ctrl+alt+escape");
        // No helper at all.
        assert!(!registered(&live));
        // Not said yet, refused, said for another key.
        for report in [None, Some(("ctrl+alt+escape", false)), Some(("f12", true))] {
            live.set_probe(Some(probe(true, report).0));
            assert!(!registered(&live), "{report:?}");
        }
        let (p, alive) = probe(true, Some(("Ctrl+Alt+Escape", true)));
        live.set_probe(Some(p));
        assert!(registered(&live));
        // The helper goes between two calls: seen at once.
        alive.store(false, Ordering::Relaxed);
        assert!(!registered(&live));
        // A key the user removed is not registered, whatever the helper says.
        live.set_probe(Some(probe(true, Some(("", true))).0));
        live.set_key(" ");
        let answer = live.answer(false, Instant::now());
        assert_eq!(answer["hotkey"]["registered"], false);
        assert!(answer["hotkey"].get("key").is_none());
    }

    #[test]
    fn the_answer_carries_the_hold_the_protocol_and_the_key_as_set() {
        let live = Live::new(" ctrl+alt+escape ");
        let answer = live.answer(true, Instant::now());
        assert_eq!(answer["protocol"], 2);
        assert!(answer["agent"].is_null());
        assert_eq!(answer["stopped"], true);
        assert_eq!(answer["hotkey"]["key"], "ctrl+alt+escape");
        assert!(answer["controlling"].is_null());
        assert_eq!(live.answer(false, Instant::now())["stopped"], false);
    }

    /// What a host is told: only with a host, in the contract's words, and
    /// naming the agent only while it is in a hub.
    #[test]
    fn an_event_names_the_agent_only_in_a_hub() {
        let live = Live::default();
        // No host: nothing to tell, nothing breaks.
        live.emit(HostEvent::HotkeyLost);
        let told = Arc::new(Mutex::new(Vec::new()));
        let into = told.clone();
        live.set_sink(Some(Arc::new(move |v| into.lock().unwrap().push(v))));
        live.emit(HostEvent::Stopped { by: StopBy::Key });
        live.set_agent(Some(3));
        live.emit(HostEvent::Resumed { by: StopBy::Host });
        live.emit(HostEvent::Message {
            to: None,
            text: "done with the sheet".into(),
        });
        let told = told.lock().unwrap();
        assert!(told.iter().all(|m| m["method"] == EVENT_METHOD));
        let params: Vec<&Value> = told.iter().map(|m| &m["params"]).collect();
        assert_eq!(
            *params[0],
            json!({"type": "stopped", "source": "hotkey", "all": true})
        );
        assert_eq!(
            *params[1],
            json!({"type": "resumed", "source": "host", "all": false, "agent": 3})
        );
        assert_eq!(
            *params[2],
            json!({"type": "message", "to": null, "text": "done with the sheet", "agent": 3})
        );
        assert_eq!(live.answer(false, Instant::now())["agent"], 3);
    }

    #[test]
    fn an_app_is_controlled_while_a_call_runs_and_for_a_moment_after() {
        let live = Live::default();
        let t0 = Instant::now();
        assert!(live.answer(false, t0)["controlling"].is_null());
        live.begin("notepad.exe", "Notepad", t0);
        let named = live.answer(false, t0 + Duration::from_secs(60));
        assert_eq!(named["controlling"]["app"]["id"], "notepad.exe");
        assert_eq!(named["controlling"]["app"]["name"], "Notepad");
        live.end(t0);
        assert_eq!(
            live.answer(false, t0 + Duration::from_secs(1))["controlling"]["app"]["name"],
            "Notepad"
        );
        assert!(live.answer(false, t0 + LINGER)["controlling"].is_null());
        // An app with no id is still named.
        live.begin("", "Notepad", t0);
        assert_eq!(
            live.answer(false, t0)["controlling"]["app"]["id"],
            "Notepad"
        );
    }
}
