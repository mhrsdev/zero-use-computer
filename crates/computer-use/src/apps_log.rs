//! How often each app needed pixels: windows with little in their
//! accessibility tree, text read off the screen, screenshots sent and left
//! out. Kept across sessions in `apps.json` in the server's folder, so
//! `doctor` can say which apps the tree doesn't serve well.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// One app's counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppRecord {
    /// Looks at its windows (get_app_state).
    pub looks: u64,
    /// Looks at a window with almost nothing to act on in its tree, or an
    /// area the tree says nothing about (a canvas).
    pub little_tree: u64,
    /// Looks that read text off the screen.
    pub text_read: u64,
    /// Screenshots sent with a look (whole or the part that changed).
    pub pictures: u64,
    /// Automatic screenshots left out because the tree said what changed.
    pub pictures_left_out: u64,
}

impl AppRecord {
    /// Share of looks the tree alone didn't serve.
    pub fn share_little(&self) -> f64 {
        self.little_tree as f64 / self.looks.max(1) as f64
    }

    fn add(&mut self, other: &AppRecord) {
        self.looks += other.looks;
        self.little_tree += other.little_tree;
        self.text_read += other.text_read;
        self.pictures += other.pictures;
        self.pictures_left_out += other.pictures_left_out;
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    apps: BTreeMap<String, AppRecord>,
}

/// The most apps kept: the ones looked at least are dropped.
const MAX_APPS: usize = 200;
/// Written at most this often while the server runs (and when it stops).
const WRITE_EVERY: Duration = Duration::from_secs(10);

/// The counts, and where they are kept (none: counted, never written).
/// Several servers may share the file (agents side by side): each adds
/// what it counted since its last write to what the file holds then,
/// one at a time (see [`WriteLock`]).
#[derive(Debug, Default)]
pub struct AppsLog {
    path: Option<PathBuf>,
    apps: BTreeMap<String, AppRecord>,
    /// Counted since the last write.
    added: BTreeMap<String, AppRecord>,
    /// Looked at while this server runs.
    used: std::collections::BTreeSet<String>,
    written: Option<Instant>,
}

impl AppsLog {
    /// The counts kept at `path`, to add to.
    pub fn at(path: PathBuf) -> Self {
        let apps = read(&path);
        Self {
            path: Some(path),
            apps,
            added: BTreeMap::new(),
            used: Default::default(),
            written: None,
        }
    }

    /// Add to `app`'s counts (`add` only adds).
    pub fn note(&mut self, app: &str, add: impl FnOnce(&mut AppRecord)) {
        if self.path.is_none() {
            return;
        }
        let mut more = AppRecord::default();
        add(&mut more);
        self.apps.entry(app.to_string()).or_default().add(&more);
        self.added.entry(app.to_string()).or_default().add(&more);
        if !self.used.contains(app) {
            self.used.insert(app.to_string());
        }
    }

