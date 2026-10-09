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

    /// This program's version, its numbers only: a preview build
    /// ("5.0.0-preview") is 5.0.0, and [`current_is_preview`] says so.
    pub fn current() -> Version {
        Version::of_program(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }

    /// A version as a program states it, a preview's label left off
    /// ("5.0.0-preview" is 5.0.0). Release tags are read with [`Version::parse`],
    /// which takes numbers only.
    pub fn of_program(s: &str) -> Option<Version> {
        Version::parse(s.trim().split(['-', '+']).next().unwrap_or_default())
    }
}

/// Whether this program is a preview build (its version has a label, such
/// as 5.0.0-preview).
pub fn current_is_preview() -> bool {
    env!("CARGO_PKG_VERSION").contains(['-', '+'])
}

/// Whether a release of version `v` is newer than `current`. When `current`
/// is this program and it is a preview, the release of the same numbers is
/// the finished one, and newer.
pub fn is_newer(v: Version, current: Version) -> bool {
    v > current || (v == current && current == Version::current() && current_is_preview())
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
    /// What the release says about itself (cut short).
    pub notes: String,
    /// Its page on GitHub, or empty.
    pub page: String,
    /// GitHub marks it a pre-release.
    pub prerelease: bool,
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
    /// GitHub marks it a pre-release.
    #[serde(default)]
    pub prerelease: bool,
}

impl Pending {
    pub fn binary(&self) -> PathBuf {
        self.dir.join(BIN_NAME)
    }
}

// ---- asking GitHub -------------------------------------------------------------

/// curl with this program's usual safety: `-q` first (the user's
/// ~/.curlrc never applies), https only even through redirects (GitHub's
/// downloads redirect to its file host).
fn base_curl(time: Duration) -> Command {
    let mut cmd = Command::new("curl");
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
    cmd
}

fn curl_failed(url: &str, o: &std::process::Output) -> Error {
    let why = String::from_utf8_lossy(&o.stderr);
    Error::Platform(format!(
        "could not fetch {url}: {}",
        why.trim().chars().take(300).collect::<String>()
    ))
}

fn curl_missing(e: std::io::Error) -> Error {
    Error::Platform(if e.kind() == std::io::ErrorKind::NotFound {
        "updates need curl, and it isn't installed here".into()
    } else {
        format!("can't run curl: {e}")
    })
}

/// Run curl (every supported system has it) for an https URL; what it
/// wrote to stdout.
fn curl(url: &str, out: Option<&Path>, time: Duration) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        return Err(Error::InvalidArgs(format!("not an https address: {url}")));
    }
    let mut cmd = base_curl(time);
    if let Some(p) = out {
        cmd.arg("-o").arg(p);
    }
    cmd.arg(url);
    let o = cmd.output().map_err(curl_missing)?;
    if !o.status.success() {
        return Err(curl_failed(url, &o));
    }
    Ok(o.stdout)
}

/// What asking GitHub for a page of its API came back with.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched {
    /// The page, and the tag GitHub gave it (to ask "changed?" next time).
    Body(Vec<u8>, Option<String>),
    /// Not changed since the tag we sent: nothing to read.
    NotModified,
}

/// An API page, asking only "changed since?" when we hold its tag: an
/// answer of "no" doesn't count against GitHub's limit on requests.
fn curl_api(url: &str, etag: Option<&str>) -> Result<Fetched> {
    if !url.starts_with("https://") {
        return Err(Error::InvalidArgs(format!("not an https address: {url}")));
    }
    let dir = updates_dir();
    std::fs::create_dir_all(&dir).io()?;
    let head = dir.join(format!("headers.{}.tmp", std::process::id()));
    let mut cmd = base_curl(Duration::from_secs(30));
    cmd.arg("-D").arg(&head);
    if let Some(t) = etag.filter(|t| t.len() < 200 && !t.contains(['\r', '\n'])) {
        cmd.args(["-H", &format!("If-None-Match: {t}")]);
    }
    cmd.arg(url);
    let done = cmd.output();
    let headers = std::fs::read_to_string(&head).unwrap_or_default();
    let _ = std::fs::remove_file(&head);
    let o = done.map_err(curl_missing)?;
    if !o.status.success() {
        return Err(curl_failed(url, &o));
    }
    // The last answer, after any redirect.
    let last = headers
        .rsplit("\r\n\r\n")
        .find(|b| !b.trim().is_empty())
        .unwrap_or("");
    let status = last
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    if status == "304" {
        return Ok(Fetched::NotModified);
    }
    let tag = last
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("etag").then(|| v.trim().to_string())
        })
        .filter(|t| t.len() < 200 && !t.contains(['\r', '\n']));
    Ok(Fetched::Body(o.stdout, tag))
}

/// The page of GitHub's API that says which release to take under these
/// settings.
pub fn endpoint(cfg: &UpdateConfig) -> String {
    let repo = cfg.repo.trim();
    if let Some(v) = Version::parse(&cfg.pin) {
        format!("https://api.github.com/repos/{repo}/releases/tags/v{v}")
    } else if cfg.channel.trim() == "prerelease" {
        format!("https://api.github.com/repos/{repo}/releases?per_page=30")
    } else {
        format!("https://api.github.com/repos/{repo}/releases/latest")
    }
}

