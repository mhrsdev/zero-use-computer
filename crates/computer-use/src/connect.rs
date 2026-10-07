//! Adding this program to the agents people use: Claude Code, Claude
//! Desktop, Codex, Cursor and VS Code. Each keeps its list of MCP servers
//! in a file of its own (Claude Code through its own command); this adds,
//! replaces or removes only the `computer-use` entry in it, keeps the rest
//! of the file as it was, and keeps a copy of the file first.
//!
//! The settings panel's Connect page and `computer-use-mcp install` both
//! use it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::error::{Error, Result};

/// The name the server is registered under.
pub const NAME: &str = "computer-use";
/// The name Claude Code gets when it keeps `computer-use` for itself
/// ("this name is reserved").
pub const ALT_NAME: &str = "zero-use-computer";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    ClaudeDesktop,
    Codex,
    Cursor,
    VsCode,
}

impl Client {
    pub const ALL: [Client; 5] = [
        Client::ClaudeCode,
        Client::ClaudeDesktop,
        Client::Codex,
        Client::Cursor,
        Client::VsCode,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::ClaudeDesktop => "claude-desktop",
            Client::Codex => "codex",
            Client::Cursor => "cursor",
            Client::VsCode => "vscode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Claude Code",
            Client::ClaudeDesktop => "Claude Desktop",
            Client::Codex => "Codex",
            Client::Cursor => "Cursor",
            Client::VsCode => "VS Code (GitHub Copilot)",
        }
    }

    pub fn parse(s: &str) -> Option<Client> {
        let s = s.trim().to_ascii_lowercase();
        Client::ALL
            .into_iter()
            .find(|c| c.id() == s || c.label().to_ascii_lowercase() == s)
            .or(match s.as_str() {
                "claude" => Some(Client::ClaudeCode),
                "code" | "vs-code" => Some(Client::VsCode),
                _ => None,
            })
    }

    /// What to do once it is in.
    pub fn after(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Check it with `claude mcp list`, then /mcp inside a session.",
            Client::ClaudeDesktop => {
                "Quit Claude Desktop and start it again. On macOS give Accessibility and Screen Recording to Claude."
            }
            Client::Codex => {
                "Start Codex again. On macOS give Accessibility and Screen Recording to the app that starts it."
            }
            Client::Cursor => {
                "Restart Cursor, or switch the server on in Settings, MCP. On macOS give Accessibility and Screen Recording to Cursor."
            }
            Client::VsCode => {
                "Restart VS Code, or run MCP: List Servers and start it. On macOS give Accessibility and Screen Recording to VS Code."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

/// Where things are on this computer (replaceable, for tests).
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    /// `%APPDATA%` on Windows.
    pub appdata: Option<PathBuf>,
    pub os: Os,
    /// The folders programs are looked for in.
    pub path: Vec<PathBuf>,
    /// `$CODEX_HOME`, if set.
    pub codex_home: Option<PathBuf>,
    /// The server's own folder (`~/.computer-use`): where a stable copy of
    /// the program lives.
    pub zero_home: PathBuf,
}

impl Env {
    pub fn real() -> Env {
        let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        Env {
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
            os: if cfg!(windows) {
                Os::Windows
            } else if cfg!(target_os = "macos") {
                Os::Mac
            } else {
                Os::Linux
            },
            path: std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
            codex_home: std::env::var_os("CODEX_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            zero_home: crate::config::home_dir(),
            home,
        }
    }

    /// A program on the path.
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        let names: Vec<String> = if self.os == Os::Windows {
            ["exe", "cmd", "bat"]
                .iter()
                .map(|e| format!("{name}.{e}"))
                .collect()
        } else {
            vec![name.to_string()]
        };
        self.path
            .iter()
            .flat_map(|d| names.iter().map(move |n| d.join(n)))
            .find(|p| p.is_file())
    }

    fn app_support(&self) -> PathBuf {
        match self.os {
            Os::Mac => self.home.join("Library/Application Support"),
            Os::Windows => self
                .appdata
                .clone()
                .unwrap_or_else(|| self.home.join("AppData/Roaming")),
            Os::Linux => self.home.join(".config"),
        }
    }

    /// The file a client keeps its servers in (none: it has its own command).
    pub fn config_file(&self, c: Client) -> Option<PathBuf> {
        Some(match c {
            Client::ClaudeCode => return None,
            Client::ClaudeDesktop => match self.os {
                Os::Linux => return None, // no Claude Desktop from Anthropic for Linux
                _ => self.app_support().join("Claude/claude_desktop_config.json"),
            },
            Client::Codex => self
                .codex_home
                .clone()
                .unwrap_or_else(|| self.home.join(".codex"))
                .join("config.toml"),
            Client::Cursor => self.home.join(".cursor/mcp.json"),
            Client::VsCode => self.app_support().join("Code/User/mcp.json"),
        })
    }

    /// Whether the client looks installed here.
    fn found(&self, c: Client) -> bool {
        match c {
            Client::ClaudeCode => self.which("claude").is_some(),
            Client::ClaudeDesktop => self
                .config_file(c)
                .and_then(|f| f.parent().map(Path::exists))
                .unwrap_or(false),
            Client::Codex => {
                self.which("codex").is_some()
                    || self
                        .config_file(c)
                        .and_then(|f| f.parent().map(Path::exists))
                        .unwrap_or(false)
            }
            Client::Cursor => self.home.join(".cursor").exists(),
            Client::VsCode => self
                .config_file(c)
                .and_then(|f| f.parent().map(Path::exists))
                .unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The program of the client isn't here.
    NotFound,
    /// The client is here; this program isn't in it.
    NotInstalled,
    Installed,
    /// In it, but pointing at another program.
    Different(String),
    /// Its file can't be read or changed safely.
    Unreadable(String),
}

#[derive(Debug, Clone)]
pub struct Info {
    pub client: Client,
    pub state: State,
    /// The file it is kept in, or how (`claude mcp add`).
    pub where_: String,
}

fn read(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t.strip_prefix('\u{feff}').unwrap_or(&t).to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

// ---- JSON files, in their own order ----------------------------------------------

/// An object's members in the order they were written, each as its own text.
struct Pairs(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Pairs {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Pairs;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut m: A,
            ) -> std::result::Result<Pairs, A::Error> {
                let mut out = Vec::new();
                while let Some(kv) = m.next_entry::<String, Box<RawValue>>()? {
                    out.push(kv);
                }
                Ok(Pairs(out))
            }
        }
        d.deserialize_map(V)
    }
}

fn parse_pairs(text: &str) -> std::result::Result<Pairs, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

fn indent(text: &str, by: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.to_string()
            } else {
                format!("{by}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_pairs(pairs: &Pairs, level: &str) -> String {
    if pairs.0.is_empty() {
        return "{}".into();
    }
    let inner = format!("{level}  ");
    let body: Vec<String> = pairs
        .0
        .iter()
        .map(|(k, v)| {
            format!(
                "{inner}{}: {}",
                serde_json::to_string(k).unwrap_or_default(),
                v.get()
            )
        })
        .collect();
    format!("{{\n{}\n{level}}}", body.join(",\n"))
}

/// `text` (a JSON file, or nothing) with `servers.NAME` set to `entry`, or
/// removed with `None`; everything else as it was, in its order.
fn edit_json(
    text: Option<&str>,
    servers: &str,
    entry: Option<&str>,
) -> std::result::Result<String, String> {
    let text = text.filter(|t| !t.trim().is_empty()).unwrap_or("{}");
    let mut top = parse_pairs(text)
        .map_err(|e| format!("it isn't plain JSON (comments aren't allowed here): {e}"))?;
    let at = top.0.iter().position(|(k, _)| k == servers);
    let mut inner = match at {
        Some(i) => parse_pairs(top.0[i].1.get())
            .map_err(|e| format!("`{servers}` isn't an object: {e}"))?,
        None => Pairs(Vec::new()),
    };
    let existing = inner.0.iter().position(|(k, _)| k == NAME);
    match (entry, existing) {
        (Some(v), found) => {
            let raw = RawValue::from_string(indent(v, "    ")).map_err(|e| e.to_string())?;
            match found {
                Some(i) => inner.0[i].1 = raw,
                None => inner.0.push((NAME.into(), raw)),
            }
        }
        (None, Some(i)) => {
            inner.0.remove(i);
        }
        (None, None) => {}
    }
    let raw = RawValue::from_string(write_pairs(&inner, "  ")).map_err(|e| e.to_string())?;
    match at {
        Some(i) => top.0[i].1 = raw,
        None => top.0.push((servers.into(), raw)),
    }
    let out = format!("{}\n", write_pairs(&top, ""));
    serde_json::from_str::<Value>(&out).map_err(|e| format!("the result wouldn't be JSON: {e}"))?;
    Ok(out)
}

/// The entry as text, its members in the order people write them.
fn json_entry(c: Client, program: &Path) -> String {
    let command = serde_json::to_string(&program.display().to_string()).unwrap_or_default();
    let kind = if c == Client::VsCode {
        "  \"type\": \"stdio\",\n"
    } else {
        ""
    };
    format!("{{\n{kind}  \"command\": {command},\n  \"args\": [\n    \"serve\"\n  ]\n}}")
}

fn servers_key(c: Client) -> &'static str {
    if c == Client::VsCode {
        "servers"
    } else {
        "mcpServers"
    }
}

fn registered_json(text: &str, servers: &str) -> std::result::Result<Option<Value>, String> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| format!("it isn't plain JSON (comments aren't allowed here): {e}"))?;
    Ok(v.get(servers).and_then(|s| s.get(NAME)).cloned())
}

