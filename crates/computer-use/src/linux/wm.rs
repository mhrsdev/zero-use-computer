//! Window management on X11 through the conventions every window manager
//! implements (EWMH `_NET_*` client messages and ICCCM), with plain X
//! requests when no window manager is running. Displays come from RandR.

use x11rb::connection::Connection as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
    InputFocus, MapState, StackMode, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::error::{Error, Result};
use crate::types::{Display, Rect, WindowOp};

pub struct Wm<'a> {
    conn: &'a RustConnection,
    root: Window,
}

fn xe(e: impl std::fmt::Display) -> Error {
    Error::Platform(format!("X11: {e}"))
}

// _NET_WM_STATE actions.
const REMOVE: u32 = 0;
const ADD: u32 = 1;
// "Source indication": a pager / tool acting for the user.
const SOURCE: u32 = 2;

impl<'a> Wm<'a> {
    pub fn new(conn: &'a RustConnection, root: Window) -> Self {
        Self { conn, root }
    }

    fn atom(&self, name: &str) -> Option<Atom> {
        self.conn
            .intern_atom(false, name.as_bytes())
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    }

    fn prop32(&self, win: Window, prop: &str, kind: impl Into<Atom>) -> Option<Vec<u32>> {
        let prop = self.atom(prop)?;
        let r = self
            .conn
            .get_property(false, win, prop, kind, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        Some(r.value32()?.collect())
    }

    fn text(&self, win: Window) -> Option<String> {
        let utf8 = self.atom("UTF8_STRING")?;
        let name = self.atom("_NET_WM_NAME")?;
        for (prop, kind) in [
            (name, utf8),
            (AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()),
        ] {
            if let Ok(c) = self.conn.get_property(false, win, prop, kind, 0, 1024)
                && let Ok(r) = c.reply()
                && !r.value.is_empty()
            {
                return Some(String::from_utf8_lossy(&r.value).into_owned());
            }
        }
        None
    }

    /// Whether a window manager runs, and what it supports.
    fn has_wm(&self) -> bool {
        self.prop32(self.root, "_NET_SUPPORTING_WM_CHECK", AtomEnum::WINDOW)
            .is_some_and(|v| !v.is_empty())
    }

    fn supports(&self, name: &str) -> bool {
        let Some(atom) = self.atom(name) else {
            return false;
        };
        self.has_wm()
            && self
                .prop32(self.root, "_NET_SUPPORTED", AtomEnum::ATOM)
                .is_some_and(|v| v.contains(&atom))
    }

    /// Where a window is on screen.
    fn geometry(&self, win: Window) -> Option<Rect> {
        let g = self.conn.get_geometry(win).ok()?.reply().ok()?;
        let t = self
            .conn
            .translate_coordinates(win, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some(Rect::new(
            f64::from(t.dst_x),
            f64::from(t.dst_y),
            f64::from(g.width),
            f64::from(g.height),
        ))
    }

    fn viewable(&self, win: Window) -> bool {
        self.conn
            .get_window_attributes(win)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|a| a.map_state == MapState::VIEWABLE)
    }

    /// The X window behind an app window: same process, then the same title,
    /// then the closest position.
    pub fn find(&self, pid: u32, title: &str, bounds: Option<Rect>) -> Option<Window> {
        let clients = self
            .prop32(self.root, "_NET_CLIENT_LIST", AtomEnum::WINDOW)
            .filter(|v| !v.is_empty())
            .or_else(|| {
                let tree = self.conn.query_tree(self.root).ok()?.reply().ok()?;
                Some(tree.children)
            })?;
        let mine: Vec<Window> = clients
            .into_iter()
            .filter(|w| {
                self.prop32(*w, "_NET_WM_PID", AtomEnum::CARDINAL)
                    .is_some_and(|p| p.first() == Some(&pid))
            })
            .collect();
        let titled: Vec<Window> = mine
            .iter()
            .copied()
            .filter(|w| self.text(*w).as_deref() == Some(title))
            .collect();
        let pool = if titled.is_empty() { mine } else { titled };
        let dist = |w: &Window| -> f64 {
            match (bounds, self.geometry(*w)) {
                (Some(a), Some(b)) => (a.x - b.x).abs() + (a.y - b.y).abs(),
                _ => 0.0,
            }
        };
        pool.into_iter()
            .map(|w| (!self.viewable(w), dist(&w), w))
            .min_by(|a, b| {
                (a.0, a.1)
                    .partial_cmp(&(b.0, b.1))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(_, _, w)| w)
    }

    /// A client message to the window manager about `win`.
    fn tell_wm(&self, win: Window, kind: &str, data: [u32; 5]) -> Result<()> {
        let kind = self
            .atom(kind)
            .ok_or_else(|| Error::Platform(format!("no atom {kind}")))?;
        let ev = ClientMessageEvent::new(32, win, kind, data);
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                ev,
            )
            .map_err(xe)?;
        Ok(())
    }

    fn set_state(&self, win: Window, action: u32, states: &[&str]) -> Result<()> {
        let a = |n: &str| self.atom(n).unwrap_or(0);
        let first = states.first().map_or(0, |s| a(s));
        let second = states.get(1).map_or(0, |s| a(s));
        self.tell_wm(win, "_NET_WM_STATE", [action, first, second, SOURCE, 0])
    }

    fn state_has(&self, win: Window, name: &str) -> bool {
        match (
            self.atom(name),
            self.prop32(win, "_NET_WM_STATE", AtomEnum::ATOM),
        ) {
            (Some(atom), Some(v)) => v.contains(&atom),
            _ => false,
        }
    }

    fn set_bounds(&self, win: Window, r: Rect) -> Result<()> {
        if self.state_has(win, "_NET_WM_STATE_MAXIMIZED_VERT")
            || self.state_has(win, "_NET_WM_STATE_MAXIMIZED_HORZ")
        {
            self.set_state(
                win,
                REMOVE,
                &[
                    "_NET_WM_STATE_MAXIMIZED_VERT",
                    "_NET_WM_STATE_MAXIMIZED_HORZ",
                ],
            )?;
        }
        let (x, y) = (r.x.round() as i32, r.y.round() as i32);
        if self.supports("_NET_MOVERESIZE_WINDOW") {
            // Window bounds (as the accessibility tree reports them) include
            // the window manager's frame: put the frame's corner at x, y
            // (north-west gravity) and size the client to fit inside it.
            let e = self
                .prop32(win, "_NET_FRAME_EXTENTS", AtomEnum::CARDINAL)
                .filter(|v| v.len() >= 4)
                .unwrap_or_else(|| vec![0; 4]);
            let [l, rt, t, b] = [e[0], e[1], e[2], e[3]].map(f64::from);
            let w = (r.width - l - rt).round().max(1.0) as u32;
            let h = (r.height - t - b).round().max(1.0) as u32;
            let flags = 1 | (0xF << 8) | (SOURCE << 12);
            return self.tell_wm(
                win,
                "_NET_MOVERESIZE_WINDOW",
                [flags, x as u32, y as u32, w, h],
            );
        }
        let (w, h) = (
            r.width.round().max(1.0) as u32,
            r.height.round().max(1.0) as u32,
        );
        self.conn
            .configure_window(win, &ConfigureWindowAux::new().x(x).y(y).width(w).height(h))
            .map_err(xe)?;
        Ok(())
    }

    fn activate(&self, win: Window) -> Result<()> {
        if self.supports("_NET_ACTIVE_WINDOW") {
            return self.tell_wm(win, "_NET_ACTIVE_WINDOW", [SOURCE, 0, 0, 0, 0]);
        }
        self.conn.map_window(win).map_err(xe)?;
        self.conn
            .configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))
            .map_err(xe)?;
        if self.viewable(win) {
            self.conn
                .set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)
                .map_err(xe)?;
        }
        Ok(())
    }

    /// The work area of the display showing `win`.
    fn area_of(&self, win: Window) -> Option<Rect> {
        let g = self.geometry(win)?;
        let displays = self.displays();
        displays
            .iter()
            .find(|d| d.bounds.contains(g.center()))
            .or(displays.first())
            .map(|d| d.work_area)
    }

    pub fn apply(&self, win: Window, op: &WindowOp) -> Result<()> {
        match *op {
            WindowOp::Focus => self.activate(win)?,
            WindowOp::SetBounds(r) => self.set_bounds(win, r)?,
            WindowOp::Maximize => {
                if self.supports("_NET_WM_STATE_MAXIMIZED_VERT") {
                    self.set_state(
                        win,
                        ADD,
                        &[
                            "_NET_WM_STATE_MAXIMIZED_VERT",
                            "_NET_WM_STATE_MAXIMIZED_HORZ",
                        ],
                    )?;
                } else {
                    let area = self
                        .area_of(win)
                        .ok_or_else(|| Error::ActionFailed("no display for the window".into()))?;
                    self.set_bounds(win, area)?;
                }
            }
            WindowOp::Minimize => {
                if self.has_wm() {
                    // ICCCM: ask to become iconic.
                    self.tell_wm(win, "WM_CHANGE_STATE", [3, 0, 0, 0, 0])?;
                } else {
                    self.conn.unmap_window(win).map_err(xe)?;
                }
            }
            WindowOp::Restore => {
                if self.has_wm() {
                    self.set_state(
                        win,
                        REMOVE,
                        &[
                            "_NET_WM_STATE_MAXIMIZED_VERT",
                            "_NET_WM_STATE_MAXIMIZED_HORZ",
                        ],
                    )?;
                    self.set_state(win, REMOVE, &["_NET_WM_STATE_FULLSCREEN"])?;
                }
                self.activate(win)?;
            }
            WindowOp::Fullscreen(on) => {
                if self.supports("_NET_WM_STATE_FULLSCREEN") {
                    let action = if on { ADD } else { REMOVE };
                    self.set_state(win, action, &["_NET_WM_STATE_FULLSCREEN"])?;
                } else {
                    return Err(Error::Unsupported(
                        "full screen needs a window manager".into(),
                    ));
                }
            }
            WindowOp::Close => {
                if self.supports("_NET_CLOSE_WINDOW") {
                    self.tell_wm(win, "_NET_CLOSE_WINDOW", [0, SOURCE, 0, 0, 0])?;
                } else {
                    // ICCCM: ask the app itself (as its close button would).
                    let (Some(protocols), Some(delete)) =
                        (self.atom("WM_PROTOCOLS"), self.atom("WM_DELETE_WINDOW"))
                    else {
                        return Err(Error::ActionFailed("cannot ask the window to close".into()));
                    };
                    let supported = self
                        .prop32(win, "WM_PROTOCOLS", AtomEnum::ATOM)
                        .is_some_and(|v| v.contains(&delete));
                    if !supported {
                        return Err(Error::ActionFailed(
                            "this window doesn't accept a close request; use its own close button"
                                .into(),
                        ));
                    }
                    let ev = ClientMessageEvent::new(32, win, protocols, [delete, 0, 0, 0, 0]);
                    self.conn
                        .send_event(false, win, EventMask::NO_EVENT, ev)
                        .map_err(xe)?;
                }
            }
            WindowOp::ToDesktop(n) => {
                if !self.supports("_NET_WM_DESKTOP") {
                    return Err(Error::Unsupported(
                        "no virtual desktops here (the window manager doesn't offer them)".into(),
                    ));
                }
                self.tell_wm(win, "_NET_WM_DESKTOP", [n, SOURCE, 0, 0, 0])?;
            }
        }
        self.conn.flush().map_err(xe)?;
        Ok(())
    }

    pub fn displays(&self) -> Vec<Display> {
        let setup = self.conn.setup();
        let screen = setup
            .roots
            .iter()
            .find(|s| s.root == self.root)
            .unwrap_or(&setup.roots[0]);
        let whole = Rect::new(
            0.0,
            0.0,
            f64::from(screen.width_in_pixels),
            f64::from(screen.height_in_pixels),
        );
        let mut out: Vec<Display> = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                r.monitors
                    .iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let b = Rect::new(
                            f64::from(m.x),
                            f64::from(m.y),
                            f64::from(m.width),
                            f64::from(m.height),
                        );
                        Display {
                            index: i as u32,
                            bounds: b,
                            work_area: b,
                            primary: m.primary,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if out.is_empty() {
            out.push(Display {
                index: 0,
                bounds: whole,
                work_area: whole,
                primary: true,
            });
        }
        if !out.iter().any(|d| d.primary) {
            out[0].primary = true;
        }
        // The window manager's work area (panels excluded), per display.
        if let Some(wa) = self
            .prop32(self.root, "_NET_WORKAREA", AtomEnum::CARDINAL)
            .filter(|v| v.len() >= 4)
        {
            let wa = Rect::new(
                f64::from(wa[0] as i32),
                f64::from(wa[1] as i32),
                f64::from(wa[2]),
                f64::from(wa[3]),
            );
            for d in &mut out {
                let x0 = d.bounds.x.max(wa.x);
                let y0 = d.bounds.y.max(wa.y);
                let x1 = (d.bounds.x + d.bounds.width).min(wa.x + wa.width);
                let y1 = (d.bounds.y + d.bounds.height).min(wa.y + wa.height);
                if x1 > x0 && y1 > y0 {
                    d.work_area = Rect::new(x0, y0, x1 - x0, y1 - y0);
                }
            }
        }
        out
    }

    pub fn desktops(&self) -> Option<(u32, u32)> {
        if !self.has_wm() {
            return None;
        }
        let n = *self
            .prop32(self.root, "_NET_NUMBER_OF_DESKTOPS", AtomEnum::CARDINAL)?
            .first()?;
        let cur = self
            .prop32(self.root, "_NET_CURRENT_DESKTOP", AtomEnum::CARDINAL)
            .and_then(|v| v.first().copied())
            .unwrap_or(0);
        Some((n, cur))
    }
}
