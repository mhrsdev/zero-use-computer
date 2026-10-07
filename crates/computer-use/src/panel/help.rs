//! The guide inside the panel: short pages written by hand, kept in the
//! program. Each is a piece of HTML the page shows as it is.

/// (address, title, text). The address is `help-` plus the first part.
pub const PAGES: &[(&str, &str, &str)] = &[
    ("start", "Getting started", include_str!("help/start.html")),
    (
        "pointers",
        "Pointers and the mouse",
        include_str!("help/pointers.html"),
    ),
    (
        "tokens",
        "Spending fewer tokens",
        include_str!("help/tokens.html"),
    ),
    ("agents", "Several agents", include_str!("help/agents.html")),
    (
        "safety",
        "Staying in control",
        include_str!("help/safety.html"),
    ),
    ("updates", "Updates", include_str!("help/updates.html")),
    (
        "decision",
        "The decision model",
        include_str!("help/decision.html"),
    ),
    (
        "files",
        "Files and the command line",
        include_str!("help/files.html"),
    ),
    (
        "trouble",
        "When something is wrong",
        include_str!("help/trouble.html"),
    ),
];

pub fn page(slug: &str) -> Option<&'static str> {
    PAGES.iter().find(|p| p.0 == slug).map(|p| p.2)
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
        for (name, _, text) in PAGES {
            for l in links(text) {
                assert!(
                    ids.iter().any(|i| i == l),
                    "help/{name}.html links to #{l}, which is not a page"
                );
            }
        }
    }

    #[test]
    fn the_pages_are_whole_and_plain() {
        for (name, title, text) in PAGES {
            assert!(text.len() > 500, "{name} is short");
            assert!(!title.is_empty());
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
