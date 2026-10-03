//! X11 synthesized input (XTest) and window capture (GetImage), the fallback
//! path when the accessibility API can't do something directly.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _, ImageFormat, Mapping, ModMask};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::{DefaultStream, RustConnection};

use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, Modifiers, NamedKey, Pad};
use crate::types::{Capture, Rect};

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
/// How many free keycodes (at most) are used for characters not on the
/// keyboard: enough that one is rarely rebound in the middle of a text.
const SPARES: usize = 16;
/// How long connecting to the X server may take: a TCP display whose host
/// doesn't answer (WSL2 with no X server running) would otherwise hang for
/// minutes, and again on every retry.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Connection attempts given up on that are still waiting for an answer.
static UNANSWERED: AtomicIsize = AtomicIsize::new(0);

/// A keycode with no keysyms, bound on the fly to a character that is not
/// on the keyboard (another script, a symbol).
#[derive(Debug, Clone, Copy)]
struct Spare {
    code: u8,
    /// The keysym it is bound to now (0: unbound).
    sym: u32,
    /// When it was last pressed: it isn't rebound or unbound before the
    /// focused app has had time to translate that press.
    pressed: Option<Instant>,
}

/// Keyboard state changed for typing, to put back afterwards.
#[derive(Debug, Default)]
struct Saved {
    /// The locked group (layout) to restore.
    group: Option<xkb::Group>,
    /// Caps Lock was on.
    caps: bool,
}

pub struct X11 {
    conn: RustConnection,
    root: xproto::Window,
    /// The root window's size at connect time (see `root_size`).
    root_w: u16,
    root_h: u16,
    /// keysym -> (keycode, needs_shift), from the keymap's first group.
    keymap: HashMap<u32, (u8, bool)>,
    shift: u8,
    ctrl: u8,
    alt: u8,
    meta: u8,
    /// Keycodes to bind characters that are not on the keyboard to:
    /// several, so that one is rarely rebound while an app may still be
    /// translating its last press (none when every keycode is in use).
    spares: Vec<Spare>,
    /// The keyboard mapping changed since it was read (MappingNotify).
    keymap_stale: Cell<bool>,
    /// The connection to the X server broke: the owner reconnects.
    lost: Cell<bool>,
    /// The XKB extension is in use (keyboard groups and lock state).
    xkb: bool,
    /// The XTEST extension is there: synthesized input works.
    xtest: bool,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    /// Put the pointer back after synthesized mouse input.
    pub restore_pointer: bool,
}

impl Drop for X11 {
    /// Give bound spare keycodes back: one would otherwise keep its last
    /// character, and stop being spare, until the X session ends.
    fn drop(&mut self) {
        for s in self.spares.iter().filter(|s| s.sym != 0) {
            let _ = self.conn.change_keyboard_mapping(1, s.code, 2, &[0, 0]);
        }
        let _ = self.conn.flush();
    }
}

