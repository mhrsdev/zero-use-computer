//! Understanding `launch_app` requests.
//!
//! A request is either an **app name** — often with spaces ("Google Chrome",
//! "Visual Studio Code") — or a **command line** (`code --new-window`,
//! `sh -c id`). Telling them apart matters twice: the policy must judge the
//! program of a command line, and the backends must find an app by its
//! display name (Start Menu shortcut on Windows, `.desktop` entry on Linux)
//! rather than run "Google" with the argument "Chrome".

use std::path::{Path, PathBuf};

/// The program of a request (its first word, or a leading quoted path) and
/// the rest of the line.
pub fn split_launch(request: &str) -> (String, String) {
    let r = request.trim();
    if let Some(rest) = r.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
    }
    match r.split_once(char::is_whitespace) {
        Some((p, tail)) => (p.to_string(), tail.trim().to_string()),
        None => (r.to_string(), String::new()),
    }
}

/// Whether the words after the program look like command-line arguments
/// (`-c`, `/c`, `--flag`, `KEY=value`, a path, a URL, a file name) rather than
/// more words of an app's name.
pub fn has_arguments(request: &str) -> bool {
    let (_, rest) = split_launch(request);
    rest.split_whitespace().any(|t| {
        t.starts_with(['-', '/', '"', '\'', '~', '$', '%', '@'])
            || t.contains(['=', '\\', ':', '.', ';', '&', '|', '<', '>', '`'])
    })
}

/// The words of a request, for checking each one against the policy (so a
/// wrapper like `sudo xterm` can't hide a terminal).
pub fn words(request: &str) -> Vec<String> {
    request
        .split_whitespace()
        .map(|w| w.trim_matches('"').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// A program as apps are matched: lowercase file name without `.exe`.
pub fn program_key(program: &str) -> String {
    let file = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_lowercase();
    file.strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(file)
}

/// The best candidate for `query`: exact name, then a name starting with it,
/// then one containing it (case-insensitive); the shortest name wins ties.
pub fn best_match<T>(query: &str, candidates: impl IntoIterator<Item = (String, T)>) -> Option<T> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    candidates
        .into_iter()
        .filter_map(|(name, item)| {
            let n = name.to_lowercase();
            let rank = if n == q {
                0
            } else if n.starts_with(&q) {
                1
            } else if n.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, n.len(), item))
        })
        .min_by_key(|(rank, len, _)| (*rank, *len))
        .map(|(_, _, item)| item)
}

fn walk(dir: &Path, depth: usize, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if depth > 0 {
                walk(&p, depth - 1, ext, out);
            }
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            out.push(p);
        }
    }
}

/// A Start Menu shortcut (`.lnk`) whose name matches `name`.
pub fn find_shortcut(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let mut all = Vec::new();
    for d in dirs {
        walk(d, 3, "lnk", &mut all);
    }
    best_match(
        name,
        all.into_iter().filter_map(|p| {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            Some((stem, p))
        }),
    )
}

/// An application from a freedesktop `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// The file name without `.desktop` (`google-chrome`).
    pub id: String,
    /// `Name=` (`Google Chrome`).
    pub name: String,
    /// The command to run, with the `%f`/`%U`… placeholders removed.
    pub command: Vec<String>,
}

fn parse_desktop(id: &str, text: &str) -> Option<DesktopEntry> {
    let mut in_entry = false;
    let (mut name, mut exec, mut kind) = (None, None, None);
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("Type=") {
            kind = Some(v.to_string());
        } else if line == "NoDisplay=true" || line == "Hidden=true" {
            hidden = true;
        }
    }
    if hidden || kind.as_deref() != Some("Application") {
        return None;
    }
    let command = shell_words(&exec?);
    (!command.is_empty()).then(|| DesktopEntry {
        id: id.to_string(),
        name: name.unwrap_or_else(|| id.to_string()),
        command,
    })
}

/// Split an `Exec=` line into words (double quotes group; `%f`, `%U`… are
/// dropped, `%%` is a percent sign).
fn shell_words(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = exec.chars().peekable();
    let mut started = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(cur);
    }
    words
        .into_iter()
        .filter_map(|w| {
            if w.len() == 2 && w.starts_with('%') && w != "%%" {
                return None; // a field code
            }
            Some(w.replace("%%", "%"))
        })
        .collect()
}

