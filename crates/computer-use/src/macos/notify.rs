//! Notifications on macOS. There is no public API for reading other apps'
//! notifications, so this reads what Notification Center shows through the
//! Accessibility API: the banners and alerts on screen now (and the whole
//! list while Notification Center is open).

use objc2_app_kit::NSWorkspace;

use super::ffi::{self, AxRef};
use crate::error::{Error, Result};
use crate::types::Notification;

const BUNDLE: &str = "com.apple.notificationcenterui";

fn texts(el: &AxRef, depth: usize, out: &mut Vec<String>) {
    if depth > 12 || out.len() > 20 {
        return;
    }
    let role = ffi::copy_string(el.as_ref(), "AXRole").unwrap_or_default();
    if role == "AXStaticText"
        && let Some(v) = ffi::copy_string(el.as_ref(), "AXValue")
        && !v.trim().is_empty()
    {
        out.push(v.trim().to_string());
    }
    for c in ffi::copy_elements(el.as_ref(), "AXChildren") {
        texts(&c, depth + 1, out);
    }
}

fn banners(el: &AxRef, depth: usize, out: &mut Vec<Notification>) {
    if depth > 12 {
        return;
    }
    let subrole = ffi::copy_string(el.as_ref(), "AXSubrole").unwrap_or_default();
    if subrole.starts_with("AXNotificationCenter") {
        let mut lines = Vec::new();
        texts(el, 0, &mut lines);
        let app = ffi::copy_string(el.as_ref(), "AXDescription")
            .map(|d| d.split(',').next().unwrap_or("").trim().to_string())
            .unwrap_or_default();
        let mut lines = lines.into_iter();
        let title = lines.next().unwrap_or_default();
        if !title.is_empty() {
            out.push(Notification {
                app,
                title,
                body: lines.collect::<Vec<_>>().join("\n"),
                time: None,
            });
        }
        return;
    }
    for c in ffi::copy_elements(el.as_ref(), "AXChildren") {
        banners(&c, depth + 1, out);
    }
}

/// The notifications on screen. The caller holds an autorelease pool.
pub fn recent() -> Result<Vec<Notification>> {
    // Without Accessibility access every read comes back empty: say so,
    // rather than "no notifications".
    // SAFETY: a plain query, without side effects (no prompt).
    if unsafe { ffi::AXIsProcessTrusted() } == 0 {
        return Err(super::accessibility_off());
    }
    let running = NSWorkspace::sharedWorkspace().runningApplications();
    let pid = (0..running.count())
        .map(|i| running.objectAtIndex(i))
        .find(|a| {
            a.bundleIdentifier()
                .is_some_and(|b| b.to_string() == BUNDLE)
        })
        .map(|a| a.processIdentifier())
        .ok_or_else(|| Error::Unsupported("Notification Center is not running".into()))?;
    // SAFETY: creates an AX reference for a running process (+1, owned).
    let app = unsafe { AxRef::from_create(ffi::AXUIElementCreateApplication(pid)) }
        .ok_or_else(|| Error::Platform("no accessibility access to Notification Center".into()))?;
    let mut out = Vec::new();
    for w in ffi::copy_elements(app.as_ref(), "AXWindows") {
        banners(&w, 0, &mut out);
    }
    Ok(out)
}
