//! macOS backend: the Accessibility (AX) API for the tree and semantic
//! actions, and CoreGraphics events posted to the target pid for background
//! input, with `CGWindowListCreateImage` for capturing background windows.

pub(crate) mod cg;
mod ffi;
mod notify;
mod ocr;
mod pasteboard;
mod wm;

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::{Duration, Instant};

use core_foundation::base::CFType;
use core_graphics::geometry::CGPoint;
use ffi::{AXError, AxRef};
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};

use crate::backend::{Backend, Native};
use crate::error::{Error, Result};
use crate::keys::KeyCombo;
use crate::roles;
use crate::types::*;

/// How long one tree walk may take before it returns what it has.
const WALK_DEADLINE: Duration = Duration::from_secs(10);

/// Messaging timeout (s) for an action: a press that opens a modal dialog
/// is only answered once the dialog closes, so don't wait long for that.
const ACTION_TIMEOUT: f32 = 1.0;

pub struct MacBackend {
    /// The system-wide AX element, kept for its messaging timeout: set on
    /// it, the timeout is the default for every element of every app (an
    /// app element's own setting doesn't reach its children).
    system_wide: Option<AxRef>,
    /// pid → application AX element.
    apps: HashMap<u32, AxRef>,
    /// handle → (pid, element).
    handles: HashMap<ElementHandle, (u32, AxRef)>,
    /// Window handles from the latest `list_windows` of each app. Kept
    /// apart so a snapshot (which renews the app's element handles) doesn't
    /// invalidate a window list callers still reuse.
    window_handles: HashMap<ElementHandle, (u32, AxRef)>,
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

impl Attrs {
    /// From the values of [`ATTRS`], in order.
    fn from_values(v: &[Option<CFType>]) -> Self {
        let bounds = match (ffi::value_to_point(&v[6]), ffi::value_to_size(&v[7])) {
            (Some(p), Some(s)) => Some(Rect::new(p.x, p.y, s.width, s.height)),
            _ => None,
        };
        Attrs {
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
        }
    }
}

/// The state of one tree walk: its deadline, and what stopped it early.
struct Walk {
    deadline: Instant,
    /// The app stopped answering (`kAXErrorCannotComplete`) or AX access
    /// was revoked (`kAXErrorAPIDisabled`): every further call would fail
    /// the same way (after waiting out the timeout), so the walk stops.
    failed: Option<AXError>,
    timed_out: bool,
}

impl Walk {
    fn new() -> Self {
        Self {
            deadline: Instant::now() + WALK_DEADLINE,
            failed: None,
            timed_out: false,
        }
    }

    /// Whether the walk must stop now.
    fn stopped(&mut self) -> bool {
        if self.failed.is_none() && !self.timed_out && Instant::now() >= self.deadline {
            self.timed_out = true;
        }
        self.failed.is_some() || self.timed_out
    }

    /// Note an AX error from the walk: a fatal one stops it.
    fn note(&mut self, err: AXError) {
        if ffi::is_fatal(err) && self.failed.is_none() {
            self.failed = Some(err);
        }
    }
}

/// Accessibility access is off (`kAXErrorAPIDisabled`, or not trusted).
fn accessibility_off() -> Error {
    Error::Permission(
        "Accessibility is turned off for this server: enable the app that runs it (your terminal or MCP client) under System Settings ▸ Privacy & Security ▸ Accessibility, then try again".into(),
    )
}

/// The app's name for messages.
fn app_name(pid: u32) -> String {
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
            .and_then(|a| a.localizedName())
            .map(|n| n.to_string())
    })
    .unwrap_or_else(|| format!("the app (pid {pid})"))
}

/// The error for an action or a change the app did not confirm: when it
/// didn't answer in time, it was sent and may have happened (never to be
/// repeated blindly); `refused` describes any other failure.
fn action_error(err: AXError, pid: u32, refused: impl FnOnce() -> String) -> Error {
    match err {
        ffi::kAXErrorCannotComplete => Error::Unanswered(app_name(pid)),
        ffi::kAXErrorAPIDisabled => accessibility_off(),
        _ => Error::ActionFailed(refused()),
    }
}

