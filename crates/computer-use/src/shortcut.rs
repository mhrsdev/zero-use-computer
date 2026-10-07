//! A shortcut that opens the settings panel: an icon on the desktop, and an
//! entry in the system's app menu (the Start menu, ~/Applications, the
//! desktop's list of apps). It starts `computer-use-mcp settings`, which
//! serves the panel on its own port (`panel.port`) and opens it in the
//! browser, or shows the one already open.
//!
//! The settings panel's Home page and `computer-use-mcp shortcut` make and
//! remove them. Only shortcuts this program made are ever removed: each
//! carries a mark.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::connect::Os;
use crate::error::{Error, Result};

/// What a shortcut is called.
pub const TITLE: &str = "Zero panel";
/// The mark in every shortcut this program makes.
const MARK: &str = "X-Zero-Use-Computer";
/// The panel's icon (the orbit pointer).
const ICON_PNG: &[u8] = include_bytes!("../assets/cursors/orbit.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// An icon on the desktop.
    Desktop,
    /// An entry with the system's apps.
    Menu,
}

impl Place {
    pub const ALL: [Place; 2] = [Place::Desktop, Place::Menu];

    pub fn id(self) -> &'static str {
        match self {
            Place::Desktop => "desktop",
            Place::Menu => "menu",
        }
    }

    pub fn parse(s: &str) -> Option<Place> {
        match s.trim().to_ascii_lowercase().as_str() {
            "desktop" => Some(Place::Desktop),
            "menu" | "apps" | "start" | "start-menu" | "applications" => Some(Place::Menu),
            _ => None,
        }
    }

    pub fn label(self, os: Os) -> &'static str {
        match (self, os) {
            (Place::Desktop, _) => "Desktop",
            (Place::Menu, Os::Windows) => "Start menu",
            (Place::Menu, Os::Mac) => "Applications",
            (Place::Menu, Os::Linux) => "App menu",
        }
    }
}

/// Where shortcuts go on this computer (replaceable, for tests).
#[derive(Debug, Clone)]
pub struct Places {
    pub os: Os,
    pub desktop: Option<PathBuf>,
    pub menu: Option<PathBuf>,
    /// Where the icon is kept (the server's folder).
    pub icons: PathBuf,
}

impl Places {
    pub fn real() -> Places {
        let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        };
        let menu = match os {
            Os::Windows => std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .map(|a| a.join("Microsoft/Windows/Start Menu/Programs")),
            Os::Mac => Some(home.join("Applications")),
            Os::Linux => Some(
                std::env::var_os("XDG_DATA_HOME")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("applications"),
            ),
        };
        Places {
            os,
            // The system's own idea of the desktop (a Windows desktop moved
            // to OneDrive, a Linux one in another language).
            desktop: dirs::desktop_dir().or_else(|| Some(home.join("Desktop"))),
            menu,
            icons: crate::config::home_dir().join("icons"),
        }
    }

    fn dir(&self, place: Place) -> Option<&Path> {
        match place {
            Place::Desktop => self.desktop.as_deref(),
            Place::Menu => self.menu.as_deref(),
        }
    }

    /// The shortcut's file (a folder, for a Mac app).
    pub fn path(&self, place: Place) -> Option<PathBuf> {
        let dir = self.dir(place)?;
        Some(match self.os {
            Os::Linux => dir.join("zero-panel.desktop"),
            Os::Mac => dir.join(format!("{TITLE}.app")),
            Os::Windows => dir.join(format!("{TITLE}.lnk")),
        })
    }
}

/// A shortcut as it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub place: Place,
    /// Where it is (or would be); none when this system has no such place.
    pub path: Option<PathBuf>,
    pub exists: bool,
    /// The program it starts, when that can be read from it.
    pub starts: Option<String>,
    /// Made by this program (it carries the mark).
    pub ours: bool,
}

pub fn status(p: &Places) -> Vec<Status> {
    Place::ALL
        .into_iter()
        .map(|place| {
            let path = p.path(place);
            let exists = path.as_ref().is_some_and(|f| f.exists());
            let (starts, ours) = match (&path, exists) {
                (Some(f), true) => read_back(p.os, f),
                _ => (None, false),
            };
            Status {
                place,
                path,
                exists,
                starts,
                ours,
            }
        })
        .collect()
}