// ---- Codex's TOML -----------------------------------------------------------------

fn edit_toml(text: Option<&str>, program: Option<&Path>) -> std::result::Result<String, String> {
    let mut doc: toml_edit::DocumentMut = text
        .unwrap_or_default()
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    match program {
        Some(p) => {
            let servers = doc
                .entry("mcp_servers")
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_like_mut()
                .ok_or("`mcp_servers` isn't a table")?;
            let mut t = toml_edit::Table::new();
            t.insert("command", toml_edit::value(p.display().to_string()));
            let mut args = toml_edit::Array::new();
            args.push("serve");
            t.insert("args", toml_edit::value(args));
            servers.insert(NAME, toml_edit::Item::Table(t));
        }
        None => {
            if let Some(s) = doc
                .get_mut("mcp_servers")
                .and_then(|i| i.as_table_like_mut())
            {
                s.remove(NAME);
            }
        }
    }
    Ok(doc.to_string())
}

fn registered_toml(text: &str) -> std::result::Result<Option<String>, String> {
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    Ok(doc
        .get("mcp_servers")
        .and_then(|s| s.get(NAME))
        .and_then(|e| e.get("command"))
        .and_then(|c| c.as_str())
        .map(str::to_string))
}

// ---- what is there ----------------------------------------------------------------