impl MacBackend {
    pub fn new() -> Result<Self> {
        let defaults = crate::config::MacosConfig::default();
        // SAFETY: creates the system-wide AX element (+1, owned).
        let system_wide = unsafe { AxRef::from_create(ffi::AXUIElementCreateSystemWide()) };
        let backend = Self {
            system_wide,
            apps: HashMap::new(),
            handles: HashMap::new(),
            window_handles: HashMap::new(),
            next_handle: 1,
            batch_attributes: defaults.batch_attributes,
            messaging_timeout: defaults.messaging_timeout_secs,
        };
        backend.apply_timeout();
        Ok(backend)
    }

    /// Set the messaging timeout globally (on the system-wide element) and
    /// on the app elements already made.
    fn apply_timeout(&self) {
        for el in self.system_wide.iter().chain(self.apps.values()) {
            // SAFETY: a live AX element.
            unsafe { ffi::AXUIElementSetMessagingTimeout(el.as_ref(), self.messaging_timeout) };
        }
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

    /// The element behind a handle, and its app's pid.
    fn resolve(&self, handle: ElementHandle) -> Result<(u32, AxRef)> {
        self.handles
            .get(&handle)
            .or_else(|| self.window_handles.get(&handle))
            .map(|(pid, el)| (*pid, el.clone()))
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

    /// The error for a read the app did not complete.
    fn read_error(&self, err: AXError, app: &str) -> Error {
        match err {
            ffi::kAXErrorAPIDisabled => accessibility_off(),
            ffi::kAXErrorCannotComplete => Error::ActionFailed(format!(
                "{app} is not responding: it did not answer the accessibility request within {:.1} s (it may be busy, hung, or showing a dialog). Wait a moment and try again.",
                self.messaging_timeout
            )),
            e => Error::Platform(format!("accessibility error {e} from {app}")),
        }
    }

    /// Read an element's attributes: one batched IPC call when enabled,
    /// falling back to per-attribute reads. Fails only with a fatal AX
    /// error ([`ffi::is_fatal`]), without the fallback: the app isn't
    /// answering (each read would wait out the timeout again) or AX access
    /// is off.
    fn read_attrs(&self, el: &AxRef) -> std::result::Result<Attrs, AXError> {
        let r = el.as_ref();
        if self.batch_attributes {
            match ffi::copy_attrs(r, &ATTRS) {
                Ok(v) => return Ok(Attrs::from_values(&v)),
                Err(e) if ffi::is_fatal(e) => return Err(e),
                Err(_) => {}
            }
        }
        let mut v = Vec::with_capacity(ATTRS.len());
        for name in ATTRS {
            v.push(ffi::read_attr(r, name)?);
        }
        Ok(Attrs::from_values(&v))
    }

    fn build_node(
        &mut self,
        pid: u32,
        el: &AxRef,
        a: Attrs,
        parent: Option<usize>,
        walk: &mut Walk,
    ) -> RawNode {
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
            && match ffi::try_is_settable(r, "AXValue") {
                Ok(s) => s,
                Err(e) => {
                    walk.note(e);
                    false
                }
            };
        let editable = text_role || (value_settable && a.value.is_some());

        // Never surface password contents; keep AXValue for non-checkable roles.
        let value = if checkable || role == "secure text field" {
            None
        } else {
            a.value
        };

        // Not asked once the app stopped answering (it would only time out).
        let names = match walk.failed {
            Some(_) => Vec::new(),
            None => ffi::action_names(r).unwrap_or_else(|e| {
                walk.note(e);
                Vec::new()
            }),
        };
        let actions = names
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

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        pid: u32,
        el: &AxRef,
        parent: Option<usize>,
        depth: usize,
        opts: &SnapshotOptions,
        state: &mut Walk,
        out: &mut Vec<RawNode>,
    ) {
        if out.len() >= opts.max_nodes || depth > opts.max_depth || state.stopped() {
            return;
        }
        let mut attrs = match self.read_attrs(el) {
            Ok(a) => a,
            Err(e) => {
                state.note(e);
                return;
            }
        };
        let idx = out.len();
        let children = std::mem::take(&mut attrs.children);
        let node = self.build_node(pid, el, attrs, parent, state);
        out.push(node);
        for child in children {
            if out.len() >= opts.max_nodes || state.stopped() {
                break;
            }
            self.walk(pid, &child, Some(idx), depth + 1, opts, state, out);
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
        self.apply_timeout();
    }

    fn permissions(&mut self) -> Vec<PermissionStatus> {
        let trusted = unsafe { ffi::AXIsProcessTrusted() } != 0;
        let mut out = vec![PermissionStatus {
            name: "Accessibility".into(),
            granted: trusted,
            detail: if trusted {
                "granted".into()
            } else {
                "not granted — enable this app under System Settings ▸ Privacy & Security ▸ Accessibility".into()
            },
        }];
        // Before macOS 10.15 there is no such permission.
        if let Some(granted) = ffi::screen_capture_allowed() {
            out.push(PermissionStatus {
                name: "Screen Recording".into(),
                granted,
                detail: if granted {
                    "granted".into()
                } else {
                    format!("not granted — {}", cg::SCREEN_RECORDING_HELP)
                },
            });
        }
        out
    }

    fn list_apps(&mut self) -> Result<Vec<AppInfo>> {
        let (out, alive) = autoreleasepool(|_| {
            // NSWorkspace updates its list of running apps from notifications
            // delivered on this thread's run loop, which nothing else runs here
            // (the server waits on stdin): let them in, or apps launched or
            // quit since the first call would never show.
            objc2_foundation::NSRunLoop::currentRunLoop().runUntilDate(
                &objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.01),
            );
            let ws = NSWorkspace::sharedWorkspace();
            let running = ws.runningApplications();
            let mut out = Vec::new();
            let mut alive = HashSet::new();
            for i in 0..running.count() {
                let app = running.objectAtIndex(i);
                let pid = app.processIdentifier();
                if pid <= 0 {
                    continue;
                }
                let pid = pid as u32;
                alive.insert(pid);
                if app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
                    continue;
                }
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
            (out, alive)
        });
        // Forget the elements of apps that quit (a pid may be reused).
        self.apps.retain(|pid, _| alive.contains(pid));
        self.handles.retain(|_, (pid, _)| alive.contains(pid));
        self.window_handles
            .retain(|_, (pid, _)| alive.contains(pid));
        Ok(out)
    }

    fn launch_app(&mut self, query: &str) -> Result<Option<String>> {
        // LaunchServices finds the app by its exact name (`open -a`), else by
        // bundle id (`open -b`). No `--args`: nothing of the query is passed on.
        let mut last = String::new();
        for flag in ["-a", "-b"] {
            match run_open(flag, query) {
                Ok(()) => return Ok(None),
                Err(e) => last = e,
            }
        }
        Err(Error::ActionFailed(format!(
            "no app named `{query}` (name or bundle id): {last}"
        )))
    }

    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let app_el = self.app_element(app.pid)?;
        // Asked first: an app that isn't answering is told at once, without
        // waiting out the timeout for every other attribute too.
        let windows = ffi::read_attr(app_el.as_ref(), "AXWindows")
            .map_err(|e| self.read_error(e, &app.name))?;
        let windows = ffi::value_to_elements(&windows);
        let main_id = ffi::copy_single_element(app_el.as_ref(), "AXMainWindow")
            .and_then(|m| ffi::window_id(m.as_ref()));
        let focused_id = ffi::copy_single_element(app_el.as_ref(), "AXFocusedWindow")
            .and_then(|f| ffi::window_id(f.as_ref()));

        // This listing replaces the app's previous window handles.
        self.window_handles.retain(|_, (p, _)| *p != app.pid);
        let mut out = Vec::new();
        for win in windows {
            let title = match ffi::read_attr(win.as_ref(), "AXTitle") {
                Ok(t) => ffi::value_to_string(&t).unwrap_or_default(),
                // It stopped answering: the windows listed so far are all.
                Err(e) if out.is_empty() => return Err(self.read_error(e, &app.name)),
                Err(_) => break,
            };
            let bounds = self.rect_of(&win);
            let minimized = ffi::copy_bool(win.as_ref(), "AXMinimized").unwrap_or(false);
            let cg_id = ffi::window_id(win.as_ref());
            let id = cg_id
                .map(u64::from)
                .unwrap_or_else(|| stable_id(&title, out.len()));
            let handle = self.next_handle;
            self.next_handle += 1;
            self.window_handles.insert(handle, (app.pid, win.clone()));
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
        let (_, root) = self.resolve(window.handle)?;
        // Reuse a fresh handle namespace for this app's elements (its window
        // handles are kept).
        self.drop_pid_handles(app.pid);
        let mut state = Walk::new();
        let mut out = Vec::new();
        self.walk(app.pid, &root, None, 0, opts, &mut state, &mut out);
        match state.failed {
            // Not even the window answered: say so rather than show nothing.
            Some(e) if out.is_empty() => return Err(self.read_error(e, &app.name)),
            Some(e) => log::warn!(
                "{} stopped answering (AX error {e}) after {} elements; the tree is partial",
                app.name,
                out.len()
            ),
            None if state.timed_out => log::warn!(
                "reading {}'s tree took over {} s; stopped at {} elements",
                app.name,
                WALK_DEADLINE.as_secs(),
                out.len()
            ),
            None => {}
        }
        Ok(out)
    }

    fn capture(&mut self, app: &AppInfo, window: &WindowInfo) -> Result<Capture> {
        let rect = window
            .bounds
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Error::Platform("window has no bounds to capture".into()))?;
        // Never the screen area instead: other windows may cover it.
        let id = self
            .resolve(window.handle)
            .ok()
            .and_then(|(_, win)| ffi::window_id(win.as_ref()))
            .or_else(|| cg::find_window(app.pid, rect))
            .ok_or_else(|| {
                Error::Platform(format!(
                    "could not find the window \"{}\" of {} among the screen's windows to capture it",
                    window.title, app.name
                ))
            })?;
        autoreleasepool(|_| cg::capture_window(id, rect))
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        autoreleasepool(|_| cg::capture_screen(region))
    }

    fn displays(&mut self) -> Result<Vec<Display>> {
        autoreleasepool(|_| wm::displays())
    }

    fn ocr(&mut self, cap: &Capture, languages: &[String]) -> Result<Vec<OcrLine>> {
        ocr::recognize(cap, languages)
    }

    fn notifications(&mut self) -> Result<Vec<Notification>> {
        autoreleasepool(|_| notify::recent())
    }

    fn window_op(&mut self, app: &AppInfo, window: &WindowInfo, op: &WindowOp) -> Result<()> {
        let (_, win) = self.resolve(window.handle)?;
        let app_el = self.app_element(app.pid)?;
        autoreleasepool(|_| wm::apply(win.as_ref(), app_el.as_ref(), app, op))
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
        autoreleasepool(|_| pasteboard::get())
    }

    fn clipboard_set(&mut self, text: &str) -> Result<()> {
        autoreleasepool(|_| pasteboard::set(text))
    }

    fn perform_action(&mut self, element: ElementHandle, native_action: &str) -> Result<()> {
        let (pid, el) = self.resolve(element)?;
        let r = el.as_ref();
        // SAFETY: a live AX element. A timeout of 0 afterwards puts it back
        // on the global one.
        unsafe {
            ffi::AXUIElementSetMessagingTimeout(r, self.messaging_timeout.min(ACTION_TIMEOUT))
        };
        let done = ffi::perform_action(r, native_action);
        unsafe { ffi::AXUIElementSetMessagingTimeout(r, 0.0) };
        done.map_err(|e| {
            action_error(e, pid, || {
                format!("the app rejected action `{native_action}` (AX error {e})")
            })
        })
    }

    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()> {
        let (pid, el) = self.resolve(element)?;
        let r = el.as_ref();
        let role_native = ffi::read_attr(r, "AXRole")
            .map_err(|e| self.read_error(e, &app_name(pid)))?
            .as_ref()
            .and_then(ffi::cftype_to_string)
            .unwrap_or_default();
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
            if is != want {
                ffi::perform_action(r, "AXPress")
                    .map_err(|e| action_error(e, pid, || "could not toggle the control".into()))?;
            }
            return Ok(());
        }
        if ffi::is_settable(r, "AXValue") {
            match ffi::set_string(r, "AXValue", value) {
                Ok(()) => return Ok(()),
                Err(e) if ffi::is_fatal(e) => return Err(action_error(e, pid, String::new)),
                Err(_) => {}
            }
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
        let (pid, el) = self.resolve(element)?;
        let r = el.as_ref();
        let content = ffi::copy_string(r, "AXValue").unwrap_or_default();
        // AXSelectedTextRange counts UTF-16 code units, like NSString.
        let (loc, len) = match text {
            None => (0isize, content.encode_utf16().count() as isize),
            Some(needle) => utf16_range(&content, needle, occurrence.max(1))
                .map(|(l, n)| (l as isize, n as isize))
                .ok_or_else(|| Error::ActionFailed(format!("`{needle}` not found in the text")))?,
        };
        ffi::set_range(r, "AXSelectedTextRange", loc, len)
            .map_err(|e| action_error(e, pid, || "could not set the text selection".into()))
    }

    fn focus(&mut self, element: ElementHandle) -> Result<Native> {
        let (pid, el) = self.resolve(element)?;
        match ffi::set_bool(el.as_ref(), "AXFocused", true) {
            Ok(()) => Ok(Native::Done("focused".into())),
            Err(e) if ffi::is_fatal(e) => Err(action_error(e, pid, String::new)),
            Err(_) => Ok(Native::Unsupported),
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

    fn move_pointer(&mut self, target: &InputTarget, at: Point) -> Result<Option<Point>> {
        cg::hover(target.pid, CGPoint { x: at.x, y: at.y })?;
        Ok(None)
    }

    fn draw(
        &mut self,
        target: &InputTarget,
        strokes: &[Vec<Point>],
        button: MouseButton,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        let strokes: Vec<Vec<CGPoint>> = strokes
            .iter()
            .map(|s| s.iter().map(|p| CGPoint { x: p.x, y: p.y }).collect())
            .collect();
        cg::draw(target.pid, &strokes, button, pace)
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

/// `open <flag> <app>`, waiting (up to 10 s) for its answer so an unknown
/// app is an error, not a silent no-op.
fn run_open(flag: &str, app: &str) -> std::result::Result<(), String> {
    use std::io::Read as _;
    use std::process::Stdio;
    let mut child = Command::new("open")
        .arg(flag)
        .arg(app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut err);
                }
                let err = err.trim();
                return Err(if err.is_empty() {
                    status.to_string()
                } else {
                    err.to_string()
                });
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Still opening (a slow app): it was found; let it finish.
            Ok(None) => {
                let _ = std::thread::Builder::new()
                    .name("reap-open".into())
                    .spawn(move || child.wait());
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// The `nth` (1-based) occurrence of `needle` in `hay`, as a
/// (location, length) range in UTF-16 code units (NSString indexing).
fn utf16_range(hay: &str, needle: &str, nth: usize) -> Option<(usize, usize)> {
    let hay: Vec<u16> = hay.encode_utf16().collect();
    let needle: Vec<u16> = needle.encode_utf16().collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle.as_slice())
        .nth(nth.checked_sub(1)?)
        .map(|(i, _)| (i, needle.len()))
}

fn stable_id(title: &str, index: usize) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in title.bytes().chain(std::iter::once(index as u8)) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::utf16_range;

    #[test]
    fn selection_ranges_count_utf16_units() {
        assert_eq!(utf16_range("abcabc", "bc", 2), Some((4, 2)));
        assert_eq!(utf16_range("abcabc", "bc", 3), None);
        // An emoji is 2 UTF-16 units (1 char): offsets after it shift by 2.
        assert_eq!(utf16_range("\u{1F600} hi", "hi", 1), Some((3, 2)));
        assert_eq!(utf16_range("a\u{1F600}b", "\u{1F600}", 1), Some((1, 2)));
        assert_eq!(utf16_range("abc", "", 1), None);
        assert_eq!(utf16_range("abc", "a", 0), None);
    }
}
