//! What scripts reach outside the server: files (as `[script] files`
//! allows), the web (through `curl`, which every supported system has),
//! the memory kept between runs, and CSV text.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::{Map, Value};

use super::Env;
use crate::config::ScriptFiles;

/// Largest file a script reads, and largest it writes.
pub const MAX_READ: u64 = 32 * 1024 * 1024;
pub const MAX_WRITE: usize = 64 * 1024 * 1024;
/// Largest answer `fetch` takes.
const MAX_FETCH: u64 = 32 * 1024 * 1024;
/// Largest memory file.
const MAX_MEMORY: usize = 4 * 1024 * 1024;

/// The file a script means by `path`, if it may use it. Relative paths are
/// in the scripts' own folder; `~` is the home folder.
pub fn resolve(env: &Env, path: &str, write: bool) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("give a file path".into());
    }
    if env.files == ScriptFiles::None {
        return Err("scripts may not use files here ([script] files = \"none\")".into());
    }
    let ws = env.workspace();
    let expanded = match path.strip_prefix("~/").or(path.strip_prefix("~\\")) {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    };
    let checked = |p: PathBuf| -> Result<PathBuf, String> {
        if real(&p).is_none() {
            Err(format!(
                "\"{path}\" goes up (..) out of a folder that doesn't exist: give the path without .."
            ))
        } else if private(env, &p) {
            Err(format!(
                "\"{path}\" is the server's own (its settings and keys): scripts never use it"
            ))
        } else {
            Ok(p)
        }
    };
    let up = expanded.components().any(|c| c == Component::ParentDir);
    // Relative only in name on Windows: `\Windows\x` (rooted) and `C:x`
    // (a drive's current folder) would leave the scripts' folder when
    // joined to it, so they follow the rules for absolute paths.
    let plain = expanded.components().all(|c| {
        matches!(
            c,
            Component::Normal(_) | Component::CurDir | Component::ParentDir
        )
    });
    if expanded.is_relative() && plain {
        if up {
            return Err(format!(
                "\"{path}\" leaves the scripts' folder: relative paths stay in {}",
                ws.display()
            ));
        }
        return checked(ws.join(expanded));
    }
    let inside = !up && expanded.starts_with(&ws);
    let allowed = inside
        || match env.files {
            ScriptFiles::All => true,
            ScriptFiles::Read => !write,
            ScriptFiles::Workspace | ScriptFiles::None => false,
        };
    if allowed {
        return checked(expanded);
    }
    Err(if write {
        format!(
            "scripts may write only in their own folder, {} (give a relative path); writing elsewhere needs [script] files = \"all\" in the user's settings",
            ws.display()
        )
    } else {
        format!(
            "scripts may read only their own folder, {} ([script] files = \"workspace\")",
            ws.display()
        )
    })
}

/// Where a path really leads: its deepest existing folder resolved (links,
/// `..`), the rest as written. None when a `..` comes after a folder that
/// doesn't exist yet: writing makes that folder, and the `..` then leads
/// wherever it climbs to, not where the path seemed to stay.
fn real(p: &Path) -> Option<PathBuf> {
    let mut base = p.to_path_buf();
    let mut rest = Vec::new();
    while !base.as_os_str().is_empty() {
        if let Ok(c) = base.canonicalize() {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        match (base.file_name().map(|n| n.to_os_string()), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                base = parent.to_path_buf();
            }
            // A name ending in `..` has none: it is still to be resolved.
            _ => break,
        }
    }
    if p.components().any(|c| c == Component::ParentDir) {
        None
    } else {
        Some(p.to_path_buf())
    }
}

/// Whether `p` is the server's own: its folder (but the scripts' files in
/// it), its settings file, or, on Linux, a process's own details in /proc
/// (its environment holds the key `api_key_env` names).
fn private(env: &Env, p: &Path) -> bool {
    let Some(p) = real(p) else {
        return true;
    };
    if cfg!(target_os = "linux") && p.starts_with("/proc") {
        return true;
    }
    if real(&env.workspace()).is_some_and(|ws| p.starts_with(ws)) {
        return false;
    }
    env.private
        .iter()
        .any(|x| p.starts_with(real(x).unwrap_or_else(|| x.clone())))
}

