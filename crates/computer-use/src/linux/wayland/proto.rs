//! Wayland protocols: the output layout, screenshots (wlr-screencopy),
//! pointer and keyboard input (wlr-virtual-pointer, virtual-keyboard) and
//! the user's idle time (ext-idle-notify), spoken to the compositor
//! directly (pure-Rust client, no libwayland).
//!
//! Positions are in the compositor's logical layout, the space sway's and
//! Hyprland's IPC report windows in. A screenshot comes at the pixel
//! density of the highest-scaled output it touches.
//!
//! Nothing here waits on the compositor without a deadline: one that stops
//! answering gives an error, never a hung call.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{ErrorKind, Write as _};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

use image::{Rgba, RgbaImage, imageops};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};

use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, NamedKey, Pad};
use crate::types::{Capture, Rect};

/// How long the compositor gets to answer a roundtrip.
const ROUNDTRIP: Duration = Duration::from_secs(2);
/// How long a screenshot may take, all outputs together.
const CAPTURE_TIME: Duration = Duration::from_secs(3);
/// Pace of synthesized drag steps (as on X11): toolkits that start a drag
/// on a motion threshold or a timer miss a single burst of events.
const DRAG_STEP: Duration = Duration::from_millis(12);
/// Time for apps to notice a new input device (the seat gains a pointer or
/// keyboard if it had none, and apps bind theirs) before its first event.
const NEW_DEVICE_SETTLE: Duration = Duration::from_millis(150);
/// Before keycodes are given other keysyms, time for apps (XWayland ones
/// above all) to translate the presses made with the old keymap.
const REMAP_SETTLE: Duration = Duration::from_millis(50);
/// Pause after each typed character. Keys sent in a burst pile up in the
/// app: GTK 3 with the accessibility bridge on (as it is for this program)
/// handles queued key events re-entrantly and was seen to crash on bursts
/// that `wtype`'s pace never crashed. A pace also keeps a busy app's socket
/// from overflowing (the compositor disconnects an app it can't send to).
const TYPE_PAUSE: Duration = Duration::from_millis(3);
/// The usual wl_pointer axis value of one wheel click.
const WHEEL_STEP: f64 = 15.0;
/// Pause between wheel clicks, like a real wheel's.
const WHEEL_PAUSE: Duration = Duration::from_millis(4);
/// Absolute pointer positions are sent in 1/8 logical units (the protocol
/// takes whole numbers over an extent).
const POINTER_PRECISION: f64 = 8.0;
/// Most keys the uploaded keymap may have: XWayland apps see keycodes
/// 9..=255, so a keymap stays well under 247 keys.
const KEYMAP_CAP: usize = 240;
/// ext-idle-notify timeouts. One short timeout alone can't tell the idle
/// time once its `idled` event has sat unread (the agent thinks for a
/// while between actions, and events carry no time): the longest timeout
/// that has passed bounds it from below, the next one from above.
const IDLE_RUNGS_MS: [u32; 12] = [
    100, 250, 500, 1_000, 1_500, 2_000, 3_000, 5_000, 10_000, 30_000, 60_000, 300_000,
];
/// Slack for the compositor's idle timer to fire and its event to arrive.
const IDLE_SLACK: Duration = Duration::from_millis(50);

/// Linux input event codes of the mouse buttons.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

/// wl_keyboard keymap format: libxkbcommon's text format.
const KEYMAP_FORMAT_XKB_V1: u32 = 1;
const KEY_RELEASED: u32 = 0;
const KEY_PRESSED: u32 = 1;

/// Modifier keys: keysym, the real modifier it is mapped to, and that
/// modifier's bit in the state sent to the compositor (real modifiers are
/// always the keymap's first eight: Shift, Lock, Control, Mod1..Mod5).
const SHIFT: (u32, &str, u32) = (0xffe1, "Shift", 1 << 0);
const CONTROL: (u32, &str, u32) = (0xffe3, "Control", 1 << 2);
const ALT: (u32, &str, u32) = (0xffe9, "Mod1", 1 << 3);
const SUPER: (u32, &str, u32) = (0xffeb, "Mod4", 1 << 6);
const MODIFIERS: [(u32, &str, u32); 4] = [SHIFT, CONTROL, ALT, SUPER];

/// A screen as the compositor lays it out.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// The connector name ("DP-1", "HEADLESS-1"), as the IPC names it.
    pub name: String,
    /// Where it is in the layout, in logical units (xdg-output's logical
    /// position and size).
    pub logical: Rect,
    /// Pixels per logical unit: its mode's size over its logical size (a
    /// rotated output's mode turned sideways), exact for fractional scales
    /// such as 1.25 or 1.5.
    pub scale: f64,
}

/// A connection to the Wayland compositor, with the globals it offers.
pub struct Wl {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    registry: wl_registry::WlRegistry,
    seat: Option<wl_seat::WlSeat>,
    shm: Option<wl_shm::WlShm>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    pointers: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    keyboards: Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,
    /// Created on first use: a new input device has side effects (sway
    /// shows a cursor for a new pointer).
    pointer: Option<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1>,
    keyboard: Option<Keyboard>,
    /// The idle notifications of `IDLE_RUNGS_MS` (empty without
    /// ext-idle-notify v2).
    idle_notes: Vec<ext_idle_notification_v1::ExtIdleNotificationV1>,
    /// How long a roundtrip may take (shorter in tests).
    roundtrip_time: Duration,
    next_sync: u64,
    next_frame: u32,
    /// The connection broke: the owner reconnects.
    lost: bool,
}

/// Everything the compositor's events update.
#[derive(Default)]
struct State {
    /// The globals offered: name -> (interface, version).
    globals: BTreeMap<u32, (String, u32)>,
    /// Bound outputs by global name.
    outputs: BTreeMap<u32, Out>,
    xdg_outputs: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    /// Screencopy frames in flight, by our id.
    frames: HashMap<u32, Frame>,
    /// The latest roundtrip the compositor answered.
    synced: u64,
    rungs: Vec<Rung>,
}

/// A bound output.
struct Out {
    wl: wl_output::WlOutput,
    xdg: Option<zxdg_output_v1::ZxdgOutputV1>,
    info: OutInfo,
}

/// An output as found now, for a screenshot.
struct Live {
    out: Output,
    wl: wl_output::WlOutput,
    transform: u32,
}

/// What the compositor said about an output.
#[derive(Debug, Default)]
struct OutInfo {
    /// wl_output.name (v4) and xdg_output.name (v2).
    wl_name: Option<String>,
    xdg_name: Option<String>,
    /// The current mode, in pixels, before the transform.
    mode: Option<(i32, i32)>,
    /// wl_output.transform (0..=7: normal, 90, 180, 270, then flipped).
    transform: u32,
    int_scale: i32,
    /// wl_output.geometry's position (only used without xdg-output).
    position: (i32, i32),
    logical_pos: Option<(i32, i32)>,
    logical_size: Option<(i32, i32)>,
    done: bool,
}

/// A shm buffer the compositor offers for a screencopy frame.
#[derive(Debug, Clone, Copy)]
struct ShmSpec {
    format: u32,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Debug, Default)]
struct Frame {
    shm: Option<ShmSpec>,
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

/// One ext-idle-notify notification: idle once `timeout` passed without
/// input.
#[derive(Debug, Clone)]
struct Rung {
    timeout: Duration,
    created: Instant,
    /// When its `idled` event was read (None: not idle, or not known yet).
    idled_at: Option<Instant>,
}

/// The virtual keyboard and the keymap the compositor has for it.
struct Keyboard {
    proxy: zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
    /// Keysyms by keycode: the one at index i is evdev keycode i + 1 (XKB
    /// keycode i + 9).
    syms: Vec<u32>,
    uploaded: bool,
}

impl Keyboard {
    fn code(&self, sym: u32) -> Option<u32> {
        self.syms
            .iter()
            .position(|s| *s == sym)
            .map(|i| i as u32 + 1)
    }
}

impl Out {
    fn destroy(&self) {
        if let Some(x) = &self.xdg {
            x.destroy();
        }
        if self.wl.version() >= 3 {
            self.wl.release();
        }
    }
}

impl OutInfo {
    /// Whether everything about it has arrived.
    fn ready(&self, xdg: bool) -> bool {
        self.done && (!xdg || self.logical_size.is_some())
    }

