//! CoreGraphics input (posted to a specific pid, so it works on background
//! windows without moving the user's cursor) and window capture.

use std::ffi::c_void;
use std::time::Duration;

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventType, CGMouseButton, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use foreign_types::ForeignType;

use super::ffi;
use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, Modifiers, NamedKey};
use crate::types::{Capture, MouseButton, Rect};

fn source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| Error::Platform("could not create a CGEventSource".into()))
}

fn post(pid: u32, event: &CGEvent) {
    // Deliver to the target process only, so the user's cursor never moves.
    event.post_to_pid(pid as libc::pid_t);
}

/// Pace of synthesized drag steps: apps that start a drag on a timer or a
/// motion threshold miss a single burst of events.
const DRAG_STEP: Duration = Duration::from_millis(12);

/// CGEventKeyboardSetUnicodeString only honours about 20 UTF-16 units per
/// event.
const UNICODE_CHUNK: usize = 20;

fn flags(m: Modifiers) -> CGEventFlags {
    let mut f = CGEventFlags::empty();
    if m.shift {
        f |= CGEventFlags::CGEventFlagShift;
    }
    if m.ctrl {
        f |= CGEventFlags::CGEventFlagControl;
    }
    if m.alt {
        f |= CGEventFlags::CGEventFlagAlternate;
    }
    if m.meta {
        f |= CGEventFlags::CGEventFlagCommand;
    }
    f
}

pub fn click(pid: u32, at: CGPoint, button: MouseButton, count: u8) -> Result<()> {
    let src = source()?;
    let (down, up, cg_btn) = match button {
        MouseButton::Left => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
        MouseButton::Right => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        MouseButton::Middle => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
    };
    // Move first so hover state is correct.
    if let Ok(mv) = CGEvent::new_mouse_event(src.clone(), CGEventType::MouseMoved, at, cg_btn) {
        post(pid, &mv);
    }
    for i in 1..=count.max(1) as i64 {
        let d = CGEvent::new_mouse_event(src.clone(), down, at, cg_btn)
            .map_err(|_| Error::action("mouse down event"))?;
        d.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i);
        post(pid, &d);
        let u = CGEvent::new_mouse_event(src.clone(), up, at, cg_btn)
            .map_err(|_| Error::action("mouse up event"))?;
        u.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i);
        post(pid, &u);
    }
    Ok(())
}

pub fn drag(pid: u32, from: CGPoint, to: CGPoint) -> Result<()> {
    let src = source()?;
    let mk = |ty, p| CGEvent::new_mouse_event(src.clone(), ty, p, CGMouseButton::Left);
    if let Ok(e) = mk(CGEventType::MouseMoved, from) {
        post(pid, &e);
    }
    std::thread::sleep(DRAG_STEP);
    if let Ok(e) = mk(CGEventType::LeftMouseDown, from) {
        post(pid, &e);
    }
    std::thread::sleep(DRAG_STEP);
    for step in 1..=8 {
        let p = CGPoint {
            x: from.x + (to.x - from.x) * f64::from(step) / 8.0,
            y: from.y + (to.y - from.y) * f64::from(step) / 8.0,
        };
        if let Ok(e) = mk(CGEventType::LeftMouseDragged, p) {
            post(pid, &e);
        }
        std::thread::sleep(DRAG_STEP);
    }
    if let Ok(e) = mk(CGEventType::LeftMouseUp, to) {
        post(pid, &e);
    }
    Ok(())
}

pub fn scroll(pid: u32, at: CGPoint, dx: i32, dy: i32) -> Result<()> {
    let src = source()?;
    if let Ok(mv) = CGEvent::new_mouse_event(
        src.clone(),
        CGEventType::MouseMoved,
        at,
        CGMouseButton::Left,
    ) {
        post(pid, &mv);
    }
    // Negative dy scrolls content up in CG's convention (wheel1 positive = up).
    let event = CGEvent::new_scroll_event(src, ScrollEventUnit::LINE, 2, -dy, -dx, 0)
        .map_err(|_| Error::action("scroll event"))?;
    post(pid, &event);
    Ok(())
}

