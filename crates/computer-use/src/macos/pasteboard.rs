//! macOS clipboard via `NSPasteboard`.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use crate::error::Result;

pub fn get() -> Result<String> {
    let pb = NSPasteboard::generalPasteboard();
    let s = unsafe { pb.stringForType(NSPasteboardTypeString) };
    Ok(s.map(|s| s.to_string()).unwrap_or_default())
}

pub fn set(text: &str) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    unsafe {
        pb.clearContents();
        let ns = NSString::from_str(text);
        pb.setString_forType(&ns, NSPasteboardTypeString);
    }
    Ok(())
}