    /// The output as the layout has it; None until its size is known.
    fn describe(&self, global: u32) -> Option<Output> {
        let int_scale = self.int_scale.max(1);
        let physical = self.mode.map(|(w, h)| {
            // A rotated output shows its mode sideways.
            if self.transform % 2 == 1 {
                (h, w)
            } else {
                (w, h)
            }
        });
        let (pos, size) = match (self.logical_pos, self.logical_size) {
            (Some(p), Some(s)) => (p, s),
            // Without xdg-output: the integer scale is all there is.
            _ => {
                let (w, h) = physical?;
                (self.position, (w / int_scale, h / int_scale))
            }
        };
        if size.0 <= 0 || size.1 <= 0 {
            return None;
        }
        let scale = match physical {
            Some((w, h)) => output_scale(w, h, size.0, size.1),
            None => f64::from(int_scale),
        };
        let name = self
            .wl_name
            .clone()
            .or_else(|| self.xdg_name.clone())
            .unwrap_or_else(|| format!("output-{global}"));
        Some(Output {
            name,
            logical: Rect::new(
                f64::from(pos.0),
                f64::from(pos.1),
                f64::from(size.0),
                f64::from(size.1),
            ),
            scale,
        })
    }
}

impl Drop for Wl {
    /// Take the virtual devices away (the compositor also does when the
    /// connection closes).
    fn drop(&mut self) {
        for n in &self.idle_notes {
            n.destroy();
        }
        if let Some(p) = &self.pointer {
            p.destroy();
        }
        if let Some(k) = &self.keyboard {
            k.proxy.destroy();
        }
        let _ = self.conn.flush();
    }
}

impl Wl {
    /// Connect to the compositor at $WAYLAND_DISPLAY and bind what it
    /// offers of: the seat, shared memory, the outputs with xdg-output,
    /// wlr-screencopy, wlr-virtual-pointer, virtual-keyboard and
    /// ext-idle-notify. A missing protocol isn't an error here: the methods
    /// that need it fail with `Error::Unsupported` saying so (see
    /// `can_capture`, `can_point`, `can_type`). A compositor that doesn't
    /// answer within 2 s is an error, not a hang.
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().map_err(|e| {
            let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
            Error::Platform(format!(
                "cannot connect to the Wayland compositor (WAYLAND_DISPLAY={display:?}): {e}"
            ))
        })?;
        Self::with_connection(conn, ROUNDTRIP)
    }

    fn with_connection(conn: Connection, roundtrip_time: Duration) -> Result<Self> {
        let queue = conn.new_event_queue();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut wl = Self {
            conn,
            queue,
            qh,
            state: State::default(),
            registry,
            seat: None,
            shm: None,
            screencopy: None,
            pointers: None,
            keyboards: None,
            pointer: None,
            keyboard: None,
            idle_notes: Vec::new(),
            roundtrip_time,
            next_sync: 0,
            next_frame: 0,
            lost: false,
        };
        // The globals (outputs are bound as they are announced).
        wl.roundtrip()?;
        wl.seat = wl.bind(1);
        wl.shm = wl.bind(1);
        wl.state.xdg_outputs = wl.bind(3);
        if let Some(m) = &wl.state.xdg_outputs {
            for (global, o) in wl.state.outputs.iter_mut() {
                o.xdg = Some(m.get_xdg_output(&o.wl, &wl.qh, *global));
            }
        }
        wl.screencopy = wl.bind(3);
        wl.pointers = wl.bind(2);
        wl.keyboards = wl.bind(1);
        // Only v2 has input idle notifications, which idle inhibitors (a
        // playing video) don't hold back.
        if let (Some(n), Some(seat)) = (
            wl.bind::<ext_idle_notifier_v1::ExtIdleNotifierV1>(2),
            wl.seat.clone(),
        ) {
            if n.version() >= 2 {
                let now = Instant::now();
                for (i, ms) in IDLE_RUNGS_MS.iter().enumerate() {
                    wl.idle_notes
                        .push(n.get_input_idle_notification(*ms, &seat, &wl.qh, i));
                    wl.state.rungs.push(Rung {
                        timeout: Duration::from_millis(u64::from(*ms)),
                        created: now,
                        idled_at: None,
                    });
                }
            }
            // The notifications stay valid without their notifier.
            n.destroy();
        }
        // The outputs' details.
        wl.roundtrip()?;
        Ok(wl)
    }

    /// Bind the first global of interface `I`, at most at `max` version.
    fn bind<I>(&self, max: u32) -> Option<I>
    where
        I: Proxy + 'static,
        State: Dispatch<I, ()>,
    {
        let iface = I::interface();
        let (name, version) = self
            .state
            .globals
            .iter()
            .find(|(_, (i, _))| i == iface.name)
            .map(|(n, (_, v))| (*n, *v))?;
        let version = version.min(max).min(iface.version);
        Some(
            self.registry
                .bind::<I, (), State>(name, version, &self.qh, ()),
        )
    }

    /// Whether the connection broke (the compositor went away or ended it);
    /// every call fails from then on, and the owner connects again.
    pub fn lost(&self) -> bool {
        self.lost
    }

    /// What the compositor offers: (interface, version) of each global.
    /// For logs and diagnostics.
    pub fn protocols(&self) -> Vec<(String, u32)> {
        self.state.globals.values().cloned().collect()
    }

    /// The outputs as they are now: what the compositor said since the last
    /// call (a monitor plugged in or out, a new scale) is taken in first.
    /// Empty if the connection is lost.
    pub fn outputs(&mut self) -> Vec<Output> {
        self.current_outputs().into_iter().map(|l| l.out).collect()
    }

    /// The bounding box of the outputs' logical rects: the space pointer
    /// positions and screenshot regions are given in.
    pub fn layout(&mut self) -> Rect {
        union(self.outputs().iter().map(|o| o.logical))
    }

    /// Whether screenshots can be taken (wlr-screencopy).
    pub fn can_capture(&self) -> bool {
        self.screencopy.is_some() && self.shm.is_some()
    }

    /// Whether mouse input can be sent (wlr-virtual-pointer).
    pub fn can_point(&self) -> bool {
        self.pointers.is_some()
    }

    /// Whether keys can be sent (virtual-keyboard).
    pub fn can_type(&self) -> bool {
        self.keyboards.is_some() && self.seat.is_some()
    }

    /// The outputs as they are now: pending events (hotplug, a new scale)
    /// handled first, and a new output's details waited for.
    fn current_outputs(&mut self) -> Vec<Live> {
        if self.pump().is_err() {
            return Vec::new();
        }
        let xdg = self.state.xdg_outputs.is_some();
        if self.state.outputs.values().any(|o| !o.info.ready(xdg)) {
            let _ = self.roundtrip();
        }
        self.state
            .outputs
            .iter()
            .filter_map(|(g, o)| {
                o.info.describe(*g).map(|out| Live {
                    out,
                    wl: o.wl.clone(),
                    transform: o.info.transform,
                })
            })
            .collect()
    }

    // --- event dispatching -------------------------------------------------

    /// Fail if the connection broke earlier.
    fn alive(&self) -> Result<()> {
        if self.lost {
            return Err(Error::Platform(
                "the connection to the Wayland compositor was lost; it is made again on the next action".into(),
            ));
        }
        Ok(())
    }

    /// Note a broken connection; the error to return.
    fn broke(&mut self, what: impl std::fmt::Display) -> Error {
        self.lost = true;
        let why = match self.conn.protocol_error() {
            Some(p) => format!(
                "the compositor ended the connection over a protocol error ({} error {}: {})",
                p.object_interface, p.code, p.message
            ),
            None => format!("{what}"),
        };
        log::warn!("lost the connection to the Wayland compositor: {why}");
        Error::Platform(format!(
            "lost the connection to the Wayland compositor: {why}"
        ))
    }

