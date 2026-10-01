//! Linux backend: AT-SPI2 (over D-Bus) for the accessibility tree and
//! semantic actions, X11/XTest for synthesized input, and X11 GetImage for
//! screenshots.
//!
//! The accessibility path (DoAction, SetTextContents, GrabFocus, text
//! selection) is preferred; synthesized mouse/keyboard input is the fallback.

mod atspi;
mod clipboard;
mod notify;
mod wm;
pub(crate) mod x11;

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

pub use atspi::ipc_calls;
use atspi::{AtspiConnection, CallError, Fail, ObjRef, state};
use wm::Front;
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

/// How often to try connecting to an X server that isn't there.
const X11_RETRY: Duration = Duration::from_secs(5);

pub struct LinuxBackend {
    a11y: AtspiConnection,
    x11: Option<X11>,
    /// When to try connecting to the X server again (after a failure).
    x11_retry: Option<Instant>,
    restore_pointer: bool,
    /// pid → application accessible.
    app_refs: HashMap<u32, ObjRef>,
    /// pid → app name, from the last listing (for messages).
    app_names: HashMap<u32, String>,
    /// handle → (owning pid, element). Cleared per app on each snapshot so it
    /// never grows beyond the elements of the latest views.
    handles: HashMap<ElementHandle, (u32, ObjRef)>,
    /// Window (root) handles from `list_windows`, one per window element.
    /// They survive snapshots, since callers reuse a window list for a
    /// while, and are only replaced by the next listing of the app.
    window_handles: HashMap<ObjRef, (u32, ElementHandle)>,
    next_handle: ElementHandle,
    batch_size: usize,
    text_max: usize,
    /// Listens for notifications while [notifications] is enabled.
    notifications: Option<notify::Listener>,
}

impl LinuxBackend {
    pub fn new() -> Result<Self> {
        let a11y = AtspiConnection::connect()?;
        // X11 is optional: element-level actions work without it, but input
        // and screenshots need it.
        let (x11, x11_retry) = match X11::connect() {
            Ok(x) => (Some(x), None),
            Err(e) => {
                log::warn!("X11 input/capture unavailable: {e}");
                (None, Some(Instant::now() + X11_RETRY))
            }
        };
        let defaults = crate::config::LinuxConfig::default();
        Ok(Self {
            a11y,
            x11,
            x11_retry,
            restore_pointer: true,
            app_refs: HashMap::new(),
            app_names: HashMap::new(),
            handles: HashMap::new(),
            window_handles: HashMap::new(),
            next_handle: 1,
            batch_size: defaults.batch_size,
            text_max: defaults.text_max_chars,
            notifications: None,
        })
    }

    /// The X server connection, after handling what it sent meanwhile; a
    /// broken one (the server restarted) is replaced by a new one.
    fn x11(&mut self) -> Result<&mut X11> {
        if let Some(x) = &self.x11 {
            x.drain();
            if x.lost() {
                log::warn!("reconnecting to the X server");
                self.x11 = None;
                self.x11_retry = None;
            }
        }
        if self.x11.is_none() && self.x11_retry.is_none_or(|t| Instant::now() >= t) {
            match X11::connect() {
                Ok(mut x) => {
                    x.restore_pointer = self.restore_pointer;
                    self.x11 = Some(x);
                    self.x11_retry = None;
                }
                Err(e) => {
                    log::debug!("X11 still unavailable: {e}");
                    self.x11_retry = Some(Instant::now() + X11_RETRY);
                }
            }
        }
        self.x11
            .as_mut()
            .ok_or_else(|| Error::Platform("no X11 connection for input/capture".into()))
    }

    /// Reconnect to the accessibility bus if the connection broke (the bus
    /// restarted); every element handle is stale then. Whether it did.
    fn revive_a11y(&mut self) -> bool {
        if !self.a11y.lost() {
            return false;
        }
        match AtspiConnection::connect() {
            Ok(c) => {
                log::warn!("reconnected to the accessibility bus");
                self.a11y = c;
                self.app_refs.clear();
                self.handles.clear();
                self.window_handles.clear();
                true
            }
            Err(e) => {
                log::warn!("cannot reconnect to the accessibility bus: {e}");
                false
            }
        }
    }

    /// Run an accessibility query, reconnecting and trying once more when
    /// the bus connection broke.
    fn with_a11y<T>(&mut self, f: impl Fn(&mut Self) -> Result<T>) -> Result<T> {
        self.revive_a11y();
        match f(self) {
            Err(_) if self.revive_a11y() => f(self),
            r => r,
        }
    }

