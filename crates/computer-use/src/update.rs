//! Updates (`[update]`). A while after the server starts it asks GitHub
//! for the latest release of `update.repo`; when that is newer than this
//! program, its zip for this system is downloaded, checked against the
//! SHA-256 GitHub gives for it, unpacked into `~/.computer-use/updates/`
//! and the program in it asked its version. There it waits: nothing an
//! agent may be using is replaced while it works. When the server next
//! starts after the computer has restarted (`update.install`), the waiting
//! version takes this program's place (and the rest of the package's
//! files, when the program runs from an unpacked package or plugin) before
//! anything else runs, and the server goes on as the new version.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{UpdateConfig, UpdateInstall};
use crate::error::{Error, Result};

/// File errors as this crate's.
trait Io<T> {
    fn io(self) -> Result<T>;
}

impl<T> Io<T> for std::io::Result<T> {
    fn io(self) -> Result<T> {
        self.map_err(|e| Error::Platform(e.to_string()))
    }
}

/// The program's file name in a release zip.
pub const BIN_NAME: &str = if cfg!(windows) {
    "computer-use-mcp.exe"
} else {
    "computer-use-mcp"
};

/// The longest a download may take.
const DOWNLOAD_TIME: Duration = Duration::from_secs(600);

/// A version number, major.minor.patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// "3.9.8" or "v3.9.8" (a missing patch is 0).
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let mut parts = s.split('.').map(|p| p.parse::<u32>().ok());
        let major = parts.next()??;
        let minor = parts.next().unwrap_or(Some(0))?;
        let patch = parts.next().unwrap_or(Some(0))?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version(major, minor, patch))
    }

    /// This program's version.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The release zip for this system, if releases have one.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("computer-use-mcp-linux-x64.zip")
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some("computer-use-mcp-windows-x64.zip")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("computer-use-mcp-macos-arm64.zip")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("computer-use-mcp-macos-x64.zip")
    } else {
        None
    }
}

/// Where updates wait: `~/.computer-use/updates`.
pub fn updates_dir() -> PathBuf {
    crate::config::home_dir().join("updates")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A newer release, as GitHub describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    pub url: String,
    /// Lower-case hex.
    pub sha256: String,
}

/// An update downloaded and waiting to go in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub version: String,
    /// The unpacked package.
    pub dir: PathBuf,
    /// When it was downloaded (seconds since 1970).
    pub downloaded: u64,
    pub sha256: String,
}

impl Pending {
    pub fn binary(&self) -> PathBuf {
        self.dir.join(BIN_NAME)
    }
}

// ---- asking GitHub -------------------------------------------------------------

/// Run curl (every supported system has it) for an https URL; what it
/// wrote to stdout.
fn curl(url: &str, out: Option<&Path>, time: Duration) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        return Err(Error::InvalidArgs(format!("not an https address: {url}")));
    }
    let mut cmd = Command::new("curl");
    // `-q` first: the user's ~/.curlrc never applies. Https only, even
    // through redirects (GitHub's downloads redirect to its file host).
    cmd.args([
        "-q",
        "-sS",
        "-f",
        "-L",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
    ])
    .args(["--max-time", &time.as_secs().to_string()])
    .args(["-H", "Accept: application/vnd.github+json"])
    .args([
        "-H",
        concat!("User-Agent: computer-use-mcp/", env!("CARGO_PKG_VERSION")),
    ]);
    if let Some(p) = out {
        cmd.arg("-o").arg(p);
    }
    cmd.arg(url);
    let o = cmd.output().map_err(|e| {
        Error::Platform(if e.kind() == std::io::ErrorKind::NotFound {
            "updates need curl, and it isn't installed here".into()
        } else {
            format!("can't run curl: {e}")
        })
    })?;
    if !o.status.success() {
        let why = String::from_utf8_lossy(&o.stderr);
        return Err(Error::Platform(format!(
            "could not fetch {url}: {}",
            why.trim().chars().take(300).collect::<String>()
        )));
    }
    Ok(o.stdout)
}

/// The latest release of `repo` when it is newer than this program and has
/// a zip for this system (None when up to date).
pub fn latest(repo: &str) -> Result<Option<Release>> {
    let Some(asset) = asset_name() else {
        return Ok(None);
    };
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let body = curl(&url, None, Duration::from_secs(30))?;
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| Error::Platform(format!("GitHub's answer isn't JSON: {e}")))?;
    release_from(&json, repo, asset, Version::current())
}