    /// Handle events until `done` says so or `deadline` passes (then
    /// `Ok(false)`). Never blocks past the deadline: the socket is polled
    /// with a timeout instead of read blocking.
    fn wait_until(&mut self, deadline: Instant, done: impl Fn(&State) -> bool) -> Result<bool> {
        self.alive()?;
        loop {
            if let Err(e) = self.queue.dispatch_pending(&mut self.state) {
                return Err(self.broke(e));
            }
            if done(&self.state) {
                return Ok(true);
            }
            self.flush()?;
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let left = deadline.saturating_duration_since(Instant::now());
            let readable = match poll_fd(guard.connection_fd().as_raw_fd(), libc::POLLIN, left) {
                Ok(r) => r,
                Err(e) => {
                    drop(guard);
                    return Err(self.broke(e));
                }
            };
            if readable {
                match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => return Err(self.broke(e)),
                }
            } else {
                drop(guard);
            }
            if !readable || Instant::now() >= deadline {
                if let Err(e) = self.queue.dispatch_pending(&mut self.state) {
                    return Err(self.broke(e));
                }
                return Ok(done(&self.state));
            }
        }
    }

    /// Handle what has arrived, without waiting.
    fn pump(&mut self) -> Result<()> {
        self.wait_until(Instant::now(), |_| false).map(|_| ())
    }

    /// Send what is queued. A full socket (the compositor is behind) is
    /// waited on, for a bounded time.
    fn flush(&mut self) -> Result<()> {
        let deadline = Instant::now() + self.roundtrip_time;
        loop {
            match self.conn.flush() {
                Ok(()) => return Ok(()),
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    let backend = self.conn.backend();
                    let fd = backend.poll_fd().as_raw_fd();
                    let left = deadline.saturating_duration_since(Instant::now());
                    match poll_fd(fd, libc::POLLOUT, left) {
                        Ok(true) => {}
                        Ok(false) => {
                            return Err(Error::Platform(format!(
                                "the Wayland compositor stopped reading requests for {} s",
                                self.roundtrip_time.as_secs_f64()
                            )));
                        }
                        Err(e) => return Err(self.broke(e)),
                    }
                }
                Err(e) => return Err(self.broke(e)),
            }
        }
    }

    /// Wait until the compositor has handled everything sent so far (and
    /// handle its answers), for a bounded time.
    fn roundtrip(&mut self) -> Result<()> {
        self.alive()?;
        self.next_sync += 1;
        let id = self.next_sync;
        self.conn.display().sync(&self.qh, id);
        let deadline = Instant::now() + self.roundtrip_time;
        if self.wait_until(deadline, |s| s.synced >= id)? {
            Ok(())
        } else {
            Err(Error::Platform(format!(
                "the Wayland compositor didn't answer within {} s",
                self.roundtrip_time.as_secs_f64()
            )))
        }
    }

    // --- screenshots -------------------------------------------------------

    /// A screenshot of `region` (layout coordinates, edges rounded to whole
    /// units), without the mouse cursor, through wlr-screencopy. It may span
    /// outputs; what no output shows is black. `bounds` is the region cut
    /// to the layout. The image has the pixel density of the highest-scaled
    /// output it touches (the parts from lower-scaled ones are enlarged to
    /// match), so `width / bounds.width` is that scale. Where a fractional
    /// scale puts the region's edges between pixels, the compositor rounds
    /// them, so the image can be up to a pixel off. Every output's part is
    /// copied at once; a copy that fails or isn't done within 3 s is an
    /// error.
    pub fn capture(&mut self, region: Rect) -> Result<Capture> {
        self.alive()?;
        let (Some(manager), Some(shm)) = (self.screencopy.clone(), self.shm.clone()) else {
            return Err(Error::Unsupported(
                "this compositor doesn't offer wlr-screencopy (zwlr_screencopy_manager_v1 with wl_shm), so screenshots of native Wayland windows can't be taken here. Hyprland, sway and other wlroots-based compositors offer it; GNOME and KDE don't.".into(),
            ));
        };
        let outputs = self.current_outputs();
        if outputs.is_empty() {
            return Err(Error::Platform(
                "the Wayland compositor reports no outputs (screens) to take a screenshot of"
                    .into(),
            ));
        }
        let layout = union(outputs.iter().map(|l| l.out.logical));
        let Some(bounds) = clip(region, layout) else {
            return Err(Error::InvalidArgs(format!(
                "the area ({:.0}, {:.0}) {:.0}x{:.0} is outside the screen ({:.0}, {:.0}) {:.0}x{:.0}",
                region.x,
                region.y,
                region.width,
                region.height,
                layout.x,
                layout.y,
                layout.width,
                layout.height
            )));
        };
        let mut pieces: Vec<Piece> = Vec::new();
        for Live { out, wl, transform } in outputs {
            let Some(sub) = clip(bounds, out.logical) else {
                continue;
            };
            self.next_frame = self.next_frame.wrapping_add(1);
            let id = self.next_frame;
            let frame = manager.capture_output_region(
                0,
                &wl,
                (sub.x - out.logical.x) as i32,
                (sub.y - out.logical.y) as i32,
                sub.width as i32,
                sub.height as i32,
                &self.qh,
                id,
            );
            self.state.frames.insert(id, Frame::default());
            pieces.push(Piece {
                id,
                frame,
                sub,
                out,
                transform,
                buffer: None,
            });
        }
        let density = pieces.iter().map(|p| p.out.scale).fold(0.0, f64::max);
        let density = if density > 0.0 { density } else { 1.0 };
        let shot = self.copy_pieces(&mut pieces, &shm, manager.version(), bounds, density);
        for p in pieces {
            p.frame.destroy();
            if let Some(b) = p.buffer {
                b.buffer.destroy();
                b.pool.destroy();
            }
            self.state.frames.remove(&p.id);
        }
        let _ = self.flush();
        shot
    }

    /// Copy every piece's output region and put them together.
    fn copy_pieces(
        &mut self,
        pieces: &mut [Piece],
        shm: &wl_shm::WlShm,
        version: u32,
        bounds: Rect,
        density: f64,
    ) -> Result<Capture> {
        let deadline = Instant::now() + CAPTURE_TIME;
        let ids: Vec<u32> = pieces.iter().map(|p| p.id).collect();
        // The buffer each frame wants (v3 lists every kind, then says done).
        let described = |s: &State| {
            ids.iter().all(|id| {
                s.frames.get(id).is_none_or(|f| {
                    f.failed || (f.shm.is_some() && (version < 3 || f.buffer_done)) || f.buffer_done
                })
            })
        };
        if !self.wait_until(deadline, described)? {
            return Err(Error::Platform(format!(
                "the compositor didn't start the screenshot within {} s",
                CAPTURE_TIME.as_secs()
            )));
        }
        for p in pieces.iter_mut() {
            let f = &self.state.frames[&p.id];
            if f.failed {
                return Err(copy_failed(&p.out.name));
            }
            let Some(spec) = f.shm else {
                return Err(Error::Unsupported(format!(
                    "the compositor offers screenshots of {} only in GPU buffers (dmabuf), not shared memory",
                    p.out.name
                )));
            };
            p.buffer = Some(ShmBuffer::new(shm, spec, &self.qh)?);
            if let Some(b) = &p.buffer {
                p.frame.copy(&b.buffer);
            }
        }
        let copied = |s: &State| {
            ids.iter()
                .all(|id| s.frames.get(id).is_none_or(|f| f.ready || f.failed))
        };
        if !self.wait_until(deadline, copied)? {
            return Err(Error::Platform(format!(
                "the compositor didn't finish the screenshot within {} s",
                CAPTURE_TIME.as_secs()
            )));
        }
        let width = (bounds.width * density).round().max(1.0) as u32;
        let height = (bounds.height * density).round().max(1.0) as u32;
        let mut img = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 255]));
        for p in pieces.iter() {
            let f = &self.state.frames[&p.id];
            if f.failed || !f.ready {
                return Err(copy_failed(&p.out.name));
            }
            let Some(b) = &p.buffer else { continue };
            let piece = to_rgba(&b.map, b.spec, f.y_invert).ok_or_else(|| {
                Error::Unsupported(format!(
                    "the compositor gives screenshots of {} in pixel format {}, which isn't handled",
                    p.out.name,
                    format_name(b.spec.format)
                ))
            })?;
            let piece = untransform(piece, p.transform);
            let x0 = ((p.sub.x - bounds.x) * density).round() as i64;
            let y0 = ((p.sub.y - bounds.y) * density).round() as i64;
            let x1 = ((p.sub.x + p.sub.width - bounds.x) * density).round() as i64;
            let y1 = ((p.sub.y + p.sub.height - bounds.y) * density).round() as i64;
            place(&mut img, &piece, x0, y0, x1 - x0, y1 - y0);
        }
        Ok(Capture {
            width,
            height,
            rgba: img.into_raw(),
            bounds,
        })
    }

    // --- pointer -----------------------------------------------------------

    /// The virtual pointer (made on first use) and the layout it moves over.
    fn pointer(&mut self) -> Result<(zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1, Rect)> {
        self.alive()?;
        let Some(manager) = self.pointers.clone() else {
            return Err(Error::Unsupported(
                "this compositor doesn't offer wlr-virtual-pointer (zwlr_virtual_pointer_manager_v1), so mouse input can't be sent to native Wayland windows here; use the accessibility actions (element_index) instead. Hyprland, sway and other wlroots-based compositors offer it; GNOME and KDE don't.".into(),
            ));
        };
        let area = self.layout();
        if area.is_empty() {
            return Err(Error::Platform(
                "the Wayland compositor reports no outputs (screens) to point at".into(),
            ));
        }
        if self.pointer.is_none() {
            self.pointer = Some(manager.create_virtual_pointer(self.seat.as_ref(), &self.qh, ()));
            self.roundtrip()?;
            std::thread::sleep(NEW_DEVICE_SETTLE);
        }
        let p = self
            .pointer
            .clone()
            .ok_or_else(|| Error::Platform("the virtual pointer is gone".into()))?;
        Ok((p, area))
    }

    /// Move the pointer to (x, y) in the layout. The pointer methods use a
    /// virtual pointer (made on first use) moved by absolute positions over
    /// the layout, and return once the compositor has handled the events.
    /// The pointer stays where they leave it: Wayland gives clients no way
    /// to read the user's pointer position, so it can't be put back.
    pub fn move_pointer(&mut self, x: f64, y: f64) -> Result<()> {
        let (p, area) = self.pointer()?;
        motion(&p, area, x, y);
        self.roundtrip()
    }

    /// Click `button` (1 left, 2 middle, 3 right, 8 back, 9 forward)
    /// `count` times at (x, y). The clicks share one moment, so apps count
    /// them as a double or triple click.
    pub fn click(&mut self, x: f64, y: f64, button: u8, count: u8) -> Result<()> {
        let code = button_code(button)?;
        let (p, area) = self.pointer()?;
        motion(&p, area, x, y);
        for _ in 0..count.max(1) {
            press_button(&p, code, true);
            press_button(&p, code, false);
        }
        self.roundtrip()
    }

    /// Drag with the left button from `from` to `to`, paced like a hand
    /// would (steps 12 ms apart), so that apps see a drag; the button is
    /// let go even if sending fails midway.
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64)) -> Result<()> {
        let (p, area) = self.pointer()?;
        let mut held = false;
        let dragged = self.drag_steps(&p, area, from, to, &mut held);
        if held {
            // Never leave the button held down.
            press_button(&p, BTN_LEFT, false);
        }
        let done = self.roundtrip();
        dragged.and(done)
    }

    fn drag_steps(
        &mut self,
        p: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
        area: Rect,
        from: (f64, f64),
        to: (f64, f64),
        held: &mut bool,
    ) -> Result<()> {
        motion(p, area, from.0, from.1);
        self.pause(DRAG_STEP)?;
        press_button(p, BTN_LEFT, true);
        *held = true;
        self.pause(DRAG_STEP)?;
        // A few intermediate motions so drag-aware widgets follow.
        for step in 1..=8 {
            let t = f64::from(step) / 8.0;
            motion(
                p,
                area,
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
            );
            self.pause(DRAG_STEP)?;
        }
        press_button(p, BTN_LEFT, false);
        *held = false;
        self.pause(DRAG_STEP)
    }

    /// Send what is queued, then wait `d`.
    fn pause(&mut self, d: Duration) -> Result<()> {
        self.flush()?;
        std::thread::sleep(d);
        Ok(())
    }

    /// Draw `strokes` with `button` held, as the X11 path does: `pace` is
    /// called with the distance before each move (0 before a stroke starts)
    /// and may stop the drawing with an error; the button is never left
    /// held down.
    pub fn draw(
        &mut self,
        strokes: &[Vec<(f64, f64)>],
        button: u8,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        let code = button_code(button)?;
        let (p, area) = self.pointer()?;
        let mut held = false;
        let drawn = self.draw_strokes(&p, area, strokes, code, pace, &mut held);
        if held {
            // Never leave the button held down.
            press_button(&p, code, false);
        }
        let done = self.roundtrip();
        drawn.and(done)
    }

    fn draw_strokes(
        &mut self,
        p: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
        area: Rect,
        strokes: &[Vec<(f64, f64)>],
        code: u32,
        pace: &mut dyn FnMut(f64) -> Result<()>,
        held: &mut bool,
    ) -> Result<()> {
        for stroke in strokes {
            let Some(&(x, y)) = stroke.first() else {
                continue;
            };
            pace(0.0)?;
            motion(p, area, x, y);
            self.pause(DRAG_STEP)?;
            press_button(p, code, true);
            *held = true;
            self.pause(DRAG_STEP)?;
            let mut last = (x, y);
            for &(px, py) in &stroke[1..] {
                pace((px - last.0).hypot(py - last.1))?;
                if (px, py) == last {
                    continue;
                }
                motion(p, area, px, py);
                self.flush()?;
                last = (px, py);
            }
            std::thread::sleep(DRAG_STEP);
            press_button(p, code, false);
            *held = false;
            self.flush()?;
        }
        Ok(())
    }

    /// Scroll at (x, y) by `dx`, `dy` wheel clicks (positive: right, down),
    /// sent as a mouse wheel's clicks (apps scroll by their step per click).
    pub fn scroll(&mut self, x: f64, y: f64, dx: i32, dy: i32) -> Result<()> {
        let (p, area) = self.pointer()?;
        motion(&p, area, x, y);
        for (axis, n) in [
            (wl_pointer::Axis::VerticalScroll, dy),
            (wl_pointer::Axis::HorizontalScroll, dx),
        ] {
            for _ in 0..n.unsigned_abs() {
                p.axis_source(wl_pointer::AxisSource::Wheel);
                p.axis_discrete(
                    now_ms(),
                    axis,
                    WHEEL_STEP * f64::from(n.signum()),
                    n.signum(),
                );
                p.frame();
                self.pause(WHEEL_PAUSE)?;
            }
        }
        self.roundtrip()
    }

    // --- keyboard ----------------------------------------------------------

    /// Make the virtual keyboard (on first use).
    fn keyboard(&mut self) -> Result<()> {
        self.alive()?;
        if self.keyboard.is_some() {
            return Ok(());
        }
        let (Some(manager), Some(seat)) = (self.keyboards.clone(), self.seat.clone()) else {
            return Err(Error::Unsupported(
                "this compositor doesn't offer virtual-keyboard (zwp_virtual_keyboard_manager_v1 and a wl_seat), so keys can't be sent to native Wayland windows here; use set_value to enter text instead. Hyprland, sway and other wlroots-based compositors offer it; GNOME and KDE don't.".into(),
            ));
        };
        let proxy = manager.create_virtual_keyboard(&seat, &self.qh, ());
        self.keyboard = Some(Keyboard {
            proxy,
            syms: Vec::new(),
            uploaded: false,
        });
        // The keymap goes with the keyboard, before any key.
        self.ensure_keys(&[])?;
        std::thread::sleep(NEW_DEVICE_SETTLE);
        Ok(())
    }

    /// Make sure the compositor's keymap for the virtual keyboard has
    /// `needed` (keysyms). It only grows, so keycodes keep their keysyms
    /// and keys already sent can't change meaning; past `KEYMAP_CAP` keys
    /// it starts again from the base keys plus `needed`.
    fn ensure_keys(&mut self, needed: &[u32]) -> Result<()> {
        let Some(kb) = self.keyboard.as_ref() else {
            return Err(Error::Platform("no virtual keyboard".into()));
        };
        let mut missing: Vec<u32> = Vec::new();
        for s in needed {
            if !kb.syms.contains(s) && !missing.contains(s) {
                missing.push(*s);
            }
        }
        if kb.uploaded && missing.is_empty() {
            return Ok(());
        }
        let grown = if kb.syms.is_empty() {
            base_keys()
        } else {
            kb.syms.clone()
        };
        let syms = if grown.len() + missing.len() <= KEYMAP_CAP {
            let mut s = grown;
            for m in missing {
                if !s.contains(&m) {
                    s.push(m);
                }
            }
            s
        } else {
            let mut s = base_keys();
            for n in needed {
                if !s.contains(n) {
                    s.push(*n);
                }
            }
            s
        };
        if syms.len() > KEYMAP_CAP {
            return Err(Error::ActionFailed(format!(
                "too many different characters at once ({} keys, at most {KEYMAP_CAP})",
                syms.len()
            )));
        }
        let rebuilt = kb.uploaded && !syms.starts_with(&kb.syms);
        if rebuilt {
            // Keycodes change meaning: let apps finish with the old ones.
            self.roundtrip()?;
            std::thread::sleep(REMAP_SETTLE);
        }
        let text = keymap_text(&syms);
        let fd = memfd(c"cu-keymap")?;
        let mut file = File::from(fd);
        file.write_all(text.as_bytes())
            .and_then(|()| file.write_all(&[0]))
            .map_err(|e| Error::Platform(format!("cannot write the keymap: {e}")))?;
        let Some(kb) = self.keyboard.as_mut() else {
            return Err(Error::Platform("no virtual keyboard".into()));
        };
        kb.proxy
            .keymap(KEYMAP_FORMAT_XKB_V1, file.as_fd(), text.len() as u32 + 1);
        kb.syms = syms;
        kb.uploaded = true;
        self.roundtrip()
    }

    /// Press a key combination: its modifiers go down (as keys and as
    /// modifier state), the key is pressed and released, the modifiers come
    /// up. A character key means that character whatever the user's layout.
    /// It goes to the window with the keyboard focus, like a real
    /// keyboard's, and the compositor's own shortcuts see it first.
    pub fn press(&mut self, combo: &KeyCombo) -> Result<()> {
        let sym = keysym(combo.key)
            .ok_or_else(|| Error::InvalidArgs(format!("there is no key for {combo}")))?;
        self.keyboard()?;
        self.ensure_keys(&[sym])?;
        let Some(kb) = self.keyboard.as_ref() else {
            return Err(Error::Platform("no virtual keyboard".into()));
        };
        let m = combo.modifiers;
        let mask: u32 = MODIFIERS
            .iter()
            .zip([m.shift, m.ctrl, m.alt, m.meta])
            .filter(|(_, on)| *on)
            .map(|((_, _, bit), _)| *bit)
            .sum();
        let code = kb
            .code(sym)
            .ok_or_else(|| Error::Platform("the key is missing from the keymap".into()))?;
        // Everything, the release too, is queued before anything can fail.
        let t = now_ms();
        kb.proxy.modifiers(mask, 0, 0, 0);
        kb.proxy.key(t, code, KEY_PRESSED);
        kb.proxy.key(t, code, KEY_RELEASED);
        kb.proxy.modifiers(0, 0, 0, 0);
        self.roundtrip()
    }

    /// Type `text` exactly, any script or emoji, whatever keyboard layout
    /// the user has: the virtual keyboard's keymap gives each character a
    /// key of its own (as `wtype` does). "\n" presses Return and "\t" Tab;
    /// other control characters are refused. It goes to the window with the
    /// keyboard focus.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        let syms = text
            .chars()
            .map(|c| {
                keysym(Key::Char(c)).ok_or_else(|| {
                    Error::InvalidArgs(format!(
                        "can't type the control character U+{:04X}",
                        c as u32
                    ))
                })
            })
            .collect::<Result<Vec<u32>>>()?;
        self.keyboard()?;
        let base = base_keys();
        let mut rest = &syms[..];
        while !rest.is_empty() {
            let n = chunk_len(rest, &base, KEYMAP_CAP - base.len());
            let (chunk, tail) = rest.split_at(n);
            self.ensure_keys(chunk)?;
            let Some(kb) = self.keyboard.as_ref() else {
                return Err(Error::Platform("no virtual keyboard".into()));
            };
            let proxy = kb.proxy.clone();
            let codes: Vec<u32> = chunk.iter().filter_map(|s| kb.code(*s)).collect();
            proxy.modifiers(0, 0, 0, 0);
            for code in codes {
                let t = now_ms();
                proxy.key(t, code, KEY_PRESSED);
                proxy.key(t, code, KEY_RELEASED);
                self.pause(TYPE_PAUSE)?;
            }
            rest = tail;
        }
        self.roundtrip()
    }

    // --- idle --------------------------------------------------------------

    /// How long ago anyone last used the mouse or keyboard (the virtual
    /// devices count too), from ext-idle-notify's input idle notifications
    /// (version 2). None without them: version 1 only has notifications
    /// that idle inhibitors hold back, so a playing video would look like a
    /// user who never stops.
    ///
    /// Several notifications with longer and longer timeouts are used, so
    /// an `idled` read late (between actions) still gives the right order
    /// of magnitude. Right after connecting it waits up to the shortest
    /// timeout to learn whether the user is idle at all; longer idle times
    /// are only known once their timeout has passed since connecting.
    pub fn idle(&mut self) -> Option<Duration> {
        let first = self.state.rungs.first()?;
        let settled = first.created + first.timeout + IDLE_SLACK;
        let deadline = settled.max(Instant::now());
        self.wait_until(deadline, |_| false).ok()?;
        Some(idle_estimate(&self.state.rungs, Instant::now()))
    }
}

