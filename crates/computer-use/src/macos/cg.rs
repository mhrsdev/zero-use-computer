//! CoreGraphics input (posted to a specific pid, so it works on background
//! windows without moving the user's cursor) and window capture.

use std::ffi::c_void;
use std::time::Duration;

use core_graphics::base::{
    kCGBitmapByteOrder32Big, kCGBitmapByteOrder32Little, kCGBitmapByteOrderDefault,
    kCGImageAlphaFirst, kCGImageAlphaLast, kCGImageAlphaNoneSkipFirst, kCGImageAlphaNoneSkipLast,
    kCGImageAlphaPremultipliedFirst, kCGImageAlphaPremultipliedLast,
};
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventType, CGMouseButton, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use foreign_types::ForeignType;

use super::ffi;
use crate::error::{Error, Result};
use crate::keys::{Key, KeyCombo, Modifiers, NamedKey, Pad};
use crate::types::{Capture, MouseButton, Rect};

fn source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| Error::Platform("could not create a CGEventSource".into()))
}

fn post(pid: u32, event: &CGEvent) {
    // Deliver to the target process only, so the user's cursor never moves.
    event.post_to_pid(pid as libc::pid_t);
}

/// A mouse event at `at`, without the modifier flags the user happens to
/// hold (a held ⌘ or ⇧ would turn a click into a different one).
fn mouse_event(
    src: &CGEventSource,
    ty: CGEventType,
    at: CGPoint,
    button: CGMouseButton,
) -> Result<CGEvent> {
    let e = CGEvent::new_mouse_event(src.clone(), ty, at, button)
        .map_err(|_| Error::action("mouse event"))?;
    e.set_flags(CGEventFlags::empty());
    Ok(e)
}

/// Pace of synthesized drag steps: apps that start a drag on a timer or a
/// motion threshold miss a single burst of events.
const DRAG_STEP: Duration = Duration::from_millis(12);

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
    if let Ok(mv) = mouse_event(&src, CGEventType::MouseMoved, at, cg_btn) {
        post(pid, &mv);
    }
    for i in 1..=count.max(1) as i64 {
        let d = mouse_event(&src, down, at, cg_btn)?;
        d.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i);
        post(pid, &d);
        let u = mouse_event(&src, up, at, cg_btn)?;
        u.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i);
        post(pid, &u);
    }
    Ok(())
}

pub fn drag(pid: u32, from: CGPoint, to: CGPoint) -> Result<()> {
    let src = source()?;
    let mk = |ty, p| mouse_event(&src, ty, p, CGMouseButton::Left);
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

/// Tell the app the pointer is at `at` (posted to its process: the user's
/// cursor doesn't move).
pub fn hover(pid: u32, at: CGPoint) -> Result<()> {
    let src = source()?;
    let e = mouse_event(&src, CGEventType::MouseMoved, at, CGMouseButton::Left)?;
    post(pid, &e);
    Ok(())
}

/// Draw `strokes` with `button` held (see `Backend::draw`). Posted to the
/// app's process, so the user's cursor doesn't move.
pub fn draw(
    pid: u32,
    strokes: &[Vec<CGPoint>],
    button: MouseButton,
    pace: &mut dyn FnMut(f64) -> Result<()>,
) -> Result<()> {
    let src = source()?;
    let (down, dragged, up, cg_btn) = match button {
        MouseButton::Left => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseDragged,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
        MouseButton::Right => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseDragged,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        MouseButton::Middle => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseDragged,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
    };
    let send = |ty: CGEventType, p: CGPoint| -> Result<()> {
        post(pid, &mouse_event(&src, ty, p, cg_btn)?);
        Ok(())
    };
    for stroke in strokes {
        let Some(&first) = stroke.first() else {
            continue;
        };
        pace(0.0)?;
        send(CGEventType::MouseMoved, first)?;
        std::thread::sleep(DRAG_STEP);
        send(down, first)?;
        std::thread::sleep(DRAG_STEP);
        let mut last = first;
        let mut moved = Ok(());
        for &p in &stroke[1..] {
            moved = pace((p.x - last.x).hypot(p.y - last.y)).and_then(|()| send(dragged, p));
            if moved.is_err() {
                break;
            }
            last = p;
        }
        std::thread::sleep(DRAG_STEP);
        // Never leave the button held down, whatever stopped the stroke.
        let released = send(up, last);
        moved?;
        released?;
    }
    Ok(())
}

pub fn scroll(pid: u32, at: CGPoint, dx: i32, dy: i32) -> Result<()> {
    let src = source()?;
    if let Ok(mv) = mouse_event(&src, CGEventType::MouseMoved, at, CGMouseButton::Left) {
        post(pid, &mv);
    }
    // Negative dy scrolls content up in CG's convention (wheel1 positive = up).
    let event = CGEvent::new_scroll_event(src, ScrollEventUnit::LINE, 2, -dy, -dx, 0)
        .map_err(|_| Error::action("scroll event"))?;
    // A new scroll event sits at the user's cursor: the app scrolls the
    // view under its location, so put it at the target. And a held ⌘ or
    // ⇧ would turn it into a zoom or a sideways scroll.
    event.set_location(at);
    event.set_flags(CGEventFlags::empty());
    post(pid, &event);
    Ok(())
}

/// Longest string one keyboard event carries: apps drop what's beyond it.
const UNITS_PER_EVENT: usize = 20;

pub fn type_text(pid: u32, text: &str) -> Result<()> {
    let src = source()?;
    // Keyboard events carrying a unicode string type it verbatim, in any
    // language. Each carries at most UNITS_PER_EVENT UTF-16 units (cut
    // between characters), and every key-down gets its key-up. Flags are
    // cleared so a modifier the user happens to hold doesn't turn the text
    // into shortcuts.
    let mut chunk = String::new();
    let mut units = 0;
    let mut chunks = Vec::new();
    for c in text.chars() {
        if units + c.len_utf16() > UNITS_PER_EVENT && !chunk.is_empty() {
            chunks.push(std::mem::take(&mut chunk));
            units = 0;
        }
        units += c.len_utf16();
        chunk.push(c);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    for (i, part) in chunks.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(src.clone(), 0, down)
                .map_err(|_| Error::action("keyboard event"))?;
            event.set_flags(CGEventFlags::empty());
            event.set_string(part);
            post(pid, &event);
        }
    }
    Ok(())
}

