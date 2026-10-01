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
    let up = expanded.components().any(|c| c == Component::ParentDir);
    if expanded.is_relative() {
        if up {
            return Err(format!(
                "\"{path}\" leaves the scripts' folder: relative paths stay in {}",
                ws.display()
            ));
        }
        return Ok(ws.join(expanded));
    }
    let inside = !up && expanded.starts_with(&ws);
    let allowed = inside
        || match env.files {
            ScriptFiles::All => true,
            ScriptFiles::Read => !write,
            ScriptFiles::Workspace | ScriptFiles::None => false,
        };
    if allowed {
        return Ok(expanded);
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
    if meta.len() > MAX_READ {
        return Err(format!(
            "{} is {} MB; scripts read files up to {} MB",
            p.display(),
            meta.len() / (1024 * 1024),
            MAX_READ / (1024 * 1024)
        ));
    }
    std::fs::read(p).map_err(|e| format!("can't read {}: {e}", p.display()))
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
    cmd.args(["-sS", "-L", "--max-redirs", "5"])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", &format!("{secs:.0}")])
        .args(["--max-filesize", &MAX_FETCH.to_string()])
        .args(["-A", concat!("computer-use/", env!("CARGO_PKG_VERSION"))]);
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

/// Run curl; its stdout (capped) and the HTTP status it printed last.
fn run_curl(mut cmd: Command, body: Option<&str>) -> Result<(Vec<u8>, u16), String> {
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
    let mut out = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(MAX_FETCH + 64).read_to_end(&mut out);
    }
    let mut err = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    let status = child.wait().map_err(|e| format!("curl failed: {e}"))?;
    // The status line `-w` adds after the body.
    let cut = out.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let code: u16 = String::from_utf8_lossy(&out[cut..])
        .trim()
        .parse()
        .unwrap_or(0);
    out.truncate(cut);
    if !status.success() && code == 0 {
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
    let (body, code) = run_curl(cmd, req.body.as_deref())?;
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
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    }
    let mut cmd = curl(env, url, &Fetch::default(), deadline)?;
    cmd.arg("-o")
        .arg(&p)
        .args(["-w", "\n%{http_code}"])
        .arg(url.trim());
    let (_, code) = run_curl(cmd, None).inspect_err(|_| {
        let _ = std::fs::remove_file(&p);
    })?;
    if code >= 400 {
        let _ = std::fs::remove_file(&p);
        return Err(format!("HTTP {code} from {url}"));
    }
    Ok(p)
}

// -- memory -------------------------------------------------------------

fn memory_path(env: &Env) -> PathBuf {
    env.library.join("memory.json")
}

/// Everything remembered.
pub fn memory(env: &Env) -> Map<String, Value> {
    std::fs::read_to_string(memory_path(env))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
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
    let tmp = env.library.join(".memory.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("can't keep it: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("can't keep it: {e}"))
}

// -- CSV -----------------------------------------------------------------

/// Rows of CSV (or semicolon- or tab-separated) text. Numbers become
/// numbers.
pub fn parse_csv(text: &str) -> Vec<Vec<Value>> {
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
            c if c == sep => row.push(cell(std::mem::take(&mut field))),
            '\r' => {}
            '\n' => {
                row.push(cell(std::mem::take(&mut field)));
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(cell(field));
        rows.push(row);
    }
    rows
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

/// CSV text of rows (arrays, or maps that share their keys).
pub fn to_csv(rows: &[Value]) -> String {
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
    }
    out
}
