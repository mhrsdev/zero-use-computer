//! Desktop notifications on Linux: apps send them to the notification
//! server as an `org.freedesktop.Notifications.Notify` call on the session
//! bus (GNOME apps as `org.gtk.Notifications.AddNotification`, sandboxed
//! ones to the desktop portal as
//! `org.freedesktop.portal.Notification.AddNotification`). A connection that has asked the bus to make it a monitor
//! (`org.freedesktop.DBus.Monitoring.BecomeMonitor`, the documented way
//! tools like `dbus-monitor` work) receives a copy of exactly those calls —
//! nothing else — and keeps the most recent ones. It only listens while
//! `[notifications]` is enabled.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt as _;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

use crate::error::{Error, Result};
use crate::types::Notification;

const RULES: [&str; 3] = [
    "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
    "type='method_call',interface='org.gtk.Notifications',member='AddNotification'",
    "type='method_call',interface='org.freedesktop.portal.Notification',member='AddNotification'",
];
/// The portal passes a notification on to the notification server: the
/// same one seen again this soon (seconds) is that copy.
const SAME_WITHIN: u64 = 2;
/// How often the listening thread checks whether it was stopped while no
/// notification arrives.
const STOP_POLL: std::time::Duration = std::time::Duration::from_millis(250);

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

/// App, title and body of a call that posts a notification, by which call
/// it is (see [`RULES`]).
fn texts(msg: &zbus::Message) -> Option<(String, String, String)> {
    let h = msg.header();
    let iface = h.interface().map(|i| i.as_str().to_string());
    let member = h.member().map(|m| m.as_str().to_string());
    // A dictionary's string, by key.
    let get = |d: &HashMap<String, OwnedValue>, k: &str| {
        d.get(k)
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_default()
    };
    match (iface.as_deref(), member.as_deref()) {
        (Some("org.freedesktop.Notifications") | None, Some("Notify")) => notify_texts(msg),
        (Some("org.gtk.Notifications"), Some("AddNotification")) => {
            let (app, _id, n): (String, String, HashMap<String, OwnedValue>) =
                msg.body().deserialize().ok()?;
            Some((app, get(&n, "title"), get(&n, "body")))
        }
        (Some("org.freedesktop.portal.Notification"), Some("AddNotification")) => {
            let (_id, n): (String, HashMap<String, OwnedValue>) = msg.body().deserialize().ok()?;
            let mut body = get(&n, "body");
            if body.is_empty() {
                body = get(&n, "markup-body");
            }
            // The portal knows which app; the call doesn't say.
            Some((String::new(), get(&n, "title"), body))
        }
        _ => None,
    }
}

/// Keep a notification, at most `keep` of them: the copy the portal passes
/// on (the same title and body just after) only names the app.
fn record(q: &mut VecDeque<Notification>, n: Notification, keep: usize) {
    if let Some(last) = q.back_mut()
        && last.title == n.title
        && last.body == n.body
        && matches!((last.time, n.time), (Some(a), Some(b)) if b.saturating_sub(a) <= SAME_WITHIN)
    {
        if last.app.is_empty() {
            last.app = n.app;
        }
        return;
    }
    q.push_back(n);
    while q.len() > keep {
        q.pop_front();
    }
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
        &(RULES.to_vec(), 0u32),
    )?;
    let mut stream = zbus::MessageStream::from(conn.inner());
    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        // Wake up regularly so stop() ends the thread (and closes the
        // connection) even when no notification ever arrives.
        let next = async_io::block_on(futures_util::future::select(
            stream.next(),
            async_io::Timer::after(STOP_POLL),
        ));
        let msg = match next {
            futures_util::future::Either::Left((Some(Ok(msg)), _)) => msg,
            futures_util::future::Either::Left(_) => break,
            futures_util::future::Either::Right(_) => continue,
        };
        let Some((app, summary, body)) = texts(&msg) else {
            continue;
        };
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        let mut q = seen.lock().unwrap_or_else(|p| p.into_inner());
        let n = Notification {
            app,
            title: plain(&summary),
            body: plain(&body),
            time,
        };
        record(&mut q, n, keep.load(Ordering::SeqCst));
    }
    Ok(())
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call<B>(iface: &str, member: &str, body: &B) -> zbus::Message
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        zbus::Message::method_call("/org/x", member)
            .unwrap()
            .interface(iface)
            .unwrap()
            .build(body)
            .unwrap()
    }

    fn dict(pairs: &[(&str, &str)]) -> HashMap<String, zbus::zvariant::Value<'static>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), zbus::zvariant::Value::from(v.to_string())))
            .collect()
    }

    #[test]
    fn every_way_of_posting_a_notification_is_read() {
        let hints: HashMap<String, zbus::zvariant::Value> = HashMap::new();
        let fdo = call(
            "org.freedesktop.Notifications",
            "Notify",
            &(
                "Mail",
                0u32,
                "",
                "New mail",
                "From Ada",
                Vec::<String>::new(),
                hints,
                -1i32,
            ),
        );
        assert_eq!(
            texts(&fdo),
            Some(("Mail".into(), "New mail".into(), "From Ada".into()))
        );
        let gtk = call(
            "org.gtk.Notifications",
            "AddNotification",
            &(
                "org.gnome.Calendar",
                "n1",
                dict(&[("title", "Meeting"), ("body", "at 10")]),
            ),
        );
        assert_eq!(
            texts(&gtk),
            Some((
                "org.gnome.Calendar".into(),
                "Meeting".into(),
                "at 10".into()
            ))
        );
        let portal = call(
            "org.freedesktop.portal.Notification",
            "AddNotification",
            &(
                "n2",
                dict(&[("title", "Done"), ("body", "Export finished")]),
            ),
        );
        assert_eq!(
            texts(&portal),
            Some((String::new(), "Done".into(), "Export finished".into()))
        );
        // Not a notification being posted.
        let other = call("org.gtk.Notifications", "RemoveNotification", &("a", "n1"));
        assert_eq!(texts(&other), None);
    }

    #[test]
    fn the_portals_copy_is_kept_once_with_the_app_name() {
        let n = |app: &str, title: &str, time| Notification {
            app: app.into(),
            title: title.into(),
            body: "b".into(),
            time: Some(time),
        };
        let mut q = VecDeque::new();
        record(&mut q, n("", "Done", 100), 10);
        record(&mut q, n("org.app", "Done", 101), 10);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].app, "org.app");
        // The same again later is another notification.
        record(&mut q, n("org.app", "Done", 110), 10);
        record(&mut q, n("org.app", "Other", 110), 10);
        assert_eq!(q.len(), 3);
        record(&mut q, n("x", "Last", 111), 2);
        assert_eq!(q.len(), 2);
        assert_eq!(q[1].title, "Last");
    }

    #[test]
    fn markup_is_removed() {
        assert_eq!(
            super::plain("<b>Ada</b>: see <a href=\"x\">this</a> &amp; that"),
            "Ada: see this & that"
        );
    }
}
