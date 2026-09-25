//! Desktop notifications on Linux: every app sends them to the notification
//! server as an `org.freedesktop.Notifications.Notify` call on the session
//! bus. A connection that has asked the bus to make it a monitor
//! (`org.freedesktop.DBus.Monitoring.BecomeMonitor`, the documented way
//! tools like `dbus-monitor` work) receives a copy of exactly those calls —
//! nothing else — and keeps the most recent ones. It only listens while
//! `[notifications]` is enabled.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::OwnedValue;

use crate::error::{Error, Result};
use crate::types::Notification;

const RULE: &str = "type='method_call',interface='org.freedesktop.Notifications',member='Notify'";

pub struct Listener {
    seen: Arc<Mutex<VecDeque<Notification>>>,
    keep: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

/// Notification bodies may carry simple markup (`<b>`, `<a href>`…).
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

impl Listener {
    pub fn start(keep: usize) -> Self {
        let l = Self {
            seen: Arc::new(Mutex::new(VecDeque::new())),
            keep: Arc::new(AtomicUsize::new(keep.max(1))),
            running: Arc::new(AtomicBool::new(true)),
            error: Arc::new(Mutex::new(None)),
        };
        let (seen, keep, running, error) = (
            l.seen.clone(),
            l.keep.clone(),
            l.running.clone(),
            l.error.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || {
                if let Err(e) = listen(&seen, &keep, &running) {
                    log::warn!("not listening for notifications: {e}");
                    *error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
                }
                running.store(false, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            *l.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
            l.running.store(false, Ordering::SeqCst);
        }
        l
    }

    pub fn set_keep(&self, keep: usize) {
        self.keep.store(keep.max(1), Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn recent(&self) -> Result<Vec<Notification>> {
        if let Some(e) = self.error.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            return Err(Error::Unsupported(format!(
                "cannot listen for notifications on the session bus ({e})"
            )));
        }
        Ok(self
            .seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .cloned()
            .collect())
    }
}

type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedValue>,
    i32,
);

/// App name, summary and body of a Notify call; by position when a client
/// sent unusual types for the other arguments.
fn notify_texts(msg: &zbus::Message) -> Option<(String, String, String)> {
    let body = msg.body();
    if let Ok((app, _, _, summary, text, _, _, _)) = body.deserialize::<NotifyArgs>() {
        return Some((app, summary, text));
    }
    let s = body.deserialize::<zbus::zvariant::Structure>().ok()?;
    let text = |i: usize| -> Option<String> {
        match s.fields().get(i)? {
            zbus::zvariant::Value::Str(v) => Some(v.to_string()),
            _ => None,
        }
    };
    Some((text(0)?, text(3)?, text(4)?))
}

fn listen(
    seen: &Mutex<VecDeque<Notification>>,
    keep: &AtomicUsize,
    running: &AtomicBool,
) -> zbus::Result<()> {
    let conn = Connection::session()?;
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus.Monitoring"),
        "BecomeMonitor",
        &(vec![RULE], 0u32),
    )?;
    for msg in MessageIterator::from(&conn) {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        let Ok(msg) = msg else { break };
        if msg.header().member().map(|m| m.as_str()) != Some("Notify") {
            continue;
        }
        let Some((app, summary, body)) = notify_texts(&msg) else {
            continue;
        };
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        let mut q = seen.lock().unwrap_or_else(|p| p.into_inner());
        q.push_back(Notification {
            app,
            title: plain(&summary),
            body: plain(&body),
            time,
        });
        while q.len() > keep.load(Ordering::SeqCst) {
            q.pop_front();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn markup_is_removed() {
        assert_eq!(
            super::plain("<b>Ada</b>: see <a href=\"x\">this</a> &amp; that"),
            "Ada: see this & that"
        );
    }
}
