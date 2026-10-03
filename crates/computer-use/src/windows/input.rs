//! Synthesized input via `SendInput`, and the screenshot-pixel → screen
//! coordinate helpers Windows needs.

use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_SWAPBUTTON,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, NamedKey, Pad};
use crate::types::{MouseButton, Point};

fn send(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(Error::Platform(
            "SendInput was blocked (UIPI or a secure desktop)".into(),
        ))
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// The point is on a display. Off every display, absolute input would be
/// clamped to the desktop's edge and land on whatever window is there.
fn on_screen(p: Point) -> Result<()> {
    let pt = POINT {
        // `as` saturates (NaN gives 0).
        x: p.x.round() as i32,
        y: p.y.round() as i32,
    };
    // SAFETY: a pure lookup.
    if unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONULL) }.is_invalid() {
        return Err(Error::ActionFailed(format!(
            "({:.0}, {:.0}) is not on any display; use a point inside the window",
            p.x, p.y
        )));
    }
    Ok(())
}

/// An empty input event: no movement, no button, no key. It changes
/// nothing, but makes this process the one that sent the last input, which
/// Windows asks of a program that brings a window to the front.
pub(super) fn empty_event() -> Result<()> {
    send(&[mouse_input(MOUSE_EVENT_FLAGS(0), 0, 0, 0)])
}

/// Normalize a screen point to the 0..65535 absolute range over the virtual
/// desktop, for MOUSEEVENTF_ABSOLUTE.
fn normalize(p: Point) -> (i32, i32) {
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    let nx = ((p.x - f64::from(vx)) * 65535.0 / f64::from(vw - 1).max(1.0)).round() as i32;
    let ny = ((p.y - f64::from(vy)) * 65535.0 / f64::from(vh - 1).max(1.0)).round() as i32;
    (nx.clamp(0, 65535), ny.clamp(0, 65535))
}

fn move_to(p: Point) -> INPUT {
    let (nx, ny) = normalize(p);
    mouse_input(
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        nx,
        ny,
        0,
    )
}

/// Put the cursor back after synthesized mouse input (`restore_pointer`).
static RESTORE_POINTER: AtomicBool = AtomicBool::new(true);

pub fn set_restore_pointer(on: bool) {
    RESTORE_POINTER.store(on, Ordering::Relaxed);
}

/// Pause before putting the pointer back, so the app has handled the click
/// (or wheel) where it happened before the pointer leaves: some apps look
/// at the pointer's position when the event arrives, not where it was sent.
const RESTORE_DELAY: std::time::Duration = std::time::Duration::from_millis(40);

/// Put the pointer back (see [`home`]), on its own after a short pause.
fn restore(back: Option<INPUT>) {
    if let Some(b) = back {
        std::thread::sleep(RESTORE_DELAY);
        if let Err(e) = send(&[b]) {
            log::debug!("could not put the pointer back: {e}");
        }
    }
}

/// A move back to where the user's cursor is now, if it should be restored.
fn home() -> Option<INPUT> {
    if !RESTORE_POINTER.load(Ordering::Relaxed) {
        return None;
    }
    let mut p = POINT::default();
    // SAFETY: reading the cursor position into a local.
    unsafe { GetCursorPos(&mut p) }.ok()?;
    Some(move_to(Point::new(f64::from(p.x), f64::from(p.y))))
}

/// The down and up flags for `button`. SendInput's left and right are the
/// physical buttons: with the buttons swapped (a left-handed mouse), the
/// left button the user means is the physical right one.
fn flags_for(button: MouseButton, swapped: bool) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    match (button, swapped) {
        (MouseButton::Left, false) | (MouseButton::Right, true) => {
            (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
        }
        (MouseButton::Right, false) | (MouseButton::Left, true) => {
            (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
        }
        (MouseButton::Middle, _) => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    }
}

/// The flags for `button` with the user's current button setting.
fn button_flags(button: MouseButton) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    // SAFETY: a plain metric query.
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    flags_for(button, swapped)
}

