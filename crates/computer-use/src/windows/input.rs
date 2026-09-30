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

fn key_event(vk: VIRTUAL_KEY, scan: u16, up: bool, unicode: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if unicode {
        flags |= KEYEVENTF_UNICODE;
    }
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

/// A virtual-key event with its scan code, flagged extended for the keys
/// on the extended part of the keyboard (arrows, navigation cluster…), so
/// apps reading scan codes or the extended bit see the right key.
fn vk_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    // SAFETY: a pure keyboard-layout lookup.
    let sc = unsafe { MapVirtualKeyW(u32::from(vk.0), MAPVK_VK_TO_VSC_EX) };
    let extended = matches!(sc >> 8, 0xe0 | 0xe1) || is_extended(vk);
    let mut input = key_event(vk, (sc & 0xff) as u16, up, false);
    if extended {
        // SAFETY: `ki` is the active member for INPUT_KEYBOARD.
        unsafe { input.Anonymous.ki.dwFlags |= KEYEVENTF_EXTENDEDKEY };
    }
    input
}

/// Keys that live on the extended part of the keyboard.
fn is_extended(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_LEFT
            | VK_RIGHT
            | VK_UP
            | VK_DOWN
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
            | VK_INSERT
            | VK_DELETE
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

fn tap(inputs: &mut Vec<INPUT>, vk: VIRTUAL_KEY) {
    inputs.push(vk_event(vk, false));
    inputs.push(vk_event(vk, true));
}

pub fn type_text(text: &str) -> Result<()> {
    let mut inputs = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        // Line breaks and tabs as the keys (a `\r\n` pair is one Return).
        match c {
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                tap(&mut inputs, VK_RETURN);
                continue;
            }
            '\t' => {
                tap(&mut inputs, VK_TAB);
                continue;
            }
            _ => {}
        }
        let mut buf = [0u16; 2];
        for unit in c.encode_utf16(&mut buf) {
            inputs.push(key_event(VIRTUAL_KEY(0), *unit, false, true));
            inputs.push(key_event(VIRTUAL_KEY(0), *unit, true, true));
        }
    }
    if inputs.is_empty() {
        return Ok(());
    }
    send(&inputs)
}

pub fn press(combo: &KeyCombo) -> Result<()> {
    let (vk, needed) = match (resolve_with_mods(combo.key), combo.key) {
        (Ok(r), _) => r,
        // Not on this layout: a plain character still types as unicode.
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
        (mods.ctrl, VK_CONTROL),
        (mods.alt, VK_MENU),
        (mods.shift, VK_SHIFT),
        (mods.meta, VK_LWIN),
    ] {
        if on {
            down.push(vk_event(key, false));
            up.insert(0, vk_event(key, true));
        }
    }
    down.push(vk_event(vk, false));
    down.push(vk_event(vk, true));
    down.extend(up);
    send(&down)
}

fn combo_has_command(m: Modifiers) -> bool {
    m.ctrl || m.alt || m.meta
}

/// The modifiers a `VkKeyScanW` result needs: its high byte holds Shift (1),
/// Ctrl (2) and Alt (4); Ctrl+Alt is AltGr (`@` on a German layout).
fn scan_mods(res: i16) -> Modifiers {
    let state = (res as u16) >> 8;
    Modifiers {
        shift: state & 1 != 0,
        ctrl: state & 2 != 0,
        alt: state & 4 != 0,
        meta: false,
    }
}

/// Virtual-key code for a key, and whether Shift is required.
pub(crate) fn resolve(key: Key) -> Result<(VIRTUAL_KEY, bool)> {
    resolve_with_mods(key).map(|(vk, m)| (vk, m.shift))
}

/// Virtual-key code for a key and the modifiers the layout needs for it.
fn resolve_with_mods(key: Key) -> Result<(VIRTUAL_KEY, Modifiers)> {
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
        Key::Char(c) => {
            // VkKeyScanW returns the VK in the low byte and the shift state
            // in the high byte; characters outside the BMP have no key.
            let Ok(unit) = u16::try_from(u32::from(c)) else {
                return Err(Error::ActionFailed(format!("no virtual key for {c:?}")));
            };
            let res = unsafe { VkKeyScanW(unit) };
            if res == -1 || (res as u16) >> 8 & !7 != 0 {
                return Err(Error::ActionFailed(format!("no virtual key for {c:?}")));
            }
            let vk = VIRTUAL_KEY((res & 0xff) as u16);
            return Ok((vk, scan_mods(res)));
        }
    };
    Ok((vk, Modifiers::default()))
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
}
