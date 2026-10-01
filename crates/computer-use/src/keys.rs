//! Platform-neutral key combos: `"cmd+shift+s"`, `"Return"`, `"ctrl+alt+t"`,
//! `"Down Down Return"` (a space-separated sequence).

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command on macOS, Windows key on Windows, Super on Linux.
    pub meta: bool,
}

impl Modifiers {
    pub fn any(&self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Return,
    Tab,
    Space,
    Backspace,
    /// Forward delete.
    Delete,
    Escape,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    Insert,
    CapsLock,
    Menu,
    F(u8),
    /// A key on the numeric keypad (Blender's views, calculators).
    Numpad(Pad),
}

/// Keys of the numeric keypad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pad {
    Digit(u8),
    Decimal,
    Add,
    Subtract,
    Multiply,
    Divide,
    Enter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character; ASCII letters are always lowercase (use `shift+`).
    Char(char),
    Named(NamedKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl std::fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = self.modifiers;
        for (on, name) in [
            (m.ctrl, "ctrl"),
            (m.alt, "alt"),
            (m.shift, "shift"),
            (m.meta, "meta"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            Key::Char(c) => write!(f, "{c}"),
            Key::Named(NamedKey::F(n)) => write!(f, "F{n}"),
            Key::Named(NamedKey::Numpad(Pad::Digit(d))) => write!(f, "Numpad{d}"),
            Key::Named(NamedKey::Numpad(p)) => write!(f, "Numpad{p:?}"),
            Key::Named(k) => write!(f, "{k:?}"),
        }
    }
}

/// Parse one combo or a space-separated sequence of combos.
pub fn parse_sequence(input: &str) -> Result<Vec<KeyCombo>> {
    let combos: Vec<KeyCombo> = input
        .split_whitespace()
        .map(parse_combo)
        .collect::<Result<_>>()?;
    if combos.is_empty() {
        return Err(Error::InvalidArgs("`key` must not be empty".into()));
    }
    Ok(combos)
}

/// Parse a single combo such as `cmd+shift+s`, `ctrl++` or `Page_Up`.
pub fn parse_combo(input: &str) -> Result<KeyCombo> {
    let bad = |why: &str| Error::InvalidArgs(format!("invalid key `{input}`: {why}"));
    // Split on '+' but let a trailing '+' be the key itself ("ctrl++").
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = input;
    while let Some(pos) = rest.find('+') {
        if pos == 0 {
            // Leading '+': it is the key if it is the last char, else an empty part.
            if rest.len() == 1 {
                break;
            }
            return Err(bad("empty modifier"));
        }
        parts.push(&rest[..pos]);
        rest = &rest[pos + 1..];
    }
    parts.push(rest);
    let (key_part, mod_parts) = parts.split_last().ok_or_else(|| bad("empty"))?;
    if key_part.is_empty() {
        return Err(bad("missing key after modifiers"));
    }

    let mut modifiers = Modifiers::default();
    for m in mod_parts {
        match normalize(m).as_str() {
            "shift" => modifiers.shift = true,
            "ctrl" | "control" | "ctl" => modifiers.ctrl = true,
            "alt" | "option" | "opt" => modifiers.alt = true,
            // The Windows/Super key only when named as such: models write
            // "cmd+s" meaning "save", and on Windows Win+L locks the screen
            // and Win+R opens Run.
            "meta" | "super" | "win" | "windows" => modifiers.meta = true,
            // The platform's primary shortcut modifier: Cmd on a Mac, Ctrl
            // elsewhere.
            "cmd" | "command" | "primary" | "mod" | "cmdorctrl" => {
                if cfg!(target_os = "macos") {
                    modifiers.meta = true
                } else {
                    modifiers.ctrl = true
                }
            }
            other => return Err(bad(&format!("unknown modifier `{other}`"))),
        }
    }

    let key = parse_key(key_part).ok_or_else(|| bad(&format!("unknown key `{key_part}`")))?;
    Ok(KeyCombo { modifiers, key })
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '_' && *c != '-')
        .collect::<String>()
        .to_lowercase()
}

/// `numpad7`, `kp7` -> 7.
fn numpad_digit(name: &str) -> Option<u8> {
    let d = name
        .strip_prefix("numpad")
        .or_else(|| name.strip_prefix("kp"))?;
    match d.as_bytes() {
        [c] if c.is_ascii_digit() => Some(c - b'0'),
        _ => None,
    }
}

