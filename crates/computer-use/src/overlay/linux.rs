//! X11 overlay: override-redirect windows (no window-manager decoration or
//! focus) whose input region is empty, so every click passes through to the
//! apps below. With a compositing manager the windows use a 32-bit ARGB
//! visual for smooth edges; without one, a SHAPE mask cuts them out. X11 has
//! no "exclude from capture", so the engine asks the helper to hide the
//! overlay for the instant of a screenshot. When this process dies, the X
//! server destroys its windows.
//!
//! Compositors (picom, KWin, Mutter, xfwm, and Wayland compositors through
//! XWayland) animate windows that are mapped or unmapped: a fade or a zoom
//! each time, and the overlay would still be fading out when the screenshot
//! is taken. So a window stays mapped while the overlay is in use, and
//! hiding it (around every screenshot) leaves it empty instead. Its image is
//! the window's background pixmap, which the X server paints by itself: on
//! map (no black first frame), on exposure, and all at once when the image
//! changes (a compositor never sees half of it). Window properties ask
//! compositors for no shadow and no animation, and name the windows
//! (`WM_CLASS` "computer-use-overlay") for users' compositor rules.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClipOrdering, ColormapAlloc, ConfigureWindowAux,
    ConnectionExt as _, CreateGCAux, CreateWindowAux, GrabMode, ImageFormat, ModMask, PropMode,
    Rectangle, StackMode, VisualClass, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::draw;
use super::helper::{Hotkey, Layer, Surface, SurfaceEvent};
use crate::keys::KeyCombo;
use crate::types::Rect;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        UTF8_STRING,
        _NET_WM_NAME,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_BYPASS_COMPOSITOR,
        _COMPTON_SHADOW,
        _KDE_NET_WM_SKIP_CLOSE_ANIMATION,
    }
}

/// With nothing shown for this long, the windows go (they are empty by
/// then, so this is invisible): a mapped window, even an empty one, keeps
/// compositors from handing the screen to a full-screen game.
const IDLE_DROP: Duration = Duration::from_secs(3);
/// How often the screen layout and the compositor are checked again.
const REFRESH_EVERY: Duration = Duration::from_secs(1);
/// The longest wait for a compositor to draw the screen without the overlay.
const SETTLE_MAX: Duration = Duration::from_millis(100);
/// A compositor's frame, waited for when its drawing can't be watched.
const FRAME: Duration = Duration::from_millis(20);

struct Win {
    id: Window,
    gc: u32,
    /// The image on the server; the window's background while it shows.
    pixmap: u32,
    /// The image, to build the window again for another visual.
    img: Pixmap,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
    /// Without ARGB: where the image is opaque.
    shape: Vec<Rectangle>,
    /// The painter shows this layer.
    shown: bool,
    /// What the window lets be seen now: its image, or nothing (taken as
    /// true for a new window, so the first update sets everything).
    visible: bool,
    /// Its background is the image (else transparent, or not set yet).
    bg_image: bool,
}

impl Win {
    fn rect(&self) -> Rectangle {
        Rectangle {
            x: self.x,
            y: self.y,
            width: self.w,
            height: self.h,
        }
    }