fn same_program(a: &str, b: &Path) -> bool {
    let (a, b) = (Path::new(a), b);
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(x, y)| x == y)
}

/// How each client stands.
pub fn detect(env: &Env, program: &Path) -> Vec<Info> {
    Client::ALL
        .into_iter()
        .map(|c| info(env, c, program))
        .collect()
}

pub fn info(env: &Env, c: Client, program: &Path) -> Info {
    let where_ = match (c, env.config_file(c)) {
        (_, Some(f)) => f.display().to_string(),
        (Client::ClaudeCode, None) => "claude mcp add --scope user".to_string(),
        _ => "not available on this system".to_string(),
    };
    let found = env.found(c);
    if c == Client::ClaudeDesktop && env.os == Os::Linux {
        return Info {
            client: c,
            state: State::NotFound,
            where_,
        };
    }
    let state = (|| -> std::result::Result<State, String> {
        let registered: Option<String> = match c {
            Client::ClaudeCode => {
                let file = env.home.join(".claude.json");
                match read(&file).map_err(|e| format!("{}: {e}", file.display()))? {
                    // A big file with a lot of other things in it: only its
                    // user-level servers are looked at.
                    Some(t) => [NAME, ALT_NAME].iter().find_map(|n| {
                        let v: Value = serde_json::from_str(&t).ok()?;
                        let e = v.get("mcpServers")?.get(*n)?;
                        e.get("command").and_then(Value::as_str).map(str::to_string)
                    }),
                    None => None,
                }
            }
            Client::Codex => {
                let file = env.config_file(c).unwrap_or_default();
                match read(&file).map_err(|e| format!("{}: {e}", file.display()))? {
                    Some(t) => registered_toml(&t)?,
                    None => None,
                }
            }
            _ => {
                let file = env.config_file(c).unwrap_or_default();
                match read(&file).map_err(|e| format!("{}: {e}", file.display()))? {
                    Some(t) if !t.trim().is_empty() => registered_json(&t, servers_key(c))?
                        .and_then(|e| e.get("command").and_then(Value::as_str).map(str::to_string)),
                    _ => None,
                }
            }
        };
        Ok(match registered {
            Some(p) if same_program(&p, program) => State::Installed,
            Some(p) => State::Different(p),
            None if found => State::NotInstalled,
            None => State::NotFound,
        })
    })()
    .unwrap_or_else(State::Unreadable);
    Info {
        client: c,
        state,
        where_,
    }
}