/// The application in `dirs` whose name or id matches `name`.
pub fn find_desktop_entry(dirs: &[PathBuf], name: &str) -> Option<DesktopEntry> {
    let mut files = Vec::new();
    for d in dirs {
        walk(d, 2, "desktop", &mut files);
    }
    let entries: Vec<DesktopEntry> = files
        .iter()
        .filter_map(|p| {
            let id = p.file_stem()?.to_string_lossy().into_owned();
            parse_desktop(&id, &std::fs::read_to_string(p).ok()?)
        })
        .collect();
    let by_name = best_match(name, entries.iter().map(|e| (e.name.clone(), e.clone())));
    by_name.or_else(|| best_match(name, entries.iter().map(|e| (e.id.clone(), e.clone()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_with_spaces_are_names_not_command_lines() {
        for name in [
            "Google Chrome",
            "Visual Studio Code",
            "Microsoft Edge",
            "Adobe Photoshop 2026",
            "Notepad",
            "Windows Terminal",
        ] {
            assert!(!has_arguments(name), "{name}");
        }
        for cmd in [
            "xterm -e sh",
            "sh -c id",
            "cmd /c calc",
            "code --new-window",
            "python3 evil.py",
            "env FOO=bar xterm",
            "notepad C:\\x.txt",
            "chrome https://example.com",
            "\"C:\\Program Files\\x.exe\" --a",
        ] {
            assert!(has_arguments(cmd), "{cmd}");
        }
    }

    #[test]
    fn splitting_and_keys() {
        assert_eq!(
            split_launch("  \"C:\\Program Files\\x.exe\" --a "),
            ("C:\\Program Files\\x.exe".to_string(), "--a".to_string())
        );
        assert_eq!(
            split_launch("Google Chrome"),
            ("Google".to_string(), "Chrome".to_string())
        );
        assert_eq!(program_key("C:\\Apps\\Calc.EXE"), "calc");
        assert_eq!(words("sudo \"xterm\" -e"), ["sudo", "xterm", "-e"]);
    }

    #[test]
    fn best_match_prefers_exact_then_prefix_then_shortest() {
        let c = |v: &[&str]| {
            v.iter()
                .map(|s| (s.to_string(), s.to_string()))
                .collect::<Vec<_>>()
        };
        let all = c(&[
            "Google Chrome Canary",
            "Google Chrome",
            "Chrome Remote Desktop",
        ]);
        assert_eq!(
            best_match("google chrome", all.clone()).unwrap(),
            "Google Chrome"
        );
        assert_eq!(
            best_match("chrome", all.clone()).unwrap(),
            "Chrome Remote Desktop"
        );
        assert_eq!(
            best_match("remote", all.clone()).unwrap(),
            "Chrome Remote Desktop"
        );
        assert!(best_match("firefox", all).is_none());
    }

    #[test]
    fn finds_start_menu_shortcuts_and_desktop_entries() {
        let dir = std::env::temp_dir().join(format!("cu-launch-{}", std::process::id()));
        let menu = dir.join("Programs").join("Google Chrome");
        std::fs::create_dir_all(&menu).unwrap();
        std::fs::write(menu.join("Google Chrome.lnk"), "x").unwrap();
        std::fs::write(dir.join("Programs").join("Notepad.lnk"), "x").unwrap();
        let found = find_shortcut(&[dir.join("Programs")], "google chrome").unwrap();
        assert!(found.ends_with("Google Chrome.lnk"), "{found:?}");
        assert!(find_shortcut(&[dir.join("Programs")], "nothing here").is_none());

        let apps = dir.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("google-chrome.desktop"),
            "[Desktop Entry]\nType=Application\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\n[Desktop Action new-window]\nName=New Window\nExec=/usr/bin/google-chrome-stable\n",
        )
        .unwrap();
        std::fs::write(
            apps.join("hidden.desktop"),
            "[Desktop Entry]\nType=Application\nName=Google Hidden\nNoDisplay=true\nExec=x\n",
        )
        .unwrap();
        let e = find_desktop_entry(std::slice::from_ref(&apps), "Google Chrome").unwrap();
        assert_eq!(e.id, "google-chrome");
        assert_eq!(e.command, ["/usr/bin/google-chrome-stable"]);
        // The id matches too (`chrome`), hidden entries never do.
        assert!(find_desktop_entry(std::slice::from_ref(&apps), "google-chrome").is_some());
        assert!(find_desktop_entry(std::slice::from_ref(&apps), "Google Hidden").is_none());
        assert_eq!(
            shell_words(r#""/opt/My App/run" --name "a b" %f %% "#),
            ["/opt/My App/run", "--name", "a b", "%"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