/// The newer release in GitHub's description of one (see [`latest`]).
pub fn release_from(
    json: &serde_json::Value,
    repo: &str,
    asset: &str,
    current: Version,
) -> Result<Option<Release>> {
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let tag = json["tag_name"].as_str().unwrap_or_default();
    let Some(version) = Version::parse(tag) else {
        return Err(Error::Platform(format!(
            "the latest release's tag \"{tag}\" isn't a version"
        )));
    };
    if version <= current {
        return Ok(None);
    }
    let Some(a) = json["assets"]
        .as_array()
        .and_then(|list| list.iter().find(|a| a["name"].as_str() == Some(asset)))
    else {
        return Ok(None);
    };
    let url = a["browser_download_url"].as_str().unwrap_or_default();
    // Only the repository's own release files.
    let from = format!("https://github.com/{repo}/releases/download/");
    if !url.starts_with(&from) {
        return Err(Error::Platform(format!(
            "{asset} is not a file of {repo}'s releases: {url}"
        )));
    }
    // GitHub's checksum of the file: without one there is nothing to check
    // the download against, so it isn't taken.
    let sha = a["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| Error::Platform(format!("GitHub gives no SHA-256 for {asset}")))?;
    Ok(Some(Release {
        version,
        url: url.to_string(),
        sha256: sha.to_ascii_lowercase(),
    }))
}

// ---- downloading ----------------------------------------------------------------

/// Download `rel`, check it, unpack it into `dir` and leave it waiting
/// (`pending.json`). An update already waiting for that version is kept.
pub fn download(rel: &Release, dir: &Path) -> Result<Pending> {
    if let Some(p) = pending(dir).filter(|p| p.version == rel.version.to_string()) {
        return Ok(p);
    }
    std::fs::create_dir_all(dir).io()?;
    let tag = format!("v{}", rel.version);
    let pid = std::process::id();
    let zip_path = dir.join(format!("{tag}.{pid}.zip.part"));
    let unpacked = dir.join(format!("{tag}.{pid}.part"));
    let done = (|| -> Result<Pending> {
        curl(&rel.url, Some(&zip_path), DOWNLOAD_TIME)?;
        let bytes = std::fs::read(&zip_path).io()?;
        let got = sha256_hex(&bytes);
        if got != rel.sha256 {
            return Err(Error::Platform(format!(
                "the download's SHA-256 is {got}, GitHub says {}: not taken",
                rel.sha256
            )));
        }
        let _ = std::fs::remove_dir_all(&unpacked);
        unzip(&bytes, &unpacked)?;
        let bin = unpacked.join(BIN_NAME);
        if !bin.is_file() {
            return Err(Error::Platform(format!("the zip has no {BIN_NAME}")));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).io()?;
        }
        // The program in it runs here and is the version it should be.
        let said = run_version(&bin)?;
        if Version::parse(said.split_whitespace().last().unwrap_or_default()) != Some(rel.version) {
            return Err(Error::Platform(format!(
                "the downloaded program says it is \"{said}\", not {}",
                rel.version
            )));
        }
        let final_dir = dir.join(&tag);
        let _ = std::fs::remove_dir_all(&final_dir);
        std::fs::rename(&unpacked, &final_dir).io()?;
        let p = Pending {
            version: rel.version.to_string(),
            dir: final_dir,
            downloaded: now_secs(),
            sha256: rel.sha256.clone(),
        };
        write_pending(dir, &p)?;
        Ok(p)
    })();
    let _ = std::fs::remove_file(&zip_path);
    if done.is_err() {
        let _ = std::fs::remove_dir_all(&unpacked);
    }
    done
}

