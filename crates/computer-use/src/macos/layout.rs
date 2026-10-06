//! The current keyboard layout (Text Input Sources and `UCKeyTranslate`):
//! which key types a character, so `cmd+a` hits the key that types "a" on
//! this layout (key 12 on AZERTY, where key 0 types "q"), and switching away
//! from an input method (Japanese, Chinese…) while text is typed.
//!
//! Text Input Sources must be used on the main thread (macOS 14 asserts
//! it); elsewhere nothing here is called and callers get `None`.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::data::CFData;
use core_foundation::string::{CFString, CFStringRef};

type TISInputSourceRef = CFTypeRef;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> TISInputSourceRef;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> TISInputSourceRef;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> TISInputSourceRef;
    fn TISGetInputSourceProperty(source: TISInputSourceRef, key: CFStringRef) -> CFTypeRef;
    fn TISSelectInputSource(source: TISInputSourceRef) -> i32;
    fn LMGetKbdType() -> u8;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    static kTISPropertyInputSourceID: CFStringRef;
    static kTISPropertyInputSourceType: CFStringRef;
    static kTISTypeKeyboardInputMode: CFStringRef;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn UCKeyTranslate(
        layout: *const c_void,
        key_code: u16,
        key_action: u16,
        modifier_state: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_len: usize,
        actual_len: *mut usize,
        chars: *mut u16,
    ) -> i32;
}

const K_UC_KEY_ACTION_DOWN: u16 = 0;
const K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK: u32 = 1;

/// The modifiers held while a key types a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Plain,
    Shift,
    Option,
    /// With ⌘ held: what shortcuts see (a layout may map keys differently
    /// then, as AZERTY's number row gives digits).
    Command,
}

impl Level {
    /// `UCKeyTranslate`'s modifier state: Carbon's modifier bits >> 8.
    fn state(self) -> u32 {
        match self {
            Level::Plain => 0,
            Level::Shift => 0x0200 >> 8,
            Level::Option => 0x0800 >> 8,
            Level::Command => 0x0100 >> 8,
        }
    }
}

/// One character a key types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub code: u16,
    pub level: Level,
    pub ch: char,
    /// A dead key: it starts an accent instead of typing `ch` at once.
    pub dead: bool,
}

/// A key to press for a character, and the modifiers it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub code: u16,
    pub shift: bool,
    pub option: bool,
    /// A dead key: pressed on its own it starts an accent, typing nothing.
    pub dead: bool,
}

/// The key for `c` in a layout's `entries`. For a `shortcut` (⌘ or ⌃
/// held) a letter is looked up in lower case (shift added for an upper
/// case one), first among what keys give with ⌘ held, so `cmd+a` hits the
/// key that types "a". Otherwise the key that types `c` itself, with the
/// fewest modifiers. Live keys beat dead ones, then lower key codes win.
pub fn pick(entries: &[Entry], c: char, shortcut: bool) -> Option<Stroke> {
    let upper = c.is_uppercase() && c.to_lowercase().count() == 1;
    let (want, extra_shift) = if shortcut && upper {
        (c.to_lowercase().next().unwrap_or(c), true)
    } else {
        (c, false)
    };
    let levels: &[Level] = if shortcut {
        &[Level::Command, Level::Plain, Level::Shift, Level::Option]
    } else {
        &[Level::Plain, Level::Shift, Level::Option]
    };
    let rank = |e: &Entry| {
        let level = levels.iter().position(|l| *l == e.level)?;
        Some((e.dead, level, e.code))
    };
    let best = entries
        .iter()
        .filter(|e| e.ch == want)
        .filter_map(|e| rank(e).map(|r| (r, e)))
        .min_by_key(|(r, _)| *r)?
        .1;
    Some(Stroke {
        code: best.code,
        shift: extra_shift || best.level == Level::Shift,
        option: best.level == Level::Option,
        dead: best.dead,
    })
}

/// Keypad keys type digits and signs too; they are never the key for one.
fn is_keypad(code: u16) -> bool {
    matches!(code, 65 | 67 | 69 | 71 | 75 | 76 | 78 | 81..=92)
}

/// Whether the main thread runs this (Text Input Sources need it).
fn on_main_thread() -> bool {
    objc2::MainThreadMarker::new().is_some()
}

/// Wrap a +1 reference from a Create/Copy call.
fn owned(raw: CFTypeRef) -> Option<CFType> {
    // SAFETY: a +1 reference (null checked), released on drop.
    (!raw.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(raw) })
}

/// A string property of an input source.
fn string_property(source: &CFType, key: CFStringRef) -> Option<String> {
    // SAFETY: a live input source; the property is +0, retained while
    // wrapped, and only read as a string when it is one.
    unsafe {
        let raw = TISGetInputSourceProperty(source.as_CFTypeRef(), key);
        if raw.is_null() {
            return None;
        }
        CFType::wrap_under_get_rule(raw)
            .downcast::<CFString>()
            .map(|s| s.to_string())
    }
}

