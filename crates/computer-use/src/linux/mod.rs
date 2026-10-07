//! Linux backend: AT-SPI2 (over D-Bus) for the accessibility tree and
//! semantic actions; for synthesized input and screenshots, X11 (XTest,
//! GetImage) on an X11 desktop, and in a Wayland session the compositor's
//! protocols and IPC ([`wayland`]: Hyprland, sway and the other
//! wlroots-family compositors), with X11 kept for XWayland apps where the
//! compositor offers neither.
//!
//! The accessibility path (DoAction, SetTextContents, GrabFocus, text
//! selection) is preferred; synthesized mouse/keyboard input is the fallback.

mod atspi;
mod clipboard;
mod notify;
pub(crate) mod wayland;
mod wm;
pub(crate) mod x11;

use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

pub use atspi::ipc_calls;
use atspi::{AtspiConnection, CallError, Fail, ObjRef, state};
use wayland::ipc::{Compositor, Toplevel};
use wayland::proto::Wl;
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

/// How often to try connecting to an X server (or an accessibility bus)
/// that isn't there.
const X11_RETRY: Duration = Duration::from_secs(5);

pub struct LinuxBackend {
    /// The accessibility bus. Optional (over ssh, in a container, a
    /// session without at-spi): screenshots, input and windows work
    /// without it.
    a11y: Option<AtspiConnection>,
    /// When to try connecting to the accessibility bus again, and why the
    /// last try failed.
    a11y_retry: Option<Instant>,
    a11y_error: String,
    x11: Option<X11>,
    /// When to try connecting to the X server again (after a failure).
    x11_retry: Option<Instant>,
    restore_pointer: bool,
    natural_mouse: bool,
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
    /// A Wayland session: the compositor's protocols (input, screenshots,
    /// idle time), connected when first needed, and when to try again.
    wl: Option<Wl>,
    wl_retry: Option<Instant>,
    /// The compositor's IPC (Hyprland, sway): where windows are, which has
    /// the keyboard, window changes.
    comp: Option<Compositor>,
    /// Window handle → the compositor's window, from the last listing.
    toplevels: HashMap<ElementHandle, Toplevel>,
    /// Windows of apps that aren't on the accessibility bus (terminals,
    /// some Electron apps), as the compositor or the X server lists them:
    /// they are seen in screenshots (and text read off them) and used with
    /// the mouse and keyboard.
    bare: HashMap<ElementHandle, Bare>,
    /// pid → how much larger the app's windows are on the X screen than
    /// its accessibility says (2 for a GTK 3 app with `GDK_SCALE=2`).
    scales: HashMap<u32, f64>,
}

/// A window with no accessibility behind it.
#[derive(Debug, Clone)]
struct Bare {
    pid: u32,
    /// The compositor's or the X server's id for it.
    key: String,
}

impl LinuxBackend {
    pub fn new() -> Result<Self> {
        // X11 is optional: element-level actions work without it, but input
        // and screenshots need it.
        let (x11, x11_retry) = match X11::connect() {
            Ok(x) => (Some(x), None),
            Err(e) => {
                log::warn!("X11 input/capture unavailable: {e}");
                (None, Some(Instant::now() + X11_RETRY))
            }
        };
        // So is the accessibility bus (connected again later).
        let x_bus = || x11.as_ref().and_then(|x| x.wm().root_string("AT_SPI_BUS"));
        let (a11y, a11y_retry, a11y_error) = match AtspiConnection::connect(x_bus) {
            Ok(c) => (Some(c), None, String::new()),
            Err(e) => {
                log::warn!("no accessibility bus: {e}");
                (None, Some(Instant::now() + X11_RETRY), reason(&e))
            }
        };
        let defaults = crate::config::LinuxConfig::default();
        Ok(Self {
            a11y,
            a11y_retry,
            a11y_error,
            x11,
            x11_retry,
            restore_pointer: true,
            natural_mouse: true,
            app_refs: HashMap::new(),
            app_names: HashMap::new(),
            handles: HashMap::new(),
            window_handles: HashMap::new(),
            next_handle: 1,
            batch_size: defaults.batch_size,
            text_max: defaults.text_max_chars,
            notifications: None,
            wl: None,
            wl_retry: None,
            comp: if wayland_session() {
                Compositor::detect()
            } else {
                None
            },
            toplevels: HashMap::new(),
            bare: HashMap::new(),
            scales: HashMap::new(),
        })
    }

    /// The Wayland protocols, connected (or reconnected) when needed.
    fn wl(&mut self) -> Result<&mut Wl> {
        if self.wl.as_ref().is_some_and(Wl::lost) {
            log::warn!("reconnecting to the Wayland compositor");
            self.wl = None;
            self.wl_retry = None;
        }
        if self.wl.is_none()
            && wayland_session()
            && self.wl_retry.is_none_or(|t| Instant::now() >= t)
        {
            match Wl::connect() {
                Ok(mut w) => {
                    w.natural = self.natural_mouse;
                    self.wl = Some(w);
                    self.wl_retry = None;
                }
                Err(e) => {
                    log::warn!("Wayland input/capture unavailable: {e}");
                    self.wl_retry = Some(Instant::now() + X11_RETRY);
                }
            }
        }
        self.wl.as_mut().ok_or_else(|| {
            Error::Unsupported("no connection to the Wayland compositor for input/capture".into())
        })
    }

    /// Whether pointer input goes through the compositor (a Wayland session
    /// whose compositor offers wlr-virtual-pointer): for every app, XWayland
    /// ones included.
    fn wl_points(&mut self) -> bool {
        wayland_session() && self.wl().is_ok_and(|w| w.can_point())
    }

    /// Whether keyboard input goes through the compositor (virtual-keyboard).
    fn wl_types(&mut self) -> bool {
        wayland_session() && self.wl().is_ok_and(|w| w.can_type())
    }

    /// Whether screenshots come from the compositor (wlr-screencopy).
    fn wl_captures(&mut self) -> bool {
        wayland_session() && self.wl().is_ok_and(|w| w.can_capture())
    }

