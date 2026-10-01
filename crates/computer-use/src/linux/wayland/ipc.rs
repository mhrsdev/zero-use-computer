//! The compositor's IPC: Hyprland's and sway's control sockets.
//!
//! On Wayland an app doesn't know where its windows are, and other programs
//! can't move or focus them: only the compositor can. Hyprland and sway
//! answer over a Unix socket: where every window is (in the logical layout
//! coordinates the outputs are placed in), which one has the keyboard, the
//! outputs; and they take commands to focus, move, resize or close windows
//! and to bind keys. Both protocols are spoken here directly (`hyprctl` and
//! `swaymsg` may not be installed), with a timeout on every socket operation
//! so a hung compositor can't hang the server.
//!
//! - Hyprland: `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`,
//!   one text request per connection (`j/clients`, `/dispatch …`); the
//!   reply is what comes back until Hyprland closes the connection.
//! - sway: `$SWAYSOCK`, i3-ipc framing ("i3-ipc", a u32 length and a u32
//!   type in native byte order, then the payload; JSON replies).
//!
//! Coordinates are logical (layout) pixels everywhere: an output's rectangle
//! is its position and its mode divided by its scale.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, NamedKey, Pad};
use crate::types::{Display, Rect, WindowOp};

/// How long one socket operation may take before the compositor counts as hung.
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for a change to show in the compositor's window list.
const SETTLE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(20);
/// No compositor answers with more than this.
const MAX_REPLY: u64 = 64 << 20;

/// Every key binding [`Compositor::bind_key`] makes has this in its command,
/// so a binding left over by an earlier run (one that crashed) can be told
/// from the user's own: only bindings with it are ever replaced or removed.
pub const BIND_MARKER: &str = "computer-use-overlay";

/// Hyprland has no minimized state; a special workspace nobody shows is the
/// usual stand-in (and what Hyprland users' "minimize" scripts use).
const HYPR_MINIMIZED: &str = "special:minimized";
/// sway's scratchpad: windows put away, shown again with `scratchpad show`.
const SWAY_SCRATCH: &str = "__i3_scratch";

// i3-ipc message types.
const I3_MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const GET_WORKSPACES: u32 = 1;
const GET_OUTPUTS: u32 = 3;
const GET_TREE: u32 = 4;
const GET_VERSION: u32 = 7;
const GET_CONFIG: u32 = 9;

// Modifier bits, as Hyprland's `j/binds` reports them (wlroots' order).
const SHIFT: u32 = 1;
const CTRL: u32 = 1 << 2;
const ALT: u32 = 1 << 3;
const LOGO: u32 = 1 << 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hyprland,
    Sway,
}

/// The session's compositor, reached through its IPC socket. Each request
/// opens its own connection, so this is cheap to keep and to share.
#[derive(Debug, Clone)]
pub struct Compositor {
    kind: Kind,
    socket: PathBuf,
    timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toplevel {
    /// The compositor's own id: Hyprland's window address ("0x55d0c1e2a9b0"), sway's con_id as a string.
    pub id: String,
    pub pid: Option<u32>,
    pub title: String,
    /// Wayland app_id, or the X11 class of an XWayland window.
    pub app_id: String,
    /// The window's content box (no compositor borders/title bar), in logical layout coordinates.
    pub rect: Rect,
    /// Has the keyboard focus.
    pub focused: bool,
    /// Shown on some output now (its workspace is visible on a monitor; not minimized / in a hidden special workspace or scratchpad).
    pub visible: bool,
    pub fullscreen: bool,
    pub floating: bool,
    /// An X11 app running under XWayland.
    pub xwayland: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    /// Logical layout rectangle (Hyprland: position, and pixel size divided by scale, width/height swapped for 90/270° transforms).
    pub rect: Rect,
    /// Rectangle minus bars/panels (Hyprland "reserved"; sway: from get_workspaces' rect of the visible workspace, or equal to rect).
    pub work_area: Rect,
    pub scale: f64,
    pub focused: bool,
}

/// A window with what the actions need beyond [`Toplevel`].
#[derive(Debug, Clone)]
struct Win {
    top: Toplevel,
    /// Workspace number: Hyprland's workspace id, sway's `num`.
    ws_num: Option<i64>,
    /// Hyprland: maximized (its "fullscreen mode 1"). sway: maximized by us.
    maximized: bool,
    /// Put away: in Hyprland's special:minimized, or sway's hidden scratchpad.
    minimized: bool,
    /// sway: the borders and title bar around the content box (left, top,
    /// right, bottom), which `resize set` and `move position` include.
    frame: [f64; 4],
}

/// Full-screen states of a Hyprland window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fs {
    Normal,
    Maximized,
    Full,
}

impl Win {
    fn fs(&self) -> Fs {
        if self.top.fullscreen {
            Fs::Full
        } else if self.maximized {
            Fs::Maximized
        } else {
            Fs::Normal
        }
    }
}

fn gone() -> Error {
    Error::ActionFailed(
        "the window is no longer there (it was closed); list the windows again".into(),
    )
}

impl Compositor {
    /// The compositor this session runs, from the environment: Hyprland when
    /// HYPRLAND_INSTANCE_SIGNATURE is set, else sway when SWAYSOCK is set.
    /// None otherwise (GNOME, KDE, X11...), or when the socket isn't there
    /// (a variable left over from another session). Doesn't block.
    pub fn detect() -> Option<Self> {
        let (kind, socket) = find_socket(
            std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
            std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
            std::env::var_os("SWAYSOCK").as_deref(),
            |p| p.exists(),
        )?;
        Some(Self::new(kind, socket))
    }

    /// The compositor of `kind` that listens on `socket`.
    pub fn new(kind: Kind, socket: PathBuf) -> Self {
        Self {
            kind,
            socket,
            timeout: IO_TIMEOUT,
        }
    }