/// What would be written, as the user would read it.
pub fn entry_text(c: Client, program: &Path) -> String {
    match c {
        Client::ClaudeCode => format!(
            "claude mcp add --scope user {NAME} -- \"{}\" serve",
            program.display()
        ),
        Client::Codex => format!(
            "[mcp_servers.{NAME}]\ncommand = {}\nargs = [\"serve\"]",
            toml::Value::String(program.display().to_string())
        ),
        _ => format!(
            "{{\n  \"{}\": {{\n    \"{NAME}\": {}\n  }}\n}}",
            servers_key(c),
            indent(&json_entry(c, program), "    ")
        ),
    }
}

// ---- the program, somewhere that stays ---------------------------------------------

/// Where a copy of the program is kept for clients to start: the same place
/// the Claude Code installers use.
pub fn stable_program_path(env: &Env) -> PathBuf {
    env.zero_home.join("bin").join(if env.os == Os::Windows {
        "computer-use-mcp.exe"
    } else {
        "computer-use-mcp"
    })
}

/// The path to register for the program at `exe`: itself, when it lives in
/// the server's folder, else a copy made there (put in place as a new file,
/// never written over: a signed program changed under a running copy gets
/// killed on a Mac). If the copy can't be made, `exe` itself.
pub fn program_for_clients(env: &Env, exe: &Path) -> (PathBuf, Option<String>) {
    let stable = stable_program_path(env);
    if same_program(&stable.display().to_string(), exe) {
        return (exe.to_path_buf(), None);
    }
    let tmp = stable.with_extension(format!("new.{}", std::process::id()));
    let made = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(stable.parent().unwrap_or(Path::new(".")))?;
        std::fs::copy(exe, &tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, &stable)
    })();
    match made {
        Ok(()) => (
            stable.clone(),
            Some(format!(
                "A copy of the program is kept at {}, so this folder can be moved or deleted.",
                stable.display()
            )),
        ),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            (
                exe.to_path_buf(),
                Some(format!(
                    "Couldn't keep a copy in {} ({e}); the program is registered where it is, so don't move it.",
                    stable.display()
                )),
            )
        }
    }
}

/// Whether a program's path is somewhere that is cleaned out or moved.
pub fn unstable_place(program: &Path) -> bool {
    let s = program.display().to_string().to_ascii_lowercase();
    [
        "/tmp/",
        "\\temp\\",
        "/downloads/",
        "\\downloads\\",
        "/var/folders/",
        "/appdata/local/temp",
    ]
    .iter()
    .any(|m| s.contains(m))
}