    /// The compositor's window behind a window handle, or else (looked up
    /// afresh) the one with the compositor's id a bare window was listed
    /// under, or the one of `pid` titled `title`.
    fn toplevel(&self, handle: ElementHandle, pid: u32, title: &str) -> Option<Toplevel> {
        if let Some(t) = self.toplevels.get(&handle) {
            return Some(t.clone());
        }
        let tops = self.comp.as_ref()?.toplevels().ok()?;
        // Several of an app's windows can have the same title (terminals).
        if let Some(id) = self
            .bare
            .get(&handle)
            .and_then(|b| b.key.strip_prefix("wl:"))
        {
            return tops.into_iter().find(|t| t.id == id);
        }
        let mine: Vec<&Toplevel> = tops.iter().filter(|t| t.pid == Some(pid)).collect();
        match mine.as_slice() {
            [only] => Some((*only).clone()),
            many => many.iter().find(|t| t.title == title).map(|t| (*t).clone()),
        }
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
                    x.natural = self.natural_mouse;
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
    /// restarted), or connect if there was none (tried again every
    /// [`X11_RETRY`]); every element handle is stale then. Whether it did.
    fn revive_a11y(&mut self) -> bool {
        let lost = match &self.a11y {
            Some(a) if !a.lost() => return false,
            Some(_) => true,
            None => false,
        };
        if !lost && self.a11y_retry.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        if lost {
            self.a11y = None;
            self.app_refs.clear();
            self.handles.clear();
            self.window_handles.clear();
        }
        let x_bus = || {
            self.x11
                .as_ref()
                .and_then(|x| x.wm().root_string("AT_SPI_BUS"))
        };
        match AtspiConnection::connect(x_bus) {
            Ok(c) => {
                log::warn!("connected to the accessibility bus");
                self.a11y = Some(c);
                self.a11y_retry = None;
                self.app_refs.clear();
                self.handles.clear();
                self.window_handles.clear();
                true
            }
            Err(e) => {
                log::warn!("cannot connect to the accessibility bus: {e}");
                self.a11y_error = reason(&e);
                self.a11y_retry = Some(Instant::now() + X11_RETRY);
                false
            }
        }
    }

    /// The accessibility bus, or why there is none.
    fn bus(&self) -> Result<&AtspiConnection> {
        self.a11y.as_ref().ok_or_else(|| no_bus(&self.a11y_error))
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
        let a11y = self.bus()?;
        let root = a11y.root();
        let children = a11y.children(&root)?;
        let pids = a11y.pids_of(&children);
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
        // Apps not on the bus keep their (bare) windows until they exit.
        self.bare.retain(|_, b| process_alive(b.pid));
        let windows: std::collections::HashSet<ElementHandle> =
            self.window_handles.values().map(|(_, h)| *h).collect();
        let bare = &self.bare;
        self.toplevels
            .retain(|h, _| windows.contains(h) || bare.contains_key(h));
        self.handles.retain(|_, (p, _)| live.contains_key(p));
        self.app_names.retain(|p, _| live.contains_key(p));
        self.scales.retain(|p, _| live.contains_key(p));
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
        let mut owners: Vec<u32> = self
            .active_windows(apps)
            .into_iter()
            .filter(|(_, name)| *name == title)
            .map(|(pid, _)| pid)
            .collect();
        owners.dedup();
        match owners.as_slice() {
            [pid] => Some(*pid),
            _ => None, // none, or several: never guess
        }
    }

    /// The windows that are active (have the focus) as their apps tell:
    /// each one's app (pid) and title.
    fn active_windows(&self, apps: &[(ObjRef, u32)]) -> Vec<(u32, String)> {
        let Ok(a11y) = self.bus() else {
            return Vec::new();
        };
        let refs: Vec<ObjRef> = apps.iter().map(|(r, _)| r.clone()).collect();
        let mut windows = Vec::new();
        for ((_, pid), kids) in apps.iter().zip(a11y.children_many(&refs)) {
            windows.extend(kids.unwrap_or_default().into_iter().map(|w| (*pid, w)));
        }
        let refs: Vec<ObjRef> = windows.iter().map(|(_, w)| w.clone()).collect();
        windows
            .iter()
            .zip(a11y.active_names(&refs))
            .filter_map(|((pid, _), name)| Some((*pid, name?)))
            .collect()
    }

    /// Who has the keyboard, for `list_apps`: marks the app in front, or
    /// (when the window in front is no accessible app's: a terminal, an app
    /// without accessibility) adds an entry for that window, so that the
    /// engine brings the app it types into to the front first. `apps` are
    /// the apps that answered (only they are asked about their windows).
    fn mark_front(&mut self, out: &mut Vec<AppInfo>, apps: &[(ObjRef, u32)]) {
        // In a Wayland session the compositor knows (X11 sees only XWayland).
        if let Some(active) = self.comp.as_ref().map(Compositor::active) {
            match active {
                Ok(Some(t)) => {
                    if let Some(pid) = t.pid.filter(|p| out.iter().any(|a| a.pid == *p)) {
                        for a in out.iter_mut() {
                            a.frontmost = a.pid == pid;
                        }
                    } else {
                        let (exe, comm) = t.pid.map(proc_info).unwrap_or_default();
                        out.push(AppInfo {
                            name: if t.app_id.is_empty() {
                                comm.clone().unwrap_or_else(|| t.title.clone())
                            } else {
                                t.app_id.clone()
                            },
                            id: comm.unwrap_or_else(|| t.app_id.clone()),
                            pid: t.pid.unwrap_or(0),
                            exe,
                            frontmost: true,
                            hidden: false,
                        });
                    }
                    return;
                }
                Ok(None) => {
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
                Err(e) => {
                    // Can't tell what is in front: say "something else", so
                    // the engine brings the target forward (or refuses to
                    // type) instead of typing into whatever has the focus.
                    log::debug!("compositor: {e}");
                    if self.x11().is_err() {
                        out.push(AppInfo {
                            name: "Unknown window".into(),
                            id: "unknown".into(),
                            pid: 0,
                            exe: None,
                            frontmost: true,
                            hidden: false,
                        });
                        return;
                    }
                }
            }
        }
        let front = self.x11().ok().and_then(|x| x.wm().front());
        // A Wayland session whose compositor doesn't say (GNOME, KDE): X11
        // sees only XWayland windows, so a native one in front looks like
        // none; the apps' own accessibility says which window is active.
        let blind = wayland_session() && self.comp.is_none();
        let active: Vec<u32> = if blind && !matches!(front, Some(Front::Window { .. })) {
            self.active_windows(apps)
                .into_iter()
                .map(|(pid, _)| pid)
                .collect()
        } else {
            Vec::new()
        };
        let (win, pid) = match pick_front(blind, front, &active) {
            FrontPick::Window(win, pid) => (win, pid),
            FrontPick::App(pid) => {
                for a in out.iter_mut() {
                    a.frontmost = a.pid == pid;
                }
                return;
            }
            FrontPick::Desktop => {
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
            FrontPick::Unknown => {
                out.push(AppInfo {
                    name: "Unknown window".into(),
                    id: "unknown".into(),
                    pid: 0,
                    exe: None,
                    frontmost: true,
                    hidden: false,
                });
                return;
            }
            FrontPick::CantTell => return,
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
        let a11y = self.bus()?;
        let children = a11y.children(app_ref)?;
        let data = a11y.fetch_many(&children);
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
        // A label with no Name but text content: promote the text to a name;
        // a control with no name: the label it is labelled by.
        let (name, value) = if acc.name.is_empty()
            && let Some(label) = data.label
        {
            (Some(label), text_value)
        } else if acc.name.is_empty() {
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

        // An action with no name: the first is the element's default one
        // (a link's "jump" in some apps); others can't be told apart.
        let actions = data
            .actions
            .into_iter()
            .enumerate()
            .filter_map(|(i, native)| match (i, native.is_empty()) {
                (_, false) => Some(ActionDesc::new(roles::atspi_action(&native), native)),
                (0, true) => Some(ActionDesc::new("press", "#0")),
                _ => None,
            })
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
        self.revive_a11y();
        let mut out = vec![match &self.a11y {
            Some(_) => a11y_status(None, atspi::enabled()),
            None => a11y_status(Some(&self.a11y_error), None),
        }];
        if wayland_session() {
            // What this compositor lets us do, and how.
            let comp = self.comp.as_ref().map(Compositor::name);
            let wl = self.wl().ok().map(|w| {
                let has = |p: &str| w.protocols().iter().any(|(i, _)| i == p);
                (
                    w.can_point(),
                    w.can_type(),
                    w.can_capture(),
                    has("zwlr_layer_shell_v1"),
                )
            });
            let (point, typing, capture, layer) = wl.unwrap_or_default();
            let mut missing = Vec::new();
            for (ok, what) in [
                (comp.is_some(), "window positions (Hyprland or sway IPC)"),
                (point, "pointer input (wlr-virtual-pointer)"),
                (typing, "keyboard input (virtual-keyboard)"),
                (capture, "screenshots (wlr-screencopy)"),
                (layer, "overlay (wlr-layer-shell)"),
            ] {
                if !ok {
                    missing.push(what);
                }
            }
            out.push(PermissionStatus {
                name: "Wayland compositor".into(),
                granted: missing.is_empty(),
                detail: match (comp, missing.is_empty()) {
                    (Some(c), true) => format!(
                        "{c}: windows, input, screenshots and overlay through the compositor"
                    ),
                    (c, false) => format!(
                        "{}: no {} (only X11/XWayland apps get those)",
                        c.unwrap_or("this compositor"),
                        missing.join(", ")
                    ),
                    (None, true) => "input and screenshots through the compositor".into(),
                },
            });
        }
        let input = self.x11().is_ok_and(|x| x.can_input());
        out.push(
            PermissionStatus {
                name: "X11 input/capture".into(),
                granted: if wayland_session() {
                    self.wl_points()
                } else {
                    input
                },
                detail: if wayland_session() && self.wl_points() {
                    "not needed: input and screenshots go through the compositor".into()
                } else if wayland_session() {
                    "Wayland session: keys, clicks and screenshots only reach X11 (XWayland) apps; log in to an X11 (\"Xorg\") session for the rest".into()
                } else if self.x11.is_some() && input {
                    "connected".into()
                } else if self.x11.is_some() {
                    "connected, but the X server has no XTEST extension: screenshots work, synthesized keyboard/mouse input doesn't".into()
                } else {
                    "no X11 connection (DISPLAY unset or unreachable)".into()
                },
            },
        );
        out
    }

    fn session_note(&mut self) -> Option<String> {
        if !wayland_session() {
            return None;
        }
        let mut missing = Vec::new();
        if !self.wl_points() {
            missing.push("click");
        }
        if !self.wl_types() {
            missing.push("type");
        }
        if !self.wl_captures() {
            missing.push("take screenshots");
        }
        if missing.is_empty() {
            return None;
        }
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| "this desktop".into());
        Some(format!(
            "This is a Wayland session ({desktop}) that doesn't let other programs {} in its own Wayland apps. In those, act through accessibility: click by element_index, set_value, perform_secondary_action (keys and coordinate clicks can't reach them; set_value then a click on the page's own button submits a form). X11 (XWayland) apps work fully. Everything works under Hyprland or sway, or in an X11 (\"Xorg\") session.",
            missing.join(", ")
        ))
    }

    fn list_apps(&mut self) -> Result<Vec<AppInfo>> {
        let apps = match self.with_a11y(|b| b.refresh_apps()) {
            Ok(apps) => apps,
            // No accessibility bus: the apps with windows, below.
            Err(_) if self.a11y.is_none() => Vec::new(),
            Err(e) => return Err(e),
        };
        let refs: Vec<ObjRef> = apps.iter().map(|(r, _)| r.clone()).collect();
        let data = match self.bus() {
            Ok(a11y) => a11y.fetch_many(&refs),
            Err(_) => Vec::new(),
        };
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
        // Apps with windows but no accessibility (terminals, some Electron
        // apps), as the compositor or the window manager lists them: usable
        // through screenshots and the mouse and keyboard.
        let owners: Vec<(u32, String, String)> = match self.comp.as_ref().map(Compositor::toplevels)
        {
            Some(Ok(tops)) => tops
                .into_iter()
                .filter_map(|t| t.pid.map(|p| (p, t.app_id, t.title)))
                .collect(),
            Some(Err(_)) => Vec::new(),
            None => self
                .x11()
                .map(|x| x.wm().window_owners())
                .unwrap_or_default(),
        };
        let me = std::process::id();
        for (pid, class, title) in owners {
            if pid == me || pid == 0 || out.iter().any(|a| a.pid == pid) {
                continue;
            }
            let (exe, comm) = proc_info(pid);
            out.push(AppInfo {
                name: if class.is_empty() {
                    comm.clone().unwrap_or(title)
                } else {
                    class.clone()
                },
                id: comm.unwrap_or(class),
                pid,
                exe,
                frontmost: false,
                hidden: false,
            });
        }
        if out.is_empty() && self.a11y.is_none() {
            return Err(no_bus(&self.a11y_error));
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
        let mut cmd = Command::new(query);
        with_a11y_env(&mut cmd);
        let err = match crate::backend::spawn_detached(cmd) {
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
                with_a11y_env(&mut cmd);
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
        let listed = self.with_a11y(|b| {
            let app_ref = b.app_ref(pid)?;
            b.windows_of(&app_ref)
        });
        let windows = match listed {
            Ok(w) => w,
            // Listed because nothing else has the keyboard.
            Err(Error::AppNotFound(_)) if pid == 0 => {
                return Err(Error::ActionFailed(format!(
                    "{} isn't an app: it is listed because no app window has the keyboard (an empty workspace, or the desktop itself). Bring an app forward (window action=focus, or launch_app), or look at the whole screen (screenshot).",
                    app.name
                )));
            }
            // Not on the bus: its windows as the compositor or the X
            // server sees them.
            Err(Error::AppNotFound(_)) if process_alive(pid) => {
                let bare = self.bare_windows(app);
                if !bare.is_empty() {
                    return Ok(bare);
                }
                return Err(Error::ActionFailed(format!(
                    "{} (pid {pid}) isn't on the accessibility bus (AT-SPI) and has no window this desktop shows other programs: it may still be starting (try again in a moment), or it has no window",
                    app.name
                )));
            }
            // No accessibility bus at all: the same.
            Err(e) if self.a11y.is_none() && pid != 0 => {
                let bare = self.bare_windows(app);
                if !bare.is_empty() {
                    return Ok(bare);
                }
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        // Windows no longer listed drop their handles.
        self.window_handles
            .retain(|r, (p, _)| *p != app.pid || windows.iter().any(|(w, _)| w == r));
        // In a Wayland session the compositor says where each window is.
        let mut tops: Vec<Toplevel> = self
            .comp
            .as_ref()
            .and_then(|c| c.toplevels().ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.pid == Some(pid))
            .collect();
        let frames: Vec<Frame> = windows
            .iter()
            .map(|(_, d)| (d.acc.name.clone(), d.extents))
            .collect();
        let matched = match_toplevels(&frames, &mut tops);
        let scale = self.scale_of(pid, &frames);
        let mut out = Vec::new();
        for ((r, d), top) in windows.into_iter().zip(matched) {
            let id = stable_id(&r.path);
            let handle = self.window_handle(app.pid, r);
            let active = d.acc.states.has(state::ACTIVE);
            let mut info = WindowInfo {
                id,
                title: if d.acc.name.is_empty() {
                    app.name.clone()
                } else {
                    d.acc.name
                },
                bounds: d.extents.map(|(x, y, w, h)| {
                    scaled(Rect::new(x.into(), y.into(), w.into(), h.into()), scale)
                }),
                focused: active,
                main: active,
                minimized: !d.acc.states.has(state::SHOWING),
                handle,
            };
            match top {
                Some(t) => {
                    info.bounds = Some(t.rect);
                    info.focused = t.focused;
                    info.main = t.focused || info.main;
                    // On another workspace it can't be seen or clicked.
                    info.minimized = !t.visible;
                    self.toplevels.insert(handle, t);
                }
                None => {
                    self.toplevels.remove(&handle);
                    // A Wayland app's own coordinates are relative to its
                    // window, whose place only the compositor knows.
                    if wayland_session() && !self.x11().is_ok_and(|x| x.wm().has_window_of(pid)) {
                        info.bounds = None;
                    }
                }
            }
            out.push(info);
        }
        Ok(out)
    }

    fn snapshot(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        opts: &SnapshotOptions,
    ) -> Result<Vec<RawNode>> {
        if let Some(b) = self.bare.get(&window.handle) {
            // Nothing but the window: what is in it is read off the screen.
            return Ok(vec![RawNode {
                handle: window.handle,
                parent: None,
                key: Some(b.key.clone()),
                role: "window".into(),
                native_role: "frame".into(),
                name: Some(window.title.clone()).filter(|t| !t.is_empty()),
                description: None,
                value: None,
                placeholder: None,
                identifier: None,
                bounds: window.bounds,
                actions: Vec::new(),
                states: NodeStates {
                    enabled: true,
                    focused: window.focused,
                    ..NodeStates::default()
                },
            }]);
        }
        self.revive_a11y();
        self.bus()?;
        let root = self.resolve(window.handle)?;
        // Handles from this app's previous views are no longer needed; its
        // window handles (kept separately) stay valid for the next snapshot.
        self.handles.retain(|_, (p, _)| *p != app.pid);
        let a11y = self.bus()?;
        let walked = a11y.walk(
            &root,
            opts.max_nodes,
            opts.max_depth,
            self.batch_size,
            self.text_max,
        );
        // A native Wayland window reports its elements relative to itself:
        // place them where the compositor has the window.
        let shift = self
            .toplevels
            .get(&window.handle)
            .filter(|t| !t.xwayland)
            .map(|t| (t.rect, wayland_offset(&walked, t.rect)));
        // A web page's address, as its document's value (browsers show it
        // only in the address bar, which may be out of date or hidden).
        let urls: Vec<Option<String>> = walked
            .iter()
            .map(|w| {
                let d = &w.data.acc;
                (matches!(d.role_name.as_str(), "document web" | "document frame")
                    && d.has_iface("Document"))
                .then(|| a11y.doc_url(&w.r))
                .flatten()
            })
            .collect();
        let mut out = Vec::with_capacity(walked.len());
        for (w, url) in walked.into_iter().zip(urls) {
            let mut node = self.build_node(app.pid, w);
            if let Some(url) = url.filter(|u| u.starts_with("http") || u.starts_with("file:")) {
                node.value = Some(url);
            }
            out.push(node);
        }
        if let Some((rect, (dx, dy))) = shift {
            for n in &mut out {
                if let Some(b) = n.bounds.as_mut() {
                    b.x += dx;
                    b.y += dy;
                }
            }
            if let Some(root) = out.first_mut().filter(|n| n.parent.is_none()) {
                root.bounds = Some(rect);
            }
        }
        // A HiDPI app whose accessibility is in its own (smaller) units.
        if let Some(s) = self.scales.get(&app.pid).copied() {
            for n in &mut out {
                n.bounds = n.bounds.map(|b| scaled(b, s));
            }
        }
        Ok(out)
    }

    fn configure(&mut self, cfg: &crate::config::Config) {
        self.batch_size = cfg.linux.batch_size.max(1);
        self.text_max = cfg.linux.text_max_chars.max(1);
        self.restore_pointer = cfg.restore_pointer;
        self.natural_mouse = cfg.natural_mouse;
        if let Some(x) = self.x11.as_mut() {
            x.restore_pointer = cfg.restore_pointer;
            x.natural = cfg.natural_mouse;
        }
        if let Some(w) = self.wl.as_mut() {
            w.natural = cfg.natural_mouse;
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
        if wayland_session() {
            // XWayland's idle counter sees only X11 input: never trust it
            // here (it could make the engine wait for ever).
            return self.wl().ok().and_then(Wl::idle);
        }
        self.x11().ok().and_then(|x| x.idle())
    }

    fn displays(&mut self) -> Result<Vec<Display>> {
        if let Some(Ok(d)) = self.comp.as_ref().map(Compositor::displays) {
            return Ok(d);
        }
        Ok(self.x11()?.wm().displays())
    }

    fn desktops(&mut self) -> Option<(u32, u32)> {
        if wayland_session() {
            return None;
        }
        self.x11().ok().and_then(|x| x.wm().desktops())
    }

    fn window_op(&mut self, app: &AppInfo, window: &WindowInfo, op: &WindowOp) -> Result<()> {
        // In a Wayland session the compositor does it (XWayland windows too).
        if let Some(comp) = &self.comp {
            let top = self
                .toplevel(window.handle, app.pid, &window.title)
                .ok_or_else(|| {
                    Error::ActionFailed(format!(
                        "the compositor has no window of {} titled \"{}\"",
                        app.name, window.title
                    ))
                })?;
            return match op {
                WindowOp::Focus => comp.focus(&top),
                op => comp.apply(&top, op),
            };
        }
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
        if self.wl_captures() {
            if window.minimized {
                return Err(Error::ActionFailed(format!(
                    "\"{}\" isn't on screen (on another workspace, or minimized), so a screenshot would show something else; bring it forward first (window action=focus)",
                    window.title
                )));
            }
            return self.wl()?.capture(rect);
        }
        // X11 reads the screen where the window was: a window that isn't
        // shown there would come back as whatever is in its place.
        if window.minimized {
            return Err(Error::ActionFailed(format!(
                "\"{}\" isn't on screen (on another workspace, or minimized), so a screenshot would show something else; bring it forward first (window action=focus)",
                window.title
            )));
        }
        self.x11_reaches(app.pid, "A screenshot")?;
        if wayland_session() {
            // Rootless XWayland (GNOME, KDE) has nothing on its root window:
            // the app's own window has its pixels.
            let x11 = self.x11()?;
            if let Some(win) = x11.wm().find(app.pid, &window.title, window.bounds) {
                return x11.capture_window(win, rect);
            }
        }
        self.x11()?.capture(rect)
    }

    fn capture_screen(&mut self, region: Option<Rect>) -> Result<Capture> {
        if self.wl_captures() {
            let wl = self.wl()?;
            let rect = region.unwrap_or_else(|| wl.layout());
            return wl.capture(rect);
        }
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
        let a11y = self.bus()?;
        let (pid, r) = self.resolve_owned(element)?;
        let idx = a11y.action_index(&r, native_action)?.ok_or_else(|| {
            Error::ActionFailed(format!("element no longer offers action `{native_action}`"))
        })?;
        // Timed out: sent, and maybe done (GTK runs the handler, a modal
        // dialog included, before it answers): never to be repeated.
        let ok = a11y
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
        let a11y = self.bus()?;
        let (pid, r) = self.resolve_owned(element)?;
        let acc = a11y.describe(&r)?;
        // A request that timed out was sent: it may have set the value,
        // so nothing else is tried.
        if acc.has_iface("EditableText") || acc.states.has(state::EDITABLE) {
            match a11y.set_text(&r, value) {
                Ok(true) => return Ok(()),
                Err(e) if e.fail == Fail::Timeout => return Err(self.sent_error(pid, e)),
                _ => {}
            }
        }
        if acc.has_iface("Value")
            && let Ok(n) = value.trim().parse::<f64>()
        {
            match a11y.set_value(&r, n) {
                Ok(true) => return Ok(()),
                Err(e) if e.fail == Fail::Timeout => return Err(self.sent_error(pid, e)),
                _ => {}
            }
        }
        // Checkbox/toggle: flip to the requested boolean via its action.
        // Only for one: "activate" on a read-only field is Enter (the
        // dialog's default button), and "click" on a button presses it.
        let toggle = acc.states.has(state::CHECKABLE)
            || ["check", "radio", "toggle", "switch"]
                .iter()
                .any(|r| acc.role_name.to_lowercase().contains(r));
        let want = matches!(
            value.trim().to_lowercase().as_str(),
            "true" | "1" | "on" | "checked" | "yes"
        );
        let is = acc.states.has(state::CHECKED) || acc.states.has(state::PRESSED);
        if !toggle {
            // Nothing below applies.
        } else if is != want {
            for action in ["toggle", "click", "press", "activate"] {
                if let Some(idx) = a11y.action_index(&r, action)?
                    && a11y
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
        let a11y = self.bus()?;
        let (pid, r) = self.resolve_owned(element)?;
        // Read errors are errors: as "empty" they made a selection of
        // nothing that reported success.
        let acc = a11y.describe(&r)?;
        if !acc.has_iface("Text") {
            return Err(Error::ActionFailed(
                "this element has no text to select".into(),
            ));
        }
        let count = a11y.character_count(&r);
        if count == 0 && text.is_none() {
            return Err(Error::ActionFailed(
                "there is no text here to select (it is empty, or it couldn't be read)".into(),
            ));
        }
        let (start, end) = match text {
            None => (0, count),
            Some(needle) => {
                let hay = a11y
                    .get_text(&r, 0, count)
                    .map_err(|e| self.sent_error(pid, e))?;
                let chars: Vec<char> = hay.chars().collect();
                let needle_chars: Vec<char> = needle.chars().collect();
                let start =
                    find_nth(&chars, &needle_chars, occurrence.max(1)).ok_or_else(|| {
                        Error::ActionFailed(format!("`{needle}` not found in the text"))
                    })?;
                (start as i32, (start + needle_chars.len()) as i32)
            }
        };
        let _ = a11y.set_caret(&r, end);
        if a11y.set_selection(&r, start, end)? {
            Ok(())
        } else {
            Err(Error::ActionFailed("could not select the text".into()))
        }
    }

    fn focus(&mut self, element: ElementHandle) -> Result<Native> {
        self.revive_a11y();
        let a11y = self.bus()?;
        let r = self.resolve(element)?;
        if a11y.grab_focus(&r).unwrap_or(false) {
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
        let b = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        };
        if self.wl_points() {
            return self.wl()?.click(at.x, at.y, b, count);
        }
        self.x11_reaches(target.pid, "A click")?;
        self.x11()?
            .click(at.x.round() as i32, at.y.round() as i32, b, count)
    }

    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()> {
        if self.wl_points() {
            return self.wl()?.drag((from.x, from.y), (to.x, to.y));
        }
        self.x11_reaches(target.pid, "A drag")?;
        self.x11()?.drag(
            (from.x.round() as i32, from.y.round() as i32),
            (to.x.round() as i32, to.y.round() as i32),
        )
    }

    fn move_pointer(&mut self, target: &InputTarget, at: Point) -> Result<Option<Point>> {
        if self.wl_points() {
            // Wayland never tells where the pointer is: it stays here.
            self.wl()?.move_pointer(at.x, at.y)?;
            return Ok(None);
        }
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
        let b = match button {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        };
        if self.wl_points() {
            let strokes: Vec<Vec<(f64, f64)>> = strokes
                .iter()
                .map(|s| s.iter().map(|p| (p.x, p.y)).collect())
                .collect();
            return self.wl()?.draw(&strokes, b, pace);
        }
        self.x11_reaches(target.pid, "Drawing")?;
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
        if self.wl_points() {
            return self.wl()?.scroll(at.x, at.y, dx, dy);
        }
        self.x11_reaches(target.pid, "Scrolling")?;
        self.x11()?
            .scroll(at.x.round() as i32, at.y.round() as i32, dx, dy)
    }

    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()> {
        if self.wl_types() {
            return self.wl()?.press(combo);
        }
        self.x11_reaches(target.pid, "A key press")?;
        self.x11()?.press(combo)
    }

    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()> {
        if self.wl_types() {
            self.wl()?.type_text(text)?;
            // The keys reach the app through the compositor, after this
            // returns: let it work through them before the engine reads the
            // app again (GTK 3 can crash when its accessibility is read in
            // the middle of a burst of keys).
            let n = text.chars().count() as u64;
            std::thread::sleep(Duration::from_millis((40 + 3 * n).min(1500)));
            return Ok(());
        }
        self.x11_reaches(target.pid, "Typing")?;
        self.x11()?.type_text(text)
    }
}

impl LinuxBackend {
    /// How much larger `pid`'s windows are on the X screen than its
    /// accessibility says (1: the same), measured once per app on the
    /// windows (`frames`: title, extents) whose X window has the same
    /// title. Only in an X11 session.
    fn scale_of(&mut self, pid: u32, frames: &[Frame]) -> f64 {
        if let Some(s) = self.scales.get(&pid) {
            return *s;
        }
        if wayland_session() {
            return 1.0;
        }
        let Ok(x) = self.x11() else {
            return 1.0;
        };
        let xwins = x.wm().windows_of(pid);
        let sizes: Vec<(f64, f64, f64, f64)> = frames
            .iter()
            .filter(|(title, _)| !title.is_empty())
            .filter_map(|(title, ext)| {
                let (_, _, w, h) = (*ext)?;
                let (_, _, r, _) = xwins
                    .iter()
                    .find(|(_, t, r, shown)| t == title && *shown && r.is_some())?;
                let r = (*r)?;
                Some((f64::from(w), f64::from(h), r.width, r.height))
            })
            .collect();
        match fit_scale(&sizes) {
            Some(s) => {
                if s != 1.0 {
                    log::info!("pid {pid}: its accessibility is scaled by 1/{s} (HiDPI)");
                }
                self.scales.insert(pid, s);
                s
            }
            None => 1.0, // nothing to compare yet
        }
    }

    /// The windows of an app that isn't on the accessibility bus, as the
    /// compositor (Wayland) or the X server lists them.
    fn bare_windows(&mut self, app: &AppInfo) -> Vec<WindowInfo> {
        let pid = app.pid;
        // (key, title, where, has the keyboard, on screen, the compositor's)
        let mut found: Vec<(String, String, Rect, bool, bool, Option<Toplevel>)> = Vec::new();
        if let Some(comp) = &self.comp {
            for t in comp.toplevels().unwrap_or_default() {
                if t.pid == Some(pid) {
                    found.push((
                        format!("wl:{}", t.id),
                        t.title.clone(),
                        t.rect,
                        t.focused,
                        t.visible,
                        Some(t),
                    ));
                }
            }
        } else if let Ok(x) = self.x11() {
            let wm = x.wm();
            let front = wm.front();
            for (win, title, rect, viewable) in wm.windows_of(pid) {
                let Some(rect) = rect.filter(|r| !r.is_empty()) else {
                    continue;
                };
                let focused = matches!(front, Some(Front::Window { win: w, .. }) if w == win);
                found.push((format!("x11:{win}"), title, rect, focused, viewable, None));
            }
        }
        // The same windows keep their handles.
        let old: HashMap<String, ElementHandle> = self
            .bare
            .iter()
            .filter(|(_, b)| b.pid == pid)
            .map(|(h, b)| (b.key.clone(), *h))
            .collect();
        self.bare.retain(|_, b| b.pid != pid);
        let mut out = Vec::new();
        for (key, title, rect, focused, visible, top) in found {
            let handle = old.get(&key).copied().unwrap_or_else(|| {
                let h = self.next_handle;
                self.next_handle += 1;
                h
            });
            self.bare.insert(
                handle,
                Bare {
                    pid,
                    key: key.clone(),
                },
            );
            match top {
                Some(t) => {
                    self.toplevels.insert(handle, t);
                }
                None => {
                    self.toplevels.remove(&handle);
                }
            }
            out.push(WindowInfo {
                id: stable_id(&format!("{pid}:{key}")),
                title: if title.is_empty() {
                    app.name.clone()
                } else {
                    title
                },
                bounds: Some(rect),
                focused,
                main: focused,
                minimized: !visible,
                handle,
            });
        }
        out
    }
}

/// An accessible window as matched to the compositor's: its title, and its
/// extents (x, y, width, height).
type Frame = (String, Option<(i32, i32, i32, i32)>);

/// For each accessible window (title, extents), the compositor's window it
/// is, taken from `tops` (one window each): the only one, else the one with
/// that title, else the one whose size fits inside its frame (a frame can
/// be larger by the shadow an app draws around itself).
fn match_toplevels(frames: &[Frame], tops: &mut Vec<Toplevel>) -> Vec<Option<Toplevel>> {
    let mut out: Vec<Option<Toplevel>> = vec![None; frames.len()];
    if frames.len() == 1 && tops.len() == 1 {
        out[0] = tops.pop();
        return out;
    }
    for (i, (title, _)) in frames.iter().enumerate() {
        if let Some(k) = tops
            .iter()
            .position(|t| !title.is_empty() && t.title == *title)
        {
            out[i] = Some(tops.remove(k));
        }
    }
    for (i, (_, ext)) in frames.iter().enumerate() {
        let Some((_, _, fw, fh)) = *ext else {
            continue;
        };
        if out[i].is_some() {
            continue;
        }
        let fits = |t: &Toplevel| {
            let (dw, dh) = (f64::from(fw) - t.rect.width, f64::from(fh) - t.rect.height);
            (0.0..=160.0).contains(&dw) && (0.0..=160.0).contains(&dh)
        };
        if let Some(k) = tops
            .iter()
            .enumerate()
            .filter(|(_, t)| fits(t))
            .min_by(|a, b| {
                let d =
                    |t: &Toplevel| (f64::from(fw) - t.rect.width) + (f64::from(fh) - t.rect.height);
                d(a.1).total_cmp(&d(b.1))
            })
            .map(|(k, _)| k)
        {
            out[i] = Some(tops.remove(k));
        }
    }
    out
}

/// How far to move a native Wayland window's element coordinates (relative
/// to its own surface) to put them on screen, where the compositor has the
/// window's content at `rect`. The surface can be larger than the content
/// by a drawn shadow (GTK's client-side decorations): the frame's visible
/// children start where the content does; without them, the shadow is
/// taken to be even all round.
fn wayland_offset(walked: &[atspi::Walked], rect: Rect) -> (f64, f64) {
    let Some((fx, fy, fw, fh)) = walked.first().and_then(|w| w.data.extents) else {
        return (rect.x, rect.y);
    };
    let (dw, dh) = (
        (f64::from(fw) - rect.width).max(0.0),
        (f64::from(fh) - rect.height).max(0.0),
    );
    let shown = |w: &&atspi::Walked| {
        let s = w.data.acc.states;
        w.parent == Some(0)
            && s.has(state::SHOWING)
            && w.data
                .extents
                .is_some_and(|(_, _, cw, ch)| cw > 0 && ch > 0)
    };
    let children: Vec<(i32, i32)> = walked
        .iter()
        .filter(shown)
        .filter_map(|w| w.data.extents.map(|(x, y, _, _)| (x, y)))
        .collect();
    let (left, top) = match (
        children.iter().map(|c| c.0).min(),
        children.iter().map(|c| c.1).min(),
    ) {
        (Some(x), Some(y)) => (
            f64::from(x - fx).clamp(0.0, dw),
            f64::from(y - fy).clamp(0.0, dh),
        ),
        _ => (dw / 2.0, dh / 2.0),
    };
    (rect.x - f64::from(fx) - left, rect.y - f64::from(fy) - top)
}

/// Screen scales a HiDPI app may run at (`GDK_SCALE`, `QT_SCALE_FACTOR`).
const SCALES: [f64; 6] = [1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

/// The scale by which an app's windows are larger on the X screen than its
/// accessibility says, from (accessible width, height, X width, height) of
/// some of its windows: one of [`SCALES`] that fits every window on both
/// axes within 2 %, else 1 (never a guess); `None` with nothing to compare.
fn fit_scale(sizes: &[(f64, f64, f64, f64)]) -> Option<f64> {
    let sizes: Vec<_> = sizes
        .iter()
        .filter(|(aw, ah, xw, xh)| [aw, ah, xw, xh].iter().all(|v| **v >= 1.0))
        .collect();
    if sizes.is_empty() {
        return None;
    }
    let near = |a: f64, x: f64| (a - x).abs() <= 0.02 * x;
    let fits = |s: f64| {
        sizes
            .iter()
            .all(|(aw, ah, xw, xh)| near(aw * s, *xw) && near(ah * s, *xh))
    };
    Some(SCALES.into_iter().find(|s| fits(*s)).unwrap_or(1.0))
}

/// A rectangle in an app's own units, on the screen.
fn scaled(r: Rect, s: f64) -> Rect {
    if s == 1.0 {
        return r;
    }
    Rect::new(r.x * s, r.y * s, r.width * s, r.height * s)
}

/// What is in front, from what X11 says and (in a Wayland session whose
/// compositor doesn't tell, `blind`) the apps whose windows are active.
#[derive(Debug, PartialEq)]
enum FrontPick {
    /// This X window (and its pid, when known).
    Window(u32, Option<u32>),
    /// This accessible app's window.
    App(u32),
    Desktop,
    /// Something, but not something accessible (or several say they are).
    Unknown,
    CantTell,
}

fn pick_front(blind: bool, x11: Option<Front>, active: &[u32]) -> FrontPick {
    match x11 {
        // An X window has the focus: X11 knows which.
        Some(Front::Window { win, pid }) => FrontPick::Window(win, pid),
        // X11 sees no window, or nothing: a native Wayland one may be.
        _ if blind => {
            let mut pids = active.to_vec();
            pids.sort_unstable();
            pids.dedup();
            match pids.as_slice() {
                [pid] => FrontPick::App(*pid),
                _ => FrontPick::Unknown,
            }
        }
        Some(Front::Nothing) => FrontPick::Desktop,
        None => FrontPick::CantTell,
    }
}

/// Environment that has a launched app's toolkit expose its accessibility
/// whatever the desktop's setting (Qt, then GTK 2/older apps, Firefox and
/// Chromium): those of `vars` that `set` says aren't set already.
fn a11y_env(set: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    [
        ("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1"),
        ("ACCESSIBILITY_ENABLED", "1"),
        ("GNOME_ACCESSIBILITY", "1"),
    ]
    .into_iter()
    .filter(|(k, _)| !set(k))
    .collect()
}

fn with_a11y_env(cmd: &mut Command) {
    cmd.envs(a11y_env(|k| std::env::var_os(k).is_some()));
}

/// What an error says, without its kind's prefix ("platform error: ").
fn reason(e: &Error) -> String {
    match e {
        Error::Platform(m) | Error::Unsupported(m) => m.clone(),
        other => other.to_string(),
    }
}

/// The error of an accessibility call without an accessibility bus.
fn no_bus(why: &str) -> Error {
    Error::Unsupported(format!(
        "no accessibility bus: {why}. Element trees and element_index actions need AT-SPI (at-spi2-core, with a D-Bus session); screenshots, keyboard/mouse input and windows still work."
    ))
}

/// The accessibility entry of `permissions`: no bus (and why), or a bus and
/// whether accessibility is switched on for the session.
fn a11y_status(no_bus: Option<&str>, enabled: Option<bool>) -> PermissionStatus {
    let (granted, detail) = match (no_bus, enabled) {
        (Some(why), _) => (
            false,
            format!(
                "no accessibility bus ({why}): no element trees; screenshots, input and windows still work (tried again every {} s)",
                X11_RETRY.as_secs()
            ),
        ),
        (None, Some(false)) => (
            false,
            "connected, but accessibility is switched off for the session (org.a11y.Status IsEnabled = false) and couldn't be switched on: Qt, Firefox and Chromium apps won't show their elements".into(),
        ),
        (None, Some(true)) => (true, "connected; accessibility is on".into()),
        (None, None) => (true, "connected".into()),
    };
    PermissionStatus {
        name: "AT-SPI accessibility bus".into(),
        granted,
        detail,
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
    wayland::session()
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
    fn a_hidpi_apps_scale_is_found_only_when_clear() {
        // GDK_SCALE=2: 400x300 to accessibility, 800x600 on the screen.
        assert_eq!(fit_scale(&[(400.0, 300.0, 800.0, 600.0)]), Some(2.0));
        assert_eq!(
            fit_scale(&[(400.0, 300.0, 800.0, 600.0), (200.0, 100.0, 401.0, 199.0)]),
            Some(2.0)
        );
        assert_eq!(fit_scale(&[(400.0, 300.0, 600.0, 450.0)]), Some(1.5));
        assert_eq!(fit_scale(&[(401.0, 300.0, 501.0, 375.0)]), Some(1.25));
        assert_eq!(fit_scale(&[(400.0, 300.0, 400.0, 300.0)]), Some(1.0));
        // A frame around the window, or windows that disagree: no scaling.
        assert_eq!(fit_scale(&[(400.0, 330.0, 400.0, 300.0)]), Some(1.0));
        assert_eq!(fit_scale(&[(400.0, 300.0, 800.0, 300.0)]), Some(1.0));
        assert_eq!(
            fit_scale(&[(400.0, 300.0, 800.0, 600.0), (400.0, 300.0, 400.0, 300.0)]),
            Some(1.0)
        );
        // Nothing to compare.
        assert_eq!(fit_scale(&[]), None);
        assert_eq!(fit_scale(&[(0.0, 0.0, 800.0, 600.0)]), None);
        let r = scaled(Rect::new(10.0, 20.0, 30.0, 40.0), 2.0);
        assert_eq!((r.x, r.y, r.width, r.height), (20.0, 40.0, 60.0, 80.0));
    }

    #[test]
    fn a_native_wayland_window_in_front_is_found_by_accessibility() {
        let win = Some(Front::Window {
            win: 7,
            pid: Some(42),
        });
        // An X window in front: X11 knows best, in any session.
        assert_eq!(pick_front(false, win, &[]), FrontPick::Window(7, Some(42)));
        assert_eq!(pick_front(true, win, &[9]), FrontPick::Window(7, Some(42)));
        // X11 sees nothing: an X11 session's desktop...
        assert_eq!(
            pick_front(false, Some(Front::Nothing), &[9]),
            FrontPick::Desktop
        );
        assert_eq!(pick_front(false, None, &[9]), FrontPick::CantTell);
        // ... but on GNOME/KDE Wayland a native window may have the focus.
        assert_eq!(
            pick_front(true, Some(Front::Nothing), &[9, 9]),
            FrontPick::App(9)
        );
        assert_eq!(pick_front(true, None, &[9]), FrontPick::App(9));
        assert_eq!(
            pick_front(true, Some(Front::Nothing), &[]),
            FrontPick::Unknown
        );
        assert_eq!(
            pick_front(true, Some(Front::Nothing), &[9, 3]),
            FrontPick::Unknown
        );
    }

    #[test]
    fn launched_apps_expose_their_accessibility() {
        let all = a11y_env(|_| false);
        assert_eq!(
            all,
            [
                ("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1"),
                ("ACCESSIBILITY_ENABLED", "1"),
                ("GNOME_ACCESSIBILITY", "1"),
            ]
        );
        // The user's own setting stays.
        let some = a11y_env(|k| k == "GNOME_ACCESSIBILITY");
        assert_eq!(some.len(), 2);
        assert!(!some.iter().any(|(k, _)| *k == "GNOME_ACCESSIBILITY"));
    }

    #[test]
    fn accessibility_is_reported_as_it_is() {
        let s = a11y_status(Some("no session bus"), None);
        assert!(!s.granted);
        assert!(s.detail.contains("no session bus"), "{}", s.detail);
        assert!(s.detail.contains("screenshots"), "{}", s.detail);
        assert!(!a11y_status(None, Some(false)).granted);
        assert!(a11y_status(None, Some(true)).granted);
        assert!(a11y_status(None, None).granted);
        assert_eq!(
            reason(&Error::Platform("AT-SPI: gone".into())),
            "AT-SPI: gone"
        );
        let e = no_bus("x");
        assert!(matches!(e, Error::Unsupported(ref m) if m.starts_with("no accessibility bus: x")));
    }

    #[test]
    fn stable_id_is_deterministic() {
        assert_eq!(stable_id("/org/a11y/x"), stable_id("/org/a11y/x"));
        assert_ne!(stable_id("/a"), stable_id("/b"));
    }
}
