//! The panel's languages. English is what the program is written in; the
//! others are files of translations (`lang/<code>.json`, see `lang/README.md`)
//! the page asks for when it needs one, so a visit in English carries none
//! of them. Persian is written right to left and brings its own font
//! (Vazirmatn, SIL Open Font License), because a computer with no good
//! Persian font would show it in whatever is at hand.

/// (code, the language's own name, written right to left).
pub const LANGUAGES: &[(&str, &str, bool)] = &[
    ("en", "English", false),
    ("fa", "فارسی", true),
    ("zh", "中文", false),
    ("ru", "Русский", false),
];

/// The translations of `code` (not English), as JSON.
pub fn translations(code: &str) -> Option<&'static str> {
    Some(match code {
        "fa" => include_str!("lang/fa.json"),
        "zh" => include_str!("lang/zh.json"),
        "ru" => include_str!("lang/ru.json"),
        _ => return None,
    })
}

/// Every text of the page in English, the list the translations follow.
#[cfg(test)]
pub const SOURCE: &str = include_str!("lang/en.json");

/// The Persian font.
pub fn font() -> &'static [u8] {
    include_bytes!("lang/fonts/Vazirmatn-Variable.woff2")
}

/// Whether `code` is written right to left.
pub fn is_rtl(code: &str) -> bool {
    LANGUAGES.iter().any(|l| l.0 == code && l.2)
}

/// `code` if it is one the page has, else English.
pub fn known(code: &str) -> &'static str {
    LANGUAGES.iter().find(|l| l.0 == code).map_or("en", |l| l.0)
}
