//! macOS backend: the Accessibility (AX) API for the tree and semantic
//! actions, and CoreGraphics events posted to the target pid for background
//! input, with `CGWindowListCreateImage` for capturing background windows.

pub(crate) mod cg;
mod ffi;
mod layout;
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
    /// pid → the CGWindowIDs its latest `list_windows` found, to tell a
    /// real window id from a made-up one in an [`InputTarget`].
    cg_windows: HashMap<u32, HashSet<u32>>,
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
    /// The element the walk started at no longer exists.
    root_gone: bool,
}

impl Walk {
    fn new() -> Self {
        Self {
            deadline: Instant::now() + WALK_DEADLINE,
            failed: None,
            timed_out: false,
            root_gone: false,
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

/// An error after which an element is not read further: it no longer
/// exists, or the app isn't answering ([`ffi::is_fatal`]).
fn ends_walk(err: AXError) -> bool {
    err == ffi::kAXErrorInvalidUIElement || ffi::is_fatal(err)
}

/// The error for a read the app did not complete: `timeout` is the
/// messaging timeout (s).
fn read_error(err: AXError, app: &str, timeout: f32) -> Error {
    match err {
        ffi::kAXErrorAPIDisabled => accessibility_off(),
        ffi::kAXErrorCannotComplete => Error::ActionFailed(format!(
            "{app} is not responding: it did not answer the accessibility request within {timeout:.1} s (it may be busy, hung, or showing a dialog). Wait a moment and try again."
        )),
        ffi::kAXErrorCannotCompleteAtOnce => Error::ActionFailed(format!(
            "{app} could not answer the accessibility request right now (it may still be starting up, or this part of it is drawn by another process). Try again in a moment."
        )),
        ffi::kAXErrorInvalidUIElement => Error::ActionFailed(format!(
            "this part of {app} no longer exists (its window closed, or the app quit). Call get_app_state again."
        )),
        e => match ffi::describe(e) {
            Some(what) => Error::Platform(format!("{app}: {what} (accessibility error {e})")),
            None => Error::Platform(format!("accessibility error {e} from {app}")),
        },
    }
}

/// The app's name for messages.
fn app_name(pid: u32) -> String {
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
            .and_then(|a| a.localizedName())
            .map(|n| ffi::nsstring_text(&n))
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
            cg_windows: HashMap::new(),
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
        ffi::set_messaging_timeout(self.messaging_timeout);
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
        // Chromium and Electron apps (VS Code, Slack…) build their tree
        // only for assistive apps that ask: AXManualAccessibility does,
        // without AXEnhancedUserInterface's slow window animations. Other
        // apps refuse or ignore it. Asked once per app element (per pid),
        // without waiting long on an app that doesn't answer.
        // SAFETY: a live AX element.
        unsafe {
            ffi::AXUIElementSetMessagingTimeout(el.as_ref(), self.messaging_timeout.min(0.5))
        };
        let enabled = ffi::set_bool(el.as_ref(), "AXManualAccessibility", true).is_ok();
        unsafe { ffi::AXUIElementSetMessagingTimeout(el.as_ref(), self.messaging_timeout) };
        self.apps.insert(pid, el.clone());
        if enabled {
            // Let it start building the tree; a first snapshot may still
            // be thin, the next one fuller.
            log::debug!("enabled AXManualAccessibility for pid {pid}");
            std::thread::sleep(Duration::from_millis(150));
        }
        Ok(el)
    }

    /// The target's window as a CGWindowID, when it is one the app's
    /// latest window list found (window ids without one are made up).
    fn cg_window(&self, target: &InputTarget) -> Option<u32> {
        let id = u32::try_from(target.window_id?).ok()?;
        self.cg_windows
            .get(&target.pid)
            .is_some_and(|ids| ids.contains(&id))
            .then_some(id)
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
        read_error(err, app, self.messaging_timeout)
    }

    /// Read an element's attributes: one batched IPC call when enabled,
    /// falling back to per-attribute reads. Fails only with a fatal AX
    /// error ([`ffi::is_fatal`]), without the fallback: the app isn't
    /// answering (each read would wait out the timeout again) or AX access
    /// is off.
    ///
    /// An element that no longer exists (`kAXErrorInvalidUIElement`: its
    /// window closed) is an error too, never a node without a role.
    fn read_attrs(&self, el: &AxRef) -> std::result::Result<Attrs, AXError> {
        let r = el.as_ref();
        if self.batch_attributes {
            match ffi::copy_attrs(r, &ATTRS) {
                Ok(mut v) => {
                    // No role: ask for it alone, to tell a vanished element.
                    if v[0].is_none() {
                        match ffi::try_copy_attr(r, ATTRS[0]) {
                            Ok(role) => v[0] = Some(role),
                            Err(e) if ends_walk(e) => return Err(e),
                            Err(_) => {}
                        }
                    }
                    return Ok(Attrs::from_values(&v));
                }
                Err(e) if ends_walk(e) => return Err(e),
                Err(_) => {}
            }
        }
        let mut v = Vec::with_capacity(ATTRS.len());
        v.push(match ffi::try_copy_attr(r, ATTRS[0]) {
            Ok(role) => Some(role),
            Err(e) if ends_walk(e) => return Err(e),
            Err(_) => None,
        });
        for name in &ATTRS[1..] {
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
            // Gone (closed meanwhile): skipped, as is all under it.
            Err(e) => {
                if depth == 0 && e == ffi::kAXErrorInvalidUIElement {
                    state.root_gone = true;
                }
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
        cg::set_natural(cfg.natural_mouse);
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
                    .map(|s| ffi::nsstring_text(&s))
                    .unwrap_or_else(|| format!("pid {pid}"));
                let bundle = app.bundleIdentifier().map(|s| ffi::nsstring_text(&s));
                let exe = app
                    .executableURL()
                    .and_then(|u| u.path())
                    .map(|s| ffi::nsstring_text(&s));
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
        self.cg_windows.retain(|pid, _| alive.contains(pid));
        self.handles.retain(|_, (pid, _)| alive.contains(pid));
        self.window_handles
            .retain(|_, (pid, _)| alive.contains(pid));
        Ok(out)
    }

    fn launch_app(&mut self, query: &str) -> Result<Option<String>> {
        // LaunchServices finds the app by its exact name (`open -a`), else by
        // bundle id (`open -b`). No `--args`: nothing of the query is passed on.
        // The app's bundle id is returned: it is the `id` list_apps shows,
        // so the engine recognises the app by it even when its process is
        // named otherwise ("Visual Studio Code" runs as "Code").
        let mut last = String::new();
        for flag in ["-a", "-b"] {
            match run_open(flag, query) {
                Ok(()) if flag == "-b" => return Ok(Some(query.to_string())),
                Ok(()) => return Ok(autoreleasepool(|_| bundle_id_of(query))),
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
        if windows.is_empty() {
            // AX lists only the windows on the current Space.
            let elsewhere = cg::offscreen_windows(app.pid);
            if elsewhere > 0 {
                return Err(Error::ActionFailed(other_space(app, elsewhere)));
            }
        }
        let main_id = ffi::copy_single_element(app_el.as_ref(), "AXMainWindow")
            .and_then(|m| ffi::window_id(m.as_ref()));
        let focused_id = ffi::copy_single_element(app_el.as_ref(), "AXFocusedWindow")
            .and_then(|f| ffi::window_id(f.as_ref()));

        // This listing replaces the app's previous window handles, once it
        // has worked (a failed one leaves them as they were).
        let mut handles = Vec::new();
        let mut cg_ids = HashSet::new();
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
            cg_ids.extend(cg_id);
            let id = cg_id
                .map(u64::from)
                .unwrap_or_else(|| stable_id(&title, out.len()));
            let handle = self.next_handle;
            self.next_handle += 1;
            handles.push((handle, (app.pid, win.clone())));
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
        self.window_handles.retain(|_, (p, _)| *p != app.pid);
        self.window_handles.extend(handles);
        self.cg_windows.insert(app.pid, cg_ids);
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
        if state.root_gone {
            self.window_handles.remove(&window.handle);
            return Err(Error::ActionFailed(format!(
                "the window \"{}\" of {} was closed. Call get_app_state again to see its other windows.",
                window.title, app.name
            )));
        }
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
                let why = ffi::describe(e)
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default();
                format!("the app rejected action `{native_action}`{why} (AX error {e})")
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
        // A value that can't be read is not an empty one (selecting 0..0
        // reported success with nothing selected).
        let content = ffi::copy_string(r, "AXValue").ok_or_else(|| {
            Error::ActionFailed("couldn't read this element's text to select it".into())
        })?;
        if content.is_empty() && text.is_none() {
            return Err(Error::ActionFailed(
                "there is no text here to select".into(),
            ));
        }
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
        let window = self.cg_window(target);
        cg::click(
            target.pid,
            window,
            CGPoint { x: at.x, y: at.y },
            button,
            count,
        )
    }

    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()> {
        cg::drag(
            target.pid,
            self.cg_window(target),
            CGPoint {
                x: from.x,
                y: from.y,
            },
            CGPoint { x: to.x, y: to.y },
        )
    }

    fn move_pointer(&mut self, target: &InputTarget, at: Point) -> Result<Option<Point>> {
        cg::hover(
            target.pid,
            self.cg_window(target),
            CGPoint { x: at.x, y: at.y },
        )?;
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
        cg::draw(target.pid, self.cg_window(target), &strokes, button, pace)
    }

    fn scroll_wheel(&mut self, target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()> {
        cg::scroll(
            target.pid,
            self.cg_window(target),
            CGPoint { x: at.x, y: at.y },
            dx,
            dy,
        )
    }

    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        cg::press(target.pid, combo)
    }

    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()> {
        cg::type_text(target.pid, text)
    }
}

/// The bundle id of the app `open -a` found for `query` (a name, or a
/// path to an .app); `None` when it can't be told. The caller holds an
/// autorelease pool.
fn bundle_id_of(query: &str) -> Option<String> {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSString;

    let path = if query.contains('/') {
        NSString::from_str(query)
    } else {
        // Deprecated for URLForApplicationWithBundleIdentifier, which needs
        // the bundle id this looks for; still the lookup by name `open -a`
        // makes.
        #[allow(deprecated)]
        NSWorkspace::sharedWorkspace().fullPathForApplication(&NSString::from_str(query))?
    };
    let class = AnyClass::get(c"NSBundle")?;
    // SAFETY: NSBundle's documented class and instance methods, with
    // argument and return types as declared.
    let id: Option<Retained<NSString>> = unsafe {
        let bundle: Option<Retained<AnyObject>> = msg_send![class, bundleWithPath: &*path];
        msg_send![&*bundle?, bundleIdentifier]
    };
    let id = ffi::nsstring_text(&*id?);
    (!id.is_empty()).then_some(id)
}

/// The message for an app whose `count` windows are all off this Space.
fn other_space(app: &AppInfo, count: usize) -> String {
    let (windows, are) = if count == 1 {
        ("window", "is")
    } else {
        ("windows", "are")
    };
    format!(
        "{} has no window on the current Space: {count} {windows} of it {are} on another Space or in full screen, where they can't be read. Bring it forward first with launch_app app=\"{}\" (that switches to it), or ask the user to; then try again.",
        app.name, app.id
    )
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
    use super::*;

    #[test]
    fn read_errors_say_what_happened() {
        let e = read_error(ffi::kAXErrorCannotCompleteAtOnce, "Code", 2.0).to_string();
        assert!(!e.contains("-25299"), "{e}");
        assert!(e.contains("Code") && e.contains("Try again"), "{e}");
        let e = read_error(ffi::kAXErrorCannotComplete, "Code", 2.0).to_string();
        assert!(e.contains("2.0 s"), "{e}");
        let e = read_error(ffi::kAXErrorInvalidUIElement, "Code", 2.0).to_string();
        assert!(e.contains("no longer exists"), "{e}");
        assert!(matches!(
            read_error(ffi::kAXErrorAPIDisabled, "Code", 2.0),
            Error::Permission(_)
        ));
        let e = read_error(-25208, "Code", 2.0).to_string();
        assert!(e.contains("does not implement"), "{e}");
        // An undocumented code still names the app and the code.
        let e = read_error(-1, "Code", 2.0).to_string();
        assert!(e.contains("-1") && e.contains("Code"), "{e}");
    }

    #[test]
    fn vanished_elements_end_the_walk_below_them() {
        assert!(ends_walk(ffi::kAXErrorInvalidUIElement));
        assert!(ends_walk(ffi::kAXErrorCannotComplete));
        assert!(ends_walk(ffi::kAXErrorAPIDisabled));
        assert!(!ends_walk(ffi::kAXErrorNoValue));
        assert!(!ends_walk(ffi::kAXErrorCannotCompleteAtOnce));
    }

    #[test]
    fn windows_elsewhere_point_to_launch_app() {
        let app = AppInfo {
            id: "com.microsoft.VSCode".into(),
            name: "Code".into(),
            pid: 42,
            exe: None,
            frontmost: false,
            hidden: false,
        };
        let one = other_space(&app, 1);
        assert!(one.contains("1 window of it is"), "{one}");
        assert!(
            one.contains("launch_app app=\"com.microsoft.VSCode\""),
            "{one}"
        );
        assert!(other_space(&app, 3).contains("3 windows of it are"));
    }

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
