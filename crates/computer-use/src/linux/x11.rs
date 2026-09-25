//! X11 synthesized input (XTest) and window capture (GetImage), the fallback
//! path when the accessibility API can't do something directly.

use std::collections::HashMap;

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
    /// A keycode we can rebind on the fly for characters not on the keyboard.
    spare: u8,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    bgr: bool,
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
        let bgr = blue_mask > red_mask;

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
            red_mask,
            green_mask,
            blue_mask,
            bgr,
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
        let per = mapping.keysyms_per_keycode as usize;
        let mut used = vec![false; count as usize];
        for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
            let keycode = min + i as u8;
            let base = chunk.first().copied().unwrap_or(0);
            let shifted = chunk.get(1).copied().unwrap_or(0);
            if base != 0 {
                self.keymap.entry(base).or_insert((keycode, false));
                used[i] = true;
            }
            if shifted != 0 && shifted != base {
                self.keymap.entry(shifted).or_insert((keycode, true));
                used[i] = true;
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
        // A spare keycode with no keysym, for remapping arbitrary chars.
        self.spare = used
            .iter()
            .position(|u| !u)
            .map(|i| min + i as u8)
            .unwrap_or(max);
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
        self.warp(from.0, from.1)?;
        self.fake(6, 0, from.0 as i16, from.1 as i16)?;
        self.fake(BUTTON_PRESS, 1, from.0 as i16, from.1 as i16)?;
        // A few intermediate motions so drag-aware widgets follow.
        for step in 1..=8 {
            let x = from.0 + (to.0 - from.0) * step / 8;
            let y = from.1 + (to.1 - from.1) * step / 8;
            self.warp(x, y)?;
            self.fake(6, 0, x as i16, y as i16)?;
        }
        self.fake(BUTTON_RELEASE, 1, to.0 as i16, to.1 as i16)?;
        self.put_back(home)?;
        self.flush()
    }

    /// Type a run of text as key events (fallback; AT-SPI insert is preferred).
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            if c == '\n' {
                self.press(&KeyCombo {
                    modifiers: Modifiers::default(),
                    key: Key::Named(NamedKey::Return),
                })?;
                continue;
            }
            self.press(&KeyCombo {
                modifiers: Modifiers::default(),
                key: Key::Char(c),
            })?;
        }
        Ok(())
    }

    pub fn press(&mut self, combo: &KeyCombo) -> Result<()> {
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
        self.flush()
    }

    /// Returns (keycode, needs_shift), remapping the spare keycode if needed.
    fn resolve_key(&mut self, key: Key) -> Result<(u8, bool)> {
        let keysym =
            keysym_for(key).ok_or_else(|| Error::ActionFailed(format!("no keysym for {key:?}")))?;
        if let Some((kc, sh)) = self.keymap.get(&keysym) {
            return Ok((*kc, *sh));
        }
        // Remap the spare keycode to this keysym for one press.
        if self.spare == 0 {
            return Err(Error::ActionFailed(
                "no free keycode to type this character".into(),
            ));
        }
        let syms = [keysym, keysym];
        // .check() round-trips so the mapping is live before the fake key event.
        self.conn
            .change_keyboard_mapping(1, self.spare, 2, &syms)
            .map_err(xe)?
            .check()
            .map_err(xe)?;
        self.keymap.insert(keysym, (self.spare, false));
        Ok((self.spare, false))
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

    /// The full screen rectangle (the root window's size).
    pub fn root_rect(&self) -> Rect {
        Rect::new(0.0, 0.0, f64::from(self.root_w), f64::from(self.root_h))
    }

    pub fn capture(&self, rect: Rect) -> Result<Capture> {
        let x = rect.x.max(0.0) as i16;
        let y = rect.y.max(0.0) as i16;
        let w = (rect.width.round() as i32)
            .clamp(1, i32::from(self.root_w) - i32::from(x))
            .max(1) as u16;
        let h = (rect.height.round() as i32)
            .clamp(1, i32::from(self.root_h) - i32::from(y))
            .max(1) as u16;
        let img = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, x, y, w, h, !0)
            .map_err(xe)?
            .reply()
            .map_err(|e| Error::Platform(format!("GetImage failed: {e}")))?;

        let px = img.data.len() / (w as usize * h as usize).max(1);
        let (rs, gs, bs) = (
            self.red_mask.trailing_zeros(),
            self.green_mask.trailing_zeros(),
            self.blue_mask.trailing_zeros(),
        );
        let rgba = if px >= 4 {
            // 32bpp: convert in place in the reply buffer (no second copy).
            let mut data = img.data;
            data.truncate(w as usize * h as usize * 4);
            for chunk in data.chunks_exact_mut(4) {
                let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                chunk[0] = ((word & self.red_mask) >> rs) as u8;
                chunk[1] = ((word & self.green_mask) >> gs) as u8;
                chunk[2] = ((word & self.blue_mask) >> bs) as u8;
                chunk[3] = 255;
            }
            data
        } else {
            // 24bpp packed (3 bytes/pixel) needs a wider buffer.
            let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
            for chunk in img.data.chunks_exact(3) {
                let (r, g, b) = if self.bgr {
                    (chunk[2], chunk[1], chunk[0])
                } else {
                    (chunk[0], chunk[1], chunk[2])
                };
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
            rgba
        };
        Ok(Capture {
            width: w as u32,
            height: h as u32,
            rgba,
            bounds: Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h)),
        })
    }
}

pub(crate) fn keysym_for(key: Key) -> Option<u32> {
    Some(match key {
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