pub fn click(at: Point, button: MouseButton, count: u8) -> Result<()> {
    on_screen(at)?;
    let (down, up) = button_flags(button);
    let back = home();
    let mut inputs = vec![move_to(at)];
    for _ in 0..count.max(1) {
        inputs.push(mouse_input(down, 0, 0, 0));
        inputs.push(mouse_input(up, 0, 0, 0));
    }
    let sent = send(&inputs);
    restore(back);
    sent
}

/// Pace of synthesized drag steps: apps that start a drag on a timer or a
/// motion threshold (SM_CXDRAG) miss a single burst of events.
const DRAG_STEP: std::time::Duration = std::time::Duration::from_millis(12);

pub fn drag(from: Point, to: Point) -> Result<()> {
    on_screen(from)?;
    on_screen(to)?;
    let back = home();
    let (down, up) = button_flags(MouseButton::Left);
    let mut pressed = false;
    let dragged = drag_steps(from, to, (down, up), &mut pressed);
    if dragged.is_err() && pressed {
        // Never leave the button held down.
        let _ = send(&[mouse_input(up, 0, 0, 0)]);
    }
    restore(back);
    dragged
}

/// The drag itself, one event at a time; `pressed` tells whether the
/// button went down (and so must come up if a later step fails).
fn drag_steps(
    from: Point,
    to: Point,
    (down, up): (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS),
    pressed: &mut bool,
) -> Result<()> {
    let step = |input: INPUT| -> Result<()> {
        send(&[input])?;
        std::thread::sleep(DRAG_STEP);
        Ok(())
    };
    step(move_to(from))?;
    step(mouse_input(down, 0, 0, 0))?;
    *pressed = true;
    for n in 1..=8 {
        step(move_to(Point::new(
            from.x + (to.x - from.x) * f64::from(n) / 8.0,
            from.y + (to.y - from.y) * f64::from(n) / 8.0,
        )))?;
    }
    send(&[mouse_input(up, 0, 0, 0)])
}

/// Move the pointer to `at`; where it was, if it should go back.
pub fn move_pointer(at: Point) -> Result<Option<Point>> {
    on_screen(at)?;
    let back = if RESTORE_POINTER.load(Ordering::Relaxed) {
        let mut p = POINT::default();
        // SAFETY: reading the cursor position into a local.
        unsafe { GetCursorPos(&mut p) }
            .ok()
            .map(|()| Point::new(f64::from(p.x), f64::from(p.y)))
    } else {
        None
    };
    send(&[move_to(at)])?;
    Ok(back)
}

/// Draw `strokes` with `button` held (see `Backend::draw`).
pub fn draw(
    strokes: &[Vec<Point>],
    button: MouseButton,
    pace: &mut dyn FnMut(f64) -> Result<()>,
) -> Result<()> {
    let flags = button_flags(button);
    let back = home();
    let mut held = false;
    let drawn = draw_strokes(strokes, flags, pace, &mut held);
    let (_, up) = flags;
    if held {
        // Never leave the button held down.
        let _ = send(&[mouse_input(up, 0, 0, 0)]);
    }
    restore(back);
    drawn
}

fn draw_strokes(
    strokes: &[Vec<Point>],
    (down, up): (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS),
    pace: &mut dyn FnMut(f64) -> Result<()>,
    held: &mut bool,
) -> Result<()> {
    for stroke in strokes {
        let Some(&first) = stroke.first() else {
            continue;
        };
        pace(0.0)?;
        send(&[move_to(first)])?;
        std::thread::sleep(DRAG_STEP);
        send(&[mouse_input(down, 0, 0, 0)])?;
        *held = true;
        std::thread::sleep(DRAG_STEP);
        let mut last = first;
        for &p in &stroke[1..] {
            pace((p.x - last.x).hypot(p.y - last.y))?;
            send(&[move_to(p)])?;
            last = p;
        }
        std::thread::sleep(DRAG_STEP);
        send(&[mouse_input(up, 0, 0, 0)])?;
        *held = false;
    }
    Ok(())
}

