//! Finding an installed app by the name people know it by.
//!
//! `launch_app` takes one name, never a command line. When that name isn't a
//! program the OS can start directly, the backends look it up in the OS's
//! own list of installed apps: Start Menu shortcuts on Windows, `.desktop`
//! entries on Linux (`open -a` does it on macOS). Only an exact name counts:
//! a name that matches several apps, or none, is an error that lists the
//! candidates, so the agent never starts an app it didn't name.
//!
//! What runs is what the app's installer put in its shortcut or `.desktop`
//! file, the same as clicking the menu entry; nothing of the query becomes
//! an argument.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// How many near names a "not found" error suggests.
const SUGGESTIONS: usize = 5;

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

/// The result of looking a name up in a list of installed apps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick<T> {
    /// Exactly one app has this name.
    One(T),
    /// Several different apps have this name (their names).
    Many(Vec<String>),
    /// None has; names that contain the query, as suggestions.
    None(Vec<String>),
}

/// The candidate whose name is `query` (case-insensitive). Candidates are
/// `(name, key, item)`; entries with the same `key` are one app (the first
/// wins), so the same app listed twice is not ambiguous.
pub fn pick<T: Clone>(query: &str, candidates: &[(String, String, T)]) -> Pick<T> {
    let q = crate::text::fold(query.trim());
    let mut seen = HashSet::new();
    let unique: Vec<&(String, String, T)> = candidates
        .iter()
        .filter(|(_, key, _)| seen.insert(key.to_lowercase()))
        .collect();
    let exact: Vec<&&(String, String, T)> = unique
        .iter()
        .filter(|(name, _, _)| crate::text::fold(name) == q)
        .collect();
    match exact.as_slice() {
        [(_, _, item)] => Pick::One(item.clone()),
        [] => {
            let mut near: Vec<String> = Vec::new();
            for (name, _, _) in &unique {
                if !q.is_empty()
                    && crate::text::fold(name).contains(&q)
                    && !near.contains(name)
                    && near.len() < SUGGESTIONS
                {
                    near.push(name.clone());
                }
            }
            Pick::None(near)
        }
        many => {
            let mut names: Vec<String> = many.iter().map(|(n, _, _)| n.clone()).collect();
            names.dedup();
            Pick::Many(names)
        }
    }
}

/// The error text for a name that matched several apps or none.
pub fn not_found(query: &str, pick: &Pick<impl Sized>, catalog: &str) -> String {
    match pick {
        Pick::Many(names) => format!(
            "several apps in {catalog} are named `{query}`: {}. Ask the user which one.",
            names.join(", ")
        ),
        Pick::None(near) if !near.is_empty() => format!(
            "no app named `{query}` (not a program, nor in {catalog}). Similar names: {}. Pass one exactly.",
            near.join(", ")
        ),
        _ => format!("no app named `{query}` (not a program, nor in {catalog})."),
    }
}

fn walk(dir: &Path, depth: usize, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    // A stable order, so the same files always win.
    entries.sort();
    for p in entries {
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

/// Start Menu shortcuts (`.lnk`) in `dirs`, as `(name, key, path)`: the
/// shortcut's name is the app's. Uninstallers are left out. The same name
/// in two folders (per-user and all-users) is one app; the first folder
/// wins.
pub fn start_menu_shortcuts(dirs: &[PathBuf]) -> Vec<(String, String, PathBuf)> {
    let mut all = Vec::new();
    for d in dirs {
        walk(d, 3, "lnk", &mut all);
    }
    all.into_iter()
        .filter_map(|p| {
            let name = p.file_stem()?.to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if lower.starts_with("uninstall") || lower.contains(" uninstall") {
                return None;
            }
            Some((name, lower, p))
        })
        .collect()
}

/// An application from a freedesktop `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// The file name without `.desktop` (`google-chrome`).
    pub id: String,
    /// `Name=` (`Google Chrome`).
    pub name: String,
    /// The program and its fixed arguments, field codes (`%U`…) removed.
    pub exec: Vec<String>,
    /// False for entries that are hidden, not applications, run in a
    /// terminal, or whose `TryExec` program is missing. Kept (not dropped)
    /// so they still hide an entry with the same id in a later folder.
    pub usable: bool,
}

/// Parse a `.desktop` file; `None` if it has no `[Desktop Entry]`.
pub fn parse_desktop(id: &str, text: &str) -> Option<DesktopEntry> {
    let mut in_entry = false;
    let mut found = false;
    let (mut name, mut exec, mut kind, mut try_exec) = (None, None, None, None);
    let mut usable = true;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            found |= in_entry;
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" => name = Some(value.to_string()),
            "Exec" => exec = Some(value.to_string()),
            "Type" => kind = Some(value.to_string()),
            "TryExec" => try_exec = Some(value.to_string()),
            "NoDisplay" | "Hidden" | "Terminal" if value == "true" => usable = false,
            _ => {}
        }
    }
    if !found {
        return None;
    }
    let exec = exec.as_deref().map(exec_words).unwrap_or_default();
    usable &= kind.as_deref() == Some("Application")
        && !exec.is_empty()
        && try_exec.as_deref().is_none_or(program_exists);
    Some(DesktopEntry {
        id: id.to_string(),
        name: name.unwrap_or_else(|| id.to_string()),
        exec,
        usable,
    })
}

