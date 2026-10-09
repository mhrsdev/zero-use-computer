//! The guide inside the panel: short pages written by hand, kept in the
//! program. Each is a piece of HTML the page shows as it is, in English
//! (`help/<slug>.html`) and in the other languages (`help/<slug>.<lang>.html`).

macro_rules! page {
    ($slug:literal, $title:literal) => {
        (
            $slug,
            $title,
            include_str!(concat!("help/", $slug, ".html")),
            [
                include_str!(concat!("help/", $slug, ".fa.html")),
                include_str!(concat!("help/", $slug, ".zh.html")),
                include_str!(concat!("help/", $slug, ".ru.html")),
            ],
        )
    };
}

/// (address, title, text, the text in Persian, Chinese and Russian). The
/// address is `help-` plus the first part.
pub const PAGES: &[(&str, &str, &str, [&str; 3])] = &[
    page!("start", "Getting started"),
    page!("pointers", "Pointers and the mouse"),
    page!("tokens", "Spending fewer tokens"),
    page!("agents", "Several agents"),
    page!("safety", "Staying in control"),
    page!("updates", "Updates"),
    page!("decision", "The decision model"),
    page!("files", "Files and the command line"),
    page!("trouble", "When something is wrong"),
];

/// The page `slug` in `lang` (English when it has no such translation).
pub fn page(slug: &str, lang: &str) -> Option<&'static str> {
    let p = PAGES.iter().find(|p| p.0 == slug)?;
    Some(match lang {
        "fa" => p.3[0],
        "zh" => p.3[1],
        "ru" => p.3[2],
        _ => p.2,
    })
}

/// For the schema: what the page lists.
pub fn list() -> serde_json::Value {
    PAGES
        .iter()
        .map(|p| serde_json::json!({"slug": p.0, "title": p.1}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every address a link in the guide points at.
    fn links(text: &str) -> Vec<&str> {
        text.split("href=\"#")
            .skip(1)
            .filter_map(|r| r.split('"').next())
            .collect()
    }

    #[test]
    fn every_link_in_the_guide_goes_to_a_page_that_exists() {
        let slug = |s: &str| {
            s.to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("-")
        };
        let mut ids: Vec<String> = super::super::schema::GROUPS
            .iter()
            .map(|g| slug(g))
            .collect();
        ids.extend(
            [
                "overview",
                "connect",
                "shortcut",
                "profiles",
                "tool-list",
                "apps-report",
                "audit-log",
                "settings-file",
                "import-export",
            ]
            .map(String::from),
        );
        ids.extend(PAGES.iter().map(|p| format!("help-{}", p.0)));
        for (name, _, text, others) in PAGES {
            for text in std::iter::once(text).chain(others) {
                for l in links(text) {
                    assert!(
                        ids.iter().any(|i| i == l),
                        "help/{name}.html links to #{l}, which is not a page"
                    );
                }
            }
        }
    }

    #[test]
    fn the_pages_are_whole_and_plain() {
        for (name, title, text, others) in PAGES {
            assert!(!title.is_empty());
            for (i, text) in std::iter::once(text).chain(others).enumerate() {
                let name = format!("{name} ({})", ["en", "fa", "zh", "ru"][i]);
                // A translation is shorter in Chinese, longer in Russian.
                assert!(text.len() > 300, "{name} is short");
                assert!(
                    !text.contains("<script")
                        && !text.contains("onclick")
                        && !text.contains("javascript:"),
                    "{name}"
                );
                // Every tag opened is closed.
                for tag in ["h3", "p", "ul", "ol", "li", "b", "table", "tr", "td"] {
                    let open = text.matches(&format!("<{tag}>")).count()
                        + text.matches(&format!("<{tag} ")).count();
                    let close = text.matches(&format!("</{tag}>")).count();
                    assert_eq!(open, close, "{name}: <{tag}> opened {open}, closed {close}");
                }
            }
        }
    }
}