pub fn read_text(env: &Env, path: &str) -> Result<String, String> {
    let p = resolve(env, path, false)?;
    let bytes = read_bytes(&p)?;
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

fn read_bytes(p: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(p).map_err(|e| format!("can't read {}: {e}", p.display()))?;
    if meta.is_dir() {
        return Err(format!("{} is a folder: list_files lists it", p.display()));
    }
    // Devices and pipes (/dev/zero, a FIFO, the server's own input) never
    // end or never answer.
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", p.display()));
    }
    if meta.len() > MAX_READ {
        return Err(format!(
            "{} is {} MB; scripts read files up to {} MB",
            p.display(),
            meta.len() / (1024 * 1024),
            MAX_READ / (1024 * 1024)
        ));
    }
    let mut out = Vec::with_capacity(meta.len() as usize);
    std::fs::File::open(p)
        .and_then(|f| f.take(MAX_READ).read_to_end(&mut out))
        .map_err(|e| format!("can't read {}: {e}", p.display()))?;
    Ok(out)
}

/// Refuse to write over something that isn't a regular file: opening a
/// pipe waits for a reader, and a device is no place for a script's text.
fn writable(p: &Path) -> Result<(), String> {
    match std::fs::metadata(p) {
        Ok(meta) if meta.is_dir() => Err(format!("{} is a folder", p.display())),
        Ok(meta) if !meta.is_file() => Err(format!("{} is not a regular file", p.display())),
        _ => Ok(()),
    }
}

/// Write (or with `append`, add to) a file. Returns where it went.
pub fn write_text(env: &Env, path: &str, text: &str, append: bool) -> Result<PathBuf, String> {
    if text.len() > MAX_WRITE {
        return Err(format!(
            "scripts write at most {} MB at once",
            MAX_WRITE / (1024 * 1024)
        ));
    }
    let p = resolve(env, path, true)?;
    writable(&p)?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&p)
        .map_err(|e| format!("can't write {}: {e}", p.display()))?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("can't write {}: {e}", p.display()))?;
    Ok(p)
}

/// The files and folders in a folder (folders end with `/`).
pub fn list_files(env: &Env, path: &str) -> Result<Vec<String>, String> {
    let p = if path.trim().is_empty() || path.trim() == "." {
        let ws = env.workspace();
        if env.files == ScriptFiles::None {
            return Err("scripts may not use files here ([script] files = \"none\")".into());
        }
        let _ = std::fs::create_dir_all(&ws);
        ws
    } else {
        resolve(env, path, false)?
    };
    let mut names: Vec<String> = std::fs::read_dir(&p)
        .map_err(|e| format!("can't list {}: {e}", p.display()))?
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                name + "/"
            } else {
                name
            }
        })
        .collect();
    names.sort();
    names.truncate(10_000);
    Ok(names)
}

pub fn exists(env: &Env, path: &str) -> bool {
    resolve(env, path, false).is_ok_and(|p| p.exists())
}

// -- the web ------------------------------------------------------------

/// A web request.
#[derive(Debug, Clone, Default)]
pub struct Fetch {
    pub method: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout: Option<f64>,
}

