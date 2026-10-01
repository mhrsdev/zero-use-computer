//! Capture via GDI: `PrintWindow` for a single window (works for many
//! background windows) and `BitBlt` from the screen DC for full-screen or a
//! region.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetSystemMetrics, GetWindowRect, IsIconic, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WindowFromPoint,
};

use crate::error::{Error, Result};
use crate::types::{Capture, Rect};

const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;

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
            CreateDIBSection(Some(mem_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
                .map_err(|e| Error::Platform(format!("CreateDIBSection: {e}")))?;
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
            let n = (width * height) as usize;
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
pub fn capture_window(hwnd: HWND) -> Result<Capture> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|e| Error::Platform(format!("GetWindowRect: {e}")))?;
    let frame = visible_frame(hwnd).unwrap_or(rect);
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let bounds = Rect::new(
        f64::from(rect.left),
        f64::from(rect.top),
        f64::from(width.max(1)),
        f64::from(height.max(1)),
    );
    let full = with_dib(width, height, bounds, |dc| unsafe {
        PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool()
    })?;
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
    if crate::imaging::uniform(&cap) && on_top(hwnd, &frame) {
        let r = Rect::new(
            f64::from(frame.left),
            f64::from(frame.top),
            f64::from(fw.max(1)),
            f64::from(fh.max(1)),
        );
        if let Ok(shot) = capture_screen(Some(r)) {
            return Ok(shot);
        }
    }
    Ok(cap)
}

/// The window's frame without its invisible resize borders.
fn visible_frame(hwnd: HWND) -> Option<RECT> {
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
    unsafe {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(Error::Platform("GetDC failed".into()));
        }
        let result = with_dib(w, h, rect, |dc| {
            BitBlt(dc, 0, 0, w, h, Some(screen_dc), sx, sy, SRCCOPY).is_ok()
        });
        ReleaseDC(None, screen_dc);
        result
    }
}
