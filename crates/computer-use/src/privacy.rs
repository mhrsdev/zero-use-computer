//! Keeping private data away from the model (`[privacy]` settings): password
//! fields, payment card numbers and fields with sensitive labels are masked
//! in element text and blacked out of screenshots before anything is sent.

use std::ops::Range;

use crate::config::PrivacyConfig;
use crate::types::{RawNode, Rect};

/// The masking character.
pub const MASK: char = '•';

/// Whether `role` is a password field (normalised across platforms).
pub fn is_password(role: &str) -> bool {
    role == "secure text field"
}

fn luhn(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut v = u32::from(*d);
        if i % 2 == 1 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    sum % 10 == 0
}

/// Byte ranges of payment card numbers in `s`: 13–19 digits, optionally
/// grouped by single spaces or dashes, that pass the Luhn check.
pub fn card_numbers(s: &str) -> Vec<Range<usize>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut digits = Vec::new();
        let mut j = i;
        while j < b.len() {
            if b[j].is_ascii_digit() {
                digits.push(b[j] - b'0');
                j += 1;
            } else if matches!(b[j], b' ' | b'-') && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
                j += 1;
            } else {
                break;
            }
        }
        if (13..=19).contains(&digits.len()) && luhn(&digits) {
            out.push(start..j);
        }
        i = j;
    }
    out
}

/// `s` with every card number masked except its last four digits.
pub fn mask_card_numbers(s: &str) -> Option<String> {
    let found = card_numbers(s);
    if found.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for r in found {
        out.push_str(&s[last..r.start]);
        let run = &s[r.clone()];
        let total = run.bytes().filter(u8::is_ascii_digit).count();
        let mut seen = 0;
        for c in run.chars() {
            if c.is_ascii_digit() {
                seen += 1;
                out.push(if seen + 4 > total { c } else { MASK });
            } else {
                out.push(c);
            }
        }
        last = r.end;
    }
    out.push_str(&s[last..]);
    Some(out)
}