// ---- doing it ------------------------------------------------------------------------

fn backup(path: &Path) {
    if path.is_file() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        let _ = std::fs::copy(path, PathBuf::from(bak));
    }
}

fn write_file(path: &Path, text: &str) -> Result<()> {
    backup(path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::Platform(format!("{}: {e}", dir.display())))?;
    }
    crate::config::write_atomic(path, text, false)
}

fn run_claude(env: &Env, args: &[&str]) -> Result<String> {
    let exe = env.which("claude").ok_or_else(|| {
        Error::Platform("Claude Code (the `claude` command) isn't on the path".into())
    })?;
    let out = Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::Platform(format!("can't run claude: {e}")))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(Error::Platform(format!(
            "claude said: {}",
            text.trim().chars().take(300).collect::<String>()
        )))
    }
}

/// Add (or replace) the entry for `program` in client `c`.
pub fn install(env: &Env, c: Client, program: &Path) -> Result<String> {
    let bad = |m: String| Error::Platform(m);
    match c {
        Client::ClaudeCode => {
            // Replace, as the installers do: an old one pointing elsewhere goes.
            let _ = run_claude(env, &["mcp", "remove", NAME, "--scope", "user"]);
            run_claude(
                env,
                &[
                    "mcp",
                    "add",
                    "--scope",
                    "user",
                    NAME,
                    "--",
                    &program.display().to_string(),
                    "serve",
                ],
            )?;
        }
        Client::Codex => {
            let file = env
                .config_file(c)
                .ok_or_else(|| bad("no settings file".into()))?;
            let old = read(&file).map_err(|e| bad(format!("{}: {e}", file.display())))?;
            let text = edit_toml(old.as_deref(), Some(program))
                .map_err(|e| bad(format!("{}: {e}", file.display())))?;
            write_file(&file, &text)?;
        }
        _ => {
            let file = env
                .config_file(c)
                .ok_or_else(|| bad(format!("{} isn't available on this system", c.label())))?;
            let old = read(&file).map_err(|e| bad(format!("{}: {e}", file.display())))?;
            let text = edit_json(
                old.as_deref(),
                servers_key(c),
                Some(&json_entry(c, program)),
            )
            .map_err(|e| bad(format!("{}: {e}", file.display())))?;
            write_file(&file, &text)?;
        }
    }
    Ok(format!("Added to {}. {}", c.label(), c.after()))
}

/// Take the entry out of client `c`.
pub fn remove(env: &Env, c: Client) -> Result<String> {
    let bad = |m: String| Error::Platform(m);
    match c {
        Client::ClaudeCode => {
            let a = run_claude(env, &["mcp", "remove", NAME, "--scope", "user"]);
            let b = run_claude(env, &["mcp", "remove", ALT_NAME, "--scope", "user"]);
            if a.is_err() && b.is_err() {
                return Err(a.unwrap_err());
            }
        }
        Client::Codex => {
            let file = env
                .config_file(c)
                .ok_or_else(|| bad("no settings file".into()))?;
            let Some(old) = read(&file).map_err(|e| bad(format!("{}: {e}", file.display())))?
            else {
                return Ok("Nothing to remove.".into());
            };
            let text =
                edit_toml(Some(&old), None).map_err(|e| bad(format!("{}: {e}", file.display())))?;
            write_file(&file, &text)?;
        }
        _ => {
            let file = env
                .config_file(c)
                .ok_or_else(|| bad(format!("{} isn't available on this system", c.label())))?;
            let Some(old) = read(&file).map_err(|e| bad(format!("{}: {e}", file.display())))?
            else {
                return Ok("Nothing to remove.".into());
            };
            let text = edit_json(Some(&old), servers_key(c), None)
                .map_err(|e| bad(format!("{}: {e}", file.display())))?;
            write_file(&file, &text)?;
        }
    }
    Ok(format!(
        "Removed from {}. Restart it to let go of the server.",
        c.label()
    ))
}

#[cfg(test)]
mod tests;
