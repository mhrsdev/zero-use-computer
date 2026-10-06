//! Keeping private data away from the model (`[privacy]` settings): password
//! fields, payment card numbers and fields with sensitive labels are masked
//! in element text and blacked out of screenshots before anything is sent.

use std::ops::Range;

use crate::config::PrivacyConfig;
use crate::text::{decimal_digit, is_digit};
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
    sum.is_multiple_of(10)
}

/// Byte ranges of payment card numbers in `s`: 13–19 digits (in any
/// script), optionally grouped by single spaces or dashes, that pass the
/// Luhn check.
pub fn card_numbers(s: &str) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let byte_at = |k: usize| chars.get(k).map_or(s.len(), |(b, _)| *b);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_digit(chars[i].1) {
            i += 1;
            continue;
        }
        // The run's groups of digits: (first char, end char, digits).
        let mut groups: Vec<(usize, usize, Vec<u8>)> = Vec::new();
        let mut first = i;
        let mut digits = Vec::new();
        let mut j = i;
        while j < chars.len() {
            let c = chars[j].1;
            if let Some(d) = decimal_digit(c) {
                digits.push(d);
                j += 1;
            } else if is_group_separator(c) && chars.get(j + 1).is_some_and(|(_, n)| is_digit(*n)) {
                groups.push((first, j, std::mem::take(&mut digits)));
                j += 1;
                first = j;
            } else {
                break;
            }
        }
        groups.push((first, j, digits));
        // The longest stretch of whole groups that is a card number, so a
        // number next to it (an expiry date, a CVV, a quantity) doesn't
        // hide it.
        let mut a = 0;
        while a < groups.len() {
            // Grown a group at a time and given up past 19 digits, so a
            // long line of digit groups costs no more than a short one.
            let mut digits: Vec<u8> = Vec::new();
            let mut found = None;
            for (b, g) in groups.iter().enumerate().skip(a) {
                digits.extend_from_slice(&g.2);
                if digits.len() > 19 {
                    break;
                }
                if digits.len() >= 13 && luhn(&digits) {
                    found = Some(b);
                }
            }
            match found {
                Some(b) => {
                    out.push(byte_at(groups[a].0)..byte_at(groups[b].1));
                    a = b + 1;
                }
                None => a += 1,
            }
        }
        i = j;
    }
    out
}

/// What may separate groups of digits in a card number or a code.
fn is_group_separator(c: char) -> bool {
    matches!(c, ' ' | '-' | '\u{00A0}')
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
        let total = run.chars().filter(|c| is_digit(*c)).count();
        let mut seen = 0;
        for c in run.chars() {
            if is_digit(c) {
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
/// standalone number of 5–8 digits (in any script, possibly split once by
/// a space or dash, as in "123 456"), or of 4 digits in text that mentions
/// a code, PIN or password. It doesn't depend on the language of the text
/// around it; English words only widen it to 4-digit codes.
pub fn mask_codes(s: &str) -> Option<String> {
    mask_codes_near(s, "")
}

/// [`mask_codes`] where a code, PIN or password may be mentioned in
/// `near` instead (a notification's title, for its body).
pub fn mask_codes_near(s: &str, near: &str) -> Option<String> {
    const WORDS: [&str; 8] = [
        "code",
        "otp",
        "pin",
        "passcode",
        "password",
        "verification",
        "verify",
        "2fa",
    ];
    // Whole words only: "shipping" and "barcode" mention no PIN or code.
    let low = format!("{s} {near}").to_lowercase();
    let mentioned = low
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| WORDS.contains(&word));
    let min_digits = if mentioned { 4 } else { 5 };
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        if is_digit(chars[i]) && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            // A run of digits, allowing one space or dash inside.
            let mut j = i;
            let mut digits = 0;
            let mut seps = 0;
            while j < chars.len() {
                if is_digit(chars[j]) {
                    digits += 1;
                } else if is_group_separator(chars[j])
                    && seps == 0
                    && chars.get(j + 1).is_some_and(|c| is_digit(*c))
                {
                    seps += 1;
                } else {
                    break;
                }
                j += 1;
            }
            let standalone = chars.get(j).is_none_or(|c| !c.is_alphanumeric());
            if (min_digits..=8).contains(&digits) && standalone {
                for c in &chars[i..j] {
                    out.push(if is_digit(*c) { MASK } else { *c });
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

    #[test]
    fn a_long_line_of_digit_groups_is_quick() {
        let line: String = (0..20_000).map(|i| format!("{} ", i % 10)).collect();
        let start = std::time::Instant::now();
        card_numbers(&line);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        // Still found among groups that aren't part of it.
        let s = "qty 3 4 card 4111 1111 1111 1111 12 34";
        assert_eq!(
            mask_card_numbers(s).unwrap(),
            mask_card_numbers("qty 3 4 card 4111 1111 1111 1111 12 34").unwrap()
        );
        assert!(mask_card_numbers(s).unwrap().contains("1111 12 34"));
    }

    #[test]
    fn a_card_next_to_other_numbers_is_still_masked() {
        for s in [
            "4111 1111 1111 1111 12/29",
            "4111 1111 1111 1111 123",
            "Visa 4111111111111111 2029",
            "Qty 1 4111 1111 1111 1111",
        ] {
            let m = mask_card_numbers(s);
            assert!(
                m.as_deref()
                    .is_some_and(|m| !m.contains("4111 1111 1111") && !m.contains("4111111111")),
                "card left visible in {s:?}: {m:?}"
            );
        }
    }

    #[test]
    fn code_words_count_only_as_whole_words() {
        assert_eq!(mask_codes("Shipping in 2024"), None);
        assert_eq!(mask_codes("Your opinion matters since 1998"), None);
        assert!(mask_codes("Your PIN: 4821").is_some());
        assert_eq!(mask_codes("4821"), None);
        assert!(mask_codes_near("4821", "Your bank PIN").is_some());
    }
    use crate::types::NodeStates;

    #[test]
    fn masks_codes_only_where_a_code_is_meant() {
        assert_eq!(
            mask_codes("Your verification code is 482913").as_deref(),
            Some("Your verification code is ••••••")
        );
        // Whatever the language: a 5–8 digit code, in any script's digits.
        assert_eq!(
            mask_codes("Sign-in: 123 456").as_deref(),
            Some("Sign-in: ••• •••")
        );
        assert_eq!(
            mask_codes("\u{06F4}\u{06F8}\u{06F2}\u{06F9}\u{06F1}\u{06F3}").as_deref(),
            Some("••••••")
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
        // The same card number written in Extended Arabic-Indic digits.
        let ext: String = "4111 1111 1111 1111"
            .chars()
            .map(|c| match c.to_digit(10) {
                Some(d) => char::from_u32(0x06F0 + d).unwrap(),
                None => c,
            })
            .collect();
        let masked = mask_card_numbers(&ext).unwrap();
        assert!(masked.starts_with("•••• •••• •••• "), "{masked}");
        assert!(
            !masked.ends_with("1111"),
            "the last four stay in their script"
        );
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
