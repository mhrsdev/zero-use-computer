//! X11 synthesized input (XTest) and window capture (GetImage), the fallback
//! path when the accessibility API can't do something directly.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _, ImageFormat};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, Modifiers, NamedKey};
use crate::types::{Capture, Rect};

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;

pub struct X11 {
    conn: RustConnection,
    root: xproto::Window,
    root_w: u16,
    root_h: u16,
    /// keysym -> (keycode, needs_shift)
    keymap: HashMap<u32, (u8, bool)>,
    shift: u8,
    ctrl: u8,
    alt: u8,
    meta: u8,
    /// A keycode with no keysyms we can rebind on the fly for characters not
    /// on the keyboard (0 when every keycode is in use).
    spare: u8,
    /// The keysym currently bound to `spare` (never stored in `keymap`, so a
    /// rebind can't leave a stale entry behind).
    spare_sym: Option<u32>,
    /// When the spare keycode was last pressed, so it isn't rebound before
    /// the focused client has translated that press.
    spare_pressed: Option<Instant>,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    /// Put the pointer back after synthesized mouse input.
    pub restore_pointer: bool,
}

impl X11 {
    pub fn connect() -> Result<Self> {
        let (conn, screen_num) = x11rb::connect(None)
            .map_err(|e| Error::Platform(format!("cannot connect to the X server: {e}")))?;
        // XTest must be present for synthetic input.
        conn.xtest_get_version(2, 2)
            .map_err(|e| Error::Platform(format!("XTest query failed: {e}")))?
            .reply()
            .map_err(|e| Error::Platform(format!("XTest extension unavailable: {e}")))?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let (root_w, root_h) = (screen.width_in_pixels, screen.height_in_pixels);

        // Pixel layout for the root visual.
        let (mut red_mask, mut green_mask, mut blue_mask) = (0xff0000u32, 0x00ff00, 0x0000ff);
        for depth in &screen.allowed_depths {
            for v in &depth.visuals {
                if v.visual_id == screen.root_visual {
                    red_mask = v.red_mask;
                    green_mask = v.green_mask;
                    blue_mask = v.blue_mask;
                }
            }
        }

        let mut x = Self {
            conn,
            root,
            root_w,
            root_h,
            keymap: HashMap::new(),
            shift: 0,
            ctrl: 0,
            alt: 0,
            meta: 0,
            spare: 0,
            spare_sym: None,
            spare_pressed: None,
            red_mask,
            green_mask,
            blue_mask,
            restore_pointer: true,
        };
        x.load_keymap()?;
        Ok(x)
    }

