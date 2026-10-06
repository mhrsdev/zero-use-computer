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

use super::{ffi, layout};
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

/// Address a mouse or scroll event to `window` (a CGWindowID), as the
/// window server does for events it routes itself: the app then handles it
/// in that window even when another one of its windows is in front there.
fn aim(e: &CGEvent, window: Option<u32>) {
    if let Some(id) = window {
        for field in [
            EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER,
            EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER_THAT_CAN_HANDLE_THIS_EVENT,
        ] {
            e.set_integer_value_field(field, i64::from(id));
        }
    }
}

/// A mouse event at `at` for `window`, without the modifier flags the user
/// happens to hold (a held ⌘ or ⇧ would turn a click into a different one).
fn mouse_event(
    src: &CGEventSource,
    ty: CGEventType,
    at: CGPoint,
    button: CGMouseButton,
    window: Option<u32>,
) -> Result<CGEvent> {
    let e = CGEvent::new_mouse_event(src.clone(), ty, at, button)
        .map_err(|_| Error::action("mouse event"))?;
    e.set_flags(CGEventFlags::empty());
    aim(&e, window);
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

pub fn click(
    pid: u32,
    window: Option<u32>,
    at: CGPoint,
    button: MouseButton,
    count: u8,
) -> Result<()> {
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
    if let Ok(mv) = mouse_event(&src, CGEventType::MouseMoved, at, cg_btn, window) {
        post(pid, &mv);
    }
    for i in 1..=count.max(1) as i64 {
        // Build the release before pressing, so a failure never leaves the
        // button held.
        let d = mouse_event(&src, down, at, cg_btn, window)?;
        let u = mouse_event(&src, up, at, cg_btn, window)?;
        d.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i);
        u.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i);
        post(pid, &d);
        post(pid, &u);
    }
    Ok(())
}

pub fn drag(pid: u32, window: Option<u32>, from: CGPoint, to: CGPoint) -> Result<()> {
    let src = source()?;
    let mk = |ty, p| mouse_event(&src, ty, p, CGMouseButton::Left, window);
    if let Ok(e) = mk(CGEventType::MouseMoved, from) {
        post(pid, &e);
    }
    std::thread::sleep(DRAG_STEP);
    // A press that can't be made is an error, not a silent no-op. A spare
    // release is made with it, so the button is never left held.
    let down = mk(CGEventType::LeftMouseDown, from)?;
    let spare_up = mk(CGEventType::LeftMouseUp, from)?;
    post(pid, &down);
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
    match mk(CGEventType::LeftMouseUp, to) {
        Ok(up) => {
            post(pid, &up);
            Ok(())
        }
        Err(e) => {
            post(pid, &spare_up);
            Err(e)
        }
    }
}

/// Tell the app the pointer is at `at` (posted to its process: the user's
/// cursor doesn't move).
pub fn hover(pid: u32, window: Option<u32>, at: CGPoint) -> Result<()> {
    let src = source()?;
    let e = mouse_event(
        &src,
        CGEventType::MouseMoved,
        at,
        CGMouseButton::Left,
        window,
    )?;
    post(pid, &e);
    Ok(())
}