    /// Write the counts if they changed and the last write is a while ago
    /// (or `now` is None: write now).
    pub fn flush(&mut self, now: Option<Instant>) {
        let Some(path) = &self.path else {
            return;
        };
        if self.added.is_empty() {
            return;
        }
        if let (Some(now), Some(at)) = (now, self.written)
            && now.duration_since(at) < WRITE_EVERY
        {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Another server writing now is waited for; if it takes too long,
        // this write is left for the next flush (the last one still writes).
        // The last write waits longer: past the time a lock counts as left
        // behind, so it never writes beside another server.
        let wait = if now.is_some() { 1 } else { 6 };
        let lock = WriteLock::take(path, Duration::from_secs(wait));
        if lock.is_none() && now.is_some() {
            return;
        }
        // What the file holds now (another server may have written since),
        // plus what this one counted. Those stay counted until written.
        let mut apps = read(path);
        for (app, more) in &self.added {
            apps.entry(app.clone()).or_default().add(more);
        }
        if apps.len() > MAX_APPS {
            // The ones looked at least go, but not the ones just used (a
            // new app has few looks, and would never be kept).
            let mut by_looks: Vec<(bool, u64, String)> = apps
                .iter()
                .map(|(k, r)| (self.used.contains(k), r.looks, k.clone()))
                .collect();
            by_looks.sort();
            let extra = apps.len() - MAX_APPS;
            for (_, _, k) in by_looks.into_iter().take(extra) {
                apps.remove(&k);
            }
        }
        let file = File { apps };
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            // Unique per write: servers writing at once never share it.
            static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = path.with_extension(format!("json.{}-{n}.tmp", std::process::id()));
            let saved = std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, path).is_ok();
            if saved {
                self.added.clear();
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        drop(lock);
        self.apps = file.apps;
        // Also after a failed write, so a full disk is tried again only
        // every few seconds.
        self.written = now.or_else(|| Some(Instant::now()));
    }

    #[cfg(test)]
    pub(crate) fn record(&self, app: &str) -> Option<AppRecord> {
        self.apps.get(app).copied()
    }
}

impl Drop for AppsLog {
    fn drop(&mut self) {
        self.flush(None);
    }
}

/// `apps.json.lock`, held while one server reads, adds to and replaces
/// the file, so two servers writing at the same moment keep both counts.
struct WriteLock(PathBuf);

impl WriteLock {
    /// Waits up to `wait` for another server's write. A lock left by a
    /// server that stopped mid-write (a write takes milliseconds) is taken
    /// over after a few seconds.
    fn take(path: &Path, wait: Duration) -> Option<Self> {
        let lock = path.with_extension("json.lock");
        let until = Instant::now() + wait;
        while Instant::now() < until {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
            {
                Ok(_) => return Some(Self(lock)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&lock)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(5));
                    if stale {
                        let _ = std::fs::remove_file(&lock);
                    } else {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn read(path: &Path) -> BTreeMap<String, AppRecord> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<File>(&t).ok())
        .map(|f| f.apps)
        .unwrap_or_default()
}

/// The apps that needed pixels most, as lines for `doctor`: those with
/// at least a few looks, the least served by their tree first.
pub fn summary(path: &Path, max: usize) -> Vec<String> {
    let mut apps: Vec<(String, AppRecord)> = read(path)
        .into_iter()
        .filter(|(_, r)| r.looks >= 3 && r.little_tree > 0)
        .collect();
    apps.sort_by(|a, b| {
        b.1.share_little()
            .total_cmp(&a.1.share_little())
            .then(b.1.looks.cmp(&a.1.looks))
    });
    apps.into_iter()
        .take(max)
        .map(|(name, r)| {
            format!(
                "{name}: {}% of {} looks with little in the tree; text read off the screen {}x; screenshots {} sent, {} left out",
                (r.share_little() * 100.0).round(),
                r.looks,
                r.text_read,
                r.pictures,
                r.pictures_left_out
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_kept_and_summed_across_sessions() {
        let dir = std::env::temp_dir().join(format!("cu-apps-log-{}", std::process::id()));
        let path = dir.join("apps.json");
        let _ = std::fs::remove_file(&path);
        {
            let mut log = AppsLog::at(path.clone());
            for _ in 0..4 {
                log.note("Paint", |r| {
                    r.looks += 1;
                    r.little_tree += 1;
                });
            }
            log.note("Settings", |r| r.looks += 1);
        }
        let mut log = AppsLog::at(path.clone());
        log.note("Paint", |r| {
            r.looks += 1;
            r.pictures += 1;
        });
        log.flush(None);
        let paint = log.record("Paint").unwrap();
        assert_eq!((paint.looks, paint.little_tree, paint.pictures), (5, 4, 1));
        let lines = summary(&path, 5);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("Paint: 80% of 5 looks"), "{lines:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn servers_writing_at_the_same_moment_keep_every_count() {
        let dir = std::env::temp_dir().join(format!("cu-apps-log-race-{}", std::process::id()));
        let path = dir.join("apps.json");
        let _ = std::fs::remove_dir_all(&dir);
        let servers: Vec<_> = (0..6)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut log = AppsLog::at(path);
                    for _ in 0..20 {
                        log.note("Paint", |r| r.looks += 1);
                        log.flush(None);
                    }
                })
            })
            .collect();
        for s in servers {
            s.join().unwrap();
        }
        assert_eq!(read(&path)["Paint"].looks, 120);
        assert!(!path.with_extension("json.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_a_write_failed_to_save_are_written_later() {
        let dir = std::env::temp_dir().join(format!("cu-apps-log-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The log's folder is a file for now, so writing fails.
        let blocker = dir.join("logs");
        std::fs::write(&blocker, "").unwrap();
        let path = blocker.join("apps.json");
        let mut log = AppsLog::at(path.clone());
        log.note("Paint", |r| r.looks += 2);
        log.flush(None);
        assert!(!path.exists());
        std::fs::remove_file(&blocker).unwrap();
        log.flush(None);
        assert_eq!(read(&path)["Paint"].looks, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_servers_writing_one_after_the_other_keep_both_counts() {
        let dir = std::env::temp_dir().join(format!("cu-apps-log-two-{}", std::process::id()));
        let path = dir.join("apps.json");
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = AppsLog::at(path.clone());
        let mut b = AppsLog::at(path.clone());
        a.note("Paint", |r| r.looks += 2);
        b.note("Paint", |r| r.looks += 3);
        b.note("Settings", |r| r.looks += 1);
        a.flush(None);
        b.flush(None);
        a.note("Paint", |r| r.looks += 1);
        a.flush(None);
        let all = read(&path);
        assert_eq!(all["Paint"].looks, 6);
        assert_eq!(all["Settings"].looks, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_app_is_kept_when_the_log_is_full() {
        let dir = std::env::temp_dir().join(format!("cu-apps-log-full-{}", std::process::id()));
        let path = dir.join("apps.json");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut old = AppsLog::at(path.clone());
            for i in 0..MAX_APPS {
                old.note(&format!("app{i}"), |r| r.looks += 5);
            }
        }
        let mut log = AppsLog::at(path.clone());
        log.note("New", |r| r.looks += 1);
        log.flush(None);
        let all = read(&path);
        assert_eq!(all.len(), MAX_APPS);
        assert!(all.contains_key("New"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_written_without_a_place() {
        let mut log = AppsLog::default();
        log.note("Paint", |r| r.looks += 1);
        log.flush(None);
        assert_eq!(log.record("Paint"), None);
    }
}
