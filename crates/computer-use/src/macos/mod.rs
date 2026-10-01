//! macOS backend: the Accessibility (AX) API for the tree and semantic
//! actions, and CoreGraphics events posted to the target pid for background
//! input, with `CGWindowListCreateImage` for capturing background windows.

pub(crate) mod cg;
mod ffi;
mod notify;
mod ocr;
mod pasteboard;
mod wm;

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

pub struct MacBackend {
    /// pid → application AX element.
    apps: HashMap<u32, AxRef>,
    /// handle → (pid, element).
    handles: HashMap<ElementHandle, (u32, AxRef)>,
    next_handle: ElementHandle,
    /// Read all of an element's attributes in one AX call.
    batch_attributes: bool,
    messaging_timeout: f32,
}

/// Attributes read per element, in this order, in one batched AX call.
const ATTRS: [&str; 15] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXPlaceholderValue",
    "AXPosition",
    "AXSize",
    "AXEnabled",
    "AXFocused",
    "AXSelected",
    "AXExpanded",
    "AXHidden",
    "AXIdentifier",
    "AXChildren",
];

/// One element's attributes.
struct Attrs {
    role: String,
    subrole: Option<String>,
    title: Option<String>,
    description: Option<String>,
    value: Option<String>,
    placeholder: Option<String>,
    bounds: Option<Rect>,
    enabled: bool,
    focused: bool,
    selected: bool,
    expanded: Option<bool>,
    hidden: bool,
    identifier: Option<String>,
    children: Vec<AxRef>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

impl MacBackend {
    pub fn new() -> Result<Self> {
        let defaults = crate::config::MacosConfig::default();
        Ok(Self {
            apps: HashMap::new(),
            handles: HashMap::new(),
            next_handle: 1,
            batch_attributes: defaults.batch_attributes,
            messaging_timeout: defaults.messaging_timeout_secs,
        })
    }

