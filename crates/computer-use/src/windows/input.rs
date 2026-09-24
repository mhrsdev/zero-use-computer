//! Synthesized input via `SendInput`, and the screenshot-pixel → screen
//! coordinate helpers Windows needs.

use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, NamedKey};
use crate::types::{MouseButton, Point};

fn send(inputs: &[INPUT]) -> Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(Error::Platform("SendInput was blocked (UIPI or a secure desktop)".into()))
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

pub fn click(at: Point, button: MouseButton, count: u8) -> Result<()> {
    let (down, up) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let mut inputs = vec![move_to(at)];
    for _ in 0..count.max(1) {
        inputs.push(mouse_input(down, 0, 0, 0));
        inputs.push(mouse_input(up, 0, 0, 0));
    }
    send(&inputs)
}

pub fn drag(from: Point, to: Point) -> Result<()> {
    let mut inputs = vec![move_to(from), mouse_input(MOUSEEVENTF_LEFTDOWN, 0, 0, 0)];
    for step in 1..=8 {
        let p = Point::new(
            from.x + (to.x - from.x) * f64::from(step) / 8.0,
            from.y + (to.y - from.y) * f64::from(step) / 8.0,
        );
        inputs.push(move_to(p));
    }
    inputs.push(mouse_input(MOUSEEVENTF_LEFTUP, 0, 0, 0));
    send(&inputs)
}

pub fn scroll(at: Point, dx: i32, dy: i32) -> Result<()> {
    const WHEEL_DELTA: i32 = 120;
    let mut inputs = vec![move_to(at)];
    if dy != 0 {
        inputs.push(mouse_input(MOUSEEVENTF_WHEEL, 0, 0, -dy * WHEEL_DELTA));
    }
    if dx != 0 {
        inputs.push(mouse_input(MOUSEEVENTF_HWHEEL, 0, 0, dx * WHEEL_DELTA));
    }
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

pub fn type_text(text: &str) -> Result<()> {
    let mut inputs = Vec::new();
    for c in text.chars() {
        if c == '\n' {
            inputs.push(key_event(VK_RETURN, 0, false, false));
            inputs.push(key_event(VK_RETURN, 0, true, false));
            continue;
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
    let (vk, needs_shift) = resolve(combo.key)?;
    let mut mods = combo.modifiers;
    if needs_shift {
        mods.shift = true;
    }
    let mut down = Vec::new();
    let mut up = Vec::new();
    for (on, key) in [
        (mods.ctrl, VK_CONTROL),
        (mods.alt, VK_MENU),
        (mods.shift, VK_SHIFT),
        (mods.meta, VK_LWIN),
    ] {
        if on {
            down.push(key_event(key, 0, false, false));
            up.insert(0, key_event(key, 0, true, false));
        }
    }
    down.push(key_event(vk, 0, false, false));
    down.push(key_event(vk, 0, true, false));
    down.extend(up);
    send(&down)
}

/// Virtual-key code for a key, and whether Shift is required.
fn resolve(key: Key) -> Result<(VIRTUAL_KEY, bool)> {
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
            // VkKeyScanW returns the VK in the low byte and shift state in the high byte.
            let res = unsafe { VkKeyScanW(c as u16) };
            if res == -1 {
                return Err(Error::ActionFailed(format!("no virtual key for {c:?}")));
            }
            let vk = VIRTUAL_KEY((res & 0xff) as u16);
            let shift = (res >> 8) & 0x1 != 0;
            return Ok((vk, shift));
        }
    };
    Ok((vk, false))
}