/// `bin --version`, with a time limit.
fn run_version(bin: &Path) -> Result<String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| Error::Platform(format!("the downloaded program doesn't run here: {e}")))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().io()? {
            let mut out = String::new();
            if let Some(mut s) = child.stdout.take() {
                use std::io::Read;
                let _ = s.read_to_string(&mut out);
            }
            if !status.success() {
                return Err(Error::Platform(
                    "the downloaded program failed to start".into(),
                ));
            }
            return Ok(out.trim().to_string());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Platform(
                "the downloaded program didn't answer".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn write_pending(dir: &Path, p: &Pending) -> Result<()> {
    let tmp = dir.join(format!("pending.json.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(p).unwrap_or_default()).io()?;
    std::fs::rename(&tmp, dir.join("pending.json")).io()?;
    Ok(())
}

/// The update waiting in `dir`, if it is newer than this program and still
/// all there. One that isn't newer (installed by hand meanwhile) is cleared.
pub fn pending(dir: &Path) -> Option<Pending> {
    let file = dir.join("pending.json");
    let p: Pending = serde_json::from_slice(&std::fs::read(&file).ok()?).ok()?;
    let newer = Version::parse(&p.version).is_some_and(|v| v > Version::current());
    if !newer {
        let _ = std::fs::remove_dir_all(&p.dir);
        let _ = std::fs::remove_file(&file);
        return None;
    }
    p.binary().is_file().then_some(p)
}

// ---- putting it in place ----------------------------------------------------------

/// When the computer last started (seconds since 1970).
pub fn boot_time() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        stat.lines()
            .find_map(|l| l.strip_prefix("btime "))
            .and_then(|v| v.trim().parse().ok())
    }
    #[cfg(target_os = "macos")]
    {
        // "{ sec = 1759830000, usec = 0 } Tue Oct  7 ..."
        let o = Command::new("sysctl")
            .args(["-n", "kern.boottime"])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&o.stdout);
        let rest = s.split("sec =").nth(1)?;
        rest.split(',').next()?.trim().parse().ok()
    }
    #[cfg(windows)]
    {
        // SAFETY: a plain query.
        let up = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
        Some(now_secs().saturating_sub(up / 1000))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// Whether `p` goes in now, at a server's start: after the computer has
/// restarted since it was downloaded (`restart`), always (`start`), or
/// never (`manual`).
pub fn due(p: &Pending, install: UpdateInstall) -> bool {
    match install {
        UpdateInstall::Start => true,
        UpdateInstall::Manual => false,
        UpdateInstall::Restart => boot_time().is_some_and(|b| b > p.downloaded),
    }
}

/// Put `p` in place of the program at `exe` (and of the package's other
/// files, when `exe` runs from an unpacked package: its folder has
/// `.claude-plugin/plugin.json`), then clear it from the waiting folder.
pub fn install(p: &Pending, exe: &Path, dir: &Path) -> Result<()> {
    let home = exe
        .parent()
        .ok_or_else(|| Error::Platform(format!("{} has no folder", exe.display())))?;
    if home.join(".claude-plugin").join("plugin.json").is_file() {
        copy_tree(&p.dir, home, &p.binary())?;
    }
    replace_program(&p.binary(), exe)?;
    let _ = std::fs::remove_dir_all(&p.dir);
    let _ = std::fs::remove_file(dir.join("pending.json"));
    Ok(())
}

/// Copy everything under `from` into `to`, but `skip`.
fn copy_tree(from: &Path, to: &Path, skip: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from).io()? {
        let entry = entry.io()?;
        let path = entry.path();
        if path == skip {
            continue;
        }
        let target = to.join(entry.file_name());
        if entry.file_type().io()?.is_dir() {
            std::fs::create_dir_all(&target).io()?;
            copy_tree(&path, &target, skip)?;
        } else {
            std::fs::copy(&path, &target).io()?;
        }
    }
    Ok(())
}

/// Put the program `new` where `exe` is. Unix: beside it, then renamed over
/// it (a running copy keeps its file). Windows: a running program can't be
/// overwritten but can be renamed, so it moves aside (`.old`, cleared on a
/// later start) and the new one goes in its place.
fn replace_program(new: &Path, exe: &Path) -> Result<()> {
    let name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let beside = exe.with_file_name(format!("{name}.new"));
    std::fs::copy(new, &beside).io()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o755)).io()?;
        std::fs::rename(&beside, exe).io()?;
    }
    #[cfg(not(unix))]
    {
        let old = exe.with_file_name(format!("{name}.{}.old", now_secs()));
        std::fs::rename(exe, &old).io()?;
        if let Err(e) = std::fs::rename(&beside, exe) {
            // Put the old one back rather than leave nothing there.
            let _ = std::fs::rename(&old, exe);
            let _ = std::fs::remove_file(&beside);
            return Err(Error::Platform(e.to_string()));
        }
    }
    Ok(())
}

