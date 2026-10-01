//! Text handling that works the same for every script, including
//! right-to-left ones (Arabic script, Hebrew): digits in any script,
//! invisible bidirectional controls, and matching that ignores the
//! differences people (and models) don't see.

/// The value of a decimal digit in any common script: ASCII, Arabic-Indic,
/// Extended Arabic-Indic, Devanagari, Bengali, Thai, fullwidth…
pub fn decimal_digit(c: char) -> Option<u8> {
    if c.is_ascii_digit() {
        return Some(c as u8 - b'0');
    }
    const ZEROS: [u32; 10] = [
        0x0660, // Arabic-Indic
        0x06F0, // Extended Arabic-Indic
        0x07C0, // NKo
        0x0966, // Devanagari
        0x09E6, // Bengali
        0x0A66, // Gurmukhi
        0x0AE6, // Gujarati
        0x0E50, // Thai
        0x1040, // Myanmar
        0xFF10, // Fullwidth
    ];
    let cp = c as u32;
    ZEROS
        .iter()
        .find(|z| (**z..**z + 10).contains(&cp))
        .map(|z| (cp - z) as u8)
}

/// Whether `c` is a decimal digit in any common script.
pub fn is_digit(c: char) -> bool {
    decimal_digit(c).is_some()
}

/// Invisible characters that only steer the display order of mixed
/// left-to-right / right-to-left text. They carry no meaning for the model
/// and can make text read in a different order than it is stored.
pub fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// `s` without bidirectional controls (joiners and non-joiners are kept:
/// they change how words are written).
pub fn strip_bidi_controls(s: &str) -> std::borrow::Cow<'_, str> {
    if s.chars().any(is_bidi_control) {
        std::borrow::Cow::Owned(s.chars().filter(|c| !is_bidi_control(*c)).collect())
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// A form of `s` for "does this text contain that" matching: lowercase,
/// digits of every script as ASCII, and the variants that look alike
/// folded together, so a query typed one way finds text written another:
///
/// * Arabic-script letters that are often typed with either of two code
///   points (yeh U+064A / U+06CC, alef maksura U+0649, kaf U+0643 /
///   U+06A9);
/// * tatweel (the stretching stroke), short-vowel marks and other
///   combining marks, zero-width (non-)joiners and bidi controls are
///   dropped;
/// * any run of whitespace (including no-break spaces) is one space.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if let Some(d) = decimal_digit(c) {
            out.push((b'0' + d) as char);
            space = false;
            continue;
        }
        let c = match c {
            // U+064A, U+0649 -> U+06CC; U+0643 -> U+06A9.
            '\u{064A}' | '\u{0649}' => '\u{06CC}',
            '\u{0643}' => '\u{06A9}',
            // Tatweel, harakat and other Arabic marks, joiners, bidi controls.
            '\u{0640}' | '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{200B}'..='\u{200D}' => {
                continue;
            }
            c if is_bidi_control(c) => continue,
            c => c,
        };
        if c.is_whitespace() {
            if !space && !out.is_empty() {
                out.push(' ');
            }
            space = true;
            continue;
        }
        space = false;
        out.extend(c.to_lowercase());
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Whether `haystack` contains `needle`, compared in [`fold`]ed form.
pub fn contains(haystack: &str, needle: &str) -> bool {
    fold(haystack).contains(&fold(needle))
}

/// A rough count of the tokens `s` costs a model: about four characters
/// per token for ASCII, about two for other scripts (they tokenize less
/// densely). Used to keep tool results inside their token budgets.
pub fn estimate_tokens(s: &str) -> usize {
    let (mut ascii, mut other) = (0usize, 0usize);
    for c in s.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    ascii.div_ceil(4) + other.div_ceil(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimates() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("\u{05E9}\u{05DC}\u{05D5}\u{05DD}"), 2);
    }

    #[test]
    fn digits_of_any_script() {
        assert_eq!(decimal_digit('7'), Some(7));
        assert_eq!(decimal_digit('\u{0663}'), Some(3)); // Arabic-Indic 3
        assert_eq!(decimal_digit('\u{06F9}'), Some(9)); // Extended Arabic-Indic 9
        assert_eq!(decimal_digit('\u{FF15}'), Some(5)); // fullwidth 5
        assert_eq!(decimal_digit('x'), None);
    }

    #[test]
    fn bidi_controls_are_dropped_joiners_kept() {
        let s = "\u{202B}abc\u{202C} d\u{200C}e\u{200F}";
        assert_eq!(strip_bidi_controls(s), "abc d\u{200C}e");
        assert!(matches!(
            strip_bidi_controls("plain"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn folding_matches_look_alike_text() {
        // The two code points of yeh and kaf, with a tatweel, a short
        // vowel mark and a zero-width non-joiner thrown in.
        let stored = "\u{06A9}\u{06CC}\u{200C}\u{0628}";
        let typed = "\u{0643}\u{0640}\u{064A}\u{064E}\u{0628}";
        assert!(contains(stored, typed));
        // Digits of any script, case and spacing.
        assert!(contains("Order  \u{06F1}\u{06F2}\u{06F3}", "order 123"));
        assert!(contains("Save\u{00A0}As", "save as"));
        assert!(!contains("Save", "saved"));
    }
}
