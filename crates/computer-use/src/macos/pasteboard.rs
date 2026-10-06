//! macOS clipboard via `NSPasteboard`. The callers hold an autorelease pool.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use super::ffi::nsstring_text;
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
        && (0..types.count()).any(|i| CONCEALED.contains(&&*nsstring_text(&types.objectAtIndex(i))))
    {
        return Err(Error::Blocked(
            "reading the clipboard".into(),
            "it holds concealed content (a password or another secret, as marked by the app that copied it)".into(),
        ));
    }
    let Some(s) = (unsafe { pb.stringForType(NSPasteboardTypeString) }) else {
        // Something other than text (an image, files…), or nothing.
        let types: Vec<String> = pb
            .types()
            .map(|t| {
                (0..t.count())
                    .map(|i| nsstring_text(&t.objectAtIndex(i)))
                    .collect()
            })
            .unwrap_or_default();
        return match no_text(&types) {
            Some(why) => Err(Error::ActionFailed(why)),
            None => Ok(String::new()),
        };
    };
    // Read as UTF-16: what another app copied may hold a lone surrogate.
    let mut text = nsstring_text(&s);
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

/// Most pasteboard types named in a message.
const TYPES_SHOWN: usize = 6;

/// The message for a clipboard holding `types` but no text; `None` when it
/// holds nothing (it is empty).
fn no_text(types: &[String]) -> Option<String> {
    if types.is_empty() {
        return None;
    }
    let mut shown = types[..types.len().min(TYPES_SHOWN)].join(", ");
    if types.len() > TYPES_SHOWN {
        shown.push_str(&format!(" and {} more", types.len() - TYPES_SHOWN));
    }
    Some(format!("the clipboard holds no text (it holds: {shown})"))
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

#[cfg(test)]
mod tests {
    use super::no_text;

    #[test]
    fn a_clipboard_without_text_names_what_it_holds() {
        assert_eq!(no_text(&[]), None);
        let types = vec!["public.png".to_string(), "public.tiff".to_string()];
        assert_eq!(
            no_text(&types).as_deref(),
            Some("the clipboard holds no text (it holds: public.png, public.tiff)")
        );
        let many: Vec<String> = (0..9).map(|i| format!("t{i}")).collect();
        let msg = no_text(&many).unwrap();
        assert!(msg.ends_with("t5 and 3 more)"), "{msg}");
    }
}