/// Draw `strokes` with `button` held (see `Backend::draw`). Posted to the
/// app's process, so the user's cursor doesn't move.
pub fn draw(
    pid: u32,
    window: Option<u32>,
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
        post(pid, &mouse_event(&src, ty, p, cg_btn, window)?);
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

pub fn scroll(pid: u32, window: Option<u32>, at: CGPoint, dx: i32, dy: i32) -> Result<()> {
    let src = source()?;
    if let Ok(mv) = mouse_event(
        &src,
        CGEventType::MouseMoved,
        at,
        CGMouseButton::Left,
        window,
    ) {
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
    aim(&event, window);
    post(pid, &event);
    Ok(())
}

/// Longest string one keyboard event carries: apps drop what's beyond it.
const UNITS_PER_EVENT: usize = 20;

/// How long an app may take to handle typed text before an input method
/// switched away from comes back (events are delivered asynchronously).
const INPUT_METHOD_SETTLE: Duration = Duration::from_millis(150);

pub fn type_text(pid: u32, text: &str) -> Result<()> {
    let src = source()?;
    // An input method (Japanese, Chinese…) would compose the typed keys
    // into something else: type with the ASCII-capable layout meanwhile.
    // It comes back when this guard drops, on every path.
    let ascii = layout::ascii_input();
    if ascii.switched() {
        std::thread::sleep(Duration::from_millis(30));
    }
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
    if ascii.switched() {
        std::thread::sleep(INPUT_METHOD_SETTLE);
    }
    Ok(())
}

pub fn press(pid: u32, combo: &KeyCombo) -> Result<()> {
    let src = source()?;
    // A dead key pressed on its own only starts an accent, typing nothing:
    // the character is typed as text instead. With a modifier it is a
    // shortcut, which doesn't compose, so the key is right there.
    let key = match key_stroke(combo) {
        Some((_, _, true)) if !combo.modifiers.any() => None,
        k => k.map(|(code, extra, _)| (code, extra)),
    };
    match key {
        Some((code, extra)) => {
            let mut mods = combo.modifiers;
            mods.shift |= extra.shift;
            mods.alt |= extra.alt;
            let mut f = flags(mods);
            if matches!(combo.key, Key::Named(NamedKey::Numpad(_))) {
                f |= CGEventFlags::CGEventFlagNumericPad;
            }
            // A character pressed on its own also carries its text, so an
            // app reading the event's string gets it whatever the layout.
            let text = match combo.key {
                Key::Char(c) if !combo.modifiers.any() => Some(c.to_string()),
                _ => None,
            };
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(src.clone(), code, down)
                    .map_err(|_| Error::action(if down { "key down" } else { "key up" }))?;
                event.set_flags(f);
                if let Some(text) = &text {
                    event.set_string(text);
                }
                post(pid, &event);
            }
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

/// Most pixels captured at the displays' full (Retina) resolution: past it,
/// one pixel per point.
const BEST_RESOLUTION_MAX_PIXELS: f64 = 40e6;
/// Most pixels one screen capture may have at all (400 MB of RGBA).
const CAPTURE_MAX_PIXELS: f64 = 100e6;

/// Whether to capture `width`×`height` points at the displays' full
/// resolution (`scale` pixels per point, the highest of them): only while
/// that stays under [`BEST_RESOLUTION_MAX_PIXELS`]; else at one pixel per
/// point, or not at all past [`CAPTURE_MAX_PIXELS`].
fn best_resolution(width: f64, height: f64, scale: f64) -> Result<bool> {
    let points = width.max(0.0) * height.max(0.0);
    let scale = if scale.is_finite() {
        scale.max(1.0)
    } else {
        1.0
    };
    if !points.is_finite() || points > CAPTURE_MAX_PIXELS {
        return Err(Error::ActionFailed(format!(
            "an area of {width:.0}×{height:.0} is too large to capture in one image (at most {:.0} million pixels): capture one display or a part of it (screenshot mode=region with x, y, width, height), or a window",
            CAPTURE_MAX_PIXELS / 1e6
        )));
    }
    Ok(points * scale * scale <= BEST_RESOLUTION_MAX_PIXELS)
}

/// The highest pixels-per-point of the active displays (2 on Retina).
fn max_scale() -> f64 {
    let ids = CGDisplay::active_displays().unwrap_or_default();
    ids.into_iter()
        .filter_map(|id| {
            let display = CGDisplay::new(id);
            let points = display.bounds().size.width;
            let pixels = display.display_mode()?.pixel_width() as f64;
            (points > 0.0 && pixels > 0.0).then(|| pixels / points)
        })
        .fold(1.0, f64::max)
}

/// How many of `pid`'s ordinary windows (layer 0, visible, at least
/// 50×50) the window server has off screen: on other Spaces or in full
/// screen, where the Accessibility API doesn't list them.
pub fn offscreen_windows(pid: u32) -> usize {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFStringRef;
    use core_graphics::window::{
        copy_window_info, kCGWindowAlpha, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
        kCGWindowListOptionAll, kCGWindowOwnerPID,
    };

    let Some(list) = copy_window_info(kCGWindowListOptionAll, ffi::kCGNullWindowID) else {
        return 0;
    };
    let mut count = 0;
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
                .and_then(|n| n.to_f64())
        };
        // SAFETY: reading CoreGraphics' constant keys.
        let (owner, layer, alpha, onscreen, bounds) = unsafe {
            (
                kCGWindowOwnerPID,
                kCGWindowLayer,
                kCGWindowAlpha,
                kCGWindowIsOnscreen,
                kCGWindowBounds,
            )
        };
        if num(owner) != Some(f64::from(pid)) || num(layer) != Some(0.0) {
            continue;
        }
        let on_screen = get(onscreen)
            .and_then(|v| v.downcast::<CFBoolean>())
            .is_some_and(|b| b == CFBoolean::true_value());
        let big = get(bounds)
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|d| CGRect::from_dict_representation(&d))
            .is_some_and(|b| b.size.width >= 50.0 && b.size.height >= 50.0);
        if !on_screen && big && num(alpha).is_none_or(|a| a > 0.0) {
            count += 1;
        }
    }
    count
}

/// Capture the whole desktop (every display), or a screen-space rectangle
/// of it.
pub fn capture_screen(region: Option<Rect>) -> Result<Capture> {
    ensure_capture_allowed()?;
    let rect = region.unwrap_or_else(desktop_bounds);
    let resolution = if best_resolution(rect.width, rect.height, max_scale())? {
        ffi::kCGWindowImageBestResolution
    } else {
        log::debug!(
            "capturing {:.0}×{:.0} points at one pixel per point (too large for full resolution)",
            rect.width,
            rect.height
        );
        ffi::kCGWindowImageNominalResolution
    };
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
            ffi::kCGWindowImageBoundsIgnoreFraming | resolution,
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
    let direct = match rgb_offsets(&image, info) {
        Some(offsets) => copy_pixels(&image, offsets)?,
        None => None,
    };
    let rgba = match direct {
        Some(rgba) => rgba,
        // Any other layout (16-bit or float components, 24-bit pixels…),
        // or pixels that can't be read directly: let CoreGraphics convert it.
        None => redraw(&image)?,
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
/// these offsets; `None` when its rows aren't laid out as expected.
fn copy_pixels(image: &CGImage, (r, g, b): (usize, usize, usize)) -> Result<Option<Vec<u8>>> {
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;

    let (w, h, bpr) = (image.width(), image.height(), image.bytes_per_row());
    // Not `CGImage::data`: it panics when the provider gives no data.
    // SAFETY: a live image; its provider is +0, the copy +1 (released on
    // drop), each checked for null.
    let data = unsafe {
        let provider = ffi::CGImageGetDataProvider(image.as_ptr() as *const c_void);
        let raw = if provider.is_null() {
            std::ptr::null()
        } else {
            ffi::CGDataProviderCopyData(provider)
        };
        if raw.is_null() {
            return Err(Error::Platform(
                "the captured image's pixels could not be read".into(),
            ));
        }
        CFData::wrap_under_create_rule(raw as _)
    };
    let bytes = data.bytes();
    // Each row holds `w` pixels, and the last one ends within the data.
    let fits = || {
        let row = w.checked_mul(4)?;
        let needed = h.checked_sub(1)?.checked_mul(bpr)?.checked_add(row)?;
        Some(bpr >= row && bytes.len() >= needed)
    };
    if fits() != Some(true) {
        return Ok(None);
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in bytes.chunks(bpr).take(h) {
        for p in row[..w * 4].as_chunks::<4>().0 {
            rgba.extend_from_slice(&[p[r], p[g], p[b], 255]);
        }
    }
    Ok(Some(rgba))
}

/// The image drawn into an RGBX bitmap (8 bits each, in this byte order),
/// then made opaque RGBA.
fn redraw(image: &CGImage) -> Result<Vec<u8>> {
    let (w, h) = (image.width(), image.height());
    let unreadable = || Error::Platform("the captured image could not be read".into());
    let row = w.checked_mul(4).ok_or_else(unreadable)?;
    let len = row.checked_mul(h).ok_or_else(unreadable)?;
    if len == 0 {
        return Err(unreadable());
    }
    let mut buf = vec![0u8; len];
    {
        let space = CGColorSpace::create_device_rgb();
        // Not `CGContext::create_bitmap_context`: it panics on failure.
        // SAFETY: `buf` holds `h` rows of `row` bytes and outlives the
        // context; the result is +1 (null checked), released on drop.
        let ctx = unsafe {
            let raw = ffi::CGBitmapContextCreate(
                buf.as_mut_ptr() as *mut c_void,
                w,
                h,
                8,
                row,
                space.as_ptr() as *const c_void,
                kCGImageAlphaNoneSkipLast | kCGBitmapByteOrder32Big,
            );
            if raw.is_null() {
                return Err(unreadable());
            }
            CGContext::from_ptr(raw as *mut _)
        };
        let all = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64));
        ctx.draw_image(all, image);
    }
    for p in buf.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    Ok(buf)
}

/// macOS virtual key code for a combo's key, and the modifiers the key
/// needs besides the combo's own (shift or option, for a character typed
/// with them on this layout). A character is looked up in the current
/// keyboard layout, so `cmd+a` hits the key that types "a" on AZERTY too;
/// the US layout's keys only without layout data (off the main thread),
/// or for a shortcut on a character the layout doesn't type (a Latin
/// letter on a Cyrillic layout).
pub(crate) fn keycode(combo: &KeyCombo) -> Option<(u16, Modifiers)> {
    key_stroke(combo).map(|(code, extra, _)| (code, extra))
}

/// `keycode`, and whether the key is a dead key on this layout.
fn key_stroke(combo: &KeyCombo) -> Option<(u16, Modifiers, bool)> {
    let plain = Modifiers::default();
    let c = match combo.key {
        Key::Named(n) => return named_keycode(n).map(|code| (code, plain, false)),
        Key::Char(' ') => return Some((49, plain, false)),
        Key::Char(c) => c,
    };
    let shortcut = combo.modifiers.meta || combo.modifiers.ctrl;
    let us = || char_keycode(c).map(|(code, shift)| (code, Modifiers { shift, ..plain }, false));
    let Some(table) = layout::current() else {
        return us();
    };
    match layout::pick(&table, c, shortcut) {
        Some(k) => Some((
            k.code,
            Modifiers {
                shift: k.shift,
                alt: k.option,
                ..plain
            },
            k.dead,
        )),
        None if shortcut => us(),
        None => None,
    }
}

/// macOS virtual key code for a named key (the same on every layout).
fn named_keycode(n: NamedKey) -> Option<u16> {
    let code = match n {
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
    };
    Some(code)
}

/// The US layout's key for a character, and whether it needs Shift.
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
    use super::*;

    #[test]
    fn large_desktops_drop_to_one_pixel_per_point() {
        // One Retina laptop display: full resolution.
        assert!(best_resolution(1512.0, 982.0, 2.0).unwrap());
        // Three 5K displays side by side at 2x: ~88 Mpx, so 1x.
        assert!(!best_resolution(3.0 * 2560.0, 1440.0, 2.0).unwrap());
        // Exactly at the limit is still full resolution.
        assert!(best_resolution(4000.0, 2500.0, 2.0).unwrap());
        // A nonsensical scale counts as 1.
        assert!(best_resolution(1920.0, 1080.0, f64::NAN).unwrap());
    }

    #[test]
    fn huge_captures_are_refused() {
        let err = best_resolution(20_000.0, 10_000.0, 1.0).unwrap_err();
        assert!(matches!(err, Error::ActionFailed(_)), "{err}");
        assert!(err.to_string().contains("region"), "{err}");
        assert!(best_resolution(f64::INFINITY, 10.0, 1.0).is_err());
    }

    #[test]
    fn digits_and_letters_have_us_keys() {
        assert_eq!(char_keycode('a'), Some((0, false)));
        assert_eq!(char_keycode('A'), Some((0, true)));
        assert_eq!(char_keycode('1'), Some((18, false)));
        assert_eq!(char_keycode('é'), None);
    }
}