/// The release to take, from GitHub's answer at [`endpoint`]: the latest
/// (stable), the newest of the last thirty (prerelease) or the pinned one,
/// when it is newer than `current`, has a zip for this system and isn't
/// the one to skip. None when there is nothing to take.
pub fn choose(
    json: &serde_json::Value,
    cfg: &UpdateConfig,
    asset: &str,
    current: Version,
) -> Result<Option<Release>> {
    let repo = cfg.repo.trim();
    let skip = Version::parse(&cfg.skip_version);
    let one = |j: &serde_json::Value, pre: bool| {
        release_from_with(j, repo, asset, current, pre)
            .map(|r| r.filter(|r| Some(r.version) != skip))
    };
    if Version::parse(&cfg.pin).is_some() {
        return one(json, true);
    }
    if cfg.channel.trim() != "prerelease" {
        return one(json, false);
    }
    let Some(list) = json.as_array() else {
        return Err(Error::Platform(
            "GitHub's list of releases isn't a list".into(),
        ));
    };
    let mut found: Vec<(Version, &serde_json::Value)> = list
        .iter()
        .filter(|j| j["draft"].as_bool() != Some(true))
        .filter_map(|j| Some((Version::parse(j["tag_name"].as_str()?)?, j)))
        .collect();
    found.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (v, j) in found {
        if !is_newer(v, current) {
            break;
        }
        if Some(v) == skip {
            continue;
        }
        match one(j, true)? {
            Some(r) => return Ok(Some(r)),
            None => continue, // no zip for this system (yet)
        }
    }
    Ok(None)
}

/// The latest release of `repo` when it is newer than this program and has
/// a zip for this system (None when up to date), under `cfg`'s channel,
/// pin and skipped version.
pub fn latest(cfg: &UpdateConfig) -> Result<Option<Release>> {
    let Some(asset) = asset_name() else {
        return Ok(None);
    };
    match curl_api(&endpoint(cfg), None)? {
        Fetched::Body(body, _) => {
            let json: serde_json::Value = serde_json::from_slice(&body)
                .map_err(|e| Error::Platform(format!("GitHub's answer isn't JSON: {e}")))?;
            choose(&json, cfg, asset, Version::current())
        }
        Fetched::NotModified => Ok(None),
    }
}

/// The newer release in GitHub's description of one (see [`latest`]);
/// a pre-release isn't taken.
pub fn release_from(
    json: &serde_json::Value,
    repo: &str,
    asset: &str,
    current: Version,
) -> Result<Option<Release>> {
    release_from_with(json, repo, asset, current, false)
}