/// Most wheel notches one scroll sends (50 pages of 3 lines).
const MAX_WHEEL_NOTCHES: i32 = 150;

pub fn scroll(at: Point, dx: i32, dy: i32) -> Result<()> {
    const WHEEL_DELTA: i32 = 120;
    on_screen(at)?;
    // Clamped first, so the wheel delta can't overflow.
    let delta = |n: i32| {
        n.clamp(-MAX_WHEEL_NOTCHES, MAX_WHEEL_NOTCHES)
            .saturating_mul(WHEEL_DELTA)
    };
    let back = home();
    let mut inputs = vec![move_to(at)];
    if dy != 0 {
        inputs.push(mouse_input(
            MOUSEEVENTF_WHEEL,
            0,
            0,
            delta(dy).saturating_neg(),
        ));
    }
    if dx != 0 {
        inputs.push(mouse_input(MOUSEEVENTF_HWHEEL, 0, 0, delta(dx)));
    }
    let sent = send(&inputs);
    restore(back);
    sent
}

/// The keyboard layout of the window that receives the keys (the
/// foreground window's thread), so keys are looked up in the layout the
/// target app is actually using, not this process's.
fn target_layout() -> HKL {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: plain queries; a null window gives thread 0 (this thread).
    unsafe {
        let fg = GetForegroundWindow();
        // A store app's keys go to its own window inside the frame that
        // ApplicationFrameHost draws around it.
        let fg = super::uwp_core_window(fg).unwrap_or(fg);
        let thread = if fg.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        GetKeyboardLayout(thread)
    }
}

/// Keys whose scan code has the 0xE0 prefix. Without the extended flag,
/// Windows takes them for their numeric-keypad twins (with Num Lock on,
/// Delete would type "." and Left would type "4").
fn is_extended(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_INSERT
            | VK_DELETE
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
            | VK_LEFT
            | VK_RIGHT
            | VK_UP
            | VK_DOWN
            | VK_LWIN
            | VK_RWIN
            | VK_APPS
            | VK_RCONTROL
            | VK_RMENU
            | VK_DIVIDE
            | VK_NUMLOCK
            | VK_SNAPSHOT
    )
}

/// A virtual-key press or release, with the scan code apps that read the
/// hardware key (games, remote desktops, VMs) expect.
fn vk_event(vk: VIRTUAL_KEY, up: bool, layout: HKL) -> INPUT {
    // SAFETY: a pure lookup in a keyboard layout.
    let sc = unsafe { MapVirtualKeyExW(u32::from(vk.0), MAPVK_VK_TO_VSC_EX, Some(layout)) };
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if is_extended(vk) || sc & 0xff00 == 0xe000 {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    keyboard_input(vk, (sc & 0xff) as u16, flags)
}

/// A UTF-16 code unit typed as a character (`VK_PACKET`), whatever the
/// keyboard layout: any language and script.
fn unicode_event(unit: u16, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    keyboard_input(VIRTUAL_KEY(0), unit, flags)
}

fn keyboard_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Send keyboard events. If Windows takes only some of them (UIPI, a
/// secure desktop appearing midway), every key that was pressed but not
/// released is released, so no key is left held down.
fn send_keys(inputs: &[INPUT]) -> Result<()> {
    // SAFETY: a slice of fully initialised INPUT structures.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
    if sent == inputs.len() {
        return Ok(());
    }
    let mut held: Vec<INPUT> = Vec::new();
    for input in &inputs[..sent] {
        // SAFETY: every input here is a keyboard input.
        let ki = unsafe { input.Anonymous.ki };
        let same = |i: &INPUT| {
            // SAFETY: as above.
            let k = unsafe { i.Anonymous.ki };
            k.wVk == ki.wVk && k.wScan == ki.wScan
        };
        if ki.dwFlags.contains(KEYEVENTF_KEYUP) {
            held.retain(|i| !same(i));
        } else if !held.iter().any(same) {
            held.push(*input);
        }
    }
    let release: Vec<INPUT> = held
        .iter()
        .rev()
        .map(|i| {
            // SAFETY: as above.
            let mut ki = unsafe { i.Anonymous.ki };
            ki.dwFlags |= KEYEVENTF_KEYUP;
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki },
            }
        })
        .collect();
    if !release.is_empty() {
        // SAFETY: as above.
        unsafe { SendInput(&release, std::mem::size_of::<INPUT>() as i32) };
    }
    Err(Error::ActionFailed(
        "Windows blocked the keyboard input: the window in front may belong to an app running as administrator, or a secure screen (UAC prompt, lock screen) is showing".into(),
    ))
}

