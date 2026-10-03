//! Wayland overlay (Hyprland, sway and other compositors with wlr
//! layer-shell): each layer is a layer-shell surface on the overlay layer,
//! anchored to its output's top-left corner and placed by its margins, with
//! an empty input region (clicks pass through), no keyboard focus and no
//! exclusive zone.
//!
//! Compositors animate a surface when it is mapped and unmapped (Hyprland's
//! fades and pop-ins), and the overlay hides around every screenshot. So
//! every layer's surface is created once, when the overlay starts, and
//! stays mapped for good: hiding shows a transparent buffer instead, and a
//! layer only gets a new surface when it moves to another output.
//!
//! Images are drawn at the highest output scale and shown at their logical
//! size through wp_viewporter, so they stay sharp at fractional scales.

use std::collections::HashMap;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_region, wl_registry, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use super::draw;
use super::helper::{Hotkey, Layer, Part, Surface, SurfaceEvent};
use crate::keys::KeyCombo;
use crate::types::Rect;

/// The namespace compositors know the overlay's surfaces by: a Hyprland
/// user can write `layerrule = noanim, computer-use` (or blur, etc.).
const NAMESPACE: &str = "computer-use";

/// Longest wait for the compositor to answer (a configure, a sync).
const ANSWER_WAIT: Duration = Duration::from_millis(500);

/// Every layer the painter uses, created up front.
/// The parts made at start (one engine's); a hub's other agents get theirs
/// when first shown.
const LAYERS: [Layer; 6] = [
    Layer::new(0, Part::Top),
    Layer::new(0, Part::Right),
    Layer::new(0, Part::Bottom),
    Layer::new(0, Part::Left),
    Layer::new(0, Part::Label),
    Layer::new(0, Part::Cursor),
];

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// An output and where it is in the layout.
#[derive(Default)]
struct Out {
    wl: Option<wl_output::WlOutput>,
    xdg: Option<zxdg_output_v1::ZxdgOutputV1>,
    /// Logical position and size (xdg-output), once known.
    pos: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
    /// Fallbacks from wl_output: position, current mode in pixels, scale.
    geo: (i32, i32),
    mode: (i32, i32),
    int_scale: i32,
    transform_swaps: bool,
}

impl Out {
    fn rect(&self) -> Option<Rect> {
        let (x, y) = self.pos.unwrap_or(self.geo);
        let (w, h) = match self.size {
            Some(s) => s,
            None => {
                let (mw, mh) = if self.transform_swaps {
                    (self.mode.1, self.mode.0)
                } else {
                    self.mode
                };
                let s = self.int_scale.max(1);
                (mw / s, mh / s)
            }
        };
        (w > 0 && h > 0).then(|| Rect::new(x.into(), y.into(), w.into(), h.into()))
    }

    /// Pixels per logical unit: the mode over the logical size (1.5 for a
    /// 2560-pixel output 1707 units wide).
    fn scale(&self) -> f64 {
        let mw = if self.transform_swaps {
            self.mode.1
        } else {
            self.mode.0
        };
        match self.size {
            Some((w, _)) if w > 0 && mw > 0 => f64::from(mw) / f64::from(w),
            _ => f64::from(self.int_scale.max(1)),
        }
    }
}

#[derive(Default)]
struct State {
    /// Outputs by their registry name.
    outputs: HashMap<u32, Out>,
    /// The last configure of each layer surface: serial, width, height.
    configures: HashMap<Layer, (u32, u32, u32)>,
    /// Layers whose surface the compositor closed (its output went away).
    closed: Vec<Layer>,
    synced: bool,
}

/// A wl_shm buffer in a memfd of its own.
struct Buf {
    wl: wl_buffer::WlBuffer,
    busy: Arc<AtomicBool>,
    map: *mut u8,
    len: usize,
    w: u32,
    h: u32,
    /// Holds the memory open as long as the buffer exists.
    _fd: OwnedFd,
}

impl Drop for Buf {
    fn drop(&mut self) {
        self.wl.destroy();
        // SAFETY: `map` is the mapping of `len` bytes made in `new`.
        unsafe {
            libc::munmap(self.map.cast(), self.len);
        }
    }
}