/// Split an `Exec=` value into words: double quotes group (a backslash
/// escapes the next character inside them), field codes (`%f`, `%U`…) are
/// removed (a word that was only a field code is dropped), `%%` is `%`.
pub fn exec_words(exec: &str) -> Vec<String> {
    let mut words: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut only_code = true;
    let mut started = false;
    let mut quoted = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
                only_code = false;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    only_code = false;
                }
            }
            '%' => match chars.next() {
                Some('%') => {
                    cur.push('%');
                    started = true;
                    only_code = false;
                }
                // A field code: nothing to put in its place.
                Some(_) => started = true,
                None => {
                    cur.push('%');
                    started = true;
                    only_code = false;
                }
            },
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push((std::mem::take(&mut cur), only_code));
                }
                started = false;
                only_code = true;
            }
            c => {
                cur.push(c);
                started = true;
                only_code = false;
            }
        }
    }
    if started {
        words.push((cur, only_code));
    }
    words
        .into_iter()
        .filter(|(w, only_code)| !(*only_code && w.is_empty()))
        .map(|(w, _)| w)
        .collect()
}

/// A program name on `PATH`, or a path to a file.
fn program_exists(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The applications in `dirs` (each searched two folders deep). The first
/// file with a given id wins, as in the spec, so a user's own copy (or a
/// `Hidden=true` override) replaces the system one.
pub fn desktop_entries(dirs: &[PathBuf]) -> Vec<DesktopEntry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in dirs {
        let mut files = Vec::new();
        walk(d, 2, "desktop", &mut files);
        for p in files {
            // The spec's id: the path below `applications/` with `-` for `/`.
            let id = p
                .strip_prefix(d)
                .unwrap_or(&p)
                .with_extension("")
                .to_string_lossy()
                .replace(['/', '\\'], "-");
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(e) = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| parse_desktop(&id, &t))
            {
                out.push(e);
            }
        }
    }
    out.retain(|e| e.usable);
    out
}

/// The application named `name` (its `Name=` or its id) among `entries`.
pub fn find_desktop_entry(entries: &[DesktopEntry], name: &str) -> Pick<DesktopEntry> {
    let mut candidates = Vec::new();
    for e in entries {
        candidates.push((e.name.clone(), e.id.clone(), e.clone()));
    }
    match pick(name, &candidates) {
        Pick::None(near) => {
            let by_id: Vec<(String, String, DesktopEntry)> = entries
                .iter()
                .map(|e| (e.id.clone(), e.id.clone(), e.clone()))
                .collect();
            match pick(name, &by_id) {
                Pick::One(e) => Pick::One(e),
                _ => Pick::None(near),
            }
        }
        other => other,
    }
}

/// Where `.desktop` files live: `$XDG_DATA_HOME` (`~/.local/share`), then
/// `$XDG_DATA_DIRS`, Flatpak's and Snap's exports, each with
/// `applications/`.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(v) => bases.push(v.into()),
        None => {
            if let Some(home) = dirs::home_dir() {
                bases.push(home.join(".local/share"));
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".local/share/flatpak/exports/share"));
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    bases.extend(
        system
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from),
    );
    bases.push("/var/lib/flatpak/exports/share".into());
    bases.push("/var/lib/snapd/desktop".into());
    let mut seen = HashSet::new();
    bases
        .into_iter()
        .map(|b| b.join("applications"))
        .filter(|d| seen.insert(d.clone()))
        .collect()
}