impl X11 {
    pub fn connect() -> Result<Self> {
        let (conn, screen_num) = connect_display()?;
        // Synthesized input needs XTest; screenshots and windows don't.
        let xtest = conn
            .xtest_get_version(2, 2)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some();
        if !xtest {
            log::warn!("the X server has no XTEST extension: no synthesized input");
        }
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

        // XKB, to type in the keymap's first layout whatever layout is on.
        let xkb = conn
            .xkb_use_extension(1, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.supported);
        if xkb {
            // Hear of keymap changes: a new keyboard (setxkbmap) is only
            // announced to XKB clients as an XKB event, and changed keysyms
            // (MappingNotify) only when asked for.
            let maps = xkb::MapPart::KEY_SYMS | xkb::MapPart::MODIFIER_MAP;
            if let Err(e) = conn.xkb_select_events(
                xkb::ID::USE_CORE_KBD.into(),
                xkb::EventType::from(0u16),
                xkb::EventType::NEW_KEYBOARD_NOTIFY | xkb::EventType::MAP_NOTIFY,
                maps,
                maps,
                &xkb::SelectEventsAux::new(),
            ) {
                log::warn!("XKB keymap notifications unavailable: {e}");
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
            spares: Vec::new(),
            keymap_stale: Cell::new(false),
            lost: Cell::new(false),
            xkb,
            xtest,
            red_mask,
            green_mask,
            blue_mask,
            restore_pointer: true,
        };
        x.load_keymap()?;
        Ok(x)
    }

    /// Whether synthesized input works (the server has XTEST).
    pub fn can_input(&self) -> bool {
        self.xtest
    }

    fn need_xtest(&self) -> Result<()> {
        if self.xtest {
            Ok(())
        } else {
            Err(Error::Unsupported(
                "synthesized keyboard/mouse input: this X server has no XTEST extension (screenshots and element_index actions still work)".into(),
            ))
        }
    }

    /// Whether the connection to the X server broke (seen by `drain`).
    pub fn lost(&self) -> bool {
        self.lost.get()
    }

    /// Handle what the server sent since the last call: errors of earlier
    /// requests are logged, a changed keymap is read again before the next
    /// key, and a broken connection is noted (see `lost`). Unread, the
    /// events would pile up in memory.
    pub fn drain(&self) {
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(ev)) => self.handle_event(ev),
                Ok(None) => break,
                Err(e) => {
                    log::warn!("lost the connection to the X server: {e}");
                    self.lost.set(true);
                    break;
                }
            }
        }
    }

    fn handle_event(&self, ev: Event) {
        // Our own binding of a spare keycode needs no new keymap.
        let spare_only = |first: u8, n: u8| n == 1 && self.spares.iter().any(|s| s.code == first);
        let none = xkb::MapPart::from(0u16);
        match ev {
            Event::Error(e) => log::warn!(
                "X11 error: {:?} (request {}.{}, value {:#x})",
                e.error_kind,
                e.major_opcode,
                e.minor_opcode,
                e.bad_value
            ),
            Event::MappingNotify(e) => {
                if e.request != Mapping::POINTER && !spare_only(e.first_keycode, e.count) {
                    self.keymap_stale.set(true);
                }
            }
            Event::XkbNewKeyboardNotify(_) => self.keymap_stale.set(true),
            Event::XkbMapNotify(e) => {
                let syms = e.changed & xkb::MapPart::KEY_SYMS != none
                    && e.n_key_syms > 0
                    && !spare_only(e.first_key_sym, e.n_key_syms);
                let mods = e.changed & xkb::MapPart::MODIFIER_MAP != none
                    && e.n_mod_map_keys > 0
                    && !spare_only(e.first_mod_map_key, e.n_mod_map_keys);
                if syms || mods {
                    self.keymap_stale.set(true);
                }
            }
            _ => {}
        }
    }

    /// Read the keymap again if it changed.
    fn ensure_keymap(&mut self) -> Result<()> {
        self.drain();
        if self.keymap_stale.get() {
            self.load_keymap()?;
        }
        Ok(())
    }

    fn load_keymap(&mut self) -> Result<()> {
        self.keymap_stale.set(false);
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
        // Spare keycodes with no keysym at all, for binding arbitrary
        // chars; never a real key (none: typing such chars then fails).
        // Kept while one is bound (it isn't empty then).
        if self.spares.iter().all(|s| s.sym == 0) {
            self.spares = find_spares(&mapping.keysyms, per, min, SPARES)
                .into_iter()
                .map(|code| Spare {
                    code,
                    sym: 0,
                    pressed: None,
                })
                .collect();
        }
        self.keymap.clear();
        for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
            let keycode = min + i as u8;
            if self.spares.iter().any(|s| s.code == keycode) {
                continue; // bound to one char at a time, never its key
            }
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

    fn warp(&self, x: i16, y: i16) -> Result<()> {
        self.conn
            .warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, x, y)
            .map_err(xe)?;
        Ok(())
    }

    /// Where the user's pointer is, when it should be put back afterwards.
    fn pointer(&self) -> Option<(i16, i16)> {
        if !self.restore_pointer {
            return None;
        }
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x, r.root_y))
    }

    /// Return the pointer to where the user left it.
    fn put_back(&self, at: Option<(i16, i16)>) -> Result<()> {
        if let Some((x, y)) = at {
            self.warp(x, y)?;
            self.fake(6, 0, x, y)?;
        }
        Ok(())
    }

    pub fn click(&self, x: i32, y: i32, button: u8, count: u8) -> Result<()> {
        self.need_xtest()?;
        let (x, y) = (coord(x)?, coord(y)?);
        let home = self.pointer();
        self.warp(x, y)?;
        self.fake(6, 0, x, y)?; // MotionNotify absolute
        for _ in 0..count.max(1) {
            self.fake(BUTTON_PRESS, button, x, y)?;
            self.fake(BUTTON_RELEASE, button, x, y)?;
        }
        self.put_back(home)?;
        self.flush()
    }

    pub fn scroll(&self, x: i32, y: i32, dx: i32, dy: i32) -> Result<()> {
        self.need_xtest()?;
        let (x, y) = (coord(x)?, coord(y)?);
        let home = self.pointer();
        self.warp(x, y)?;
        let tick = |button: u8, n: i32| -> Result<()> {
            for _ in 0..n.unsigned_abs() {
                self.fake(BUTTON_PRESS, button, x, y)?;
                self.fake(BUTTON_RELEASE, button, x, y)?;
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
        self.need_xtest()?;
        let (fx, fy, tx, ty) = (coord(from.0)?, coord(from.1)?, coord(to.0)?, coord(to.1)?);
        let home = self.pointer();
        // Paced like a real drag: toolkits that start a drag on a motion
        // threshold or a timer miss a single burst of events.
        let pause = || -> Result<()> {
            self.flush()?;
            std::thread::sleep(DRAG_STEP);
            Ok(())
        };
        self.warp(fx, fy)?;
        self.fake(6, 0, fx, fy)?;
        pause()?;
        self.fake(BUTTON_PRESS, 1, fx, fy)?;
        pause()?;
        // A few intermediate motions so drag-aware widgets follow.
        for step in 1..=8 {
            let x = clamp16(from.0 + (to.0 - from.0) * step / 8);
            let y = clamp16(from.1 + (to.1 - from.1) * step / 8);
            self.warp(x, y)?;
            self.fake(6, 0, x, y)?;
            pause()?;
        }
        self.fake(BUTTON_RELEASE, 1, tx, ty)?;
        pause()?;
        self.put_back(home)?;
        self.flush()
    }

    /// Move the pointer to (x, y); where it was, if it should go back.
    pub fn move_pointer(&self, x: i32, y: i32) -> Result<Option<(i32, i32)>> {
        self.need_xtest()?;
        let (x, y) = (coord(x)?, coord(y)?);
        let home = self.pointer();
        self.warp(x, y)?;
        self.fake(6, 0, x, y)?;
        self.flush()?;
        Ok(home.map(|(x, y)| (i32::from(x), i32::from(y))))
    }

    /// Draw `strokes` with `button` (1 left, 2 middle, 3 right) held (see
    /// `Backend::draw`).
    pub fn draw(
        &self,
        strokes: &[Vec<(i32, i32)>],
        button: u8,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        self.need_xtest()?;
        // Every point checked before anything is drawn.
        let strokes = strokes
            .iter()
            .map(|s| {
                s.iter()
                    .map(|&(x, y)| Ok((coord(x)?, coord(y)?)))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        let home = self.pointer();
        let mut held = None;
        let drawn = self.draw_strokes(&strokes, button, pace, &mut held);
        if let Some((x, y)) = held {
            // Never leave the button held down.
            let _ = self.fake(BUTTON_RELEASE, button, x, y);
        }
        let back = self.put_back(home);
        let flushed = self.flush();
        drawn.and(back).and(flushed)
    }

    fn draw_strokes(
        &self,
        strokes: &[Vec<(i16, i16)>],
        button: u8,
        pace: &mut dyn FnMut(f64) -> Result<()>,
        held: &mut Option<(i16, i16)>,
    ) -> Result<()> {
        for stroke in strokes {
            let Some(&(x, y)) = stroke.first() else {
                continue;
            };
            pace(0.0)?;
            self.warp(x, y)?;
            self.fake(6, 0, x, y)?;
            self.flush()?;
            std::thread::sleep(DRAG_STEP);
            self.fake(BUTTON_PRESS, button, x, y)?;
            *held = Some((x, y));
            self.flush()?;
            std::thread::sleep(DRAG_STEP);
            let mut last = (x, y);
            for &(px, py) in &stroke[1..] {
                let (dx, dy) = (
                    i32::from(px) - i32::from(last.0),
                    i32::from(py) - i32::from(last.1),
                );
                pace(f64::from(dx).hypot(f64::from(dy)))?;
                if (px, py) == last {
                    continue;
                }
                self.warp(px, py)?;
                self.fake(6, 0, px, py)?;
                self.flush()?;
                last = (px, py);
                *held = Some(last);
            }
            std::thread::sleep(DRAG_STEP);
            self.fake(BUTTON_RELEASE, button, last.0, last.1)?;
            *held = None;
            self.flush()?;
        }
        Ok(())
    }

    /// Type a run of text as key events, in the keymap's first layout
    /// whatever layout and Caps Lock the user has on (restored after).
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        self.need_xtest()?;
        self.ensure_keymap()?;
        let saved = self.plain_keyboard()?;
        let typed = self.type_chars(text);
        let back = self.restore_keyboard(saved);
        let spares = self.release_spares();
        typed.and(back).and(spares)
    }

    fn type_chars(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            let key = if c == '\n' {
                Key::Named(NamedKey::Return)
            } else {
                Key::Char(c)
            };
            self.press_one(&KeyCombo {
                modifiers: Modifiers::default(),
                key,
            })?;
        }
        Ok(())
    }

    /// Press a key combination. A character key types (or, with Ctrl,
    /// means) what it says whatever layout and Caps Lock are on; other
    /// keys (Return, Caps Lock itself) leave the keyboard state alone.
    pub fn press(&mut self, combo: &KeyCombo) -> Result<()> {
        self.need_xtest()?;
        self.ensure_keymap()?;
        let saved = match combo.key {
            Key::Char(_) => self.plain_keyboard()?,
            Key::Named(_) => Saved::default(),
        };
        let pressed = self.press_one(combo);
        let back = self.restore_keyboard(saved);
        let spares = self.release_spares();
        pressed.and(back).and(spares)
    }

    /// Set the keyboard to the state the keymap was read for: its first
    /// group (layout), and Caps Lock off. With "us,ru" and the Russian
    /// layout on, the key for "a" would type "ф"; with Caps Lock on, "A".
    /// Returns what to put back.
    fn plain_keyboard(&self) -> Result<Saved> {
        if !self.xkb {
            return Ok(Saved::default());
        }
        let st = self
            .conn
            .xkb_get_state(xkb::ID::USE_CORE_KBD.into())
            .map_err(xe)?
            .reply()
            .map_err(xe)?;
        let saved = Saved {
            group: (st.group != xkb::Group::M1).then_some(st.locked_group),
            caps: st.locked_mods & ModMask::LOCK != ModMask::from(0u16),
        };
        self.lock_state(
            saved.group.map(|_| xkb::Group::M1),
            saved.caps.then_some(false),
        )?;
        Ok(saved)
    }

    /// Put back what `plain_keyboard` changed. The key events sent before
    /// were made under the plain state (requests are handled in order).
    fn restore_keyboard(&self, saved: Saved) -> Result<()> {
        self.lock_state(saved.group, saved.caps.then_some(true))?;
        self.flush()
    }

    /// Lock the keyboard group, and/or turn Caps Lock on or off (XKB).
    fn lock_state(&self, group: Option<xkb::Group>, caps: Option<bool>) -> Result<()> {
        if group.is_none() && caps.is_none() {
            return Ok(());
        }
        let none = ModMask::from(0u16);
        let affect = if caps.is_some() { ModMask::LOCK } else { none };
        let locks = if caps == Some(true) {
            ModMask::LOCK
        } else {
            none
        };
        self.conn
            .xkb_latch_lock_state(
                xkb::ID::USE_CORE_KBD.into(),
                affect,
                locks,
                group.is_some(),
                group.unwrap_or(xkb::Group::M1),
                none,
                false,
                0,
            )
            .map_err(xe)?;
        Ok(())
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
        if let Some(s) = self.spares.iter_mut().find(|s| s.code == keycode) {
            s.pressed = Some(Instant::now());
        }
        Ok(())
    }

    /// Returns (keycode, needs_shift), binding a spare keycode if needed.
    fn resolve_key(&mut self, key: Key) -> Result<(u8, bool)> {
        let keysym =
            keysym_for(key).ok_or_else(|| Error::ActionFailed(format!("no keysym for {key:?}")))?;
        // A keypad key reached through Shift would depend on Num Lock:
        // bind the exact keysym to a spare keycode instead.
        let keypad = (0xff80..=0xffbd).contains(&keysym);
        if let Some((kc, sh)) = self.keymap.get(&keysym)
            && !(keypad && *sh)
        {
            return Ok((*kc, *sh));
        }
        // Characters not on the keyboard (another script, a symbol): bind
        // a spare keycode to this one for the press (or reuse the one bound
        // to it already). The binding never becomes the character's key in
        // `keymap`: the keycode is rebound to other characters.
        if let Some(s) = self.spares.iter().find(|s| s.sym == keysym) {
            return Ok((s.code, false));
        }
        // The one pressed longest ago (or never).
        let i = (0..self.spares.len())
            .min_by_key(|&i| self.spares[i].pressed)
            .ok_or_else(|| Error::ActionFailed("no free keycode to type this character".into()))?;
        self.bind_spare(i, keysym)?;
        Ok((self.spares[i].code, false))
    }

    /// Bind spare keycode `i` to `keysym`, once its last press has been
    /// translated.
    fn bind_spare(&mut self, i: usize, keysym: u32) -> Result<()> {
        settle(self.spares[i].pressed.take());
        // Forget the old binding first: if the change fails, nothing claims
        // the keycode still types the previous character.
        self.spares[i].sym = 0;
        // .check() waits for the server to have done it, so it has also
        // told the apps (MappingNotify) ahead of the key event that follows.
        self.conn
            .change_keyboard_mapping(1, self.spares[i].code, 2, &[keysym, keysym])
            .map_err(xe)?
            .check()
            .map_err(xe)?;
        self.spares[i].sym = keysym;
        Ok(())
    }

    /// Unbind the spare keycodes again, after each typing, once the focused
    /// app has had time to translate the last presses on them (it reads the
    /// mapping when it gets the key, and needs the binding until then).
    fn release_spares(&mut self) -> Result<()> {
        let bound = |s: &&mut Spare| s.sym != 0;
        settle(
            self.spares
                .iter_mut()
                .filter(bound)
                .filter_map(|s| s.pressed)
                .max(),
        );
        let mut result = Ok(());
        for s in self.spares.iter_mut().filter(bound) {
            s.sym = 0;
            s.pressed = None;
            let r = self
                .conn
                .change_keyboard_mapping(1, s.code, 2, &[0, 0])
                .map_err(xe)
                .and_then(|c| c.check().map_err(xe));
            result = result.and(r);
        }
        result
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

    /// Capture a screen rectangle as RGBA.
    pub fn capture(&self, rect: Rect) -> Result<Capture> {
        // Only the part of `rect` that is on the screen.
        let (root_w, root_h) = self.root_size();
        let (rw, rh) = (f64::from(root_w), f64::from(root_h));
        let (x0, y0) = (rect.x.clamp(0.0, rw), rect.y.clamp(0.0, rh));
        let (x1, y1) = (
            (rect.x + rect.width).clamp(0.0, rw),
            (rect.y + rect.height).clamp(0.0, rh),
        );
        if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
            return Err(Error::InvalidArgs(format!(
                "the area ({:.0}, {:.0}) {:.0}x{:.0} is outside the screen ({root_w}x{root_h})",
                rect.x, rect.y, rect.width, rect.height
            )));
        }
        let (x, y) = (coord(x0 as i32)?, coord(y0 as i32)?);
        let w = (x1 - x0).round().max(1.0) as u16;
        let h = (y1 - y0).round().max(1.0) as u16;
        let masks = [self.red_mask, self.green_mask, self.blue_mask];
        let rgba = self.image(self.root, (x, y, w, h), masks)?;
        Ok(Capture {
            width: w as u32,
            height: h as u32,
            rgba,
            bounds: Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h)),
        })
    }

    /// Capture the part of a screen rectangle that X window `win` covers,
    /// read from the window itself. Rootless XWayland (a GNOME or KDE
    /// Wayland session) keeps no picture of the screen on its root window,
    /// only each window's own; the window may have its own visual (32-bit
    /// ARGB for a translucent one).
    pub fn capture_window(&self, win: xproto::Window, rect: Rect) -> Result<Capture> {
        let at = self
            .wm()
            .geometry(win)
            .ok_or_else(|| Error::ActionFailed(format!("the X window {win:#x} is gone")))?;
        let (x, y, w, h) = window_crop(rect, at).ok_or_else(|| {
            Error::InvalidArgs(format!(
                "the area ({:.0}, {:.0}) {:.0}x{:.0} is outside the window",
                rect.x, rect.y, rect.width, rect.height
            ))
        })?;
        let visual = self
            .conn
            .get_window_attributes(win)
            .map_err(xe)?
            .reply()
            .map_err(xe)?
            .visual;
        let masks =
            self.visual_masks(visual)
                .unwrap_or([self.red_mask, self.green_mask, self.blue_mask]);
        let rgba = self.image(win, (x, y, w, h), masks)?;
        Ok(Capture {
            width: w as u32,
            height: h as u32,
            rgba,
            bounds: Rect::new(
                at.x + f64::from(x),
                at.y + f64::from(y),
                f64::from(w),
                f64::from(h),
            ),
        })
    }

    /// The red, green and blue masks of a visual.
    fn visual_masks(&self, visual: xproto::Visualid) -> Option<[u32; 3]> {
        self.conn
            .setup()
            .roots
            .iter()
            .flat_map(|s| &s.allowed_depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == visual)
            .map(|v| [v.red_mask, v.green_mask, v.blue_mask])
    }

    /// The pixels of an area (x, y, width, height) of a drawable, as RGBA.
    fn image(
        &self,
        drawable: xproto::Drawable,
        (x, y, w, h): (i16, i16, u16, u16),
        masks: [u32; 3],
    ) -> Result<Vec<u8>> {
        let img = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, !0)
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
            masks,
        };
        to_rgba(img.data, w.into(), h.into(), &layout)
    }
}

