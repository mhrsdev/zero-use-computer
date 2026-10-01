//! Synthesized input via `SendInput`, and the screenshot-pixel → screen
//! coordinate helpers Windows needs.

use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, Modifiers, NamedKey};
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

pub fn click(at: Point, button: MouseButton, count: u8) -> Result<()> {
    let (down, up) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let back = home();
    let mut inputs = vec![move_to(at)];
    for _ in 0..count.max(1) {
        inputs.push(mouse_input(down, 0, 0, 0));
        inputs.push(mouse_input(up, 0, 0, 0));
    }
    inputs.extend(back);
    send(&inputs)
}

/// Pace of synthesized drag steps: apps that start a drag on a timer or a
/// motion threshold (SM_CXDRAG) miss a single burst of events.
const DRAG_STEP: std::time::Duration = std::time::Duration::from_millis(12);

pub fn drag(from: Point, to: Point) -> Result<()> {
    let back = home();
    let step_send = |input: INPUT| -> Result<()> {
        send(&[input])?;
        std::thread::sleep(DRAG_STEP);
        Ok(())
    };
    let dragged = (|| {
        step_send(move_to(from))?;
        step_send(mouse_input(MOUSEEVENTF_LEFTDOWN, 0, 0, 0))?;
        for step in 1..=8 {
            let p = Point::new(
                from.x + (to.x - from.x) * f64::from(step) / 8.0,
                from.y + (to.y - from.y) * f64::from(step) / 8.0,
            );
            step_send(move_to(p))?;
        }
        send(&[mouse_input(MOUSEEVENTF_LEFTUP, 0, 0, 0)])
    })();
    if dragged.is_err() {
        // Never leave the button held down.
        let _ = send(&[mouse_input(MOUSEEVENTF_LEFTUP, 0, 0, 0)]);
    }
    if let Some(b) = back {
        let _ = send(&[b]);
    }
    dragged
}

pub fn scroll(at: Point, dx: i32, dy: i32) -> Result<()> {
    const WHEEL_DELTA: i32 = 120;
    let back = home();
    let mut inputs = vec![move_to(at)];
    if dy != 0 {
        inputs.push(mouse_input(MOUSEEVENTF_WHEEL, 0, 0, -dy * WHEEL_DELTA));
    }
    if dx != 0 {
        inputs.push(mouse_input(MOUSEEVENTF_HWHEEL, 0, 0, dx * WHEEL_DELTA));
    }
    inputs.extend(back);
    send(&inputs)
}

/// The keyboard layout of the window that receives the keys (the
/// foreground window's thread), so keys are looked up in the layout the
/// target app is actually using, not this process's.
fn target_layout() -> HKL {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: plain queries; a null window gives thread 0 (this thread).
    unsafe {
        let fg = GetForegroundWindow();
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
    if is_extended(vk) || matches!(sc >> 8, 0xe0 | 0xe1) {
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
                // Line breaks and tabs as the keys.
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

pub fn press(combo: &KeyCombo) -> Result<()> {
    let layout = target_layout();
    let (vk, needed) = match (resolve_in(combo.key, layout), combo.key) {
        (Ok(r), _) => r,
        // A character this layout has no key for: a plain character (Shift
        // alone doesn't make a shortcut) still types as unicode.
        (Err(_), Key::Char(c)) if !combo_has_command(combo.modifiers) => {
            return type_text(&c.to_string());
        }
        (Err(e), _) => return Err(e),
    };
    let m = combo.modifiers;
    let mods = Modifiers {
        shift: m.shift || needed.shift,
        ctrl: m.ctrl || needed.ctrl,
        alt: m.alt || needed.alt,
        meta: m.meta,
    };
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

fn combo_has_command(m: Modifiers) -> bool {
    m.ctrl || m.alt || m.meta
}

/// The modifiers a `VkKeyScanExW` result needs: its high byte holds Shift
/// (1), Ctrl (2) and Alt (4); Ctrl+Alt is AltGr (`@` on a German layout).
fn scan_mods(res: i16) -> Modifiers {
    let state = (res as u16) >> 8;
    Modifiers {
        shift: state & 1 != 0,
        ctrl: state & 2 != 0,
        alt: state & 4 != 0,
        meta: false,
    }
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
fn resolve_in(key: Key, layout: HKL) -> Result<(VIRTUAL_KEY, Modifiers)> {
    let none = Modifiers::default();
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
        },
        Key::Char(c) if c.is_ascii_alphabetic() => {
            let shift = c.is_ascii_uppercase();
            return Ok((
                VIRTUAL_KEY(c.to_ascii_uppercase() as u16),
                Modifiers { shift, ..none },
            ));
        }
        Key::Char(c) if c.is_ascii_digit() => return Ok((VIRTUAL_KEY(c as u16), none)),
        Key::Char(' ') => VK_SPACE,
        Key::Char(c) => {
            // The VK in the low byte, the shift state in the high byte;
            // characters outside the BMP have no key.
            let Ok(unit) = u16::try_from(u32::from(c)) else {
                return Err(Error::ActionFailed(format!("no virtual key for {c:?}")));
            };
            // SAFETY: a pure lookup in a keyboard layout.
            let res = unsafe { VkKeyScanExW(unit, layout) };
            // Shift states beyond Shift/Ctrl/Alt (Hankaku, layout-specific
            // bits) can't be produced with our modifiers.
            if res == -1 || (res as u16) >> 8 & !7 != 0 {
                return Err(Error::ActionFailed(format!(
                    "the active keyboard layout has no key for {c:?}"
                )));
            }
            return Ok((VIRTUAL_KEY((res & 0xff) as u16), scan_mods(res)));
        }
    };
    Ok((vk, none))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_scan_shift_state_bits() {
        // German `@`: Q with Ctrl+Alt (AltGr).
        let at = scan_mods(0x0651);
        assert!(at.ctrl && at.alt && !at.shift);
        let upper = scan_mods(0x0141);
        assert!(upper.shift && !upper.ctrl && !upper.alt);
        assert_eq!(scan_mods(0x0041), Modifiers::default());
    }

    #[test]
    fn navigation_keys_are_extended() {
        for vk in [VK_LEFT, VK_HOME, VK_PRIOR, VK_INSERT, VK_DELETE] {
            assert!(is_extended(vk));
        }
        assert!(!is_extended(VK_RETURN));
    }

    #[test]
    fn letters_and_digits_use_fixed_virtual_keys() {
        // No layout lookup for these, so any layout handle will do.
        let layout = HKL::default();
        let (vk, m) = resolve_in(Key::Char('s'), layout).unwrap();
        assert_eq!(vk, VIRTUAL_KEY(u16::from(b'S')));
        assert!(!m.any());
        let (vk, m) = resolve_in(Key::Char('S'), layout).unwrap();
        assert_eq!(vk, VIRTUAL_KEY(u16::from(b'S')));
        assert!(m.shift && !m.ctrl && !m.alt);
        let (vk, m) = resolve_in(Key::Char('7'), layout).unwrap();
        assert_eq!(vk, VIRTUAL_KEY(u16::from(b'7')));
        assert!(!m.any());
    }
}
