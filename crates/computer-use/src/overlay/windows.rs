//! Windows overlay: per-pixel-alpha layered windows (`UpdateLayeredWindow`)
//! that are topmost, never activated, click-through (`WS_EX_TRANSPARENT`,
//! `HTTRANSPARENT`) and left out of screen captures
//! (`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, Windows 10 2004+;
//! older systems fall back to hiding for the moment of a capture). Fading
//! uses the windows' constant alpha. Confirmations are our own panel (a
//! layered window that does take clicks), so every text on it is the
//! configured one. The helper keeps the same DPI awareness as the engine so
//! both use the same coordinates. Windows destroys the windows if the helper
//! process dies.

use std::cell::RefCell;
use std::collections::HashMap;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{
    COLORREF, CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ReleaseDC,
    SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetSystemMetrics,
    HTTRANSPARENT, HWND_TOPMOST, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SM_CXSCREEN,
    SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, TranslateMessage,
    ULW_ALPHA, UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE, WM_HOTKEY, WM_LBUTTONUP, WM_NCHITTEST,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::draw;
use super::helper::{Ask, Layer, Surface, SurfaceEvent};
use super::text::Fonts;
use crate::keys::KeyCombo;
use crate::types::Rect;

const CLASS: PCWSTR = w!("ComputerUseOverlay");
/// Id of the stop key registration (thread-wide, no window).
const HOTKEY_ID: i32 = 0x5A01;

type Button = (f32, f32, f32, f32);

/// An on-screen confirmation: its window, request id and button rectangles.
struct PanelInfo {
    hwnd: isize,
    id: u64,
    allow: Button,
    deny: Button,
}

thread_local! {
    // The window procedure has no `self`; the (single-threaded) overlay
    // keeps its confirmation panels and their answers here.
    static PANELS: RefCell<Vec<PanelInfo>> = const { RefCell::new(Vec::new()) };
    static ANSWERS: RefCell<Vec<(u64, bool)>> = const { RefCell::new(Vec::new()) };
}

fn is_panel(hwnd: HWND) -> bool {
    PANELS.with(|p| p.borrow().iter().any(|x| x.hwnd == hwnd.0 as isize))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST && !is_panel(hwnd) {
        // Never take the mouse: let it fall through to the window below.
        return LRESULT(HTTRANSPARENT as isize);
    }
    if msg == WM_LBUTTONUP && is_panel(hwnd) {
        let x = f32::from((lparam.0 & 0xffff) as u16 as i16);
        let y = f32::from(((lparam.0 >> 16) & 0xffff) as u16 as i16);
        let hit = |r: Button| x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3;
        let answered = PANELS.with(|p| {
            let mut p = p.borrow_mut();
            let i = p.iter().position(|x| x.hwnd == hwnd.0 as isize)?;
            let ok = if hit(p[i].allow) {
                true
            } else if hit(p[i].deny) {
                false
            } else {
                return None;
            };
            Some((p.remove(i).id, ok))
        });
        if let Some(a) = answered {
            ANSWERS.with(|v| v.borrow_mut().push(a));
            // SAFETY: closing the panel we created, from its own thread.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        return LRESULT(0);
    }
    // SAFETY: forwarding a message we received to the default handler.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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
    fonts: Fonts,
    hotkey: bool,
}

impl WinSurface {
    pub fn open() -> Result<Self, String> {
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
            excluded: true,
            hidden: false,
            opacity: 1.0,
            fonts: Fonts::default(),
            hotkey: false,
        };
        // Find out now whether captures can leave us out (Windows 10 2004+),
        // before telling the engine: a never-shown test window.
        match s.create_window(true) {
            Some(test) => {
                // SAFETY: destroying the window just created.
                unsafe {
                    let _ = DestroyWindow(test);
                }
            }
            None => s.excluded = false,
        }
        Ok(s)
    }

    /// A layered popup; `click_through` for everything but confirmation panels.
    fn create_window(&mut self, click_through: bool) -> Option<HWND> {
        let mut ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if click_through {
            ex |= WS_EX_TRANSPARENT;
        }
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
        // Keep it out of screenshots; remember if the system can't.
        // SAFETY: `hwnd` is our window.
        if unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.is_err() {
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

    fn show_panels(&self, show: bool) {
        for p in PANELS.with(|p| p.borrow().iter().map(|x| x.hwnd).collect::<Vec<_>>()) {
            let hwnd = HWND(p as *mut _);
            // SAFETY: showing/hiding our own panel windows.
            unsafe {
                let _ = ShowWindow(hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
            if show {
                Self::raise(hwnd);
            }
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
        // SAFETY: simple metric queries.
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h))
    }

    fn screen_at(&self, x: f64, y: f64) -> Rect {
        let pt = POINT {
            x: x.round() as i32,
            y: y.round() as i32,
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: plain monitor queries into a correctly sized struct.
        let ok = unsafe {
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            GetMonitorInfoW(mon, &mut info).as_bool()
        };
        if !ok {
            return self.screen();
        }
        let r = info.rcMonitor;
        Rect::new(
            f64::from(r.left),
            f64::from(r.top),
            f64::from(r.right - r.left),
            f64::from(r.bottom - r.top),
        )
    }

    fn render_scale(&self) -> f32 {
        1.0
    }

    fn px_per_unit(&self) -> f32 {
        1.0
    }

    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
        let hwnd = match self.layers.get(&layer) {
            Some(w) => w.hwnd,
            None => {
                let Some(hwnd) = self.create_window(true) else {
                    return;
                };
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
        // Panels too, where captures can't leave them out.
        if !self.excluded {
            self.show_panels(!hidden);
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

    fn confirm(&mut self, id: u64, ask: &Ask) {
        // Our own panel (not a system dialog), so its texts and buttons are
        // exactly the configured ones.
        if self.fonts.is_empty() {
            self.fonts = Fonts::load("");
        }
        let accent = draw::parse_color("#FFE600").unwrap_or(tiny_skia::Color::WHITE);
        let (img, [allow, deny]) = draw::panel(
            &self.fonts,
            &ask.title,
            &ask.message,
            &ask.allow,
            &ask.deny,
            accent,
            self.render_scale(),
        );
        let Some(hwnd) = self.create_window(false) else {
            return;
        };
        let screen = self.screen();
        let x = screen.x + (screen.width - f64::from(img.width())) / 2.0;
        let y = screen.y + (screen.height - f64::from(img.height())) / 3.0;
        self.update(hwnd, &img, x, y, 1.0);
        PANELS.with(|p| {
            p.borrow_mut().push(PanelInfo {
                hwnd: hwnd.0 as isize,
                id,
                allow,
                deny,
            })
        });
        if !self.hidden || self.excluded {
            // SAFETY: showing our own window without activating it.
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            Self::raise(hwnd);
        }
    }

    fn dismiss(&mut self) {
        for p in PANELS.with(|p| std::mem::take(&mut *p.borrow_mut())) {
            // SAFETY: destroying our own panels.
            unsafe {
                let _ = DestroyWindow(HWND(p.hwnd as *mut _));
            }
        }
    }

    fn set_hotkey(&mut self, combo: Option<KeyCombo>) -> bool {
        if self.hotkey {
            // SAFETY: removing our own thread's registration.
            unsafe {
                let _ = UnregisterHotKey(None, HOTKEY_ID);
            }
            self.hotkey = false;
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
        self.hotkey =
            unsafe { RegisterHotKey(None, HOTKEY_ID, HOT_KEY_MODIFIERS(mods.0), u32::from(vk.0)) }
                .is_ok();
        self.hotkey
    }

    fn pump(&mut self) -> Vec<SurfaceEvent> {
        let mut msg = MSG::default();
        let mut events = Vec::new();
        // SAFETY: the standard non-blocking message pump for this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_HOTKEY && msg.wParam.0 == HOTKEY_ID as usize {
                    events.push(SurfaceEvent::Hotkey);
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        events.extend(
            ANSWERS
                .with(|a| std::mem::take(&mut *a.borrow_mut()))
                .into_iter()
                .map(|(id, ok)| SurfaceEvent::Answer(id, ok)),
        );
        events
    }

    fn close(&mut self) {
        self.set_hotkey(None);
        self.dismiss();
        for (_, w) in self.layers.drain() {
            // SAFETY: destroying our own windows.
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
        }
    }
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
