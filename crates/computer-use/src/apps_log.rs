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
#[derive(Debug, Default)]
pub struct AppsLog {
    path: Option<PathBuf>,
    apps: BTreeMap<String, AppRecord>,
    dirty: bool,
    written: Option<Instant>,
}

impl AppsLog {
    /// The counts kept at `path`, to add to.
    pub fn at(path: PathBuf) -> Self {
        let apps = read(&path);
        Self {
            path: Some(path),
            apps,
            dirty: false,
            written: None,
        }
    }

    /// Add to `app`'s counts.
    pub fn note(&mut self, app: &str, add: impl FnOnce(&mut AppRecord)) {
        if self.path.is_none() {
            return;
        }
        add(self.apps.entry(app.to_string()).or_default());
        self.dirty = true;
    }

    /// Write the counts if they changed and the last write is a while ago
    /// (or `now` is None: write now).
    pub fn flush(&mut self, now: Option<Instant>) {
        let Some(path) = &self.path else {
            return;
        };
        if !self.dirty {
            return;
        }
        if let (Some(now), Some(at)) = (now, self.written)
            && now.duration_since(at) < WRITE_EVERY
        {
            return;
        }
        if self.apps.len() > MAX_APPS {
            let mut by_looks: Vec<(u64, String)> = self
                .apps
                .iter()
                .map(|(k, r)| (r.looks, k.clone()))
                .collect();
            by_looks.sort();
            for (_, k) in by_looks.iter().take(self.apps.len() - MAX_APPS) {
                self.apps.remove(k);
            }
        }
        let file = File {
            apps: self.apps.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            let tmp = path.with_extension("json.tmp");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
        self.dirty = false;
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
    fn nothing_is_written_without_a_place() {
        let mut log = AppsLog::default();
        log.note("Paint", |r| r.looks += 1);
        log.flush(None);
        assert_eq!(log.record("Paint"), None);
    }
}