/// The program a shortcut starts, and whether it is ours.
fn read_back(os: Os, f: &Path) -> (Option<String>, bool) {
    match os {
        Os::Linux => {
            let text = std::fs::read_to_string(f).unwrap_or_default();
            let ours = text.lines().any(|l| l.trim() == format!("{MARK}=true"));
            let exec = text
                .lines()
                .find_map(|l| l.strip_prefix("Exec="))
                .map(unquote_exec);
            (exec, ours)
        }
        Os::Mac => {
            let script =
                std::fs::read_to_string(f.join("Contents/MacOS/zero-panel")).unwrap_or_default();
            let ours = script.contains(MARK);
            let exec = script
                .lines()
                .find_map(|l| l.strip_prefix("prog="))
                .map(unquote_sh);
            (exec, ours)
        }
        // A .lnk is binary: it is ours by its name.
        Os::Windows => (None, true),
    }
}

// ---- making one ---------------------------------------------------------------------

/// A value for a .desktop file's Exec line. The format quotes an argument
/// in double quotes with `"`, `` ` ``, `$` and `\` escaped by a backslash,
/// and then reads the whole line as a string in which a backslash is
/// escaped again: `$` is written `\\$` in the file, a backslash `\\\\`.
fn quote_exec(arg: &str) -> String {
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

/// The program an Exec line starts (its first argument), read back the
/// way a desktop reads it.
fn unquote_exec(line: &str) -> String {
    // The string level first: `\\` is one backslash (and \s \n \t \r).
    let mut text = String::new();
    let mut chars = line.trim().chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => text.push(' '),
                Some('n') => text.push('\n'),
                Some('t') => text.push('\t'),
                Some('r') => text.push('\r'),
                Some(o) => text.push(o),
                None => {}
            }
        } else {
            text.push(c);
        }
    }
    let Some(rest) = text.strip_prefix('"') else {
        return text
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .replace("%%", "%");
    };
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '"' => break,
            '%' => {
                out.push('%');
                chars.next();
            }
            c => out.push(c),
        }
    }
    out
}

/// A value written by [`quote_sh`], read back.
fn unquote_sh(q: &str) -> String {
    q.strip_prefix('\'')
        .and_then(|r| r.strip_suffix('\''))
        .unwrap_or(q)
        .replace("'\\''", "'")
}

/// A path for a POSIX shell, in single quotes.
fn quote_sh(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn desktop_file(program: &Path, icon: &Path) -> String {
    let line = |s: String| s.replace(['\n', '\r'], " ");
    format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName={TITLE}\nComment=Settings for Zero Use Computer\nExec={} settings\nIcon={}\nTerminal=false\nCategories=Settings;Utility;\nStartupNotify=false\n{MARK}=true\n",
        line(quote_exec(&program.display().to_string())),
        line(icon.display().to_string()),
    )
}

fn mac_script(program: &Path) -> String {
    // Started in the background, and the app is done: a second click starts
    // it again, which shows the panel already open (a running app would
    // only be brought forward, with nothing to show).
    format!(
        "#!/bin/sh\n# {MARK}: opens the settings panel of Zero Use Computer\nprog={}\nnohup \"$prog\" settings >/dev/null 2>&1 &\n",
        quote_sh(&program.display().to_string())
    )
}

fn mac_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>zero-panel</string>
  <key>CFBundleIdentifier</key><string>dev.mhrs.zero-panel</string>
  <key>CFBundleName</key><string>{TITLE}</string>
  <key>CFBundleDisplayName</key><string>{TITLE}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>{}</string>
  <key>LSUIElement</key><true/>
</dict>
</plist>
"#,
        env!("CARGO_PKG_VERSION")
    )
}

/// The PowerShell that makes the Windows shortcut. The paths come in
/// environment variables, never in the script's text: a name with a quote
/// in it (O’Brien) can't end a string and run as code.
pub const WINDOWS_SCRIPT: &str = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:ZERO_PANEL_LNK); $s.TargetPath = $env:ZERO_PANEL_PROGRAM; $s.Arguments = 'settings'; $s.WindowStyle = 7; $s.IconLocation = $env:ZERO_PANEL_ICON; $s.Description = 'Settings for Zero Use Computer'; $s.Save()";

/// An .ico holding the PNG as it is (Windows reads PNG inside an icon).
fn ico_from_png(png: &[u8]) -> Vec<u8> {
    let dim = |at: usize| {
        let v = u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
        if v >= 256 { 0u8 } else { v as u8 }
    };
    let mut out = vec![0, 0, 1, 0, 1, 0];
    out.extend([dim(16), dim(20), 0, 0, 1, 0, 32, 0]);
    out.extend((png.len() as u32).to_le_bytes());
    out.extend(22u32.to_le_bytes());
    out.extend_from_slice(png);
    out
}