pub fn type_text(pid: u32, text: &str) -> Result<()> {
    let src = source()?;
    // Keyboard events carrying a unicode string type it verbatim, in any
    // language, a chunk per down/up pair (with a short pause between chunks
    // so the app keeps up). Flags are cleared so a modifier the user happens
    // to hold doesn't turn the text into shortcuts.
    for (i, chunk) in utf16_chunks(text, UNICODE_CHUNK).iter().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(src.clone(), 0, down)
                .map_err(|_| Error::action("keyboard event"))?;
            event.set_flags(CGEventFlags::empty());
            event.set_string_from_utf16_unchecked(chunk);
            post(pid, &event);
        }
    }
    Ok(())
}

/// `text` as UTF-16 chunks of at most `max` units, never splitting a
/// surrogate pair.
fn utf16_chunks(text: &str, max: usize) -> Vec<Vec<u16>> {
    let max = max.max(2);
    let mut out = Vec::new();
    let mut cur: Vec<u16> = Vec::with_capacity(max);
    let mut buf = [0u16; 2];
    for c in text.chars() {
        let units = c.encode_utf16(&mut buf);
        if cur.len() + units.len() > max {
            out.push(std::mem::replace(&mut cur, Vec::with_capacity(max)));
        }
        cur.extend_from_slice(units);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn press(pid: u32, combo: &KeyCombo) -> Result<()> {
    let src = source()?;
    match keycode(combo.key) {
        Some((code, shift)) => {
            let mut mods = combo.modifiers;
            if shift {
                mods.shift = true;
            }
            let f = flags(mods);
            let down = CGEvent::new_keyboard_event(src.clone(), code, true)
                .map_err(|_| Error::action("key down"))?;
            down.set_flags(f);
            post(pid, &down);
            let up = CGEvent::new_keyboard_event(src, code, false)
                .map_err(|_| Error::action("key up"))?;
            up.set_flags(f);
            post(pid, &up);
            Ok(())
        }
        None => match combo.key {
            // No virtual keycode: type the character as a unicode string,
            // unless it is combined with a command/control shortcut.
            Key::Char(c) if !(combo.modifiers.ctrl || combo.modifiers.meta) => {
                type_text(pid, &c.to_string())
            }
            _ => Err(Error::ActionFailed(format!(
                "no macOS key code for {:?}",
                combo.key
            ))),
        },
    }
}

/// The union of all active displays' bounds (global coordinates), falling
/// back to the main display.
fn desktop_bounds() -> Rect {
    let ids = core_graphics::display::CGDisplay::active_displays().unwrap_or_default();
    let mut rects = ids
        .into_iter()
        .map(|id| unsafe { ffi::CGDisplayBounds(id) });
    let first = rects
        .next()
        .unwrap_or_else(|| unsafe { ffi::CGDisplayBounds(ffi::CGMainDisplayID()) });
    let (mut x0, mut y0) = (first.origin.x, first.origin.y);
    let (mut x1, mut y1) = (x0 + first.size.width, y0 + first.size.height);
    for b in rects {
        x0 = x0.min(b.origin.x);
        y0 = y0.min(b.origin.y);
        x1 = x1.max(b.origin.x + b.size.width);
        y1 = y1.max(b.origin.y + b.size.height);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Capture the whole desktop (every display), or a screen-space rectangle
/// of it.
pub fn capture_screen(region: Option<Rect>) -> Result<Capture> {
    let rect = region.unwrap_or_else(desktop_bounds);
    let bounds = CGRect {
        origin: CGPoint {
            x: rect.x,
            y: rect.y,
        },
        size: CGSize {
            width: rect.width,
            height: rect.height,
        },
    };
    let ptr = unsafe {
        ffi::CGWindowListCreateImage(
            bounds,
            ffi::kCGWindowListOptionOnScreenOnly,
            ffi::kCGNullWindowID,
            ffi::kCGWindowImageBoundsIgnoreFraming | ffi::kCGWindowImageBestResolution,
        )
    };
    capture_from_image(ptr, rect)
}

pub fn capture_window(window_id: u32, rect: Rect) -> Result<Capture> {
    // CGRectNull tells CoreGraphics to use the window's own bounds.
    let null_rect = CGRect {
        origin: CGPoint {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    };
    let ptr = unsafe {
        ffi::CGWindowListCreateImage(
            null_rect,
            ffi::kCGWindowListOptionIncludingWindow,
            window_id,
            ffi::kCGWindowImageBoundsIgnoreFraming | ffi::kCGWindowImageBestResolution,
        )
    };
    capture_from_image(ptr, rect)
}

fn capture_from_image(ptr: *const c_void, rect: Rect) -> Result<Capture> {
    if ptr.is_null() {
        return Err(Error::Platform(
            "screen capture failed (grant Screen Recording permission)".into(),
        ));
    }
    let image = unsafe { CGImage::from_ptr(ptr as *mut _) };
    let width = image.width() as u32;
    let height = image.height() as u32;
    let bpr = image.bytes_per_row();
    let bpp = image.bits_per_pixel() / 8;
    let data = image.data();
    let bytes = data.bytes();
    if width == 0 || height == 0 || bpp < 3 {
        return Err(Error::Platform("unexpected capture format".into()));
    }
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        let row = &bytes[y * bpr..];
        for x in 0..width as usize {
            let p = &row[x * bpp..x * bpp + bpp];
            // CoreGraphics window images are little-endian ARGB → B,G,R,A.
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    Ok(Capture {
        width,
        height,
        rgba,
        bounds: rect,
    })
}

/// macOS virtual key code for a key, and whether it needs Shift.
pub(crate) fn keycode(key: Key) -> Option<(u16, bool)> {
    let code = match key {
        Key::Named(n) => match n {
            NamedKey::Return => 36,
            NamedKey::Tab => 48,
            NamedKey::Space => 49,
            NamedKey::Backspace => 51,
            NamedKey::Delete => 117,
            NamedKey::Escape => 53,
            NamedKey::Home => 115,
            NamedKey::End => 119,
            NamedKey::PageUp => 116,
            NamedKey::PageDown => 121,
            NamedKey::Left => 123,
            NamedKey::Right => 124,
            NamedKey::Down => 125,
            NamedKey::Up => 126,
            NamedKey::Insert => return None,
            NamedKey::CapsLock => 57,
            NamedKey::Menu => return None,
            NamedKey::F(n) => match n {
                1 => 122,
                2 => 120,
                3 => 99,
                4 => 118,
                5 => 96,
                6 => 97,
                7 => 98,
                8 => 100,
                9 => 101,
                10 => 109,
                11 => 103,
                12 => 111,
                13 => 105,
                14 => 107,
                15 => 113,
                16 => 106,
                17 => 64,
                18 => 79,
                19 => 80,
                20 => 90,
                _ => return None,
            },
        },
        Key::Char(c) => return char_keycode(c),
    };
    Some((code, false))
}

fn char_keycode(c: char) -> Option<(u16, bool)> {
    let lower = c.to_ascii_lowercase();
    let base: u16 = match lower {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        'o' => 31,
        'u' => 32,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        'k' => 40,
        'n' => 45,
        'm' => 46,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '5' => 23,
        '6' => 22,
        '7' => 26,
        '8' => 28,
        '9' => 25,
        '0' => 29,
        '=' => 24,
        '-' => 27,
        ']' => 30,
        '[' => 33,
        '\'' => 39,
        ';' => 41,
        '\\' => 42,
        ',' => 43,
        '/' => 44,
        '.' => 47,
        '`' => 50,
        ' ' => 49,
        _ => return None,
    };
    // Uppercase letters need Shift; other characters use their unshifted code.
    Some((base, c.is_ascii_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::utf16_chunks;

    #[test]
    fn unicode_chunks_keep_surrogate_pairs_whole() {
        let text = "a".repeat(19) + "😀" + "bc";
        let chunks = utf16_chunks(&text, 20);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 19);
        assert_eq!(chunks[1].len(), 4);
        assert!(chunks.iter().all(|c| c.len() <= 20));
        let joined: Vec<u16> = chunks.concat();
        assert_eq!(String::from_utf16(&joined).unwrap(), text);
        assert!(utf16_chunks("", 20).is_empty());
        assert_eq!(utf16_chunks(&"x".repeat(45), 20).len(), 3);
    }
}
