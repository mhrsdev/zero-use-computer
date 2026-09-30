//! X11 overlay: override-redirect windows (no window-manager decoration or
//! focus) whose input region is empty, so every click passes through to the
//! apps below. With a compositing manager the windows use a 32-bit ARGB
//! visual for smooth edges; without one, a SHAPE mask cuts them out. X11 has
//! no "exclude from capture", so the engine asks the helper to hide the
//! overlay for the instant of a screenshot. When this process dies, the X
//! server destroys its windows.

use std::collections::HashMap;
use std::time::Instant;

use tiny_skia::Pixmap;
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, GrabMode, ImageFormat, ModMask, Rectangle, StackMode, VisualClass,
    Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::draw;
use super::helper::{Ask, Layer, Surface, SurfaceEvent};
use super::text::Fonts;
use crate::keys::KeyCombo;
use crate::types::Rect;

struct Win {
    id: Window,
    gc: u32,
    w: u16,
    /// The image being shown, for Expose repaints.
    data: Vec<u8>,
    mapped: bool,
}

struct Panel {
    win: Win,
    id: u64,
    allow: (f32, f32, f32, f32),
    deny: (f32, f32, f32, f32),
}

pub struct X11Surface {
    conn: RustConnection,
    root: Window,
    screen: Rect,
    depth: u8,
    visual: u32,
    colormap: u32,
    argb: bool,
    layers: HashMap<Layer, Win>,
    panels: Vec<Panel>,
    hidden: bool,
    last_raise: Instant,
    fonts: Fonts,
    /// The stop key's passive grab: keycode and modifiers.
    hotkey: Option<(u8, ModMask)>,
    /// The stop key is held down (key repeat is not a new press), and when
    /// it last reported a press.
    hotkey_down: bool,
    hotkey_pressed: u32,
    /// When the stop key was last released (X sends release + press with
    /// the same time for each key repeat).
    hotkey_released: Option<u32>,
}

