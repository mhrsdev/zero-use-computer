//! Window management on Windows: `SetWindowPos` / `ShowWindow` for placement
//! and state, `WM_CLOSE` to ask a window to close (as its close button
//! does), and the monitor list for displays.

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetWindowThreadProcessId, IsIconic, IsZoomed, MONITORINFOF_PRIMARY,
    PostMessageW, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER,
    SetForegroundWindow, SetWindowPos, ShowWindow, WM_CLOSE,
};
use windows::core::BOOL;

use crate::error::{Error, Result};
use crate::types::{Display, Rect, WindowOp};

fn rect(r: RECT) -> Rect {
    Rect::new(
        f64::from(r.left),
        f64::from(r.top),
        f64::from(r.right - r.left),
        f64::from(r.bottom - r.top),
    )
}

unsafe extern "system" fn monitor(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the Vec passed to EnumDisplayMonitors below.
    let out = unsafe { &mut *(data.0 as *mut Vec<Display>) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is initialised with its size.
    if unsafe { GetMonitorInfoW(m, &mut info) }.as_bool() {
        out.push(Display {
            index: out.len() as u32,
            bounds: rect(info.rcMonitor),
            work_area: rect(info.rcWork),
            primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    true.into()
}

pub fn displays() -> Result<Vec<Display>> {
    let mut out: Vec<Display> = Vec::new();
    // SAFETY: the callback only writes into `out`, which outlives the call.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(monitor),
            LPARAM(&mut out as *mut Vec<Display> as isize),
        )
    };
    if !ok.as_bool() || out.is_empty() {
        return Err(Error::Platform("could not list the displays".into()));
    }
    Ok(out)
}

pub fn apply(hwnd: HWND, pid: u32, op: &WindowOp) -> Result<()> {
    // Only windows of the app that was approved.
    let mut owner = 0u32;
    // SAFETY: reads the owning process of a window handle.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
    if owner != pid {
        return Err(Error::ActionFailed("that window is gone".into()));
    }
    // SAFETY: plain Win32 calls on a live top-level window of the app.
    unsafe {
        match *op {
            WindowOp::Focus => {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = BringWindowToTop(hwnd);
                if !SetForegroundWindow(hwnd).as_bool() {
                    return Err(Error::ActionFailed(
                        "Windows didn't let it come to the front (its taskbar button may flash); click inside the window instead".into(),
                    ));
                }
            }
            WindowOp::SetBounds(r) => {
                if IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                SetWindowPos(
                    hwnd,
                    None,
                    r.x.round() as i32,
                    r.y.round() as i32,
                    r.width.round().max(1.0) as i32,
                    r.height.round().max(1.0) as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .map_err(|e| Error::ActionFailed(format!("could not move the window: {e}")))?;
            }
            WindowOp::Maximize => {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
            WindowOp::Minimize => {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
            }
            WindowOp::Restore => {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            WindowOp::Fullscreen(_) => {
                return Err(Error::Unsupported(
                    "Windows has no general full-screen switch; many apps use F11 (press_key)"
                        .into(),
                ));
            }
            WindowOp::Close => {
                PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))
                    .map_err(|e| Error::ActionFailed(format!("could not ask it to close: {e}")))?;
            }
            WindowOp::ToDesktop(_) => {
                return Err(Error::Unsupported(
                    "Windows only lets a program move its own windows between virtual desktops"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

pub fn minimized(hwnd: HWND) -> bool {
    // SAFETY: a read-only query.
    unsafe { IsIconic(hwnd) }.as_bool()
}