/// A screenshot of one output's part of the region.
struct Piece {
    id: u32,
    frame: zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    /// The part, in logical layout coordinates.
    sub: Rect,
    out: Output,
    transform: u32,
    buffer: Option<ShmBuffer>,
}

/// A shared-memory buffer for the compositor to copy into.
struct ShmBuffer {
    spec: ShmSpec,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    map: memmap2::Mmap,
}

impl ShmBuffer {
    fn new(shm: &wl_shm::WlShm, spec: ShmSpec, qh: &QueueHandle<State>) -> Result<Self> {
        let size = u64::from(spec.stride) * u64::from(spec.height);
        let format = wl_shm::Format::try_from(spec.format).map_err(|()| {
            Error::Unsupported(format!(
                "the compositor wants screenshots in an unknown pixel format ({})",
                format_name(spec.format)
            ))
        })?;
        if size == 0 || size > i32::MAX as u64 || spec.width > i32::MAX as u32 {
            return Err(Error::Platform(format!(
                "the compositor asked for a {}x{} screenshot buffer",
                spec.width, spec.height
            )));
        }
        let file = File::from(memfd(c"cu-screencopy")?);
        file.set_len(size)
            .map_err(|e| Error::Platform(format!("cannot size the screenshot buffer: {e}")))?;
        // SAFETY: the file is ours alone (a fresh memfd) and stays this size.
        let map = unsafe { memmap2::Mmap::map(&file) }
            .map_err(|e| Error::Platform(format!("cannot map the screenshot buffer: {e}")))?;
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            spec.width as i32,
            spec.height as i32,
            spec.stride as i32,
            format,
            qh,
            (),
        );
        Ok(Self {
            spec,
            pool,
            buffer,
            map,
        })
    }
}

