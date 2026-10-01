//! Window management on Windows: `SetWindowPos` / `ShowWindowAsync` for
//! placement and state, `WM_CLOSE` to ask a window to close (as its close
//! button does), and the monitor list for displays.
//!
//! A synchronous `ShowWindow` or `SetWindowPos` on another program's window
//! waits for that program's UI thread, forever if it hangs: requests are
//! posted instead (`ShowWindowAsync`, `SWP_ASYNCWINDOWPOS`), a window
//! Windows reports as hung is refused up front, and the result is waited
//! for, briefly.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOTOWNER, GetAncestor, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId,
    HWND_TOP, IsHungAppWindow, IsIconic, IsZoomed, MONITORINFOF_PRIMARY, PostMessageW,
    SET_WINDOW_POS_FLAGS, SHOW_WINDOW_CMD, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetWindowPos, ShowWindowAsync, WM_CLOSE,
};
use windows::core::BOOL;

use crate::error::{Error, Result};
use crate::types::{AppInfo, Display, Rect, WindowOp};

/// How long to wait for a posted window change to show.
const CHANGE_WAIT: Duration = Duration::from_millis(500);
/// How long a busy app may take to restore a minimized or maximized window
/// before the window is changed further.
const RESTORE_WAIT: Duration = Duration::from_secs(3);
/// How long to wait for each attempt at bringing a window to the front.
const FRONT_WAIT: Duration = Duration::from_millis(150);