/// Characters per `SendInput` call when typing. Short batches with a short
/// pause between them let slow apps and remote sessions keep up, so no
/// key-up arrives late (a late key-up is what makes a key repeat).
const TYPE_BATCH: usize = 16;
const TYPE_PAUSE: std::time::Duration = std::time::Duration::from_millis(3);

pub fn type_text(text: &str) -> Result<()> {
    let layout = target_layout();
    // One key press per character; "\r\n" is one line break, not two.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let chars: Vec<char> = text.chars().collect();
    for (n, batch) in chars.chunks(TYPE_BATCH).enumerate() {
        if n > 0 {
            std::thread::sleep(TYPE_PAUSE);
        }
        let mut inputs = Vec::with_capacity(batch.len() * 2);
        for &c in batch {
            match c {
                '\n' => {
                    inputs.push(vk_event(VK_RETURN, false, layout));
                    inputs.push(vk_event(VK_RETURN, true, layout));
                }
                '\t' => {
                    inputs.push(vk_event(VK_TAB, false, layout));
                    inputs.push(vk_event(VK_TAB, true, layout));
                }
                c => {
                    let mut buf = [0u16; 2];
                    let units = c.encode_utf16(&mut buf);
                    for unit in units.iter() {
                        inputs.push(unicode_event(*unit, false));
                    }
                    for unit in units.iter() {
                        inputs.push(unicode_event(*unit, true));
                    }
                }
            }
        }
        send_keys(&inputs)?;
    }
    Ok(())
}

/// How a plain character (no modifiers) is typed in a keyboard layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharKey {
    /// The layout's key for it, with the modifiers the layout needs.
    Key(VIRTUAL_KEY, crate::keys::Modifiers),
    /// As a character (`VK_PACKET`): the layout has no key for it, or only
    /// a dead key, which would leave an accent waiting for the next key.
    Unicode,
}

/// Decide from `VkKeyScanExW`'s answer (`scan`: the key in the low byte,
/// Shift (1), Ctrl (2), Alt (4) in the high; -1 for none) and whether that
/// key is a dead key in the layout.
fn char_key(scan: i16, dead: bool) -> CharKey {
    if scan == -1 || dead {
        return CharKey::Unicode;
    }
    let state = (scan >> 8) & 0xff;
    // Other shift states (Kana, layout-specific ones) can't be pressed.
    if state & !0b111 != 0 {
        return CharKey::Unicode;
    }
    CharKey::Key(
        VIRTUAL_KEY((scan & 0xff) as u16),
        crate::keys::Modifiers {
            shift: state & 1 != 0,
            ctrl: state & 2 != 0,
            alt: state & 4 != 0,
            meta: false,
        },
    )
}

/// Whether `MapVirtualKeyExW(.., MAPVK_VK_TO_CHAR, ..)` says the key is a
/// dead key (the top bit).
fn is_dead(mapped: u32) -> bool {
    mapped & 0x8000_0000 != 0
}