    fn load_keymap(&mut self) -> Result<()> {
        let setup = self.conn.setup();
        let min = setup.min_keycode;
        let max = setup.max_keycode;
        let count = max - min + 1;
        let mapping = self
            .conn
            .get_keyboard_mapping(min, count)
            .map_err(xe)?
            .reply()
            .map_err(xe)?;
        let per = (mapping.keysyms_per_keycode as usize).max(1);
        for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
            let keycode = min + i as u8;
            let base = chunk.first().copied().unwrap_or(0);
            let shifted = chunk.get(1).copied().unwrap_or(0);
            if base != 0 {
                self.keymap.entry(base).or_insert((keycode, false));
            }
            if shifted != 0 && shifted != base {
                self.keymap.entry(shifted).or_insert((keycode, true));
            }
        }
        // Modifier keycodes by keysym.
        self.shift = self.keymap.get(&0xffe1).map(|k| k.0).unwrap_or(0); // Shift_L
        self.ctrl = self.keymap.get(&0xffe3).map(|k| k.0).unwrap_or(0); // Control_L
        self.alt = self
            .keymap
            .get(&0xffe9)
            .or_else(|| self.keymap.get(&0xff7e)) // Alt_L / ISO_Level3? fall back
            .map(|k| k.0)
            .unwrap_or(0);
        self.meta = self.keymap.get(&0xffeb).map(|k| k.0).unwrap_or(0); // Super_L
        // A spare keycode with no keysym at all, for remapping arbitrary
        // chars; never a real key (0 = none, typing such chars then fails).
        self.spare = find_spare(&mapping.keysyms, per, min).unwrap_or(0);
        Ok(())
    }

    fn fake(&self, type_: u8, detail: u8, x: i16, y: i16) -> Result<()> {
        self.conn
            .xtest_fake_input(type_, detail, 0, self.root, x, y, 0)
            .map_err(xe)?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        self.conn.flush().map_err(xe)?;
        Ok(())
    }

    fn warp(&self, x: i32, y: i32) -> Result<()> {
        self.conn
            .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, x as i16, y as i16)
            .map_err(xe)?;
        Ok(())
    }

    /// Where the user's pointer is, when it should be put back afterwards.
    fn pointer(&self) -> Option<(i32, i32)> {
        if !self.restore_pointer {
            return None;
        }
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((i32::from(r.root_x), i32::from(r.root_y)))
    }

    /// Return the pointer to where the user left it.
    fn put_back(&self, at: Option<(i32, i32)>) -> Result<()> {
        if let Some((x, y)) = at {
            self.warp(x, y)?;
            self.fake(6, 0, x as i16, y as i16)?;
        }
        Ok(())
    }

    pub fn click(&self, x: i32, y: i32, button: u8, count: u8) -> Result<()> {
        let home = self.pointer();
        self.warp(x, y)?;
        self.fake(6, 0, x as i16, y as i16)?; // MotionNotify absolute
        for _ in 0..count.max(1) {
            self.fake(BUTTON_PRESS, button, x as i16, y as i16)?;
            self.fake(BUTTON_RELEASE, button, x as i16, y as i16)?;
        }
        self.put_back(home)?;
        self.flush()
    }

    pub fn scroll(&self, x: i32, y: i32, dx: i32, dy: i32) -> Result<()> {
        let home = self.pointer();
        self.warp(x, y)?;
        let tick = |button: u8, n: i32| -> Result<()> {
            for _ in 0..n.abs() {
                self.fake(BUTTON_PRESS, button, x as i16, y as i16)?;
                self.fake(BUTTON_RELEASE, button, x as i16, y as i16)?;
            }
            Ok(())
        };
        if dy > 0 {
            tick(5, dy)?; // down
        } else if dy < 0 {
            tick(4, dy)?; // up
        }
        if dx > 0 {
            tick(7, dx)?; // right
        } else if dx < 0 {
            tick(6, dx)?; // left
        }
        self.put_back(home)?;
        self.flush()
    }

    pub fn drag(&self, from: (i32, i32), to: (i32, i32)) -> Result<()> {
        let home = self.pointer();
        // Paced like a real drag: toolkits that start a drag on a motion
        // threshold or a timer miss a single burst of events.
        let pause = || -> Result<()> {
            self.flush()?;
            std::thread::sleep(DRAG_STEP);
            Ok(())
        };
        self.warp(from.0, from.1)?;
        self.fake(6, 0, from.0 as i16, from.1 as i16)?;
        pause()?;
        self.fake(BUTTON_PRESS, 1, from.0 as i16, from.1 as i16)?;
        pause()?;
        // A few intermediate motions so drag-aware widgets follow.
        for step in 1..=8 {
            let x = from.0 + (to.0 - from.0) * step / 8;
            let y = from.1 + (to.1 - from.1) * step / 8;
            self.warp(x, y)?;
            self.fake(6, 0, x as i16, y as i16)?;
            pause()?;
        }
        self.fake(BUTTON_RELEASE, 1, to.0 as i16, to.1 as i16)?;
        pause()?;
        self.put_back(home)?;
        self.flush()
    }

    /// Type a run of text as key events.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        let typed = text_keys(text).try_for_each(|key| {
            self.press_one(&KeyCombo {
                modifiers: Modifiers::default(),
                key,
            })
        });
        // Put the spare keycode back even when a key failed.
        let restored = self.restore_spare();
        typed.and(restored)
    }

    pub fn press(&mut self, combo: &KeyCombo) -> Result<()> {
        let pressed = self.press_one(combo);
        let restored = self.restore_spare();
        pressed.and(restored)
    }

    fn press_one(&mut self, combo: &KeyCombo) -> Result<()> {
        let (keycode, shift_from_key) = self.resolve_key(combo.key)?;
        let m = combo.modifiers;
        let mut down: Vec<u8> = Vec::new();
        if (m.shift || shift_from_key) && self.shift != 0 {
            down.push(self.shift);
        }
        if m.ctrl && self.ctrl != 0 {
            down.push(self.ctrl);
        }
        if m.alt && self.alt != 0 {
            down.push(self.alt);
        }
        if m.meta && self.meta != 0 {
            down.push(self.meta);
        }
        for kc in &down {
            self.fake(KEY_PRESS, *kc, 0, 0)?;
        }
        self.fake(KEY_PRESS, keycode, 0, 0)?;
        self.fake(KEY_RELEASE, keycode, 0, 0)?;
        for kc in down.iter().rev() {
            self.fake(KEY_RELEASE, *kc, 0, 0)?;
        }
        self.flush()?;
        if keycode == self.spare {
            self.spare_pressed = Some(Instant::now());
        }
        Ok(())
    }

    /// Returns (keycode, needs_shift), remapping the spare keycode if needed.
    fn resolve_key(&mut self, key: Key) -> Result<(u8, bool)> {
        let keysym =
            keysym_for(key).ok_or_else(|| Error::ActionFailed(format!("no keysym for {key:?}")))?;
        match plan_key(&self.keymap, self.spare, self.spare_sym, keysym) {
            KeyPlan::Mapped(kc, shift) => Ok((kc, shift)),
            KeyPlan::Spare => Ok((self.spare, false)),
            KeyPlan::Rebind => {
                self.bind_spare(keysym)?;
                Ok((self.spare, false))
            }
            KeyPlan::NoSpare => Err(Error::ActionFailed(
                "no free keycode to type this character".into(),
            )),
        }
    }

    /// Bind the spare keycode to `keysym` (0 = unbind), letting the last
    /// press on it be translated first and clients see the new mapping
    /// (MappingNotify) before the next press.
    fn bind_spare(&mut self, keysym: u32) -> Result<()> {
        if let Some(t) = self.spare_pressed.take() {
            let since = t.elapsed();
            if since < REMAP_SETTLE {
                std::thread::sleep(REMAP_SETTLE - since);
            }
        }
        // Forget the old binding first: if the change fails, nothing claims
        // the spare keycode still types the previous character.
        self.spare_sym = None;
        let syms = [keysym, keysym];
        // .check() round-trips so the mapping is live before the fake key event.
        self.conn
            .change_keyboard_mapping(1, self.spare, 2, &syms)
            .map_err(xe)?
            .check()
            .map_err(xe)?;
        self.flush()?;
        if keysym != 0 {
            std::thread::sleep(REMAP_SETTLE);
            self.spare_sym = Some(keysym);
        }
        Ok(())
    }

    /// Unbind the spare keycode after use, restoring the original mapping.
    fn restore_spare(&mut self) -> Result<()> {
        if self.spare_sym.is_none() {
            return Ok(());
        }
        self.bind_spare(0)
    }

    /// Capture a screen rectangle as RGBA.
    /// Pid of the window manager's active window (`_NET_ACTIVE_WINDOW` +
    /// `_NET_WM_PID`), when a window manager provides them.
    pub fn active_pid(&self) -> Option<u32> {
        let atom = |name: &str| -> Option<u32> {
            self.conn
                .intern_atom(true, name.as_bytes())
                .ok()?
                .reply()
                .ok()
                .map(|r| r.atom)
                .filter(|a| *a != 0)
        };
        let active = atom("_NET_ACTIVE_WINDOW")?;
        let wm_pid = atom("_NET_WM_PID")?;
        let win = self
            .conn
            .get_property(false, self.root, active, xproto::AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()?;
        if win == 0 {
            return None;
        }
        self.conn
            .get_property(false, win, wm_pid, xproto::AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
    }

    /// Time since the last mouse or keyboard input on this display (the
    /// MIT-SCREEN-SAVER idle counter; synthesized input counts too). Only
    /// the time is read, never the input itself.
    pub fn idle(&self) -> Option<std::time::Duration> {
        use x11rb::protocol::screensaver::ConnectionExt as _;
        let info = self
            .conn
            .screensaver_query_info(self.root)
            .ok()?
            .reply()
            .ok()?;
        Some(std::time::Duration::from_millis(u64::from(
            info.ms_since_user_input,
        )))
    }

    /// Window management on this display.
    pub fn wm(&self) -> super::wm::Wm<'_> {
        super::wm::Wm::new(&self.conn, self.root)
    }

    /// The root window's current size (it changes with RandR resizes and
    /// monitor hotplug), falling back to the size at connect time.
    fn root_size(&self) -> (u16, u16) {
        self.conn
            .get_geometry(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|g| (g.width, g.height))
            .unwrap_or((self.root_w, self.root_h))
    }

    /// The full screen rectangle (the root window's size).
    pub fn root_rect(&self) -> Rect {
        let (w, h) = self.root_size();
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h))
    }

    pub fn capture(&self, rect: Rect) -> Result<Capture> {
        let (root_w, root_h) = self.root_size();
        let (x, y, w, h) = clip_to_root(rect, root_w, root_h).ok_or_else(|| {
            Error::ActionFailed(format!(
                "the capture area ({:.0},{:.0} {:.0}x{:.0}) is off-screen (screen is {root_w}x{root_h})",
                rect.x, rect.y, rect.width, rect.height
            ))
        })?;
        let img = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, x, y, w, h, !0)
            .map_err(xe)?
            .reply()
            .map_err(|e| Error::Platform(format!("GetImage failed: {e}")))?;

        let setup = self.conn.setup();
        let format = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == img.depth)
            .ok_or_else(|| {
                Error::Unsupported(format!("no pixmap format for depth {}", img.depth))
            })?;
        let layout = PixelLayout {
            bits_per_pixel: format.bits_per_pixel,
            pad: format.scanline_pad,
            msb_first: setup.image_byte_order == xproto::ImageOrder::MSB_FIRST,
            masks: [self.red_mask, self.green_mask, self.blue_mask],
        };
        let rgba = to_rgba(img.data, w.into(), h.into(), &layout)?;
        Ok(Capture {
            width: w as u32,
            height: h as u32,
            rgba,
            bounds: Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h)),
        })
    }
}

