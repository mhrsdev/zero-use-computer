//! Windows clipboard via the Win32 clipboard API (CF_UNICODETEXT).

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};

use crate::error::{Error, Result};

const CF_UNICODETEXT: u32 = 13;
/// Extra attempts to open a clipboard another process holds.
const OPEN_RETRIES: u64 = 5;

struct ClipboardGuard;

impl ClipboardGuard {
    /// Open the clipboard, retrying briefly while another process holds
    /// it (clipboard managers and remote-desktop clients often do).
    fn open() -> Result<Self> {
        let mut attempt = 0;
        loop {
            match unsafe { OpenClipboard(Some(HWND::default())) } {
                Ok(()) => return Ok(ClipboardGuard),
                Err(_) if attempt < OPEN_RETRIES => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20 * attempt));
                }
                Err(e) => return Err(Error::Platform(format!("OpenClipboard: {e}"))),
            }
        }
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

pub fn get() -> Result<String> {
    let _guard = ClipboardGuard::open()?;
    unsafe {
        let handle = GetClipboardData(CF_UNICODETEXT)
            .map_err(|_| Error::Platform("clipboard has no text".into()))?;
        if handle.0.is_null() {
            return Ok(String::new());
        }
        let hglobal = HGLOBAL(handle.0);
        let ptr = GlobalLock(hglobal) as *const u16;
        if ptr.is_null() {
            return Err(Error::Platform("GlobalLock failed".into()));
        }
        // Read the null-terminated UTF-16 string, never past the block.
        let max = GlobalSize(hglobal) / std::mem::size_of::<u16>();
        let mut len = 0usize;
        while len < max && *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let text = String::from_utf16_lossy(slice);
        let _ = GlobalUnlock(hglobal);
        Ok(text)
    }
}

pub fn set(text: &str) -> Result<()> {
    let _guard = ClipboardGuard::open()?;
    unsafe {
        EmptyClipboard().map_err(|e| Error::Platform(format!("EmptyClipboard: {e}")))?;
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = utf16.len() * std::mem::size_of::<u16>();
        let hglobal = GlobalAlloc(GMEM_MOVEABLE, bytes)
            .map_err(|e| Error::Platform(format!("GlobalAlloc: {e}")))?;
        let ptr = GlobalLock(hglobal) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hglobal));
            return Err(Error::Platform("GlobalLock failed".into()));
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
        let _ = GlobalUnlock(hglobal);
        // Ownership of hglobal transfers to the clipboard on success only.
        if let Err(e) = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hglobal.0))) {
            let _ = GlobalFree(Some(hglobal));
            return Err(Error::Platform(format!("SetClipboardData: {e}")));
        }
        Ok(())
    }
}