/// The key that types `c` in `layout`, as the user's keyboard would: on
/// AZERTY "1" is Shift plus the "&" key, and on a Russian layout "a" has no
/// key at all (it is typed as a character).
fn plain_char_key(c: char, layout: HKL) -> CharKey {
    let mut buf = [0u16; 2];
    let [unit] = *c.encode_utf16(&mut buf) else {
        return CharKey::Unicode;
    };
    // SAFETY: pure lookups in a keyboard layout.
    let scan = unsafe { VkKeyScanExW(unit, layout) };
    let dead = scan != -1
        && is_dead(unsafe {
            MapVirtualKeyExW(
                u32::from((scan & 0xff) as u16),
                MAPVK_VK_TO_CHAR,
                Some(layout),
            )
        });
    char_key(scan, dead)
}

pub fn press(combo: &KeyCombo) -> Result<()> {
    let layout = target_layout();
    let mut mods = combo.modifiers;
    // A plain character goes through the target's layout; with modifiers,
    // letters and digits keep their fixed keys (ctrl+s, ctrl+1 work by key
    // position whatever the layout).
    if let Key::Char(c) = combo.key
        && c != ' '
        && !c.is_control()
        && !combo.modifiers.any()
    {
        match plain_char_key(c, layout) {
            CharKey::Unicode => return type_text(&c.to_string()),
            CharKey::Key(vk, extra) => return press_vk(vk, extra, layout),
        }
    }
    let vk = match resolve_in(combo.key, layout) {
        Ok((vk, extra)) => {
            mods.shift |= extra.shift;
            mods.ctrl |= extra.ctrl;
            mods.alt |= extra.alt;
            vk
        }
        // A character this layout has no key for: type it, if it is a
        // plain character and not a shortcut.
        Err(e) => match combo.key {
            Key::Char(c) if !combo.modifiers.any() => return type_text(&c.to_string()),
            _ => return Err(e),
        },
    };
    press_vk(vk, mods, layout)
}

/// Press and release `vk` with `mods` held around it.
fn press_vk(vk: VIRTUAL_KEY, mods: crate::keys::Modifiers, layout: HKL) -> Result<()> {
    let mut down = Vec::new();
    let mut up = Vec::new();
    for (on, key) in [
        (mods.ctrl, VK_LCONTROL),
        (mods.alt, VK_LMENU),
        (mods.shift, VK_LSHIFT),
        (mods.meta, VK_LWIN),
    ] {
        if on {
            down.push(vk_event(key, false, layout));
            up.insert(0, vk_event(key, true, layout));
        }
    }
    down.push(vk_event(vk, false, layout));
    down.push(vk_event(vk, true, layout));
    down.extend(up);
    send_keys(&down)
}

/// Virtual-key code for a key, and whether Shift is required (in this
/// process's keyboard layout; the stop key's registration uses it).
pub(crate) fn resolve(key: Key) -> Result<(VIRTUAL_KEY, bool)> {
    // SAFETY: this thread's layout.
    let layout = unsafe { GetKeyboardLayout(0) };
    resolve_in(key, layout).map(|(vk, m)| (vk, m.shift))
}

