//! What a host may read at any time, even while a tool call runs
//! (`computer_use/state`): whether the stop key works right now, and which
//! app the agent is working on. The stop flag itself is the engine's
//! ([`crate::engine::Engine::stop_handle`]); this holds what the engine learns
//! while it works and the server's reader thread must read without waiting
//! for the engine.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::overlay::StopKeyProbe;

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
            "protocol": 1,
            "stopped": stopped,
            "hotkey": hotkey,
            "controlling": controlling,
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
        assert_eq!(answer["protocol"], 1);
        assert_eq!(answer["stopped"], true);
        assert_eq!(answer["hotkey"]["key"], "ctrl+alt+escape");
        assert!(answer["controlling"].is_null());
        assert_eq!(live.answer(false, Instant::now())["stopped"], false);
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
