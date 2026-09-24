//! macOS backend: the Accessibility (AX) API for the tree and semantic
//! actions, and CoreGraphics events posted to the target pid for background
//! input, with `CGWindowListCreateImage` for capturing background windows.

mod cg;
mod ffi;

use std::collections::HashMap;
use std::process::Command;

use core_graphics::geometry::CGPoint;
use ffi::AxRef;
use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};

use crate::backend::{Backend, Native};
use crate::error::{Error, Result};
use crate::keys::KeyCombo;
use crate::roles;
use crate::types::*;

/// Attribute-timeout so a hung app can't block us for long.
const MESSAGING_TIMEOUT: f32 = 2.0;

pub struct MacBackend {
    /// pid → application AX element.
    apps: HashMap<u32, AxRef>,
    /// handle → (pid, element).
    handles: HashMap<ElementHandle, (u32, AxRef)>,
    next_handle: ElementHandle,
}

impl MacBackend {
    pub fn new() -> Result<Self> {
        Ok(Self {
            apps: HashMap::new(),
            handles: HashMap::new(),
            next_handle: 1,
        })
    }

    fn app_element(&mut self, pid: u32) -> Result<AxRef> {
        if let Some(a) = self.apps.get(&pid) {
            return Ok(a.clone());
        }
        let raw = unsafe { ffi::AXUIElementCreateApplication(pid as i32) };
        let el = unsafe { AxRef::from_create(raw) }
            .ok_or_else(|| Error::AppNotFound(format!("pid {pid}")))?;
        unsafe { ffi::AXUIElementSetMessagingTimeout(el.as_ref(), MESSAGING_TIMEOUT) };
        self.apps.insert(pid, el.clone());
        Ok(el)
    }

    fn handle_for(&mut self, pid: u32, el: AxRef) -> ElementHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(h, (pid, el));
        h
    }

    fn resolve(&self, handle: ElementHandle) -> Result<AxRef> {
        self.handles
            .get(&handle)
            .map(|(_, el)| el.clone())
            .ok_or_else(|| Error::Internal(format!("stale element handle {handle}")))
    }

    fn drop_pid_handles(&mut self, pid: u32) {
        self.handles.retain(|_, (p, _)| *p != pid);
    }

    fn rect_of(&self, el: &AxRef) -> Option<Rect> {
        let p = ffi::copy_point(el.as_ref(), "AXPosition")?;
        let s = ffi::copy_size(el.as_ref(), "AXSize")?;
        Some(Rect::new(p.x, p.y, s.width, s.height))
    }

    fn build_node(&mut self, pid: u32, el: &AxRef, parent: Option<usize>) -> RawNode {
        let r = el.as_ref();
        let role_native = ffi::copy_string(r, "AXRole").unwrap_or_default();
        let subrole = ffi::copy_string(r, "AXSubrole");
        let role = roles::from_ax(&role_native, subrole.as_deref());

        let name = ffi::copy_string(r, "AXTitle").filter(|s| !s.is_empty());
        let description = ffi::copy_string(r, "AXDescription").filter(|s| !s.is_empty());
        let raw_value = ffi::copy_string(r, "AXValue").filter(|s| !s.is_empty());
        let placeholder = ffi::copy_string(r, "AXPlaceholderValue").filter(|s| !s.is_empty());

        let bounds = self.rect_of(el);
        let enabled = ffi::copy_bool(r, "AXEnabled").unwrap_or(true);
        let focused = ffi::copy_bool(r, "AXFocused").unwrap_or(false);
        let selected = ffi::copy_bool(r, "AXSelected").unwrap_or(false);
        let expanded = ffi::copy_bool(r, "AXExpanded");
        let hidden =
            ffi::copy_bool(r, "AXHidden").unwrap_or(false) || bounds.is_some_and(|b| b.is_empty());

        let checkable = matches!(
            role.as_str(),
            "checkbox" | "radio button" | "toggle button" | "switch"
        );
        let checked = checkable
            .then(|| raw_value.as_deref() == Some("1") || raw_value.as_deref() == Some("true"));

        let editable = matches!(
            role.as_str(),
            "text field" | "text area" | "secure text field"
        ) || ffi::is_settable(r, "AXValue") && raw_value.is_some();
        let value_settable = ffi::is_settable(r, "AXValue");

        // Keep AXValue as the element value for non-checkable roles.
        let value = if checkable { None } else { raw_value };

        let actions = ffi::action_names(r)
            .into_iter()
            .map(|native| ActionDesc::new(roles::ax_action(&native), native))
            .collect();

        let handle = self.handle_for(pid, el.clone());
        RawNode {
            handle,
            parent,
            key: None, // structural key assigned by the tree pruner
            role,
            native_role: role_native,
            name,
            description,
            value,
            placeholder,
            identifier: ffi::copy_string(r, "AXIdentifier").filter(|s| !s.is_empty()),
            bounds,
            actions,
            states: NodeStates {
                enabled,
                focused,
                selected,
                checked,
                expanded,
                editable,
                value_settable,
                hidden,
            },
        }
    }

    fn walk(
        &mut self,
        pid: u32,
        el: &AxRef,
        parent: Option<usize>,
        depth: usize,
        opts: &SnapshotOptions,
        out: &mut Vec<RawNode>,
    ) {
        if out.len() >= opts.max_nodes || depth > opts.max_depth {
            return;
        }
        let idx = out.len();
        let node = self.build_node(pid, el, parent);
        out.push(node);
        for child in ffi::copy_elements(el.as_ref(), "AXChildren") {
            if out.len() >= opts.max_nodes {
                break;
            }
            self.walk(pid, &child, Some(idx), depth + 1, opts, out);
        }
    }
}