/// Pace of synthesized drag steps.
const DRAG_STEP: Duration = Duration::from_millis(12);
/// Time for clients to act on a keyboard remap (MappingNotify) and to
/// translate a press on the spare keycode before it is rebound.
const REMAP_SETTLE: Duration = Duration::from_millis(30);

/// The keys that type `text`: line breaks and tabs are Return and Tab
/// (a `\r\n` pair is one Return), everything else its character.
fn text_keys(text: &str) -> impl Iterator<Item = Key> + '_ {
    let mut chars = text.chars().peekable();
    std::iter::from_fn(move || {
        let c = chars.next()?;
        Some(match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                Key::Named(NamedKey::Return)
            }
            '\n' => Key::Named(NamedKey::Return),
            '\t' => Key::Named(NamedKey::Tab),
            c => Key::Char(c),
        })
    })
}

/// A keycode none of whose keysyms is set.
fn find_spare(keysyms: &[u32], per: usize, min: u8) -> Option<u8> {
    keysyms
        .chunks(per.max(1))
        .position(|chunk| chunk.iter().all(|s| *s == 0))
        .and_then(|i| u8::try_from(usize::from(min) + i).ok())
}

#[derive(Debug, PartialEq, Eq)]
enum KeyPlan {
    /// A real key types it: (keycode, needs_shift).
    Mapped(u8, bool),
    /// The spare keycode is already bound to it.
    Spare,
    /// The spare keycode must be rebound to it.
    Rebind,
    /// Not on the keyboard and no spare keycode.
    NoSpare,
}

