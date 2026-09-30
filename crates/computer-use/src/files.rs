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
    ".docker",
    ".config/gh",
    ".password-store",
    "keyrings",
    "gnome-keyring",
    ".mozilla",
    "google-chrome",
    "chromium",
    "brave-browser",
    "bravesoftware",
    "user data", // Chrome / Edge / Brave profiles on Windows
];

/// Whole trees that are never read (Linux virtual filesystems: process
/// environments and memory, devices).
const DENIED_PREFIXES: &[&str] = &["/proc", "/sys", "/dev"];

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
    "cookies.sqlite",
    "local state", // Chrome: holds the cookie/password encryption key
    "hosts.yml",   // gh CLI token
];

/// File extensions that are never read.
const DENIED_EXTS: &[&str] = &["pem", "key", "p12", "pfx", "kdbx", "keystore", "jks", "ppk"];

/// Expand a leading `~` and require an absolute path.
pub fn expand(raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    // `\\server\share` / `//server/share`: resolving it would make Windows
    // connect to that server and send the user's credentials.
    if raw.starts_with("\\\\") || raw.starts_with("//") {
        return Err(Error::ActionFailed(
            "network paths (\\\\server\\share) are not supported; use a local path.".into(),
        ));
    }
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
    let with_root = format!("/{joined}");
    for p in DENIED_PREFIXES {
        if with_root == *p || with_root.starts_with(&format!("{p}/")) {
            return Some(format!("`{p}` holds process and system internals"));
        }
    }
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

/// Why `real` (an existing, symlink-free path) falls inside one of the
/// `extra` protected paths (the agent's own config, managed policy, audit log).
fn protected(real: &Path, extra: &[PathBuf]) -> Option<String> {
    extra.iter().find_map(|p| {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        real.starts_with(&p)
            .then(|| format!("{} is this tool's own configuration/log", p.display()))
    })
}

/// Resolve symlinks, then apply the deny list to both spellings.
fn checked(raw: &str, extra: &[PathBuf]) -> Result<PathBuf> {
    let path = expand(raw)?;
    if let Some(why) = denied(&path) {
        return Err(Error::Blocked(path.display().to_string(), why));
    }
    let real = std::fs::canonicalize(&path)
        .map_err(|e| Error::ActionFailed(format!("{}: {e}", path.display())))?;
    if let Some(why) = denied(&real).or_else(|| protected(&real, extra)) {
        return Err(Error::Blocked(real.display().to_string(), why));
    }
    Ok(real)
}

/// A path to create: the same deny list applies, to the path as written and
/// to where it really lands (its nearest existing parent, symlinks resolved).
pub fn check_create(raw: &str, extra: &[PathBuf]) -> Result<PathBuf> {
    let path = expand(raw)?;
    if let Some(why) = denied(&path) {
        return Err(Error::Blocked(path.display().to_string(), why));
    }
    let mut existing = path.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => break,
        }
    }
    let mut real = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for name in rest.iter().rev() {
        real.push(name);
    }
    if let Some(why) = denied(&real).or_else(|| protected(&real, extra)) {
        return Err(Error::Blocked(real.display().to_string(), why));
    }
    Ok(path)
}