/// [`release_from`], taking a pre-release too when `pre`.
pub fn release_from_with(
    json: &serde_json::Value,
    repo: &str,
    asset: &str,
    current: Version,
    pre: bool,
) -> Result<Option<Release>> {
    if json["draft"].as_bool() == Some(true) || (!pre && json["prerelease"].as_bool() == Some(true))
    {
        return Ok(None);
    }
    let tag = json["tag_name"].as_str().unwrap_or_default();
    let Some(version) = Version::parse(tag) else {
        return Err(Error::Platform(format!(
            "the latest release's tag \"{tag}\" isn't a version"
        )));
    };
    if !is_newer(version, current) {
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
        notes: json["body"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(6000)
            .collect(),
        page: json["html_url"]
            .as_str()
            .filter(|u| u.starts_with(&format!("https://github.com/{repo}/")))
            .unwrap_or_default()
            .to_string(),
        prerelease: json["prerelease"].as_bool() == Some(true),
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
            prerelease: rel.prerelease,
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
pub(crate) fn run_version(bin: &Path) -> Result<String> {
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
    let newer = Version::parse(&p.version).is_some_and(|v| is_newer(v, Version::current()));
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
    install_keeping(p, exe, dir, None)
}

/// `install`, also keeping the settings file at `config` beside the program
/// it puts aside, so going back can bring the settings that worked with
/// it back too (see [`rollback_to`]).
pub fn install_keeping(
    p: &Pending,
    exe: &Path,
    dir: &Path,
    config: Option<&Path>,
) -> Result<()> {
    // Already there (put in place by another server, or twice from the
    // panel): nothing to replace, and the copy kept to go back to stays.
    // (as it says it: a 5.0.0-preview is not the 5.0.0 release)
    if said_version(exe).as_deref() == Some(p.version.as_str()) {
        discard(dir);
        return Ok(());
    }
    let home = exe
        .parent()
        .ok_or_else(|| Error::Platform(format!("{} has no folder", exe.display())))?;
    if home.join(".claude-plugin").join("plugin.json").is_file() {
        copy_tree(&p.dir, home, &p.binary())?;
    }
    keep_previous(exe, dir, config);
    replace_program(&p.binary(), exe)?;
    let _ = std::fs::remove_dir_all(&p.dir);
    let _ = std::fs::remove_file(dir.join("pending.json"));
    Ok(())
}

/// The version the program at `path` says it is, as it says it
/// ("5.0.0-preview").
pub(crate) fn said_version(path: &Path) -> Option<String> {
    let said = run_version(path).ok()?;
    said.split_whitespace().last().map(str::to_string)
}

/// The version the program at `path` says it is, its numbers.
pub(crate) fn version_of(path: &Path) -> Option<Version> {
    Version::of_program(&said_version(path)?)
}

/// Whether the waiting update may still go in under these settings: the
/// user may have skipped it, pinned another version or left the
/// pre-release channel since it was downloaded.
pub fn allowed(p: &Pending, cfg: &UpdateConfig) -> bool {
    let Some(v) = Version::parse(&p.version) else {
        return false;
    };
    if Version::parse(&cfg.skip_version) == Some(v) {
        return false;
    }
    if let Some(pin) = Version::parse(&cfg.pin) {
        return v == pin;
    }
    !(p.prerelease && cfg.channel.trim() != "prerelease")
}

/// Forget the waiting update: its files, and GitHub's tags, so the next
/// look reads the answer in full again.
pub fn discard(dir: &Path) {
    if let Some(p) = std::fs::read(dir.join("pending.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Pending>(&b).ok())
        && p.dir.starts_with(dir)
    {
        let _ = std::fs::remove_dir_all(&p.dir);
    }
    let _ = std::fs::remove_file(dir.join("pending.json"));
    let _ = std::fs::remove_file(etag_file(dir));
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

/// Look for a newer release and download it, asking GitHub only "changed?"
/// when nothing changed since the last look.
pub fn check_now(cfg: &UpdateConfig) -> Result<Found> {
    check_with(cfg, &updates_dir(), false, &mut curl_api, &mut |r, d| {
        download(r, d)
    })
}

/// [`check_now`] without the "changed?" shortcut (a look the user asked for).
pub fn check_now_forced(cfg: &UpdateConfig) -> Result<Found> {
    check_in(cfg, &updates_dir(), true)
}

/// A look, keeping what it finds in `dir`.
pub fn check_in(cfg: &UpdateConfig, dir: &Path, force: bool) -> Result<Found> {
    check_with(cfg, dir, force, &mut curl_api, &mut |r, d| download(r, d))
}

type Fetch<'a> = &'a mut dyn FnMut(&str, Option<&str>) -> Result<Fetched>;
type Download<'a> = &'a mut dyn FnMut(&Release, &Path) -> Result<Pending>;

/// One look, with how to ask GitHub and how to download (so a test can
/// stand in for both).
pub fn check_with(
    cfg: &UpdateConfig,
    dir: &Path,
    force: bool,
    fetch: Fetch,
    fetch_zip: Download,
) -> Result<Found> {
    let Some(asset) = asset_name() else {
        return Ok(Found::UpToDate);
    };
    // One look at a time in this process (the watcher, and the panel's
    // "look now"): they would share their files.
    static LOOKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = LOOKING.lock().unwrap_or_else(|e| e.into_inner());
    let url = endpoint(cfg);
    // The tag stands for an answer under these settings and this version:
    // another version, or another skipped one, reads the answer again.
    let key = etag_key(cfg);
    let held = if force { None } else { read_etag(dir, &key) };
    let (body, etag) = match fetch(&url, held.as_deref())? {
        Fetched::NotModified => return Ok(Found::UpToDate),
        Fetched::Body(b, t) => (b, t),
    };
    let json: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| Error::Platform(format!("GitHub's answer isn't JSON: {e}")))?;
    let found = choose(&json, cfg, asset, Version::current())?;
    let result = match found {
        None => Found::UpToDate,
        Some(rel) => {
            write_release_notes(dir, &rel);
            Found::Waiting(fetch_zip(&rel, dir)?)
        }
    };
    // Kept only once this look is over: one that failed is looked at again
    // in full, not answered "nothing new".
    if let Some(t) = etag {
        write_etag(dir, &key, &t);
    }
    Ok(result)
}

/// What a kept tag stands for: the page, this version and the skipped one.
fn etag_key(cfg: &UpdateConfig) -> String {
    format!(
        "{} {} {}",
        endpoint(cfg),
        Version::current(),
        cfg.skip_version.trim()
    )
}

fn etag_file(dir: &Path) -> PathBuf {
    dir.join("etags.json")
}

fn read_etag(dir: &Path, url: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(etag_file(dir)).ok()?).ok()?;
    v.get(url)?.as_str().map(str::to_string)
}

fn write_etag(dir: &Path, url: &str, tag: &str) {
    // One address at a time: a change of channel or pin is another page.
    let _ = std::fs::create_dir_all(dir);
    let text = serde_json::json!({ url: tag }).to_string();
    let tmp = dir.join(format!("etags.json.{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, etag_file(dir));
    }
}

/// What the release a look found says about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseNotes {
    pub version: String,
    pub notes: String,
    pub page: String,
}

fn write_release_notes(dir: &Path, rel: &Release) {
    let n = ReleaseNotes {
        version: rel.version.to_string(),
        notes: rel.notes.clone(),
        page: rel.page.clone(),
    };
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(
        dir.join("release-notes.json"),
        serde_json::to_vec(&n).unwrap_or_default(),
    );
}

/// The notes of the release the last look found, if it is newer than this
/// program.
pub fn release_notes(dir: &Path) -> Option<ReleaseNotes> {
    let n: ReleaseNotes =
        serde_json::from_slice(&std::fs::read(dir.join("release-notes.json")).ok()?).ok()?;
    Version::parse(&n.version)
        .is_some_and(|v| is_newer(v, Version::current()))
        .then_some(n)
}

/// When any server on this computer last looked (`last-check`), as seconds
/// since 1970 (0: never).
pub fn last_check(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("last-check"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Seconds between looks.
pub fn interval_secs(cfg: &UpdateConfig) -> u64 {
    if cfg.check_every_mins > 0 {
        cfg.check_every_mins.max(5).saturating_mul(60)
    } else {
        cfg.check_every_hours.max(1).saturating_mul(3600)
    }
}

/// Until when looks wait because GitHub said "too many requests" (seconds
/// since 1970; 0: not waiting).
pub fn backoff_until(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("backoff-until"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// How long to wait after GitHub refuses a look for asking too often.
const BACKOFF: u64 = 3600;

/// Whether an error is GitHub saying "too many requests".
fn rate_limited(e: &Error) -> bool {
    let m = e.to_string().to_ascii_lowercase();
    // curl -f says "The requested URL returned error: 403"; a number
    // elsewhere in the text (a time, a checksum) doesn't count.
    m.contains("returned error: 403")
        || m.contains("returned error: 429")
        || m.contains("rate limit")
}

/// Look at the first time after `check_after_mins`, then whenever the shared
/// clock says an interval has passed, in a thread of its own: `settings`
/// gives the current settings each time (they may have changed). Servers on
/// one computer share the looks.
pub fn watch(settings: impl Fn() -> UpdateConfig + Send + 'static) {
    let first = settings().check_after_mins;
    let spawned = std::thread::Builder::new()
        .name("updates".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(first.saturating_mul(60)));
            loop {
                let cfg = settings();
                let every = interval_secs(&cfg);
                let dir = updates_dir();
                if cfg.enabled
                    && now_secs() >= backoff_until(&dir)
                    && now_secs().saturating_sub(last_check(&dir)) >= every
                {
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
                        Err(e) => {
                            log::warn!("updates: {e}");
                            if rate_limited(&e) {
                                let _ = std::fs::write(
                                    dir.join("backoff-until"),
                                    (now_secs() + BACKOFF).to_string(),
                                );
                            }
                        }
                    }
                }
                // Look at the shared clock again in a while: often enough
                // for a short interval, rarely for a long one.
                std::thread::sleep(Duration::from_secs((every / 4).clamp(30, 30 * 60)));
            }
        });
    if let Err(e) = spawned {
        log::warn!("updates: can't start looking: {e}");
    }
}

// ---- going back ----------------------------------------------------------------------

/// The version put aside when an update went in: what `rollback` returns to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Previous {
    pub version: String,
    /// When it was put aside (seconds since 1970).
    pub saved: u64,
    /// Its settings file was kept with it (`previous/config.toml`).
    #[serde(default)]
    pub settings: bool,
}

/// The settings file kept with the version an update replaced.
const KEPT_SETTINGS: &str = "config.toml";

/// The settings the version `previous` ran with, when they were kept.
pub fn previous_settings(dir: &Path) -> Option<PathBuf> {
    previous(dir)
        .filter(|p| p.settings)
        .map(|_| previous_dir(dir).join(KEPT_SETTINGS))
        .filter(|f| f.is_file())
}

fn previous_dir(dir: &Path) -> PathBuf {
    dir.join("previous")
}

/// The version an update replaced, kept to go back to (one only).
pub fn previous(dir: &Path) -> Option<Previous> {
    let p: Previous =
        serde_json::from_slice(&std::fs::read(previous_dir(dir).join("previous.json")).ok()?)
            .ok()?;
    Version::of_program(&p.version)?;
    previous_dir(dir).join(BIN_NAME).is_file().then_some(p)
}

/// Keep the program at `exe` (this version) before an update replaces it,
/// and the settings file at `config` (as the next version may add values
/// this one doesn't know, or change what they mean).
fn keep_previous(exe: &Path, dir: &Path, config: Option<&Path>) {
    let keep = previous_dir(dir);
    let done = (|| -> std::io::Result<()> {
        let _ = std::fs::remove_dir_all(&keep);
        std::fs::create_dir_all(&keep)?;
        std::fs::copy(exe, keep.join(BIN_NAME))?;
        // The settings are kept as they are, secrets and all, under the
        // same owner-only rule as the file they came from.
        let settings = config.is_some_and(|c| {
            let kept = keep.join(KEPT_SETTINGS);
            std::fs::copy(c, &kept).is_ok() && {
                crate::config::owner_only(&kept);
                true
            }
        });
        // The program on disk, which may be newer than the one running.
        let p = Previous {
            version: said_version(exe).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            saved: now_secs(),
            settings,
        };
        std::fs::write(
            keep.join("previous.json"),
            serde_json::to_vec(&p).unwrap_or_default(),
        )
    })();
    if let Err(e) = done {
        log::warn!(
            "updates: couldn't keep {} to go back to: {e}",
            Version::current()
        );
        let _ = std::fs::remove_dir_all(&keep);
    }
}

/// Put the version an update replaced back in place of the program at
/// `exe`; (the version it was, the version it is now). The update it undid
/// is cleared; set `update.skip_version` to it so it isn't taken again.
pub fn rollback(exe: &Path, dir: &Path) -> Result<(Version, Version)> {
    rollback_to(exe, dir, None).map(|(from, to, _)| (from, to))
}

/// `rollback`, and when `settings` is the settings file, the one kept with
/// the old version put back in its place (what it replaces is kept beside it
/// with `.before-rollback` added, so nothing is lost). The third value is
/// what came of the settings: `Some` when they were put back.
pub fn rollback_to(
    exe: &Path,
    dir: &Path,
    settings: Option<&Path>,
) -> Result<(Version, Version, Option<SettingsBack>)> {
    let prev = previous(dir)
        .ok_or_else(|| Error::Platform("there is no earlier version kept to go back to".into()))?;
    let bin = previous_dir(dir).join(BIN_NAME);
    let said = run_version(&bin)?;
    let want = Version::of_program(&prev.version);
    if want.is_none()
        || Version::of_program(said.split_whitespace().last().unwrap_or_default()) != want
    {
        return Err(Error::Platform(format!(
            "the kept program says it is \"{said}\", not {}: not put back",
            prev.version
        )));
    }
    // What is being replaced is the program at `exe`, which may be newer
    // than the one running (an update went in since it started).
    let from = run_version(exe)
        .ok()
        .and_then(|said| Version::of_program(said.split_whitespace().last().unwrap_or_default()))
        .unwrap_or_else(Version::current);
    let kept = settings.and_then(|_| previous_settings(dir));
    replace_program(&bin, exe)?;
    let back = match (settings, kept) {
        (Some(config), Some(kept)) => put_back_settings(&kept, config),
        _ => None,
    };
    discard(dir);
    let _ = std::fs::remove_dir_all(previous_dir(dir));
    Ok((from, want.unwrap_or(from), back))
}

/// The settings of the old version, put back.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsBack {
    /// Where the settings they replaced were kept (none when there were
    /// none to replace).
    pub aside: Option<PathBuf>,
}

/// Replace the settings file at `config` with `kept`; the one it replaces is
/// kept in `<config>.before-rollback`. Whatever fails leaves the file as
/// it was: the new file is written beside it and moved over it.
fn put_back_settings(kept: &Path, config: &Path) -> Option<SettingsBack> {
    let name = config.file_name()?.to_string_lossy().into_owned();
    let aside = config.with_file_name(format!("{name}.before-rollback"));
    let staged = config.with_file_name(format!("{name}.rolling-back"));
    let had = config.is_file();
    let done = (|| -> std::io::Result<()> {
        if had {
            std::fs::copy(config, &aside)?;
            crate::config::owner_only(&aside);
        }
        std::fs::copy(kept, &staged)?;
        crate::config::owner_only(&staged);
        std::fs::rename(&staged, config)
    })();
    match done {
        Ok(()) => Some(SettingsBack {
            aside: had.then_some(aside),
        }),
        Err(e) => {
            log::warn!("updates: couldn't put the old settings back: {e}");
            let _ = std::fs::remove_file(&staged);
            None
        }
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
        assert_eq!(
            Version::current().to_string(),
            env!("CARGO_PKG_VERSION").split('-').next().unwrap()
        );
        // A program's own version may carry a label; a tag may not.
        assert_eq!(Version::of_program("5.0.0-preview"), Some(Version(5, 0, 0)));
        assert_eq!(Version::of_program("5.0.0+build.3"), Some(Version(5, 0, 0)));
        assert_eq!(Version::parse("v5.0.0-preview"), None);
    }

    #[test]
    fn a_preview_takes_the_finished_release_of_its_numbers() {
        let cur = Version::current();
        assert!(is_newer(Version(cur.0, cur.1, cur.2 + 1), cur));
        assert!(!is_newer(Version(0, 0, 1), cur));
        // The same numbers: newer only for a preview build.
        assert_eq!(is_newer(cur, cur), current_is_preview());
        // Another "current" (a test's own) never gets that.
        assert!(!is_newer(Version(1, 2, 3), Version(1, 2, 3)));
        // And never the version 0.0.0 that an unreadable version once was.
        assert_ne!(cur, Version(0, 0, 0));
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

    fn cfg_with(f: impl FnOnce(&mut UpdateConfig)) -> UpdateConfig {
        let mut c = UpdateConfig::default();
        f(&mut c);
        c
    }

    const ASSET: &str = "computer-use-mcp-linux-x64.zip";

    /// This system's zip (a look takes only that), or Linux's elsewhere.
    fn asset() -> &'static str {
        asset_name().unwrap_or(ASSET)
    }

    fn rel(tag: &str, pre: bool) -> serde_json::Value {
        let url = format!(
            "https://github.com/mhrsdev/zero-use-computer/releases/download/{tag}/{}",
            asset()
        );
        let mut j = release_json(tag, Some(&format!("sha256:{}", "cd".repeat(32))), &url);
        j["assets"][1]["name"] = asset().into();
        j["prerelease"] = pre.into();
        j["body"] = format!("Notes of {tag}").into();
        j["html_url"] =
            format!("https://github.com/mhrsdev/zero-use-computer/releases/tag/{tag}").into();
        j
    }

    #[test]
    fn the_channel_pin_and_skip_decide_which_release_is_taken() {
        let now = Version(4, 0, 1);
        let all = serde_json::json!([
            rel("v4.9.0-rc.1", true),
            rel("v4.8.0", false),
            rel("v4.7.0", false)
        ]);
        // "v4.9.0-rc.1" isn't a version this program reads: it is passed over.
        let stable = cfg_with(|_| {});
        assert!(endpoint(&stable).ends_with("/releases/latest"));
        assert_eq!(
            choose(&rel("v4.8.0", false), &stable, asset(), now)
                .unwrap()
                .unwrap()
                .version,
            Version(4, 8, 0)
        );
        assert_eq!(
            choose(&rel("v4.9.0", true), &stable, asset(), now).unwrap(),
            None
        );
        let pre = cfg_with(|c| c.channel = "prerelease".into());
        assert!(endpoint(&pre).contains("/releases?per_page=30"));
        assert_eq!(
            choose(&all, &pre, asset(), now).unwrap().unwrap().version,
            Version(4, 8, 0)
        );
        let all = serde_json::json!([rel("v4.9.0", true), rel("v4.8.0", false)]);
        assert_eq!(
            choose(&all, &pre, asset(), now).unwrap().unwrap().version,
            Version(4, 9, 0)
        );
        // Skipped: the next one down.
        let skip = cfg_with(|c| {
            c.channel = "prerelease".into();
            c.skip_version = "4.9.0".into();
        });
        assert_eq!(
            choose(&all, &skip, asset(), now).unwrap().unwrap().version,
            Version(4, 8, 0)
        );
        // Stable with the newest skipped: nothing (it isn't looking further back).
        let skip = cfg_with(|c| c.skip_version = "4.8.0".into());
        assert_eq!(
            choose(&rel("v4.8.0", false), &skip, asset(), now).unwrap(),
            None
        );
        // Pinned: that release, a pre-release too, and nothing else.
        let pin = cfg_with(|c| c.pin = "4.7.0".into());
        assert!(endpoint(&pin).ends_with("/releases/tags/v4.7.0"));
        assert_eq!(
            choose(&rel("v4.7.0", false), &pin, asset(), now)
                .unwrap()
                .unwrap()
                .version,
            Version(4, 7, 0)
        );
        assert_eq!(
            choose(&rel("v4.7.0", true), &pin, asset(), now)
                .unwrap()
                .unwrap()
                .version,
            Version(4, 7, 0)
        );
        // Already there or beyond it: nothing.
        assert_eq!(
            choose(&rel("v4.7.0", false), &pin, asset(), Version(4, 7, 0)).unwrap(),
            None
        );
        assert_eq!(
            choose(&rel("v4.7.0", false), &pin, asset(), Version(4, 8, 0)).unwrap(),
            None
        );
        // The notes and page come along, the page only from the repository.
        let r = choose(&rel("v4.8.0", false), &stable, asset(), now)
            .unwrap()
            .unwrap();
        assert_eq!(r.notes, "Notes of v4.8.0");
        assert!(r.page.ends_with("/releases/tag/v4.8.0"));
        let mut odd = rel("v4.8.0", false);
        odd["html_url"] = "https://evil.example/x".into();
        assert_eq!(
            choose(&odd, &stable, asset(), now).unwrap().unwrap().page,
            ""
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cu-upd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pending_for(rel: &Release, dir: &Path) -> Pending {
        Pending {
            version: rel.version.to_string(),
            dir: dir.join("x"),
            downloaded: 1,
            sha256: rel.sha256.clone(),
            prerelease: rel.prerelease,
        }
    }

    #[test]
    fn a_look_asks_only_whether_it_changed_and_keeps_the_tag_when_it_is_over() {
        let dir = temp_dir("etag");
        let cfg = cfg_with(|_| {});
        let newer = serde_json::to_vec(&rel("v99.0.0", false)).unwrap();
        let mut asked: Vec<Option<String>> = Vec::new();
        let mut answers = vec![
            Fetched::Body(newer.clone(), Some("\"one\"".into())),
            Fetched::NotModified,
            Fetched::Body(newer.clone(), Some("\"two\"".into())),
        ]
        .into_iter();
        let mut fetch = |_: &str, tag: Option<&str>| {
            asked.push(tag.map(str::to_string));
            Ok(answers.next().unwrap())
        };
        // A download that fails: the tag isn't kept, so it is looked at again in full.
        let mut fail =
            |_: &Release, _: &Path| -> Result<Pending> { Err(Error::Platform("no".into())) };
        assert!(check_with(&cfg, &dir, false, &mut fetch, &mut fail).is_err());
        assert!(read_etag(&dir, &etag_key(&cfg)).is_none());
        // One that works keeps it, and the next look asks "changed?".
        let mut ok = |r: &Release, d: &Path| -> Result<Pending> { Ok(pending_for(r, d)) };
        let mut again = vec![
            Fetched::Body(newer.clone(), Some("\"one\"".into())),
            Fetched::NotModified,
        ]
        .into_iter();
        let mut asked2: Vec<Option<String>> = Vec::new();
        let mut fetch2 = |_: &str, tag: Option<&str>| {
            asked2.push(tag.map(str::to_string));
            Ok(again.next().unwrap())
        };
        assert!(matches!(
            check_with(&cfg, &dir, false, &mut fetch2, &mut ok).unwrap(),
            Found::Waiting(_)
        ));
        assert_eq!(read_etag(&dir, &etag_key(&cfg)).as_deref(), Some("\"one\""));
        assert_eq!(
            check_with(&cfg, &dir, false, &mut fetch2, &mut ok).unwrap(),
            Found::UpToDate
        );
        // A look the user asked for sends no tag.
        let mut third = vec![Fetched::Body(newer, Some("\"two\"".into()))].into_iter();
        let mut sent: Vec<Option<String>> = Vec::new();
        let mut fetch3 = |_: &str, tag: Option<&str>| {
            sent.push(tag.map(str::to_string));
            Ok(third.next().unwrap())
        };
        check_with(&cfg, &dir, true, &mut fetch3, &mut ok).unwrap();
        assert_eq!(asked2, [None, Some("\"one\"".to_string())]);
        assert_eq!(sent, [None]);
        // The notes of what it found.
        assert_eq!(release_notes(&dir).unwrap().version, "99.0.0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn looks_come_as_often_as_asked_and_a_refusal_waits() {
        assert_eq!(interval_secs(&cfg_with(|_| {})), 12 * 3600);
        assert_eq!(interval_secs(&cfg_with(|c| c.check_every_hours = 0)), 3600);
        assert_eq!(interval_secs(&cfg_with(|c| c.check_every_mins = 10)), 600);
        assert_eq!(interval_secs(&cfg_with(|c| c.check_every_mins = 1)), 300);
        assert!(rate_limited(&Error::Platform(
            "could not fetch x: The requested URL returned error: 403".into()
        )));
        assert!(rate_limited(&Error::Platform(
            "API rate limit exceeded".into()
        )));
        assert!(!rate_limited(&Error::Platform(
            "could not resolve host".into()
        )));
        let dir = temp_dir("backoff");
        assert_eq!(backoff_until(&dir), 0);
        assert_eq!(last_check(&dir), 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_version_an_update_replaced_can_be_put_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("rollback");
        let exe = dir.join(BIN_NAME);
        let script = |v: &str| format!("#!/bin/sh\necho computer-use-mcp {v}\n");
        let put = |path: &Path, v: &str| {
            std::fs::write(path, script(v)).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // Nothing kept yet.
        assert!(previous(&dir).is_none());
        assert!(rollback(&exe, &dir).is_err());
        // An update goes in: the running program is kept first.
        put(&exe, &Version::current().to_string());
        let newer = dir.join("v99.0.0");
        std::fs::create_dir_all(&newer).unwrap();
        put(&newer.join(BIN_NAME), "99.0.0");
        let p = Pending {
            version: "99.0.0".into(),
            dir: newer,
            downloaded: 1,
            sha256: String::new(),
            prerelease: false,
        };
        install(&p, &exe, &dir).unwrap();
        assert!(std::fs::read_to_string(&exe).unwrap().contains("99.0.0"));
        let kept = previous(&dir).unwrap();
        assert_eq!(kept.version, Version::current().to_string());
        // Put back.
        let (from, to) = rollback(&exe, &dir).unwrap();
        assert_eq!((from, to), (Version(99, 0, 0), Version::current()));
        assert!(
            std::fs::read_to_string(&exe)
                .unwrap()
                .contains(&Version::current().to_string())
        );
        assert!(previous(&dir).is_none());
        // A kept program that isn't the version it says is not put back.
        let keep = previous_dir(&dir);
        std::fs::create_dir_all(&keep).unwrap();
        put(&keep.join(BIN_NAME), "1.2.3");
        std::fs::write(
            keep.join("previous.json"),
            serde_json::to_vec(&Previous {
                version: "4.0.0".into(),
                saved: 1,
                settings: false,
            })
            .unwrap(),
        )
        .unwrap();
        assert!(rollback(&exe, &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_settings_an_update_replaced_come_back_with_the_program() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("rollback-settings");
        let exe = dir.join(BIN_NAME);
        let config = dir.join("config.toml");
        let put = |path: &Path, v: &str| {
            std::fs::write(path, format!("#!/bin/sh\necho computer-use-mcp {v}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        put(&exe, &Version::current().to_string());
        std::fs::write(&config, "[overlay]\ncursor_style = \"ice\"\n").unwrap();
        let newer = dir.join("v99.0.0");
        std::fs::create_dir_all(&newer).unwrap();
        put(&newer.join(BIN_NAME), "99.0.0");
        let p = Pending {
            version: "99.0.0".into(),
            dir: newer,
            downloaded: 1,
            sha256: String::new(),
            prerelease: false,
        };
        // Without being told, no settings are kept.
        assert!(previous_settings(&dir).is_none());
        install_keeping(&p, &exe, &dir, Some(&config)).unwrap();
        let kept = previous_settings(&dir).expect("kept with the program");
        assert_eq!(std::fs::read_to_string(kept).unwrap(), "[overlay]\ncursor_style = \"ice\"\n");
        assert!(previous(&dir).unwrap().settings);
        // The newer version's settings, then back.
        std::fs::write(&config, "[overlay]\ncursor_style = \"jelly\"\n[future]\nx = 1\n").unwrap();
        let (from, to, back) = rollback_to(&exe, &dir, Some(&config)).unwrap();
        assert_eq!((from, to), (Version(99, 0, 0), Version::current()));
        let back = back.expect("the settings came back");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "[overlay]\ncursor_style = \"ice\"\n");
        let aside = back.aside.expect("the ones replaced are kept");
        assert!(std::fs::read_to_string(aside).unwrap().contains("[future]"));
        assert!(!dir.join("config.toml.rolling-back").exists());
        assert!(previous(&dir).is_none());
        // An update put in without settings keeps none, and going back with
        // a file named leaves it alone.
        let newer = dir.join("v99.0.1");
        std::fs::create_dir_all(&newer).unwrap();
        put(&newer.join(BIN_NAME), "99.0.1");
        let p = Pending {
            version: "99.0.1".into(),
            dir: newer,
            ..p
        };
        install(&p, &exe, &dir).unwrap();
        assert!(previous_settings(&dir).is_none());
        std::fs::write(&config, "mine = 1\n").unwrap();
        let (_, _, back) = rollback_to(&exe, &dir, Some(&config)).unwrap();
        assert!(back.is_none());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "mine = 1\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_waiting_update_goes_in_only_while_the_settings_still_allow_it() {
        let p = |v: &str, pre: bool| Pending {
            version: v.into(),
            dir: PathBuf::from("/x"),
            downloaded: 1,
            sha256: String::new(),
            prerelease: pre,
        };
        let cfg = |f: &dyn Fn(&mut UpdateConfig)| {
            let mut c = UpdateConfig::default();
            f(&mut c);
            c
        };
        assert!(allowed(&p("9.1.0", false), &cfg(&|_| {})));
        assert!(!allowed(
            &p("9.1.0", false),
            &cfg(&|c| c.skip_version = "9.1".into())
        ));
        assert!(!allowed(
            &p("9.1.0", false),
            &cfg(&|c| c.pin = "9.0.5".into())
        ));
        assert!(allowed(
            &p("9.0.5", true),
            &cfg(&|c| c.pin = "9.0.5".into())
        ));
        assert!(!allowed(&p("9.2.0", true), &cfg(&|_| {})));
        assert!(allowed(
            &p("9.2.0", true),
            &cfg(&|c| c.channel = "prerelease".into())
        ));
        assert!(!allowed(&p("not a version", false), &cfg(&|_| {})));
        // An old pending.json without the field reads as a proper release.
        let old: Pending =
            serde_json::from_str(r#"{"version":"9.1.0","dir":"/x","downloaded":1,"sha256":""}"#)
                .unwrap();
        assert!(!old.prerelease);
    }

    #[test]
    fn only_a_refusal_from_github_waits_an_hour() {
        let e = |m: &str| Error::Platform(m.into());
        assert!(rate_limited(&e(
            "could not fetch x: The requested URL returned error: 403"
        )));
        assert!(rate_limited(&e(
            "could not fetch x: The requested URL returned error: 429"
        )));
        assert!(rate_limited(&e("API rate limit exceeded for 1.2.3.4")));
        assert!(!rate_limited(&e(
            "curl: (28) Failed to connect to api.github.com port 443 after 21403 ms"
        )));
        assert!(!rate_limited(&e(
            "the download's SHA-256 is ab403cd, GitHub says 4291: not taken"
        )));
    }

    #[test]
    fn forgetting_a_waiting_update_forgets_githubs_tag_too() {
        let dir = temp_dir("discard");
        let waiting = dir.join("v9.9.9");
        std::fs::create_dir_all(&waiting).unwrap();
        write_pending(
            &dir,
            &Pending {
                version: "9.9.9".into(),
                dir: waiting.clone(),
                downloaded: 1,
                sha256: String::new(),
                prerelease: false,
            },
        )
        .unwrap();
        write_etag(&dir, "k", "\"t\"");
        discard(&dir);
        assert!(
            !waiting.exists()
                && !dir.join("pending.json").exists()
                && read_etag(&dir, "k").is_none()
        );
        // A pending.json pointing outside the folder: the folder there stays.
        let outside = temp_dir("discard-outside");
        write_pending(
            &dir,
            &Pending {
                version: "9.9.9".into(),
                dir: outside.clone(),
                downloaded: 1,
                sha256: String::new(),
                prerelease: false,
            },
        )
        .unwrap();
        discard(&dir);
        assert!(outside.exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn installing_twice_keeps_the_version_to_go_back_to() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("twice");
        let exe = dir.join(BIN_NAME);
        let put = |path: &Path, v: &str| {
            std::fs::write(path, format!("#!/bin/sh\necho computer-use-mcp {v}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        put(&exe, "5.0.0");
        let pending_of = |v: &str| {
            let d = dir.join(format!("v{v}"));
            std::fs::create_dir_all(&d).unwrap();
            put(&d.join(BIN_NAME), v);
            Pending {
                version: v.into(),
                dir: d,
                downloaded: 1,
                sha256: String::new(),
                prerelease: false,
            }
        };
        install(&pending_of("99.1.0"), &exe, &dir).unwrap();
        assert_eq!(previous(&dir).unwrap().version, "5.0.0");
        // The same update again (this server still thinks it is older):
        // nothing replaced, the kept 5.0.0 stays.
        install(&pending_of("99.1.0"), &exe, &dir).unwrap();
        assert_eq!(previous(&dir).unwrap().version, "5.0.0");
        // A newer one: the kept copy is the one on disk, named by itself.
        install(&pending_of("99.2.0"), &exe, &dir).unwrap();
        assert_eq!(previous(&dir).unwrap().version, "99.1.0");
        assert_eq!(
            rollback(&exe, &dir).unwrap(),
            (Version(99, 2, 0), Version(99, 1, 0))
        );
        let _ = std::fs::remove_dir_all(&dir);
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
            prerelease: false,
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
            prerelease: false,
        };
        write_pending(&dir, &p).unwrap();
        assert!(pending(&dir).is_none());
        assert!(!unpacked.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