fn copy_failed(output: &str) -> Error {
    Error::Platform(format!(
        "the compositor failed to copy the screen of output {output} (it may be off, or screenshots may need permission: on Hyprland with ecosystem:enforce_permissions, allow screencopy for this program)"
    ))
}

/// A memfd (an anonymous file) to hand the compositor.
fn memfd(name: &std::ffi::CStr) -> Result<OwnedFd> {
    // SAFETY: a valid C string; the returned descriptor is checked.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(Error::Platform(format!(
            "cannot create shared memory for the compositor: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: a fresh descriptor nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Wait up to `timeout` for `events` on `fd`; whether they came.
fn poll_fd(fd: RawFd, events: i16, timeout: Duration) -> std::io::Result<bool> {
    let ms = i32::try_from(timeout.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
    let mut pfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    loop {
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut pfd, 1, ms) };
        if r >= 0 {
            return Ok(r > 0);
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Input event time: CLOCK_MONOTONIC in milliseconds, the clock real
/// devices' events use (apps compare them, e.g. for double clicks).
fn now_ms() -> u32 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid timespec to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000) as u32
}

fn button_code(button: u8) -> Result<u32> {
    Ok(match button {
        1 => BTN_LEFT,
        2 => BTN_MIDDLE,
        3 => BTN_RIGHT,
        8 => BTN_SIDE,
        9 => BTN_EXTRA,
        b => {
            return Err(Error::InvalidArgs(format!(
                "mouse button {b} doesn't exist (1 left, 2 middle, 3 right, 8 back, 9 forward)"
            )));
        }
    })
}

/// Move the pointer to (x, y) in the layout `area`. The compositor maps
/// x / x_extent over the layout's bounding box (wlroots and Hyprland both).
fn motion(p: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1, area: Rect, x: f64, y: f64) {
    let ex = (area.width * POINTER_PRECISION).round().max(1.0);
    let ey = (area.height * POINTER_PRECISION).round().max(1.0);
    let px = ((x - area.x) * POINTER_PRECISION)
        .round()
        .clamp(0.0, ex - 1.0);
    let py = ((y - area.y) * POINTER_PRECISION)
        .round()
        .clamp(0.0, ey - 1.0);
    p.motion_absolute(now_ms(), px as u32, py as u32, ex as u32, ey as u32);
    p.frame();
}

fn press_button(p: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1, code: u32, down: bool) {
    let state = if down {
        wl_pointer::ButtonState::Pressed
    } else {
        wl_pointer::ButtonState::Released
    };
    p.button(now_ms(), code, state);
    p.frame();
}

/// The bounding box of `rects` (empty at the origin if there are none).
fn union(rects: impl Iterator<Item = Rect>) -> Rect {
    let mut b: Option<(f64, f64, f64, f64)> = None;
    for r in rects.filter(|r| !r.is_empty()) {
        let (x1, y1) = (r.x + r.width, r.y + r.height);
        b = Some(match b {
            None => (r.x, r.y, x1, y1),
            Some((a, c, d, e)) => (a.min(r.x), c.min(r.y), d.max(x1), e.max(y1)),
        });
    }
    b.map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |(x0, y0, x1, y1)| {
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    })
}

