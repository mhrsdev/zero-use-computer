//! Built-in skills: short how-to playbooks for common desktop tasks, written
//! separately for each operating system (shortcuts, app names and quirks
//! differ). The agent lists them and reads the one it needs with the `skill`
//! tool; they are compiled into the binary, so nothing has to be installed.

/// The operating systems that have their own skill set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    /// The OS this build runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::MacOs => "macOS",
            Os::Linux => "Linux",
        }
    }
}

/// One skill for one OS.
#[derive(Debug, Clone, Copy)]
pub struct Skill {
    pub name: &'static str,
    /// When to read it.
    pub summary: &'static str,
    pub body: &'static str,
}

macro_rules! skill {
    ($os:literal, $name:literal, $summary:literal) => {
        Skill {
            name: $name,
            summary: $summary,
            body: include_str!(concat!("../skills/", $os, "/", $name, ".md")),
        }
    };
}

macro_rules! skill_set {
    ($os:literal) => {
        &[
            skill!(
                $os,
                "files",
                "Create, move, rename, delete, find files and folders."
            ),
            skill!(
                $os,
                "browser",
                "Open pages, tabs, forms, downloads in a web browser."
            ),
            skill!(
                $os,
                "settings",
                "System settings, Wi-Fi/display/sound, uninstalling, security prompts."
            ),
            skill!(
                $os,
                "apps-and-windows",
                "Start apps, switch/snap/close windows, find menus."
            ),
            skill!(
                $os,
                "text-and-dialogs",
                "Typing, clipboard, shortcuts, open/save dialogs, confirmations."
            ),
            skill!(
                $os,
                "vscode",
                "Visual Studio Code: command palette, files, editing, search, terminal caveats."
            ),
            skill!(
                $os,
                "browser-apps",
                "Chrome/Edge/Firefox/Safari features, hard-to-read pages, untrusted web content."
            ),
            skill!(
                $os,
                "word",
                "Word processing: Microsoft Word (LibreOffice Writer on Linux) — styles, tracked changes, PDF."
            ),
            skill!(
                $os,
                "excel",
                "Spreadsheets: Microsoft Excel (LibreOffice Calc on Linux) — cells, formulas, data, pasting."
            ),
            skill!(
                $os,
                "powerpoint",
                "Presentations: PowerPoint (Impress on Linux, Keynote on Mac) — slides, outline, masters, present."
            ),
            skill!(
                $os,
                "outlook",
                "Outlook mail and calendar (web version on Linux); drafting vs sending."
            ),
            skill!(
                $os,
                "email",
                "Any email client: draft-don't-send rules, verifying recipients, phishing, search."
            ),
            skill!(
                $os,
                "messaging",
                "WhatsApp/Telegram/Signal/Teams/iMessage: who you write to, newline sends, scams."
            ),
            skill!(
                $os,
                "slack",
                "Slack: quick switcher, channels vs DMs, newline sends, pings."
            ),
            skill!(
                $os,
                "discord",
                "Discord: servers/channels, quick switcher, pings, account rules."
            ),
            skill!(
                $os,
                "photoshop",
                "Photoshop (GIMP on Linux): work on a copy, tools, layers, export."
            ),
            skill!(
                $os,
                "documents",
                "Word processors, spreadsheets, presentations, PDF export."
            ),
            skill!(
                $os,
                "troubleshooting",
                "An app shows nothing, a click did nothing, something is blocked."
            ),
        ]
    };
}

/// All skills for `os`.
pub fn skills_for(os: Os) -> &'static [Skill] {
    match os {
        Os::Windows => skill_set!("windows"),
        Os::MacOs => skill_set!("macos"),
        Os::Linux => skill_set!("linux"),
    }
}

/// The `skill` tool: with no `name`, list the skills for `os`; with a name
/// (case-insensitive, exact, or an unambiguous prefix), return that playbook.
pub fn lookup(os: Os, name: Option<&str>) -> Result<String, String> {
    let all = skills_for(os);
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        let mut out = format!(
            "Built-in skills for {} — read one with skill(name):\n",
            os.name()
        );
        for s in all {
            out.push_str(&format!("- {}: {}\n", s.name, s.summary));
        }
        return Ok(out);
    };
    let want = name.to_lowercase();
    if let Some(s) = all.iter().find(|s| s.name == want) {
        return Ok(s.body.to_string());
    }
    let hits: Vec<_> = all.iter().filter(|s| s.name.starts_with(&want)).collect();
    match hits.as_slice() {
        [s] => Ok(s.body.to_string()),
        [] => Err(format!(
            "no skill named `{name}` for {}. Available: {}.",
            os.name(),
            all.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
        )),
        many => Err(format!(
            "`{name}` matches several skills: {}. Use the full name.",
            many.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_os_has_the_same_skills_and_real_content() {
        let names = |os| skills_for(os).iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(Os::Windows), names(Os::MacOs));
        assert_eq!(names(Os::Windows), names(Os::Linux));
        for os in [Os::Windows, Os::MacOs, Os::Linux] {
            for s in skills_for(os) {
                assert!(s.body.starts_with("# "), "{} {}", os.name(), s.name);
                assert!(s.body.len() > 300, "{} {} is too short", os.name(), s.name);
                assert!(!s.summary.is_empty());
            }
        }
    }

    #[test]
    fn skills_are_specific_to_their_os() {
        let body = |os, n| lookup(os, Some(n)).unwrap();
        assert!(body(Os::Windows, "files").contains("Explorer"));
        assert!(body(Os::MacOs, "files").contains("Finder"));
        assert!(body(Os::Linux, "files").contains("Nautilus"));
        assert!(body(Os::MacOs, "vscode").contains("cmd+shift+p"));
        assert!(body(Os::Windows, "vscode").contains("ctrl+shift+p"));
        assert!(body(Os::MacOs, "browser-apps").contains("Safari"));
        assert!(!body(Os::Windows, "browser-apps").contains("Safari"));
        assert!(body(Os::Windows, "excel").contains("Name Box"));
        assert!(body(Os::Linux, "excel").contains("LibreOffice Calc"));
        assert!(body(Os::Linux, "photoshop").contains("GIMP"));
        assert!(body(Os::MacOs, "outlook").contains("cmd+Return"));
        assert!(body(Os::Linux, "outlook").contains("no Outlook desktop app"));
        for os in [Os::Windows, Os::MacOs, Os::Linux] {
            // Messaging apps must warn that a newline sends.
            for n in ["messaging", "slack", "discord"] {
                assert!(body(os, n).to_lowercase().contains("newline"), "{n}");
            }
            assert!(body(os, "email").contains("Draft, don't send"));
        }
        assert!(body(Os::MacOs, "text-and-dialogs").contains("cmd+"));
        assert!(!body(Os::Windows, "text-and-dialogs").contains("cmd+"));
    }

    #[test]
    fn lookup_lists_matches_prefixes_and_reports_unknowns() {
        let list = lookup(Os::Linux, None).unwrap();
        assert!(list.contains("Linux") && list.contains("- browser:"));
        assert!(lookup(Os::Linux, Some("FILES")).is_ok());
        assert!(lookup(Os::Linux, Some("trouble")).is_ok());
        assert!(
            lookup(Os::Linux, Some("nope"))
                .unwrap_err()
                .contains("Available")
        );
    }
}