/// Clear what an install left beside the program: older copies moved aside
/// on Windows (once nothing runs them) and a half-copied `.new`.
pub fn tidy(exe: &Path) {
    let (Some(home), Some(name)) = (exe.parent(), exe.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let Ok(entries) = std::fs::read_dir(home) else {
        return;
    };
    for e in entries.flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        let aside =
            f.starts_with(&format!("{name}.")) && (f.ends_with(".old") || f.ends_with(".new"));
        if aside {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// ---- looking now and then -----------------------------------------------------------

/// What one look found.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    UpToDate,
    /// Downloaded (or already waiting): it goes in as `install` says.
    Waiting(Pending),
}

/// Look for a newer release and download it.
pub fn check_now(cfg: &UpdateConfig) -> Result<Found> {
    match latest(cfg.repo.trim())? {
        None => Ok(Found::UpToDate),
        Some(rel) => download(&rel, &updates_dir()).map(Found::Waiting),
    }
}

/// When any server on this computer last looked (`last-check`).
fn last_check(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("last-check"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Look `check_after_mins` after now, then every `check_every_hours`, in a
/// thread of its own: `settings` gives the current settings each time (they
/// may have changed). Servers on one computer share the looks.
pub fn watch(settings: impl Fn() -> UpdateConfig + Send + 'static) {
    let first = settings().check_after_mins;
    let spawned = std::thread::Builder::new()
        .name("updates".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(first.saturating_mul(60)));
            loop {
                let cfg = settings();
                let every = cfg.check_every_hours.max(1).saturating_mul(3600);
                let dir = updates_dir();
                if cfg.enabled && now_secs().saturating_sub(last_check(&dir)) >= every {
                    // Claimed first, so the other servers wait their turn.
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join("last-check"), now_secs().to_string());
                    match check_now(&cfg) {
                        Ok(Found::UpToDate) => {
                            log::info!("updates: {} is the latest", Version::current())
                        }
                        Ok(Found::Waiting(p)) => log::info!(
                            "updates: {} downloaded; it goes in {}",
                            p.version,
                            when(cfg.install)
                        ),
                        Err(e) => log::warn!("updates: {e}"),
                    }
                }
                // Look at the shared clock again in a while.
                std::thread::sleep(Duration::from_secs(30 * 60));
            }
        });
    if let Err(e) = spawned {
        log::warn!("updates: can't start looking: {e}");
    }
}

/// When a waiting update goes in, in words.
pub fn when(install: UpdateInstall) -> &'static str {
    match install {
        UpdateInstall::Restart => "when the server starts after the computer restarts",
        UpdateInstall::Start => "the next time the server starts",
        UpdateInstall::Manual => "with `computer-use-mcp update --install`",
    }
}

// ---- zip and SHA-256 ----------------------------------------------------------------