/// `r` cut to `to`, its edges rounded to whole logical units; None if
/// nothing is left.
fn clip(r: Rect, to: Rect) -> Option<Rect> {
    let x0 = r.x.round().max(to.x);
    let y0 = r.y.round().max(to.y);
    let x1 = (r.x + r.width).round().min(to.x + to.width);
    let y1 = (r.y + r.height).round().min(to.y + to.height);
    (x1 - x0 >= 1.0 && y1 - y0 >= 1.0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Pixels per logical unit of an output `pw`x`ph` pixels (after its
/// transform) shown as `lw`x`lh` logical units. The logical size is the
/// pixel size divided by the scale and rounded or cut to whole units, so
/// the plain ratio is a little off for fractional scales (1280 / 853 =
/// 1.5006 for 1.5); compositors' scales are multiples of 1/120 (as
/// wp-fractional-scale sends them), so the one the rounding allows is taken.
fn output_scale(pw: i32, ph: i32, lw: i32, lh: i32) -> f64 {
    let (pw, ph, lw, lh) = (f64::from(pw), f64::from(ph), f64::from(lw), f64::from(lh));
    let raw = pw / lw;
    // The scales that round to these logical sizes, a unit either way.
    let lo = (pw / (lw + 1.0)).max(ph / (lh + 1.0));
    let hi = (pw / (lw - 1.0).max(0.5)).min(ph / (lh - 1.0).max(0.5));
    let snapped = (raw * 120.0).round() / 120.0;
    if snapped > lo && snapped < hi {
        snapped
    } else {
        raw
    }
}

/// DRM fourcc code.
const fn fourcc(c: &[u8; 4]) -> u32 {
    (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16) | ((c[3] as u32) << 24)
}

/// wl_shm formats (ARGB8888 and XRGB8888 have codes of their own; the
/// others are DRM fourcc codes).
const ARGB8888: u32 = 0;
const XRGB8888: u32 = 1;
const XRGB8888_CC: u32 = fourcc(b"XR24");
const ARGB8888_CC: u32 = fourcc(b"AR24");
const XBGR8888: u32 = fourcc(b"XB24");
const ABGR8888: u32 = fourcc(b"AB24");
const RGBX8888: u32 = fourcc(b"RX24");
const RGBA8888: u32 = fourcc(b"RA24");
const BGRX8888: u32 = fourcc(b"BX24");
const BGRA8888: u32 = fourcc(b"BA24");
const RGB888: u32 = fourcc(b"RG24");
const BGR888: u32 = fourcc(b"BG24");
const XRGB2101010: u32 = fourcc(b"XR30");
const ARGB2101010: u32 = fourcc(b"AR30");
const XBGR2101010: u32 = fourcc(b"XB30");
const ABGR2101010: u32 = fourcc(b"AB30");

/// A format code as its fourcc letters, for messages.
fn format_name(format: u32) -> String {
    match format {
        ARGB8888 => "ARGB8888".into(),
        XRGB8888 => "XRGB8888".into(),
        f => {
            let s: String = f.to_le_bytes().iter().map(|b| *b as char).collect();
            if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                s
            } else {
                format!("{f:#x}")
            }
        }
    }
}

/// Convert a screencopy buffer to RGBA (opaque), upright. None for a pixel
/// format not handled. DRM formats name the channels of a little-endian
/// word from the high bits down (XRGB8888: bytes B, G, R, X in memory).
fn to_rgba(data: &[u8], spec: ShmSpec, y_invert: bool) -> Option<RgbaImage> {
    let (w, h, stride) = (
        spec.width as usize,
        spec.height as usize,
        spec.stride as usize,
    );
    // (bytes per pixel, how to read one)
    type Read = fn(&[u8]) -> [u8; 3];
    let (bpp, read): (usize, Read) = match spec.format {
        ARGB8888 | XRGB8888 | XRGB8888_CC | ARGB8888_CC => (4, |p| [p[2], p[1], p[0]]),
        XBGR8888 | ABGR8888 => (4, |p| [p[0], p[1], p[2]]),
        RGBX8888 | RGBA8888 => (4, |p| [p[3], p[2], p[1]]),
        BGRX8888 | BGRA8888 => (4, |p| [p[1], p[2], p[3]]),
        RGB888 => (3, |p| [p[2], p[1], p[0]]),
        BGR888 => (3, |p| [p[0], p[1], p[2]]),
        XRGB2101010 | ARGB2101010 => (4, |p| {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            [ten(v >> 20), ten(v >> 10), ten(v)]
        }),
        XBGR2101010 | ABGR2101010 => (4, |p| {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            [ten(v), ten(v >> 10), ten(v >> 20)]
        }),
        _ => return None,
    };
    if stride < w * bpp || data.len() < stride * h.saturating_sub(1) + w * bpp {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let src = if y_invert { h - 1 - row } else { row };
        let line = &data[src * stride..src * stride + w * bpp];
        for px in line.chunks_exact(bpp) {
            let [r, g, b] = read(px);
            out.extend_from_slice(&[r, g, b, 255]);
        }
    }
    RgbaImage::from_raw(spec.width, spec.height, out)
}

/// The top 8 of a 10-bit channel in the low bits of `v`.
fn ten(v: u32) -> u8 {
    ((v & 0x3ff) >> 2) as u8
}

/// Turn a buffer in the output's own orientation upright, undoing its
/// wl_output transform (a flip about the vertical axis, then a rotation
/// counter-clockwise; the buffer holds the picture as the panel scans it).
fn untransform(img: RgbaImage, transform: u32) -> RgbaImage {
    let rotated = match transform % 4 {
        1 => imageops::rotate90(&img),
        2 => imageops::rotate180(&img),
        3 => imageops::rotate270(&img),
        _ => img,
    };
    if transform >= 4 {
        imageops::flip_horizontal(&rotated)
    } else {
        rotated
    }
}

/// Put `piece` into `img` at (x, y), `w`x`h` there. A piece that is a
/// pixel or two off that size (outputs' scaled region edges round
/// differently) is copied with its last row and column repeated; one of a
/// lower-scale output is resized.
fn place(img: &mut RgbaImage, piece: &RgbaImage, x: i64, y: i64, w: i64, h: i64) {
    if w <= 0 || h <= 0 || piece.width() == 0 || piece.height() == 0 {
        return;
    }
    let close =
        (i64::from(piece.width()) - w).abs() <= 2 && (i64::from(piece.height()) - h).abs() <= 2;
    let resized;
    let src = if close {
        piece
    } else {
        resized = imageops::resize(piece, w as u32, h as u32, imageops::FilterType::Triangle);
        &resized
    };
    let (iw, ih) = (i64::from(img.width()), i64::from(img.height()));
    let (sw, sh) = (i64::from(src.width()), i64::from(src.height()));
    let stride = iw as usize * 4;
    let dst = img.as_mut();
    let srcb = src.as_raw();
    for row in 0..h {
        let dy = y + row;
        if dy < 0 || dy >= ih {
            continue;
        }
        let sy = row.min(sh - 1) as usize;
        let src_row = &srcb[sy * sw as usize * 4..(sy + 1) * sw as usize * 4];
        for col in 0..w {
            let dx = x + col;
            if dx < 0 || dx >= iw {
                continue;
            }
            let sx = col.min(sw - 1) as usize;
            let d = dy as usize * stride + dx as usize * 4;
            dst[d..d + 4].copy_from_slice(&src_row[sx * 4..sx * 4 + 4]);
        }
    }
}

/// The idle time the notifications say: the longest timeout reported idle
/// plus the time since that was read (exact when read at once; when it sat
/// unread, still at least that timeout), but under the shortest timeout
/// that has passed since it was created without the user going idle.
fn idle_estimate(rungs: &[Rung], now: Instant) -> Duration {
    let lower = rungs
        .iter()
        .filter_map(|r| {
            r.idled_at
                .map(|t| r.timeout + now.saturating_duration_since(t))
        })
        .max()
        .unwrap_or_default();
    let upper = rungs
        .iter()
        .filter(|r| r.idled_at.is_none() && now.saturating_duration_since(r.created) >= r.timeout)
        .map(|r| r.timeout)
        .min();
    upper.map_or(lower, |u| lower.min(u))
}

/// The keysym of a key: the same as the X11 path's (`x11::keysym_for`).
fn keysym(key: Key) -> Option<u32> {
    Some(match key {
        // Control characters have function-key keysyms, not Latin-1 ones.
        Key::Char('\t') => 0xff09,
        Key::Char('\n' | '\r') => 0xff0d,
        Key::Char('\u{8}') => 0xff08,
        Key::Char('\u{1b}') => 0xff1b,
        Key::Char('\u{7f}') => 0xffff,
        Key::Char(c) if (c as u32) < 0x20 || (0x80..0xa0).contains(&(c as u32)) => return None,
        Key::Char(c) => {
            let cp = c as u32;
            if cp <= 0xff {
                cp // Latin-1 keysyms equal the code point.
            } else {
                0x0100_0000 + cp // Unicode keysym.
            }
        }
        Key::Named(n) => match n {
            NamedKey::Return => 0xff0d,
            NamedKey::Tab => 0xff09,
            NamedKey::Space => 0x0020,
            NamedKey::Backspace => 0xff08,
            NamedKey::Delete => 0xffff,
            NamedKey::Escape => 0xff1b,
            NamedKey::Home => 0xff50,
            NamedKey::End => 0xff57,
            NamedKey::PageUp => 0xff55,
            NamedKey::PageDown => 0xff56,
            NamedKey::Left => 0xff51,
            NamedKey::Right => 0xff53,
            NamedKey::Up => 0xff52,
            NamedKey::Down => 0xff54,
            NamedKey::Insert => 0xff63,
            NamedKey::CapsLock => 0xffe5,
            NamedKey::Menu => 0xff67,
            NamedKey::F(n) if (1..=35).contains(&n) => 0xffbe + (u32::from(n) - 1),
            NamedKey::F(_) => return None,
            NamedKey::Numpad(p) => match p {
                Pad::Digit(d) if d <= 9 => 0xffb0 + u32::from(d),
                Pad::Digit(_) => return None,
                Pad::Decimal => 0xffae,
                Pad::Add => 0xffab,
                Pad::Subtract => 0xffad,
                Pad::Multiply => 0xffaa,
                Pad::Divide => 0xffaf,
                Pad::Enter => 0xff8d,
            },
        },
    })
}

/// Names of the function keysyms used (xkbcommon's).
const KEYSYM_NAMES: [(u32, &str); 26] = [
    (0xff0d, "Return"),
    (0xff09, "Tab"),
    (0xff08, "BackSpace"),
    (0xff1b, "Escape"),
    (0xffff, "Delete"),
    (0xff50, "Home"),
    (0xff57, "End"),
    (0xff55, "Prior"),
    (0xff56, "Next"),
    (0xff51, "Left"),
    (0xff52, "Up"),
    (0xff53, "Right"),
    (0xff54, "Down"),
    (0xff63, "Insert"),
    (0xffe5, "Caps_Lock"),
    (0xff67, "Menu"),
    (0xffae, "KP_Decimal"),
    (0xffab, "KP_Add"),
    (0xffad, "KP_Subtract"),
    (0xffaa, "KP_Multiply"),
    (0xffaf, "KP_Divide"),
    (0xff8d, "KP_Enter"),
    (0xffe1, "Shift_L"),
    (0xffe3, "Control_L"),
    (0xffe9, "Alt_L"),
    (0xffeb, "Super_L"),
];

/// A keysym's name in a keymap. Characters go by code point ("U0633"),
/// which xkbcommon reads as the Latin-1 or Unicode keysym.
fn keysym_name(sym: u32) -> String {
    if let Some((_, name)) = KEYSYM_NAMES.iter().find(|(s, _)| *s == sym) {
        return (*name).to_string();
    }
    match sym {
        0x20..=0x7e | 0xa0..=0xff => format!("U{sym:04X}"),
        0x0100_0100..=0x0110_ffff => format!("U{:04X}", sym - 0x0100_0000),
        0xffbe..=0xffe0 => format!("F{}", sym - 0xffbe + 1),
        0xffb0..=0xffb9 => format!("KP_{}", sym - 0xffb0),
        _ => format!("{sym:#x}"),
    }
}

/// Keys every keymap has, so that most typing never needs a new one: the
/// modifiers, printable ASCII and the common editing keys.
fn base_keys() -> Vec<u32> {
    let mut v: Vec<u32> = MODIFIERS.iter().map(|m| m.0).collect();
    v.extend(0x20..=0x7e);
    v.extend([
        0xff0d, 0xff09, 0xff08, 0xff1b, 0xffff, 0xff50, 0xff57, 0xff55, 0xff56, 0xff51, 0xff52,
        0xff53, 0xff54, 0xff63, 0xff67,
    ]);
    v
}

/// How many of `syms` to type with one keymap: as many as bring at most
/// `room` keys that aren't among `base`.
fn chunk_len(syms: &[u32], base: &[u32], room: usize) -> usize {
    let mut extra: Vec<u32> = Vec::new();
    for (i, s) in syms.iter().enumerate() {
        if !base.contains(s) && !extra.contains(s) {
            if extra.len() == room {
                return i.max(1);
            }
            extra.push(*s);
        }
    }
    syms.len()
}

/// An XKB keymap with one key per keysym (one level each, so neither Shift
/// nor Caps Lock changes what a key types), the modifier keys mapped to
/// their modifiers.
fn keymap_text(syms: &[u32]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "xkb_keymap {{");
    let _ = writeln!(
        s,
        "xkb_keycodes \"computer-use\" {{\n  minimum = 8;\n  maximum = {};",
        syms.len() + 8
    );
    for i in 0..syms.len() {
        let _ = writeln!(s, "  <K{}> = {};", i + 1, i + 9);
    }
    let _ = writeln!(s, "}};");
    let _ = writeln!(s, "xkb_types \"computer-use\" {{ include \"complete\" }};");
    let _ = writeln!(
        s,
        "xkb_compatibility \"computer-use\" {{ include \"complete\" }};"
    );
    let _ = writeln!(s, "xkb_symbols \"computer-use\" {{");
    for (i, sym) in syms.iter().enumerate() {
        let _ = writeln!(s, "  key <K{}> {{ [ {} ] }};", i + 1, keysym_name(*sym));
    }
    for (sym, real, _) in MODIFIERS {
        if let Some(i) = syms.iter().position(|s| *s == sym) {
            let _ = writeln!(s, "  modifier_map {real} {{ <K{}> }};", i + 1);
        }
    }
    let _ = writeln!(s, "}};");
    let _ = writeln!(s, "}};");
    s
}

// --- event handlers -------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                // Outputs come and go (hotplug): bound as they appear.
                if interface == wl_output::WlOutput::interface().name {
                    let wl = registry.bind::<wl_output::WlOutput, u32, State>(
                        name,
                        version.min(4),
                        qh,
                        name,
                    );
                    let xdg = state
                        .xdg_outputs
                        .as_ref()
                        .map(|m| m.get_xdg_output(&wl, qh, name));
                    let info = OutInfo::default();
                    state.outputs.insert(name, Out { wl, xdg, info });
                }
                state.globals.insert(name, (interface, version));
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.globals.remove(&name);
                if let Some(o) = state.outputs.remove(&name) {
                    o.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state.outputs.get_mut(global).map(|o| &mut o.info) else {
            return;
        };
        match event {
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                o.position = (x, y);
                o.transform = match transform {
                    WEnum::Value(t) => u32::from(t),
                    WEnum::Unknown(t) => t,
                } % 8;
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if let WEnum::Value(f) = flags
                    && f.contains(wl_output::Mode::Current)
                {
                    o.mode = Some((width, height));
                }
            }
            wl_output::Event::Scale { factor } => o.int_scale = factor,
            wl_output::Event::Name { name } => o.wl_name = Some(name),
            wl_output::Event::Done => o.done = true,
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state.outputs.get_mut(global).map(|o| &mut o.info) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => o.logical_pos = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                o.logical_size = Some((width, height));
            }
            zxdg_output_v1::Event::Name { name } => o.xdg_name = Some(name),
            // Before v3 the xdg output has its own `done`.
            zxdg_output_v1::Event::Done => o.done = true,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(f) = state.frames.get_mut(id) else {
            return;
        };
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    WEnum::Value(v) => u32::from(v),
                    WEnum::Unknown(v) => v,
                };
                f.shm = Some(ShmSpec {
                    format,
                    width,
                    height,
                    stride,
                });
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => f.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                let bits = match flags {
                    WEnum::Value(v) => v.bits(),
                    WEnum::Unknown(v) => v,
                };
                f.y_invert = bits & zwlr_screencopy_frame_v1::Flags::YInvert.bits() != 0;
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => f.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => f.failed = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, u64> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = state.synced.max(*id);
        }
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(r) = state.rungs.get_mut(*i) else {
            return;
        };
        match event {
            ext_idle_notification_v1::Event::Idled => r.idled_at = Some(Instant::now()),
            ext_idle_notification_v1::Event::Resumed => r.idled_at = None,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: zxdg_output_manager_v1::ZxdgOutputManagerV1);