/// The source's `uchr` key layout data, if it has one (an input method
/// doesn't).
fn layout_data(source: &CFType) -> Option<CFData> {
    // SAFETY: as in `string_property`; the value is a CFData.
    unsafe {
        let raw =
            TISGetInputSourceProperty(source.as_CFTypeRef(), kTISPropertyUnicodeKeyLayoutData);
        if raw.is_null() {
            return None;
        }
        CFType::wrap_under_get_rule(raw).downcast::<CFData>()
    }
}

/// What `code` types at `level` on this layout, and whether it is a dead key.
fn translate(layout: &CFData, code: u16, level: Level, kbd: u32) -> Option<(char, bool)> {
    let run = |options: u32| {
        let mut dead = 0u32;
        let mut len = 0usize;
        let mut buf = [0u16; 4];
        // SAFETY: `layout` holds a UCKeyboardLayout and stays alive for the
        // call; the output buffer's length is passed.
        let status = unsafe {
            UCKeyTranslate(
                layout.bytes().as_ptr() as *const c_void,
                code,
                K_UC_KEY_ACTION_DOWN,
                level.state(),
                kbd,
                options,
                &mut dead,
                buf.len(),
                &mut len,
                buf.as_mut_ptr(),
            )
        };
        (status == 0).then(|| (String::from_utf16_lossy(&buf[..len.min(buf.len())]), dead))
    };
    let (text, dead) = run(0)?;
    let (text, dead) = if text.is_empty() && dead != 0 {
        // A dead key: what it types on its own.
        (run(K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK)?.0, true)
    } else {
        (text, false)
    };
    let mut chars = text.chars();
    let ch = chars.next()?;
    let printable = !ch.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&ch);
    (chars.next().is_none() && printable).then_some((ch, dead))
}

/// Every character the layout's main keys type, at each [`Level`].
fn entries_of(layout: &CFData) -> Vec<Entry> {
    // SAFETY: a plain query of the keyboard type.
    let kbd = u32::from(unsafe { LMGetKbdType() });
    let mut out = Vec::new();
    for code in (0u16..128).filter(|c| !is_keypad(*c)) {
        for level in [Level::Plain, Level::Shift, Level::Option, Level::Command] {
            if let Some((ch, dead)) = translate(layout, code, level, kbd) {
                out.push(Entry {
                    code,
                    level,
                    ch,
                    dead,
                });
            }
        }
    }
    out
}

/// The layout table last built, by the input source it was built for.
static CACHE: Mutex<Option<(String, Arc<Vec<Entry>>)>> = Mutex::new(None);

/// The current layout's table: the keyboard layout in use, or the
/// ASCII-capable one when that has no layout data (an input method is
/// active). `None` off the main thread or without layout data.
pub fn current() -> Option<Arc<Vec<Entry>>> {
    if !on_main_thread() {
        return None;
    }
    // SAFETY: Copy calls, on the main thread.
    let source = owned(unsafe { TISCopyCurrentKeyboardLayoutInputSource() })
        .filter(|s| layout_data(s).is_some())
        .or_else(|| owned(unsafe { TISCopyCurrentASCIICapableKeyboardLayoutInputSource() }))?;
    let data = layout_data(&source)?;
    // SAFETY: reading a constant key.
    let id = string_property(&source, unsafe { kTISPropertyInputSourceID }).unwrap_or_default();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((cached, table)) = cache.as_ref()
        && !id.is_empty()
        && *cached == id
    {
        return Some(table.clone());
    }
    let table = Arc::new(entries_of(&data));
    if table.is_empty() {
        return None;
    }
    *cache = Some((id, table.clone()));
    Some(table)
}

/// While alive, the ASCII-capable keyboard layout stands in for an input
/// method that was active (so typed text isn't composed by it); the input
/// method comes back on drop.
pub struct AsciiInput {
    previous: Option<CFType>,
}

impl AsciiInput {
    /// Whether an input method was switched away from.
    pub fn switched(&self) -> bool {
        self.previous.is_some()
    }
}