fn plan_key(
    keymap: &HashMap<u32, (u8, bool)>,
    spare: u8,
    spare_sym: Option<u32>,
    keysym: u32,
) -> KeyPlan {
    if let Some((kc, sh)) = keymap.get(&keysym) {
        KeyPlan::Mapped(*kc, *sh)
    } else if spare == 0 {
        KeyPlan::NoSpare
    } else if spare_sym == Some(keysym) {
        KeyPlan::Spare
    } else {
        KeyPlan::Rebind
    }
}

/// The part of `rect` on a `root_w`×`root_h` screen, as GetImage arguments,
/// or `None` when nothing of it is on-screen.
fn clip_to_root(rect: Rect, root_w: u16, root_h: u16) -> Option<(i16, i16, u16, u16)> {
    let finite = [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite());
    if !finite {
        return None;
    }
    let x0 = rect.x.round().max(0.0);
    let y0 = rect.y.round().max(0.0);
    let x1 = (rect.x + rect.width).round().min(f64::from(root_w));
    let y1 = (rect.y + rect.height).round().min(f64::from(root_h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as i16, y0 as i16, (x1 - x0) as u16, (y1 - y0) as u16))
}

/// How a Z-pixmap image is laid out.
struct PixelLayout {
    bits_per_pixel: u8,
    /// Scanline padding in bits.
    pad: u8,
    msb_first: bool,
    /// Red, green and blue masks of the visual.
    masks: [u32; 3],
}