fn curl(env: &Env, url: &str, req: &Fetch, deadline: Instant) -> Result<Command, String> {
    if !env.web {
        return Err("scripts may not use the web here ([script] web = false)".into());
    }
    let lower = url.trim().to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!(
            "\"{url}\" is not a web address (http:// or https://)"
        ));
    }
    let left = deadline
        .saturating_duration_since(Instant::now())
        .as_secs_f64();
    let secs = req
        .timeout
        .unwrap_or(30.0)
        .clamp(1.0, 600.0)
        .min(left.max(1.0));
    let mut cmd = Command::new("curl");
    // `-q` first (curl takes it only there): the user's ~/.curlrc never
    // changes what a script's request does. `-g`: `[]` and `{}` in an
    // address are its own, not ranges of addresses.
    cmd.args(["-q", "-g", "-sS", "-L", "--max-redirs", "5"])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", &format!("{secs:.0}")])
        .args(["--max-filesize", &MAX_FETCH.to_string()])
        .args([
            "-A",
            concat!(
                "computer-use/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/mhrsdev/zero-use-computer)"
            ),
        ]);
    if let Some(m) = &req.method {
        let m = m.trim().to_uppercase();
        if !m.chars().all(|c| c.is_ascii_uppercase()) || m.is_empty() {
            return Err(format!("\"{m}\" is not an HTTP method"));
        }
        cmd.args(["-X", &m]);
    }
    for (k, v) in &req.headers {
        if k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']) {
            return Err(format!("bad header \"{k}\""));
        }
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    if req.body.is_some() {
        cmd.args(["--data-binary", "@-"]);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

/// Run curl; its stdout (capped) and the HTTP status it printed last. The
/// stop key ends it at once.
fn run_curl(env: &Env, mut cmd: Command, body: Option<&str>) -> Result<(Vec<u8>, u16), String> {
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "fetch needs curl (part of Windows 10 and later, macOS and most Linux systems), and it isn't installed here".to_string()
        } else {
            format!("can't run curl: {e}")
        }
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let data = body.unwrap_or("").as_bytes().to_vec();
        // Written from a thread: curl may answer before reading it all.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    // Both pipes drain on threads of their own, so neither can fill up and
    // stall curl while we wait for it.
    let stdout = child.stdout.take().map(|stdout| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.take(MAX_FETCH + 64).read_to_end(&mut out);
            out
        })
    });
    let stderr = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut err = String::new();
            let _ = stderr.take(64 * 1024).read_to_string(&mut err);
            err
        })
    });
    let status = loop {
        if env.stop.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("stopped by the user".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("curl failed: {e}"));
            }
        }
    };
    let mut out = stdout.and_then(|t| t.join().ok()).unwrap_or_default();
    let err = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
    // The status line `-w` adds after the body.
    let cut = out.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let code: u16 = String::from_utf8_lossy(&out[cut..])
        .trim()
        .parse()
        .unwrap_or(0);
    out.truncate(cut);
    // A failed transfer (time out, too big, cut off) failed, even when
    // curl got a status first: the body is only part of the answer.
    if !status.success() {
        let err = err.trim().trim_start_matches("curl: ").to_string();
        return Err(if err.is_empty() {
            format!(
                "the request failed (curl exit {})",
                status.code().unwrap_or(-1)
            )
        } else {
            format!("the request failed: {err}")
        });
    }
    Ok((out, code))
}

/// GET (or another method) a URL; its text.
pub fn fetch(env: &Env, url: &str, req: &Fetch, deadline: Instant) -> Result<String, String> {
    let mut cmd = curl(env, url, req, deadline)?;
    cmd.args(["-w", "\n%{http_code}"]).arg(url.trim());
    let (body, code) = run_curl(env, cmd, req.body.as_deref())?;
    let text = String::from_utf8(body)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    if code >= 400 {
        let start: String = text.chars().take(300).collect();
        return Err(format!("HTTP {code} from {url}: {}", start.trim()));
    }
    Ok(text)
}

/// Save what a URL returns to a file; its path.
pub fn download(env: &Env, url: &str, path: &str, deadline: Instant) -> Result<PathBuf, String> {
    let p = resolve(env, path, true)?;
    writable(&p)?;
    let name = p
        .file_name()
        .ok_or_else(|| format!("{} is not a file name", p.display()))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    }
    // Into a file of its own first: a download that fails leaves the file
    // that was there as it was.
    let tmp = p.with_file_name(format!(
        ".{}.{}.download",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut cmd = curl(env, url, &Fetch::default(), deadline)?;
    cmd.arg("-o")
        .arg(&tmp)
        .args(["-w", "\n%{http_code}"])
        .arg(url.trim());
    match run_curl(env, cmd, None) {
        Ok((_, code)) if code >= 400 => {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("HTTP {code} from {url}"));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(_) => {}
    }
    // An empty answer may leave no file at all.
    if !tmp.exists() {
        std::fs::write(&tmp, b"").map_err(|e| format!("can't write {}: {e}", p.display()))?;
    }
    std::fs::rename(&tmp, &p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("can't write {}: {e}", p.display())
    })?;
    Ok(p)
}