fn human(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// A folder's entries: folders first, then files, each sorted by name.
pub fn list_folder(raw: &str, max: usize, extra: &[PathBuf]) -> Result<String> {
    let dir = checked(raw, extra)?;
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
        let entry_path = entry.path();
        if denied(&entry_path).is_some() {
            continue;
        }
        if let Ok(real) = std::fs::canonicalize(&entry_path)
            && (denied(&real).is_some() || protected(&real, extra).is_some())
        {
            continue;
        }
        // A symlink to a folder is a folder.
        match std::fs::metadata(&entry_path).or_else(|_| entry.metadata()) {
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
pub fn read_file(raw: &str, offset: u64, max_bytes: usize, extra: &[PathBuf]) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = checked(raw, extra)?;
    let meta = std::fs::metadata(&path)
        .map_err(|e| Error::ActionFailed(format!("{}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(Error::ActionFailed(format!(
            "{} is not a file (use list_folder for folders).",
            path.display()
        )));
    }
    let max = max_bytes.clamp(1, MAX_READ_BYTES);
    if offset > 0 && offset >= meta.len() {
        return Ok(format!(
            "offset {offset} is at or past the end of the file ({} bytes); nothing to read.",
            meta.len()
        ));
    }
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
    // A chunk boundary can cut a multi-byte character: drop the partial
    // bytes at either end (the next call, which starts at `end`, gets them).
    let mut skip = 0;
    if offset > 0 {
        while skip < 3 && buf.get(skip).is_some_and(|b| b & 0xC0 == 0x80) {
            skip += 1;
        }
    }
    let mut chunk = &buf[skip..];
    if let Err(e) = std::str::from_utf8(chunk)
        && e.error_len().is_none()
    {
        chunk = &chunk[..e.valid_up_to()];
    }
    let mut text = String::from_utf8_lossy(chunk).into_owned();
    let end = offset + (skip + chunk.len()) as u64;
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
        let none: &[PathBuf] = &[];

        let l = list_folder(&d, 50, none).unwrap();
        assert!(l.contains("[dir]  sub/") && l.contains("a.txt"), "{l}");
        assert!(!l.contains(".env"), "secrets are not even listed: {l}");

        let f = dir.join("a.txt");
        let fs = f.to_string_lossy();
        assert_eq!(read_file(&fs, 0, 100, none).unwrap(), "hello world");
        let cut = read_file(&fs, 0, 5, none).unwrap();
        assert!(
            cut.starts_with("hello") && cut.contains("offset=5"),
            "{cut}"
        );
        let past = read_file(&fs, 500, 5, none).unwrap();
        assert!(past.contains("past the end"), "{past}");
        let bin = read_file(&dir.join("b.bin").to_string_lossy(), 0, 100, none).unwrap();
        assert!(bin.contains("binary"), "{bin}");
        assert!(read_file(&dir.join(".env").to_string_lossy(), 0, 100, none).is_err());
        assert!(list_folder(&fs, 10, none).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chunk_boundary_never_splits_a_character() {
        let dir = std::env::temp_dir().join(format!("cu-utf8-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("u.txt");
        std::fs::write(&f, "aé€b").unwrap(); // 1 + 2 + 3 + 1 bytes
        let fs = f.to_string_lossy();
        let none: &[PathBuf] = &[];
        // Cut inside "é" and inside "€": no U+FFFD, and continuing from the
        // reported offset yields the rest.
        let first = read_file(&fs, 0, 2, none).unwrap();
        assert!(
            first.starts_with('a') && !first.contains('\u{FFFD}'),
            "{first}"
        );
        assert!(first.contains("offset=1"), "{first}");
        let second = read_file(&fs, 1, 100, none).unwrap();
        assert_eq!(second, "é€b");
        // Starting in the middle of a character skips its tail bytes.
        let mid = read_file(&fs, 2, 100, none).unwrap();
        assert_eq!(mid, "€b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn virtual_filesystems_and_network_paths_are_refused() {
        assert!(denied(Path::new("/proc/self/environ")).is_some());
        assert!(denied(Path::new("/sys/kernel")).is_some());
        assert!(denied(Path::new("/home/u/.docker/config.json")).is_some());
        assert!(denied(Path::new("/home/u/.config/gh/hosts.yml")).is_some());
        assert!(denied(Path::new("/home/u/.mozilla/firefox/x/cookies.sqlite")).is_some());
        assert!(denied(Path::new("/home/u/.password-store/a.gpg")).is_some());
        assert!(denied(Path::new("/home/u/process/notes.txt")).is_none());
        assert!(expand("\\\\attacker\\share\\x").is_err());
        assert!(expand("//attacker/share/x").is_err());
        assert!(read_file("/proc/self/environ", 0, 100, &[]).is_err());
    }

    #[test]
    fn the_tools_own_files_are_protected_and_creation_is_checked() {
        let dir = std::env::temp_dir().join(format!("cu-prot-{}", std::process::id()));
        let home = dir.join("cfg");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.toml"), "token = \"x\"").unwrap();
        let extra = vec![home.clone()];
        let f = home.join("config.toml");
        assert!(read_file(&f.to_string_lossy(), 0, 100, &extra).is_err());
        let listed = list_folder(&dir.to_string_lossy(), 50, &extra).unwrap();
        assert!(!listed.contains("cfg"), "{listed}");

        assert!(check_create(&dir.join("new").join("deep").to_string_lossy(), &extra).is_ok());
        assert!(check_create(&home.join("sub").to_string_lossy(), &extra).is_err());
        assert!(check_create("/home/u/.ssh/authorized_keys", &extra).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_are_listed_as_folders_and_cannot_dodge_the_list() {
        let dir = std::env::temp_dir().join(format!("cu-link-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::create_dir_all(dir.join(".ssh")).unwrap();
        std::fs::write(dir.join(".ssh").join("config"), "Host x").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("shortcut")).unwrap();
        std::os::unix::fs::symlink(dir.join(".ssh"), dir.join("innocent")).unwrap();
        let l = list_folder(&dir.to_string_lossy(), 50, &[]).unwrap();
        assert!(l.contains("[dir]  shortcut/"), "{l}");
        assert!(!l.contains("innocent"), "{l}");
        let via = dir.join("innocent").join("config");
        assert!(read_file(&via.to_string_lossy(), 0, 100, &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