/// Convert a Z-pixmap image (any of 16/24/32 bits per pixel, with scanline
/// padding) to tightly packed RGBA.
fn to_rgba(mut data: Vec<u8>, w: usize, h: usize, l: &PixelLayout) -> Result<Vec<u8>> {
    let bytes = match l.bits_per_pixel {
        16 => 2,
        24 => 3,
        32 => 4,
        other => {
            return Err(Error::Unsupported(format!(
                "screen capture needs a 16, 24 or 32 bits-per-pixel display (this one is {other})"
            )));
        }
    };
    let pad = usize::from(l.pad.max(8));
    let stride = (w * bytes * 8).div_ceil(pad) * pad / 8;
    if data.len() < stride * h.saturating_sub(1) + w * bytes {
        return Err(Error::Platform("GetImage returned a short image".into()));
    }
    let shifts = l.masks.map(|m| m.trailing_zeros().min(31));
    let maxes = l.masks.map(|m| (m >> m.trailing_zeros().min(31)).max(1));
    let channel = |word: u32, i: usize| -> u8 {
        let v = (word & l.masks[i]) >> shifts[i];
        if maxes[i] == 0xff {
            v as u8
        } else {
            (u64::from(v) * 255 / u64::from(maxes[i])) as u8
        }
    };
    let read = |px: &[u8]| -> u32 {
        if l.msb_first {
            px.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
        } else {
            px.iter()
                .rev()
                .fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
        }
    };
    if bytes == 4 && stride == w * 4 {
        // Common case: convert in place in the reply buffer (no second copy).
        data.truncate(w * h * 4);
        for px in data.as_chunks_mut::<4>().0 {
            let word = read(px);
            px[0] = channel(word, 0);
            px[1] = channel(word, 1);
            px[2] = channel(word, 2);
            px[3] = 255;
        }
        return Ok(data);
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let line = &data[row * stride..row * stride + w * bytes];
        for px in line.chunks_exact(bytes) {
            let word = read(px);
            rgba.extend_from_slice(&[channel(word, 0), channel(word, 1), channel(word, 2), 255]);
        }
    }
    Ok(rgba)
}

pub(crate) fn keysym_for(key: Key) -> Option<u32> {
    Some(match key {
        // Control characters have function-key keysyms, not Latin-1 ones.
        Key::Char('\t') => 0xff09,
        Key::Char('\n' | '\r') => 0xff0d,
        Key::Char('\u{8}') => 0xff08,
        Key::Char('\u{1b}') => 0xff1b,
        Key::Char('\u{7f}') => 0xffff,
        Key::Char(c) if (c as u32) < 0x20 => return None,
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
            NamedKey::F(n) => 0xffbe + (u32::from(n) - 1),
        },
    })
}