/// Connect to the X server `$DISPLAY` names (and the screen number), giving
/// up after [`CONNECT_TIMEOUT`] instead of hanging on one that doesn't
/// answer.
pub(crate) fn connect_display() -> Result<(RustConnection, usize)> {
    within(CONNECT_TIMEOUT, &UNANSWERED, open_display)
}

/// Connect to the X server `$DISPLAY` names, as `x11rb::connect` does but
/// with a time limit on reaching a TCP display.
fn open_display() -> Result<(RustConnection, usize)> {
    use x11rb::reexports::x11rb_protocol::parse_display::{ConnectAddress, parse_display};
    use x11rb::reexports::x11rb_protocol::xauth::get_auth;
    let fail =
        |e: &dyn std::fmt::Display| Error::Platform(format!("cannot connect to the X server: {e}"));
    let display = parse_display(None).map_err(|e| fail(&e))?;
    let screen = usize::from(display.screen);
    let mut last: Option<std::io::Error> = None;
    for addr in display.connect_instruction() {
        let stream = match &addr {
            ConnectAddress::Hostname(host, port) => {
                tcp_connect(host, *port, CONNECT_TIMEOUT).and_then(DefaultStream::from_tcp_stream)
            }
            ConnectAddress::Socket(path) => std::os::unix::net::UnixStream::connect(path)
                .and_then(DefaultStream::from_unix_stream),
            _ => continue,
        };
        match stream {
            Ok((stream, (family, address))) => {
                // Without auth data when there is none (or it can't be read).
                let (name, data) = get_auth(family, &address, display.display)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let conn =
                    RustConnection::connect_to_stream_with_auth_info(stream, screen, name, data)
                        .map_err(|e| fail(&e))?;
                return Ok((conn, screen));
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => fail(&e),
        None => fail(&"no address to reach the display at"),
    })
}

/// A TCP connection to `host`:`port`, giving up on each address after
/// `timeout`.
fn tcp_connect(host: &str, port: u16, timeout: Duration) -> std::io::Result<std::net::TcpStream> {
    use std::net::ToSocketAddrs as _;
    let mut last = None;
    for addr in (host, port).to_socket_addrs()? {
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no address for {host}"),
        )
    }))
}