pub fn press(pid: u32, combo: &KeyCombo) -> Result<()> {
    let src = source()?;
    match keycode(combo.key) {
        Some((code, shift)) => {
            let mut mods = combo.modifiers;
            if shift {
                mods.shift = true;
            }
            let mut f = flags(mods);
            if matches!(combo.key, Key::Named(NamedKey::Numpad(_))) {
                f |= CGEventFlags::CGEventFlagNumericPad;
            }
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
    let ids = CGDisplay::active_displays().unwrap_or_default();
    let mut rects = ids.into_iter().map(|id| CGDisplay::new(id).bounds());
    let first = rects.next().unwrap_or_else(|| CGDisplay::main().bounds());
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

/// What to do without the Screen Recording permission.
pub const SCREEN_RECORDING_HELP: &str = "screenshots would show only the desktop wallpaper: enable the app that runs this server (your terminal or MCP client) under System Settings ▸ Privacy & Security ▸ Screen Recording, then restart it";

/// Without Screen Recording, captures "succeed" with the wallpaper only.
fn ensure_capture_allowed() -> Result<()> {
    if ffi::screen_capture_allowed() == Some(false) {
        return Err(Error::Permission(format!(
            "Screen Recording — {SCREEN_RECORDING_HELP}"
        )));
    }
    Ok(())
}

/// The CGWindowID of `pid`'s window with these bounds (screen coordinates),
/// from the window server's list: for a window whose AX element doesn't
/// tell it.
pub fn find_window(pid: u32, rect: Rect) -> Option<u32> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFStringRef;
    use core_graphics::window::{
        copy_window_info, kCGWindowBounds, kCGWindowNumber, kCGWindowOwnerPID,
    };

    // Every window (on other Spaces too), front to back.
    let list = copy_window_info(
        ffi::kCGWindowListExcludeDesktopElements,
        ffi::kCGNullWindowID,
    )?;
    let mut best: Option<(u32, f64)> = None;
    for raw in list.get_all_values() {
        if raw.is_null() {
            continue;
        }
        // SAFETY: an item of a live array, retained while wrapped.
        let item = unsafe { CFType::wrap_under_get_rule(raw) };
        let Some(dict) = item.downcast::<CFDictionary>() else {
            continue;
        };
        let get = |key: CFStringRef| {
            dict.find(key as *const c_void)
                .filter(|v| !v.is_null())
                // SAFETY: a value of a live dictionary, retained while wrapped.
                .map(|v| unsafe { CFType::wrap_under_get_rule(*v) })
        };
        let num = |key| {
            get(key)
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };
        // SAFETY: reading CoreGraphics' constant keys.
        let (owner, number, bounds) =
            unsafe { (kCGWindowOwnerPID, kCGWindowNumber, kCGWindowBounds) };
        if num(owner) != Some(i64::from(pid)) {
            continue;
        }
        let Some(id) = num(number).and_then(|n| u32::try_from(n).ok()) else {
            continue;
        };
        let Some(b) = get(bounds)
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|d| CGRect::from_dict_representation(&d))
        else {
            continue;
        };
        let off = (b.origin.x - rect.x).abs()
            + (b.origin.y - rect.y).abs()
            + (b.size.width - rect.width).abs()
            + (b.size.height - rect.height).abs();
        if off < 4.0 && best.is_none_or(|(_, o)| off < o) {
            best = Some((id, off));
        }
    }
    best.map(|(id, _)| id)
}

/// Capture the whole desktop (every display), or a screen-space rectangle
/// of it.
pub fn capture_screen(region: Option<Rect>) -> Result<Capture> {
    ensure_capture_allowed()?;
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
    ensure_capture_allowed()?;
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
            "screen capture failed (the window may be gone, or Screen Recording is not granted)"
                .into(),
        ));
    }
    // SAFETY: a +1 CGImageRef from a Create call, released on drop.
    let image = unsafe { CGImage::from_ptr(ptr as *mut _) };
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err(Error::Platform("unexpected capture format".into()));
    }
    // SAFETY: `ptr` is the live CGImage held by `image`.
    let info = unsafe { ffi::CGImageGetBitmapInfo(ptr) };
    let rgba = match rgb_offsets(&image, info).and_then(|o| copy_pixels(&image, o)) {
        Some(rgba) => rgba,
        // Any other layout (16-bit or float components, 24-bit pixels…):
        // let CoreGraphics convert it.
        None => redraw(&image),
    };
    Ok(Capture {
        width: width as u32,
        height: height as u32,
        rgba,
        bounds: rect,
    })
}