    /// Let the image be seen, or nothing (the window stays mapped), as the
    /// layer's state says; `geometry` moves or resizes the window on the way.
    fn update(
        &mut self,
        conn: &RustConnection,
        argb: bool,
        hidden: bool,
        geometry: Option<&ConfigureWindowAux>,
    ) {
        let visible = self.shown && !hidden;
        if visible && !self.bg_image {
            let bg = ChangeWindowAttributesAux::new().background_pixmap(self.pixmap);
            let _ = conn.change_window_attributes(self.id, &bg);
            self.bg_image = true;
        }
        if !visible && self.visible {
            if argb {
                // Transparent pixels, for a compositor that ignores shapes
                // (xcompmgr, XWayland). First: the X server clips drawing to
                // the shape, so after the empty one below nothing would be.
                let bg = ChangeWindowAttributesAux::new().background_pixel(0);
                let _ = conn.change_window_attributes(self.id, &bg);
                let _ = conn.clear_area(false, self.id, 0, 0, 0, 0);
                self.bg_image = false;
            }
            // An empty shape: a compositor that honours shapes draws nothing
            // at all, not even a shadow or a blur behind the window.
            let _ = conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                ClipOrdering::UNSORTED,
                self.id,
                0,
                0,
                &[],
            );
        }
        if visible && !argb {
            let _ = conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                ClipOrdering::UNSORTED,
                self.id,
                0,
                0,
                &self.shape,
            );
        }
        if let Some(g) = geometry {
            let _ = conn.configure_window(self.id, g);
        }
        if visible {
            // Repaint from the background: the whole new image in one step.
            let _ = conn.clear_area(false, self.id, 0, 0, 0, 0);
        }
        if visible && argb && !self.visible {
            // Unshaped again; the server paints the background (the image)
            // into what that uncovers.
            let _ = conn.shape_mask(
                shape::SO::SET,
                shape::SK::BOUNDING,
                self.id,
                0,
                0,
                x11rb::NONE,
            );
        }
        self.visible = visible;
    }

    fn destroy(&self, conn: &RustConnection) {
        let _ = conn.destroy_window(self.id);
        let _ = conn.free_pixmap(self.pixmap);
        let _ = conn.free_gc(self.gc);
    }
}

pub struct X11Surface {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    /// `_NET_WM_CM_Sn`, owned by the running compositing manager.
    cm: u32,
    /// A compositing manager runs.
    composited: bool,
    /// The screen's own depth, visual and colormap.
    plain: (u8, u32, u32),
    /// A 32-bit TrueColor visual and a colormap for it, if there is one.
    argb_visual: Option<(u32, u32)>,
    /// The windows use the ARGB visual.
    argb: bool,
    /// The root window, and the primary monitor on it (RandR 1.5).
    screen: Rect,
    main: Rect,
    randr: bool,
    /// Damage to the root window: a compositor drawing the screen.
    damage: Option<u32>,
    /// Waits in a row that saw the compositor draw nothing; after a few, its
    /// drawing isn't watched any more.
    missed: u8,
    /// Events read while waiting for the compositor, for `pump`.
    pending: VecDeque<Event>,
    layers: HashMap<Layer, Win>,
    hidden: bool,
    /// Since when no layer is shown.
    idle_since: Option<Instant>,
    last_raise: Instant,
    last_refresh: Instant,
    /// The global keys' passive grabs (stop key, settings key).
    grabs: [Option<Grab>; 2],
}

/// A global key's passive grab, and its state.
#[derive(Debug, Clone, Copy)]
struct Grab {
    code: u8,
    mods: ModMask,
    /// Held down (key repeat is not a new press), and when it last
    /// reported a press.
    down: bool,
    pressed: u32,
    /// When it was last released (X sends release + press with the same
    /// time for each key repeat).
    released: Option<u32>,
}

/// The modifier bits a global key is told apart by (Shift, Control, Mod1,
/// Mod4), without the lock keys'.
const MOD_BITS: u16 = 0x0001 | 0x0004 | 0x0008 | 0x0040;

/// NumLock (Mod2) and CapsLock variants, so the stop key works whatever
/// state those locks are in.
const LOCKS: [u16; 4] = [0, 0x0002, 0x0010, 0x0012];

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Left, top, right and bottom edges.
fn edges(r: Rectangle) -> (i32, i32, i32, i32) {
    let (x, y) = (i32::from(r.x), i32::from(r.y));
    (x, y, x + i32::from(r.width), y + i32::from(r.height))
}

/// The part of `a` inside `b`, if any.
fn clip(a: Rectangle, b: Rectangle) -> Option<Rectangle> {
    let (al, at, ar, ab) = edges(a);
    let (bl, bt, br, bb) = edges(b);
    let (l, t, r, b) = (al.max(bl), at.max(bt), ar.min(br), ab.min(bb));
    (l < r && t < b).then(|| Rectangle {
        x: l as i16,
        y: t as i16,
        width: (r - l) as u16,
        height: (b - t) as u16,
    })
}

/// Whether `outer` contains all of `inner`.
fn covers(outer: Rectangle, inner: Rectangle) -> bool {
    let (ol, ot, or, ob) = edges(outer);
    let (il, it, ir, ib) = edges(inner);
    ol <= il && ot <= it && or >= ir && ob >= ib
}