/// Switch from an input method to the ASCII-capable layout, if one is
/// active (and this is the main thread).
pub fn ascii_input() -> AsciiInput {
    let none = AsciiInput { previous: None };
    if !on_main_thread() {
        return none;
    }
    // SAFETY: a Copy call, on the main thread.
    let Some(current) = owned(unsafe { TISCopyCurrentKeyboardInputSource() }) else {
        return none;
    };
    // SAFETY: reading constant keys.
    let (type_key, input_mode) =
        unsafe { (kTISPropertyInputSourceType, kTISTypeKeyboardInputMode) };
    // SAFETY: a constant CFString, retained while wrapped.
    let input_mode = unsafe { CFString::wrap_under_get_rule(input_mode) }.to_string();
    if string_property(&current, type_key).as_deref() != Some(input_mode.as_str()) {
        return none;
    }
    // SAFETY: a Copy call, on the main thread.
    let Some(ascii) = owned(unsafe { TISCopyCurrentASCIICapableKeyboardLayoutInputSource() })
    else {
        return none;
    };
    // SAFETY: selecting a live input source.
    if unsafe { TISSelectInputSource(ascii.as_CFTypeRef()) } != 0 {
        return none;
    }
    log::debug!("typing with the ASCII-capable layout in place of the input method");
    AsciiInput {
        previous: Some(current),
    }
}

impl Drop for AsciiInput {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            // SAFETY: selecting a live input source, on the thread that
            // switched away from it (the main thread).
            let status = unsafe { TISSelectInputSource(previous.as_CFTypeRef()) };
            if status != 0 {
                log::warn!("could not switch back to the input method (error {status})");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(code: u16, level: Level, ch: char) -> Entry {
        Entry {
            code,
            level,
            ch,
            dead: false,
        }
    }

    /// A slice of French AZERTY: key 0 types "q", 12 "a", 6 "w", 13 "z";
    /// the number row types "&" (1 with shift, and with ⌘), and key 33 is
    /// the dead circumflex.
    fn azerty() -> Vec<Entry> {
        let mut v = vec![
            e(0, Level::Plain, 'q'),
            e(0, Level::Shift, 'Q'),
            e(0, Level::Command, 'q'),
            e(12, Level::Plain, 'a'),
            e(12, Level::Shift, 'A'),
            e(12, Level::Command, 'a'),
            e(6, Level::Plain, 'w'),
            e(6, Level::Command, 'w'),
            e(13, Level::Plain, 'z'),
            e(13, Level::Command, 'z'),
            e(18, Level::Plain, '&'),
            e(18, Level::Shift, '1'),
            e(18, Level::Command, '1'),
            e(42, Level::Shift, '£'),
            e(37, Level::Option, '¬'),
            e(30, Level::Shift, '^'),
        ];
        v.push(Entry {
            code: 33,
            level: Level::Plain,
            ch: '^',
            dead: true,
        });
        v
    }

    fn stroke(code: u16, shift: bool, option: bool) -> Option<Stroke> {
        Some(Stroke {
            code,
            shift,
            option,
            dead: false,
        })
    }

    #[test]
    fn shortcuts_hit_the_key_that_types_the_letter() {
        let t = azerty();
        // cmd+a must not be key 0 (cmd+q, which quits), nor cmd+z key 6.
        assert_eq!(pick(&t, 'a', true), stroke(12, false, false));
        assert_eq!(pick(&t, 'z', true), stroke(13, false, false));
        assert_eq!(pick(&t, 'q', true), stroke(0, false, false));
        // An upper case letter: the same key, with shift.
        assert_eq!(pick(&t, 'A', true), stroke(12, true, false));
        // cmd+1 is the number-row key without shift (what ⌘ gives).
        assert_eq!(pick(&t, '1', true), stroke(18, false, false));
    }

    #[test]
    fn typing_picks_the_fewest_modifiers() {
        let t = azerty();
        assert_eq!(pick(&t, 'a', false), stroke(12, false, false));
        assert_eq!(pick(&t, '1', false), stroke(18, true, false));
        assert_eq!(pick(&t, '&', false), stroke(18, false, false));
        assert_eq!(pick(&t, 'A', false), stroke(12, true, false));
        assert_eq!(pick(&t, '£', false), stroke(42, true, false));
        assert_eq!(pick(&t, '¬', false), stroke(37, false, true));
        // Not on this layout.
        assert_eq!(pick(&t, 'é', false), None);
    }

    #[test]
    fn a_live_key_beats_a_dead_one() {
        let t = azerty();
        // "^" is a dead key unshifted (33), a live one with shift (30).
        assert_eq!(pick(&t, '^', false), stroke(30, true, false));
        // A dead key is still used when it is the only one.
        let only_dead: Vec<Entry> = t.into_iter().filter(|e| e.code != 30).collect();
        let dead = Stroke {
            code: 33,
            shift: false,
            option: false,
            dead: true,
        };
        assert_eq!(pick(&only_dead, '^', false), Some(dead));
    }

    #[test]
    fn keypad_keys_are_never_picked() {
        assert!(is_keypad(83));
        assert!(is_keypad(65));
        assert!(!is_keypad(18));
        assert!(!is_keypad(50));
    }
}