    #[cfg(test)]
    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            Kind::Hyprland => "Hyprland",
            Kind::Sway => "sway",
        }
    }

    /// Every window the compositor manages, on any workspace.
    pub fn toplevels(&self) -> Result<Vec<Toplevel>> {
        Ok(self.windows()?.into_iter().map(|w| w.top).collect())
    }

    /// The window with the keyboard focus; None when no window has it.
    pub fn active(&self) -> Result<Option<Toplevel>> {
        Ok(self
            .windows()?
            .into_iter()
            .find(|w| w.top.focused)
            .map(|w| w.top))
    }

    /// Give it the keyboard (switching to its workspace if it is on another
    /// one; un-minimizing it if needed), and check it got it: it, or
    /// another window of its process (a dialog it has open), has the focus.
    /// On Hyprland this also moves the pointer onto it (unless the user set
    /// `cursor:no_warps`).
    pub fn focus(&self, t: &Toplevel) -> Result<()> {
        self.focus_window(t, true)
    }

    fn focus_window(&self, t: &Toplevel, same_app: bool) -> Result<()> {
        let w = self.window(&t.id)?;
        match self.kind {
            Kind::Hyprland => {
                let addr = hypr_addr(&t.id)?;
                if w.minimized {
                    self.hypr_unminimize(&w)?;
                }
                self.dispatch(&format!("focuswindow address:{addr}"))?;
            }
            Kind::Sway => {
                // (Focus alone would show it from the scratchpad, but keep
                // it a scratchpad window.)
                if w.minimized {
                    self.sway_unminimize(w.clone())?;
                }
                self.sway_run(&format!("{} focus", sway_con(&t.id)?))?;
            }
        }
        self.await_focus(&w.top, same_app)
    }

    /// Wait until `t` (or, with `same_app`, another window of its process)
    /// has the keyboard.
    fn await_focus(&self, t: &Toplevel, same_app: bool) -> Result<()> {
        let deadline = Instant::now() + SETTLE;
        loop {
            let active = self.active()?;
            if let Some(a) = &active
                && (a.id == t.id || (same_app && t.pid.is_some() && a.pid == t.pid))
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                let holder = match active {
                    Some(a) => format!(" (\"{}\" has it)", a.title),
                    None => String::new(),
                };
                return Err(Error::ActionFailed(format!(
                    "{} did not give the window the keyboard focus{holder}",
                    self.name()
                )));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Change a window.
    ///
    /// - `SetBounds`: the content box, in logical layout coordinates. Only a
    ///   floating window has a position of its own, so a tiled window is
    ///   made floating first. The app may keep a minimum size; that is no
    ///   error.
    /// - `Maximize`: Hyprland's maximized state (its "fullscreen 1"). sway
    ///   has none: the window is made floating and sized to its output's
    ///   work area, and `Restore` puts it back.
    /// - `Minimize`: Hyprland has no minimized state either: the window goes
    ///   to the special workspace "special:minimized", which nothing shows.
    ///   sway: the scratchpad. `Restore` (and `Focus`) bring it back onto
    ///   the workspace on screen.
    /// - `Restore`: back from minimized, maximized or full screen, focused.
    /// - `ToDesktop(n)`: to workspace n+1, without following it.
    /// - `Close`: asks the app, as its close button does.
    ///
    /// Each waits (up to half a second) until the compositor shows the
    /// change, and fails if a state change (full screen, minimized,
    /// workspace, focus) doesn't show.
    pub fn apply(&self, t: &Toplevel, op: &WindowOp) -> Result<()> {
        if let WindowOp::SetBounds(r) = op {
            check_bounds(r)?;
        }
        let w = self.window(&t.id)?;
        match self.kind {
            Kind::Hyprland => self.hypr_apply(w, op),
            Kind::Sway => self.sway_apply(w, op),
        }
    }

    /// The outputs (monitors) in use, in logical layout coordinates.
    pub fn outputs(&self) -> Result<Vec<Output>> {
        match self.kind {
            Kind::Hyprland => Ok(hypr_outputs(&self.hypr_json("monitors")?)),
            Kind::Sway => {
                let outputs = self.sway(GET_OUTPUTS, "")?;
                let workspaces = self.sway(GET_WORKSPACES, "")?;
                Ok(sway_outputs(&outputs, &workspaces))
            }
        }
    }

    /// Displays as the backend reports them (index, bounds, work_area, primary = focused output or first).
    pub fn displays(&self) -> Result<Vec<Display>> {
        let outputs = self.outputs()?;
        let primary = outputs.iter().position(|o| o.focused).unwrap_or(0);
        Ok(outputs
            .into_iter()
            .enumerate()
            .map(|(i, o)| Display {
                index: i as u32,
                bounds: o.rect,
                work_area: o.work_area,
                primary: i == primary,
            })
            .collect())
    }

    /// Bind `combo` globally so pressing it runs `command` (a shell command
    /// line). The command gets [`BIND_MARKER`] (as a shell comment) if it
    /// doesn't have it.
    ///
    /// Returns Ok(false), binding nothing, when the user has a binding on
    /// that combination: theirs is never overridden or removed. A binding
    /// there that has the marker is ours, left by an earlier run: it is
    /// replaced. sway can't list its bindings, so there "the user's" means
    /// those in its config files (with what they include); one made at run
    /// time with `swaymsg bindsym` can't be seen and gets replaced.
    ///
    /// A config reload (Hyprland reloads by itself when its config file
    /// changes) drops the binding: calling this again re-binds it.
    pub fn bind_key(&self, combo: &KeyCombo, command: &str) -> Result<bool> {
        if command.trim().is_empty() || command.contains(['\n', '\r', '\0']) {
            return Err(Error::InvalidArgs(
                "the command of a key binding must be one non-empty line".into(),
            ));
        }
        let command = if command.contains(BIND_MARKER) {
            command.to_string()
        } else {
            format!("{command} # {BIND_MARKER}")
        };
        let chord = Chord::of(combo)?;
        match self.kind {
            Kind::Hyprland => {
                let found = hypr_binds_on(&self.hypr_json("binds")?, &chord);
                if found.iter().any(|b| !b.ours) {
                    return Ok(false);
                }
                for key in stale_keys(&found) {
                    self.hypr_cmd(&format!("keyword unbind {}, {key}", chord.hypr_mods()))?;
                }
                // Hyprland's config parser cuts lines at '#' ("##" is a '#').
                let command = command.replace('#', "##");
                self.hypr_cmd(&format!(
                    "keyword bind {}, {}, exec, {command}",
                    chord.hypr_mods(),
                    chord.name
                ))?;
                Ok(true)
            }
            Kind::Sway => {
                if has_unquoted(&command, &[';', ',']) {
                    return Err(Error::InvalidArgs(
                        "sway ends a command at ';' or ',': quote them in the key binding's command, or wrap it in sh -c '…'".into(),
                    ));
                }
                if self
                    .sway_user_binds()?
                    .iter()
                    .any(|b| b.is(&chord) && !b.ours)
                {
                    return Ok(false);
                }
                // A binding on the same keys made at run time (ours from an
                // earlier run) is replaced.
                self.sway_run(&format!(
                    "bindsym --no-repeat {} exec {command}",
                    chord.sway()
                ))?;
                Ok(true)
            }
        }
    }

    /// Remove a binding made by [`bind_key`](Self::bind_key): only ours
    /// (with [`BIND_MARKER`]); when the user has one on that combination,
    /// nothing is touched (a removal would take theirs too).
    pub fn unbind_key(&self, combo: &KeyCombo) -> Result<()> {
        let chord = Chord::of(combo)?;
        match self.kind {
            Kind::Hyprland => {
                let found = hypr_binds_on(&self.hypr_json("binds")?, &chord);
                if found.iter().any(|b| !b.ours) {
                    return Ok(());
                }
                for key in stale_keys(&found) {
                    self.hypr_cmd(&format!("keyword unbind {}, {key}", chord.hypr_mods()))?;
                }
                Ok(())
            }
            Kind::Sway => {
                if self
                    .sway_user_binds()?
                    .iter()
                    .any(|b| b.is(&chord) && !b.ours)
                {
                    return Ok(());
                }
                match self.sway_run(&format!("unbindsym {}", chord.sway())) {
                    Err(Error::ActionFailed(e)) if e.contains("Could not find binding") => Ok(()),
                    other => other,
                }
            }
        }
    }

    // ---- Shared plumbing ----

    fn connect(&self) -> Result<UnixStream> {
        connect(&self.socket, self.timeout).map_err(|e| self.io_error(e))
    }

    fn io_error(&self, e: io::Error) -> Error {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::Platform(format!(
                "{} didn't answer on its IPC socket within {:.1} s (the compositor may be hung)",
                self.name(),
                self.timeout.as_secs_f64()
            )),
            _ => Error::Platform(format!(
                "{}'s IPC socket {}: {e}",
                self.name(),
                self.socket.display()
            )),
        }
    }

    fn windows(&self) -> Result<Vec<Win>> {
        match self.kind {
            Kind::Hyprland => {
                let clients = self.hypr_json("clients")?;
                let monitors = self.hypr_json("monitors")?;
                let active = self.hypr_json("activewindow")?;
                Ok(hypr_windows(&clients, &monitors, &active))
            }
            Kind::Sway => {
                let mut wins = sway_windows(&self.sway(GET_TREE, "")?);
                for w in &mut wins {
                    w.maximized =
                        w.top.floating && recall(&self.socket, &w.top.id, Put::Maximized).is_some();
                }
                Ok(wins)
            }
        }
    }

    fn window(&self, id: &str) -> Result<Win> {
        self.windows()?
            .into_iter()
            .find(|w| w.top.id == id)
            .ok_or_else(gone)
    }

    /// Poll the window until `done` holds or [`SETTLE`] passes; its last
    /// state either way (None: it's gone).
    fn settle(&self, id: &str, done: impl Fn(&Win) -> bool) -> Result<Option<Win>> {
        let deadline = Instant::now() + SETTLE;
        loop {
            let w = self.windows()?.into_iter().find(|w| w.top.id == id);
            match w {
                Some(w) if done(&w) => return Ok(Some(w)),
                w if Instant::now() >= deadline => return Ok(w),
                _ => std::thread::sleep(POLL),
            }
        }
    }

    /// Like [`settle`](Self::settle), but an error when `done` never holds.
    fn expect(&self, id: &str, done: impl Fn(&Win) -> bool, failed: &str) -> Result<Win> {
        match self.settle(id, &done)? {
            Some(w) if done(&w) => Ok(w),
            Some(_) => Err(Error::ActionFailed(format!(
                "{} accepted the request but {failed}",
                self.name()
            ))),
            None => Err(gone()),
        }
    }

    // ---- Hyprland ----

    /// One request to Hyprland, and its reply.
    fn hypr(&self, request: &str) -> Result<String> {
        let mut s = self.connect()?;
        s.write_all(request.as_bytes())
            .map_err(|e| self.io_error(e))?;
        // Hyprland reads the request until a short read; the end of our
        // side makes sure that read can't wait.
        let _ = s.shutdown(std::net::Shutdown::Write);
        let mut reply = Vec::new();
        Read::by_ref(&mut s)
            .take(MAX_REPLY)
            .read_to_end(&mut reply)
            .map_err(|e| self.io_error(e))?;
        Ok(String::from_utf8_lossy(&reply).into_owned())
    }

    fn hypr_json(&self, what: &str) -> Result<Value> {
        let reply = self.hypr(&format!("j/{what}"))?;
        serde_json::from_str(&reply).map_err(|e| {
            Error::Platform(format!(
                "Hyprland's answer to `{what}` isn't JSON ({e}): {}",
                clip(&reply)
            ))
        })
    }

    /// A dispatch or keyword request: Hyprland answers "ok", or what's wrong.
    fn hypr_cmd(&self, cmd: &str) -> Result<()> {
        // Requests start with flags ("j") up to a '/'; without the leading
        // '/' Hyprland would take everything before a '/' in the command
        // (a path) as flags.
        let reply = self.hypr(&format!("/{cmd}"))?;
        match reply.trim() {
            "" | "ok" => Ok(()),
            why => Err(Error::ActionFailed(format!(
                "Hyprland refused `{cmd}`: {}",
                clip(why)
            ))),
        }
    }

    fn dispatch(&self, what: &str) -> Result<()> {
        self.hypr_cmd(&format!("dispatch {what}"))
    }

    fn hypr_apply(&self, w: Win, op: &WindowOp) -> Result<()> {
        let id = w.top.id.clone();
        let addr = format!("address:{}", hypr_addr(&id)?);
        match *op {
            WindowOp::Focus => self.focus(&w.top),
            WindowOp::SetBounds(r) => {
                if w.minimized {
                    self.hypr_unminimize(&w)?;
                }
                let w = self.hypr_set_fs(&id, Fs::Normal)?;
                if !w.top.floating {
                    // `setfloating` takes a window since 0.33; before, only
                    // `togglefloating` did (and it's tiled now).
                    if self.dispatch(&format!("setfloating {addr}")).is_err() {
                        self.dispatch(&format!("togglefloating {addr}"))?;
                    }
                    self.expect(&id, |w| w.top.floating, "the window didn't become floating")?;
                }
                let [x, y, width, height] = px(r);
                self.dispatch(&format!("resizewindowpixel exact {width} {height},{addr}"))?;
                self.dispatch(&format!("movewindowpixel exact {x} {y},{addr}"))?;
                self.settle(&id, |w| near(w.top.rect, r))?;
                Ok(())
            }
            WindowOp::Maximize => {
                if w.minimized {
                    self.hypr_unminimize(&w)?;
                }
                self.hypr_set_fs(&id, Fs::Maximized).map(drop)
            }
            WindowOp::Fullscreen(on) => {
                if on && w.minimized {
                    self.hypr_unminimize(&w)?;
                }
                let want = if on { Fs::Full } else { Fs::Normal };
                if !on && w.fs() == Fs::Maximized {
                    return Ok(());
                }
                self.hypr_set_fs(&id, want).map(drop)
            }
            WindowOp::Minimize => {
                if w.minimized {
                    return Ok(());
                }
                self.dispatch(&format!("movetoworkspacesilent {HYPR_MINIMIZED},{addr}"))?;
                self.expect(
                    &id,
                    |w| w.minimized,
                    "the window didn't move to special:minimized",
                )
                .map(drop)
            }
            WindowOp::Restore => {
                if w.minimized {
                    self.hypr_unminimize(&w)?;
                }
                let w = self.hypr_set_fs(&id, Fs::Normal)?;
                self.focus(&w.top)
            }
            WindowOp::Close => self.dispatch(&format!("closewindow {addr}")),
            WindowOp::ToDesktop(n) => {
                let num = i64::from(n) + 1;
                self.dispatch(&format!("movetoworkspacesilent {num},{addr}"))?;
                self.expect(
                    &id,
                    |w| w.ws_num == Some(num),
                    &format!("the window didn't move to workspace {num}"),
                )
                .map(drop)
            }
        }
    }

    /// Bring a window back from special:minimized to the workspace on the
    /// focused monitor.
    fn hypr_unminimize(&self, w: &Win) -> Result<Win> {
        let monitors = self.hypr_json("monitors")?;
        let list = monitors.as_array().map(Vec::as_slice).unwrap_or_default();
        let m = list
            .iter()
            .find(|m| m["focused"].as_bool() == Some(true))
            .or(list.first())
            .ok_or_else(|| Error::ActionFailed("Hyprland reports no monitor".into()))?;
        let ws = &m["activeWorkspace"];
        let target = match ws["id"].as_i64() {
            Some(id) if id > 0 => id.to_string(),
            _ => format!("name:{}", ws["name"].as_str().unwrap_or("1")),
        };
        self.dispatch(&format!(
            "movetoworkspacesilent {target},address:{}",
            hypr_addr(&w.top.id)?
        ))?;
        self.expect(
            &w.top.id,
            |w| !w.minimized,
            "the window didn't come back from special:minimized",
        )
    }

    /// Put a window in a full-screen state. Hyprland's `fullscreen`
    /// dispatcher acts on the focused window and toggles (0: full screen,
    /// 1: maximized); before 0.42 it toggles full screen on or off whatever
    /// the mode, so going from one mode to the other can take two steps.
    fn hypr_set_fs(&self, id: &str, want: Fs) -> Result<Win> {
        let mut w = self.window(id)?;
        for _ in 0..3 {
            let now = w.fs();
            if now == want {
                return Ok(w);
            }
            // Exactly this window: a toggle on another one would be wrong.
            self.focus_window(&w.top, false)?;
            let mode = match (want, now) {
                (Fs::Maximized, _) | (Fs::Normal, Fs::Maximized) => 1,
                _ => 0,
            };
            self.dispatch(&format!("fullscreen {mode}"))?;
            w = self.settle(id, |w| w.fs() != now)?.ok_or_else(gone)?;
        }
        if w.fs() == want {
            return Ok(w);
        }
        let what = match want {
            Fs::Normal => "take the window out of full screen",
            Fs::Maximized => "maximize the window",
            Fs::Full => "make the window full screen",
        };
        Err(Error::ActionFailed(format!("Hyprland did not {what}")))
    }

    // ---- sway ----

    /// One i3-ipc message to sway, and its reply.
    fn sway(&self, kind: u32, payload: &str) -> Result<Value> {
        let len = u32::try_from(payload.len())
            .map_err(|_| Error::InvalidArgs("sway command too long".into()))?;
        let mut msg = Vec::with_capacity(14 + payload.len());
        msg.extend_from_slice(I3_MAGIC);
        msg.extend_from_slice(&len.to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        msg.extend_from_slice(payload.as_bytes());
        let mut s = self.connect()?;
        s.write_all(&msg).map_err(|e| self.io_error(e))?;
        loop {
            let mut head = [0u8; 14];
            s.read_exact(&mut head).map_err(|e| self.io_error(e))?;
            if &head[..6] != I3_MAGIC {
                return Err(Error::Platform(
                    "sway's IPC socket answered with something that isn't i3-ipc".into(),
                ));
            }
            let len = u32::from_ne_bytes([head[6], head[7], head[8], head[9]]);
            let reply_type = u32::from_ne_bytes([head[10], head[11], head[12], head[13]]);
            if u64::from(len) > MAX_REPLY {
                return Err(Error::Platform(format!(
                    "sway's IPC reply claims {len} bytes"
                )));
            }
            let mut body = vec![0; len as usize];
            s.read_exact(&mut body).map_err(|e| self.io_error(e))?;
            // Events (high bit set) only go to subscribers; skip any anyway.
            if reply_type & 0x8000_0000 != 0 {
                continue;
            }
            return serde_json::from_slice(&body)
                .map_err(|e| Error::Platform(format!("sway's IPC reply isn't JSON: {e}")));
        }
    }

    /// Run sway commands: fails with sway's reason if any of them fails.
    fn sway_run(&self, cmd: &str) -> Result<()> {
        let reply = self.sway(RUN_COMMAND, cmd)?;
        for r in reply.as_array().into_iter().flatten() {
            if r["success"].as_bool() != Some(true) {
                let why = r["error"].as_str().unwrap_or("no reason given");
                return Err(Error::ActionFailed(format!("sway refused `{cmd}`: {why}")));
            }
        }
        Ok(())
    }

    fn sway_apply(&self, w: Win, op: &WindowOp) -> Result<()> {
        let id = w.top.id.clone();
        let con = sway_con(&id)?;
        match *op {
            WindowOp::Focus => self.focus(&w.top),
            WindowOp::SetBounds(r) => {
                forget(&self.socket, &id, Put::Maximized);
                let w = self.sway_unminimize(w)?;
                self.sway_set_bounds(&w, r)
            }
            WindowOp::Maximize => {
                let w = self.sway_unminimize(w)?;
                if w.maximized {
                    return Ok(());
                }
                let area = self.area_for(&w)?;
                remember(
                    &self.socket,
                    &id,
                    Put::Maximized,
                    w.top.floating,
                    w.top.rect,
                );
                self.sway_set_bounds(&w, area)
            }
            WindowOp::Minimize => {
                if w.minimized {
                    return Ok(());
                }
                remember(
                    &self.socket,
                    &id,
                    Put::Minimized,
                    w.top.floating,
                    w.top.rect,
                );
                self.sway_run(&format!("{con} move scratchpad"))?;
                self.expect(
                    &id,
                    |w| w.minimized,
                    "the window didn't move to the scratchpad",
                )
                .map(drop)
            }
            WindowOp::Restore => {
                let w = self.sway_unminimize(w)?;
                if w.top.fullscreen {
                    self.sway_run(&format!("{con} fullscreen disable"))?;
                }
                if let Some(saved) = forget(&self.socket, &id, Put::Maximized)
                    && w.top.floating
                {
                    if saved.floating {
                        self.sway_set_bounds(&self.window(&id)?, saved.rect)?;
                    } else {
                        self.sway_run(&format!("{con} floating disable"))?;
                    }
                }
                self.focus(&w.top)
            }
            WindowOp::Fullscreen(on) => {
                let w = if on { self.sway_unminimize(w)? } else { w };
                if w.top.fullscreen == on {
                    return Ok(());
                }
                let state = if on { "enable" } else { "disable" };
                self.sway_run(&format!("{con} fullscreen {state}"))?;
                self.expect(
                    &id,
                    |w| w.top.fullscreen == on,
                    &format!("full screen didn't turn {}", if on { "on" } else { "off" }),
                )
                .map(drop)
            }
            WindowOp::Close => self.sway_run(&format!("{con} kill")),
            WindowOp::ToDesktop(n) => {
                let num = i64::from(n) + 1;
                self.sway_run(&format!("{con} move container to workspace number {num}"))?;
                self.expect(
                    &id,
                    |w| w.ws_num == Some(num),
                    &format!("the window didn't move to workspace {num}"),
                )
                .map(drop)
            }
        }
    }

    /// Bring a window back from the scratchpad onto the workspace on screen,
    /// as it was before [`WindowOp::Minimize`] put it there.
    fn sway_unminimize(&self, w: Win) -> Result<Win> {
        if !w.minimized {
            return Ok(w);
        }
        let con = sway_con(&w.top.id)?;
        let saved = forget(&self.socket, &w.top.id, Put::Minimized);
        self.sway_run(&format!("{con} scratchpad show"))?;
        if let Some(s) = saved
            && !s.floating
        {
            // Tiling it again also takes it out of the scratchpad.
            self.sway_run(&format!("{con} floating disable"))?;
        }
        let back = self.expect(
            &w.top.id,
            |w| !w.minimized,
            "the window didn't come back from the scratchpad",
        )?;
        match saved {
            Some(s) if s.floating => {
                self.sway_set_bounds(&back, s.rect)?;
                self.window(&w.top.id)
            }
            _ => Ok(back),
        }
    }

    /// Place a window's content box at `r`: `resize set` and `move
    /// position` are about the container, borders and title bar included.
    fn sway_set_bounds(&self, w: &Win, r: Rect) -> Result<()> {
        let id = &w.top.id;
        let con = sway_con(id)?;
        let mut w = w.clone();
        if w.top.fullscreen {
            self.sway_run(&format!("{con} fullscreen disable"))?;
        }
        if !w.top.floating {
            self.sway_run(&format!("{con} floating enable"))?;
            // Floating windows can have other borders: read them again.
            w = self.expect(id, |w| w.top.floating, "the window didn't become floating")?;
        }
        let [left, top, right, bottom] = w.frame;
        let [x, y, width, height] = px(Rect::new(
            r.x - left,
            r.y - top,
            r.width + left + right,
            r.height + top + bottom,
        ));
        self.sway_run(&format!(
            "{con} resize set width {width} px height {height} px, move absolute position {x} {y}"
        ))?;
        self.settle(id, |w| near(w.top.rect, r))?;
        Ok(())
    }

    /// The work area of the output a window is on (else the focused one).
    fn area_for(&self, w: &Win) -> Result<Rect> {
        let outputs = self.outputs()?;
        let center = w.top.rect.center();
        outputs
            .iter()
            .find(|o| !w.minimized && o.rect.contains(center))
            .or_else(|| outputs.iter().find(|o| o.focused))
            .or(outputs.first())
            .map(|o| o.work_area)
            .ok_or_else(|| Error::ActionFailed("sway reports no output".into()))
    }

    /// The key bindings in sway's config (the main file, as sway loaded it,
    /// and what it includes) that are active in the default mode.
    fn sway_user_binds(&self) -> Result<Vec<ConfBind>> {
        let config = self.sway(GET_CONFIG, "")?;
        let text = config["config"].as_str().unwrap_or_default();
        let dir = self
            .sway(GET_VERSION, "")
            .ok()
            .and_then(|v| v["loaded_config_file_name"].as_str().map(PathBuf::from))
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(default_sway_dir);
        let mut conf = SwayConf::default();
        conf.read(text, &dir, 0);
        Ok(conf.binds)
    }
}

/// Which compositor's socket the environment points at.
fn find_socket(
    hyprland_signature: Option<&std::ffi::OsStr>,
    runtime_dir: Option<&std::ffi::OsStr>,
    swaysock: Option<&std::ffi::OsStr>,
    exists: impl Fn(&Path) -> bool,
) -> Option<(Kind, PathBuf)> {
    if let Some(sig) = hyprland_signature.filter(|s| !s.is_empty()) {
        // Since Hyprland 0.40 in the runtime dir; before, in /tmp.
        let mut candidates = Vec::new();
        if let Some(dir) = runtime_dir.filter(|d| !d.is_empty()) {
            candidates.push(Path::new(dir).join("hypr").join(sig).join(".socket.sock"));
        }
        candidates.push(Path::new("/tmp/hypr").join(sig).join(".socket.sock"));
        match candidates.into_iter().find(|p| exists(p)) {
            Some(p) => return Some((Kind::Hyprland, p)),
            None => {
                log::warn!("HYPRLAND_INSTANCE_SIGNATURE is set but Hyprland's socket isn't there")
            }
        }
    }
    let sock = PathBuf::from(swaysock.filter(|s| !s.is_empty())?);
    exists(&sock).then_some((Kind::Sway, sock))
}

/// Connect to a Unix socket, giving up after `timeout`. A plain blocking
/// connect waits forever once the listener's queue is full, which is what a
/// hung compositor looks like after a few requests.
fn connect(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let addr = sockaddr(path)?;
    // SAFETY: socket(2) with constant arguments; the result is checked.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just created and nothing else owns it.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: `addr` is an initialized sockaddr_un and `len` its size.
        let r = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                std::ptr::from_ref(&addr).cast::<libc::sockaddr>(),
                len,
            )
        };
        if r == 0 {
            break;
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => {}
            // The listener's queue is full: it isn't accepting.
            Some(libc::EAGAIN) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Some(libc::EAGAIN) => return Err(io::Error::from(io::ErrorKind::TimedOut)),
            _ => return Err(e),
        }
    }
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