fn xe(e: impl std::fmt::Display) -> Error {
    Error::Platform(format!("X11: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_inside_partial_and_off_screen() {
        let r = |x, y, w, h| Rect::new(x, y, w, h);
        assert_eq!(
            clip_to_root(r(10.0, 20.0, 100.0, 50.0), 1920, 1080),
            Some((10, 20, 100, 50))
        );
        // Hangs off the right/bottom edge and the top-left corner.
        assert_eq!(
            clip_to_root(r(1900.0, 1000.0, 100.0, 200.0), 1920, 1080),
            Some((1900, 1000, 20, 80))
        );
        assert_eq!(
            clip_to_root(r(-50.0, -10.0, 100.0, 30.0), 1920, 1080),
            Some((0, 0, 50, 20))
        );
        // Entirely off-screen: an error, not a panic.
        assert_eq!(
            clip_to_root(r(2000.0, 10.0, 100.0, 100.0), 1920, 1080),
            None
        );
        assert_eq!(clip_to_root(r(1920.0, 0.0, 10.0, 10.0), 1920, 1080), None);
        assert_eq!(clip_to_root(r(-200.0, 0.0, 100.0, 10.0), 1920, 1080), None);
        assert_eq!(clip_to_root(r(0.0, 0.0, 0.0, 10.0), 1920, 1080), None);
        assert_eq!(clip_to_root(r(f64::NAN, 0.0, 10.0, 10.0), 1920, 1080), None);
    }

    #[test]
    fn control_chars_type_tab_and_return() {
        let keys: Vec<Key> = text_keys("a\tb\r\nc\nd\re").collect();
        let ret = Key::Named(NamedKey::Return);
        assert_eq!(
            keys,
            vec![
                Key::Char('a'),
                Key::Named(NamedKey::Tab),
                Key::Char('b'),
                ret,
                Key::Char('c'),
                ret,
                Key::Char('d'),
                ret,
                Key::Char('e'),
            ]
        );
        assert_eq!(keysym_for(Key::Char('\t')), Some(0xff09));
        assert_eq!(keysym_for(Key::Char('\r')), Some(0xff0d));
        assert_eq!(keysym_for(Key::Char('\u{1}')), None);
        assert_eq!(keysym_for(Key::Char('é')), Some(0xe9));
        assert_eq!(keysym_for(Key::Char('€')), Some(0x0100_20ac));
    }

    #[test]
    fn spare_keycode_is_never_a_real_key() {
        // 3 keycodes from 8, 4 keysyms each; only 10 is completely empty
        // (9 has a keysym in its second group only).
        let syms = [0x61, 0x41, 0, 0, 0, 0, 0x6c6, 0, 0, 0, 0, 0];
        assert_eq!(find_spare(&syms, 4, 8), Some(10));
        assert_eq!(find_spare(&syms[..8], 4, 8), None);
    }

    #[test]
    fn rebinding_the_spare_never_reuses_a_stale_char() {
        let mut keymap = HashMap::new();
        keymap.insert(0x61, (38, false));
        let (e_acute, euro) = (0xe9, 0x0100_20ac);
        // "é€é" on a keyboard without either: every change of character
        // rebinds, a repeat doesn't.
        let mut bound = None;
        let mut plans = Vec::new();
        for sym in [e_acute, euro, e_acute, e_acute] {
            let p = plan_key(&keymap, 200, bound, sym);
            if p == KeyPlan::Rebind {
                bound = Some(sym);
            }
            plans.push(p);
        }
        assert_eq!(
            plans,
            vec![
                KeyPlan::Rebind,
                KeyPlan::Rebind,
                KeyPlan::Rebind,
                KeyPlan::Spare
            ]
        );
        assert_eq!(
            plan_key(&keymap, 200, bound, 0x61),
            KeyPlan::Mapped(38, false)
        );
        assert_eq!(plan_key(&keymap, 0, None, euro), KeyPlan::NoSpare);
    }

    fn layout(bpp: u8, pad: u8, masks: [u32; 3]) -> PixelLayout {
        PixelLayout {
            bits_per_pixel: bpp,
            pad,
            msb_first: false,
            masks,
        }
    }

    #[test]
    fn converts_32bpp_in_place() {
        // BGRX little-endian: red in bits 16..24.
        let data = vec![0x30, 0x20, 0x10, 0, 0x03, 0x02, 0x01, 0];
        let out = to_rgba(data, 2, 1, &layout(32, 32, [0xff0000, 0xff00, 0xff])).unwrap();
        assert_eq!(out, vec![0x10, 0x20, 0x30, 255, 0x01, 0x02, 0x03, 255]);
    }

    #[test]
    fn converts_24bpp_with_scanline_padding() {
        // 1 pixel per row = 3 bytes, padded to 4.
        let data = vec![0x30, 0x20, 0x10, 0xee, 0x03, 0x02, 0x01, 0xee];
        let out = to_rgba(data, 1, 2, &layout(24, 32, [0xff0000, 0xff00, 0xff])).unwrap();
        assert_eq!(out, vec![0x10, 0x20, 0x30, 255, 0x01, 0x02, 0x03, 255]);
    }

    #[test]
    fn converts_16bpp_rgb565() {
        // White and pure red in RGB565, little-endian.
        let data = vec![0xff, 0xff, 0x00, 0xf8];
        let out = to_rgba(data, 2, 1, &layout(16, 32, [0xf800, 0x07e0, 0x001f])).unwrap();
        assert_eq!(out, vec![255, 255, 255, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn rejects_other_depths_and_short_images() {
        assert!(matches!(
            to_rgba(vec![0; 8], 8, 1, &layout(8, 32, [0xe0, 0x1c, 0x3])),
            Err(Error::Unsupported(_))
        ));
        assert!(to_rgba(vec![0; 4], 2, 1, &layout(32, 32, [0xff0000, 0xff00, 0xff])).is_err());
    }
}