/// Run `f` (a connection attempt) on its own thread for at most `timeout`;
/// an attempt given up on is counted in `unanswered` until it ends, and no
/// new one starts meanwhile (a hung server would otherwise collect one
/// stuck thread per retry).
fn within<T: Send + 'static>(
    timeout: Duration,
    unanswered: &'static AtomicIsize,
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    const RUNNING: u8 = 0;
    const DONE: u8 = 1;
    const ABANDONED: u8 = 2;
    if unanswered.load(Ordering::SeqCst) > 0 {
        return Err(Error::Platform(
            "the X server hasn't answered an earlier connection attempt".into(),
        ));
    }
    let state = Arc::new(AtomicU8::new(RUNNING));
    let (tx, rx) = std::sync::mpsc::channel();
    let st = state.clone();
    std::thread::Builder::new()
        .name("x11-connect".into())
        .spawn(move || {
            let _ = tx.send(f());
            if st.swap(DONE, Ordering::SeqCst) == ABANDONED {
                unanswered.fetch_sub(1, Ordering::SeqCst);
            }
        })
        .map_err(|e| Error::Platform(format!("cannot connect to the X server: {e}")))?;
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(_) => {
            unanswered.fetch_add(1, Ordering::SeqCst);
            if state
                .compare_exchange(RUNNING, ABANDONED, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                // It ended just now.
                unanswered.fetch_sub(1, Ordering::SeqCst);
                if let Ok(r) = rx.try_recv() {
                    return r;
                }
            }
            Err(Error::Platform(format!(
                "cannot connect to the X server: no answer within {} s",
                timeout.as_secs()
            )))
        }
    }
}

