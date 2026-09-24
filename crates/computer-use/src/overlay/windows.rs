//! Windows overlay: per-pixel-alpha layered windows (`UpdateLayeredWindow`)
//! that are topmost, never activated, click-through (`WS_EX_TRANSPARENT`,
//! `HTTRANSPARENT`) and left out of screen captures
//! (`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, Windows 10 2004+;
//! older systems fall back to hiding for the moment of a capture). The
//! helper keeps the same DPI awareness as the engine so both use the same
//! coordinates. Windows destroys the windows if the helper process dies.

use std::collections::HashMap;
use std::sync::mpsc;

use tiny_skia::Pixmap;
use windows::Win32::Foundation::{
    COLORREF, CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
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
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetSystemMetrics,
    HTTRANSPARENT, HWND_TOPMOST, IDYES, MB_ICONWARNING, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
    MSG, MessageBoxW, PM_REMOVE, PeekMessageW, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA,
    UpdateLayeredWindow, WDA_EXCLUDEFROMCAPTURE, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::draw;
use super::helper::{Ask, Layer, Surface};
use crate::types::Rect;

const CLASS: PCWSTR = w!("ComputerUseOverlay");

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        // Never take the mouse: let it fall through to the window below.
        return LRESULT(HTTRANSPARENT as isize);
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
    answers_tx: mpsc::Sender<(u64, bool)>,
    answers_rx: mpsc::Receiver<(u64, bool)>,
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
        let (answers_tx, answers_rx) = mpsc::channel();
        Ok(Self {
            instance,
            layers: HashMap::new(),
            excluded: true,
            hidden: false,
            answers_tx,
            answers_rx,
        })
    }

    fn create(&mut self) -> Option<HWND> {
        // SAFETY: creating a top-level popup of our registered class.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
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

    fn update(&self, hwnd: HWND, img: &Pixmap, x: f64, y: f64) {
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
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
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
                let Some(hwnd) = self.create() else { return };
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
        self.update(hwnd, img, x, y);
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

    fn confirm(&mut self, id: u64, ask: &Ask) {
        // The system dialog runs its own message loop on a worker thread so
        // the overlay keeps animating.
        let tx = self.answers_tx.clone();
        let (title, text) = (
            HSTRING::from(ask.title.as_str()),
            HSTRING::from(ask.message.as_str()),
        );
        std::thread::spawn(move || {
            // SAFETY: a modal system message box with valid strings.
            let r = unsafe {
                MessageBoxW(
                    None,
                    &text,
                    &title,
                    MB_YESNO | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
                )
            };
            let _ = tx.send((id, r == IDYES));
        });
    }

    fn pump(&mut self) -> Vec<(u64, bool)> {
        let mut msg = MSG::default();
        // SAFETY: the standard non-blocking message pump for this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        self.answers_rx.try_iter().collect()
    }

    fn close(&mut self) {
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