const ALPHA_INFO_MASK: u32 = 0x1F;
const BYTE_ORDER_MASK: u32 = 0x7000;
const FLOAT_COMPONENTS: u32 = 1 << 8;

/// Where R, G and B sit in each 4-byte pixel of `image`, read off its
/// bitmap info; `None` for a layout other than 8-bit RGB(A) in 32 bits.
fn rgb_offsets(image: &CGImage, info: u32) -> Option<(usize, usize, usize)> {
    if image.bits_per_component() != 8
        || image.bits_per_pixel() != 32
        || info & FLOAT_COMPONENTS != 0
    {
        return None;
    }
    let alpha = info & ALPHA_INFO_MASK;
    let alpha_first = if [
        kCGImageAlphaPremultipliedFirst,
        kCGImageAlphaFirst,
        kCGImageAlphaNoneSkipFirst,
    ]
    .contains(&alpha)
    {
        true
    } else if [
        kCGImageAlphaPremultipliedLast,
        kCGImageAlphaLast,
        kCGImageAlphaNoneSkipLast,
    ]
    .contains(&alpha)
    {
        false
    } else {
        return None;
    };
    // A 32-bit little-endian pixel lies in memory in reverse order.
    let order = info & BYTE_ORDER_MASK;
    let little = if order == kCGBitmapByteOrder32Little {
        true
    } else if order == kCGBitmapByteOrderDefault || order == kCGBitmapByteOrder32Big {
        false
    } else {
        return None;
    };
    Some(match (alpha_first, little) {
        (true, false) => (1, 2, 3),  // A R G B
        (false, false) => (0, 1, 2), // R G B A
        (true, true) => (2, 1, 0),   // B G R A
        (false, true) => (3, 2, 1),  // A B G R
    })
}

/// The image's pixels as opaque RGBA, from 4-byte pixels with R, G and B at
/// these offsets.
fn copy_pixels(image: &CGImage, (r, g, b): (usize, usize, usize)) -> Option<Vec<u8>> {
    let (w, h, bpr) = (image.width(), image.height(), image.bytes_per_row());
    let data = image.data();
    let bytes = data.bytes();
    if bpr < w * 4 || bytes.len() < (h - 1) * bpr + w * 4 {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in bytes.chunks(bpr).take(h) {
        for p in row[..w * 4].as_chunks::<4>().0 {
            rgba.extend_from_slice(&[p[r], p[g], p[b], 255]);
        }
    }
    Some(rgba)
}

/// The image drawn into an RGBX bitmap (8 bits each, in this byte order),
/// then made opaque RGBA.
fn redraw(image: &CGImage) -> Vec<u8> {
    let (w, h) = (image.width(), image.height());
    let mut buf = vec![0u8; w * h * 4];
    {
        let ctx = CGContext::create_bitmap_context(
            Some(buf.as_mut_ptr() as *mut c_void),
            w,
            h,
            8,
            w * 4,
            &CGColorSpace::create_device_rgb(),
            kCGImageAlphaNoneSkipLast | kCGBitmapByteOrder32Big,
        );
        let all = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64));
        ctx.draw_image(all, image);
    }
    for p in buf.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    buf
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
            NamedKey::Numpad(p) => match p {
                Pad::Digit(d) => [82, 83, 84, 85, 86, 87, 88, 89, 91, 92][usize::from(d.min(9))],
                Pad::Decimal => 65,
                Pad::Multiply => 67,
                Pad::Add => 69,
                Pad::Divide => 75,
                Pad::Subtract => 78,
                Pad::Enter => 76,
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