fn sockaddr(path: &Path) -> io::Result<libc::sockaddr_un> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is plain data; all zeroes is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path empty or too long",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    Ok(addr)
}

fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

fn check_bounds(r: &Rect) -> Result<()> {
    let finite = [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite());
    if !finite || r.width < 1.0 || r.height < 1.0 {
        return Err(Error::InvalidArgs(format!(
            "bounds {r:?}: need finite numbers and a width and height of at least 1"
        )));
    }
    Ok(())
}

/// Whole pixels: x, y, width, height.
fn px(r: Rect) -> [i64; 4] {
    [
        r.x.round() as i64,
        r.y.round() as i64,
        r.width.round().max(1.0) as i64,
        r.height.round().max(1.0) as i64,
    ]
}

/// Within a pixel and a half (rounding) of each other.
fn near(a: Rect, b: Rect) -> bool {
    [
        (a.x, b.x),
        (a.y, b.y),
        (a.width, b.width),
        (a.height, b.height),
    ]
    .iter()
    .all(|(p, q)| (p - q).abs() < 1.5)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// `{x, y, width, height}`.
fn rect_of(v: &Value) -> Rect {
    Rect::new(
        num(&v["x"]),
        num(&v["y"]),
        num(&v["width"]),
        num(&v["height"]),
    )
}

/// A Hyprland window address, checked before it goes into a request.
fn hypr_addr(id: &str) -> Result<&str> {
    let hex = id.strip_prefix("0x").unwrap_or_default();
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidArgs(format!(
            "`{id}` is not a Hyprland window address"
        )));
    }
    Ok(id)
}

/// sway criteria for one container.
fn sway_con(id: &str) -> Result<String> {
    let n: u64 = id
        .parse()
        .map_err(|_| Error::InvalidArgs(format!("`{id}` is not a sway container id")))?;
    Ok(format!("[con_id={n}]"))
}

// ---- Hyprland's replies ----

/// Windows from `j/clients`, with what `j/monitors` says is on screen and
/// `j/activewindow` (`{}` when no window has the focus).
fn hypr_windows(clients: &Value, monitors: &Value, active: &Value) -> Vec<Win> {
    let active = active["address"].as_str();
    // A monitor shows its active workspace, and over it its special
    // workspace when one is open (id 0 when none).
    let shown: HashSet<i64> = monitors
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["disabled"].as_bool() != Some(true))
        .flat_map(|m| {
            [
                m["activeWorkspace"]["id"].as_i64(),
                m["specialWorkspace"]["id"].as_i64(),
            ]
        })
        .flatten()
        .filter(|id| *id != 0)
        .collect();
    clients
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let address = c["address"].as_str()?;
            let workspace = c["workspace"]["name"].as_str().unwrap_or_default();
            let ws_id = c["workspace"]["id"].as_i64();
            let (fullscreen, maximized) = hypr_fullscreen(c);
            let rect = Rect::new(
                num(&c["at"][0]),
                num(&c["at"][1]),
                num(&c["size"][0]),
                num(&c["size"][1]),
            );
            let visible = c["mapped"].as_bool() != Some(false)
                && c["hidden"].as_bool() != Some(true)
                && ws_id.is_some_and(|id| shown.contains(&id))
                && !rect.is_empty();
            Some(Win {
                top: Toplevel {
                    id: address.to_string(),
                    pid: c["pid"]
                        .as_i64()
                        .filter(|p| *p > 0)
                        .and_then(|p| u32::try_from(p).ok()),
                    title: c["title"].as_str().unwrap_or_default().to_string(),
                    app_id: c["class"].as_str().unwrap_or_default().to_string(),
                    rect,
                    focused: active == Some(address),
                    visible,
                    fullscreen,
                    floating: c["floating"].as_bool() == Some(true),
                    xwayland: c["xwayland"].as_bool() == Some(true),
                },
                ws_num: ws_id,
                maximized,
                minimized: workspace == HYPR_MINIMIZED,
                frame: [0.0; 4],
            })
        })
        .collect()
}

/// (full screen, maximized). Before Hyprland 0.42 `fullscreen` is a bool
/// and `fullscreenMode` says which (0 full screen, 1 maximized); since, it
/// is a bit mask (1 maximized, 2 full screen).
fn hypr_fullscreen(c: &Value) -> (bool, bool) {
    match &c["fullscreen"] {
        Value::Bool(on) => {
            let mode = c["fullscreenMode"].as_i64().unwrap_or(0);
            (*on && mode != 1, *on && mode == 1)
        }
        v => {
            let mask = v.as_i64().unwrap_or(0);
            let full = mask & 2 != 0;
            (full, !full && mask & 1 != 0)
        }
    }
}

/// Outputs from `j/monitors`: `x`/`y` are logical, `width`/`height` are the
/// mode in pixels (before the transform), `reserved` is [left, top, right,
/// bottom] in logical pixels.
fn hypr_outputs(monitors: &Value) -> Vec<Output> {
    monitors
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["disabled"].as_bool() != Some(true))
        .map(|m| {
            let (w, h) = (num(&m["width"]), num(&m["height"]));
            let scale = hypr_scale(num(&m["scale"]), w, h);
            // Odd wl_output transforms turn it by 90 or 270 degrees.
            let (w, h) = if m["transform"].as_i64().unwrap_or(0) % 2 == 1 {
                (h, w)
            } else {
                (w, h)
            };
            let rect = Rect::new(
                num(&m["x"]),
                num(&m["y"]),
                (w / scale).round(),
                (h / scale).round(),
            );
            let r = |i: usize| num(&m["reserved"][i]).max(0.0);
            let work = Rect::new(
                rect.x + r(0),
                rect.y + r(1),
                rect.width - r(0) - r(2),
                rect.height - r(1) - r(3),
            );
            Output {
                name: m["name"].as_str().unwrap_or_default().to_string(),
                rect,
                work_area: if work.is_empty() { rect } else { work },
                scale,
                focused: m["focused"].as_bool() == Some(true),
            }
        })
        .collect()
}

/// Hyprland prints the scale with two decimals (1.33 for 4/3), but only
/// takes scales in steps of 1/120 (the fractional-scale protocol's unit)
/// that give a whole logical size: the one it meant.
fn hypr_scale(printed: f64, w: f64, h: f64) -> f64 {
    if printed.is_nan() || printed <= 0.0 {
        return 1.0;
    }
    let off = |s: f64| (w / s - (w / s).round()).abs() + (h / s - (h / s).round()).abs();
    let lo = ((printed - 0.006) * 120.0).floor() as i64;
    let hi = ((printed + 0.006) * 120.0).ceil() as i64;
    (lo..=hi)
        .map(|k| k as f64 / 120.0)
        .filter(|s| *s > 0.0 && (s - printed).abs() <= 0.005 + 1e-9)
        .min_by(|a, b| off(*a).total_cmp(&off(*b)))
        .filter(|s| off(*s) < 0.01)
        .unwrap_or(printed)
}

/// A binding Hyprland has on a chord.
#[derive(Debug, Clone, PartialEq)]
struct HyprBind {
    /// The key as bound ("Escape", "code:9"): what `unbind` needs, exactly.
    key: String,
    /// Has [`BIND_MARKER`].
    ours: bool,
}

/// The bindings in `j/binds` that fire on `chord` (in any submap: `unbind`
/// removes them all).
fn hypr_binds_on(binds: &Value, chord: &Chord) -> Vec<HyprBind> {
    binds
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["mouse"].as_bool() != Some(true))
        .filter(|b| b["modmask"].as_u64() == Some(u64::from(chord.mask)))
        .filter_map(|b| {
            let key = b["key"].as_str().unwrap_or_default();
            let code = b["keycode"]
                .as_u64()
                .filter(|c| *c != 0)
                .and_then(|c| u32::try_from(c).ok());
            let hit = if key.is_empty() {
                code.is_some() && code == chord.code
            } else {
                same_key(key, &chord.name)
            };
            hit.then(|| HyprBind {
                key: if key.is_empty() {
                    format!("code:{}", code.unwrap_or_default())
                } else {
                    key.to_string()
                },
                ours: b["arg"].as_str().unwrap_or_default().contains(BIND_MARKER),
            })
        })
        .collect()
}

/// The distinct keys of our leftover bindings.
fn stale_keys(found: &[HyprBind]) -> Vec<String> {
    let mut keys: Vec<String> = found.iter().map(|b| b.key.clone()).collect();
    keys.sort();
    keys.dedup();
    keys
}

// ---- sway's replies ----

#[derive(Debug, Clone, Default)]
struct SwayCtx {
    workspace: String,
    num: Option<i64>,
    /// The workspace its output shows.
    current: Option<String>,
}

/// The views (windows) in GET_TREE, on every output and in the scratchpad.
fn sway_windows(tree: &Value) -> Vec<Win> {
    let mut out = Vec::new();
    sway_walk(tree, &SwayCtx::default(), &mut out);
    out
}

fn sway_walk(node: &Value, ctx: &SwayCtx, out: &mut Vec<Win>) {
    let mut ctx = ctx.clone();
    match node["type"].as_str() {
        Some("output") => ctx.current = node["current_workspace"].as_str().map(str::to_string),
        Some("workspace") => {
            ctx.workspace = node["name"].as_str().unwrap_or_default().to_string();
            ctx.num = node["num"].as_i64().filter(|n| *n >= 0);
        }
        // A view has a process and a shell; split containers have neither.
        Some("con" | "floating_con")
            if node.get("pid").is_some() || node.get("shell").is_some() =>
        {
            out.push(sway_view(node, &ctx));
        }
        _ => {}
    }
    for key in ["nodes", "floating_nodes"] {
        for child in node[key].as_array().into_iter().flatten() {
            sway_walk(child, &ctx, out);
        }
    }
}

fn sway_view(n: &Value, ctx: &SwayCtx) -> Win {
    // `rect` is the container without its title bar (that's `deco_rect`);
    // `window_rect` the content, relative to `rect`.
    let rect = rect_of(&n["rect"]);
    let mut content = rect_of(&n["window_rect"]);
    if content.is_empty() {
        content = Rect::new(0.0, 0.0, rect.width, rect.height);
    }
    let title_bar = num(&n["deco_rect"]["height"]);
    let props = &n["window_properties"];
    let xwayland = match n["shell"].as_str() {
        Some(shell) => shell == "xwayland",
        None => !n["window"].is_null(),
    };
    let app_id = n["app_id"]
        .as_str()
        .filter(|a| !a.is_empty())
        .or(props["class"].as_str())
        .or(props["instance"].as_str())
        .unwrap_or_default();
    let minimized = ctx.workspace == SWAY_SCRATCH;
    let visible = match n["visible"].as_bool() {
        Some(v) => v,
        None => !minimized && ctx.current.as_deref() == Some(ctx.workspace.as_str()),
    };
    Win {
        top: Toplevel {
            id: n["id"].as_u64().unwrap_or_default().to_string(),
            pid: n["pid"]
                .as_i64()
                .filter(|p| *p > 0)
                .and_then(|p| u32::try_from(p).ok()),
            title: n["name"].as_str().unwrap_or_default().to_string(),
            app_id: app_id.to_string(),
            rect: Rect::new(
                rect.x + content.x,
                rect.y + content.y,
                content.width,
                content.height,
            ),
            focused: n["focused"].as_bool() == Some(true),
            visible,
            fullscreen: n["fullscreen_mode"].as_i64().unwrap_or(0) != 0,
            floating: n["type"].as_str() == Some("floating_con"),
            xwayland,
        },
        ws_num: ctx.num,
        maximized: false,
        minimized,
        frame: [
            content.x,
            content.y + title_bar,
            rect.width - content.x - content.width,
            rect.height - content.y - content.height,
        ],
    }
}

/// Outputs from GET_OUTPUTS (`rect` is logical already), with the work area
/// from the workspace each shows (GET_WORKSPACES: its `rect` leaves out
/// bars and panels).
fn sway_outputs(outputs: &Value, workspaces: &Value) -> Vec<Output> {
    outputs
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| {
            o["active"].as_bool() != Some(false) && o["non_desktop"].as_bool() != Some(true)
        })
        .map(|o| {
            let name = o["name"].as_str().unwrap_or_default();
            let rect = rect_of(&o["rect"]);
            let work_area = workspaces
                .as_array()
                .into_iter()
                .flatten()
                .find(|w| {
                    w["output"].as_str() == Some(name) && w["visible"].as_bool() == Some(true)
                })
                .map(|w| rect_of(&w["rect"]))
                .filter(|r| !r.is_empty())
                .unwrap_or(rect);
            Output {
                name: name.to_string(),
                rect,
                work_area,
                scale: o["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0),
                focused: o["focused"].as_bool() == Some(true),
            }
        })
        .collect()
}

// ---- What sway can't be asked: state we keep ----

/// Why a sway window's earlier state was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Put {
    Maximized,
    Minimized,
}

/// A sway window's state before we maximized or minimized it (sway has
/// neither state): what Restore goes back to.
#[derive(Debug, Clone, Copy)]
struct Saved {
    floating: bool,
    rect: Rect,
}

/// (socket, con_id, why, state). Process-wide, so it holds whatever
/// [`Compositor`] value did the change.
static SAVED: Mutex<Vec<(PathBuf, String, Put, Saved)>> = Mutex::new(Vec::new());

fn remember(socket: &Path, id: &str, put: Put, floating: bool, rect: Rect) {
    forget(socket, id, put);
    if let Ok(mut saved) = SAVED.lock() {
        // Windows we never see again shouldn't pile up.
        if saved.len() >= 256 {
            saved.remove(0);
        }
        saved.push((
            socket.to_path_buf(),
            id.to_string(),
            put,
            Saved { floating, rect },
        ));
    }
}

fn recall(socket: &Path, id: &str, put: Put) -> Option<Saved> {
    let saved = SAVED.lock().ok()?;
    saved
        .iter()
        .find(|(s, i, p, _)| s == socket && i == id && *p == put)
        .map(|e| e.3)
}

fn forget(socket: &Path, id: &str, put: Put) -> Option<Saved> {
    let mut saved = SAVED.lock().ok()?;
    let i = saved
        .iter()
        .position(|(s, i, p, _)| s == socket && i == id && *p == put)?;
    Some(saved.remove(i).3)
}

// ---- Keys ----

/// A key combination as both compositors name it.
#[derive(Debug, Clone, PartialEq)]
struct Chord {
    /// [`SHIFT`] | [`CTRL`] | [`ALT`] | [`LOGO`].
    mask: u32,
    /// The xkb keysym name: "Escape", "F1", "a", "comma".
    name: String,
    /// The keycode, for keys whose place doesn't depend on the layout.
    code: Option<u32>,
}