/// NumLock (Mod2) and CapsLock variants, so the stop key works whatever
/// state those locks are in.
const LOCKS: [u16; 4] = [0, 0x0002, 0x0010, 0x0012];

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl X11Surface {
    pub fn open() -> Result<Self, String> {
        let (conn, num) = x11rb::connect(None).map_err(err)?;
        if conn
            .extension_information(shape::X11_EXTENSION_NAME)
            .map_err(err)?
            .is_none()
        {
            return Err("the X server has no SHAPE extension (needed for click-through)".into());
        }
        let screen = conn.setup().roots[num].clone();
        let root = screen.root;

        // A compositing manager lets us use real transparency.
        let cm = conn
            .intern_atom(false, format!("_NET_WM_CM_S{num}").as_bytes())
            .map_err(err)?
            .reply()
            .map_err(err)?
            .atom;
        let composited = conn
            .get_selection_owner(cm)
            .map_err(err)?
            .reply()
            .map_err(err)?
            .owner
            != x11rb::NONE;
        let argb_visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id);

        let (depth, visual, colormap, argb) = match (composited, argb_visual) {
            (true, Some(v)) => {
                let cmap = conn.generate_id().map_err(err)?;
                conn.create_colormap(ColormapAlloc::NONE, cmap, root, v)
                    .map_err(err)?;
                (32, v, cmap, true)
            }
            _ => (
                screen.root_depth,
                screen.root_visual,
                screen.default_colormap,
                false,
            ),
        };

        Ok(Self {
            screen: Rect::new(
                0.0,
                0.0,
                f64::from(screen.width_in_pixels),
                f64::from(screen.height_in_pixels),
            ),
            conn,
            root,
            depth,
            visual,
            colormap,
            argb,
            layers: HashMap::new(),
            panels: Vec::new(),
            hidden: false,
            last_raise: Instant::now(),
            fonts: Fonts::default(),
            hotkey: None,
            hotkey_down: false,
            hotkey_pressed: 0,
            hotkey_released: None,
        })
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

    fn ungrab_hotkey(&mut self) {
        if let Some((code, mods)) = self.hotkey.take() {
            for lock in LOCKS {
                let _ = self
                    .conn
                    .ungrab_key(code, self.root, mods | ModMask::from(lock));
            }
            let _ = self.conn.flush();
        }
    }

    fn create(&self, x: i16, y: i16, w: u16, h: u16, input: bool) -> Result<Win, String> {
        let id = self.conn.generate_id().map_err(err)?;
        let mask = if input {
            EventMask::EXPOSURE | EventMask::BUTTON_PRESS
        } else {
            EventMask::EXPOSURE
        };
        self.conn
            .create_window(
                self.depth,
                id,
                self.root,
                x,
                y,
                w.max(1),
                h.max(1),
                0,
                WindowClass::INPUT_OUTPUT,
                self.visual,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .background_pixel(0)
                    .border_pixel(0)
                    .colormap(self.colormap)
                    .event_mask(mask),
            )
            .map_err(err)?;
        if !input {
            // An empty input region: clicks go to whatever is underneath.
            self.conn
                .shape_rectangles(
                    shape::SO::SET,
                    shape::SK::INPUT,
                    ClipOrdering::UNSORTED,
                    id,
                    0,
                    0,
                    &[],
                )
                .map_err(err)?;
        }
        let gc = self.conn.generate_id().map_err(err)?;
        self.conn
            .create_gc(gc, id, &CreateGCAux::new())
            .map_err(err)?;
        Ok(Win {
            id,
            gc,
            w: w.max(1),
            data: Vec::new(),
            mapped: false,
        })
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

    fn put(&self, win: &Win) {
        if win.data.is_empty() {
            return;
        }
        // Keep each request well under the X request size limit.
        let stride = usize::from(win.w) * 4;
        let rows = (60_000 / stride.max(1)).max(1);
        for (i, chunk) in win.data.chunks(stride * rows).enumerate() {
            let h = (chunk.len() / stride) as u16;
            let _ = self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                win.id,
                win.gc,
                win.w,
                h,
                0,
                (i * rows) as i16,
                0,
                self.depth,
                chunk,
            );
        }
    }

    fn destroy_panels(&mut self) {
        for p in self.panels.drain(..) {
            let _ = self.conn.destroy_window(p.win.id);
        }
        let _ = self.conn.flush();
    }

    fn raise_all(&self) {
        let above = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        for w in self.layers.values().filter(|w| w.mapped) {
            let _ = self.conn.configure_window(w.id, &above);
        }
        for p in &self.panels {
            let _ = self.conn.configure_window(p.win.id, &above);
        }
    }

    fn sync(&self) {
        if let Ok(c) = self.conn.get_input_focus() {
            let _ = c.reply();
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

    fn render_scale(&self) -> f32 {
        1.0
    }

    fn px_per_unit(&self) -> f32 {
        1.0
    }

    fn translucent(&self) -> bool {
        self.argb
    }

    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
        let (w, h) = (img.width() as u16, img.height() as u16);
        let (x, y) = (x.round() as i16, y.round() as i16);
        let (data, shape) = self.pixels(img);
        let win = match self.layers.remove(&layer) {
            Some(win) => {
                let _ = self.conn.configure_window(
                    win.id,
                    &ConfigureWindowAux::new()
                        .x(i32::from(x))
                        .y(i32::from(y))
                        .width(u32::from(w.max(1)))
                        .height(u32::from(h.max(1)))
                        .stack_mode(StackMode::ABOVE),
                );
                Win { w: w.max(1), ..win }
            }
            None => match self.create(x, y, w, h, false) {
                Ok(win) => win,
                Err(_) => return,
            },
        };
        if let Some(rects) = shape {
            let _ = self.conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                ClipOrdering::UNSORTED,
                win.id,
                0,
                0,
                &rects,
            );
        }
        let mut win = Win { data, ..win };
        if !win.mapped && !self.hidden {
            let _ = self.conn.map_window(win.id);
        }
        win.mapped = true;
        if !self.hidden {
            self.put(&win);
        }
        self.layers.insert(layer, win);
        let _ = self.conn.flush();
    }

    fn move_to(&mut self, layer: Layer, x: f64, y: f64) {
        if let Some(win) = self.layers.get(&layer) {
            let _ = self.conn.configure_window(
                win.id,
                &ConfigureWindowAux::new()
                    .x(x.round() as i32)
                    .y(y.round() as i32)
                    .stack_mode(StackMode::ABOVE),
            );
            let _ = self.conn.flush();
        }
    }

    fn hide(&mut self, layer: Layer) {
        if let Some(win) = self.layers.get_mut(&layer)
            && win.mapped
        {
            let _ = self.conn.unmap_window(win.id);
            win.mapped = false;
            let _ = self.conn.flush();
        }
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        // Confirmation panels too: nothing of ours may be in a screenshot.
        let wins = || {
            self.layers
                .values()
                .filter(|w| w.mapped)
                .chain(self.panels.iter().map(|p| &p.win))
        };
        for win in wins() {
            if hidden {
                let _ = self.conn.unmap_window(win.id);
            } else {
                let _ = self.conn.map_window(win.id);
            }
        }
        if !hidden {
            for win in wins() {
                self.put(win);
            }
            self.raise_all();
        }
        // Make sure the server has done it before we answer.
        self.sync();
    }

    fn confirm(&mut self, id: u64, ask: &Ask) {
        if self.fonts.is_empty() {
            self.fonts = Fonts::load("");
        }
        let accent = draw::parse_color("#FFE600").unwrap_or(tiny_skia::Color::WHITE);
        let (img, [allow_r, deny_r]) = draw::panel(
            &self.fonts,
            &ask.title,
            &ask.message,
            &ask.allow,
            &ask.deny,
            accent,
            self.render_scale(),
        );
        let (w, h) = (img.width() as u16, img.height() as u16);
        let x = (self.screen.x + (self.screen.width - f64::from(w)) / 2.0) as i16;
        let y = (self.screen.y + (self.screen.height - f64::from(h)) / 3.0) as i16;
        let Ok(win) = self.create(x, y, w, h, true) else {
            return;
        };
        let (data, shape) = self.pixels(&img);
        if let Some(rects) = shape {
            let _ = self.conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                ClipOrdering::UNSORTED,
                win.id,
                0,
                0,
                &rects,
            );
        }
        let mut win = Win { data, ..win };
        if !self.hidden {
            let _ = self.conn.map_window(win.id);
            self.put(&win);
        }
        win.mapped = true;
        let _ = self.conn.configure_window(
            win.id,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        );
        let _ = self.conn.flush();
        self.panels.push(Panel {
            win,
            id,
            allow: allow_r,
            deny: deny_r,
        });
    }

    fn dismiss(&mut self) {
        self.destroy_panels();
    }

    fn set_hotkey(&mut self, combo: Option<KeyCombo>) -> bool {
        self.ungrab_hotkey();
        self.hotkey_down = false;
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
        self.hotkey = Some((code, mods));
        if !ok {
            self.ungrab_hotkey();
        }
        let _ = self.conn.flush();
        ok
    }

    fn pump(&mut self) -> Vec<SurfaceEvent> {
        let mut answers = Vec::new();
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            match ev {
                Event::KeyPress(e) if self.hotkey.is_some_and(|(code, _)| code == e.detail) => {
                    // Holding the key repeats it: only a fresh press counts.
                    // (A press long after the last one is new even if its
                    // release got lost.)
                    let repeat = (self.hotkey_down
                        && e.time.wrapping_sub(self.hotkey_pressed) < 1000)
                        || self
                            .hotkey_released
                            .is_some_and(|t| e.time.wrapping_sub(t) <= 1);
                    self.hotkey_down = true;
                    self.hotkey_pressed = e.time;
                    if !repeat {
                        answers.push(SurfaceEvent::Hotkey);
                    }
                }
                Event::KeyRelease(e) if self.hotkey.is_some_and(|(code, _)| code == e.detail) => {
                    self.hotkey_down = false;
                    self.hotkey_released = Some(e.time);
                }
                Event::Expose(e) if e.count == 0 => {
                    if let Some(w) = self.layers.values().find(|w| w.id == e.window) {
                        self.put(w);
                    } else if let Some(p) = self.panels.iter().find(|p| p.win.id == e.window) {
                        self.put(&p.win);
                    }
                }
                Event::ButtonPress(e) => {
                    let Some(i) = self.panels.iter().position(|p| p.win.id == e.event) else {
                        continue;
                    };
                    let p = &self.panels[i];
                    let (x, y) = (f32::from(e.event_x), f32::from(e.event_y));
                    let hit = |r: (f32, f32, f32, f32)| {
                        x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3
                    };
                    let answer = if hit(p.allow) {
                        Some(true)
                    } else if hit(p.deny) {
                        Some(false)
                    } else {
                        None
                    };
                    if let Some(ok) = answer {
                        let p = self.panels.remove(i);
                        let _ = self.conn.destroy_window(p.win.id);
                        let _ = self.conn.flush();
                        answers.push(SurfaceEvent::Answer(p.id, ok));
                    }
                }
                _ => {}
            }
        }
        // Stay above windows raised since.
        if self.last_raise.elapsed().as_millis() >= 500 && !self.hidden {
            self.last_raise = Instant::now();
            self.raise_all();
            let _ = self.conn.flush();
        }
        answers
    }

    fn close(&mut self) {
        self.ungrab_hotkey();
        for w in self.layers.values() {
            let _ = self.conn.destroy_window(w.id);
        }
        self.layers.clear();
        self.destroy_panels();
    }
}
