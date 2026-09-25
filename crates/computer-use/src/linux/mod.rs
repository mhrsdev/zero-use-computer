//! Linux backend: AT-SPI2 (over D-Bus) for the accessibility tree and
//! semantic actions, X11/XTest for synthesized input, and X11 GetImage for
//! screenshots.
//!
//! The accessibility path (DoAction, SetTextContents, GrabFocus, text
//! selection) is preferred; synthesized mouse/keyboard input is the fallback.

mod atspi;
mod clipboard;
pub(crate) mod x11;

use std::collections::HashMap;
use std::process::Command;

pub use atspi::ipc_calls;
use atspi::{AtspiConnection, ObjRef, state};
use x11::X11;

use crate::backend::{Backend, Native};
use crate::error::{Error, Result};
use crate::keys::KeyCombo;
use crate::roles;
use crate::types::*;

/// Roles that count as top-level windows.
fn is_window_role(role: &str) -> bool {
    matches!(
        role,
        "frame" | "window" | "dialog" | "alert" | "file chooser" | "color chooser"
    )
}

pub struct LinuxBackend {
    a11y: AtspiConnection,
    x11: Option<X11>,
    /// pid → application accessible.
    app_refs: HashMap<u32, ObjRef>,
    /// handle → (owning pid, element). Cleared per app on each snapshot so it
    /// never grows beyond the elements of the latest views.
    handles: HashMap<ElementHandle, (u32, ObjRef)>,
    next_handle: ElementHandle,
    batch_size: usize,
    text_max: usize,
}

impl LinuxBackend {
    pub fn new() -> Result<Self> {
        let a11y = AtspiConnection::connect()?;
        // X11 is optional: element-level actions work without it, but input
        // and screenshots need it.
        let x11 = match X11::connect() {
            Ok(x) => Some(x),
            Err(e) => {
                log::warn!("X11 input/capture unavailable: {e}");
                None
            }
        };
        let defaults = crate::config::LinuxConfig::default();
        Ok(Self {
            a11y,
            x11,
            app_refs: HashMap::new(),
            handles: HashMap::new(),
            next_handle: 1,
            batch_size: defaults.batch_size,
            text_max: defaults.text_max_chars,
        })
    }

    fn x11(&mut self) -> Result<&mut X11> {
        self.x11
            .as_mut()
            .ok_or_else(|| Error::Platform("no X11 connection for input/capture".into()))
    }