    fn app_element(&mut self, pid: u32) -> Result<AxRef> {
        if let Some(a) = self.apps.get(&pid) {
            return Ok(a.clone());
        }
        let raw = unsafe { ffi::AXUIElementCreateApplication(pid as i32) };
        let el = unsafe { AxRef::from_create(raw) }
            .ok_or_else(|| Error::AppNotFound(format!("pid {pid}")))?;
        unsafe { ffi::AXUIElementSetMessagingTimeout(el.as_ref(), self.messaging_timeout) };
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

    /// Read an element's attributes: one batched IPC call when enabled,
    /// falling back to per-attribute reads.
    fn read_attrs(&self, el: &AxRef) -> Attrs {
        let r = el.as_ref();
        if self.batch_attributes
            && let Some(v) = ffi::copy_attrs(r, &ATTRS)
        {
            let bounds = match (ffi::value_to_point(&v[6]), ffi::value_to_size(&v[7])) {
                (Some(p), Some(s)) => Some(Rect::new(p.x, p.y, s.width, s.height)),
                _ => None,
            };
            return Attrs {
                role: ffi::value_to_string(&v[0]).unwrap_or_default(),
                subrole: ffi::value_to_string(&v[1]),
                title: non_empty(ffi::value_to_string(&v[2])),
                description: non_empty(ffi::value_to_string(&v[3])),
                value: non_empty(ffi::value_to_string(&v[4])),
                placeholder: non_empty(ffi::value_to_string(&v[5])),
                bounds,
                enabled: ffi::value_to_bool(&v[8]).unwrap_or(true),
                focused: ffi::value_to_bool(&v[9]).unwrap_or(false),
                selected: ffi::value_to_bool(&v[10]).unwrap_or(false),
                expanded: ffi::value_to_bool(&v[11]),
                hidden: ffi::value_to_bool(&v[12]).unwrap_or(false),
                identifier: non_empty(ffi::value_to_string(&v[13])),
                children: ffi::value_to_elements(&v[14]),
            };
        }
        Attrs {
            role: ffi::copy_string(r, "AXRole").unwrap_or_default(),
            subrole: ffi::copy_string(r, "AXSubrole"),
            title: non_empty(ffi::copy_string(r, "AXTitle")),
            description: non_empty(ffi::copy_string(r, "AXDescription")),
            value: non_empty(ffi::copy_string(r, "AXValue")),
            placeholder: non_empty(ffi::copy_string(r, "AXPlaceholderValue")),
            bounds: self.rect_of(el),
            enabled: ffi::copy_bool(r, "AXEnabled").unwrap_or(true),
            focused: ffi::copy_bool(r, "AXFocused").unwrap_or(false),
            selected: ffi::copy_bool(r, "AXSelected").unwrap_or(false),
            expanded: ffi::copy_bool(r, "AXExpanded"),
            hidden: ffi::copy_bool(r, "AXHidden").unwrap_or(false),
            identifier: non_empty(ffi::copy_string(r, "AXIdentifier")),
            children: ffi::copy_elements(r, "AXChildren"),
        }
    }

    fn build_node(&mut self, pid: u32, el: &AxRef, a: Attrs, parent: Option<usize>) -> RawNode {
        let r = el.as_ref();
        let role = roles::from_ax(&a.role, a.subrole.as_deref());
        let hidden = a.hidden || a.bounds.is_some_and(|b| b.is_empty());

        let checkable = matches!(
            role.as_str(),
            "checkbox" | "radio button" | "toggle button" | "switch"
        );
        let checked = checkable
            .then(|| a.value.as_deref() == Some("1") || a.value.as_deref() == Some("true"));

        let text_role = matches!(
            role.as_str(),
            "text field" | "text area" | "secure text field" | "search field" | "combo box"
        );
        // Settability is its own IPC call, so only ask where it matters.
        let value_settable = (text_role
            || matches!(
                role.as_str(),
                "slider" | "stepper" | "date field" | "time field"
            ))
            && ffi::is_settable(r, "AXValue");
        let editable = text_role || (value_settable && a.value.is_some());

        // Never surface password contents; keep AXValue for non-checkable roles.
        let value = if checkable || role == "secure text field" {
            None
        } else {
            a.value
        };

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
            native_role: a.role,
            name: a.title,
            description: a.description,
            value,
            placeholder: a.placeholder,
            identifier: a.identifier,
            bounds: a.bounds,
            actions,
            states: NodeStates {
                enabled: a.enabled,
                focused: a.focused,
                selected: a.selected,
                checked,
                expanded: a.expanded,
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
        let mut attrs = self.read_attrs(el);
        let children = std::mem::take(&mut attrs.children);
        let node = self.build_node(pid, el, attrs, parent);
        out.push(node);
        for child in children {
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

    fn configure(&mut self, cfg: &crate::config::Config) {
        self.batch_attributes = cfg.macos.batch_attributes;
        self.messaging_timeout = cfg.macos.messaging_timeout_secs.max(0.1);
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
        let mut cmd = Command::new("open");
        cmd.arg(flag).arg(query);
        crate::backend::spawn_detached(cmd)
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
        cg::capture_screen(Some(rect))
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        cg::capture_screen(region)
    }

    fn displays(&mut self) -> Result<Vec<Display>> {
        wm::displays()
    }

    fn ocr(&mut self, cap: &Capture, languages: &[String]) -> Result<Vec<OcrLine>> {
        ocr::recognize(cap, languages)
    }

    fn notifications(&mut self) -> Result<Vec<Notification>> {
        notify::recent()
    }

    fn window_op(&mut self, app: &AppInfo, window: &WindowInfo, op: &WindowOp) -> Result<()> {
        let win = self.resolve(window.handle)?;
        wm::apply(win.as_ref(), app.pid, op)
    }

    fn input_needs_front(&self) -> bool {
        // Events are posted to the target app's process (CGEventPostToPid).
        false
    }

    fn user_idle(&mut self) -> Option<std::time::Duration> {
        // SAFETY: a plain query of the HID system's idle time.
        let secs = unsafe {
            ffi::CGEventSourceSecondsSinceLastEventType(
                ffi::kCGEventSourceStateHIDSystemState,
                ffi::kCGAnyInputEventType,
            )
        };
        (secs.is_finite() && secs >= 0.0).then(|| std::time::Duration::from_secs_f64(secs))
    }

    fn clipboard_get(&mut self) -> Result<String> {
        pasteboard::get()
    }

    fn clipboard_set(&mut self, text: &str) -> Result<()> {
        pasteboard::set(text)
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