    fn handle_for(&mut self, pid: u32, r: ObjRef) -> ElementHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(h, (pid, r));
        h
    }

    fn resolve(&self, handle: ElementHandle) -> Result<ObjRef> {
        self.resolve_owned(handle).map(|(_, r)| r)
    }

    /// The element behind a handle, and the pid of its app.
    fn resolve_owned(&self, handle: ElementHandle) -> Result<(u32, ObjRef)> {
        self.handles
            .get(&handle)
            .cloned()
            .or_else(|| {
                self.window_handles
                    .iter()
                    .find(|(_, (_, h))| *h == handle)
                    .map(|(r, (p, _))| (*p, r.clone()))
            })
            .ok_or_else(|| Error::Internal(format!("stale element handle {handle}")))
    }

    /// The failure of a request that does something (DoAction, setting a
    /// value) of `pid`'s app.
    fn sent_error(&self, pid: u32, e: CallError) -> Error {
        sent_error(e, || {
            self.app_names
                .get(&pid)
                .cloned()
                .or_else(|| proc_info(pid).1)
                .unwrap_or_else(|| format!("the app (pid {pid})"))
        })
    }

    /// In a Wayland session synthesized input and X11 screenshots only
    /// reach apps running under XWayland (anything else would act on
    /// whichever X window has the focus): refuse unless the app has one.
    fn x11_reaches(&mut self, pid: u32, what: &str) -> Result<()> {
        if !wayland_session() {
            return Ok(());
        }
        if self.x11().is_ok_and(|x| x.wm().has_window_of(pid)) {
            return Ok(());
        }
        Err(Error::Unsupported(format!(
            "{what} can't reach this app: this is a Wayland session, where synthesized keyboard/mouse input and screenshots only reach X11 (XWayland) apps, and this one has no X11 window. Use element_index actions (they work through accessibility), or log in to an X11 (\"Xorg\") session."
        )))
    }

    /// The handle of a window element: the same one for as long as the
    /// window is listed.
    fn window_handle(&mut self, pid: u32, r: ObjRef) -> ElementHandle {
        if let Some((p, h)) = self.window_handles.get(&r)
            && *p == pid
        {
            return *h;
        }
        let h = self.next_handle;
        self.next_handle += 1;
        self.window_handles.insert(r, (pid, h));
        h
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
        // Apps that quit take their handles with them.
        let live = &self.app_refs;
        self.window_handles.retain(|_, (p, _)| live.contains_key(p));
        self.handles.retain(|_, (p, _)| live.contains_key(p));
        self.app_names.retain(|p, _| live.contains_key(p));
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

    /// The AT-SPI app an X window belongs to when its pid doesn't tell (no
    /// `_NET_WM_PID`, or a pid from another pid namespace, as in a Flatpak
    /// sandbox): the only app with an active window of the same title.
    fn owner_by_window(&mut self, win: u32, apps: &[(ObjRef, u32)]) -> Option<u32> {
        let title = self.x11().ok()?.wm().title(win)?;
        let refs: Vec<ObjRef> = apps.iter().map(|(r, _)| r.clone()).collect();
        let mut windows = Vec::new();
        for ((_, pid), kids) in apps.iter().zip(self.a11y.children_many(&refs)) {
            windows.extend(kids.unwrap_or_default().into_iter().map(|w| (*pid, w)));
        }
        let refs: Vec<ObjRef> = windows.iter().map(|(_, w)| w.clone()).collect();
        let mut owners: Vec<u32> = windows
            .iter()
            .zip(self.a11y.active_names(&refs))
            .filter(|(_, name)| name.as_deref() == Some(title.as_str()))
            .map(|((pid, _), _)| *pid)
            .collect();
        owners.dedup();
        match owners.as_slice() {
            [pid] => Some(*pid),
            _ => None, // none, or several: never guess
        }
    }

    /// Who has the keyboard, for `list_apps`: marks the app in front, or
    /// (when the window in front is no accessible app's: a terminal, an app
    /// without accessibility) adds an entry for that window, so that the
    /// engine brings the app it types into to the front first. `apps` are
    /// the apps that answered (only they are asked about their windows).
    fn mark_front(&mut self, out: &mut Vec<AppInfo>, apps: &[(ObjRef, u32)]) {
        let Some(front) = self.x11().ok().and_then(|x| x.wm().front()) else {
            return; // can't tell
        };
        let (win, pid) = match front {
            Front::Window { win, pid } => (win, pid),
            Front::Nothing => {
                out.push(AppInfo {
                    name: "Desktop".into(),
                    id: "desktop".into(),
                    pid: 0,
                    exe: None,
                    frontmost: true,
                    hidden: false,
                });
                return;
            }
        };
        let owner = pid
            .filter(|p| out.iter().any(|a| a.pid == *p))
            .or_else(|| self.owner_by_window(win, apps));
        if let Some(owner) = owner {
            for a in out.iter_mut() {
                a.frontmost = a.pid == owner;
            }
            return;
        }
        let (title, class) = match self.x11() {
            Ok(x) => (x.wm().title(win), x.wm().class(win)),
            Err(_) => (None, None),
        };
        let (exe, comm) = pid.map(proc_info).unwrap_or_default();
        out.push(AppInfo {
            name: class
                .clone()
                .or_else(|| comm.clone())
                .or(title)
                .unwrap_or_else(|| "another window".into()),
            id: comm.or(class).unwrap_or_else(|| format!("window {win:#x}")),
            pid: pid.unwrap_or(0),
            exe,
            frontmost: true,
            hidden: false,
        });
    }

    /// Top-level windows of an app (children fetched concurrently).
    fn windows_of(&mut self, app_ref: &ObjRef) -> Result<Vec<(ObjRef, atspi::NodeData)>> {
        let children = self.a11y.children(app_ref)?;
        let data = self.a11y.fetch_many(&children);
        Ok(children
            .into_iter()
            .zip(data)
            .filter(|(_, d)| {
                if d.fail.is_some() {
                    return false; // gone, or the app is frozen
                }
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
                granted: self.x11().is_ok() && !wayland_session(),
                detail: if wayland_session() {
                    "Wayland session: keys, clicks and screenshots only reach X11 (XWayland) apps; log in to an X11 (\"Xorg\") session for the rest".into()
                } else if self.x11.is_some() {
                    "connected".into()
                } else {
                    "no X11 connection (DISPLAY unset or unreachable)".into()
                },
            },
        ]
    }

    fn list_apps(&mut self) -> Result<Vec<AppInfo>> {
        let apps = self.with_a11y(|b| b.refresh_apps())?;
        let refs: Vec<ObjRef> = apps.iter().map(|(r, _)| r.clone()).collect();
        let data = self.a11y.fetch_many(&refs);
        let mut out = Vec::new();
        let mut answered = Vec::new();
        for ((r, pid), d) in apps.iter().zip(data) {
            let (exe, comm) = proc_info(*pid);
            let acc = match d.fail {
                None => {
                    answered.push((r.clone(), *pid));
                    d.acc
                }
                // Busy (a modal dialog's loop can hold its accessibility
                // for as long as the dialog is open): still running, and
                // still reachable with the keyboard and mouse.
                Some(Fail::Timeout) => atspi::Accessible {
                    name: self.app_names.get(pid).cloned().unwrap_or_default(),
                    role_name: "application".into(),
                    ..Default::default()
                },
                Some(_) => continue, // quit meanwhile
            };
            if acc.role_name != "application" && acc.name.is_empty() {
                continue;
            }
            let name = if !acc.name.is_empty() {
                acc.name.clone()
            } else {
                comm.clone().unwrap_or_else(|| format!("pid {pid}"))
            };
            self.app_names.insert(*pid, name.clone());
            out.push(AppInfo {
                name,
                id: comm.unwrap_or_else(|| acc.name.clone()),
                pid: *pid,
                exe,
                frontmost: false,
                hidden: false,
            });
        }
        // Who has the keyboard, from the window manager (one X11 query for
        // all apps rather than walking every app's windows).
        self.mark_front(&mut out, &answered);
        Ok(out)
    }

    fn launch_app(&mut self, query: &str) -> Result<Option<String>> {
        use crate::launch::{self, Pick};
        // The whole query is the program: never split into arguments, so a
        // launch can't become a command line (`xterm -e …`).
        let err = match crate::backend::spawn_detached(Command::new(query)) {
            Ok(()) => return Ok(None),
            Err(e) => e,
        };
        if err.kind() != std::io::ErrorKind::NotFound || query.contains('/') {
            return Err(Error::ActionFailed(format!(
                "could not launch `{query}`: {err}"
            )));
        }
        // Not a program: an app's name in the menu ("Text Editor").
        let entries = launch::desktop_entries(&launch::application_dirs());
        match launch::find_desktop_entry(&entries, query) {
            Pick::One(entry) => {
                let mut cmd = Command::new(&entry.exec[0]);
                cmd.args(&entry.exec[1..]);
                crate::backend::spawn_detached(cmd).map_err(|e| {
                    Error::ActionFailed(format!(
                        "could not launch `{}` ({}): {e}",
                        entry.name, entry.exec[0]
                    ))
                })?;
                Ok(Some(entry.exec[0].clone()))
            }
            other => Err(Error::ActionFailed(launch::not_found(
                query,
                &other,
                "the installed applications",
            ))),
        }
    }

    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let pid = app.pid;
        let windows = self
            .with_a11y(|b| {
                let app_ref = b.app_ref(pid)?;
                b.windows_of(&app_ref)
            })
            .map_err(|e| match e {
                // Listed for having the keyboard, but not on the bus.
                Error::AppNotFound(_) if pid == 0 => Error::Unsupported(format!(
                    "{} is listed because it has the keyboard, but it is no app whose windows can be read or used",
                    app.name
                )),
                Error::AppNotFound(_) if process_alive(pid) => Error::Unsupported(format!(
                    "{} (pid {pid}) isn't on the accessibility bus (AT-SPI): it may still be starting (try again in a moment), or it doesn't support accessibility (a terminal, some Electron apps), so its windows can't be read or used",
                    app.name
                )),
                e => e,
            })?;
        // Windows no longer listed drop their handles.
        self.window_handles
            .retain(|r, (p, _)| *p != app.pid || windows.iter().any(|(w, _)| w == r));
        let mut out = Vec::new();
        for (r, d) in windows {
            let bounds = d
                .extents
                .map(|(x, y, w, h)| Rect::new(x.into(), y.into(), w.into(), h.into()));
            let id = stable_id(&r.path);
            let handle = self.window_handle(app.pid, r);
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
        self.revive_a11y();
        let root = self.resolve(window.handle)?;
        // Handles from this app's previous views are no longer needed; its
        // window handles (kept separately) stay valid for the next snapshot.
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
        self.restore_pointer = cfg.restore_pointer;
        if let Some(x) = self.x11.as_mut() {
            x.restore_pointer = cfg.restore_pointer;
        }
        let n = &cfg.notifications;
        match (&self.notifications, n.enabled) {
            (Some(l), true) if l.running() => l.set_keep(n.keep),
            (_, true) => self.notifications = Some(notify::Listener::start(n.keep)),
            (Some(l), false) => {
                l.stop();
                self.notifications = None;
            }
            (None, false) => {}
        }
    }

    fn notifications(&mut self) -> Result<Vec<Notification>> {
        match &self.notifications {
            Some(l) => l.recent(),
            None => Err(Error::Unsupported(
                "not listening for notifications ([notifications] enabled = false)".into(),
            )),
        }
    }

    fn user_idle(&mut self) -> Option<std::time::Duration> {
        self.x11().ok().and_then(|x| x.idle())
    }

    fn displays(&mut self) -> Result<Vec<Display>> {
        Ok(self.x11()?.wm().displays())
    }

    fn desktops(&mut self) -> Option<(u32, u32)> {
        self.x11().ok().and_then(|x| x.wm().desktops())
    }

    fn window_op(&mut self, app: &AppInfo, window: &WindowInfo, op: &WindowOp) -> Result<()> {
        let x11 = self.x11()?;
        let wm = x11.wm();
        let win = wm
            .find(app.pid, &window.title, window.bounds)
            .ok_or_else(|| {
                Error::ActionFailed(format!(
                    "could not find the X11 window of \"{}\"",
                    window.title
                ))
            })?;
        wm.apply(win, op)
    }

    fn capture(&mut self, app: &AppInfo, window: &WindowInfo) -> Result<Capture> {
        let rect = window
            .bounds
            .filter(|b| !b.is_empty())
            .ok_or_else(|| Error::Platform("window has no on-screen bounds to capture".into()))?;
        self.x11_reaches(app.pid, "A screenshot")?;
        self.x11()?.capture(rect)
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        if wayland_session() {
            return Err(Error::Unsupported(
                "screenshots of the screen: this is a Wayland session, where X11 screenshots show only X11 (XWayland) windows. Log in to an X11 (\"Xorg\") session for them.".into(),
            ));
        }
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
        self.revive_a11y();
        let (pid, r) = self.resolve_owned(element)?;
        let idx = self.a11y.action_index(&r, native_action)?.ok_or_else(|| {
            Error::ActionFailed(format!("element no longer offers action `{native_action}`"))
        })?;
        // Timed out: sent, and maybe done (GTK runs the handler, a modal
        // dialog included, before it answers): never to be repeated.
        let ok = self
            .a11y
            .do_action(&r, idx)
            .map_err(|e| self.sent_error(pid, e))?;
        if ok {
            Ok(())
        } else {
            Err(Error::ActionFailed(format!(
                "action `{native_action}` was not accepted"
            )))
        }
    }

    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()> {
        self.revive_a11y();
        let (pid, r) = self.resolve_owned(element)?;
        let acc = self.a11y.describe(&r)?;
        // A request that timed out was sent: it may have set the value,
        // so nothing else is tried.
        if acc.has_iface("EditableText") || acc.states.has(state::EDITABLE) {
            match self.a11y.set_text(&r, value) {
                Ok(true) => return Ok(()),
                Err(e) if e.fail == Fail::Timeout => return Err(self.sent_error(pid, e)),
                _ => {}
            }
        }
        if acc.has_iface("Value")
            && let Ok(n) = value.trim().parse::<f64>()
        {
            match self.a11y.set_value(&r, n) {
                Ok(true) => return Ok(()),
                Err(e) if e.fail == Fail::Timeout => return Err(self.sent_error(pid, e)),
                _ => {}
            }
        }
        // Checkbox/toggle: flip to the requested boolean via its action.
        let want = matches!(
            value.trim().to_lowercase().as_str(),
            "true" | "1" | "on" | "checked" | "yes"
        );
        let is = acc.states.has(state::CHECKED) || acc.states.has(state::PRESSED);
        if is != want {
            for action in ["toggle", "click", "press", "activate"] {
                if let Some(idx) = self.a11y.action_index(&r, action)?
                    && self
                        .a11y
                        .do_action(&r, idx)
                        .map_err(|e| self.sent_error(pid, e))?
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
        self.revive_a11y();
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
        self.revive_a11y();
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
        target: &InputTarget,
        at: Point,
        button: MouseButton,
        count: u8,
    ) -> Result<()> {
        self.x11_reaches(target.pid, "A click")?;
        let b = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        };
        self.x11()?.click(at.x as i32, at.y as i32, b, count)
    }

    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()> {
        self.x11_reaches(target.pid, "A drag")?;
        self.x11()?
            .drag((from.x as i32, from.y as i32), (to.x as i32, to.y as i32))
    }

    fn move_pointer(&mut self, target: &InputTarget, at: Point) -> Result<Option<Point>> {
        self.x11_reaches(target.pid, "Moving the pointer")?;
        let back = self
            .x11()?
            .move_pointer(at.x.round() as i32, at.y.round() as i32)?;
        Ok(back.map(|(x, y)| Point::new(f64::from(x), f64::from(y))))
    }

    fn draw(
        &mut self,
        target: &InputTarget,
        strokes: &[Vec<Point>],
        button: MouseButton,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        self.x11_reaches(target.pid, "Drawing")?;
        let b = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        };
        let strokes: Vec<Vec<(i32, i32)>> = strokes
            .iter()
            .map(|s| {
                s.iter()
                    .map(|p| (p.x.round() as i32, p.y.round() as i32))
                    .collect()
            })
            .collect();
        self.x11()?.draw(&strokes, b, pace)
    }

    fn scroll_wheel(&mut self, target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()> {
        self.x11_reaches(target.pid, "Scrolling")?;
        self.x11()?.scroll(at.x as i32, at.y as i32, dx, dy)
    }

    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        self.x11_reaches(target.pid, "A key press")?;
        self.x11()?.press(combo)
    }

    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()> {
        self.x11_reaches(target.pid, "Typing")?;
        self.x11()?.type_text(text)
    }
}

/// The failure of a request that does something (DoAction, setting a
/// value): one that timed out was sent and may have happened, so the engine
/// must not do it again another way ([`Error::Unanswered`]). Bridges that
/// run the action before answering hold the reply while it runs (Qt's, for
/// a button that opens a modal dialog with `exec()`).
fn sent_error(e: CallError, app: impl FnOnce() -> String) -> Error {
    if e.fail == Fail::Timeout {
        Error::Unanswered(app())
    } else {
        e.into()
    }
}

fn process_alive(pid: u32) -> bool {
    pid != 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()
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

/// Whether the desktop session is Wayland: XTest input and X11 captures then
/// only reach apps running under XWayland, not native Wayland apps.
fn wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
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
    fn an_action_that_timed_out_is_unanswered_not_failed() {
        let e = sent_error(CallError::of(Fail::Timeout), || "Editor".into());
        assert!(
            matches!(e, Error::Unanswered(ref app) if app == "Editor"),
            "{e}"
        );
        for fail in [Fail::Gone, Fail::Other, Fail::Disconnected] {
            let e = sent_error(CallError::of(fail), || unreachable!());
            assert!(matches!(e, Error::Platform(_)), "{e}");
        }
    }

    #[test]
    fn stable_id_is_deterministic() {
        assert_eq!(stable_id("/org/a11y/x"), stable_id("/org/a11y/x"));
        assert_ne!(stable_id("/a"), stable_id("/b"));
    }
}
