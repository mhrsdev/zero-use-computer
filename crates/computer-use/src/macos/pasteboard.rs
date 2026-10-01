//! macOS clipboard via `NSPasteboard`. The callers hold an autorelease pool.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use crate::error::{Error, Result};

/// The marker type (nspasteboard.org) password managers put on what they
/// copy: a secret. (TransientType only asks clipboard histories not to keep
/// it, as text expanders do with everything: that is ordinary text.)
const CONCEALED: [&str; 1] = ["org.nspasteboard.ConcealedType"];

/// Most clipboard text returned (bytes of UTF-8).
const MAX_BYTES: usize = 1 << 20;

pub fn get() -> Result<String> {
    let pb = NSPasteboard::generalPasteboard();
    if let Some(types) = pb.types()
        && (0..types.count()).any(|i| CONCEALED.contains(&&*types.objectAtIndex(i).to_string()))
    {
        return Err(Error::Blocked(
            "reading the clipboard".into(),
            "it holds concealed content (a password or another secret, as marked by the app that copied it)".into(),
        ));
    }
    let s = unsafe { pb.stringForType(NSPasteboardTypeString) };
    let mut text = s.map(|s| s.to_string()).unwrap_or_default();
    if text.len() > MAX_BYTES {
        let total = text.chars().count();
        let mut cut = MAX_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        let kept = text.chars().count();
        text.push_str(&format!(
            "\n[… cut here: the clipboard holds {total} characters, {kept} shown]"
        ));
    }
    Ok(text)
}

pub fn set(text: &str) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    let written = unsafe {
        pb.clearContents();
        let ns = NSString::from_str(text);
        pb.setString_forType(&ns, NSPasteboardTypeString)
    };
    if written {
        Ok(())
    } else {
        Err(Error::Platform(
            "could not write the text to the pasteboard".into(),
        ))
    }
}