/// An X11 coordinate (16 bits signed): out of range is an error, never a
/// position wrapped to the other side of the screen.
fn coord(v: i32) -> Result<i16> {
    i16::try_from(v).map_err(|_| {
        Error::InvalidArgs(format!(
            "coordinate {v} is outside what X11 can address ({}..={})",
            i16::MIN,
            i16::MAX
        ))
    })
}

/// An X11 coordinate for a point on the way (a drag's): the nearest one.
fn clamp16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// The part of `rect` (screen coordinates) inside a window at `at`, in the
/// window's coordinates: (x, y, width, height), or `None` when they don't
/// overlap by a pixel.
fn window_crop(rect: Rect, at: Rect) -> Option<(i16, i16, u16, u16)> {
    let x0 = (rect.x - at.x).clamp(0.0, at.width).round();
    let y0 = (rect.y - at.y).clamp(0.0, at.height).round();
    let x1 = (rect.x + rect.width - at.x).clamp(0.0, at.width).round();
    let y1 = (rect.y + rect.height - at.y).clamp(0.0, at.height).round();
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return None;
    }
    Some((
        i16::try_from(x0 as i64).ok()?,
        i16::try_from(y0 as i64).ok()?,
        u16::try_from((x1 - x0) as i64).ok()?,
        u16::try_from((y1 - y0) as i64).ok()?,
    ))
}