fn rect(r: RECT) -> Rect {
    Rect::new(
        f64::from(r.left),
        f64::from(r.top),
        f64::from(r.right) - f64::from(r.left),
        f64::from(r.bottom) - f64::from(r.top),
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

/// Windows considers the window hung (its thread hasn't taken messages for
/// about five seconds).
pub fn hung(hwnd: HWND) -> bool {
    // SAFETY: a read-only query that never waits on the window's thread.
    unsafe { IsHungAppWindow(hwnd) }.as_bool()
}

/// Poll `done` until it holds or `limit` passes; whether it held.
fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if done() {
            return true;
        }
        if start.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Ask the window's own thread to change its show state (never waits on it).
fn show_async(hwnd: HWND, cmd: SHOW_WINDOW_CMD, what: &str) -> Result<()> {
    // SAFETY: posts a request to a live top-level window.
    if unsafe { ShowWindowAsync(hwnd, cmd) }.as_bool() {
        Ok(())
    } else {
        Err(Error::ActionFailed(format!("could not {what} the window")))
    }
}

/// `SetWindowPos`, posted to the window's thread (never waits on it).
fn set_pos_async(
    hwnd: HWND,
    after: Option<HWND>,
    r: (i32, i32, i32, i32),
    flags: SET_WINDOW_POS_FLAGS,
) -> windows::core::Result<()> {
    // SAFETY: posts a request to a live top-level window.
    unsafe { SetWindowPos(hwnd, after, r.0, r.1, r.2, r.3, flags | SWP_ASYNCWINDOWPOS) }
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: a read-only query into a local.
    unsafe { GetWindowRect(hwnd, &mut r) }.ok().map(|()| r)
}

/// `hwnd`, or a window it owns (a dialog of it), is in front.
fn in_front(hwnd: HWND) -> bool {
    // SAFETY: read-only queries.
    unsafe {
        let fg = GetForegroundWindow();
        // A minimized window can be the foreground one, with nothing shown.
        !fg.0.is_null()
            && (fg == hwnd || GetAncestor(fg, GA_ROOTOWNER) == hwnd)
            && !IsIconic(hwnd).as_bool()
    }
}

/// Bring `hwnd` to the front despite the foreground lock, which lets only
/// the program that got the last input event change the foreground window:
/// first plainly; then after an empty input event (no movement, no button,
/// no key), which makes this process that program; then attached to the
/// input of the window in front. Whether it is in front.
fn bring_to_front(hwnd: HWND) -> bool {
    // SAFETY: plain Win32 calls; the input attachment is undone at once.
    unsafe {
        if in_front(hwnd) {
            return true;
        }
        let _ = SetForegroundWindow(hwnd);
        if wait_until(FRONT_WAIT, || in_front(hwnd)) {
            return true;
        }
        if super::input::empty_event().is_ok() {
            let _ = SetForegroundWindow(hwnd);
            if wait_until(FRONT_WAIT, || in_front(hwnd)) {
                return true;
            }
        }
        let fg = GetForegroundWindow();
        // Never attach to a hung thread: its input state is stuck.
        if !fg.0.is_null() && !hung(fg) {
            let theirs = GetWindowThreadProcessId(fg, None);
            let ours = GetCurrentThreadId();
            if theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool() {
                let _ = SetForegroundWindow(hwnd);
                let _ = AttachThreadInput(ours, theirs, false);
            }
        }
        wait_until(FRONT_WAIT, || in_front(hwnd))
    }
}

pub fn apply(hwnd: HWND, app: &AppInfo, op: &WindowOp) -> Result<()> {
    // Only windows of the app the call is about.
    let mut owner = 0u32;
    // SAFETY: reads the owning process of a window handle.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
    if owner != app.pid {
        return Err(Error::ActionFailed("that window is gone".into()));
    }
    // Whatever is asked of a hung window would wait for it (or, posted,
    // happen at some later time): say so instead.
    let responding = || {
        if hung(hwnd) {
            Err(Error::ActionFailed(format!(
                "{} is not responding (Windows reports its window as hung), so its window can't be changed now. Wait a moment and try again, or ask the user.",
                app.name
            )))
        } else {
            Ok(())
        }
    };
    // SAFETY (below): read-only state queries on a live top-level window.
    let iconic = || unsafe { IsIconic(hwnd) }.as_bool();
    let zoomed = || unsafe { IsZoomed(hwnd) }.as_bool();
    match *op {
        WindowOp::Focus => {
            responding()?;
            if iconic() {
                show_async(hwnd, SW_RESTORE, "restore")?;
                // Still minimized, it would "come to the front" with nothing
                // of it on screen: input would land on what is there instead.
                if !wait_until(RESTORE_WAIT, || !iconic()) {
                    return Err(Error::ActionFailed(format!(
                        "{} hasn't restored its window from the taskbar yet (it may be busy); try again in a moment",
                        app.name
                    )));
                }
            }
            // Raise it, without waiting on its thread.
            let _ = set_pos_async(hwnd, Some(HWND_TOP), (0, 0, 0, 0), SWP_NOMOVE | SWP_NOSIZE);
            if !bring_to_front(hwnd) {
                return Err(Error::ActionFailed(format!(
                    "Windows didn't let {} come to the front (its taskbar button may flash); click inside the window instead",
                    app.name
                )));
            }
        }
        WindowOp::SetBounds(r) => {
            responding()?;
            if zoomed() || iconic() {
                show_async(hwnd, SW_RESTORE, "restore")?;
                // Moved before the restore lands, the restore would undo it.
                if !wait_until(RESTORE_WAIT, || !zoomed() && !iconic()) {
                    return Err(Error::ActionFailed(format!(
                        "{} hasn't restored its window yet (it may be busy); try again in a moment",
                        app.name
                    )));
                }
            }
            let want = (
                r.x.round() as i32,
                r.y.round() as i32,
                r.width.round().max(1.0) as i32,
                r.height.round().max(1.0) as i32,
            );
            let target = RECT {
                left: want.0,
                top: want.1,
                right: want.0.saturating_add(want.2),
                bottom: want.1.saturating_add(want.3),
            };
            let before = window_rect(hwnd);
            set_pos_async(hwnd, None, want, SWP_NOZORDER | SWP_NOACTIVATE)
                .map_err(|e| Error::ActionFailed(format!("could not move the window: {e}")))?;
            // Until it moves (the app may settle on a size of its own).
            wait_until(CHANGE_WAIT, || {
                let now = window_rect(hwnd);
                now != before || now == Some(target)
            });
        }
        WindowOp::Maximize => {
            responding()?;
            show_async(hwnd, SW_MAXIMIZE, "maximize")?;
            wait_until(CHANGE_WAIT, zoomed);
        }
        WindowOp::Minimize => {
            responding()?;
            show_async(hwnd, SW_MINIMIZE, "minimize")?;
            wait_until(CHANGE_WAIT, iconic);
        }
        WindowOp::Restore => {
            responding()?;
            let was_iconic = iconic();
            show_async(hwnd, SW_RESTORE, "restore")?;
            wait_until(
                CHANGE_WAIT,
                || {
                    if was_iconic { !iconic() } else { !zoomed() }
                },
            );
        }
        WindowOp::Fullscreen(_) => {
            return Err(Error::Unsupported(
                "Windows has no general full-screen switch; many apps use F11 (press_key)".into(),
            ));
        }
        WindowOp::Close => {
            responding()?;
            // SAFETY: posting (never sending) a message to the window.
            unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
                .map_err(|e| Error::ActionFailed(format!("could not ask it to close: {e}")))?;
        }
        WindowOp::ToDesktop(_) => {
            return Err(Error::Unsupported(
                "Windows only lets a program move its own windows between virtual desktops".into(),
            ));
        }
    }
    Ok(())
}

pub fn minimized(hwnd: HWND) -> bool {
    // SAFETY: a read-only query.
    unsafe { IsIconic(hwnd) }.as_bool()
}