    fn handle_for(&mut self, pid: u32, r: ObjRef) -> ElementHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(h, (pid, r));
        h
    }

    fn resolve(&self, handle: ElementHandle) -> Result<ObjRef> {
        self.handles
            .get(&handle)
            .map(|(_, r)| r.clone())
            .ok_or_else(|| Error::Internal(format!("stale element handle {handle}")))
    }

    /// Application accessibles and their pids, looked up concurrently.
    fn refresh_apps(&mut self) -> Result<Vec<(ObjRef, u32)>> {
        let root = self.a11y.root();
        let children = self.a11y.children(&root)?;
        let pids = self.a11y.pids_of(&children);
        self.app_refs.clear();
        let mut apps = Vec::new();
        for (child, pid) in children.into_iter().zip(pids) {
            if let Some(pid) = pid {
                self.app_refs.insert(pid, child.clone());
                apps.push((child, pid));
            }
        }
        Ok(apps)
    }

    fn app_ref(&mut self, pid: u32) -> Result<ObjRef> {
        if let Some(r) = self.app_refs.get(&pid) {
            return Ok(r.clone());
        }
        self.refresh_apps()?;
        self.app_refs
            .get(&pid)
            .cloned()
            .ok_or(Error::AppNotFound(format!("pid {pid}")))
    }

    /// Top-level windows of an app (children fetched concurrently).
    fn windows_of(&mut self, app_ref: &ObjRef) -> Result<Vec<(ObjRef, atspi::NodeData)>> {
        let children = self.a11y.children(app_ref)?;
        let data = self.a11y.fetch_many(&children);
        Ok(children
            .into_iter()
            .zip(data)
            .filter(|(_, d)| {
                let role = roles::from_atspi(&d.acc.role_name);
                let has_extent = d.extents.is_some_and(|(_, _, w, h)| w > 0 && h > 0);
                is_window_role(&role) || (has_extent && d.acc.states.has(state::SHOWING))
            })
            .collect())
    }

    fn build_node(&mut self, pid: u32, w: atspi::Walked) -> RawNode {
        let atspi::Walked { r, data, parent } = w;
        let acc = data.acc;
        let role = roles::from_atspi(&acc.role_name);
        let bounds = data
            .extents
            .map(|(x, y, w, h)| Rect::new(x.into(), y.into(), w.into(), h.into()));

        let editable = acc.states.has(state::EDITABLE);
        let text_value = data.text;
        // A label with no Name but text content: promote the text to a name.
        let (name, value) = if acc.name.is_empty() {
            match text_value {
                Some(t) if role == "text" => (Some(t), None),
                other => (None, other),
            }
        } else if role == "text" && text_value.as_deref() == Some(acc.name.as_str()) {
            (Some(acc.name.clone()), None)
        } else {
            (Some(acc.name.clone()), text_value)
        };

        let s = &acc.states;
        let checkable = s.has(state::CHECKABLE)
            || matches!(
                role.as_str(),
                "checkbox" | "radio button" | "toggle button" | "switch"
            );
        let states = NodeStates {
            enabled: s.has(state::ENABLED) && s.has(state::SENSITIVE),
            focused: s.has(state::FOCUSED),
            selected: s.has(state::SELECTED),
            checked: checkable.then(|| s.has(state::CHECKED) || s.has(state::PRESSED)),
            expanded: s.has(state::EXPANDABLE).then(|| s.has(state::EXPANDED)),
            editable,
            value_settable: editable || acc.has_iface("EditableText") || acc.has_iface("Value"),
            hidden: !(s.has(state::SHOWING) && s.has(state::VISIBLE)),
        };

        let actions = data
            .actions
            .into_iter()
            .map(|native| ActionDesc::new(roles::atspi_action(&native), native))
            .collect();

        let key = Some(r.path.clone());
        let handle = self.handle_for(pid, r);
        RawNode {
            handle,
            parent,
            key,
            role,
            native_role: acc.role_name,
            name: name.filter(|s| !s.is_empty()),
            description: (!acc.description.is_empty()).then_some(acc.description),
            value,
            placeholder: None,
            identifier: None,
            bounds,
            actions,
            states,
        }
    }
}