// -- memory -------------------------------------------------------------

fn memory_path(env: &Env) -> PathBuf {
    env.library.join("memory.json")
}

/// Everything remembered.
pub fn memory(env: &Env) -> Map<String, Value> {
    read_bytes(&memory_path(env))
        .ok()
        .and_then(|t| serde_json::from_slice(&t).ok())
        .unwrap_or_default()
}

/// Keep `value` under `key` (null forgets it).
pub fn remember(env: &Env, key: &str, value: Value) -> Result<(), String> {
    let mut all = memory(env);
    if value.is_null() {
        all.remove(key);
    } else {
        all.insert(key.to_string(), value);
    }
    let text = serde_json::to_string_pretty(&Value::Object(all)).map_err(|e| e.to_string())?;
    if text.len() > MAX_MEMORY {
        return Err(format!(
            "the memory is full ({} MB): forget something first, or write a file",
            MAX_MEMORY / (1024 * 1024)
        ));
    }
    let path = memory_path(env);
    std::fs::create_dir_all(&env.library)
        .map_err(|e| format!("can't make {}: {e}", env.library.display()))?;
    // A name of its own per process: two servers may share the folder.
    let tmp = env
        .library
        .join(format!(".memory.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| format!("can't keep it: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("can't keep it: {e}")
    })
}

// -- CSV -----------------------------------------------------------------

/// Most cells a CSV text may hold (each becomes a value: a text of commas
/// alone would fill the memory).
const MAX_CELLS: usize = 2_000_000;

/// Rows of CSV (or semicolon- or tab-separated) text. Numbers become
/// numbers.
pub fn parse_csv(text: &str) -> Result<Vec<Vec<Value>>, String> {
    let first = text.lines().next().unwrap_or("");
    let sep = [',', ';', '\t']
        .into_iter()
        .max_by_key(|c| first.matches(*c).count())
        .filter(|c| first.contains(*c))
        .unwrap_or(',');
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut cells = 0usize;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            '\r' => {}
            c if c == sep || c == '\n' => {
                row.push(cell(std::mem::take(&mut field)));
                if c == '\n' {
                    rows.push(std::mem::take(&mut row));
                }
                cells += 1;
                if cells > MAX_CELLS {
                    return Err(format!("the CSV has more than {MAX_CELLS} cells"));
                }
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(cell(field));
        rows.push(row);
    }
    Ok(rows)
}

fn cell(s: String) -> Value {
    let t = s.trim();
    if !t.is_empty() && t.len() < 40 {
        if let Ok(i) = t.parse::<i64>() {
            return Value::from(i);
        }
        if t.chars().any(|c| c.is_ascii_digit())
            && !t.contains(|c: char| c.is_alphabetic() && c != 'e' && c != 'E')
            && let Ok(f) = t.parse::<f64>()
            && f.is_finite()
        {
            return Value::from(f);
        }
    }
    Value::String(s)
}

