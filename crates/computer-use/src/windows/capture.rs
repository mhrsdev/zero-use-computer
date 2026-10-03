//! Capture via GDI: `PrintWindow` for a single window (works for many
//! background windows) and `BitBlt` from the screen DC for full-screen or a
//! region.

use std::ffi::c_void;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GW_HWNDPREV, GWL_EXSTYLE, GetAncestor, GetClassNameW, GetSystemMetrics, GetWindow,
    GetWindowLongW, GetWindowRect, IsIconic, IsWindowVisible, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WINDOW_EX_STYLE, WS_EX_TRANSPARENT,
    WindowFromPoint,
};

use super::wm::{answers, hung};

use crate::error::{Error, Result};
use crate::types::{Capture, Rect};

const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
/// How long a window may take to answer before it counts as busy.
const BUSY_PROBE: Duration = Duration::from_millis(300);
/// Why the screen can't be read, as far as this process can tell.
const SCREEN_UNAVAILABLE: &str = "Windows didn't let this server read the screen: it may be locked, on the secure desktop (a UAC prompt), or in a disconnected remote session. Try again once the desktop is back.";

/// Render into an off-screen 32-bit top-down DIB, then read it out as RGBA.
/// `render` is given the memory DC and returns whether it succeeded.
fn with_dib(
    width: i32,
    height: i32,
    bounds: Rect,
    render: impl FnOnce(HDC) -> bool,
) -> Result<Capture> {
    let width = width.max(1);
    let height = height.max(1);
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(Error::Platform("GetDC failed".into()));
        }
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        ReleaseDC(None, screen_dc);
        if mem_dc.is_invalid() {
            return Err(Error::Platform("CreateCompatibleDC failed".into()));
        }

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap: HBITMAP =
            match CreateDIBSection(Some(mem_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(mem_dc);
                    return Err(Error::Platform(format!("CreateDIBSection: {e}")));
                }
            };
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem_dc);
            return Err(Error::Platform(
                "CreateDIBSection returned no pixels".into(),
            ));
        }
        let old = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

        let ok = render(mem_dc);

        let mut rgba = Vec::new();
        if ok {
            // GDI may batch drawing: finish it before reading the pixels.
            let _ = GdiFlush();
            let n = width as usize * height as usize;
            let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
            rgba.reserve(n * 4);
            for px in src.as_chunks::<4>().0 {
                // DIB is BGRA; force opaque alpha.
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }

        SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);

        if !ok {
            return Err(Error::Platform("capture failed".into()));
        }
        Ok(Capture {
            width: width as u32,
            height: height as u32,
            rgba,
            bounds,
        })
    }
}

/// Capture one window: what it draws (`PrintWindow`), cut to the frame the
/// user sees. Windows 10/11 frames have invisible resize borders a few
/// pixels wide around them, which `GetWindowRect` includes.
///
/// GPU-drawn windows (browsers, Electron, store apps) sometimes give
/// `PrintWindow` nothing but a blank image. When that happens and the
/// window is the one showing on screen there, the screen itself is captured
/// instead.
///
/// `PrintWindow` waits for the window's thread to draw, forever if the app
/// hangs, and for as long as it is busy: a window Windows reports as hung,
/// or that doesn't take a message within [`BUSY_PROBE`], is captured off
/// the screen (where Windows shows it, or a frozen copy of it).
pub fn capture_window(hwnd: HWND, app: &str) -> Result<Capture> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|e| Error::Platform(format!("GetWindowRect: {e}")))?;
    let frame = visible_frame(hwnd).unwrap_or(rect);
    if hung(hwnd) || !answers(hwnd, BUSY_PROBE) {
        // SAFETY: a read-only query.
        if unsafe { IsIconic(hwnd) }.as_bool() {
            return Err(Error::ActionFailed(format!(
                "{app} is busy or not responding and its window is minimized, so it can't be captured; wait a moment and try again"
            )));
        }
        // Off the screen, a window over it would be taken for it.
        if covered(hwnd, &frame) {
            return Err(Error::ActionFailed(format!(
                "{app} is busy or not responding and other windows cover it, so it can't be captured; wait a moment and try again"
            )));
        }
        log::info!("{app} is busy or not responding; capturing its window off the screen");
        return capture_screen(Some(frame_rect(&frame)));
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let bounds = Rect::new(
        f64::from(rect.left),
        f64::from(rect.top),
        f64::from(width.max(1)),
        f64::from(height.max(1)),
    );
    let printed = with_dib(width, height, bounds, |dc| unsafe {
        PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool()
    });
    let full = match printed {
        Ok(full) => full,
        // The window couldn't draw itself (PrintWindow failed): the screen
        // shows it, if it is on top there.
        Err(e) => {
            if on_top(hwnd, &frame) {
                return capture_screen(Some(frame_rect(&frame)));
            }
            log::debug!("PrintWindow of {app} failed: {e}");
            return Err(Error::ActionFailed(format!(
                "Windows couldn't draw {app}'s window off the screen, and it isn't in front where the screen shows it; bring it to the front and try again"
            )));
        }
    };
    // Cut off the invisible borders.
    let (dx, dy) = (
        (frame.left - rect.left).max(0),
        (frame.top - rect.top).max(0),
    );
    let (fw, fh) = (frame.right - frame.left, frame.bottom - frame.top);
    let cap = if (dx, dy, fw, fh) != (0, 0, width, height) && fw > 0 && fh > 0 {
        crate::imaging::crop(&full, (dx as u32, dy as u32, fw as u32, fh as u32))
    } else {
        full
    };
    if crate::imaging::uniform(&cap)
        && on_top(hwnd, &frame)
        && let Ok(shot) = capture_screen(Some(frame_rect(&frame)))
    {
        return Ok(shot);
    }
    Ok(cap)
}

