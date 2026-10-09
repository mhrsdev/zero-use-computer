//! Windows overlay: per-pixel-alpha layered windows (`UpdateLayeredWindow`)
//! that are topmost, never activated, click-through (`WS_EX_TRANSPARENT`,
//! `HTTRANSPARENT`; the label's buttons and their list drop
//! `WS_EX_TRANSPARENT` while they take the pointer) and left out of screen
//! captures
//! (`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, Windows 10 2004+;
//! older systems fall back to hiding for the moment of a capture). Fading
//! uses the windows' constant alpha. The helper keeps the same DPI awareness
//! as the engine so both use the same coordinates. Windows destroys the
//! windows if the helper process dies. Each window gets its image before it
//! is first shown, and DWM is told not to animate it or round its corners.
//! Topmost windows share one band, and the taskbar (or any topmost window
//! the user brings forward) can come above them: the overlay puts itself
//! back on top when the foreground window changes, and every second.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{
    COLORREF, CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    ReleaseCapture, SetCapture, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE, GetCursorPos,
    GetForegroundWindow, GetSystemMetrics, GetWindowLongW, GetWindowRect, HTTRANSPARENT,
    HWND_TOPMOST, IDC_HAND, LoadCursorW, MA_NOACTIVATE, MSG, PM_REMOVE, PeekMessageW,
    RegisterClassW, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetCursor,
    SetWindowDisplayAffinity, SetWindowLongW, SetWindowPos, ShowWindow, TranslateMessage,
    ULW_ALPHA, UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE, WM_HOTKEY, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_SETCURSOR, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, w};

use super::draw;
use super::helper::{Hotkey, Layer, Pointer, Surface, SurfaceEvent};
use crate::keys::KeyCombo;
use crate::types::Rect;

const CLASS: PCWSTR = w!("ComputerUseOverlay");
/// Ids of the global keys' registrations (thread-wide, no window): the
/// stop key's, then the settings key's.
const HOTKEY_IDS: [i32; 2] = [0x5A01, 0x5A02];
/// Back on top at least this often while shown (the taskbar can come above
/// without the foreground window changing, e.g. one that hides itself).
const RAISE_EVERY: Duration = Duration::from_secs(1);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Never take the mouse: let it fall through to the window below
        // (the label's buttons, while they take the pointer, are hit).
        WM_NCHITTEST if !live(hwnd) => return LRESULT(HTTRANSPARENT as isize),
        // A click on a button never activates it: the user's window keeps
        // the keyboard.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR if live(hwnd) => {
            // SAFETY: a system cursor, set while it is over our window.
            unsafe {
                let _ = SetCursor(LoadCursorW(None, IDC_HAND).ok());
            }
            return LRESULT(1);
        }
        _ => {}
    }
    // SAFETY: forwarding a message we received to the default handler.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Whether one of our windows takes the pointer (it isn't `WS_EX_TRANSPARENT`).
fn live(hwnd: HWND) -> bool {
    // SAFETY: reading our own window's style.
    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    ex & WS_EX_TRANSPARENT.0 == 0
}