impl Buf {
    /// A buffer of `w` x `h` premultiplied ARGB pixels, all transparent.
    fn new(shm: &wl_shm::WlShm, qh: &QueueHandle<State>, w: u32, h: u32) -> Result<Self, String> {
        let stride = w as usize * 4;
        let len = stride * h as usize;
        // SAFETY: plain syscalls; the fd is owned from here on.
        let fd = unsafe {
            let raw = libc::memfd_create(c"computer-use-overlay".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
            }
            OwnedFd::from_raw_fd(raw)
        };
        use std::os::fd::AsRawFd as _;
        // SAFETY: as above, on the fd we own.
        let map = unsafe {
            if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) != 0 {
                return Err(format!("ftruncate: {}", std::io::Error::last_os_error()));
            }
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(format!("mmap: {}", std::io::Error::last_os_error()));
        }
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let busy = Arc::new(AtomicBool::new(false));
        let wl = pool.create_buffer(
            0,
            w as i32,
            h as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            qh,
            busy.clone(),
        );
        // The buffer keeps the memory; the pool isn't needed any more.
        pool.destroy();
        Ok(Self {
            wl,
            busy,
            map: map.cast(),
            len,
            w,
            h,
            _fd: fd,
        })
    }

    fn fill(&mut self, bgra: &[u8]) {
        let n = bgra.len().min(self.len);
        // SAFETY: the mapping is `len` bytes long and only written here,
        // while the compositor isn't reading it (the buffer is released).
        unsafe {
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), self.map, n);
        }
    }

    fn clear(&mut self) {
        // SAFETY: as in `fill`.
        unsafe {
            std::ptr::write_bytes(self.map, 0, self.len);
        }
    }
}

/// One layer's surface.
struct Surf {
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    viewport: Option<wp_viewport::WpViewport>,
    /// The output (registry name) it is on.
    output: u32,
    /// Logical size asked for, and the configure last acknowledged
    /// (serial, width, height).
    size: (u32, u32),
    acked: Option<(u32, u32, u32)>,
    /// Margins from the output's top-left corner.
    margin: (i32, i32),
    buffers: Vec<Buf>,
    /// What it shows when not hidden: premultiplied BGRA, pixel size.
    image: Option<(Vec<u8>, u32, u32)>,
}

impl Drop for Surf {
    fn drop(&mut self) {
        if let Some(v) = &self.viewport {
            v.destroy();
        }
        self.layer.destroy();
        self.surface.destroy();
    }
}

pub struct WaylandSurface {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    globals: GlobalList,
    state: State,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    xdg_outputs: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    surfs: HashMap<Layer, Surf>,
    /// Where each layer was last placed (logical units), so one the
    /// compositor closes can be put back.
    placed: HashMap<Layer, (f64, f64)>,
    /// Layers whose surface the compositor closed and that couldn't be put
    /// back yet (no output there): their image, for the next move.
    orphans: HashMap<Layer, (Vec<u8>, u32, u32)>,
    hidden: bool,
    /// The global keys, bound in the compositor (stop, settings).
    keys: [super::wayland_stop::BoundKey; 2],
}

