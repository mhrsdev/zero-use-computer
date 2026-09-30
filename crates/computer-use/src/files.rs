//! Read-only file access for the agent: list a folder, read a text file.
//!
//! Much cheaper than driving a file manager through its accessibility tree.
//! Nothing here writes, and places that commonly hold secrets (SSH/GPG/cloud
//! credentials, browser profiles, password stores, key files) are refused.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Most entries `list_folder` returns.
pub const MAX_ENTRIES: usize = 200;
/// Default and hard limit for one `read_file` call.
pub const DEFAULT_READ_BYTES: usize = 32 * 1024;
pub const MAX_READ_BYTES: usize = 256 * 1024;

/// Folders (by name, any depth) that are never read.
const DENIED_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".config/gcloud",
    "gcloud",
    "keychains",
    "credentials",
    "protect", // Windows DPAPI master keys
    "login data for account",
];

/// File names that are never read.
const DENIED_FILES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".netrc",
    ".pgpass",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".env",
    "login data",
    "cookies",
    "web data",
    "key4.db",
    "logins.json",
    "shadow",
    "gshadow",
    "sam",
    "ntuser.dat",
    "keychain-db",
    "secrets.json",
];

/// File extensions that are never read.
const DENIED_EXTS: &[&str] = &["pem", "key", "p12", "pfx", "kdbx", "keystore", "jks", "ppk"];

/// Expand a leading `~` and require an absolute path.
pub fn expand(raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let path = match (raw.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            PathBuf::from(h).join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(raw),
    };
    if raw.is_empty() || !path.is_absolute() {
        return Err(Error::ActionFailed(
            "give an absolute path (or one starting with ~).".into(),
        ));
    }
    Ok(path)
}

/// Why `path` must not be read, if it must not.
pub fn denied(path: &Path) -> Option<String> {
    // Split on both separators so Windows paths are judged the same anywhere.
    let lower: Vec<String> = path
        .to_string_lossy()
        .to_lowercase()
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .map(str::to_string)
        .collect();
    let joined = lower.join("/");
    for d in DENIED_DIRS {
        if lower.iter().any(|c| c == d)
            || joined.contains(&format!("/{d}/"))
            || joined.ends_with(&format!("/{d}"))
        {
            return Some(format!("`{d}` holds credentials or secrets"));
        }
    }
    if let Some(name) = lower.last() {
        if DENIED_FILES.contains(&name.as_str()) || name.starts_with(".env.") {
            return Some(format!("`{name}` is a credentials/secrets file"));
        }
        if let Some(ext) = Path::new(name).extension().and_then(|e| e.to_str())
            && DENIED_EXTS.contains(&ext)
        {
            return Some(format!("`.{ext}` files hold keys or password stores"));
        }
    }
    None
}

/// Resolve symlinks, then apply the deny list to both spellings.
fn checked(raw: &str) -> Result<PathBuf> {
    let path = expand(raw)?;
    if let Some(why) = denied(&path) {
        return Err(Error::Blocked(path.display().to_string(), why));
    }
    let real = std::fs::canonicalize(&path)
        .map_err(|e| Error::ActionFailed(format!("{}: {e}", path.display())))?;
    if let Some(why) = denied(&real) {
        return Err(Error::Blocked(real.display().to_string(), why));
    }
    Ok(real)
}