/// Virtual-key code for a key in `layout`, and the modifiers the layout
/// needs for it (Shift for "?", AltGr = Ctrl+Alt for "@" on some layouts).
/// Letters and digits use their fixed virtual keys, so shortcuts like
/// ctrl+s work whatever layout (Latin or not) is active.
fn resolve_in(key: Key, layout: HKL) -> Result<(VIRTUAL_KEY, crate::keys::Modifiers)> {
    let none = crate::keys::Modifiers::default();
    let vk = match key {
        Key::Named(n) => match n {
            NamedKey::Return => VK_RETURN,
            NamedKey::Tab => VK_TAB,
            NamedKey::Space => VK_SPACE,
            NamedKey::Backspace => VK_BACK,
            NamedKey::Delete => VK_DELETE,
            NamedKey::Escape => VK_ESCAPE,
            NamedKey::Home => VK_HOME,
            NamedKey::End => VK_END,
            NamedKey::PageUp => VK_PRIOR,
            NamedKey::PageDown => VK_NEXT,
            NamedKey::Left => VK_LEFT,
            NamedKey::Right => VK_RIGHT,
            NamedKey::Up => VK_UP,
            NamedKey::Down => VK_DOWN,
            NamedKey::Insert => VK_INSERT,
            NamedKey::CapsLock => VK_CAPITAL,
            NamedKey::Menu => VK_APPS,
            NamedKey::F(n) => VIRTUAL_KEY(VK_F1.0 + u16::from(n) - 1),
            NamedKey::Numpad(p) => match p {
                Pad::Digit(d) => VIRTUAL_KEY(VK_NUMPAD0.0 + u16::from(d)),
                Pad::Decimal => VK_DECIMAL,
                Pad::Add => VK_ADD,
                Pad::Subtract => VK_SUBTRACT,
                Pad::Multiply => VK_MULTIPLY,
                Pad::Divide => VK_DIVIDE,
                Pad::Enter => VK_RETURN,
            },
        },
        Key::Char(c) if c.is_ascii_alphabetic() => {
            let upper = c.to_ascii_uppercase();
            let shift = c.is_ascii_uppercase();
            return Ok((
                VIRTUAL_KEY(upper as u16),
                crate::keys::Modifiers { shift, ..none },
            ));
        }
        Key::Char(c) if c.is_ascii_digit() => return Ok((VIRTUAL_KEY(c as u16), none)),
        Key::Char(' ') => VK_SPACE,
        Key::Char(c) => {
            let mut buf = [0u16; 2];
            let units = c.encode_utf16(&mut buf);
            if units.len() != 1 {
                return Err(Error::ActionFailed(format!("no key for {c:?}")));
            }
            // The VK in the low byte; Shift (1), Ctrl (2), Alt (4) in the high.
            // SAFETY: a pure lookup in a keyboard layout.
            let res = unsafe { VkKeyScanExW(units[0], layout) };
            if res == -1 {
                return Err(Error::ActionFailed(format!(
                    "the active keyboard layout has no key for {c:?}"
                )));
            }
            let state = (res >> 8) & 0xff;
            return Ok((
                VIRTUAL_KEY((res & 0xff) as u16),
                crate::keys::Modifiers {
                    shift: state & 1 != 0,
                    ctrl: state & 2 != 0,
                    alt: state & 4 != 0,
                    ..none
                },
            ));
        }
    };
    Ok((vk, none))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Modifiers;

    #[test]
    fn swapped_buttons_swap_left_and_right_only() {
        let left = (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
        let right = (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP);
        let middle = (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP);
        assert_eq!(flags_for(MouseButton::Left, false), left);
        assert_eq!(flags_for(MouseButton::Right, false), right);
        assert_eq!(flags_for(MouseButton::Middle, false), middle);
        assert_eq!(flags_for(MouseButton::Left, true), right);
        assert_eq!(flags_for(MouseButton::Right, true), left);
        assert_eq!(flags_for(MouseButton::Middle, true), middle);
    }

    #[test]
    fn plain_characters_use_the_layouts_key() {
        // AZERTY: "1" is Shift plus the key with VK '1' (0x31).
        assert_eq!(
            char_key(0x0131, false),
            CharKey::Key(
                VIRTUAL_KEY(0x31),
                Modifiers {
                    shift: true,
                    ..Default::default()
                }
            )
        );
        // AltGr (Ctrl+Alt) for "@" on a German layout (the Q key).
        assert_eq!(
            char_key(0x0651, false),
            CharKey::Key(
                VIRTUAL_KEY(0x51),
                Modifiers {
                    ctrl: true,
                    alt: true,
                    ..Default::default()
                }
            )
        );
        // No key ("a" on a Russian layout), a dead key, or a shift state
        // that can't be pressed: typed as a character.
        assert_eq!(char_key(-1, false), CharKey::Unicode);
        assert_eq!(char_key(0x00DE, true), CharKey::Unicode);
        assert_eq!(char_key(0x0841, false), CharKey::Unicode);
        assert!(is_dead(0x8000_005E));
        assert!(!is_dead(0x0000_0041));
    }
}