/// A frame as a screen rectangle at least one pixel across.
fn frame_rect(frame: &RECT) -> Rect {
    Rect::new(
        f64::from(frame.left),
        f64::from(frame.top),
        (f64::from(frame.right) - f64::from(frame.left)).max(1.0),
        (f64::from(frame.bottom) - f64::from(frame.top)).max(1.0),
    )
}

/// The window's frame without its invisible resize borders.
pub(super) fn visible_frame(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: DWM writes a RECT of exactly this size.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut r as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// The window is on screen and on top at its centre, so a screen capture of
/// its frame shows it.
fn on_top(hwnd: HWND, frame: &RECT) -> bool {
    let centre = POINT {
        x: frame.left + (frame.right - frame.left) / 2,
        y: frame.top + (frame.bottom - frame.top) / 2,
    };
    // SAFETY: plain window queries.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            return false;
        }
        let at = WindowFromPoint(centre);
        !at.0.is_null() && GetAncestor(at, GA_ROOT) == hwnd
    }
}

/// Whether a window the user sees lies over part of `frame` (`hwnd`'s).
/// Walks the windows above it in z-order: no messages are sent, so this
/// works while `hwnd` hangs. Windows that show nothing of their own are
/// skipped: cloaked ones, click-through overlays (this server's included)
/// and the frozen "ghost" copy Windows shows of a hung window.
fn covered(hwnd: HWND, frame: &RECT) -> bool {
    // SAFETY: plain window queries; each handle comes from GetWindow.
    unsafe {
        let mut h = GetWindow(hwnd, GW_HWNDPREV).unwrap_or_default();
        for _ in 0..1024 {
            if h.0.is_null() {
                return false;
            }
            let mut r = RECT::default();
            let ex = WINDOW_EX_STYLE(GetWindowLongW(h, GWL_EXSTYLE) as u32);
            let mut class = [0u16; 16];
            let n = GetClassNameW(h, &mut class).max(0) as usize;
            let shows = IsWindowVisible(h).as_bool()
                && !IsIconic(h).as_bool()
                && !ex.contains(WS_EX_TRANSPARENT)
                && String::from_utf16_lossy(&class[..n.min(class.len())]) != "Ghost"
                && !super::ghost(h);
            if shows
                && GetWindowRect(h, &mut r).is_ok()
                && r.left < frame.right
                && frame.left < r.right
                && r.top < frame.bottom
                && frame.top < r.bottom
            {
                return true;
            }
            h = GetWindow(h, GW_HWNDPREV).unwrap_or_default();
        }
        // Too many to tell: say it is covered.
        true
    }
}

/// Capture the whole virtual desktop, or a screen-space rectangle of it.
pub fn capture_screen(region: Option<Rect>) -> Result<Capture> {
    let rect = region.unwrap_or_else(|| unsafe {
        Rect::new(
            f64::from(GetSystemMetrics(SM_XVIRTUALSCREEN)),
            f64::from(GetSystemMetrics(SM_YVIRTUALSCREEN)),
            f64::from(GetSystemMetrics(SM_CXVIRTUALSCREEN)),
            f64::from(GetSystemMetrics(SM_CYVIRTUALSCREEN)),
        )
    });
    let (sx, sy) = (rect.x as i32, rect.y as i32);
    let (w, h) = (rect.width as i32, rect.height as i32);
    let unavailable = |e: Error| {
        log::debug!("screen capture failed: {e}");
        Error::ActionFailed(SCREEN_UNAVAILABLE.into())
    };
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(unavailable(Error::Platform("GetDC failed".into())));
        }
        let result = with_dib(w, h, rect, |dc| {
            BitBlt(dc, 0, 0, w, h, Some(screen_dc), sx, sy, SRCCOPY).is_ok()
        });
        ReleaseDC(None, screen_dc);
        result.map_err(unavailable)
    }
}
