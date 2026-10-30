//! The user's Stop, kept across restarts of the program a host supervises.
//!
//! The stop flag lives in memory, so a program that is killed and started
//! again (a host restarts it, or it crashes) would come back not stopped and
//! let the agent act with nobody having pressed Resume. A host-started program
//! therefore keeps a marker file for as long as the flag is set, and starts
//! stopped when it finds one. Only the user's own act clears it: Continue on
//! the overlay, or the host's Resume (`computer_use/resume`). The stop key
//! only ever stops.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How soon a change of the flag reaches the file.
const EVERY: Duration = Duration::from_millis(50);

/// Follows the stop flag into the marker file.
pub struct Hold {
    path: PathBuf,
    flag: Arc<AtomicBool>,
    /// What the file says now, and whether writing it failed since.
    written: Mutex<(bool, bool)>,
}

impl Hold {
    /// Whether the marker at `path` says the user stopped the agent in an
    /// earlier run. A marker that can't be looked at counts as set: not being
    /// sure is not a reason to let the agent act.
    pub fn was_held(path: &std::path::Path) -> bool {
        path.try_exists().unwrap_or(true)
    }

    /// Keep the marker at `path` the same as `flag` from now on, `held` being
    /// what it says now. Call [`Hold::sync`] once more before the program ends.
    pub fn follow(path: PathBuf, flag: Arc<AtomicBool>, held: bool) -> Arc<Self> {
        let hold = Arc::new(Self {
            path,
            flag,
            written: Mutex::new((held, false)),
        });
        let keeper = hold.clone();
        let started = std::thread::Builder::new()
            .name("stop-hold".into())
            .spawn(move || {
                loop {
                    keeper.sync();
                    std::thread::sleep(EVERY);
                }
            });
        if let Err(e) = started {
            // The stop works either way; only a restart would lose it.
            log::warn!("the stop will not be kept over a restart: {e}");
        }
        hold
    }

    /// Write the marker if the flag changed since it was last written.
    pub fn sync(&self) {
        let now = self.flag.load(Ordering::SeqCst);
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        if written.0 == now {
            return;
        }
        match self.write(now) {
            Ok(()) => *written = (now, false),
            // Tried again at the next turn; said once.
            Err(e) if !written.1 => {
                written.1 = true;
                log::warn!(
                    "the stop could not be written to {}: {e}; it holds until the program ends but a restart would lose it",
                    self.path.display()
                );
            }
            Err(_) => {}
        }
    }

    fn write(&self, held: bool) -> io::Result<()> {
        if held {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            return std::fs::write(&self.path, "the user stopped the agent\n");
        }
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cu-hold-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("stop-held")
    }

    /// A [`Hold`] with no thread, driven by `sync`.
    fn hold(path: PathBuf, held: bool) -> (Hold, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(held));
        let hold = Hold {
            path,
            flag: flag.clone(),
            written: Mutex::new((held, false)),
        };
        (hold, flag)
    }

    #[test]
    fn the_marker_follows_the_flag_both_ways() {
        let path = marker("follows");
        assert!(!Hold::was_held(&path));
        let (hold, flag) = hold(path.clone(), false);
        hold.sync();
        assert!(!path.exists());
        flag.store(true, Ordering::SeqCst);
        hold.sync();
        assert!(Hold::was_held(&path));
        flag.store(false, Ordering::SeqCst);
        hold.sync();
        assert!(!Hold::was_held(&path));
        // Letting go twice is nothing.
        hold.sync();
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn the_thread_writes_a_stop_the_user_pressed_without_anyone_asking() {
        let path = marker("thread");
        let flag = Arc::new(AtomicBool::new(false));
        let _hold = Hold::follow(path.clone(), flag.clone(), false);
        flag.store(true, Ordering::SeqCst);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while !Hold::was_held(&path) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(Hold::was_held(&path), "a restart would not find the stop");
        flag.store(false, Ordering::SeqCst);
        while Hold::was_held(&path) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!Hold::was_held(&path));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    // A path under a file: Windows says "not there" for it, not "can't tell".
    #[cfg(unix)]
    #[test]
    fn a_marker_that_cannot_be_looked_at_counts_as_set() {
        // A path under a file is neither there nor missing: it can't be told.
        let path = marker("blocked");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = path.parent().unwrap().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(Hold::was_held(&file.join("stop-held")));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_marker_that_cannot_be_written_is_tried_again() {
        let path = marker("retry");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // The folder it should be in is a file for now.
        let blocked = path.parent().unwrap().join("home");
        std::fs::write(&blocked, "x").unwrap();
        let (hold, flag) = hold(blocked.join("stop-held"), false);
        flag.store(true, Ordering::SeqCst);
        hold.sync();
        assert!(hold.written.lock().unwrap().1, "the failure is noted");
        std::fs::remove_file(&blocked).unwrap();
        hold.sync();
        assert!(Hold::was_held(&blocked.join("stop-held")));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