/// Unpack a zip (stored or deflated entries) into `to`. Names that would
/// land outside `to` (absolute, `..`) are refused.
pub fn unzip(data: &[u8], to: &Path) -> Result<()> {
    let bad = |why: &str| Error::Platform(format!("the update's zip is broken: {why}"));
    let u16_at = |i: usize| -> Result<usize> {
        data.get(i..i + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .ok_or_else(|| bad("cut short"))
    };
    let u32_at = |i: usize| -> Result<usize> {
        data.get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| bad("cut short"))
    };
    // The end of central directory record, searched from the end.
    let eocd = (0..data.len().saturating_sub(21))
        .rev()
        .take(65_557)
        .find(|&i| data[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06]))
        .ok_or_else(|| bad("no directory"))?;
    let count = u16_at(eocd + 10)?;
    let mut at = u32_at(eocd + 16)?;
    std::fs::create_dir_all(to).io()?;
    for _ in 0..count {
        if data.get(at..at + 4) != Some(&[0x50, 0x4b, 0x01, 0x02]) {
            return Err(bad("a directory entry is missing"));
        }
        let method = u16_at(at + 10)?;
        let crc = u32_at(at + 16)? as u32;
        let packed = u32_at(at + 20)?;
        let size = u32_at(at + 24)?;
        let (name_len, extra_len, note_len) =
            (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?);
        let local = u32_at(at + 42)?;
        // Made on Unix with an executable mode (install.sh, the program).
        let runnable = data.get(at + 5) == Some(&3) && (u32_at(at + 38)? >> 16) & 0o111 != 0;
        let name = data
            .get(at + 46..at + 46 + name_len)
            .ok_or_else(|| bad("cut short"))?;
        let name = String::from_utf8_lossy(name).replace('\\', "/");
        at += 46 + name_len + extra_len + note_len;
        let rel = name.trim_start_matches("./");
        if rel.is_empty() {
            continue;
        }
        let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
        if rel.starts_with('/') || parts.iter().any(|p| *p == ".." || p.contains(':')) {
            return Err(bad(&format!("\"{name}\" points outside the package")));
        }
        let target = parts.iter().fold(to.to_path_buf(), |p, part| p.join(part));
        if name.ends_with('/') {
            std::fs::create_dir_all(&target).io()?;
            continue;
        }
        // The local header, then the data.
        if data.get(local..local + 4) != Some(&[0x50, 0x4b, 0x03, 0x04]) {
            return Err(bad("a file's header is missing"));
        }
        let start = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        let raw = data
            .get(start..start + packed)
            .ok_or_else(|| bad("cut short"))?;
        let bytes = match method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size.max(1) * 2 + 1024)
                .map_err(|_| bad(&format!("\"{name}\" doesn't unpack")))?,
            m => {
                return Err(bad(&format!(
                    "\"{name}\" is packed a way it can't read ({m})"
                )));
            }
        };
        if bytes.len() != size || crc32(&bytes) != crc {
            return Err(bad(&format!("\"{name}\" doesn't check out")));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).io()?;
        }
        std::fs::write(&target, &bytes).io()?;
        #[cfg(unix)]
        if runnable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).io()?;
        }
        #[cfg(not(unix))]
        let _ = runnable;
    }
    Ok(())
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// SHA-256 of `data`, lower-case hex.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(Version::parse("v3.9.8"), Some(Version(3, 9, 8)));
        assert_eq!(Version::parse("3.10"), Some(Version(3, 10, 0)));
        assert!(Version::parse("v3.10.0").unwrap() > Version::parse("v3.9.12").unwrap());
        assert_eq!(Version::parse("v3.9.8-beta"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn sha256_and_crc32_match_known_values() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let long = vec![b'a'; 1_000];
        assert_eq!(
            sha256_hex(&long),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    fn release_json(tag: &str, digest: Option<&str>, url: &str) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag, "draft": false, "prerelease": false,
            "assets": [
                {"name": "other.zip", "browser_download_url": "https://x", "digest": null},
                {"name": "computer-use-mcp-linux-x64.zip", "browser_download_url": url, "digest": digest},
            ]
        })
    }

    #[test]
    fn only_a_newer_checked_release_of_the_repo_is_taken() {
        let repo = "mhrsdev/zero-use-computer";
        let asset = "computer-use-mcp-linux-x64.zip";
        let url = "https://github.com/mhrsdev/zero-use-computer/releases/download/v3.9.9/computer-use-mcp-linux-x64.zip";
        let sha = format!("sha256:{}", "ab".repeat(32));
        let now = Version(3, 9, 8);
        let r = release_from(&release_json("v3.9.9", Some(&sha), url), repo, asset, now)
            .unwrap()
            .unwrap();
        assert_eq!(r.version, Version(3, 9, 9));
        assert_eq!(r.sha256, "ab".repeat(32));
        // Not newer.
        let same = release_json("v3.9.8", Some(&sha), url);
        assert_eq!(release_from(&same, repo, asset, now).unwrap(), None);
        // No checksum, or a file from elsewhere: refused.
        let bare = release_json("v3.9.9", None, url);
        assert!(release_from(&bare, repo, asset, now).is_err());
        let away = release_json("v3.9.9", Some(&sha), "https://evil.example/x.zip");
        assert!(release_from(&away, repo, asset, now).is_err());
        // A pre-release isn't taken.
        let mut pre = release_json("v4.0.0", Some(&sha), url);
        pre["prerelease"] = true.into();
        assert_eq!(release_from(&pre, repo, asset, now).unwrap(), None);
        // No zip for this system.
        let r = release_from(
            &release_json("v3.9.9", Some(&sha), url),
            repo,
            "nope.zip",
            now,
        );
        assert_eq!(r.unwrap(), None);
    }

    /// A zip with `files` (name, contents), stored or deflated.
    fn make_zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut dir = Vec::new();
        for (name, body, deflate) in files {
            let packed = if *deflate {
                miniz_oxide::deflate::compress_to_vec(body, 6)
            } else {
                body.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let at = out.len() as u32;
            let crc = crc32(body);
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);
            dir.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0]);
            dir.extend_from_slice(&method.to_le_bytes());
            dir.extend_from_slice(&[0, 0, 0, 0]);
            dir.extend_from_slice(&crc.to_le_bytes());
            dir.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(body.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&at.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let dir_at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        let n = (files.len() as u16).to_le_bytes();
        out.extend_from_slice(&n);
        out.extend_from_slice(&n);
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cu-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_zip_unpacks_and_a_bad_one_is_refused() {
        let dir = temp("unzip");
        let big = b"hello hello hello hello hello".repeat(50);
        let zip = make_zip(&[
            ("computer-use-mcp", b"#!/bin/sh\n", false),
            ("skills/a/SKILL.md", &big, true),
            ("skills/", b"", false),
        ]);
        unzip(&zip, &dir).unwrap();
        assert_eq!(
            std::fs::read(dir.join("computer-use-mcp")).unwrap(),
            b"#!/bin/sh\n"
        );
        assert_eq!(std::fs::read(dir.join("skills/a/SKILL.md")).unwrap(), big);
        // A name going up out of the folder.
        let evil = make_zip(&[("../escape", b"x", false)]);
        assert!(unzip(&evil, &temp("evil")).is_err());
        // A changed byte fails its check.
        let mut broken = make_zip(&[("f", b"abcdef", false)]);
        let i = broken.windows(6).position(|w| w == b"abcdef").unwrap();
        broken[i] = b'X';
        assert!(unzip(&broken, &temp("broken")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_waiting_update_goes_in_by_the_setting_and_replaces_the_program() {
        let dir = temp("install");
        let unpacked = dir.join("v99.0.0");
        std::fs::create_dir_all(unpacked.join("skills")).unwrap();
        std::fs::write(unpacked.join(BIN_NAME), b"new program").unwrap();
        std::fs::write(unpacked.join("skills").join("SKILL.md"), b"new skill").unwrap();
        let p = Pending {
            version: "99.0.0".into(),
            dir: unpacked,
            downloaded: now_secs(),
            sha256: String::new(),
        };
        write_pending(&dir, &p).unwrap();
        assert_eq!(pending(&dir), Some(p.clone()));
        assert!(due(&p, UpdateInstall::Start));
        assert!(!due(&p, UpdateInstall::Manual));
        // Downloaded after the computer started: not until it restarts.
        if boot_time().is_some() {
            assert!(!due(&p, UpdateInstall::Restart));
            let before = Pending {
                downloaded: 0,
                ..p.clone()
            };
            assert!(due(&before, UpdateInstall::Restart));
        }
        // A package folder: the program and the other files are replaced.
        let home = dir.join("pkg");
        std::fs::create_dir_all(home.join(".claude-plugin")).unwrap();
        std::fs::write(home.join(".claude-plugin").join("plugin.json"), b"{}").unwrap();
        let exe = home.join(BIN_NAME);
        std::fs::write(&exe, b"old program").unwrap();
        install(&p, &exe, &dir).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new program");
        assert_eq!(
            std::fs::read(home.join("skills/SKILL.md")).unwrap(),
            b"new skill"
        );
        assert!(pending(&dir).is_none());
        tidy(&exe);
        let left: Vec<_> = std::fs::read_dir(&home)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".old") || n.ends_with(".new"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_no_newer_than_this_program_is_cleared() {
        let dir = temp("old");
        let unpacked = dir.join("v0.0.1");
        std::fs::create_dir_all(&unpacked).unwrap();
        std::fs::write(unpacked.join(BIN_NAME), b"x").unwrap();
        let p = Pending {
            version: "0.0.1".into(),
            dir: unpacked.clone(),
            downloaded: 0,
            sha256: String::new(),
        };
        write_pending(&dir, &p).unwrap();
        assert!(pending(&dir).is_none());
        assert!(!unpacked.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