/// The icon file for this system, written into the server's folder.
fn icon(p: &Places) -> Result<PathBuf> {
    let io = |e: std::io::Error| Error::Platform(format!("{}: {e}", p.icons.display()));
    std::fs::create_dir_all(&p.icons).map_err(io)?;
    let (name, bytes) = match p.os {
        Os::Windows => ("zero-panel.ico", ico_from_png(ICON_PNG)),
        _ => ("zero-panel.png", ICON_PNG.to_vec()),
    };
    let path = p.icons.join(name);
    std::fs::write(&path, bytes).map_err(io)?;
    Ok(path)
}

/// Make (or make again) the shortcut at `place` that starts `program`.
pub fn create(p: &Places, place: Place, program: &Path) -> Result<PathBuf> {
    let path = p
        .path(place)
        .ok_or_else(|| Error::Platform(format!("this system has no {}", place.label(p.os))))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let io = |e: std::io::Error| Error::Platform(format!("{}: {e}", path.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    if path.exists() && !status(p).iter().any(|s| s.place == place && s.ours) {
        return Err(Error::Platform(format!(
            "{} is there already and wasn't made by this program: it is left alone",
            path.display()
        )));
    }
    let icon = icon(p)?;
    match p.os {
        Os::Linux => {
            std::fs::write(&path, desktop_file(program, &icon)).map_err(io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .map_err(io)?;
            }
            // GNOME runs a desktop file only once it is trusted.
            if place == Place::Desktop {
                let _ = Command::new("gio")
                    .args([
                        "set",
                        &path.display().to_string(),
                        "metadata::trusted",
                        "true",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        Os::Mac => {
            let contents = path.join("Contents");
            std::fs::create_dir_all(contents.join("MacOS")).map_err(io)?;
            std::fs::create_dir_all(contents.join("Resources")).map_err(io)?;
            std::fs::write(contents.join("Info.plist"), mac_plist()).map_err(io)?;
            std::fs::write(contents.join("Resources/zero-panel.png"), ICON_PNG).map_err(io)?;
            let script = contents.join("MacOS/zero-panel");
            std::fs::write(&script, mac_script(program)).map_err(io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                    .map_err(io)?;
            }
        }
        Os::Windows => {
            let out = Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT])
                .env("ZERO_PANEL_LNK", &path)
                .env("ZERO_PANEL_PROGRAM", program)
                .env("ZERO_PANEL_ICON", &icon)
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| Error::Platform(format!("can't run PowerShell: {e}")))?;
            if !out.status.success() || !path.exists() {
                return Err(Error::Platform(format!(
                    "PowerShell couldn't make the shortcut: {}",
                    String::from_utf8_lossy(&out.stderr)
                        .trim()
                        .chars()
                        .take(300)
                        .collect::<String>()
                )));
            }
        }
    }
    Ok(path)
}

/// Take the shortcut at `place` away, if this program made it. Whether
/// there was one.
pub fn remove(p: &Places, place: Place) -> Result<bool> {
    let Some(s) = status(p).into_iter().find(|s| s.place == place) else {
        return Ok(false);
    };
    let (Some(path), true) = (s.path, s.exists) else {
        return Ok(false);
    };
    if !s.ours {
        return Err(Error::Platform(format!(
            "{} wasn't made by this program: it is left alone",
            path.display()
        )));
    }
    let done = if path.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };
    done.map_err(|e| Error::Platform(format!("{}: {e}", path.display())))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(name: &str, os: Os) -> (PathBuf, Places) {
        let d = std::env::temp_dir().join(format!("cu-shortcut-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = Places {
            os,
            desktop: Some(d.join("Desktop")),
            menu: Some(d.join("menu")),
            icons: d.join("home/icons"),
        };
        (d, p)
    }

    #[test]
    fn a_linux_shortcut_is_made_read_back_and_taken_away() {
        let (d, p) = places("linux", Os::Linux);
        let program = Path::new("/home/me/.computer-use/bin/computer-use-mcp");
        assert!(status(&p).iter().all(|s| !s.exists));
        let made = create(&p, Place::Desktop, program).unwrap();
        assert_eq!(made, d.join("Desktop/zero-panel.desktop"));
        let text = std::fs::read_to_string(&made).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n"), "{text}");
        assert!(
            text.contains("Exec=\"/home/me/.computer-use/bin/computer-use-mcp\" settings\n"),
            "{text}"
        );
        assert!(text.contains("Terminal=false") && text.contains(&format!("{MARK}=true")));
        let icon = p.icons.join("zero-panel.png");
        assert!(text.contains(&format!("Icon={}", icon.display())));
        assert_eq!(std::fs::read(&icon).unwrap(), ICON_PNG);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&made).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
        let s = status(&p);
        assert!(s[0].exists && s[0].ours);
        assert_eq!(
            s[0].starts.as_deref(),
            Some("/home/me/.computer-use/bin/computer-use-mcp")
        );
        assert!(!s[1].exists);
        // Made again (another program): replaced.
        create(&p, Place::Desktop, Path::new("/opt/z/computer-use-mcp")).unwrap();
        assert_eq!(
            status(&p)[0].starts.as_deref(),
            Some("/opt/z/computer-use-mcp")
        );
        // The menu too, and away again.
        create(&p, Place::Menu, program).unwrap();
        assert!(d.join("menu/zero-panel.desktop").is_file());
        assert!(remove(&p, Place::Menu).unwrap());
        assert!(!remove(&p, Place::Menu).unwrap());
        assert!(remove(&p, Place::Desktop).unwrap());
        assert!(status(&p).iter().all(|s| !s.exists));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_of_the_same_name_that_isnt_ours_is_left_alone() {
        let (d, p) = places("theirs", Os::Linux);
        std::fs::create_dir_all(d.join("Desktop")).unwrap();
        let theirs = "[Desktop Entry]\nName=Mine\nExec=something\n";
        std::fs::write(d.join("Desktop/zero-panel.desktop"), theirs).unwrap();
        assert!(!status(&p)[0].ours);
        assert!(create(&p, Place::Desktop, Path::new("/p")).is_err());
        assert!(remove(&p, Place::Desktop).is_err());
        assert_eq!(
            std::fs::read_to_string(d.join("Desktop/zero-panel.desktop")).unwrap(),
            theirs
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn odd_paths_survive_every_format() {
        // The form the desktop entry spec asks for: escaped for the quotes,
        // then every backslash doubled for the string level.
        assert_eq!(quote_exec("/a$b"), r#""/a\\$b""#);
        assert_eq!(quote_exec(r"/a\b"), r#""/a\\\\b""#);
        assert_eq!(quote_exec("/a 100%"), r#""/a 100%%""#);
        let odd = r#"/home/a "b" $c `d` 100%\e's/computer-use-mcp"#;
        let q = quote_exec(odd);
        assert_eq!(unquote_exec(&format!("{q} settings")), odd);
        assert!(!q.contains('\n'));
        let sh = mac_script(Path::new(odd));
        let line = sh.lines().find(|l| l.starts_with("prog=")).unwrap();
        assert_eq!(line, format!("prog={}", quote_sh(odd)));
        assert_eq!(unquote_sh(&quote_sh(odd)), odd);
        assert!(sh.contains("nohup \"$prog\" settings >/dev/null 2>&1 &"));
        assert_eq!(quote_sh("it's"), r"'it'\''s'");
        // The Windows script carries no path at all.
        assert!(
            WINDOWS_SCRIPT.contains("$env:ZERO_PANEL_PROGRAM") && !WINDOWS_SCRIPT.contains(":\\")
        );
        // A newline in a path can't add a line to a desktop file.
        let f = desktop_file(Path::new("/a\nExec=evil"), Path::new("/i"));
        assert_eq!(f.lines().filter(|l| l.starts_with("Exec=")).count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_mac_shortcut_is_a_small_app() {
        let (d, p) = places("mac", Os::Mac);
        let program = Path::new("/Users/me/.computer-use/bin/computer-use-mcp");
        let made = create(&p, Place::Menu, program).unwrap();
        assert_eq!(made, d.join("menu/Zero panel.app"));
        let plist = std::fs::read_to_string(made.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>zero-panel</string>") && plist.contains("LSUIElement"));
        let s = status(&p);
        assert!(s[1].exists && s[1].ours);
        assert_eq!(
            s[1].starts.as_deref(),
            Some("/Users/me/.computer-use/bin/computer-use-mcp")
        );
        assert!(remove(&p, Place::Menu).unwrap());
        assert!(!made.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_windows_icon_holds_the_picture() {
        let ico = ico_from_png(ICON_PNG);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!((ico[6], ico[7]), (224, 242));
        assert_eq!(&ico[22..], ICON_PNG);
        assert_eq!(
            u32::from_le_bytes(ico[14..18].try_into().unwrap()) as usize,
            ICON_PNG.len()
        );
        let (_, p) = places("win", Os::Windows);
        assert!(p.path(Place::Desktop).unwrap().ends_with("Zero panel.lnk"));
        assert_eq!(Place::Menu.label(Os::Windows), "Start menu");
        assert_eq!(Place::parse("start-menu"), Some(Place::Menu));
    }
}