impl Chord {
    fn of(combo: &KeyCombo) -> Result<Self> {
        let m = combo.modifiers;
        let mask = [
            (m.shift, SHIFT),
            (m.ctrl, CTRL),
            (m.alt, ALT),
            (m.meta, LOGO),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .fold(0, |acc, (_, bit)| acc | bit);
        let name = keysym_name(combo.key).ok_or_else(|| {
            Error::InvalidArgs(format!("`{combo}` can't be bound: no key makes that"))
        })?;
        Ok(Self {
            mask,
            name,
            code: keycode(combo.key),
        })
    }

    /// Hyprland's modifier list: "CTRL ALT".
    fn hypr_mods(&self) -> String {
        [
            (CTRL, "CTRL"),
            (ALT, "ALT"),
            (SHIFT, "SHIFT"),
            (LOGO, "SUPER"),
        ]
        .iter()
        .filter(|(bit, _)| self.mask & bit != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(" ")
    }

    /// sway's combination: "Ctrl+Alt+Escape" (sway calls Super "Mod4").
    fn sway(&self) -> String {
        let mut parts: Vec<&str> = [
            (CTRL, "Ctrl"),
            (ALT, "Alt"),
            (SHIFT, "Shift"),
            (LOGO, "Mod4"),
        ]
        .iter()
        .filter(|(bit, _)| self.mask & bit != 0)
        .map(|(_, name)| *name)
        .collect();
        parts.push(&self.name);
        parts.join("+")
    }
}

/// The same keysym, as xkb looks names up (case-insensitively, with the
/// common aliases).
fn same_key(a: &str, b: &str) -> bool {
    fn canon(k: &str) -> String {
        let k = k.to_ascii_lowercase();
        match k.as_str() {
            "prior" => "page_up".into(),
            "next" => "page_down".into(),
            "kp_prior" => "kp_page_up".into(),
            "kp_next" => "kp_page_down".into(),
            _ => k,
        }
    }
    canon(a) == canon(b)
}

/// The xkb keysym name of a key, as both compositors' bindings name keys.
fn keysym_name(key: Key) -> Option<String> {
    let name = match key {
        Key::Named(n) => match n {
            NamedKey::Return => "Return",
            NamedKey::Tab => "Tab",
            NamedKey::Space => "space",
            NamedKey::Backspace => "BackSpace",
            NamedKey::Delete => "Delete",
            NamedKey::Escape => "Escape",
            NamedKey::Home => "Home",
            NamedKey::End => "End",
            NamedKey::PageUp => "Page_Up",
            NamedKey::PageDown => "Page_Down",
            NamedKey::Left => "Left",
            NamedKey::Right => "Right",
            NamedKey::Up => "Up",
            NamedKey::Down => "Down",
            NamedKey::Insert => "Insert",
            NamedKey::CapsLock => "Caps_Lock",
            NamedKey::Menu => "Menu",
            NamedKey::F(n) => return Some(format!("F{n}")),
            NamedKey::Numpad(p) => match p {
                Pad::Digit(d) => return Some(format!("KP_{d}")),
                Pad::Decimal => "KP_Decimal",
                Pad::Add => "KP_Add",
                Pad::Subtract => "KP_Subtract",
                Pad::Multiply => "KP_Multiply",
                Pad::Divide => "KP_Divide",
                Pad::Enter => "KP_Enter",
            },
        },
        Key::Char(c) => match c {
            c if c.is_ascii_alphanumeric() => return Some(c.to_ascii_lowercase().to_string()),
            '\t' => "Tab",
            '\n' | '\r' => "Return",
            '\u{8}' => "BackSpace",
            '\u{1b}' => "Escape",
            '\u{7f}' => "Delete",
            ' ' => "space",
            '!' => "exclam",
            '"' => "quotedbl",
            '#' => "numbersign",
            '$' => "dollar",
            '%' => "percent",
            '&' => "ampersand",
            '\'' => "apostrophe",
            '(' => "parenleft",
            ')' => "parenright",
            '*' => "asterisk",
            '+' => "plus",
            ',' => "comma",
            '-' => "minus",
            '.' => "period",
            '/' => "slash",
            ':' => "colon",
            ';' => "semicolon",
            '<' => "less",
            '=' => "equal",
            '>' => "greater",
            '?' => "question",
            '@' => "at",
            '[' => "bracketleft",
            '\\' => "backslash",
            ']' => "bracketright",
            '^' => "asciicircum",
            '_' => "underscore",
            '`' => "grave",
            '{' => "braceleft",
            '|' => "bar",
            '}' => "braceright",
            '~' => "asciitilde",
            c if (c as u32) < 0x20 || (0x7f..0xa0).contains(&(c as u32)) => return None,
            // xkb takes any character as "U" and its code point.
            c => return Some(format!("U{:04X}", c as u32)),
        },
    };
    Some(name.to_string())
}

/// The keycode (evdev + 8: what Hyprland's `code:N` and sway's `bindcode`
/// use) of keys that are in the same place on every layout.
fn keycode(key: Key) -> Option<u32> {
    let named = match key {
        Key::Named(n) => n,
        Key::Char('\u{1b}') => NamedKey::Escape,
        Key::Char(' ') => NamedKey::Space,
        _ => return None,
    };
    Some(match named {
        NamedKey::Escape => 9,
        NamedKey::Backspace => 22,
        NamedKey::Tab => 23,
        NamedKey::Return => 36,
        NamedKey::Space => 65,
        NamedKey::CapsLock => 66,
        NamedKey::F(n @ 1..=10) => 66 + u32::from(n),
        NamedKey::F(11) => 95,
        NamedKey::F(12) => 96,
        NamedKey::Numpad(Pad::Enter) => 104,
        NamedKey::Home => 110,
        NamedKey::Up => 111,
        NamedKey::PageUp => 112,
        NamedKey::Left => 113,
        NamedKey::Right => 114,
        NamedKey::End => 115,
        NamedKey::Down => 116,
        NamedKey::PageDown => 117,
        NamedKey::Insert => 118,
        NamedKey::Delete => 119,
        NamedKey::Menu => 135,
        _ => return None,
    })
}

/// Whether `s` has one of `chars` outside single or double quotes.
fn has_unquoted(s: &str, chars: &[char]) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for c in s.chars() {
        match (quote, c) {
            _ if escaped => escaped = false,
            (_, '\\') => escaped = true,
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, c) if chars.contains(&c) => return true,
            _ => {}
        }
    }
    false
}

// ---- sway's config: the user's key bindings ----

/// A key binding of sway's default mode, from its config.
#[derive(Debug, Clone, PartialEq)]
struct ConfBind {
    mask: u32,
    /// Keysym names (`bindsym`) or keycodes (`bindcode`); more than one is a chord of keys.
    keys: Vec<String>,
    code: bool,
    /// Has [`BIND_MARKER`].
    ours: bool,
}

impl ConfBind {
    fn is(&self, chord: &Chord) -> bool {
        if self.mask != chord.mask || self.keys.len() != 1 {
            return false;
        }
        if self.code {
            self.keys[0]
                .parse::<u32>()
                .ok()
                .is_some_and(|c| Some(c) == chord.code)
        } else {
            same_key(&self.keys[0], &chord.name)
        }
    }
}

/// sway's modifier names (`Super` and `Logo` are taken too, for safety).
fn sway_modifier(name: &str) -> Option<u32> {
    Some(match name.to_ascii_lowercase().as_str() {
        "shift" => SHIFT,
        "lock" => 1 << 1,
        "control" | "ctrl" => CTRL,
        "mod1" | "alt" => ALT,
        "mod2" => 1 << 4,
        "mod3" => 1 << 5,
        "mod4" | "super" | "logo" => LOGO,
        "mod5" => 1 << 7,
        _ => return None,
    })
}

/// Reads sway's config as sway does, as far as key bindings go: variables
/// (`set $mod Mod4`), `include` (with `~`, environment variables and
/// globs), blocks (`bindsym { … }`, `mode "resize" { … }`) and continued
/// lines.
#[derive(Debug, Default)]
struct SwayConf {
    /// `$name` → value, longest name first (as sway replaces them).
    vars: Vec<(String, String)>,
    binds: Vec<ConfBind>,
    included: HashSet<PathBuf>,
}

impl SwayConf {
    fn read(&mut self, text: &str, dir: &Path, depth: usize) {
        let mut blocks: Vec<String> = Vec::new();
        let mut last = String::new();
        for line in config_lines(text) {
            if line == "}" {
                blocks.pop();
                continue;
            }
            // A block can open on the line after its prefix.
            if line == "{" {
                blocks.push(std::mem::take(&mut last));
                continue;
            }
            if let Some(prefix) = line.strip_suffix('{') {
                blocks.push(prefix.trim().to_string());
                continue;
            }
            let full = if blocks.is_empty() {
                line.clone()
            } else {
                format!("{} {line}", blocks.join(" "))
            };
            self.command(&full, dir, depth);
            last = line;
        }
    }

    fn command(&mut self, line: &str, dir: &Path, depth: usize) {
        let (word, rest) = split_word(line);
        match word {
            "set" => {
                let (name, value) = split_word(rest);
                if name.is_empty() {
                    return;
                }
                let name = if name.starts_with('$') {
                    name.to_string()
                } else {
                    format!("${name}")
                };
                let value = self.substitute(value);
                self.vars.retain(|(n, _)| *n != name);
                self.vars.push((name, value));
                self.vars.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
            }
            "include" if depth < 8 => {
                for path in include_paths(&self.substitute(rest), dir) {
                    let key = path.canonicalize().unwrap_or_else(|_| path.clone());
                    if !self.included.insert(key) {
                        continue;
                    }
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        let sub_dir = path.parent().unwrap_or(dir).to_path_buf();
                        self.read(&text, &sub_dir, depth + 1);
                    }
                }
            }
            "bindsym" | "bindcode" => {
                let line = self.substitute(rest);
                let mut rest = line.as_str();
                // Flags: --no-repeat, --to-code, --input-device=…
                let combo = loop {
                    let (w, r) = split_word(rest);
                    rest = r;
                    if !w.starts_with("--") {
                        break w;
                    }
                };
                if combo.is_empty() {
                    return;
                }
                let mut mask = 0;
                let mut keys = Vec::new();
                for part in combo.split('+').filter(|p| !p.is_empty()) {
                    match sway_modifier(part) {
                        Some(bit) => mask |= bit,
                        None => keys.push(part.to_string()),
                    }
                }
                self.binds.push(ConfBind {
                    mask,
                    keys,
                    code: word == "bindcode",
                    ours: rest.contains(BIND_MARKER),
                });
            }
            // Bindings of other modes ("mode … bindsym") don't fire in the default one.
            _ => {}
        }
    }

    fn substitute(&self, s: &str) -> String {
        let mut s = s.to_string();
        for (name, value) in &self.vars {
            if s.contains(name.as_str()) {
                s = s.replace(name.as_str(), value);
            }
        }
        s
    }
}

/// The first word and the rest, trimmed.
fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.split_once(char::is_whitespace) {
        Some((w, rest)) => (w, rest.trim()),
        None => (s, ""),
    }
}

/// Config lines with continuations joined, comments and blank lines left out.
fn config_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if cur.is_empty() && (line.is_empty() || line.starts_with('#')) {
            continue;
        }
        if let Some(head) = line.strip_suffix('\\') {
            cur.push_str(head.trim_end());
            cur.push(' ');
            continue;
        }
        cur.push_str(line);
        out.push(std::mem::take(&mut cur).trim().to_string());
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The files an `include` names: `~` and `$VARIABLES` expanded, relative to
/// the including file's directory, a glob in the file name matched.
fn include_paths(pattern: &str, dir: &Path) -> Vec<PathBuf> {
    let expanded = expand_env(pattern.trim().trim_matches(|c| c == '"' || c == '\''));
    if expanded.is_empty() {
        return Vec::new();
    }
    let path = dir.join(expanded);
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| wildcard(&name, &n.to_string_lossy()))
        })
        .collect();
    found.sort();
    found
}

/// `~/` and `$NAME` / `${NAME}` from the environment, as sway's wordexp does.
fn expand_env(s: &str) -> String {
    let home = || std::env::var("HOME").unwrap_or_default();
    let s = match s.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", home()),
        _ => s.to_string(),
    };
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = chars.next_if_eq(&'{').is_some();
        let mut name = String::new();
        while let Some(c) = chars.next_if(|c| c.is_ascii_alphanumeric() || *c == '_') {
            name.push(c);
        }
        if braced {
            chars.next_if_eq(&'}');
        }
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&std::env::var(&name).unwrap_or_default());
        }
    }
    out
}

