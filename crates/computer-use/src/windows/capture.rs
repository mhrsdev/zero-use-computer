//! Window capture via `PrintWindow` into a DIB. `PW_RENDERFULLCONTENT`
//! captures many background windows without bringing them to the front.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GetDC, HBITMAP, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::error::{Error, Result};
use crate::types::{Capture, Rect};

const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;

pub fn capture_window(hwnd: HWND) -> Result<Capture> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|e| Error::Platform(format!("GetWindowRect: {e}")))?;
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);

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

        let mut info = BITMAPINFO {
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
            return Err(Error::Platform("CreateDIBSection returned no pixels".into()));
        }
        let old = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

        let ok = PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();

        let mut rgba = Vec::new();
        if ok {
            let n = (width * height) as usize;
            let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
            rgba.reserve(n * 4);
            for px in src.chunks_exact(4) {
                // DIB is BGRA; the alpha byte is unreliable, so force opaque.
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }

        SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);

        if !ok {
            return Err(Error::Platform("PrintWindow failed".into()));
        }
        // Refresh the info header (unused, but keeps the binding live).
        let _ = &mut info;
        Ok(Capture {
            width: width as u32,
            height: height as u32,
            rgba,
            bounds: Rect::new(
                f64::from(rect.left),
                f64::from(rect.top),
                f64::from(width),
                f64::from(height),
            ),
        })
    }
}