impl Backend for MacBackend {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn permissions(&mut self) -> Vec<PermissionStatus> {
        let trusted = unsafe { ffi::AXIsProcessTrusted() } != 0;
        vec![PermissionStatus {
            name: "Accessibility".into(),
            granted: trusted,
            detail: if trusted {
                "granted".into()
            } else {
                "not granted — enable this app under System Settings ▸ Privacy & Security ▸ Accessibility (and Screen Recording for screenshots)".into()
            },
        }]
    }

    fn list_apps(&mut self) -> Result<Vec<AppInfo>> {
        let ws = NSWorkspace::sharedWorkspace();
        let running = ws.runningApplications();
        let mut out = Vec::new();
        for i in 0..running.count() {
            let app = running.objectAtIndex(i);
            if app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
                continue;
            }
            let pid = app.processIdentifier();
            if pid <= 0 {
                continue;
            }
            let pid = pid as u32;
            let name = app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("pid {pid}"));
            let bundle = app.bundleIdentifier().map(|s| s.to_string());
            let exe = app
                .executableURL()
                .and_then(|u| u.path())
                .map(|s| s.to_string());
            out.push(AppInfo {
                id: bundle.unwrap_or_else(|| name.clone()),
                name,
                pid,
                exe,
                frontmost: app.isActive(),
                hidden: app.isHidden(),
            });
        }
        Ok(out)
    }

    fn launch_app(&mut self, query: &str) -> Result<()> {
        // `open -a Name` (or `-b bundle.id` when it looks like a bundle id).
        let flag = if query.contains('.') && !query.contains(' ') {
            "-b"
        } else {
            "-a"
        };
        Command::new("open")
            .arg(flag)
            .arg(query)
            .spawn()
            .map(|_| ())
            .map_err(|e| Error::ActionFailed(format!("could not launch `{query}`: {e}")))
    }

    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let app_el = self.app_element(app.pid)?;
        let main_id = ffi::copy_single_element(app_el.as_ref(), "AXMainWindow")
            .and_then(|m| ffi::window_id(m.as_ref()));
        let focused_id = ffi::copy_single_element(app_el.as_ref(), "AXFocusedWindow")
            .and_then(|f| ffi::window_id(f.as_ref()));

        let mut out = Vec::new();
        for win in ffi::copy_elements(app_el.as_ref(), "AXWindows") {
            let title = ffi::copy_string(win.as_ref(), "AXTitle").unwrap_or_default();
            let bounds = self.rect_of(&win);
            let minimized = ffi::copy_bool(win.as_ref(), "AXMinimized").unwrap_or(false);
            let cg_id = ffi::window_id(win.as_ref());
            let id = cg_id
                .map(u64::from)
                .unwrap_or_else(|| stable_id(&title, out.len()));
            let handle = self.handle_for(app.pid, win.clone());
            out.push(WindowInfo {
                id,
                title: if title.is_empty() {
                    app.name.clone()
                } else {
                    title
                },
                bounds,
                focused: cg_id.is_some() && cg_id == focused_id,
                main: cg_id.is_some() && cg_id == main_id,
                minimized,
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
        // Reuse a fresh handle namespace for this app's elements.
        self.drop_pid_handles(app.pid);
        let mut out = Vec::new();
        self.walk(app.pid, &root, None, 0, opts, &mut out);
        Ok(out)
    }

    fn capture(&mut self, _app: &AppInfo, window: &WindowInfo) -> Result<Capture> {
        let rect = window
            .bounds
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Error::Platform("window has no bounds to capture".into()))?;
        if let Ok(win) = self.resolve(window.handle) {
            if let Some(id) = ffi::window_id(win.as_ref()) {
                return cg::capture_window(id, rect);
            }
        }
        cg::capture(rect)
    }

    fn perform_action(&mut self, element: ElementHandle, native_action: &str) -> Result<()> {
        let el = self.resolve(element)?;
        if ffi::perform_action(el.as_ref(), native_action) {
            Ok(())
        } else {
            Err(Error::ActionFailed(format!(
                "the app rejected action `{native_action}`"
            )))
        }
    }

    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()> {
        let el = self.resolve(element)?;
        let r = el.as_ref();
        let role_native = ffi::copy_string(r, "AXRole").unwrap_or_default();
        let role = roles::from_ax(&role_native, ffi::copy_string(r, "AXSubrole").as_deref());
        if matches!(
            role.as_str(),
            "checkbox" | "radio button" | "toggle button" | "switch"
        ) {
            let want = matches!(
                value.trim().to_lowercase().as_str(),
                "true" | "1" | "on" | "yes" | "checked"
            );
            let is = matches!(
                ffi::copy_string(r, "AXValue").as_deref(),
                Some("1") | Some("true")
            );
            if is != want && !ffi::perform_action(r, "AXPress") {
                return Err(Error::ActionFailed("could not toggle the control".into()));
            }
            return Ok(());
        }
        if ffi::is_settable(r, "AXValue") && ffi::set_string(r, "AXValue", value) {
            return Ok(());
        }
        Err(Error::ActionFailed(
            "this element does not accept a value directly; try click + type_text".into(),
        ))
    }

    fn select_text(
        &mut self,
        element: ElementHandle,
        text: Option<&str>,
        occurrence: usize,
    ) -> Result<()> {
        let el = self.resolve(element)?;
        let r = el.as_ref();
        let content = ffi::copy_string(r, "AXValue").unwrap_or_default();
        let chars: Vec<char> = content.chars().collect();
        let (loc, len) = match text {
            None => (0isize, chars.len() as isize),
            Some(needle) => {
                let needle: Vec<char> = needle.chars().collect();
                let start = find_nth(&chars, &needle, occurrence.max(1)).ok_or_else(|| {
                    Error::ActionFailed(format!("`{}` not found in the text", text.unwrap()))
                })?;
                (start as isize, needle.len() as isize)
            }
        };
        if ffi::set_range(r, "AXSelectedTextRange", loc, len) {
            Ok(())
        } else {
            Err(Error::ActionFailed(
                "could not set the text selection".into(),
            ))
        }
    }

    fn focus(&mut self, element: ElementHandle) -> Result<Native> {
        let el = self.resolve(element)?;
        if ffi::set_bool(el.as_ref(), "AXFocused", true) {
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
        Ok(Native::Unsupported)
    }

    fn click(
        &mut self,
        target: &InputTarget,
        at: Point,
        button: MouseButton,
        count: u8,
    ) -> Result<()> {
        cg::click(target.pid, CGPoint { x: at.x, y: at.y }, button, count)
    }

    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()> {
        cg::drag(
            target.pid,
            CGPoint {
                x: from.x,
                y: from.y,
            },
            CGPoint { x: to.x, y: to.y },
        )
    }

    fn scroll_wheel(&mut self, target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()> {
        cg::scroll(target.pid, CGPoint { x: at.x, y: at.y }, dx, dy)
    }

    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        cg::press(target.pid, combo)
    }

    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()> {
        cg::type_text(target.pid, text)
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

fn stable_id(title: &str, index: usize) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in title.bytes().chain(std::iter::once(index as u8)) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}