/// Whether `s` is a web address `open_url` takes: http or https, one
/// line, no spaces.
pub fn is_url(s: &str) -> bool {
    let low = s.trim().to_ascii_lowercase();
    (low.starts_with("http://") || low.starts_with("https://"))
        && s.len() < 4096
        && !s
            .trim()
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        && s.trim().len() > "https://".len()
}

/// Open a web address in the user's default browser (only http and https
/// addresses, one line).
pub fn open_url(url: &str) -> Result<()> {
    let url = url.trim();
    if !is_url(url) {
        return Err(Error::InvalidArgs(format!("not a web address: {url}")));
    }
    #[cfg(target_os = "windows")]
    {
        // The shell may hand the address to COM objects and DDE: on a
        // thread of its own, with COM set up there, and not waited on
        // forever (a hung shell extension or browser can't block).
        crate::windows::open_url(url)
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(url);
        crate::backend::spawn_detached(cmd).map_err(|e| {
            Error::Platform(format!(
                "couldn't open the browser ({e}); open {url} yourself"
            ))
        })
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut last = None;
        for opener in ["xdg-open", "gio", "sensible-browser", "x-www-browser"] {
            let mut cmd = std::process::Command::new(opener);
            if opener == "gio" {
                cmd.arg("open");
            }
            cmd.arg(url);
            match crate::backend::spawn_detached(cmd) {
                Ok(()) => return Ok(()),
                Err(e) => last = Some(e),
            }
        }
        Err(Error::Platform(format!(
            "couldn't open the browser ({}); open {url} yourself",
            last.map(|e| e.to_string()).unwrap_or_default()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_are_told_from_apps() {
        assert!(is_url("https://www.digikala.com/search/?q=headphones"));
        assert!(is_url("http://127.0.0.1:8080/x"));
        assert!(!is_url("firefox"));
        assert!(!is_url("https://"));
        assert!(!is_url("https://a b"));
        assert!(!is_url("file:///etc/passwd"));
        assert!(!is_url("javascript:alert(1)"));
        assert!(!is_url("https://x\n--flag"));
    }

    #[test]
    fn program_keys_are_file_names_without_extension() {
        assert_eq!(program_key("C:\\Apps\\Calc.EXE"), "calc");
        assert_eq!(program_key("/usr/bin/gedit"), "gedit");
        assert_eq!(program_key("Google Chrome"), "google chrome");
    }

    fn cands(names: &[&str]) -> Vec<(String, String, String)> {
        names
            .iter()
            .map(|n| (n.to_string(), n.to_lowercase(), n.to_string()))
            .collect()
    }

    #[test]
    fn only_an_exact_name_is_picked() {
        let all = cands(&[
            "Google Chrome",
            "Google Chrome Canary",
            "Chrome Remote Desktop",
        ]);
        assert_eq!(
            pick("google chrome", &all),
            Pick::One("Google Chrome".into())
        );
        // A part of a name is never enough: suggestions only.
        assert_eq!(
            pick("chrome", &all),
            Pick::None(vec![
                "Google Chrome".into(),
                "Google Chrome Canary".into(),
                "Chrome Remote Desktop".into()
            ])
        );
        assert_eq!(pick("firefox", &all), Pick::None(vec![]));
        assert_eq!(pick("", &all), Pick::None(vec![]));
        // Two different apps with one name: ask.
        let twins = vec![
            ("Mail".to_string(), "a".to_string(), 1),
            ("Mail".to_string(), "b".to_string(), 2),
        ];
        assert_eq!(pick("mail", &twins), Pick::Many(vec!["Mail".into()]));
        // The same app listed twice is one app; the first wins.
        let same = vec![
            ("Mail".to_string(), "mail".to_string(), 1),
            ("Mail".to_string(), "MAIL".to_string(), 2),
        ];
        assert_eq!(pick("Mail", &same), Pick::One(1));
        let text = not_found("chrome", &pick("chrome", &all), "the Start Menu");
        assert!(text.contains("Similar names: Google Chrome,"), "{text}");
    }

    #[test]
    fn exec_lines_keep_fixed_arguments_only() {
        assert_eq!(
            exec_words(r#""/opt/My App/run" --name "a b" %f %% "#),
            ["/opt/My App/run", "--name", "a b", "%"]
        );
        assert_eq!(
            exec_words("/usr/bin/app --file=%f %U"),
            ["/usr/bin/app", "--file="]
        );
        assert_eq!(
            exec_words(r#"sh -c "echo \"hi\"""#),
            ["sh", "-c", "echo \"hi\""]
        );
        assert!(exec_words("  %U ").is_empty());
    }

    #[test]
    fn desktop_entries_follow_the_spec() {
        let parse = |t: &str| parse_desktop("x", t).unwrap();
        let e = parse(
            "[Desktop Entry]\nType = Application\nName=Gedit\nName[de]=Textbearbeitung\nExec=gedit %U\n[Desktop Action new]\nName=New Window\nExec=gedit --new-window\n",
        );
        assert!(e.usable);
        assert_eq!(
            (e.name.as_str(), e.exec.clone()),
            ("Gedit", vec!["gedit".to_string()])
        );
        assert!(
            !parse("[Desktop Entry]\nType=Application\nName=T\nExec=t\nTerminal=true\n").usable
        );
        assert!(!parse("[Desktop Entry]\nType=Link\nName=T\nURL=http://x\n").usable);
        assert!(
            !parse("[Desktop Entry]\nType=Application\nName=T\nExec=t\nTryExec=/nonexistent/t\n")
                .usable
        );
        assert!(parse_desktop("x", "Name=T\n").is_none());
    }

    #[test]
    fn finds_shortcuts_and_desktop_entries_by_exact_name() {
        let dir = std::env::temp_dir().join(format!("cu-launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let user = dir.join("user");
        let all = dir.join("all");
        std::fs::create_dir_all(user.join("Google Chrome")).unwrap();
        std::fs::create_dir_all(&all).unwrap();
        std::fs::write(user.join("Google Chrome").join("Google Chrome.lnk"), "x").unwrap();
        std::fs::write(all.join("Google Chrome.lnk"), "x").unwrap();
        std::fs::write(all.join("Uninstall Zoom.lnk"), "x").unwrap();
        let shortcuts = start_menu_shortcuts(&[user.clone(), all.clone()]);
        match pick("google chrome", &shortcuts) {
            Pick::One(p) => assert!(p.starts_with(&user), "{p:?}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(pick("zoom", &shortcuts), Pick::None(vec![]));

        let local = dir.join("local");
        let system = dir.join("system");
        std::fs::create_dir_all(local.join("vendor")).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let app = |name: &str, exec: &str| {
            format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n")
        };
        std::fs::write(
            system.join("google-chrome.desktop"),
            app("Google Chrome", "/usr/bin/google-chrome-stable %U"),
        )
        .unwrap();
        std::fs::write(system.join("hidden.desktop"), app("Hidden App", "h")).unwrap();
        // The user's override hides the system entry with the same id.
        std::fs::write(
            local.join("hidden.desktop"),
            "[Desktop Entry]\nType=Application\nName=Hidden App\nExec=h\nHidden=true\n",
        )
        .unwrap();
        std::fs::write(
            local.join("vendor").join("tool.desktop"),
            app("Tool", "tool"),
        )
        .unwrap();
        let entries = desktop_entries(&[local, system]);
        match find_desktop_entry(&entries, "Google Chrome") {
            Pick::One(e) => {
                assert_eq!(e.id, "google-chrome");
                assert_eq!(e.exec, ["/usr/bin/google-chrome-stable"]);
            }
            other => panic!("{other:?}"),
        }
        // By id too, and ids of subfolders use `-`.
        assert!(matches!(
            find_desktop_entry(&entries, "google-chrome"),
            Pick::One(_)
        ));
        assert!(matches!(
            find_desktop_entry(&entries, "vendor-tool"),
            Pick::One(_)
        ));
        assert_eq!(
            find_desktop_entry(&entries, "Hidden App"),
            Pick::None(vec![])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn application_dirs_are_applications_folders() {
        let dirs = application_dirs();
        assert!(!dirs.is_empty());
        assert!(dirs.iter().all(|d| d.ends_with("applications")));
    }
}