delegate_noop!(State: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);
delegate_noop!(State: zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
delegate_noop!(State: zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);
delegate_noop!(State: zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1);
delegate_noop!(State: ext_idle_notifier_v1::ExtIdleNotifierV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_scales_come_out_exact() {
        // 1280x800 at 1.5: sway reports 853x533 logical.
        assert_eq!(output_scale(1280, 800, 853, 533), 1.5);
        assert_eq!(output_scale(1920, 1080, 1536, 864), 1.25);
        assert_eq!(output_scale(2560, 1600, 1280, 800), 2.0);
        assert_eq!(output_scale(1280, 800, 1280, 800), 1.0);
        // 2880x1800 at 1.75 (1645.71 -> 1645 or 1646).
        assert_eq!(output_scale(2880, 1800, 1645, 1028), 1.75);
        assert_eq!(output_scale(2880, 1800, 1646, 1029), 1.75);
        // A ratio no 1/120 step explains stays as it is.
        let odd = output_scale(1000, 1000, 333, 500);
        assert!((odd - 1000.0 / 333.0).abs() < 1e-9, "{odd}");
    }

    #[test]
    fn rotated_outputs_swap_width_and_height() {
        let o = OutInfo {
            mode: Some((1920, 1080)),
            transform: 1, // 90
            logical_pos: Some((1280, 0)),
            logical_size: Some((720, 1280)),
            wl_name: Some("DP-1".into()),
            ..Default::default()
        };
        let d = o.describe(7).unwrap();
        assert_eq!(d.name, "DP-1");
        assert_eq!(d.logical, Rect::new(1280.0, 0.0, 720.0, 1280.0));
        assert_eq!(d.scale, 1.5);
        // No xdg-output: integer scale, wl_output position.
        let o = OutInfo {
            mode: Some((2560, 1440)),
            int_scale: 2,
            position: (10, 20),
            ..Default::default()
        };
        let d = o.describe(3).unwrap();
        assert_eq!(d.name, "output-3");
        assert_eq!(d.logical, Rect::new(10.0, 20.0, 1280.0, 720.0));
        assert_eq!(d.scale, 2.0);
        // Nothing known yet.
        assert!(OutInfo::default().describe(1).is_none());
    }

    #[test]
    fn union_and_clip() {
        let a = Rect::new(0.0, 0.0, 1280.0, 800.0);
        let b = Rect::new(1280.0, -100.0, 1920.0, 1080.0);
        let u = union([a, b].into_iter());
        assert_eq!(u, Rect::new(0.0, -100.0, 3200.0, 1080.0));
        assert_eq!(union(std::iter::empty()), Rect::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(
            clip(Rect::new(1200.4, 700.0, 200.0, 200.0), a),
            Some(Rect::new(1200.0, 700.0, 80.0, 100.0))
        );
        assert_eq!(clip(Rect::new(2000.0, 0.0, 10.0, 10.0), a), None);
        assert_eq!(clip(Rect::new(10.0, 10.0, 0.3, 10.0), a), None);
    }

    fn spec(format: u32, width: u32, height: u32, stride: u32) -> ShmSpec {
        ShmSpec {
            format,
            width,
            height,
            stride,
        }
    }

    #[test]
    fn converts_the_formats_compositors_use() {
        // One pixel of red 0x10, green 0x20, blue 0x30 in each layout.
        let cases: [(u32, &[u8]); 10] = [
            (XRGB8888, &[0x30, 0x20, 0x10, 0x00]),
            (ARGB8888, &[0x30, 0x20, 0x10, 0x80]),
            (XBGR8888, &[0x10, 0x20, 0x30, 0x00]),
            (ABGR8888, &[0x10, 0x20, 0x30, 0xff]),
            (RGBX8888, &[0x00, 0x30, 0x20, 0x10]),
            (BGRX8888, &[0x00, 0x10, 0x20, 0x30]),
            (RGB888, &[0x30, 0x20, 0x10]),
            (BGR888, &[0x10, 0x20, 0x30]),
            (
                XRGB2101010,
                &((0x10u32 << 2) << 20 | (0x20 << 2) << 10 | (0x30 << 2)).to_le_bytes(),
            ),
            (
                XBGR2101010,
                &((0x30u32 << 2) << 20 | (0x20 << 2) << 10 | (0x10 << 2)).to_le_bytes(),
            ),
        ];
        for (format, px) in cases {
            let s = spec(format, 1, 1, px.len() as u32);
            let img = to_rgba(px, s, false).unwrap_or_else(|| panic!("{}", format_name(format)));
            assert_eq!(
                img.as_raw(),
                &[0x10, 0x20, 0x30, 255],
                "{}",
                format_name(format)
            );
        }
        assert!(to_rgba(&[0; 4], spec(fourcc(b"YUYV"), 1, 1, 4), false).is_none());
        assert_eq!(format_name(fourcc(b"YUYV")), "YUYV");
    }

    #[test]
    fn honours_stride_and_y_invert() {
        // 1x2 XRGB with rows padded to 8 bytes: white over black.
        let data = [255, 255, 255, 0, 9, 9, 9, 9, 0, 0, 0, 0, 9, 9, 9, 9];
        let s = spec(XRGB8888, 1, 2, 8);
        let up = to_rgba(&data, s, false).unwrap();
        assert_eq!(up.as_raw(), &[255, 255, 255, 255, 0, 0, 0, 255]);
        let inv = to_rgba(&data, s, true).unwrap();
        assert_eq!(inv.as_raw(), &[0, 0, 0, 255, 255, 255, 255, 255]);
        // Too short for its size.
        assert!(to_rgba(&data[..10], s, false).is_none());
    }

    #[test]
    fn untransform_turns_buffers_upright() {
        // A 2x1 buffer: red, green.
        let img = RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        let r = untransform(img.clone(), 1);
        assert_eq!((r.width(), r.height()), (1, 2));
        assert_eq!(untransform(img.clone(), 0), img);
        let flipped = untransform(img.clone(), 4);
        assert_eq!(flipped.get_pixel(0, 0), &Rgba([0, 255, 0, 255]));
        assert_eq!(
            untransform(img.clone(), 2).get_pixel(0, 0),
            &Rgba([0, 255, 0, 255])
        );
    }

    #[test]
    fn places_pieces_resizing_lower_scales() {
        let mut img = RgbaImage::from_pixel(6, 4, Rgba([0, 0, 0, 255]));
        let white = RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]));
        // A 1x scale piece in a 2x image: doubled.
        place(&mut img, &white, 0, 0, 4, 4);
        assert_eq!(img.get_pixel(3, 3), &Rgba([255, 255, 255, 255]));
        assert_eq!(img.get_pixel(4, 0), &Rgba([0, 0, 0, 255]));
        // A pixel short: the edge repeats; off the image: cut.
        let red = RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]));
        place(&mut img, &red, 4, 2, 2, 3);
        assert_eq!(img.get_pixel(5, 3), &Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn idle_from_the_notifications() {
        let now = Instant::now();
        let ago = |ms: u64| now.checked_sub(Duration::from_millis(ms)).unwrap_or(now);
        let rung = |timeout: u64, created: u64, idled: Option<u64>| Rung {
            timeout: Duration::from_millis(timeout),
            created: ago(created),
            idled_at: idled.map(ago),
        };
        // Active: nothing idle.
        let active = [rung(100, 60_000, None), rung(1000, 60_000, None)];
        assert_eq!(idle_estimate(&active, now), Duration::ZERO);
        // Idle, read on time 400 ms ago: 100 ms + 400 ms.
        let read_on_time = [rung(100, 60_000, Some(400)), rung(1000, 60_000, None)];
        assert_eq!(
            idle_estimate(&read_on_time, now),
            Duration::from_millis(500)
        );
        // Read late (all just now): at least the longest timeout passed.
        let late = [
            rung(100, 60_000, Some(0)),
            rung(1000, 60_000, Some(0)),
            rung(5000, 60_000, Some(0)),
            rung(10_000, 60_000, None),
        ];
        assert_eq!(idle_estimate(&late, now), Duration::from_millis(5000));
        // Never above a timeout that passed without going idle.
        let capped = [rung(100, 60_000, Some(3000)), rung(1000, 60_000, None)];
        assert_eq!(idle_estimate(&capped, now), Duration::from_millis(1000));
        // A rung too young to have fired doesn't cap.
        let young = [rung(100, 500, Some(300)), rung(1000, 500, None)];
        assert_eq!(idle_estimate(&young, now), Duration::from_millis(400));
    }

    #[test]
    fn keysyms_and_their_names() {
        assert_eq!(keysym(Key::Char('\t')), Some(0xff09));
        assert_eq!(keysym(Key::Char('\n')), Some(0xff0d));
        assert_eq!(keysym(Key::Char('\u{1}')), None);
        assert_eq!(keysym(Key::Char('\u{85}')), None);
        assert_eq!(keysym(Key::Char('a')), Some(0x61));
        assert_eq!(keysym(Key::Char('é')), Some(0xe9));
        assert_eq!(keysym(Key::Char('س')), Some(0x0100_0633));
        assert_eq!(keysym(Key::Named(NamedKey::F(12))), Some(0xffc9));
        assert_eq!(
            keysym(Key::Named(NamedKey::Numpad(Pad::Digit(5)))),
            Some(0xffb5)
        );
        assert_eq!(keysym_name(0x61), "U0061");
        assert_eq!(keysym_name(0x20), "U0020");
        assert_eq!(keysym_name(0xe9), "U00E9");
        assert_eq!(keysym_name(0x0100_0633), "U0633");
        assert_eq!(keysym_name(0x0101_f600), "U1F600");
        assert_eq!(keysym_name(0xff09), "Tab");
        assert_eq!(keysym_name(0xffc9), "F12");
        assert_eq!(keysym_name(0xffb5), "KP_5");
        assert_eq!(keysym_name(0xff55), "Prior");
    }

    #[test]
    fn keymap_has_a_key_per_keysym_and_the_modifiers() {
        let mut syms = base_keys();
        syms.push(0x0100_0633);
        let text = keymap_text(&syms);
        assert!(text.contains(&format!("maximum = {};", syms.len() + 8)));
        assert!(text.contains("<K1> = 9;"));
        assert!(text.contains("key <K1> { [ Shift_L ] };"));
        assert!(text.contains("modifier_map Shift { <K1> };"));
        assert!(text.contains("modifier_map Control { <K2> };"));
        assert!(text.contains("modifier_map Mod1 { <K3> };"));
        assert!(text.contains("modifier_map Mod4 { <K4> };"));
        assert!(text.contains(&format!("key <K{}> {{ [ U0633 ] }};", syms.len())));
        // The base keys leave room for others, under XWayland's 255.
        assert!(base_keys().len() + 100 < KEYMAP_CAP);
        const { assert!(KEYMAP_CAP + 8 <= 255) };
    }

    #[test]
    fn long_texts_are_typed_in_keymap_sized_chunks() {
        let base = base_keys();
        let ascii: Vec<u32> = "hello".chars().map(|c| c as u32).collect();
        assert_eq!(chunk_len(&ascii, &base, 2), 5);
        // Three new keys with room for two: the third waits.
        let syms = [
            0x0100_0633,
            0x61,
            0x0100_0644,
            0x0100_0633,
            0x0100_0645,
            0x62,
        ];
        assert_eq!(chunk_len(&syms, &base, 2), 4);
        assert_eq!(chunk_len(&syms[4..], &base, 2), 2);
        // Always progress.
        assert_eq!(chunk_len(&syms, &base, 0), 1);
    }

    #[test]
    fn button_codes() {
        assert_eq!(button_code(1).unwrap(), BTN_LEFT);
        assert_eq!(button_code(2).unwrap(), BTN_MIDDLE);
        assert_eq!(button_code(3).unwrap(), BTN_RIGHT);
        assert!(button_code(4).is_err());
    }

    /// A compositor that never answers: connecting gives up in time.
    #[test]
    fn a_silent_compositor_is_an_error_not_a_hang() {
        let (ours, _theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let conn = Connection::from_socket(ours).unwrap();
        let start = Instant::now();
        let r = Wl::with_connection(conn, Duration::from_millis(200));
        assert!(r.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    /// A compositor that hung up: the connection is lost, quickly.
    #[test]
    fn a_closed_connection_is_lost() {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(theirs);
        let conn = Connection::from_socket(ours).unwrap();
        let r = Wl::with_connection(conn, Duration::from_secs(5));
        let Err(e) = r else {
            panic!("connected to nothing")
        };
        assert!(e.to_string().contains("lost the connection"), "{e}");
    }
}