/// CSV text of rows (arrays, or maps that share their keys), at most `max`
/// bytes. Every map row gets a cell for each of the first row's keys, so
/// many small rows can make a text far bigger than the values.
pub fn to_csv(rows: &[Value], max: usize) -> Result<String, String> {
    let field = |v: &Value| {
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    };
    let mut out = String::new();
    let keys: Option<Vec<String>> = rows
        .first()
        .and_then(|r| r.as_object())
        .map(|m| m.keys().cloned().collect());
    if let Some(keys) = &keys {
        out.push_str(
            &keys
                .iter()
                .map(|k| field(&Value::String(k.clone())))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    for r in rows {
        let cells: Vec<String> = match (r, &keys) {
            (Value::Array(a), _) => a.iter().map(field).collect(),
            (Value::Object(m), Some(keys)) => keys
                .iter()
                .map(|k| field(m.get(k).unwrap_or(&Value::Null)))
                .collect(),
            (other, _) => vec![field(other)],
        };
        out.push_str(&cells.join(","));
        out.push('\n');
        if out.len() > max {
            return Err(format!("the CSV would be more than {max} bytes"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(library: &str) -> Env {
        Env {
            files: ScriptFiles::Read,
            web: false,
            library: library.into(),
            max_seconds: 5,
            stop: Default::default(),
            app_tools: Vec::new(),
            decision: Default::default(),
            settings_key: None,
            private: Vec::new(),
        }
    }

    #[test]
    fn relative_paths_stay_in_the_scripts_folder() {
        let e = env("lib");
        let ws = e.workspace();
        assert_eq!(resolve(&e, "a/b.txt", true), Ok(ws.join("a/b.txt")));
        assert!(resolve(&e, "../b.txt", true).is_err());
    }

    /// `<scripts' folder>/nope/../../x` looked as if it stayed in the
    /// folder, but writing makes `nope` and the `..` then climbs out of it
    /// (to the server's settings).
    #[test]
    fn a_climb_out_of_a_folder_still_to_be_made_is_refused() {
        let dir = std::env::temp_dir().join(format!("cu-io-climb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = env(dir.join("lib").to_str().unwrap());
        e.files = ScriptFiles::All;
        let settings = dir.join("config.toml");
        e.private = vec![settings.clone()];
        let ws = e.workspace();
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(&settings, "[script]\n").unwrap();
        let climb = ws
            .join("nope")
            .join("..")
            .join("..")
            .join("..")
            .join("config.toml");
        let r = resolve(&e, climb.to_str().unwrap(), true);
        assert!(r.as_ref().is_err_and(|e| e.contains("goes up")), "{r:?}");
        assert!(write_text(&e, climb.to_str().unwrap(), "x", false).is_err());
        assert!(!ws.join("nope").exists());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "[script]\n");
        // Through folders that exist, `..` is resolved and checked as ever.
        let up = ws.join("..").join("..").join("config.toml");
        let r = resolve(&e, up.to_str().unwrap(), true);
        assert!(
            r.as_ref().is_err_and(|e| e.contains("server's own")),
            "{r:?}"
        );
        // A new folder without `..` is fine.
        let new = ws.join("nope").join("x.txt");
        assert_eq!(resolve(&e, new.to_str().unwrap(), true), Ok(new));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_with_too_many_cells_is_refused() {
        assert_eq!(
            parse_csv("a,b\n1,2\n"),
            Ok(vec![
                vec![Value::from("a"), Value::from("b")],
                vec![Value::from(1), Value::from(2)]
            ])
        );
        let commas = ",".repeat(MAX_CELLS + 1);
        assert!(parse_csv(&commas).is_err_and(|e| e.contains("more than")));
        let lines = "\n".repeat(MAX_CELLS + 1);
        assert!(parse_csv(&lines).is_err());
    }

    /// Each map row gets a cell for every key of the first row, so empty
    /// maps under a wide first row made a text far bigger than the rows.
    #[test]
    fn a_csv_bigger_than_the_limit_is_refused_while_it_is_made() {
        use serde_json::json;
        let wide: Map<String, Value> = (0..1000)
            .map(|i| (format!("k{i}"), Value::from(1)))
            .collect();
        let mut rows = vec![Value::Object(wide)];
        rows.extend((0..1000).map(|_| json!({})));
        let r = to_csv(&rows, 100_000);
        assert!(
            r.as_ref()
                .is_err_and(|e| e.contains("more than 100000 bytes")),
            "{r:?}"
        );
        assert_eq!(
            to_csv(&[json!({"a": 1, "b": "x,y"}), json!({"a": 2})], 100),
            Ok("a,b\n1,\"x,y\"\n2,\n".to_string())
        );
    }

    /// `\Windows\x` and `C:x` are "relative" on Windows, but joined to the
    /// scripts' folder they would leave it.
    #[cfg(windows)]
    #[test]
    fn rooted_relative_paths_are_not_written_to() {
        let e = env(r"C:\lib");
        for p in [r"\Windows\evil.txt", r"C:evil.txt"] {
            let r = resolve(&e, p, true);
            assert!(
                r.as_ref().is_err() || r.as_ref().unwrap().starts_with(r"C:\lib\files"),
                "{p} -> {r:?}"
            );
        }
    }
}