impl X11Surface {
    pub fn open() -> Result<Self, String> {
        let (conn, num) = crate::linux::x11::connect_display().map_err(err)?;
        if conn
            .extension_information(shape::X11_EXTENSION_NAME)
            .map_err(err)?
            .is_none()
        {
            return Err("the X server has no SHAPE extension (needed for click-through)".into());
        }
        let screen = conn.setup().roots[num].clone();
        let root = screen.root;
        let atoms = Atoms::new(&conn).map_err(err)?.reply().map_err(err)?;
        let cm = conn
            .intern_atom(false, format!("_NET_WM_CM_S{num}").as_bytes())
            .map_err(err)?
            .reply()
            .map_err(err)?
            .atom;
        let argb_visual = match screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)
        {
            Some(v) => {
                let cmap = conn.generate_id().map_err(err)?;
                conn.create_colormap(ColormapAlloc::NONE, cmap, root, v.visual_id)
                    .map_err(err)?;
                Some((v.visual_id, cmap))
            }
            None => None,
        };
        let has = |name: &'static str| conn.extension_information(name).ok().flatten().is_some();
        let randr = has(randr::X11_EXTENSION_NAME)
            && conn
                .randr_query_version(1, 5)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|v| (v.major_version, v.minor_version) >= (1, 5));
        if has(xfixes::X11_EXTENSION_NAME)
            && conn
                .xfixes_query_version(5, 0)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some()
        {
            // Hear at once when a compositor starts or stops.
            let mask = xfixes::SelectionEventMask::SET_SELECTION_OWNER
                | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE;
            conn.xfixes_select_selection_input(root, cm, mask)
                .map_err(err)?;
        }
        // Under XWayland the screen is the Wayland compositor's: nothing it
        // draws shows up on the X root window.
        let xwayland = has("XWAYLAND") || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let damage = if !xwayland
            && has(damage::X11_EXTENSION_NAME)
            && conn
                .damage_query_version(1, 1)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some()
        {
            let id = conn.generate_id().map_err(err)?;
            conn.damage_create(id, root, damage::ReportLevel::BOUNDING_BOX)
                .map_err(err)?;
            Some(id)
        } else {
            None
        };
        let whole = Rect::new(
            0.0,
            0.0,
            f64::from(screen.width_in_pixels),
            f64::from(screen.height_in_pixels),
        );
        let mut s = Self {
            conn,
            root,
            atoms,
            cm,
            composited: false,
            plain: (
                screen.root_depth,
                screen.root_visual,
                screen.default_colormap,
            ),
            argb_visual,
            argb: false,
            screen: whole,
            main: whole,
            randr,
            damage,
            missed: 0,
            pending: VecDeque::new(),
            layers: HashMap::new(),
            hidden: false,
            idle_since: None,
            last_raise: Instant::now(),
            last_refresh: Instant::now(),
            grabs: [None; 2],
        };
        s.refresh();
        Ok(s)
    }

    /// Read the screen layout and whether a compositor runs, again:
    /// monitors come and go (RandR), compositors start and stop.
    fn refresh(&mut self) {
        let c = &self.conn;
        let geometry = c.get_geometry(self.root).ok();
        let monitors = if self.randr {
            c.randr_get_monitors(self.root, true).ok()
        } else {
            None
        };
        let owner = c.get_selection_owner(self.cm).ok();
        if let Some(g) = geometry.and_then(|c| c.reply().ok()) {
            self.screen = Rect::new(0.0, 0.0, f64::from(g.width), f64::from(g.height));
        }
        // The primary monitor (else the first), as the engine sees it.
        self.main = monitors
            .and_then(|c| c.reply().ok())
            .and_then(|r| {
                let m = r
                    .monitors
                    .iter()
                    .find(|m| m.primary)
                    .or(r.monitors.first())?;
                (m.width > 0 && m.height > 0).then(|| {
                    Rect::new(
                        f64::from(m.x),
                        f64::from(m.y),
                        f64::from(m.width),
                        f64::from(m.height),
                    )
                })
            })
            .unwrap_or(self.screen);
        if let Some(r) = owner.and_then(|c| c.reply().ok()) {
            self.composited = r.owner != x11rb::NONE;
        }
        // Without a compositor an ARGB window is drawn opaque (its clear
        // parts black); with one, a shaped window has jagged edges. When
        // that changes, build the windows again for the other visual.
        let argb = self.composited && self.argb_visual.is_some();
        if argb != self.argb {
            // (A compositor that just started may be reading the old ones.)
            let old = self.drop_windows(!self.composited);
            self.argb = argb;
            for (layer, w) in old.into_iter().filter(|(_, w)| w.shown) {
                self.show(layer, &w.img, f64::from(w.x), f64::from(w.y));
            }
        }
    }

    /// The keycode that produces `keysym` on this keyboard.
    fn keycode_for(&self, keysym: u32) -> Option<u8> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let map = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)
            .ok()?
            .reply()
            .ok()?;
        let per = usize::from(map.keysyms_per_keycode).max(1);
        map.keysyms
            .chunks(per)
            .position(|syms| syms.contains(&keysym))
            .map(|i| min + i as u8)
    }

    fn ungrab_hotkey(&mut self, which: Hotkey) {
        if let Some(g) = self.grabs[which.index()].take() {
            for lock in LOCKS {
                let _ = self
                    .conn
                    .ungrab_key(g.code, self.root, g.mods | ModMask::from(lock));
            }
            let _ = self.conn.flush();
        }
    }

    /// Which global key a key event with this keycode and modifier state is.
    fn grab_of(&self, code: u8, state: u16) -> Option<usize> {
        self.grabs.iter().position(|g| {
            g.is_some_and(|g| g.code == code && (state & MOD_BITS) == u16::from(g.mods))
        })
    }

    /// Depth, visual and colormap for new windows.
    fn visual(&self) -> (u8, u32, u32) {
        match self.argb_visual {
            Some((visual, cmap)) if self.argb => (32, visual, cmap),
            _ => self.plain,
        }
    }

    /// A new, unmapped window for `img`, with nothing set to show yet.
    fn create(&self, img: &Pixmap, x: i16, y: i16) -> Result<Win, String> {
        let c = &self.conn;
        let (depth, visual, colormap) = self.visual();
        let (w, h) = (img.width() as u16, img.height() as u16);
        let id = c.generate_id().map_err(err)?;
        c.create_window(
            depth,
            id,
            self.root,
            x,
            y,
            w.max(1),
            h.max(1),
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .border_pixel(0)
                .colormap(colormap),
        )
        .map_err(err)?;
        self.set_properties(id);
        // An empty input region: clicks go to whatever is underneath.
        c.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            ClipOrdering::UNSORTED,
            id,
            0,
            0,
            &[],
        )
        .map_err(err)?;
        let gc = c.generate_id().map_err(err)?;
        c.create_gc(gc, id, &CreateGCAux::new()).map_err(err)?;
        let pixmap = c.generate_id().map_err(err)?;
        c.create_pixmap(depth, pixmap, id, w.max(1), h.max(1))
            .map_err(err)?;
        Ok(Win {
            id,
            gc,
            pixmap,
            img: img.clone(),
            x,
            y,
            w: w.max(1),
            h: h.max(1),
            shape: Vec::new(),
            shown: false,
            visible: true,
            bg_image: false,
        })
    }

    /// What compositors and window rules go by. Override-redirect windows
    /// are never managed, so `_NET_WM_STATE` (above, skip taskbar/pager)
    /// would be a request nobody reads; taskbars only list managed windows.
    fn set_properties(&self, id: Window) {
        let (c, a) = (&self.conn, &self.atoms);
        let name = b"computer-use overlay";
        let _ = c.change_property8(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            name,
        );
        let _ = c.change_property8(PropMode::REPLACE, id, a._NET_WM_NAME, a.UTF8_STRING, name);
        let _ = c.change_property8(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"computer-use-overlay\0computer-use-overlay\0",
        );
        // Utility: KWin animates override-redirect windows unless they are
        // utility windows (other types — notification, DND — get its popup
        // fade), and the XWayland window managers of sway, Hyprland and
        // other wlroots compositors give keyboard focus to an
        // override-redirect window that has no type (or a normal one) when
        // it is mapped. GNOME Shell animates only normal windows, dialogs
        // and menus.
        let _ = c.change_property32(
            PropMode::REPLACE,
            id,
            a._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[a._NET_WM_WINDOW_TYPE_UTILITY],
        );
        for (prop, value) in [
            // Keep compositing while the overlay is up (2 = never bypass);
            // unredirected, an ARGB window shows its clear parts black.
            (a._NET_WM_BYPASS_COMPOSITOR, 2),
            // No shadow (picom, and compton before it).
            (a._COMPTON_SHADOW, 0),
            // No closing animation (KWin).
            (a._KDE_NET_WM_SKIP_CLOSE_ANIMATION, 1),
        ] {
            let _ = c.change_property32(PropMode::REPLACE, id, prop, AtomEnum::CARDINAL, &[value]);
        }
    }

    /// Pixels for this visual, and (without ARGB) the shape of opaque pixels.
    fn pixels(&self, img: &Pixmap) -> (Vec<u8>, Option<Vec<Rectangle>>) {
        if self.argb {
            return (draw::to_bgra_premultiplied(img), None);
        }
        let (data, mask) = draw::to_bgra_opaque(img);
        let w = img.width() as usize;
        let mut rects = Vec::new();
        for (y, row) in mask.chunks(w.max(1)).enumerate() {
            let mut x = 0;
            while x < row.len() {
                if row[x] {
                    let start = x;
                    while x < row.len() && row[x] {
                        x += 1;
                    }
                    rects.push(Rectangle {
                        x: start as i16,
                        y: y as i16,
                        width: (x - start) as u16,
                        height: 1,
                    });
                } else {
                    x += 1;
                }
            }
        }
        (data, Some(rects))
    }

    /// Upload pixels into the window's pixmap (not on screen until the
    /// window is repainted from it).
    fn put(&self, win: &Win, data: &[u8]) {
        let depth = self.visual().0;
        // Keep each request well under the X request size limit.
        let stride = usize::from(win.w) * 4;
        let rows = (60_000 / stride.max(1)).max(1);
        for (i, chunk) in data.chunks(stride * rows).enumerate() {
            let h = (chunk.len() / stride) as u16;
            let _ = self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                win.pixmap,
                win.gc,
                win.w,
                h,
                0,
                (i * rows) as i16,
                0,
                depth,
                chunk,
            );
        }
    }

    fn raise_all(&self) {
        let above = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        for w in self.layers.values().filter(|w| w.shown) {
            let _ = self.conn.configure_window(w.id, &above);
        }
    }

    fn sync(&self) {
        if let Ok(c) = self.conn.get_input_focus() {
            let _ = c.reply();
        }
    }

    /// Wait until the screen shows our latest changes to the windows at
    /// `rects`. The X server has made them once it answers a request; a
    /// compositor shows them in its next frame, and its drawing on the
    /// screen is damage to the root window. So wait (briefly) for root
    /// damage over all of each, from drawing done after the changes.
    fn settle(&mut self, rects: &[Rectangle]) {
        // (Only what is on the screen gets drawn.)
        let root = Rectangle {
            x: 0,
            y: 0,
            width: self.screen.width as u16,
            height: self.screen.height as u16,
        };
        let mut todo: Vec<Rectangle> = rects.iter().filter_map(|r| clip(*r, root)).collect();
        if !self.composited || todo.is_empty() {
            self.sync();
            return;
        }
        let Some(damage) = self.damage.filter(|_| self.missed < 3) else {
            // No way to watch it draw (XWayland: another program's screen).
            self.sync();
            std::thread::sleep(FRAME);
            return;
        };
        // Damage reported from here on comes from drawing done after the
        // changes (events carry the last request of ours the server had
        // handled).
        let Ok(after) = self
            .conn
            .damage_subtract(damage, x11rb::NONE, x11rb::NONE)
            .map(|c| c.sequence_number())
        else {
            return;
        };
        self.sync();
        let deadline = Instant::now() + SETTLE_MAX;
        loop {
            match self.conn.poll_for_event_with_sequence() {
                Ok(Some((Event::DamageNotify(e), seq))) if e.damage == damage => {
                    // Report the next drawing too.
                    let _ = self.conn.damage_subtract(damage, x11rb::NONE, x11rb::NONE);
                    let _ = self.conn.flush();
                    if seq >= after {
                        todo.retain(|r| !covers(e.area, *r));
                        if todo.is_empty() {
                            self.missed = 0;
                            return;
                        }
                    }
                }
                Ok(Some((ev, _))) => self.pending.push_back(ev),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(None) => {
                    self.missed += 1;
                    return;
                }
                Err(_) => return,
            }
        }
    }

    /// Destroy every window; returns them (for their images and places).
    /// A compositor re-reads a window's pixmap after its shape or size
    /// changes, and picom (v10) crashes when the window is gone by then: so
    /// unless the windows haven't changed for a while (`settled`), first
    /// wait until it has drawn them as they are now.
    fn drop_windows(&mut self, settled: bool) -> Vec<(Layer, Win)> {
        if !settled {
            let rects: Vec<Rectangle> = self.layers.values().map(Win::rect).collect();
            self.settle(&rects);
        }
        let old: Vec<(Layer, Win)> = self.layers.drain().collect();
        for (_, w) in &old {
            w.destroy(&self.conn);
        }
        let _ = self.conn.flush();
        old
    }

    fn next_event(&mut self) -> Option<Event> {
        if let Some(ev) = self.pending.pop_front() {
            return Some(ev);
        }
        match self.conn.poll_for_event() {
            Ok(ev) => ev,
            Err(e) => {
                // The X server is gone (it restarted): the windows and the
                // stop key's grab went with it. Exit with an error, so the
                // engine starts a new helper on the new server.
                eprintln!("overlay: lost the X server connection: {e}");
                std::process::exit(2);
            }
        }
    }
}