/// `s` with what looks like a one-time / verification code masked: a
/// standalone 4–8 digit number (or two groups like "123 456") in text that
/// talks about a code, PIN or password.
pub fn mask_codes(s: &str) -> Option<String> {
    const WORDS: [&str; 10] = [
        "code",
        "otp",
        "pin",
        "passcode",
        "password",
        "verification",
        "verify",
        "2fa",
        "کد",
        "رمز",
    ];
    let low = s.to_lowercase();
    if !WORDS.iter().any(|w| low.contains(w)) {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            // A run of digits, allowing one space or dash inside.
            let mut j = i;
            let mut digits = 0;
            let mut seps = 0;
            while j < chars.len() {
                if chars[j].is_ascii_digit() {
                    digits += 1;
                } else if matches!(chars[j], ' ' | '-')
                    && seps == 0
                    && chars.get(j + 1).is_some_and(char::is_ascii_digit)
                {
                    seps += 1;
                } else {
                    break;
                }
                j += 1;
            }
            let standalone = chars.get(j).is_none_or(|c| !c.is_alphanumeric());
            if (4..=8).contains(&digits) && standalone {
                for c in &chars[i..j] {
                    out.push(if c.is_ascii_digit() { MASK } else { *c });
                }
                changed = true;
            } else {
                out.extend(&chars[i..j]);
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    changed.then_some(out)
}

fn masked(v: &str) -> String {
    MASK.to_string().repeat(v.chars().count().clamp(4, 8))
}

fn label_matches(cfg: &PrivacyConfig, n: &RawNode) -> bool {
    if cfg.redact_labels.is_empty() {
        return false;
    }
    let label = [&n.name, &n.description, &n.placeholder, &n.identifier]
        .into_iter()
        .flatten()
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    !label.is_empty()
        && cfg
            .redact_labels
            .iter()
            .any(|k| !k.trim().is_empty() && label.contains(&k.trim().to_lowercase()))
}

/// Mask private data in a freshly read tree, in place, and return the
/// screen rectangles to black out of screenshots of it.
pub fn scrub(nodes: &mut [RawNode], cfg: &PrivacyConfig) -> Vec<Rect> {
    let mut rects = Vec::new();
    for n in nodes.iter_mut() {
        let mut hide = false;
        if cfg.redact_passwords && is_password(&n.role) {
            // Never show a password, not even its length.
            n.value = n.value.take().filter(|v| v.is_empty());
            hide = true;
        }
        let entry = n.states.editable || n.value.is_some();
        if entry && label_matches(cfg, n) {
            if let Some(v) = n.value.as_mut().filter(|v| !v.is_empty()) {
                *v = masked(v);
            }
            hide = true;
        }
        if cfg.redact_card_numbers {
            for field in [&mut n.name, &mut n.value, &mut n.description] {
                if let Some(text) = field.as_ref()
                    && let Some(m) = mask_card_numbers(text)
                {
                    *field = Some(m);
                    hide = true;
                }
            }
        }
        if hide && let Some(b) = n.bounds.filter(|b| !b.is_empty()) {
            rects.push(b);
        }
    }
    rects
}

/// Whether any redaction is switched on.
pub fn active(cfg: &PrivacyConfig) -> bool {
    cfg.redact_passwords || cfg.redact_card_numbers || !cfg.redact_labels.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NodeStates;

    #[test]
    fn masks_codes_only_where_a_code_is_meant() {
        assert_eq!(
            mask_codes("Your verification code is 482913").as_deref(),
            Some("Your verification code is ••••••")
        );
        assert_eq!(
            mask_codes("کد ورود شما: 123 456").as_deref(),
            Some("کد ورود شما: ••• •••")
        );
        assert_eq!(mask_codes("Meeting at 1530 in room 4"), None);
        assert_eq!(mask_codes("code v2.1 released"), None);
        assert_eq!(mask_codes("Order 123456789 code"), None, "too long");
    }

    #[test]
    fn finds_card_numbers_only() {
        let s = "Card 4111 1111 1111 1111 exp 12/29, phone 555-123-4567";
        let r = card_numbers(s);
        assert_eq!(r.len(), 1);
        assert_eq!(&s[r[0].clone()], "4111 1111 1111 1111");
        assert_eq!(
            mask_card_numbers(s).unwrap(),
            "Card •••• •••• •••• 1111 exp 12/29, phone 555-123-4567"
        );
        assert_eq!(
            mask_card_numbers("5500-0000-0000-0004").unwrap(),
            "••••-••••-••••-0004"
        );
        // Not Luhn-valid, too short, or too long.
        assert!(card_numbers("4111 1111 1111 1112").is_empty());
        assert!(card_numbers("order 123456789012").is_empty());
        assert!(card_numbers("12345678901234567890123").is_empty());
        assert!(mask_card_numbers("nothing here").is_none());
    }

    fn node(role: &str, name: &str, value: Option<&str>) -> RawNode {
        RawNode {
            role: role.into(),
            name: Some(name.into()),
            value: value.map(String::from),
            bounds: Some(Rect::new(10.0, 20.0, 100.0, 24.0)),
            states: NodeStates {
                editable: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn scrubs_passwords_cards_and_labelled_fields() {
        let cfg = PrivacyConfig::default();
        let mut nodes = vec![
            node("secure text field", "Password", Some("hunter2")),
            node("text field", "Card", Some("4111111111111111")),
            node("text field", "CVV", Some("123")),
            node("text field", "Name", Some("Ada")),
        ];
        let rects = scrub(&mut nodes, &cfg);
        assert_eq!(rects.len(), 3);
        assert_eq!(nodes[0].value, None);
        assert_eq!(nodes[1].value.as_deref(), Some("••••••••••••1111"));
        assert_eq!(nodes[2].value.as_deref(), Some("••••"));
        assert_eq!(nodes[3].value.as_deref(), Some("Ada"));

        let off = PrivacyConfig {
            redact_passwords: false,
            redact_card_numbers: false,
            redact_labels: Vec::new(),
            ..cfg
        };
        assert!(!active(&off));
        let mut nodes = vec![node("text field", "Card", Some("4111111111111111"))];
        assert!(scrub(&mut nodes, &off).is_empty());
        assert_eq!(nodes[0].value.as_deref(), Some("4111111111111111"));
    }
}