fn human(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// A folder's entries: folders first, then files, each sorted by name.
pub fn list_folder(raw: &str, max: usize) -> Result<String> {
    let dir = checked(raw)?;
    if !dir.is_dir() {
        return Err(Error::ActionFailed(format!(
            "{} is not a folder (use read_file for files).",
            dir.display()
        )));
    }
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let rd = std::fs::read_dir(&dir)
        .map_err(|e| Error::ActionFailed(format!("{}: {e}", dir.display())))?;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if denied(&entry.path()).is_some() {
            continue;
        }
        match entry.metadata() {
            Ok(m) if m.is_dir() => dirs.push(name),
            Ok(m) => files.push((name, m.len())),
            Err(_) => files.push((name, 0)),
        }
    }
    dirs.sort_by_key(|n| n.to_lowercase());
    files.sort_by_key(|(n, _)| n.to_lowercase());
    let total = dirs.len() + files.len();
    let max = max.clamp(1, MAX_ENTRIES);
    let mut out = format!(
        "{} — {} folder(s), {} file(s)\n",
        dir.display(),
        dirs.len(),
        files.len()
    );
    for n in dirs.iter().take(max) {
        out.push_str(&format!("[dir]  {n}/\n"));
    }
    let left = max.saturating_sub(dirs.len());
    for (n, size) in files.iter().take(left) {
        out.push_str(&format!("[file] {n} ({})\n", human(*size)));
    }
    if total > max {
        out.push_str(&format!("… {} more not shown\n", total - max));
    }
    Ok(out)
}

/// A text file's contents from `offset`, at most `max_bytes`.
pub fn read_file(raw: &str, offset: u64, max_bytes: usize) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = checked(raw)?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| Error::ActionFailed(format!("{}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(Error::ActionFailed(format!(
            "{} is not a file (use list_folder for folders).",
            path.display()
        )));
    }
    let max = max_bytes.clamp(1, MAX_READ_BYTES);
    let mut f = std::fs::File::open(&path)
        .map_err(|e| Error::ActionFailed(format!("{}: {e}", path.display())))?;
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::ActionFailed(e.to_string()))?;
    let mut buf = Vec::new();
    f.take(max as u64)
        .read_to_end(&mut buf)
        .map_err(|e| Error::ActionFailed(format!("{}: {e}", path.display())))?;
    if buf.iter().take(8192).any(|&b| b == 0) {
        return Ok(format!(
            "{} is a binary file ({}); only text files can be read.",
            path.display(),
            human(meta.len())
        ));
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    let end = offset + buf.len() as u64;
    if end < meta.len() {
        text.push_str(&format!(
            "\n… truncated at byte {end} of {}; call again with offset={end} for more.",
            meta.len()
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_denied() {
        for p in [
            "/home/u/.ssh/config",
            "/home/u/.aws/credentials",
            "/home/u/id_rsa",
            "/home/u/project/.env",
            "/home/u/project/.env.local",
            "/home/u/server.pem",
            "C:\\Users\\u\\.ssh\\id_ed25519",
            "/home/u/vault.kdbx",
        ] {
            assert!(denied(Path::new(p)).is_some(), "{p}");
        }
        assert!(denied(Path::new("/home/u/notes/todo.txt")).is_none());
        assert!(denied(Path::new("/home/u/environment.md")).is_none());
    }

    #[test]
    fn relative_paths_are_refused() {
        assert!(expand("docs/a.txt").is_err());
        assert!(expand("").is_err());
    }

    #[test]
    fn lists_and_reads() {
        let dir = std::env::temp_dir().join(format!("cu-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "hello world").unwrap();
        std::fs::write(dir.join("b.bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        let d = dir.to_string_lossy();

        let l = list_folder(&d, 50).unwrap();
        assert!(l.contains("[dir]  sub/") && l.contains("a.txt"), "{l}");
        assert!(!l.contains(".env"), "secrets are not even listed: {l}");

        let f = dir.join("a.txt");
        assert_eq!(
            read_file(&f.to_string_lossy(), 0, 100).unwrap(),
            "hello world"
        );
        let cut = read_file(&f.to_string_lossy(), 0, 5).unwrap();
        assert!(
            cut.starts_with("hello") && cut.contains("offset=5"),
            "{cut}"
        );
        let bin = read_file(&dir.join("b.bin").to_string_lossy(), 0, 100).unwrap();
        assert!(bin.contains("binary"), "{bin}");
        assert!(read_file(&dir.join(".env").to_string_lossy(), 0, 100).is_err());
        assert!(list_folder(&f.to_string_lossy(), 10).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