impl Backend for LinuxBackend {
    fn name(&self) -> &'static str {
        "linux"
    }

    fn permissions(&mut self) -> Vec<PermissionStatus> {
        vec![
            PermissionStatus {
                name: "AT-SPI accessibility bus".into(),
                granted: true,
                detail: "connected".into(),
            },
            PermissionStatus {
                name: "X11 input/capture".into(),
                granted: self.x11.is_some(),
                detail: if self.x11.is_some() {
                    "connected".into()
                } else {
                    "no X11 connection (DISPLAY unset or unreachable)".into()
                },
            },
        ]
    }

    fn list_apps(&mut self) -> Result<Vec<AppInfo>> {
        let apps = self.refresh_apps()?;
        let refs: Vec<ObjRef> = apps.iter().map(|(r, _)| r.clone()).collect();
        let data = self.a11y.fetch_many(&refs);
        // The active window's pid from the window manager (one X11 query for
        // all apps) instead of walking every app's windows.
        let active = self.x11.as_ref().and_then(|x| x.active_pid());
        let mut out = Vec::new();
        for ((_, pid), d) in apps.into_iter().zip(data) {
            let acc = d.acc;
            if acc.role_name != "application" && acc.name.is_empty() {
                continue;
            }
            let (exe, comm) = proc_info(pid);
            let name = if !acc.name.is_empty() {
                acc.name.clone()
            } else {
                comm.clone().unwrap_or_else(|| format!("pid {pid}"))
            };
            out.push(AppInfo {
                name,
                id: comm.unwrap_or_else(|| acc.name.clone()),
                pid,
                exe,
                frontmost: active == Some(pid),
                hidden: false,
            });
        }
        Ok(out)
    }

    fn launch_app(&mut self, query: &str) -> Result<()> {
        let mut parts = query.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| Error::InvalidArgs("empty app".into()))?;
        let args: Vec<&str> = parts.collect();
        Command::new(program)
            .args(&args)
            .spawn()
            .map(|_| ())
            .map_err(|e| {
                Error::ActionFailed(format!(
                    "could not launch `{program}`: {e}. Pass an executable name on PATH."
                ))
            })
    }

    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let app_ref = self.app_ref(app.pid)?;
        let windows = self.windows_of(&app_ref)?;
        let mut out = Vec::new();
        for (r, d) in windows {
            let bounds = d
                .extents
                .map(|(x, y, w, h)| Rect::new(x.into(), y.into(), w.into(), h.into()));
            let id = stable_id(&r.path);
            let handle = self.handle_for(app.pid, r);
            let active = d.acc.states.has(state::ACTIVE);
            out.push(WindowInfo {
                id,
                title: if d.acc.name.is_empty() {
                    app.name.clone()
                } else {
                    d.acc.name
                },
                bounds,
                focused: active,
                main: active,
                minimized: !d.acc.states.has(state::SHOWING),
                handle,
            });
        }
        Ok(out)
    }

    fn snapshot(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        opts: &SnapshotOptions,
    ) -> Result<Vec<RawNode>> {
        let root = self.resolve(window.handle)?;
        // Handles from this app's previous views are no longer needed.
        self.handles.retain(|_, (p, _)| *p != app.pid);
        let walked = self.a11y.walk(
            &root,
            opts.max_nodes,
            opts.max_depth,
            self.batch_size,
            self.text_max,
        );
        let mut out = Vec::with_capacity(walked.len());
        for w in walked {
            out.push(self.build_node(app.pid, w));
        }
        Ok(out)
    }

    fn configure(&mut self, cfg: &crate::config::Config) {
        self.batch_size = cfg.linux.batch_size.max(1);
        self.text_max = cfg.linux.text_max_chars.max(1);
        if let Some(x) = self.x11.as_mut() {
            x.restore_pointer = cfg.restore_pointer;
        }
    }

    fn user_idle(&mut self) -> Option<std::time::Duration> {
        self.x11.as_ref().and_then(|x| x.idle())
    }

    fn capture(&mut self, _app: &AppInfo, window: &WindowInfo) -> Result<Capture> {
        let rect = window
            .bounds
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Error::Platform("window has no on-screen bounds to capture".into()))?;
        self.x11()?.capture(rect)
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        let x11 = self.x11()?;
        let rect = region.unwrap_or_else(|| x11.root_rect());
        x11.capture(rect)
    }

    fn clipboard_get(&mut self) -> Result<String> {
        clipboard::get()
    }

    fn clipboard_set(&mut self, text: &str) -> Result<()> {
        clipboard::set(text)
    }

    fn perform_action(&mut self, element: ElementHandle, native_action: &str) -> Result<()> {
        let r = self.resolve(element)?;
        let idx = self.a11y.action_index(&r, native_action).ok_or_else(|| {
            Error::ActionFailed(format!("element no longer offers action `{native_action}`"))
        })?;
        let ok = self.a11y.do_action(&r, idx)?;
        if ok {
            Ok(())
        } else {
            Err(Error::ActionFailed(format!(
                "action `{native_action}` was not accepted"
            )))
        }
    }

    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()> {
        let r = self.resolve(element)?;
        let acc = self.a11y.describe(&r)?;
        if (acc.has_iface("EditableText") || acc.states.has(state::EDITABLE))
            && self.a11y.set_text(&r, value).unwrap_or(false)
        {
            return Ok(());
        }
        if acc.has_iface("Value")
            && let Ok(n) = value.trim().parse::<f64>()
            && self.a11y.set_value(&r, n).unwrap_or(false)
        {
            return Ok(());
        }
        // Checkbox/toggle: flip to the requested boolean via its action.
        let want = matches!(
            value.trim().to_lowercase().as_str(),
            "true" | "1" | "on" | "checked" | "yes"
        );
        let is = acc.states.has(state::CHECKED) || acc.states.has(state::PRESSED);
        if is != want {
            for action in ["toggle", "click", "press", "activate"] {
                if let Some(idx) = self.a11y.action_index(&r, action)
                    && self.a11y.do_action(&r, idx)?
                {
                    return Ok(());
                }
            }
        } else if acc.states.has(state::CHECKABLE) {
            return Ok(()); // already in the requested state
        }
        Err(Error::ActionFailed(
            "this element does not support setting a value directly".into(),
        ))
    }

    fn select_text(
        &mut self,
        element: ElementHandle,
        text: Option<&str>,
        occurrence: usize,
    ) -> Result<()> {
        let r = self.resolve(element)?;
        let count = self.a11y.character_count(&r);
        let (start, end) = match text {
            None => (0, count),
            Some(needle) => {
                let hay = self.a11y.get_text(&r, 0, count).unwrap_or_default();
                let chars: Vec<char> = hay.chars().collect();
                let needle_chars: Vec<char> = needle.chars().collect();
                let start =
                    find_nth(&chars, &needle_chars, occurrence.max(1)).ok_or_else(|| {
                        Error::ActionFailed(format!("`{needle}` not found in the text"))
                    })?;
                (start as i32, (start + needle_chars.len()) as i32)
            }
        };
        let _ = self.a11y.set_caret(&r, end);
        if self.a11y.set_selection(&r, start, end)? {
            Ok(())
        } else {
            Err(Error::ActionFailed("could not select the text".into()))
        }
    }

    fn focus(&mut self, element: ElementHandle) -> Result<Native> {
        let r = self.resolve(element)?;
        if self.a11y.grab_focus(&r).unwrap_or(false) {
            Ok(Native::Done("focused".into()))
        } else {
            Ok(Native::Unsupported)
        }
    }

    fn scroll_element(
        &mut self,
        _element: ElementHandle,
        _direction: ScrollDirection,
        _pages: f64,
    ) -> Result<Native> {
        // AT-SPI has no reliable directional scroll; fall back to the wheel.
        Ok(Native::Unsupported)
    }

    fn click(
        &mut self,
        _target: &InputTarget,
        at: Point,
        button: MouseButton,
        count: u8,
    ) -> Result<()> {
        let b = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        };
        self.x11()?.click(at.x as i32, at.y as i32, b, count)
    }

    fn drag(&mut self, _target: &InputTarget, from: Point, to: Point) -> Result<()> {
        self.x11()?
            .drag((from.x as i32, from.y as i32), (to.x as i32, to.y as i32))
    }

    fn scroll_wheel(&mut self, _target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()> {
        self.x11()?.scroll(at.x as i32, at.y as i32, dx, dy)
    }

    fn press_key(&mut self, _target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        self.x11()?.press(combo)
    }

    fn type_text(&mut self, _target: &InputTarget, text: &str) -> Result<()> {
        self.x11()?.type_text(text)
    }
}

fn find_nth(hay: &[char], needle: &[char], nth: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let mut seen = 0;
    for i in 0..=hay.len() - needle.len() {
        if hay[i..i + needle.len()] == *needle {
            seen += 1;
            if seen == nth {
                return Some(i);
            }
        }
    }
    None
}

fn stable_id(path: &str) -> u64 {
    // FNV-1a over the object path; stable for the app's lifetime.
    let mut h = 0xcbf29ce484222325u64;
    for b in path.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

fn proc_info(pid: u32) -> (Option<String>, Option<String>) {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    (exe, comm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_nth_occurrence() {
        let hay: Vec<char> = "abcabcabc".chars().collect();
        let needle: Vec<char> = "bc".chars().collect();
        assert_eq!(find_nth(&hay, &needle, 1), Some(1));
        assert_eq!(find_nth(&hay, &needle, 3), Some(7));
        assert_eq!(find_nth(&hay, &needle, 4), None);
    }

    #[test]
    fn stable_id_is_deterministic() {
        assert_eq!(stable_id("/org/a11y/x"), stable_id("/org/a11y/x"));
        assert_ne!(stable_id("/a"), stable_id("/b"));
    }
}
