//! The global keys (the emergency stop key, the settings key) on Wayland.
//! A Wayland client can't listen for a key while another app has the
//! keyboard, so the compositor is asked to: Hyprland and sway get a key
//! binding (made through their IPC, removed when the overlay ends) that
//! sends this process a signal — SIGUSR1 for the stop key, SIGUSR2 for the
//! settings key. Only these combinations are ever bound, and never over a
//! binding of the user's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::helper::Hotkey;
use crate::keys::KeyCombo;
use crate::linux::wayland::ipc::Compositor;

/// Set by the signal handlers; taken by `pressed`.
static PRESSED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

extern "C" fn on_usr1(_: libc::c_int) {
    PRESSED[0].store(true, Ordering::SeqCst);
}

extern "C" fn on_usr2(_: libc::c_int) {
    PRESSED[1].store(true, Ordering::SeqCst);
}

/// Marks our bindings, so a stale one (left by a helper that crashed) is
/// told apart from the user's own.
pub const MARKER: &str = "computer-use-overlay";

pub struct BoundKey {
    which: Hotkey,
    /// The binding made, to remove at the end.
    bound: Option<KeyCombo>,
    /// When it was last made.
    since: Instant,
}

/// How often the binding is made again: a compositor drops bindings made
/// through IPC when it reloads its config (Hyprland does whenever the file
/// changes).
const REBIND: Duration = Duration::from_secs(30);

impl BoundKey {
    pub fn new(which: Hotkey) -> Self {
        // SAFETY: installs an async-signal-safe handler (one atomic store).
        unsafe {
            match which {
                Hotkey::Stop => {
                    libc::signal(libc::SIGUSR1, on_usr1 as *const () as libc::sighandler_t);
                }
                Hotkey::Settings => {
                    libc::signal(libc::SIGUSR2, on_usr2 as *const () as libc::sighandler_t);
                }
            }
        }
        Self {
            which,
            bound: None,
            since: Instant::now(),
        }
    }

    fn what(&self) -> &'static str {
        match self.which {
            Hotkey::Stop => "the stop key (control.stop_hotkey)",
            Hotkey::Settings => "the settings key (control.settings_hotkey)",
        }
    }

    /// The command the compositor runs on the key: signal this process —
    /// only if it is still this program (a pid can be reused after a crash).
    fn command(&self) -> String {
        let pid = std::process::id();
        let comm = std::fs::read_to_string("/proc/self/comm")
            .map(|c| c.trim().to_string())
            .unwrap_or_default();
        let signal = match self.which {
            Hotkey::Stop => "USR1",
            Hotkey::Settings => "USR2",
        };
        format!(
            "test \"$(cat /proc/{pid}/comm 2>/dev/null)\" = '{comm}' && kill -{signal} {pid} # {MARKER}"
        )
    }

    /// Bind `combo` (or, with `None`, remove the binding). Whether the key
    /// works now.
    pub fn set(&mut self, combo: Option<KeyCombo>) -> bool {
        let Some(comp) = Compositor::detect() else {
            // GNOME, KDE…: no way to bind a key from here.
            return false;
        };
        if let Some(old) = self.bound.take()
            && let Err(e) = comp.unbind_key(&old)
        {
            eprintln!(
                "overlay: couldn't remove the binding of {}: {e}",
                self.what()
            );
        }
        let Some(combo) = combo else {
            return false;
        };
        match comp.bind_key(&combo, &self.command()) {
            Ok(true) => {
                self.bound = Some(combo);
                self.since = Instant::now();
                true
            }
            Ok(false) => {
                eprintln!(
                    "overlay: {combo} is already bound in {}; {} needs another combination",
                    comp.name(),
                    self.what()
                );
                false
            }
            Err(e) => {
                eprintln!(
                    "overlay: couldn't bind {} in {}: {e}",
                    self.what(),
                    comp.name()
                );
                false
            }
        }
    }

    /// Make the binding again now and then (see [`REBIND`]). Our own
    /// binding is replaced, so this is harmless when it is still there.
    pub fn refresh(&mut self) {
        if self.since.elapsed() < REBIND {
            return;
        }
        self.since = Instant::now();
        if let (Some(combo), Some(comp)) = (self.bound, Compositor::detect())
            && let Err(e) = comp.bind_key(&combo, &self.command())
        {
            eprintln!(
                "overlay: couldn't renew the binding of {}: {e}",
                self.what()
            );
        }
    }

    /// The key was pressed since the last call.
    pub fn pressed(&self) -> bool {
        PRESSED[self.which.index()].swap(false, Ordering::SeqCst)
    }
}

impl Drop for BoundKey {
    fn drop(&mut self) {
        self.set(None);
    }
}