impl WaylandSurface {
    /// Connect to the compositor; fails where there is no Wayland display
    /// or no wlr layer-shell (GNOME), and the caller falls back to X11.
    pub fn open() -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(err)?;
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(err)?;
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals
            .bind(&qh, 4..=6, ())
            .map_err(|e| format!("wl_compositor: {e}"))?;
        let shm: wl_shm::WlShm = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("wl_shm: {e}"))?;
        let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
            globals.bind(&qh, 1..=4, ()).map_err(|_| {
                "this compositor has no wlr layer-shell (zwlr_layer_shell_v1)".to_string()
            })?;
        let xdg_outputs = globals.bind(&qh, 1..=3, ()).ok();
        let viewporter = globals.bind(&qh, 1..=1, ()).ok();
        let mut s = Self {
            conn,
            queue,
            qh,
            globals,
            state: State::default(),
            compositor,
            shm,
            layer_shell,
            xdg_outputs,
            viewporter,
            surfs: HashMap::new(),
            placed: HashMap::new(),
            orphans: HashMap::new(),
            hidden: false,
            keys: Hotkey::ALL.map(super::wayland_stop::BoundKey::new),
        };
        s.bind_outputs();
        s.sync();
        s.sync();
        if s.state.outputs.values().all(|o| o.rect().is_none()) {
            return Err("the compositor reported no outputs".into());
        }
        // Every layer gets its surface now, while it is still transparent:
        // whatever animation the compositor plays on a new surface plays
        // unseen, and never again.
        for layer in LAYERS {
            if let Some(out) = s.main_output() {
                let _ = s.surf(layer, out, (1, 1));
            }
        }
        s.sync();
        Ok(s)
    }

    /// Bind outputs not bound yet (at start, and when one is plugged in).
    fn bind_outputs(&mut self) {
        let list: Vec<(u32, u32)> = self.globals.contents().with_list(|l| {
            l.iter()
                .filter(|g| g.interface == wl_output::WlOutput::interface().name)
                .map(|g| (g.name, g.version))
                .collect()
        });
        for (name, version) in list {
            if self
                .state
                .outputs
                .get(&name)
                .is_some_and(|o| o.wl.is_some())
            {
                continue;
            }
            let wl: wl_output::WlOutput =
                self.globals
                    .registry()
                    .bind(name, version.min(4), &self.qh, name);
            let xdg = self
                .xdg_outputs
                .as_ref()
                .map(|m| m.get_xdg_output(&wl, &self.qh, name));
            let out = self.state.outputs.entry(name).or_default();
            out.wl = Some(wl);
            out.xdg = xdg;
        }
    }

    /// Wait (bounded) until the compositor has handled everything sent so
    /// far, dispatching what it sent meanwhile.
    fn sync(&mut self) {
        self.state.synced = false;
        self.conn.display().sync(&self.qh, ());
        let deadline = Instant::now() + ANSWER_WAIT;
        while !self.state.synced && Instant::now() < deadline {
            self.read(deadline.saturating_duration_since(Instant::now()));
        }
    }

    /// Flush, then read and dispatch events, waiting at most `wait`.
    fn read(&mut self, wait: Duration) {
        let _ = self.conn.flush();
        let _ = self.queue.dispatch_pending(&mut self.state);
        if let Some(guard) = self.queue.prepare_read() {
            use std::os::fd::AsRawFd as _;
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = wait.as_millis().min(i32::MAX as u128) as i32;
            // SAFETY: one pollfd on a live descriptor.
            let ready = unsafe { libc::poll(&mut pfd, 1, ms) } > 0;
            if ready {
                if let Err(e) = guard.read() {
                    // The compositor is gone: exit with an error, so the
                    // engine starts a new helper (as the X11 one does).
                    if !matches!(e, wayland_client::backend::WaylandError::Io(ref io) if io.kind() == std::io::ErrorKind::WouldBlock)
                    {
                        eprintln!("overlay: lost the Wayland connection: {e}");
                        std::process::exit(2);
                    }
                }
            } else {
                drop(guard);
            }
        }
        let _ = self.queue.dispatch_pending(&mut self.state);
    }

    /// The outputs with a known place, by registry name.
    fn output_rects(&self) -> Vec<(u32, Rect)> {
        let mut v: Vec<(u32, Rect)> = self
            .state
            .outputs
            .iter()
            .filter_map(|(n, o)| o.rect().map(|r| (*n, r)))
            .collect();
        // The one at the layout's origin first, then left to right.
        v.sort_by(|a, b| {
            let key = |r: &Rect| (r.x != 0.0 || r.y != 0.0, r.y, r.x);
            key(&a.1)
                .partial_cmp(&key(&b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    fn main_output(&self) -> Option<u32> {
        self.output_rects().first().map(|(n, _)| *n)
    }

    /// The output a box centred at (cx, cy) is on (the nearest one if none).
    fn output_at(&self, cx: f64, cy: f64) -> Option<(u32, Rect)> {
        let outs = self.output_rects();
        outs.iter()
            .find(|(_, r)| r.contains(crate::types::Point::new(cx, cy)))
            .or_else(|| {
                outs.iter().min_by(|a, b| {
                    let d = |r: &Rect| {
                        let dx = (r.x - cx).max(0.0).max(cx - (r.x + r.width));
                        let dy = (r.y - cy).max(0.0).max(cy - (r.y + r.height));
                        dx.hypot(dy)
                    };
                    d(&a.1).total_cmp(&d(&b.1))
                })
            })
            .copied()
    }

    /// The layer's surface on `output`, made (or moved there) if needed,
    /// sized `size` (logical units) and acknowledged.
    fn surf(&mut self, layer: Layer, output: u32, size: (u32, u32)) -> Option<&mut Surf> {
        if self.surfs.get(&layer).is_some_and(|s| s.output != output) {
            self.surfs.remove(&layer);
        }
        if !self.surfs.contains_key(&layer) {
            let wl_out = self.state.outputs.get(&output)?.wl.clone()?;
            let surface = self.compositor.create_surface(&self.qh, ());
            // Clicks go to whatever is underneath.
            let region = self.compositor.create_region(&self.qh, ());
            surface.set_input_region(Some(&region));
            region.destroy();
            let ls = self.layer_shell.get_layer_surface(
                &surface,
                Some(&wl_out),
                zwlr_layer_shell_v1::Layer::Overlay,
                NAMESPACE.to_string(),
                &self.qh,
                layer,
            );
            use zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity};
            ls.set_anchor(Anchor::Top | Anchor::Left);
            // Placed against the output's real edges, under any bars.
            ls.set_exclusive_zone(-1);
            ls.set_keyboard_interactivity(KeyboardInteractivity::None);
            let viewport = self
                .viewporter
                .as_ref()
                .map(|v| v.get_viewport(&surface, &self.qh, ()));
            self.state.configures.remove(&layer);
            self.surfs.insert(
                layer,
                Surf {
                    surface,
                    layer: ls,
                    viewport,
                    output,
                    size: (0, 0),
                    acked: None,
                    margin: (0, 0),
                    buffers: Vec::new(),
                    image: None,
                },
            );
        }
        let (w, h) = (size.0.max(1), size.1.max(1));
        if self.surfs.get(&layer).is_some_and(|s| s.size != (w, h)) {
            let s = self.surfs.get_mut(&layer)?;
            s.size = (w, h);
            s.layer.set_size(w, h);
            s.surface.commit();
            // The compositor answers with a configure (for this size, or
            // what it can give), to be acknowledged before the next buffer.
            let deadline = Instant::now() + ANSWER_WAIT;
            loop {
                let latest = self.state.configures.get(&layer).copied();
                let s = self.surfs.get_mut(&layer)?;
                if let Some((serial, cw, ch)) = latest
                    && s.acked != Some((serial, cw, ch))
                {
                    s.layer.ack_configure(serial);
                    s.acked = Some((serial, cw, ch));
                    break;
                }
                if Instant::now() >= deadline {
                    break;
                }
                self.read(Duration::from_millis(20));
            }
        }
        self.surfs.get_mut(&layer)
    }

    /// A released buffer of `w` x `h` pixels for `layer` (a new one when the
    /// size changed or the compositor still reads every one).
    fn buffer(&mut self, layer: Layer, w: u32, h: u32) -> Option<usize> {
        let (shm, qh) = (self.shm.clone(), self.qh.clone());
        let s = self.surfs.get_mut(&layer)?;
        s.buffers
            .retain(|b| (b.w, b.h) == (w, h) || b.busy.load(Ordering::Acquire));
        if let Some(i) = s
            .buffers
            .iter()
            .position(|b| (b.w, b.h) == (w, h) && !b.busy.load(Ordering::Acquire))
        {
            return Some(i);
        }
        if s.buffers.len() >= 3 {
            // All still in use: drop the oldest (the compositor copies shm
            // buffers on commit, so this is the rare case).
            s.buffers.remove(0);
        }
        s.buffers.push(Buf::new(&shm, &qh, w, h).ok()?);
        Some(s.buffers.len() - 1)
    }

    /// Put `layer`'s image (or, when hidden, nothing) on its surface.
    fn present(&mut self, layer: Layer) {
        let hidden = self.hidden;
        let Some(s) = self.surfs.get(&layer) else {
            return;
        };
        let (pw, ph) = s.image.as_ref().map_or((1, 1), |(_, w, h)| (*w, *h));
        let Some(i) = self.buffer(layer, pw, ph) else {
            return;
        };
        let Some(s) = self.surfs.get_mut(&layer) else {
            return;
        };
        let (img, buf) = (&s.image, &mut s.buffers[i]);
        match img {
            Some((bgra, _, _)) if !hidden => buf.fill(bgra),
            _ => buf.clear(),
        }
        buf.busy.store(true, Ordering::Release);
        s.surface.attach(Some(&buf.wl), 0, 0);
        s.surface.damage_buffer(0, 0, pw as i32, ph as i32);
        if let Some(v) = &s.viewport {
            v.set_destination(s.size.0 as i32, s.size.1 as i32);
        }
        s.layer.set_margin(s.margin.1, 0, 0, s.margin.0);
        s.surface.commit();
    }

    /// Image pixels per logical unit: the highest output scale, so images
    /// are sharp everywhere (without wp_viewporter, 1).
    fn ppu(&self) -> f64 {
        if self.viewporter.is_none() {
            return 1.0;
        }
        self.state
            .outputs
            .values()
            .filter(|o| o.rect().is_some())
            .map(Out::scale)
            .fold(1.0, f64::max)
    }

    /// Place `layer` at (x, y) logical units, sized for `pixels`.
    fn place(&mut self, layer: Layer, x: f64, y: f64, pixels: (u32, u32)) -> bool {
        let ppu = self.ppu();
        let size = (
            (f64::from(pixels.0) / ppu).ceil().max(1.0) as u32,
            (f64::from(pixels.1) / ppu).ceil().max(1.0) as u32,
        );
        let Some((out, r)) =
            self.output_at(x + f64::from(size.0) / 2.0, y + f64::from(size.1) / 2.0)
        else {
            return false;
        };
        let Some(s) = self.surf(layer, out, size) else {
            return false;
        };
        s.margin = ((x - r.x).round() as i32, (y - r.y).round() as i32);
        self.placed.insert(layer, (x, y));
        true
    }

    /// Show `image` for `layer` at (x, y) on a new surface (its old one
    /// was closed); false when no output is there.
    fn restore(&mut self, layer: Layer, x: f64, y: f64, image: (Vec<u8>, u32, u32)) -> bool {
        if !self.place(layer, x, y, (image.1, image.2)) {
            return false;
        }
        if let Some(s) = self.surfs.get_mut(&layer) {
            s.image = Some(image);
        }
        self.present(layer);
        true
    }
}

impl Surface for WaylandSurface {
    fn excluded_from_capture(&self) -> bool {
        false
    }

    fn screen(&self) -> Rect {
        let outs = self.output_rects();
        let mut it = outs.iter().map(|(_, r)| *r);
        let first = it.next().unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
        it.fold(first, |a, b| {
            let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
            let (x1, y1) = (
                (a.x + a.width).max(b.x + b.width),
                (a.y + a.height).max(b.y + b.height),
            );
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        })
    }

    fn main_screen(&self) -> Rect {
        self.output_rects()
            .first()
            .map_or_else(|| self.screen(), |(_, r)| *r)
    }

    fn render_scale(&self) -> f32 {
        self.ppu() as f32
    }

    fn px_per_unit(&self) -> f32 {
        self.ppu() as f32
    }

    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
        self.orphans.remove(&layer);
        let (w, h) = (img.width(), img.height());
        if !self.place(layer, x, y, (w, h)) {
            return;
        }
        if let Some(s) = self.surfs.get_mut(&layer) {
            s.image = Some((draw::to_bgra_premultiplied(img), w, h));
        }
        self.present(layer);
        let _ = self.conn.flush();
    }

    fn move_to(&mut self, layer: Layer, x: f64, y: f64) {
        // Closed by the compositor before: back where it goes now.
        if let Some(image) = self.orphans.remove(&layer) {
            if !self.restore(layer, x, y, image.clone()) {
                self.orphans.insert(layer, image);
            }
            let _ = self.conn.flush();
            return;
        }
        let Some(pixels) = self
            .surfs
            .get(&layer)
            .and_then(|s| s.image.as_ref().map(|(_, w, h)| (*w, *h)))
        else {
            return;
        };
        let output = self.surfs.get(&layer).map(|s| s.output);
        if !self.place(layer, x, y, pixels) {
            return;
        }
        // Moved to another output: a new surface, so the image again.
        if self.surfs.get(&layer).map(|s| s.output) != output
            || self.surfs.get(&layer).is_some_and(|s| s.buffers.is_empty())
        {
            self.present(layer);
        } else if let Some(s) = self.surfs.get(&layer) {
            s.layer.set_margin(s.margin.1, 0, 0, s.margin.0);
            s.surface.commit();
        }
        let _ = self.conn.flush();
    }

    fn hide(&mut self, layer: Layer) {
        self.orphans.remove(&layer);
        let shown = self.surfs.get_mut(&layer).and_then(|s| s.image.take());
        if shown.is_some() {
            self.present(layer);
            let _ = self.conn.flush();
        }
    }

    fn set_hidden(&mut self, hidden: bool) {
        if self.hidden == hidden {
            return;
        }
        self.hidden = hidden;
        let layers: Vec<Layer> = self
            .surfs
            .iter()
            .filter(|(_, s)| s.image.is_some())
            .map(|(l, _)| *l)
            .collect();
        for layer in layers {
            self.present(layer);
        }
        // Make sure the compositor has it before we answer (the engine
        // takes its screenshot right after).
        self.sync();
    }

    fn set_hotkey(&mut self, which: Hotkey, combo: Option<KeyCombo>) -> bool {
        self.keys[which.index()].set(combo)
    }

    fn pump(&mut self) -> Vec<SurfaceEvent> {
        self.read(Duration::ZERO);
        // Outputs plugged in since.
        self.bind_outputs();
        for layer in std::mem::take(&mut self.state.closed) {
            // Its output went away (or the compositor closed it): a new
            // surface where it was, on an output still there, or on the
            // next move. It used to stay hidden until its image changed.
            let image = self.surfs.remove(&layer).and_then(|mut s| s.image.take());
            if let (Some(image), Some(&(x, y))) = (image, self.placed.get(&layer))
                && !self.restore(layer, x, y, image.clone())
            {
                self.orphans.insert(layer, image);
            }
        }
        let _ = self.conn.flush();
        let mut events = Vec::new();
        for (key, which) in self.keys.iter_mut().zip(Hotkey::ALL) {
            key.refresh();
            if key.pressed() {
                events.push(SurfaceEvent::Hotkey(which));
            }
        }
        events
    }

    fn close(&mut self) {
        for key in &mut self.keys {
            key.set(None);
        }
        self.surfs.clear();
        let _ = self.conn.flush();
    }
}

// -- events ------------------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // New outputs are bound in `pump` (from the global list); a removed
        // one is forgotten here.
        if let wl_registry::Event::GlobalRemove { name } = event {
            state.outputs.remove(&name);
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let out = state.outputs.entry(*name).or_default();
        match event {
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                out.geo = (x, y);
                use wayland_client::WEnum;
                use wl_output::Transform as T;
                out.transform_swaps = matches!(
                    transform,
                    WEnum::Value(T::_90 | T::_270 | T::Flipped90 | T::Flipped270)
                );
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if let wayland_client::WEnum::Value(f) = flags
                    && f.contains(wl_output::Mode::Current)
                {
                    out.mode = (width, height);
                }
            }
            wl_output::Event::Scale { factor } => out.int_scale = factor,
            _ => {}
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let out = state.outputs.entry(*name).or_default();
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => out.pos = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                out.size = Some((width, height));
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, Layer> for State {
    fn event(
        state: &mut Self,
        _: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        layer: &Layer,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                state.configures.insert(*layer, (serial, width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed.push(*layer),
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<AtomicBool>> for State {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        busy: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            busy.store(false, Ordering::Release);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = true;
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_region::WlRegion);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(State: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);
delegate_noop!(State: ignore wp_viewporter::WpViewporter);
delegate_noop!(State: ignore wp_viewport::WpViewport);