/// Let one of our windows take the pointer (`live`), or let it through.
fn make_live(hwnd: HWND, live: bool) {
    // SAFETY: changing our own window's style, then letting the system
    // take the change.
    unsafe {
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let new = if live {
            ex & !WS_EX_TRANSPARENT.0
        } else {
            ex | WS_EX_TRANSPARENT.0
        };
        if new != ex {
            SetWindowLongW(hwnd, GWL_EXSTYLE, new as i32);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

struct Win {
    hwnd: HWND,
    visible: bool,
}

pub struct WinSurface {
    instance: HINSTANCE,
    layers: HashMap<Layer, Win>,
    excluded: bool,
    hidden: bool,
    /// Current fade level (the windows' constant alpha).
    opacity: f32,
    /// Which global keys are registered.
    hotkeys: [bool; 2],
    /// The foreground window when the overlay last went back on top, and when.
    foreground: isize,
    raised: Instant,
    /// Layers that take the pointer (the label's buttons), and whether the
    /// pointer is on one (its leaving is noticed by where it is).
    live: HashSet<Layer>,
    pointing: bool,
}

impl WinSurface {
    pub fn open() -> Result<Self, String> {
        // The same physical-pixel coordinates as the engine.
        crate::windows::make_dpi_aware();
        // SAFETY: plain Win32 calls with valid arguments.
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
            .map_err(|e| e.to_string())?
            .into();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        // SAFETY: `class` is fully initialised; a repeat registration fails
        // harmlessly.
        unsafe { RegisterClassW(&class) };
        let mut s = Self {
            instance,
            layers: HashMap::new(),
            excluded: capture_exclusion_supported(),
            hidden: false,
            opacity: 1.0,
            hotkeys: [false; 2],
            foreground: 0,
            raised: Instant::now(),
            live: HashSet::new(),
            pointing: false,
        };
        // Find out now whether captures can leave us out, before telling the
        // engine: a never-shown test window.
        if s.excluded {
            match s.create_window() {
                Some(test) => {
                    // SAFETY: destroying the window just created.
                    unsafe {
                        let _ = DestroyWindow(test);
                    }
                }
                None => s.excluded = false,
            }
        }
        Ok(s)
    }

    /// A click-through layered popup.
    fn create_window(&mut self) -> Option<HWND> {
        let ex =
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
        // SAFETY: creating a top-level popup of our registered class.
        let hwnd = unsafe {
            CreateWindowExW(
                ex,
                CLASS,
                w!("computer-use overlay"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(self.instance),
                None,
            )
        }
        .ok()?;
        // No DWM transition when it is shown or hidden (it appears and goes
        // exactly when told), and no rounded corners on Windows 11 (the
        // glow's corners stay as drawn). Older systems refuse harmlessly.
        let off = BOOL::from(true);
        let square = DWMWCP_DONOTROUND;
        // SAFETY: `hwnd` is our window; DWM reads one value of each size.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED,
                (&off as *const BOOL).cast(),
                std::mem::size_of::<BOOL>() as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&square as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
        }
        // Keep it out of screenshots; remember if the system can't.
        // SAFETY: `hwnd` is our window.
        if self.excluded
            && unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.is_err()
        {
            self.excluded = false;
        }
        Some(hwnd)
    }

    fn blend(alpha: f32) -> BLENDFUNCTION {
        BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        }
    }

    fn update(&self, hwnd: HWND, img: &Pixmap, x: f64, y: f64, alpha: f32) {
        let (w, h) = (img.width() as i32, img.height() as i32);
        let bgra = draw::to_bgra_premultiplied(img);
        // SAFETY: a standard layered-window update: a top-down 32-bit DIB
        // selected into a memory DC, released afterwards.
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            if let Ok(bmp) = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                if !bits.is_null() {
                    std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len());
                }
                let old = SelectObject(mem, bmp.into());
                let blend = Self::blend(alpha);
                let dst = POINT {
                    x: x.round() as i32,
                    y: y.round() as i32,
                };
                let size = SIZE { cx: w, cy: h };
                let src = POINT { x: 0, y: 0 };
                let _ = UpdateLayeredWindow(
                    hwnd,
                    Some(screen),
                    Some(&dst),
                    Some(&size),
                    Some(mem),
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
                SelectObject(mem, old);
                let _ = DeleteObject(bmp.into());
            }
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }

    /// Back on top of the topmost band, when the foreground window changed
    /// (the user clicked the taskbar or another topmost window, which then
    /// comes above the overlay) or a while has passed.
    fn keep_on_top(&mut self) {
        if self.hidden {
            return;
        }
        // SAFETY: a plain query.
        let fg = unsafe { GetForegroundWindow() }.0 as isize;
        if fg == self.foreground && self.raised.elapsed() < RAISE_EVERY {
            return;
        }
        self.foreground = fg;
        self.raised = Instant::now();
        // In paint order (each raise goes on top): edges, then labels,
        // then cursors last, so a cursor is never under a label or a glow.
        let mut shown: Vec<(&Layer, HWND)> = self
            .layers
            .iter()
            .filter(|(_, w)| w.visible)
            .map(|(l, w)| (l, w.hwnd))
            .collect();
        shown.sort_by_key(|(l, _)| (l.part as u8, l.agent));
        for (_, hwnd) in shown {
            Self::raise(hwnd);
        }
    }

    /// Whether the pointer at (x, y) is on a window that takes it.
    fn on_live(&self, x: i32, y: i32) -> bool {
        self.live
            .iter()
            .filter_map(|l| self.layers.get(l).filter(|w| w.visible))
            .any(|w| {
                let mut r = RECT::default();
                // SAFETY: reading our own window's place.
                unsafe { GetWindowRect(w.hwnd, &mut r) }.is_ok()
                    && x >= r.left
                    && x < r.right
                    && y >= r.top
                    && y < r.bottom
            })
    }

    fn raise(hwnd: HWND) {
        // SAFETY: repositioning our own window in the z-order.
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }
}

impl Surface for WinSurface {
    fn excluded_from_capture(&self) -> bool {
        self.excluded
    }

    fn screen(&self) -> Rect {
        // The whole virtual desktop (every monitor; its origin may be
        // negative), as on X11, so a target window on any monitor gets its
        // border and label.
        // SAFETY: simple metric queries.
        let (x, y, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h))
    }

    fn main_screen(&self) -> Rect {
        // The primary monitor: its corner is the desktop's origin.
        // SAFETY: simple metric queries.
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h))
    }

    fn render_scale(&self) -> f32 {
        // Draw at the scale (1.5 at 150%) of the monitor the overlay is on,
        // so it isn't tiny there; before it is shown anywhere, the
        // system's.
        // The primary monitor's (the border's and the label's are drawn
        // per monitor through scale_at); before, whichever layer the map
        // gave first decided, and could change from one paint to the next.
        let main = self.main_screen();
        self.scale_at(main.width / 2.0, main.height / 2.0)
    }

    fn scale_at(&self, x: f64, y: f64) -> f32 {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
        use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
        let (mut dx, mut dy) = (0u32, 0u32);
        let p = POINT {
            x: x.round() as i32,
            y: y.round() as i32,
        };
        // SAFETY: read-only queries into locals.
        let dpi = unsafe {
            let m = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            if m.is_invalid() || GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() {
                windows::Win32::UI::HiDpi::GetDpiForSystem()
            } else {
                dx
            }
        };
        (dpi as f32 / 96.0).max(1.0)
    }

    fn px_per_unit(&self) -> f32 {
        1.0
    }

    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
        let hwnd = match self.layers.get(&layer) {
            Some(w) => w.hwnd,
            None => {
                let Some(hwnd) = self.create_window() else {
                    return;
                };
                if self.live.contains(&layer) {
                    make_live(hwnd, true);
                }
                self.layers.insert(
                    layer,
                    Win {
                        hwnd,
                        visible: false,
                    },
                );
                hwnd
            }
        };
        self.update(hwnd, img, x, y, self.opacity);
        if let Some(w) = self.layers.get_mut(&layer) {
            w.visible = true;
        }
        if !self.hidden {
            // SAFETY: showing our own window without activating it.
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            Self::raise(hwnd);
        }
    }

    fn move_to(&mut self, layer: Layer, x: f64, y: f64) {
        if let Some(w) = self.layers.get(&layer) {
            // SAFETY: moving our own window.
            let _ = unsafe {
                SetWindowPos(
                    w.hwnd,
                    Some(HWND_TOPMOST),
                    x.round() as i32,
                    y.round() as i32,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
        }
    }

    fn hide(&mut self, layer: Layer) {
        if let Some(w) = self.layers.get_mut(&layer) {
            w.visible = false;
            // SAFETY: hiding our own window.
            unsafe {
                let _ = ShowWindow(w.hwnd, SW_HIDE);
            }
        }
    }

    fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        for w in self.layers.values().filter(|w| w.visible) {
            // SAFETY: showing/hiding our own windows.
            unsafe {
                let _ = ShowWindow(w.hwnd, if hidden { SW_HIDE } else { SW_SHOWNOACTIVATE });
            }
            if !hidden {
                // SAFETY: as above.
                let _ = unsafe {
                    SetWindowPos(
                        w.hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    )
                };
            }
        }
    }

    fn set_opacity(&mut self, opacity: f32) -> bool {
        self.opacity = opacity;
        let blend = Self::blend(opacity);
        for w in self.layers.values().filter(|w| w.visible) {
            // SAFETY: only the blend changes; the window keeps its image.
            let _ = unsafe {
                UpdateLayeredWindow(
                    w.hwnd,
                    None,
                    None,
                    None,
                    None,
                    None,
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )
            };
        }
        true
    }

    fn pointer_input(&self) -> bool {
        true
    }

    fn set_live(&mut self, layer: Layer, live: bool) {
        if live {
            self.live.insert(layer);
        } else {
            self.live.remove(&layer);
        }
        if let Some(w) = self.layers.get(&layer) {
            make_live(w.hwnd, live);
        }
    }

    fn set_hotkey(&mut self, which: Hotkey, combo: Option<KeyCombo>) -> bool {
        let (i, id) = (which.index(), HOTKEY_IDS[which.index()]);
        if self.hotkeys[i] {
            // SAFETY: removing our own thread's registration.
            unsafe {
                let _ = UnregisterHotKey(None, id);
            }
            self.hotkeys[i] = false;
        }
        let Some(combo) = combo else {
            return false;
        };
        let Ok((vk, _)) = crate::windows::input::resolve(combo.key) else {
            return false;
        };
        let m = combo.modifiers;
        let mut mods = MOD_NOREPEAT;
        for (on, flag) in [
            (m.shift, MOD_SHIFT),
            (m.ctrl, MOD_CONTROL),
            (m.alt, MOD_ALT),
            (m.meta, MOD_WIN),
        ] {
            if on {
                mods |= flag;
            }
        }
        // RegisterHotKey delivers WM_HOTKEY for this one combination only;
        // it fails if another program already registered it.
        // SAFETY: a thread-wide registration (no window), removed in close().
        self.hotkeys[i] =
            unsafe { RegisterHotKey(None, id, HOT_KEY_MODIFIERS(mods.0), u32::from(vk.0)) }.is_ok();
        self.hotkeys[i]
    }

    fn pump(&mut self) -> Vec<SurfaceEvent> {
        let mut msg = MSG::default();
        let mut events = Vec::new();
        // SAFETY: the standard non-blocking message pump for this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_HOTKEY
                    && let Some(i) = HOTKEY_IDS
                        .iter()
                        .position(|id| msg.wParam.0 == *id as usize)
                {
                    events.push(SurfaceEvent::Hotkey(Hotkey::ALL[i]));
                    continue;
                }
                // The pointer on a button (at the screen point the message
                // was sent for). It is held from the press to the release,
                // which then comes here wherever the pointer is.
                let ours = !msg.hwnd.is_invalid()
                    && self.layers.values().any(|w| w.hwnd == msg.hwnd)
                    && live(msg.hwnd);
                let (x, y) = (f64::from(msg.pt.x), f64::from(msg.pt.y));
                let pointer = match msg.message {
                    WM_MOUSEMOVE if ours => Some(Pointer::Move(x, y)),
                    WM_LBUTTONDOWN if ours => {
                        let _ = SetCapture(msg.hwnd);
                        Some(Pointer::Press(x, y))
                    }
                    WM_LBUTTONUP if ours => {
                        let _ = ReleaseCapture();
                        Some(Pointer::Release(x, y))
                    }
                    _ => None,
                };
                if let Some(p) = pointer {
                    self.pointing = true;
                    events.push(SurfaceEvent::Pointer(p));
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // Off the buttons: windows hear no leaving unless asked to, so it
        // is told by where the pointer is now.
        if self.pointing {
            let mut p = POINT::default();
            // SAFETY: reading the cursor position into a local.
            let off = unsafe { GetCursorPos(&mut p) }.is_ok() && !self.on_live(p.x, p.y);
            if off {
                self.pointing = false;
                events.push(SurfaceEvent::Pointer(Pointer::Leave));
            }
        }
        self.keep_on_top();
        events
    }

    fn close(&mut self) {
        for which in Hotkey::ALL {
            self.set_hotkey(which, None);
        }
        for (_, w) in self.layers.drain() {
            // SAFETY: destroying our own windows.
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
        }
    }
}

/// Whether this Windows can leave a window out of captures: Windows 10 2004
/// (build 19041) or later. Older builds accept `WDA_EXCLUDEFROMCAPTURE` but
/// treat it as `WDA_MONITOR`, so the overlay would show in screenshots as a
/// black box. An unknown build is left to the test window.
fn capture_exclusion_supported() -> bool {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 32];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    // SAFETY: `buf` holds `bytes` bytes; a REG_SZ value is read.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return true;
    }
    let len = (bytes as usize / 2).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
        .trim_end_matches('\0')
        .trim()
        .parse::<u32>()
        .ok()
        .is_none_or(|build| build >= 19041)
}

/// Whether process `pid` is still running.
pub fn process_alive(pid: u32) -> bool {
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: querying another process's exit code with a limited handle.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == STILL_ACTIVE
    }
}