/// Pace of synthesized drag steps.
const DRAG_STEP: Duration = Duration::from_millis(12);
/// Time for the focused app to translate a press on a spare keycode before
/// the keycode is rebound or unbound. Apps read the new mapping when they
/// get the key event, which no request to the server can confirm.
const REMAP_SETTLE: Duration = Duration::from_millis(30);

/// Wait until `REMAP_SETTLE` has passed since `pressed`.
fn settle(pressed: Option<Instant>) {
    if let Some(t) = pressed {
        let since = t.elapsed();
        if since < REMAP_SETTLE {
            std::thread::sleep(REMAP_SETTLE - since);
        }
    }
}

/// Up to `n` keycodes none of whose keysyms is set.
fn find_spares(keysyms: &[u32], per: usize, min: u8, n: usize) -> Vec<u8> {
    keysyms
        .chunks(per.max(1))
        .enumerate()
        .filter(|(_, chunk)| chunk.iter().all(|s| *s == 0))
        .filter_map(|(i, _)| u8::try_from(usize::from(min) + i).ok())
        .take(n)
        .collect()
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
            NamedKey::Numpad(p) => match p {
                Pad::Digit(d) => 0xffb0 + u32::from(d),
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

fn xe(e: impl std::fmt::Display) -> Error {
    Error::Platform(format!("X11: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ignored tests share the X server's focus and keymap.
    static X_SERVER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A window that gets key presses, focused.
    fn key_window(conn: &RustConnection, root: xproto::Window) -> xproto::Window {
        use xproto::{CreateWindowAux, EventMask, WindowClass};
        let win = conn.generate_id().unwrap();
        conn.create_window(
            0,
            win,
            root,
            0,
            0,
            200,
            100,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::KEY_PRESS),
        )
        .unwrap();
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        conn.set_input_focus(xproto::InputFocus::PARENT, win, x11rb::CURRENT_TIME)
            .unwrap();
        conn.get_input_focus().unwrap().reply().unwrap();
        win
    }

    /// (keycode, state) of each key press received so far.
    fn key_presses(conn: &RustConnection) -> Vec<(u8, u16)> {
        let mut out = Vec::new();
        while let Some(ev) = conn.poll_for_event().unwrap() {
            if let Event::KeyPress(e) = ev {
                out.push((e.detail, u16::from(e.state)));
            }
        }
        out
    }

    /// Needs an X server with XTest and XKB, a US keymap
    /// (`xvfb-run cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn typing_ignores_the_users_layout_and_caps_lock() {
        let _x = X_SERVER.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, n) = x11rb::connect(None).expect("an X display");
        conn.xkb_use_extension(1, 0).unwrap().reply().unwrap();
        let root = conn.setup().roots[n].root;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let read_map = || {
            let m = conn
                .get_keyboard_mapping(min, max - min + 1)
                .unwrap()
                .reply()
                .unwrap();
            (m.keysyms, usize::from(m.keysyms_per_keycode))
        };
        let (syms, per) = read_map();
        let code_of = |sym: u32| {
            min + syms
                .chunks(per)
                .position(|c| c[0] == sym)
                .expect("on a US keyboard") as u8
        };
        let (a, b, y, z) = (code_of(0x61), code_of(0x62), code_of(0x79), code_of(0x7a));
        let row = |code: u8| syms[usize::from(code - min) * per..][..per].to_vec();
        let originals: Vec<(u8, Vec<u32>)> = [a, b, y, z].iter().map(|c| (*c, row(*c))).collect();
        key_window(&conn, root);
        let kbd = xkb::ID::USE_CORE_KBD.into();
        let mut keys = X11::connect().unwrap();
        // The user's keyboard now has a Russian group, and German Y and Z
        // (read by `keys` from the MappingNotify before it types).
        for (code, s) in [
            (a, [0x61, 0x41, 0x6c6, 0x6e6]),
            (b, [0x62, 0x42, 0x6c9, 0x6e9]),
            (y, [0x7a, 0x5a, 0x6ce, 0x6ee]),
            (z, [0x79, 0x59, 0x6d1, 0x6f1]),
        ] {
            conn.change_keyboard_mapping(1, code, 4, &s)
                .unwrap()
                .check()
                .unwrap();
        }
        struct Restore<'a>(&'a RustConnection, Vec<(u8, Vec<u32>)>);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                for (code, syms) in &self.1 {
                    let per = syms.len() as u8;
                    let _ = self.0.change_keyboard_mapping(1, *code, per, syms);
                }
                let _ = self.0.get_input_focus().map(|c| c.reply());
            }
        }
        let _restore = Restore(&conn, originals);
        let lock = |group: xkb::Group, caps: bool| {
            let mods = if caps {
                ModMask::LOCK
            } else {
                ModMask::from(0u16)
            };
            conn.xkb_latch_lock_state(
                kbd,
                ModMask::LOCK,
                mods,
                true,
                group,
                ModMask::from(0u16),
                false,
                0,
            )
            .unwrap();
            conn.get_input_focus().unwrap().reply().unwrap();
        };
        // ... and has the Russian group and Caps Lock on.
        lock(xkb::Group::M2, true);
        let st = conn.xkb_get_state(kbd).unwrap().reply().unwrap();
        assert_eq!(st.locked_group, xkb::Group::M2, "a second group to lock");
        keys.type_text("aBz1ф").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let (syms, per) = read_map();
        let spares: Vec<u8> = keys.spares.iter().map(|s| s.code).collect();
        let (mut typed, mut spare_presses) = (Vec::new(), 0);
        for (code, state) in key_presses(&conn) {
            // Typed in the first group, without Caps Lock.
            assert_eq!(state >> 13 & 3, 0, "group in state {state:#x}");
            assert_eq!(state & 2, 0, "Caps Lock in state {state:#x}");
            if spares.contains(&code) {
                spare_presses += 1;
                continue;
            }
            let sym = syms[usize::from(code - min) * per + usize::from(state & 1)];
            if !(0xffe1..=0xffee).contains(&sym) {
                typed.push(sym);
            }
        }
        assert_eq!(typed, [0x61, 0x42, 0x7a, 0x31], "a, B, z, 1");
        assert_eq!(spare_presses, 1, "ф on a spare keycode");
        // The user's state is back, and the spare keycode free again.
        let st = conn.xkb_get_state(kbd).unwrap().reply().unwrap();
        assert_eq!(st.locked_group, xkb::Group::M2);
        assert_ne!(st.locked_mods & ModMask::LOCK, ModMask::from(0u16));
        for code in &spares {
            let row = &syms[usize::from(code - min) * per..][..per];
            assert!(row.iter().all(|s| *s == 0), "spare {code} still bound");
        }
        lock(xkb::Group::M1, false);
    }

    /// Needs an X server with XTest (`xvfb-run cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn drawing_and_keypad_keys_reach_the_window() {
        let _x = X_SERVER.lock().unwrap_or_else(|e| e.into_inner());
        use x11rb::protocol::Event;
        use xproto::{CreateWindowAux, EventMask, KeyButMask, WindowClass};
        let (conn, n) = x11rb::connect(None).expect("an X display");
        let root = conn.setup().roots[n].root;
        let win = conn.generate_id().unwrap();
        conn.create_window(
            0,
            win,
            root,
            0,
            0,
            400,
            300,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(
                EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::BUTTON_MOTION,
            ),
        )
        .unwrap();
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(200));

        let x = X11::connect().unwrap();
        let line: Vec<(i32, i32)> = (0..=40).map(|i| (50 + i * 5, 100 + i * 2)).collect();
        let mut paces = 0;
        x.draw(&[line], 1, &mut |_| {
            paces += 1;
            Ok(())
        })
        .unwrap();
        // Stopped midway: the button still comes up.
        let mut calls = 0;
        let stopped = x.draw(
            &[vec![(60, 60), (80, 60), (100, 60), (120, 60)]],
            1,
            &mut |_| {
                calls += 1;
                if calls > 2 {
                    Err(Error::Stopped("test".into()))
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(stopped, Err(Error::Stopped(_))));
        std::thread::sleep(Duration::from_millis(200));

        // Keypad keys arrive as keypad keysyms, whatever Num Lock says.
        conn.change_window_attributes(
            win,
            &xproto::ChangeWindowAttributesAux::new().event_mask(EventMask::KEY_PRESS),
        )
        .unwrap();
        conn.set_input_focus(xproto::InputFocus::PARENT, win, x11rb::CURRENT_TIME)
            .unwrap();
        conn.flush().unwrap();
        let mut keys = X11::connect().unwrap();
        // The press alone: `press` gives the spare keycode back afterwards,
        // and the mapping is read below.
        keys.press_one(&KeyCombo {
            modifiers: Modifiers::default(),
            key: Key::Named(NamedKey::Numpad(Pad::Digit(5))),
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let map = conn
            .get_keyboard_mapping(min, max - min + 1)
            .unwrap()
            .reply()
            .unwrap();
        let per = map.keysyms_per_keycode as usize;
        let mut typed = Vec::new();
        let (mut presses, mut releases, mut held_moves) = (0, 0, 0);
        let mut last = (0, 0);
        while let Some(ev) = conn.poll_for_event().unwrap() {
            match ev {
                Event::ButtonPress(_) => presses += 1,
                Event::ButtonRelease(e) => {
                    releases += 1;
                    last = (e.event_x, e.event_y);
                }
                Event::MotionNotify(e) if e.state.contains(KeyButMask::BUTTON1) => held_moves += 1,
                Event::KeyPress(e) => {
                    let i = (e.detail - min) as usize * per;
                    let shifted = e.state.contains(KeyButMask::SHIFT);
                    typed.push(map.keysyms[i + usize::from(shifted)]);
                }
                _ => {}
            }
        }
        assert_eq!(paces, 41, "a pause before each move and the stroke");
        assert_eq!((presses, releases), (2, 2));
        assert!(held_moves >= 40, "{held_moves}");
        assert_eq!(last, (80, 60), "released where the stopped stroke got to");
        assert_eq!(typed, [0xffb5], "KP_5");
    }

    #[test]
    fn control_chars_have_function_keysyms() {
        assert_eq!(keysym_for(Key::Char('\t')), Some(0xff09));
        assert_eq!(keysym_for(Key::Char('\r')), Some(0xff0d));
        assert_eq!(keysym_for(Key::Char('\n')), Some(0xff0d));
        assert_eq!(keysym_for(Key::Char('\u{8}')), Some(0xff08));
        assert_eq!(keysym_for(Key::Char('\u{1b}')), Some(0xff1b));
        assert_eq!(keysym_for(Key::Char('\u{7f}')), Some(0xffff));
        assert_eq!(keysym_for(Key::Char('\u{1}')), None);
        assert_eq!(keysym_for(Key::Char('a')), Some(0x61));
        assert_eq!(keysym_for(Key::Char('é')), Some(0xe9));
        assert_eq!(keysym_for(Key::Char('€')), Some(0x0100_20ac));
    }

    #[test]
    fn up_to_sixteen_spare_keycodes() {
        let empty = [0; 4 * 40];
        let spares = find_spares(&empty, 4, 8, SPARES);
        assert_eq!(spares.len(), 16);
        assert_eq!(spares[0], 8);
        assert_eq!(spares[15], 23);
        // Fewer when fewer are free.
        assert_eq!(find_spares(&empty[..4 * 3], 4, 8, SPARES).len(), 3);
    }

    #[test]
    fn coordinates_out_of_range_are_refused_not_wrapped() {
        assert_eq!(coord(100).unwrap(), 100);
        assert_eq!(coord(-32768).unwrap(), i16::MIN);
        assert_eq!(coord(32767).unwrap(), i16::MAX);
        // `as i16` made 40000 -25536 (the far left of the screen).
        assert!(matches!(coord(40000), Err(Error::InvalidArgs(_))));
        assert!(matches!(coord(-40000), Err(Error::InvalidArgs(_))));
        assert_eq!(clamp16(40000), i16::MAX);
        assert_eq!(clamp16(-40000), i16::MIN);
        assert_eq!(clamp16(-5), -5);
    }

    #[test]
    fn a_window_is_captured_in_its_own_coordinates() {
        let at = Rect::new(100.0, 50.0, 400.0, 300.0);
        // Inside: shifted by the window's corner.
        assert_eq!(
            window_crop(Rect::new(110.0, 60.0, 50.0, 40.0), at),
            Some((10, 10, 50, 40))
        );
        // The frame around it (title bar, shadow): only the window.
        assert_eq!(
            window_crop(Rect::new(90.0, 20.0, 420.0, 340.0), at),
            Some((0, 0, 400, 300))
        );
        // Past its right and bottom edges.
        assert_eq!(
            window_crop(Rect::new(450.0, 300.0, 100.0, 100.0), at),
            Some((350, 250, 50, 50))
        );
        assert_eq!(window_crop(Rect::new(0.0, 0.0, 50.0, 50.0), at), None);
    }

    #[test]
    fn a_connection_attempt_that_hangs_is_given_up_on() {
        static PENDING: AtomicIsize = AtomicIsize::new(0);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let t = Instant::now();
        let r = within(Duration::from_millis(100), &PENDING, move || {
            let _ = wait.recv();
            Ok(1)
        });
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        // No second attempt while that one is still waiting.
        let t = Instant::now();
        assert!(within(Duration::from_secs(5), &PENDING, || Ok(2)).is_err());
        assert!(t.elapsed() < Duration::from_secs(1));
        // Once it ends, connecting works again.
        go.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while PENDING.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            within(Duration::from_secs(5), &PENDING, || Ok(3)).unwrap(),
            3
        );
    }

    #[test]
    fn a_tcp_display_that_doesnt_answer_times_out() {
        // A non-routable address: either refused at once, or no answer.
        let t = Instant::now();
        assert!(tcp_connect("10.255.255.1", 6000, Duration::from_millis(300)).is_err());
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[test]
    fn spare_keycode_is_never_a_real_key() {
        // 3 keycodes from 8, 4 keysyms each; only 10 is completely empty
        // (9 has a keysym in its second group only).
        let syms = [0x61, 0x41, 0, 0, 0, 0, 0x6c6, 0, 0, 0, 0, 0];
        assert_eq!(find_spares(&syms, 4, 8, 4), [10]);
        assert!(find_spares(&syms[..8], 4, 8, 4).is_empty());
        let empty = [0; 16];
        assert_eq!(find_spares(&empty, 4, 8, 2), [8, 9]);
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
    fn converts_32bpp_msb_first() {
        // XRGB big-endian.
        let data = vec![0, 0x10, 0x20, 0x30];
        let l = PixelLayout {
            msb_first: true,
            ..layout(32, 32, [0xff0000, 0xff00, 0xff])
        };
        assert_eq!(
            to_rgba(data, 1, 1, &l).unwrap(),
            vec![0x10, 0x20, 0x30, 255]
        );
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
    fn scales_channels_wider_than_8_bits() {
        // 30-bit color (10 bits per channel): full red, half green.
        let word: u32 = (0x3ff << 20) | (0x200 << 10);
        let data = word.to_le_bytes().to_vec();
        let l = layout(32, 32, [0x3ff0_0000, 0x000f_fc00, 0x0000_03ff]);
        assert_eq!(to_rgba(data, 1, 1, &l).unwrap(), vec![255, 127, 0, 255]);
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