fn parse_key(s: &str) -> Option<Key> {
    let mut chars = s.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(Key::Char(c.to_ascii_lowercase()));
    }
    let n = normalize(s);
    let named = match n.as_str() {
        "return" | "enter" => NamedKey::Return,
        "numpadenter" | "kpenter" => NamedKey::Numpad(Pad::Enter),
        "numpaddecimal" | "numpadperiod" | "numpaddot" | "kpdecimal" => {
            NamedKey::Numpad(Pad::Decimal)
        }
        "numpadadd" | "numpadplus" | "kpadd" => NamedKey::Numpad(Pad::Add),
        "numpadsubtract" | "numpadminus" | "kpsubtract" => NamedKey::Numpad(Pad::Subtract),
        "numpadmultiply" | "kpmultiply" => NamedKey::Numpad(Pad::Multiply),
        "numpaddivide" | "kpdivide" => NamedKey::Numpad(Pad::Divide),
        p if numpad_digit(p).is_some() => NamedKey::Numpad(Pad::Digit(numpad_digit(p)?)),
        "tab" => NamedKey::Tab,
        "space" | "spacebar" => NamedKey::Space,
        "backspace" | "back" => NamedKey::Backspace,
        "delete" | "del" | "forwarddelete" => NamedKey::Delete,
        "escape" | "esc" => NamedKey::Escape,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" | "pgup" | "prior" => NamedKey::PageUp,
        "pagedown" | "pgdn" | "next" => NamedKey::PageDown,
        "left" | "arrowleft" | "leftarrow" => NamedKey::Left,
        "right" | "arrowright" | "rightarrow" => NamedKey::Right,
        "up" | "arrowup" | "uparrow" => NamedKey::Up,
        "down" | "arrowdown" | "downarrow" => NamedKey::Down,
        "insert" | "ins" => NamedKey::Insert,
        "capslock" => NamedKey::CapsLock,
        "menu" | "contextmenu" | "apps" => NamedKey::Menu,
        "plus" => return Some(Key::Char('+')),
        "minus" => return Some(Key::Char('-')),
        "comma" => return Some(Key::Char(',')),
        "period" | "dot" => return Some(Key::Char('.')),
        "slash" => return Some(Key::Char('/')),
        "backslash" => return Some(Key::Char('\\')),
        "semicolon" => return Some(Key::Char(';')),
        "quote" | "apostrophe" => return Some(Key::Char('\'')),
        "grave" | "backtick" => return Some(Key::Char('`')),
        "equal" | "equals" => return Some(Key::Char('=')),
        "bracketleft" | "leftbracket" => return Some(Key::Char('[')),
        "bracketright" | "rightbracket" => return Some(Key::Char(']')),
        f if f.starts_with('f') && f.len() <= 3 => {
            let num: u8 = f[1..].parse().ok()?;
            if !(1..=24).contains(&num) {
                return None;
            }
            NamedKey::F(num)
        }
        _ => return None,
    };
    Some(Key::Named(named))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_keys() {
        let c = parse_combo("cmd+shift+S").unwrap();
        // "cmd" is the shortcut key: Cmd on a Mac, Ctrl elsewhere.
        let mac = cfg!(target_os = "macos");
        assert!(c.modifiers.meta == mac && c.modifiers.ctrl != mac && c.modifiers.shift);
        assert_eq!(c.key, Key::Char('s'));
        // The Windows/Super key only when named.
        let c = parse_combo("win+l").unwrap();
        assert!(c.modifiers.meta && !c.modifiers.ctrl);

        let c = parse_combo("Return").unwrap();
        assert!(!c.modifiers.any());
        assert_eq!(c.key, Key::Named(NamedKey::Return));

        assert_eq!(
            parse_combo("Page_Up").unwrap().key,
            Key::Named(NamedKey::PageUp)
        );
        assert_eq!(parse_combo("f12").unwrap().key, Key::Named(NamedKey::F(12)));
        for (name, pad) in [
            ("Numpad0", Pad::Digit(0)),
            ("numpad_7", Pad::Digit(7)),
            ("KP_3", Pad::Digit(3)),
            ("NumpadDecimal", Pad::Decimal),
            ("numpad_minus", Pad::Subtract),
            ("NumpadEnter", Pad::Enter),
        ] {
            let c = parse_combo(name).unwrap();
            assert_eq!(c.key, Key::Named(NamedKey::Numpad(pad)), "{name}");
        }
        assert!(parse_combo("numpad12").is_err());
        assert_eq!(
            parse_combo("ctrl+numpad0").unwrap().to_string(),
            "ctrl+Numpad0"
        );
        assert_eq!(
            parse_combo("option+ArrowLeft").unwrap(),
            KeyCombo {
                modifiers: Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: Key::Named(NamedKey::Left)
            }
        );
    }

    #[test]
    fn plus_as_key() {
        let c = parse_combo("ctrl++").unwrap();
        assert!(c.modifiers.ctrl);
        assert_eq!(c.key, Key::Char('+'));
        assert_eq!(parse_combo("+").unwrap().key, Key::Char('+'));
    }

    #[test]
    fn sequences() {
        let seq = parse_sequence("Down Down ctrl+Return").unwrap();
        assert_eq!(seq.len(), 3);
        assert!(seq[2].modifiers.ctrl);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_combo("hyper+x").is_err());
        assert!(parse_combo("ctrl+").is_err());
        assert!(parse_combo("f99").is_err());
        assert!(parse_combo("notakey").is_err());
        assert!(parse_sequence("   ").is_err());
    }
}