/// Shell glob with `*` and `?`; hidden files only for a pattern that starts with a dot.
fn wildcard(pattern: &str, name: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some((i, j));
            i += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

/// Where sway looks for its config when it doesn't say.
fn default_sway_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join("sway")
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::keys::parse_combo;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "cu-ipc-{}-{tag}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Answer every connection to `path` with `handle`, one at a time, on a thread.
    fn serve(path: &Path, handle: impl Fn(UnixStream) + Send + 'static) {
        let listener = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                match conn {
                    Ok(c) => handle(c),
                    Err(_) => return,
                }
            }
        });
    }

    fn combo(s: &str) -> KeyCombo {
        parse_combo(s).unwrap()
    }

    fn top(c: &Compositor, id: &str) -> Toplevel {
        c.toplevels()
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap_or_else(|| panic!("no window {id}"))
    }

    fn win(c: &Compositor, id: &str) -> Win {
        c.window(id).unwrap()
    }

    // ---- A Hyprland that keeps windows, monitors and bindings, and
    // answers as Hyprland does ----

    #[derive(Debug, Clone)]
    struct HWin {
        addr: &'static str,
        class: &'static str,
        title: &'static str,
        pid: i64,
        xwayland: bool,
        ws: (i64, String),
        at: [i64; 2],
        size: [i64; 2],
        floating: bool,
        /// 0.42+'s mask: 1 maximized, 2 full screen.
        fs: u8,
        hidden: bool,
    }

    #[derive(Debug, Clone)]
    struct HMon {
        name: &'static str,
        pixels: [i64; 2],
        at: [i64; 2],
        scale: f64,
        transform: i64,
        reserved: [i64; 4],
        ws: (i64, String),
        special: (i64, String),
        focused: bool,
    }

    #[derive(Debug, Clone)]
    struct HBind {
        mask: u64,
        key: String,
        keycode: u64,
        mouse: bool,
        dispatcher: String,
        arg: String,
    }

    fn hbind(mask: u64, key: &str, keycode: u64, arg: &str) -> HBind {
        HBind {
            mask,
            key: key.into(),
            keycode,
            mouse: false,
            dispatcher: "exec".into(),
            arg: arg.into(),
        }
    }

    #[derive(Debug, Default)]
    struct FakeHypr {
        wins: Vec<HWin>,
        mons: Vec<HMon>,
        active: Option<String>,
        binds: Vec<HBind>,
        log: Vec<String>,
        /// Answer like Hyprland before 0.42 (`fullscreen` a bool).
        old: bool,
    }

    impl FakeHypr {
        fn client(&self, w: &HWin, i: usize) -> Value {
            let mut c = json!({
                "address": w.addr, "mapped": true, "hidden": w.hidden,
                "at": w.at, "size": w.size,
                "workspace": {"id": w.ws.0, "name": w.ws.1},
                "floating": w.floating, "pseudo": false, "monitor": 0,
                "class": w.class, "title": w.title,
                "initialClass": w.class, "initialTitle": w.title,
                "pid": w.pid, "xwayland": w.xwayland, "pinned": false,
                "grouped": [], "tags": [], "swallowing": "0x0",
                "focusHistoryID": i, "inhibitingIdle": false
            });
            if self.old {
                c["fullscreen"] = json!(w.fs != 0);
                c["fullscreenMode"] = json!(u8::from(w.fs == 1));
                c["fakeFullscreen"] = json!(false);
            } else {
                c["fullscreen"] = json!(w.fs);
                c["fullscreenClient"] = json!(w.fs);
            }
            c
        }

        fn monitor(m: &HMon, id: usize) -> Value {
            json!({
                "id": id, "name": m.name, "description": "", "make": "", "model": "",
                "serial": "", "width": m.pixels[0], "height": m.pixels[1],
                "refreshRate": 60.0, "x": m.at[0], "y": m.at[1],
                "activeWorkspace": {"id": m.ws.0, "name": m.ws.1},
                "specialWorkspace": {"id": m.special.0, "name": m.special.1},
                "reserved": m.reserved, "scale": m.scale, "transform": m.transform,
                "focused": m.focused, "dpmsStatus": true, "vrr": false,
                "activelyTearing": false, "disabled": false
            })
        }

        fn bind(b: &HBind) -> Value {
            json!({
                "locked": false, "mouse": b.mouse, "release": false, "repeat": false,
                "longPress": false, "non_consuming": false, "has_description": false,
                "modmask": b.mask, "submap": "", "key": b.key, "keycode": b.keycode,
                "catch_all": false, "description": "", "dispatcher": b.dispatcher,
                "arg": b.arg
            })
        }

        fn find(&self, selector: &str) -> Option<usize> {
            self.wins
                .iter()
                .position(|w| selector == format!("address:{}", w.addr))
        }

        fn reply(&mut self, request: &str) -> String {
            self.log.push(request.to_string());
            // Hyprland takes everything up to the first '/' as flags.
            let (flags, cmd) = request.split_once('/').unwrap_or(("", request));
            assert!(
                flags.is_empty() || flags == "j",
                "{request:?} would be read with flags {flags:?}"
            );
            let pretty = |v: Value| serde_json::to_string_pretty(&v).unwrap();
            match cmd {
                "clients" => pretty(Value::Array(
                    self.wins
                        .iter()
                        .enumerate()
                        .map(|(i, w)| self.client(w, i))
                        .collect(),
                )),
                "monitors" => pretty(Value::Array(
                    self.mons
                        .iter()
                        .enumerate()
                        .map(|(i, m)| Self::monitor(m, i))
                        .collect(),
                )),
                "activewindow" => {
                    let active = self.active.as_deref();
                    match self.wins.iter().find(|w| Some(w.addr) == active) {
                        Some(w) => pretty(self.client(w, 0)),
                        None => "{}".into(),
                    }
                }
                "binds" => pretty(Value::Array(self.binds.iter().map(Self::bind).collect())),
                _ => {
                    if let Some(d) = cmd.strip_prefix("dispatch ") {
                        self.dispatch(d)
                    } else if let Some(k) = cmd.strip_prefix("keyword ") {
                        self.keyword(k)
                    } else {
                        "unknown request".into()
                    }
                }
            }
        }

        fn dispatch(&mut self, d: &str) -> String {
            let (name, arg) = d.split_once(' ').unwrap_or((d, ""));
            let (params, selector) = arg.rsplit_once(',').unwrap_or(("", arg));
            let missing = || "No such window found".to_string();
            match name {
                "focuswindow" => {
                    let Some(i) = self.find(arg) else {
                        return missing();
                    };
                    let ws = self.wins[i].ws.clone();
                    self.active = Some(self.wins[i].addr.into());
                    let m = self.mons.iter_mut().find(|m| m.focused).unwrap();
                    if ws.0 < 0 {
                        m.special = ws;
                    } else {
                        m.ws = ws;
                    }
                }
                "setfloating" | "togglefloating" => {
                    let Some(i) = self.find(arg) else {
                        return missing();
                    };
                    let w = &mut self.wins[i];
                    w.floating = name == "setfloating" || !w.floating;
                }
                "resizewindowpixel" | "movewindowpixel" => {
                    let Some(i) = self.find(selector) else {
                        return missing();
                    };
                    if self.wins[i].fs != 0 {
                        return "Window is fullscreen".into();
                    }
                    let v: Vec<i64> = params
                        .strip_prefix("exact ")
                        .expect("exact sizes")
                        .split(' ')
                        .map(|n| n.parse().unwrap())
                        .collect();
                    if name == "movewindowpixel" {
                        self.wins[i].at = [v[0], v[1]];
                    } else {
                        self.wins[i].size = [v[0], v[1]];
                    }
                }
                "fullscreen" => {
                    let active = self.active.clone().unwrap_or_default();
                    let Some(i) = self.find(&format!("address:{active}")) else {
                        return "Window not found".into();
                    };
                    let mode = if arg == "1" { 1 } else { 2 };
                    let old = self.old;
                    let w = &mut self.wins[i];
                    w.fs = match (old, w.fs) {
                        (true, 0) => mode,
                        (true, _) => 0,
                        (false, fs) if fs == mode => 0,
                        (false, _) => mode,
                    };
                }
                "movetoworkspacesilent" => {
                    let Some(i) = self.find(selector) else {
                        return missing();
                    };
                    let ws = if params.starts_with("special:") {
                        (-99, params.to_string())
                    } else if let Some(name) = params.strip_prefix("name:") {
                        (77, name.to_string())
                    } else {
                        (params.parse().unwrap(), params.to_string())
                    };
                    let hidden = ws.0 < 0;
                    self.wins[i].ws = ws;
                    if hidden && self.active.as_deref() == Some(self.wins[i].addr) {
                        self.active = None;
                    }
                }
                "closewindow" => {
                    let Some(i) = self.find(arg) else {
                        return missing();
                    };
                    self.wins.remove(i);
                }
                _ => return "Invalid dispatcher".into(),
            }
            "ok".into()
        }

        fn keyword(&mut self, k: &str) -> String {
            // hyprlang: '#' starts a comment, "##" is a '#'.
            let mut line = String::new();
            let mut chars = k.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '#' && chars.next_if_eq(&'#').is_none() {
                    break;
                }
                line.push(c);
            }
            let (name, value) = line.trim().split_once(' ').unwrap();
            let mask = |mods: &str| -> u64 {
                let m = mods.to_uppercase();
                [("SHIFT", 1), ("CTRL", 4), ("ALT", 8), ("SUPER", 64)]
                    .iter()
                    .filter(|(n, _)| m.contains(n))
                    .map(|(_, bit)| bit)
                    .sum()
            };
            match name {
                "bind" => {
                    let p: Vec<&str> = value.splitn(4, ',').map(str::trim).collect();
                    if p.len() < 4 {
                        return "bind: too few arguments".into();
                    }
                    self.binds.push(HBind {
                        dispatcher: p[2].into(),
                        ..hbind(mask(p[0]), p[1], 0, p[3])
                    });
                }
                "unbind" => {
                    let p: Vec<&str> = value.splitn(2, ',').map(str::trim).collect();
                    let m = mask(p[0]);
                    let (code, key) = match p[1].strip_prefix("code:") {
                        Some(c) => (c.parse().unwrap(), ""),
                        None => (0, p[1]),
                    };
                    self.binds
                        .retain(|b| !(b.mask == m && b.key == key && b.keycode == code));
                }
                _ => return format!("invalid keyword {name}"),
            }
            "ok".into()
        }
    }

    fn hypr_fixture(old: bool) -> FakeHypr {
        let win = |addr, class, ws: (i64, &str), at, size| HWin {
            addr,
            class,
            title: class,
            pid: 0,
            xwayland: false,
            ws: (ws.0, ws.1.to_string()),
            at,
            size,
            floating: false,
            fs: 0,
            hidden: false,
        };
        let mon = |name, pixels, at, scale, transform, reserved, ws: (i64, &str)| HMon {
            name,
            pixels,
            at,
            scale,
            transform,
            reserved,
            ws: (ws.0, ws.1.to_string()),
            special: (0, String::new()),
            focused: false,
        };
        FakeHypr {
            mons: vec![
                // 4/3, printed as 1.33.
                HMon {
                    focused: true,
                    ..mon(
                        "DP-1",
                        [2560, 1440],
                        [0, 0],
                        1.33,
                        0,
                        [0, 30, 0, 0],
                        (1, "1"),
                    )
                },
                HMon {
                    special: (-98, "special:scratch".into()),
                    ..mon(
                        "HDMI-A-1",
                        [1920, 1080],
                        [1920, 0],
                        1.5,
                        0,
                        [0; 4],
                        (2, "2"),
                    )
                },
                // Turned 90 degrees, a panel on the right.
                mon(
                    "DSI-1",
                    [1920, 1080],
                    [3200, 0],
                    1.0,
                    1,
                    [0, 0, 40, 0],
                    (3, "3"),
                ),
            ],
            wins: vec![
                HWin {
                    pid: 1234,
                    ..win("0x55d0c1e2a9b0", "kitty", (1, "1"), [10, 40], [940, 1030])
                },
                HWin {
                    pid: 2000,
                    ..win(
                        "0x55d0c1f00010",
                        "firefox",
                        (4, "4"),
                        [10, 40],
                        [1900, 1030],
                    )
                },
                HWin {
                    pid: 3000,
                    xwayland: true,
                    floating: true,
                    title: "Steam",
                    ..win("0x55d0c1f00020", "steam", (2, "2"), [2000, 100], [800, 600])
                },
                HWin {
                    pid: 4000,
                    ..win(
                        "0x55d0c1f00030",
                        "org.gnome.Calculator",
                        (-99, HYPR_MINIMIZED),
                        [600, 300],
                        [400, 500],
                    )
                },
                HWin {
                    pid: 5000,
                    floating: true,
                    ..win(
                        "0x55d0c1f00040",
                        "pavucontrol",
                        (-98, "special:scratch"),
                        [2100, 150],
                        [700, 500],
                    )
                },
                // A group member behind the active one.
                HWin {
                    pid: 1234,
                    hidden: true,
                    ..win("0x55d0c1f00050", "kitty", (1, "1"), [10, 40], [940, 1030])
                },
                HWin {
                    pid: 6000,
                    fs: 2,
                    ..win("0x55d0c1f00060", "mpv", (3, "3"), [3200, 0], [1080, 1920])
                },
                HWin {
                    pid: 7000,
                    fs: 1,
                    ..win("0x55d0c1f00070", "code", (3, "3"), [3200, 0], [1040, 1920])
                },
            ],
            active: Some("0x55d0c1e2a9b0".into()),
            old,
            ..Default::default()
        }
    }

    fn fake_hypr(state: FakeHypr) -> (Compositor, Arc<Mutex<FakeHypr>>, TempDir) {
        let dir = TempDir::new("hypr");
        let path = dir.0.join(".socket.sock");
        let state = Arc::new(Mutex::new(state));
        let shared = Arc::clone(&state);
        serve(&path, move |mut conn| {
            let mut request = String::new();
            // The client ends its side after the request.
            conn.read_to_string(&mut request).unwrap();
            let reply = shared.lock().unwrap().reply(&request);
            let _ = conn.write_all(reply.as_bytes());
        });
        (Compositor::new(Kind::Hyprland, path), state, dir)
    }

    /// The requests that changed something since the last call.
    fn sent<T: Logs>(state: &Mutex<T>) -> Vec<String> {
        let mut s = state.lock().unwrap();
        let log = s.log();
        let out = log
            .iter()
            .filter(|r| !r.starts_with("j/"))
            .cloned()
            .collect();
        log.clear();
        out
    }

    trait Logs {
        fn log(&mut self) -> &mut Vec<String>;
    }

    impl Logs for FakeHypr {
        fn log(&mut self) -> &mut Vec<String> {
            &mut self.log
        }
    }

    #[test]
    fn hyprland_windows_and_outputs() {
        for old in [false, true] {
            let (c, state, _dir) = fake_hypr(hypr_fixture(old));
            let kitty = top(&c, "0x55d0c1e2a9b0");
            assert_eq!(
                kitty,
                Toplevel {
                    id: "0x55d0c1e2a9b0".into(),
                    pid: Some(1234),
                    title: "kitty".into(),
                    app_id: "kitty".into(),
                    rect: Rect::new(10.0, 40.0, 940.0, 1030.0),
                    focused: true,
                    visible: true,
                    fullscreen: false,
                    floating: false,
                    xwayland: false,
                }
            );
            assert_eq!(c.active().unwrap(), Some(kitty));
            assert!(
                !top(&c, "0x55d0c1f00010").visible,
                "workspace 4 isn't shown"
            );
            let steam = top(&c, "0x55d0c1f00020");
            assert!(steam.xwayland && steam.floating && steam.visible && !steam.focused);
            assert_eq!(steam.title, "Steam");
            let calculator = win(&c, "0x55d0c1f00030");
            assert!(calculator.minimized && !calculator.top.visible);
            assert!(
                top(&c, "0x55d0c1f00040").visible,
                "HDMI-A-1 shows special:scratch"
            );
            assert!(!top(&c, "0x55d0c1f00050").visible, "hidden in its group");
            let mpv = win(&c, "0x55d0c1f00060");
            assert!(
                mpv.top.fullscreen && !mpv.maximized && mpv.top.visible,
                "old={old}"
            );
            let code = win(&c, "0x55d0c1f00070");
            assert!(!code.top.fullscreen && code.maximized, "old={old}");

            let outputs = c.outputs().unwrap();
            assert_eq!(
                outputs[0],
                Output {
                    name: "DP-1".into(),
                    rect: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                    work_area: Rect::new(0.0, 30.0, 1920.0, 1050.0),
                    scale: 4.0 / 3.0,
                    focused: true,
                }
            );
            assert_eq!(outputs[1].rect, Rect::new(1920.0, 0.0, 1280.0, 720.0));
            assert_eq!(outputs[1].work_area, outputs[1].rect);
            assert_eq!(outputs[1].scale, 1.5);
            assert_eq!(outputs[2].rect, Rect::new(3200.0, 0.0, 1080.0, 1920.0));
            assert_eq!(outputs[2].work_area, Rect::new(3200.0, 0.0, 1040.0, 1920.0));
            let displays = c.displays().unwrap();
            assert_eq!(displays.len(), 3);
            assert!(displays[0].primary && !displays[1].primary && !displays[2].primary);
            assert_eq!(displays[2].bounds, outputs[2].rect);

            // Nothing focused: activewindow is `{}`.
            state.lock().unwrap().active = None;
            assert_eq!(c.active().unwrap(), None);
            assert!(c.toplevels().unwrap().iter().all(|t| !t.focused));
        }
    }

    /// Text as Hyprland 0.41 (before the full-screen rework) prints it.
    #[test]
    fn hyprland_parses_its_own_text() {
        let clients = r#"[{
    "address": "0x5e1a8c4d2f10",
    "mapped": true,
    "hidden": false,
    "at": [1290, 42],
    "size": [1254, 1386],
    "workspace": {
        "id": 2,
        "name": "2"
    },
    "floating": false,
    "pseudo": false,
    "monitor": 0,
    "class": "org.wezfurlong.wezterm",
    "title": "~ \"quoted\"",
    "initialClass": "org.wezfurlong.wezterm",
    "initialTitle": "wezterm",
    "pid": 4242,
    "xwayland": false,
    "pinned": false,
    "fullscreen": true,
    "fullscreenMode": 0,
    "fakeFullscreen": false,
    "grouped": [],
    "tags": [],
    "swallowing": "0x0",
    "focusHistoryID": 0
}]"#;
        let monitors = r#"[{
    "id": 0,
    "name": "eDP-1",
    "description": "BOE 0x0BCA",
    "make": "BOE",
    "model": "0x0BCA",
    "serial": "",
    "width": 2256,
    "height": 1504,
    "refreshRate": 59.99900,
    "x": 0,
    "y": 0,
    "activeWorkspace": {
        "id": 2,
        "name": "2"
    },
    "specialWorkspace": {
        "id": 0,
        "name": ""
    },
    "reserved": [0, 32, 0, 0],
    "scale": 1.50,
    "transform": 0,
    "focused": true,
    "dpmsStatus": true,
    "vrr": false,
    "activelyTearing": false,
    "disabled": false,
    "currentFormat": "XRGB8888",
    "availableModes": ["2256x1504@60.00Hz"]
}]"#;
        let clients: Value = serde_json::from_str(clients).unwrap();
        let monitors: Value = serde_json::from_str(monitors).unwrap();
        let wins = hypr_windows(&clients, &monitors, &json!({}));
        let t = &wins[0].top;
        assert_eq!(t.title, "~ \"quoted\"");
        assert!(t.fullscreen && t.visible && !t.focused);
        assert_eq!(t.rect, Rect::new(1290.0, 42.0, 1254.0, 1386.0));
        let out = &hypr_outputs(&monitors)[0];
        assert_eq!(out.rect, Rect::new(0.0, 0.0, 1504.0, 1003.0));
        assert_eq!(out.work_area, Rect::new(0.0, 32.0, 1504.0, 971.0));
    }

    #[test]
    fn hyprland_scale() {
        assert_eq!(hypr_scale(1.33, 2560.0, 1440.0), 4.0 / 3.0);
        assert_eq!(hypr_scale(1.5, 1920.0, 1080.0), 1.5);
        assert_eq!(hypr_scale(1.0, 1366.0, 768.0), 1.0);
        assert_eq!(hypr_scale(1.25, 2560.0, 1600.0), 1.25);
        // No step gives whole numbers: as printed.
        assert_eq!(hypr_scale(1.17, 1001.0, 999.0), 1.17);
        assert_eq!(hypr_scale(0.0, 100.0, 100.0), 1.0);
    }

    #[test]
    fn hyprland_window_ops() {
        for old in [false, true] {
            let (c, state, _dir) = fake_hypr(hypr_fixture(old));
            let ff = "0x55d0c1f00010";
            let addr = format!("address:{ff}");
            sent(&state);

            // A tiled window: floating first, then sized and placed.
            let target = Rect::new(100.0, 50.4, 800.0, 600.0);
            c.apply(&top(&c, ff), &WindowOp::SetBounds(target)).unwrap();
            assert_eq!(
                sent(&state),
                [
                    format!("/dispatch setfloating {addr}"),
                    format!("/dispatch resizewindowpixel exact 800 600,{addr}"),
                    format!("/dispatch movewindowpixel exact 100 50,{addr}"),
                ]
            );
            let t = top(&c, ff);
            assert!(t.floating);
            assert_eq!(t.rect, Rect::new(100.0, 50.0, 800.0, 600.0));

            c.focus(&t).unwrap();
            assert_eq!(sent(&state), [format!("/dispatch focuswindow {addr}")]);
            let t = top(&c, ff);
            assert!(t.focused && t.visible);
            assert!(
                !top(&c, "0x55d0c1e2a9b0").visible,
                "DP-1 shows workspace 4 now"
            );

            let focus = format!("/dispatch focuswindow {addr}");
            let toggle = |mode: u8| format!("/dispatch fullscreen {mode}");
            c.apply(&t, &WindowOp::Fullscreen(true)).unwrap();
            assert_eq!(sent(&state), [focus.clone(), toggle(0)]);
            assert!(top(&c, ff).fullscreen);
            c.apply(&t, &WindowOp::Fullscreen(true)).unwrap();
            assert_eq!(sent(&state), Vec::<String>::new(), "already");
            c.apply(&t, &WindowOp::Fullscreen(false)).unwrap();
            assert_eq!(sent(&state), [focus.clone(), toggle(0)]);
            assert!(!top(&c, ff).fullscreen);

            c.apply(&t, &WindowOp::Maximize).unwrap();
            assert_eq!(sent(&state), [focus.clone(), toggle(1)]);
            assert!(win(&c, ff).maximized);
            // Maximized to full screen: one toggle since 0.42, two before
            // (the first one ends full screen whatever its mode).
            c.apply(&t, &WindowOp::Fullscreen(true)).unwrap();
            let expected = if old {
                vec![focus.clone(), toggle(0), focus.clone(), toggle(0)]
            } else {
                vec![focus.clone(), toggle(0)]
            };
            assert_eq!(sent(&state), expected, "old={old}");
            assert!(top(&c, ff).fullscreen);
            c.apply(&t, &WindowOp::Restore).unwrap();
            assert_eq!(sent(&state), [focus.clone(), toggle(0), focus.clone()]);
            let t = top(&c, ff);
            assert!(!t.fullscreen && t.focused && !win(&c, ff).maximized);

            c.apply(&t, &WindowOp::Minimize).unwrap();
            assert_eq!(
                sent(&state),
                [format!(
                    "/dispatch movetoworkspacesilent special:minimized,{addr}"
                )]
            );
            assert!(!top(&c, ff).visible && win(&c, ff).minimized);
            // Back to the workspace on the focused monitor (4).
            c.apply(&t, &WindowOp::Restore).unwrap();
            assert_eq!(
                sent(&state),
                [
                    format!("/dispatch movetoworkspacesilent 4,{addr}"),
                    focus.clone()
                ]
            );
            let t = top(&c, ff);
            assert!(t.visible && t.focused);

            c.apply(&t, &WindowOp::ToDesktop(4)).unwrap();
            assert_eq!(
                sent(&state),
                [format!("/dispatch movetoworkspacesilent 5,{addr}")]
            );
            assert_eq!(win(&c, ff).ws_num, Some(5));
            assert!(!top(&c, ff).visible);

            // Focus brings a minimized window back first.
            let calc = "0x55d0c1f00030";
            c.focus(&top(&c, calc)).unwrap();
            assert_eq!(
                sent(&state),
                [
                    format!("/dispatch movetoworkspacesilent 4,address:{calc}"),
                    format!("/dispatch focuswindow address:{calc}"),
                ]
            );
            assert!(top(&c, calc).visible && top(&c, calc).focused);

            // A full-screen window out of full screen before it's placed.
            let mpv = "0x55d0c1f00060";
            c.apply(
                &top(&c, mpv),
                &WindowOp::SetBounds(Rect::new(3300.0, 100.0, 640.0, 360.0)),
            )
            .unwrap();
            assert_eq!(
                sent(&state),
                [
                    format!("/dispatch focuswindow address:{mpv}"),
                    toggle(0),
                    format!("/dispatch setfloating address:{mpv}"),
                    format!("/dispatch resizewindowpixel exact 640 360,address:{mpv}"),
                    format!("/dispatch movewindowpixel exact 3300 100,address:{mpv}"),
                ]
            );

            let steam = top(&c, "0x55d0c1f00020");
            c.apply(&steam, &WindowOp::Close).unwrap();
            assert_eq!(
                sent(&state),
                ["/dispatch closewindow address:0x55d0c1f00020"]
            );
            let e = c.apply(&steam, &WindowOp::Focus).unwrap_err();
            assert!(e.to_string().contains("no longer there"), "{e}");
            let e = c
                .apply(&t, &WindowOp::SetBounds(Rect::new(0.0, 0.0, 0.0, 10.0)))
                .unwrap_err();
            assert!(matches!(e, Error::InvalidArgs(_)), "{e}");
        }
        assert!(hypr_addr("0x55d0c1f00010").is_ok());
        assert!(hypr_addr("0x12,exec rm").is_err());
        assert!(hypr_addr("55d0c1f00010").is_err());
    }

    #[test]
    fn hyprland_bind_key_leaves_the_users_bindings_alone() {
        let mut fixture = hypr_fixture(false);
        fixture.binds = vec![
            HBind {
                dispatcher: "killactive".into(),
                ..hbind(64, "Q", 0, "")
            },
            hbind(4 | 8, "Delete", 0, "wlogout"),
            // code:9 is Escape.
            hbind(4 | 8, "", 9, "notify-send hi"),
            hbind(4 | 1, "escape", 0, "swaync-client -t"),
            HBind {
                mouse: true,
                dispatcher: "movewindow".into(),
                ..hbind(64, "mouse:272", 0, "")
            },
            // Ours, from a run that crashed.
            hbind(64 | 1, "F12", 0, "kill -USR1 99 # computer-use-overlay"),
        ];
        let (c, state, _dir) = fake_hypr(fixture);
        let marked = "kill -USR1 1 # computer-use-overlay";
        for taken in [
            "super+q",
            "ctrl+alt+Delete",
            "ctrl+alt+Escape",
            "ctrl+shift+Esc",
        ] {
            assert!(!c.bind_key(&combo(taken), marked).unwrap(), "{taken}");
            c.unbind_key(&combo(taken)).unwrap();
        }
        assert_eq!(sent(&state), Vec::<String>::new(), "no keyword sent");
        assert_eq!(state.lock().unwrap().binds.len(), 6);

        // A free combination; the shell command keeps its '#' comment.
        let command = r#"test "$(cat /proc/1/comm)" = x && kill -USR1 1 # computer-use-overlay"#;
        assert!(c.bind_key(&combo("ctrl+alt+shift+F9"), command).unwrap());
        assert_eq!(
            sent(&state),
            [
                r#"/keyword bind CTRL ALT SHIFT, F9, exec, test "$(cat /proc/1/comm)" = x && kill -USR1 1 ## computer-use-overlay"#
            ]
        );
        {
            let s = state.lock().unwrap();
            let b = s.binds.last().unwrap();
            assert_eq!(
                (b.mask, b.key.as_str(), b.arg.as_str()),
                (13, "F9", command)
            );
        }
        // Again: ours is replaced, not doubled.
        assert!(c.bind_key(&combo("ctrl+alt+shift+F9"), command).unwrap());
        let f9 = |s: &FakeHypr| s.binds.iter().filter(|b| b.key == "F9").count();
        assert_eq!(f9(&state.lock().unwrap()), 1);
        sent(&state);

        // The leftover one is replaced; the marker is added when missing.
        assert!(
            c.bind_key(&combo("super+shift+F12"), "kill -USR1 2")
                .unwrap()
        );
        assert_eq!(
            sent(&state),
            [
                "/keyword unbind SHIFT SUPER, F12",
                "/keyword bind SHIFT SUPER, F12, exec, kill -USR1 2 ## computer-use-overlay",
            ]
        );
        let args: Vec<String> = state
            .lock()
            .unwrap()
            .binds
            .iter()
            .filter(|b| b.key == "F12")
            .map(|b| b.arg.clone())
            .collect();
        assert_eq!(args, ["kill -USR1 2 # computer-use-overlay"]);

        c.unbind_key(&combo("super+shift+F12")).unwrap();
        c.unbind_key(&combo("ctrl+alt+shift+F9")).unwrap();
        assert_eq!(
            sent(&state),
            [
                "/keyword unbind SHIFT SUPER, F12",
                "/keyword unbind CTRL ALT SHIFT, F9"
            ]
        );
        assert_eq!(state.lock().unwrap().binds.len(), 5);
        c.unbind_key(&combo("ctrl+alt+shift+F9")).unwrap();
        assert_eq!(sent(&state), Vec::<String>::new(), "nothing to remove");

        // If the user binds our combination after us, ours stays (removing
        // it would remove theirs).
        assert!(c.bind_key(&combo("ctrl+F7"), marked).unwrap());
        state
            .lock()
            .unwrap()
            .binds
            .push(hbind(4, "F7", 0, "playerctl next"));
        sent(&state);
        c.unbind_key(&combo("ctrl+F7")).unwrap();
        assert_eq!(sent(&state), Vec::<String>::new());

        assert!(c.bind_key(&combo("ctrl+F8"), "two\nlines").is_err());
    }

    // ---- A sway that keeps a tree and runs commands ----

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1280.0,
        height: 800.0,
    };
    /// The screen minus a bar at the top.
    const WORK: Rect = Rect {
        x: 0.0,
        y: 30.0,
        width: 1280.0,
        height: 770.0,
    };

    #[derive(Debug, Clone)]
    struct SWin {
        id: u64,
        app_id: Option<&'static str>,
        /// An X11 window's class.
        class: Option<&'static str>,
        ws: String,
        floating: bool,
        /// The container, title bar and borders included.
        con: Rect,
        border: f64,
        title_bar: f64,
        fullscreen: bool,
    }

    #[derive(Debug, Default)]
    struct FakeSway {
        wins: Vec<SWin>,
        focused: Option<u64>,
        current: String,
        config: String,
        config_path: String,
        bindings: Vec<(String, String)>,
        log: Vec<String>,
    }

    impl Logs for FakeSway {
        fn log(&mut self) -> &mut Vec<String> {
            &mut self.log
        }
    }

    fn rect_json(r: Rect) -> Value {
        json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height})
    }

    impl FakeSway {
        /// A view as sway 1.9 describes it: `rect` without the title bar,
        /// `window_rect` relative to it (y 0 under a title bar).
        fn view(&self, w: &SWin) -> Value {
            let (b, tb) = (w.border, w.title_bar);
            let rect = Rect::new(w.con.x, w.con.y + tb, w.con.width, w.con.height - tb);
            let content = if tb > 0.0 {
                Rect::new(b, 0.0, w.con.width - 2.0 * b, w.con.height - tb - b)
            } else {
                Rect::new(b, b, w.con.width - 2.0 * b, w.con.height - 2.0 * b)
            };
            let deco = Rect::new(0.0, 0.0, if tb > 0.0 { w.con.width } else { 0.0 }, tb);
            let mut v = json!({
                "id": w.id, "type": if w.floating { "floating_con" } else { "con" },
                "name": format!("window {}", w.id), "pid": w.id * 10,
                "app_id": w.app_id, "shell": if w.class.is_some() { "xwayland" } else { "xdg_shell" },
                "window": w.class.map(|_| 4194307),
                "rect": rect_json(rect), "window_rect": rect_json(content),
                "deco_rect": rect_json(deco),
                "focused": self.focused == Some(w.id), "visible": w.ws == self.current,
                "fullscreen_mode": u8::from(w.fullscreen), "nodes": [], "floating_nodes": []
            });
            if let Some(class) = w.class {
                v["window_properties"] = json!({"class": class, "instance": class.to_lowercase(), "transient_for": null});
            }
            v
        }

        fn tree(&self) -> Value {
            let views = |ws: &str, floating: bool| -> Vec<Value> {
                self.wins
                    .iter()
                    .filter(|w| w.ws == ws && w.floating == floating)
                    .map(|w| self.view(w))
                    .collect()
            };
            let mut names = vec![self.current.clone()];
            for w in &self.wins {
                if w.ws != SWAY_SCRATCH && !names.contains(&w.ws) {
                    names.push(w.ws.clone());
                }
            }
            let workspaces: Vec<Value> = names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    json!({"id": 100 + i, "type": "workspace", "name": n,
                        "num": n.parse::<i64>().unwrap_or(-1), "rect": rect_json(WORK),
                        "nodes": views(n, false), "floating_nodes": views(n, true)})
                })
                .collect();
            json!({"id": 1, "type": "root", "name": "root", "rect": rect_json(SCREEN),
                "nodes": [
                    {"id": 2147483647, "type": "output", "name": "__i3", "floating_nodes": [],
                        "nodes": [{"id": 2147483646, "type": "workspace", "name": SWAY_SCRATCH,
                            "nodes": [], "floating_nodes": views(SWAY_SCRATCH, true)}]},
                    {"id": 3, "type": "output", "name": "HEADLESS-1", "rect": rect_json(SCREEN),
                        "current_workspace": self.current, "nodes": workspaces, "floating_nodes": []}
                ],
                "floating_nodes": []})
        }

        fn outputs(&self) -> Value {
            json!([
                {"id": 3, "type": "output", "name": "HEADLESS-1", "active": true, "focused": true,
                    "scale": 1.5, "transform": "normal", "rect": rect_json(SCREEN),
                    "current_workspace": self.current},
                // Turned: sway's rect is logical, already turned.
                {"id": 6, "type": "output", "name": "DP-2", "active": true, "focused": false,
                    "scale": 1.0, "transform": "90",
                    "rect": {"x": 1280, "y": 0, "width": 1080, "height": 1920},
                    "current_workspace": "9"},
                {"id": 7, "type": "output", "name": "HDMI-A-1", "active": false, "focused": false,
                    "rect": {"x": 0, "y": 0, "width": 0, "height": 0}}
            ])
        }

        fn workspaces(&self) -> Value {
            json!([
                {"id": 100, "name": self.current, "visible": true, "focused": true,
                    "output": "HEADLESS-1", "rect": rect_json(WORK)},
                {"id": 101, "name": "9", "num": 9, "visible": true, "focused": false,
                    "output": "DP-2", "rect": {"x": 1280, "y": 0, "width": 1080, "height": 1920}},
                {"id": 102, "name": "7", "num": 7, "visible": false, "focused": false,
                    "output": "HEADLESS-1", "rect": rect_json(WORK)}
            ])
        }

        fn run(&mut self, cmd: &str) -> Value {
            self.log.push(cmd.to_string());
            let fail = |e: &str| json!([{"success": false, "parse_error": false, "error": e}]);
            if let Some(rest) = cmd.strip_prefix("bindsym --no-repeat ") {
                let (keys, command) = rest.split_once(' ').unwrap();
                self.bindings.retain(|(k, _)| k != keys);
                self.bindings.push((keys.into(), command.into()));
                return json!([{"success": true}]);
            }
            if let Some(keys) = cmd.strip_prefix("unbindsym ") {
                let before = self.bindings.len();
                self.bindings.retain(|(k, _)| k != keys);
                if self.bindings.len() == before {
                    return fail(&format!(
                        "Could not find binding `{keys}` for the given flags"
                    ));
                }
                return json!([{"success": true}]);
            }
            let Some((id, chain)) = cmd
                .strip_prefix("[con_id=")
                .and_then(|r| r.split_once("] "))
            else {
                return fail("Unknown/invalid command");
            };
            let id: u64 = id.parse().unwrap();
            let mut results = Vec::new();
            for part in chain.split(',').map(str::trim) {
                let Some(i) = self.wins.iter().position(|w| w.id == id) else {
                    return fail("No matching node.");
                };
                results.push(if self.command(i, part) {
                    json!({"success": true})
                } else {
                    json!({"success": false, "error": format!("Unknown/invalid command '{part}'")})
                });
            }
            Value::Array(results)
        }

        fn command(&mut self, i: usize, cmd: &str) -> bool {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            let current = self.current.clone();
            let id = self.wins[i].id;
            let f = |s: &str| s.parse::<f64>().unwrap();
            let w = &mut self.wins[i];
            match words.as_slice() {
                ["focus"] => {
                    if w.ws == SWAY_SCRATCH {
                        w.ws = current;
                    } else {
                        self.current = w.ws.clone();
                    }
                    self.focused = Some(id);
                }
                ["floating", "enable"] => {
                    if !w.floating {
                        // default_floating_border normal 2
                        w.floating = true;
                        w.border = 2.0;
                        w.title_bar = 24.0;
                        w.con = Rect::new(340.0, 200.0, 600.0, 400.0);
                    }
                }
                ["floating", "disable"] => {
                    w.floating = false;
                    w.border = 0.0;
                    w.title_bar = 0.0;
                    w.con = WORK;
                    if w.ws == SWAY_SCRATCH {
                        w.ws = current;
                    }
                }
                [
                    "resize",
                    "set",
                    "width",
                    width,
                    "px",
                    "height",
                    height,
                    "px",
                ] => {
                    let (width, height) = (f(width), f(height));
                    // Keeps the center.
                    w.con.x -= ((width - w.con.width) / 2.0).trunc();
                    w.con.y -= ((height - w.con.height) / 2.0).trunc();
                    w.con.width = width;
                    w.con.height = height;
                }
                ["move", "absolute", "position", x, y] => {
                    w.con.x = f(x);
                    w.con.y = f(y);
                }
                ["fullscreen", "enable"] => w.fullscreen = true,
                ["fullscreen", "disable"] => w.fullscreen = false,
                ["move", "scratchpad"] => {
                    w.ws = SWAY_SCRATCH.into();
                    w.floating = true;
                    if self.focused == Some(id) {
                        self.focused = None;
                    }
                }
                ["scratchpad", "show"] => {
                    if w.ws == SWAY_SCRATCH {
                        w.ws = current;
                        self.focused = Some(id);
                    }
                }
                ["kill"] => {
                    self.wins.remove(i);
                    if self.focused == Some(id) {
                        self.focused = None;
                    }
                }
                ["move", "container", "to", "workspace", "number", n] => w.ws = n.to_string(),
                _ => return false,
            }
            true
        }
    }

    fn sway_fixture() -> FakeSway {
        let win = |id, app_id, ws: &str, con| SWin {
            id,
            app_id: Some(app_id),
            class: None,
            ws: ws.into(),
            floating: false,
            con,
            border: 2.0,
            title_bar: 0.0,
            fullscreen: false,
        };
        FakeSway {
            wins: vec![
                win(10, "foot", "1", Rect::new(0.0, 30.0, 640.0, 770.0)),
                SWin {
                    app_id: None,
                    class: Some("Gimp-2.10"),
                    ..win(11, "", "1", Rect::new(640.0, 30.0, 640.0, 770.0))
                },
                SWin {
                    floating: true,
                    title_bar: 24.0,
                    ..win(12, "mpv", "1", Rect::new(100.0, 100.0, 400.0, 300.0))
                },
                SWin {
                    floating: true,
                    ..win(
                        13,
                        "pavucontrol",
                        SWAY_SCRATCH,
                        Rect::new(300.0, 200.0, 500.0, 400.0),
                    )
                },
                win(14, "thunderbird", "2", WORK),
            ],
            focused: Some(10),
            current: "1".into(),
            ..Default::default()
        }
    }

    fn fake_sway(state: FakeSway) -> (Compositor, Arc<Mutex<FakeSway>>, TempDir) {
        let dir = TempDir::new("sway");
        let path = dir.0.join("sway-ipc.sock");
        let state = Arc::new(Mutex::new(state));
        let shared = Arc::clone(&state);
        serve(&path, move |mut conn| {
            loop {
                let mut head = [0u8; 14];
                if conn.read_exact(&mut head).is_err() {
                    return;
                }
                assert_eq!(&head[..6], I3_MAGIC);
                let len = u32::from_ne_bytes(head[6..10].try_into().unwrap());
                let kind = u32::from_ne_bytes(head[10..14].try_into().unwrap());
                let mut body = vec![0; len as usize];
                conn.read_exact(&mut body).unwrap();
                let payload = String::from_utf8(body).unwrap();
                let reply = {
                    let mut s = shared.lock().unwrap();
                    match kind {
                        RUN_COMMAND => s.run(&payload),
                        GET_WORKSPACES => s.workspaces(),
                        GET_OUTPUTS => s.outputs(),
                        GET_TREE => s.tree(),
                        GET_VERSION => json!({"human_readable": "1.9", "variant": "sway",
                            "major": 1, "minor": 9, "patch": 0,
                            "loaded_config_file_name": s.config_path}),
                        GET_CONFIG => json!({"config": s.config}),
                        _ => json!({"success": false}),
                    }
                };
                let body = serde_json::to_vec(&reply).unwrap();
                // An event first, as a subscriber would get: skipped.
                let mut msg = I3_MAGIC.to_vec();
                msg.extend(2u32.to_ne_bytes());
                msg.extend(0x8000_0002u32.to_ne_bytes());
                msg.extend(b"{}");
                msg.extend(I3_MAGIC);
                msg.extend((body.len() as u32).to_ne_bytes());
                msg.extend(kind.to_ne_bytes());
                msg.extend(body);
                if conn.write_all(&msg).is_err() {
                    return;
                }
            }
        });
        (Compositor::new(Kind::Sway, path), state, dir)
    }

    #[test]
    fn sway_windows_and_outputs() {
        let (c, state, _dir) = fake_sway(sway_fixture());
        let foot = top(&c, "10");
        assert_eq!(
            foot,
            Toplevel {
                id: "10".into(),
                pid: Some(100),
                title: "window 10".into(),
                app_id: "foot".into(),
                rect: Rect::new(2.0, 32.0, 636.0, 766.0),
                focused: true,
                visible: true,
                fullscreen: false,
                floating: false,
                xwayland: false,
            }
        );
        assert_eq!(c.active().unwrap(), Some(foot));
        let gimp = top(&c, "11");
        assert!(gimp.xwayland && gimp.visible);
        assert_eq!(gimp.app_id, "Gimp-2.10");
        // Under a title bar.
        let mpv = win(&c, "12");
        assert!(mpv.top.floating);
        assert_eq!(mpv.top.rect, Rect::new(102.0, 124.0, 396.0, 274.0));
        assert_eq!(mpv.frame, [2.0, 24.0, 2.0, 2.0]);
        let pavucontrol = win(&c, "13");
        assert!(pavucontrol.minimized && !pavucontrol.top.visible);
        let thunderbird = win(&c, "14");
        assert!(!thunderbird.top.visible && thunderbird.ws_num == Some(2));

        let outputs = c.outputs().unwrap();
        assert_eq!(
            outputs,
            [
                Output {
                    name: "HEADLESS-1".into(),
                    rect: SCREEN,
                    work_area: WORK,
                    scale: 1.5,
                    focused: true,
                },
                Output {
                    name: "DP-2".into(),
                    rect: Rect::new(1280.0, 0.0, 1080.0, 1920.0),
                    work_area: Rect::new(1280.0, 0.0, 1080.0, 1920.0),
                    scale: 1.0,
                    focused: false,
                },
            ]
        );
        let displays = c.displays().unwrap();
        assert!(displays[0].primary && !displays[1].primary);
        assert_eq!(displays[0].work_area, WORK);

        state.lock().unwrap().focused = None;
        assert_eq!(c.active().unwrap(), None);
    }

    /// GET_TREE from sway 1.9 with gtk3-widget-factory (scripts/wayland-session.sh).
    #[test]
    fn sway_parses_its_own_tree() {
        let tree = r#"{"id":1,"type":"root","orientation":"horizontal","percent":null,"urgent":false,"marks":[],"focused":false,"layout":"splith","border":"none","current_border_width":0,"rect":{"x":0,"y":0,"width":1280,"height":800},"deco_rect":{"x":0,"y":0,"width":0,"height":0},"window_rect":{"x":0,"y":0,"width":0,"height":0},"geometry":{"x":0,"y":0,"width":0,"height":0},"name":"root","window":null,"nodes":[{"id":2147483647,"type":"output","orientation":"horizontal","percent":null,"urgent":false,"marks":[],"focused":false,"layout":"output","border":"none","current_border_width":0,"rect":{"x":0,"y":0,"width":1280,"height":800},"deco_rect":{"x":0,"y":0,"width":0,"height":0},"window_rect":{"x":0,"y":0,"width":0,"height":0},"geometry":{"x":0,"y":0,"width":0,"height":0},"name":"__i3","window":null,"nodes":[{"id":2147483646,"type":"workspace","orientation":"horizontal","percent":null,"urgent":false,"marks":[],"focused":false,"layout":"splith","border":"none","current_border_width":0,"rect":{"x":0,"y":0,"width":1280,"height":800},"deco_rect":{"x":0,"y":0,"width":0,"height":0},"window_rect":{"x":0,"y":0,"width":0,"height":0},"geometry":{"x":0,"y":0,"width":0,"height":0},"name":"__i3_scratch","window":null,"nodes":[],"floating_nodes":[],"focus":[],"fullscreen_mode":1,"sticky":false}],"floating_nodes":[],"focus":[2147483646],"fullscreen_mode":0,"sticky":false},{"id":3,"type":"output","orientation":"none","percent":1.0,"urgent":false,"marks":[],"focused":false,"layout":"output","border":"none","current_border_width":0,"rect":{"x":0,"y":0,"width":1280,"height":800},"deco_rect":{"x":0,"y":0,"width":0,"height":0},"window_rect":{"x":0,"y":0,"width":0,"height":0},"geometry":{"x":0,"y":0,"width":0,"height":0},"name":"HEADLESS-1","window":null,"nodes":[{"id":4,"type":"workspace","orientation":"horizontal","percent":null,"urgent":false,"marks":[],"focused":false,"layout":"splith","border":"none","current_border_width":0,"rect":{"x":0,"y":0,"width":1280,"height":800},"deco_rect":{"x":0,"y":0,"width":0,"height":0},"window_rect":{"x":0,"y":0,"width":0,"height":0},"geometry":{"x":0,"y":0,"width":0,"height":0},"name":"1","window":null,"nodes":[{"id":5,"type":"con","orientation":"none","percent":1.0,"urgent":false,"marks":[],"focused":true,"layout":"none","border":"none","current_border_width":2,"rect":{"x":0,"y":0,"width":1280,"height":800},"deco_rect":{"x":0,"y":0,"width":0,"height":0},"window_rect":{"x":0,"y":0,"width":1280,"height":800},"geometry":{"x":0,"y":0,"width":1415,"height":732},"name":"gtk3-widget-factory","window":null,"nodes":[],"floating_nodes":[],"focus":[],"fullscreen_mode":0,"sticky":false,"pid":4456,"app_id":"gtk3-widget-factory","visible":true,"max_render_time":0,"shell":"xdg_shell","inhibit_idle":false,"idle_inhibitors":{"user":"none","application":"none"}}],"floating_nodes":[],"focus":[5],"fullscreen_mode":1,"sticky":false,"num":1,"output":"HEADLESS-1","representation":"H[gtk3-widget-factory]"}],"floating_nodes":[],"focus":[4],"fullscreen_mode":0,"sticky":false,"primary":false,"make":"Unknown","model":"Unknown","serial":"Unknown","modes":[],"non_desktop":false,"active":true,"dpms":true,"power":true,"scale":1.0,"scale_filter":"nearest","transform":"normal","adaptive_sync_status":"disabled","current_workspace":"1","current_mode":{"width":1280,"height":800,"refresh":0},"max_render_time":0}],"floating_nodes":[],"focus":[3],"fullscreen_mode":0,"sticky":false}"#;
        let wins = sway_windows(&serde_json::from_str(tree).unwrap());
        assert_eq!(wins.len(), 1);
        assert_eq!(
            wins[0].top,
            Toplevel {
                id: "5".into(),
                pid: Some(4456),
                title: "gtk3-widget-factory".into(),
                app_id: "gtk3-widget-factory".into(),
                rect: SCREEN,
                focused: true,
                visible: true,
                fullscreen: false,
                floating: false,
                xwayland: false,
            }
        );
        assert_eq!(wins[0].ws_num, Some(1));
    }

    #[test]
    fn sway_window_ops() {
        let (c, state, _dir) = fake_sway(sway_fixture());
        let con = |id: u64, cmd: &str| format!("[con_id={id}] {cmd}");

        // Tiled: floating first; then the container is placed so that the
        // content box (inside a 2 px border and a 24 px title bar) is where
        // asked.
        let t = top(&c, "14");
        c.apply(
            &t,
            &WindowOp::SetBounds(Rect::new(100.0, 200.0, 500.0, 400.0)),
        )
        .unwrap();
        assert_eq!(
            sent(&state),
            [
                con(14, "floating enable"),
                con(
                    14,
                    "resize set width 504 px height 426 px, move absolute position 98 176"
                ),
            ]
        );
        assert_eq!(top(&c, "14").rect, Rect::new(100.0, 200.0, 500.0, 400.0));

        c.focus(&t).unwrap();
        assert_eq!(sent(&state), [con(14, "focus")]);
        assert!(top(&c, "14").focused && top(&c, "14").visible);

        c.apply(&t, &WindowOp::Fullscreen(true)).unwrap();
        assert!(top(&c, "14").fullscreen);
        c.apply(&t, &WindowOp::Fullscreen(false)).unwrap();
        c.apply(&t, &WindowOp::Fullscreen(false)).unwrap();
        assert_eq!(
            sent(&state),
            [con(14, "fullscreen enable"), con(14, "fullscreen disable")]
        );

        // Maximize: floating, the output's work area; Restore tiles it again.
        let foot = top(&c, "10");
        c.apply(&foot, &WindowOp::Maximize).unwrap();
        assert_eq!(
            sent(&state),
            [
                con(10, "floating enable"),
                con(
                    10,
                    "resize set width 1284 px height 796 px, move absolute position -2 6"
                ),
            ]
        );
        assert_eq!(top(&c, "10").rect, WORK);
        c.apply(&foot, &WindowOp::Maximize).unwrap();
        assert_eq!(sent(&state), Vec::<String>::new(), "already");
        c.apply(&foot, &WindowOp::Restore).unwrap();
        assert_eq!(
            sent(&state),
            [con(10, "floating disable"), con(10, "focus")]
        );
        assert!(!top(&c, "10").floating && top(&c, "10").focused);

        // Minimize: the scratchpad; Restore brings it back as it was (tiled).
        c.apply(&foot, &WindowOp::Minimize).unwrap();
        assert_eq!(sent(&state), [con(10, "move scratchpad")]);
        assert!(!top(&c, "10").visible);
        c.apply(&foot, &WindowOp::Restore).unwrap();
        assert_eq!(
            sent(&state),
            [
                con(10, "scratchpad show"),
                con(10, "floating disable"),
                con(10, "focus")
            ]
        );
        let back = top(&c, "10");
        assert!(back.visible && back.focused && !back.floating);

        // A floating window comes back where it was.
        let mpv = top(&c, "12");
        c.apply(&mpv, &WindowOp::Minimize).unwrap();
        c.apply(&mpv, &WindowOp::Restore).unwrap();
        assert_eq!(
            sent(&state),
            [
                con(12, "move scratchpad"),
                con(12, "scratchpad show"),
                con(
                    12,
                    "resize set width 400 px height 300 px, move absolute position 100 100"
                ),
                con(12, "focus"),
            ]
        );
        assert_eq!(top(&c, "12").rect, mpv.rect);

        // Focus shows a window from the scratchpad.
        c.focus(&top(&c, "13")).unwrap();
        assert_eq!(sent(&state), [con(13, "scratchpad show"), con(13, "focus")]);
        assert!(top(&c, "13").visible && top(&c, "13").focused);

        c.apply(&top(&c, "11"), &WindowOp::ToDesktop(4)).unwrap();
        assert_eq!(
            sent(&state),
            [con(11, "move container to workspace number 5")]
        );
        assert_eq!(win(&c, "11").ws_num, Some(5));
        c.apply(&top(&c, "11"), &WindowOp::Close).unwrap();
        assert_eq!(sent(&state), [con(11, "kill")]);
        assert!(c.toplevels().unwrap().iter().all(|t| t.id != "11"));

        assert!(sway_con("12").is_ok());
        assert!(sway_con("12] kill; [con_id=1").is_err());
    }

    #[test]
    fn sway_bind_key_leaves_the_users_bindings_alone() {
        let dir = TempDir::new("sway-config");
        let d = &dir.0;
        std::fs::create_dir_all(d.join("conf.d")).unwrap();
        std::fs::write(
            d.join("conf.d/50-keys.conf"),
            "bindsym $mod+Shift+F12 exec grim\n",
        )
        .unwrap();
        // Hidden files don't match a glob.
        std::fs::write(d.join("conf.d/.hidden"), "bindsym Ctrl+F5 exec x\n").unwrap();
        let config = r#"
# sway config
set $mod Mod4
set $term foot
bindsym $mod+Return exec $term
bindsym --to-code Ctrl+Alt+Delete exec swaynag -m bye
bindcode Mod4+9 exec swaylock
bindsym {
    $mod+Shift+e exit
    Ctrl+Mod1+End \
        exec systemctl suspend
}
mode "resize" {
    bindsym Ctrl+Alt+Home resize grow width 10px
}
bindsym
{
    Mod4+F1 exec help
}
include ~/nonexistent-dir-for-test/*
include conf.d/*
bindsym Ctrl+Alt+Escape exec old-helper # computer-use-overlay
"#;
        let fixture = FakeSway {
            config: config.into(),
            config_path: d.join("config").display().to_string(),
            ..sway_fixture()
        };
        let (c, state, _sock) = fake_sway(fixture);
        let marked = "kill -USR1 1 # computer-use-overlay";
        for taken in [
            "super+Return",
            "ctrl+alt+Delete",
            "super+Escape",
            "super+shift+e",
            "ctrl+alt+End",
            "super+F1",
            "super+shift+F12",
        ] {
            assert!(!c.bind_key(&combo(taken), marked).unwrap(), "{taken}");
            c.unbind_key(&combo(taken)).unwrap();
        }
        assert_eq!(sent(&state), Vec::<String>::new());

        // Only in another mode; only in a hidden file; ours.
        for (free, keys) in [
            ("ctrl+alt+Home", "Ctrl+Alt+Home"),
            ("ctrl+F5", "Ctrl+F5"),
            ("ctrl+alt+Escape", "Ctrl+Alt+Escape"),
        ] {
            assert!(c.bind_key(&combo(free), "kill -USR1 1").unwrap(), "{free}");
            assert_eq!(
                sent(&state),
                [format!(
                    "bindsym --no-repeat {keys} exec kill -USR1 1 # computer-use-overlay"
                )]
            );
        }
        c.unbind_key(&combo("ctrl+alt+Home")).unwrap();
        assert_eq!(sent(&state), ["unbindsym Ctrl+Alt+Home"]);
        // Not bound (any more): fine.
        c.unbind_key(&combo("ctrl+alt+Home")).unwrap();
        assert_eq!(state.lock().unwrap().bindings.len(), 2);

        let e = c.bind_key(&combo("ctrl+F6"), "a; b").unwrap_err();
        assert!(matches!(e, Error::InvalidArgs(_)), "{e}");
        assert!(c.bind_key(&combo("ctrl+F6"), "sh -c 'a; b'").unwrap());
        assert!(
            c.bind_key(
                &combo("super+shift+F11"),
                r#"test "$(cat /proc/1/comm)" = x && kill -USR1 1"#
            )
            .unwrap()
        );
    }

    #[test]
    fn key_names() {
        let chord = |s| Chord::of(&combo(s)).unwrap();
        let c = chord("ctrl+alt+Escape");
        assert_eq!(
            (c.hypr_mods(), c.name.as_str(), c.sway(), c.code),
            (
                "CTRL ALT".into(),
                "Escape",
                "Ctrl+Alt+Escape".into(),
                Some(9)
            )
        );
        let c = chord("super+shift+F12");
        assert_eq!(
            (c.hypr_mods(), c.sway()),
            ("SHIFT SUPER".into(), "Shift+Mod4+F12".into())
        );
        assert_eq!(c.code, Some(96));
        assert_eq!(chord("F1").hypr_mods(), "");
        assert_eq!(chord("F1").sway(), "F1");
        for (key, name) in [
            ("ctrl+,", "comma"),
            ("ctrl++", "plus"),
            ("ctrl+A", "a"),
            ("ctrl+1", "1"),
            ("Page_Up", "Page_Up"),
            ("numpad7", "KP_7"),
            ("NumpadEnter", "KP_Enter"),
            ("space", "space"),
            ("ctrl+é", "U00E9"),
            ("ctrl+/", "slash"),
        ] {
            assert_eq!(chord(key).name, name, "{key}");
        }
        assert!(same_key("ESCAPE", "Escape"));
        assert!(same_key("Prior", "Page_Up"));
        assert!(!same_key("Return", "KP_Enter"));
        assert!(
            Chord::of(&KeyCombo {
                modifiers: Default::default(),
                key: Key::Char('\u{1}')
            })
            .is_err()
        );
    }

    #[test]
    fn finds_the_socket() {
        let os = |s: &str| std::ffi::OsString::from(s);
        let sig = os("abc_1700000000_1");
        let run = os("/run/user/1000");
        let xdg = PathBuf::from("/run/user/1000/hypr/abc_1700000000_1/.socket.sock");
        let tmp = PathBuf::from("/tmp/hypr/abc_1700000000_1/.socket.sock");
        let sway = os("/run/user/1000/sway-ipc.1000.42.sock");
        let only = |p: Vec<PathBuf>| move |q: &Path| p.iter().any(|x| x == q);
        let find = |sig: Option<&std::ffi::OsString>, sway: Option<&std::ffi::OsString>, exists| {
            find_socket(
                sig.map(|s| s.as_os_str()),
                Some(run.as_os_str()),
                sway.map(|s| s.as_os_str()),
                exists,
            )
        };
        assert_eq!(
            find(
                Some(&sig),
                Some(&sway),
                only(vec![xdg.clone(), tmp.clone()])
            ),
            Some((Kind::Hyprland, xdg.clone()))
        );
        assert_eq!(
            find(Some(&sig), None, only(vec![tmp.clone()])),
            Some((Kind::Hyprland, tmp))
        );
        assert_eq!(
            find(Some(&sig), Some(&sway), only(vec![PathBuf::from(&sway)])),
            Some((Kind::Sway, PathBuf::from(&sway)))
        );
        assert_eq!(find(None, Some(&sway), only(vec![])), None);
        assert_eq!(find(Some(&os("")), None, only(vec![xdg])), None);
        assert_eq!(find(None, None, only(vec![])), None);
    }

    #[test]
    fn a_hung_compositor_times_out() {
        let dir = TempDir::new("hung");
        let path = dir.0.join("sock");
        // Accepts, never answers.
        serve(&path, |conn| {
            std::thread::sleep(Duration::from_secs(3));
            drop(conn);
        });
        for kind in [Kind::Sway, Kind::Hyprland] {
            let c = Compositor {
                timeout: Duration::from_millis(150),
                ..Compositor::new(kind, path.clone())
            };
            let started = Instant::now();
            let e = c.toplevels().unwrap_err().to_string();
            assert!(e.contains("didn't answer"), "{e}");
            assert!(started.elapsed() < Duration::from_secs(1));
        }
        let c = Compositor::new(Kind::Sway, dir.0.join("missing"));
        let e = c.outputs().unwrap_err().to_string();
        assert!(e.contains("missing"), "{e}");
    }

    /// A listener whose queue is full (it never accepts) makes a plain
    /// connect wait forever.
    #[test]
    fn connect_gives_up_on_a_full_queue() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let dir = TempDir::new("queue");
        let path = dir.0.join("sock");
        let addr = sockaddr(&path).unwrap();
        // SAFETY: socket/bind/listen on a descriptor we own; results checked.
        let _listener = unsafe {
            let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
            assert!(fd >= 0);
            let fd = OwnedFd::from_raw_fd(fd);
            let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
            assert_eq!(
                libc::bind(fd.as_raw_fd(), std::ptr::from_ref(&addr).cast(), len),
                0
            );
            assert_eq!(libc::listen(fd.as_raw_fd(), 0), 0);
            fd
        };
        let mut held = Vec::new();
        let started = Instant::now();
        let e = loop {
            match connect(&path, Duration::from_millis(100)) {
                Ok(s) => held.push(s),
                Err(e) => break e,
            }
            assert!(held.len() < 64, "connect never gave up");
        };
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn sway_config_reading() {
        assert_eq!(
            config_lines("# c\n\n  a \\\n  b\nc {\n}\n"),
            ["a b", "c {", "}"]
        );
        assert!(wildcard("*.conf", "50-keys.conf"));
        assert!(!wildcard("*.conf", "50-keys.conf~"));
        assert!(wildcard("*", "x"));
        assert!(!wildcard("*", ".x"));
        assert!(wildcard("a?c*", "abcdef"));
        assert!(has_unquoted("a; b", &[';']));
        assert!(!has_unquoted("sh -c 'a; b'", &[';']));
        assert!(!has_unquoted(r#"echo "a, b""#, &[',']));
    }

    // ---- The real thing ----

    /// Against a real sway, from
    /// `APPS="gtk3-widget-factory;xeyes" scripts/wayland-session.sh cargo test -p computer-use --lib live_sway -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a sway session: run it under scripts/wayland-session.sh"]
    fn live_sway_session() {
        let c =
            Compositor::detect().expect("no compositor: run this under scripts/wayland-session.sh");
        assert_eq!(c.kind(), Kind::Sway);
        let find = |id: &str| top(&c, id);
        let deadline = Instant::now() + Duration::from_secs(20);
        let app = loop {
            let tops = c.toplevels().unwrap();
            if let Some(t) = tops.into_iter().find(|t| t.app_id == "gtk3-widget-factory") {
                break t;
            }
            assert!(
                Instant::now() < deadline,
                "gtk3-widget-factory never showed up"
            );
            std::thread::sleep(Duration::from_millis(200));
        };
        println!("toplevels: {:#?}", c.toplevels().unwrap());
        assert!(app.pid.is_some() && !app.xwayland && app.visible, "{app:?}");

        let outputs = c.outputs().unwrap();
        let displays = c.displays().unwrap();
        println!("outputs: {outputs:?}\ndisplays: {displays:?}");
        let screen = outputs[0].clone();
        assert!(screen.focused && !screen.rect.is_empty());
        assert!(displays[0].primary);
        assert_eq!(displays[0].bounds, screen.rect);

        // An X11 app, when one was started.
        if let Some(x) = c.toplevels().unwrap().into_iter().find(|t| t.xwayland) {
            println!("XWayland window: {x:?}");
            assert!(!x.app_id.is_empty() && x.pid.is_some());
            c.focus(&x).unwrap();
            assert_eq!(c.active().unwrap().map(|t| t.id), Some(x.id));
        }
        c.focus(&app).unwrap();
        assert_eq!(c.active().unwrap().map(|t| t.id), Some(app.id.clone()));

        // A tiled window: made floating, its content box where asked.
        let s = screen.rect;
        let target = Rect::new(
            s.x + 40.0,
            s.y + 30.0,
            (s.width * 0.7).round(),
            (s.height * 0.7).round(),
        );
        let started = Instant::now();
        c.apply(&app, &WindowOp::SetBounds(target)).unwrap();
        let now = find(&app.id);
        println!(
            "set_bounds {target:?} -> {:?} in {:?}",
            now.rect,
            started.elapsed()
        );
        assert!(now.floating);
        assert_eq!((now.rect.x, now.rect.y), (target.x, target.y));
        // What the app took (it has a minimum size).
        let size = (now.rect.width, now.rect.height);

        // With borders and a title bar too.
        for border in ["pixel 7", "normal 3"] {
            c.sway_run(&format!("[con_id={}] border {border}", app.id))
                .unwrap();
            let t = Rect::new(s.x + 100.0, s.y + 80.0, size.0, size.1);
            c.apply(&find(&app.id), &WindowOp::SetBounds(t)).unwrap();
            let now = find(&app.id);
            println!("border {border}: set_bounds {t:?} -> {:?}", now.rect);
            assert_eq!(now.rect, t, "border {border}");
        }
        c.sway_run(&format!("[con_id={}] border none", app.id))
            .unwrap();

        c.apply(&app, &WindowOp::Fullscreen(true)).unwrap();
        let now = find(&app.id);
        println!("full screen: {:?}", now.rect);
        assert!(now.fullscreen);
        assert_eq!(now.rect, s);
        c.apply(&app, &WindowOp::Fullscreen(false)).unwrap();
        assert!(!find(&app.id).fullscreen);

        let before = find(&app.id).rect;
        c.apply(&app, &WindowOp::Maximize).unwrap();
        let now = find(&app.id);
        println!(
            "maximized: {:?} (work area {:?})",
            now.rect, screen.work_area
        );
        assert_eq!(
            (now.rect.x, now.rect.y),
            (screen.work_area.x, screen.work_area.y)
        );
        c.apply(&app, &WindowOp::Restore).unwrap();
        let now = find(&app.id);
        println!("restored: {:?}", now.rect);
        assert_eq!(now.rect, before);
        assert!(now.focused);

        c.apply(&app, &WindowOp::Minimize).unwrap();
        let now = find(&app.id);
        assert!(!now.visible && !now.focused);
        c.apply(&app, &WindowOp::Restore).unwrap();
        let now = find(&app.id);
        println!("back from the scratchpad: {:?}", now.rect);
        assert!(now.visible && now.focused);
        assert_eq!(now.rect, before);

        c.apply(&app, &WindowOp::ToDesktop(1)).unwrap();
        assert!(!find(&app.id).visible);
        c.focus(&app).unwrap();
        let now = find(&app.id);
        assert!(now.visible && now.focused);
        c.apply(&app, &WindowOp::ToDesktop(0)).unwrap();
        c.focus(&app).unwrap();

        // A global key binding runs its command; after unbind_key it doesn't.
        let fired = std::env::temp_dir().join(format!("cu-ipc-fired-{}", std::process::id()));
        let _ = std::fs::remove_file(&fired);
        let keys = combo("ctrl+alt+Escape");
        let command = format!(
            "test \"$(echo ok)\" = ok && touch '{}' # {BIND_MARKER}",
            fired.display()
        );
        assert!(c.bind_key(&keys, &command).unwrap());
        let press = || {
            let status = std::process::Command::new("wtype")
                .args([
                    "-M", "ctrl", "-M", "alt", "-k", "Escape", "-m", "alt", "-m", "ctrl",
                ])
                .status()
                .expect("wtype");
            assert!(status.success());
        };
        press();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !fired.exists() {
            assert!(
                Instant::now() < deadline,
                "the binding didn't run its command"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        println!("the binding ran its command");
        // Again: replaced.
        assert!(c.bind_key(&keys, &command).unwrap());
        c.unbind_key(&keys).unwrap();
        std::fs::remove_file(&fired).unwrap();
        press();
        std::thread::sleep(Duration::from_millis(800));
        assert!(!fired.exists(), "the binding still fires after unbind_key");
        c.unbind_key(&keys).unwrap();

        c.apply(&find(&app.id), &WindowOp::Close).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.toplevels().unwrap().iter().any(|t| t.id == app.id) {
            assert!(Instant::now() < deadline, "the window didn't close");
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("closed");
    }
}