impl Surface for X11Surface {
    fn excluded_from_capture(&self) -> bool {
        false
    }

    fn screen(&self) -> Rect {
        self.screen
    }

    fn main_screen(&self) -> Rect {
        self.main
    }

    fn render_scale(&self) -> f32 {
        1.0
    }

    fn px_per_unit(&self) -> f32 {
        1.0
    }

    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
        let (x, y) = (x.round() as i16, y.round() as i16);
        let (w, h) = ((img.width() as u16).max(1), (img.height() as u16).max(1));
        let (mut win, fresh) = match self.layers.remove(&layer) {
            Some(win) => (win, false),
            None => match self.create(img, x, y) {
                Ok(win) => (win, true),
                Err(_) => return,
            },
        };
        if (win.w, win.h) != (w, h) {
            // A pixmap can't be resized: a new one. (The window keeps the
            // old one as its background until it is given the new one.)
            let Ok(pixmap) = self.conn.generate_id() else {
                self.layers.insert(layer, win);
                return;
            };
            let _ = self
                .conn
                .create_pixmap(self.visual().0, pixmap, win.id, w, h);
            let _ = self.conn.free_pixmap(win.pixmap);
            (win.pixmap, win.w, win.h, win.bg_image) = (pixmap, w, h, false);
        }
        let (data, shape) = self.pixels(img);
        self.put(&win, &data);
        if !fresh {
            win.img = img.clone();
        }
        if let Some(rects) = shape {
            win.shape = rects;
        }
        (win.x, win.y, win.shown) = (x, y, true);
        let geometry = ConfigureWindowAux::new()
            .x(i32::from(x))
            .y(i32::from(y))
            .width(u32::from(w))
            .height(u32::from(h))
            .stack_mode(StackMode::ABOVE);
        win.update(&self.conn, self.argb, self.hidden, Some(&geometry));
        if fresh {
            // Mapped with its image (or nothing) already set: the server
            // paints that, so there is no first frame of something else.
            let _ = self.conn.map_window(win.id);
        }
        self.layers.insert(layer, win);
        self.idle_since = None;
        let _ = self.conn.flush();
    }

    fn move_to(&mut self, layer: Layer, x: f64, y: f64) {
        if let Some(win) = self.layers.get_mut(&layer) {
            (win.x, win.y) = (x.round() as i16, y.round() as i16);
            let _ = self.conn.configure_window(
                win.id,
                &ConfigureWindowAux::new()
                    .x(i32::from(win.x))
                    .y(i32::from(win.y))
                    .stack_mode(StackMode::ABOVE),
            );
            let _ = self.conn.flush();
        }
    }

    fn hide(&mut self, layer: Layer) {
        if let Some(win) = self.layers.get_mut(&layer)
            && win.shown
        {
            win.shown = false;
            win.update(&self.conn, self.argb, self.hidden, None);
            let _ = self.conn.flush();
        }
        if self.idle_since.is_none() && self.layers.values().all(|w| !w.shown) {
            self.idle_since = Some(Instant::now());
        }
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        let mut left = Vec::new();
        for win in self.layers.values_mut() {
            if win.visible {
                left.push(win.rect());
            }
            win.update(&self.conn, self.argb, hidden, None);
        }
        if hidden {
            // The engine captures the screen as soon as we answer.
            self.settle(&left);
        } else {
            self.raise_all();
            let _ = self.conn.flush();
        }
    }

    fn set_hotkey(&mut self, which: Hotkey, combo: Option<KeyCombo>) -> bool {
        self.ungrab_hotkey(which);
        let Some(combo) = combo else {
            return false;
        };
        let Some(code) = crate::linux::x11::keysym_for(combo.key).and_then(|k| self.keycode_for(k))
        else {
            return false;
        };
        let m = combo.modifiers;
        let mut mods = 0u16;
        for (on, bit) in [
            (m.shift, 0x0001u16),
            (m.ctrl, 0x0004),
            (m.alt, 0x0008),
            (m.meta, 0x0040),
        ] {
            if on {
                mods |= bit;
            }
        }
        let mods = ModMask::from(mods);
        // A passive grab of exactly this combination: the X server reports
        // this key and nothing else. It fails if another program owns it.
        let mut ok = true;
        for lock in LOCKS {
            let grabbed = self
                .conn
                .grab_key(
                    false,
                    self.root,
                    mods | ModMask::from(lock),
                    code,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .map_err(err)
                .and_then(|c| c.check().map_err(err));
            if grabbed.is_err() {
                ok = false;
                break;
            }
        }
        self.grabs[which.index()] = Some(Grab {
            code,
            mods,
            down: false,
            pressed: 0,
            released: None,
        });
        if !ok {
            self.ungrab_hotkey(which);
        }
        let _ = self.conn.flush();
        ok
    }

    fn pump(&mut self) -> Vec<SurfaceEvent> {
        let mut events = Vec::new();
        let mut refresh = self.last_refresh.elapsed() >= REFRESH_EVERY;
        while let Some(ev) = self.next_event() {
            match ev {
                // A compositor started or stopped.
                Event::XfixesSelectionNotify(_) => refresh = true,
                Event::KeyPress(e) => {
                    let Some(i) = self.grab_of(e.detail, u16::from(e.state)) else {
                        continue;
                    };
                    let Some(g) = self.grabs[i].as_mut() else {
                        continue;
                    };
                    // Holding the key repeats it: only a fresh press counts.
                    // (A press long after the last one is new even if its
                    // release got lost.)
                    let repeat = (g.down && e.time.wrapping_sub(g.pressed) < 1000)
                        || g.released.is_some_and(|t| e.time.wrapping_sub(t) <= 1);
                    g.down = true;
                    g.pressed = e.time;
                    if !repeat {
                        events.push(SurfaceEvent::Hotkey(Hotkey::ALL[i]));
                    }
                }
                Event::KeyRelease(e) => {
                    // A release may come with the modifiers already up.
                    for g in self.grabs.iter_mut().flatten() {
                        if g.code == e.detail {
                            g.down = false;
                            g.released = Some(e.time);
                        }
                    }
                }
                _ => {}
            }
        }
        if refresh {
            self.last_refresh = Instant::now();
            self.refresh();
        }
        // Stay above windows raised since.
        if self.last_raise.elapsed().as_millis() >= 500 && !self.hidden {
            self.last_raise = Instant::now();
            self.raise_all();
            let _ = self.conn.flush();
        }
        if self.idle_since.is_some_and(|t| t.elapsed() >= IDLE_DROP) {
            self.idle_since = None;
            self.drop_windows(true);
        }
        events
    }

    fn close(&mut self) {
        for which in Hotkey::ALL {
            self.ungrab_hotkey(which);
        }
        // Just hidden at the end of the fade-out, most likely.
        self.drop_windows(false);
    }
}
