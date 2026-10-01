//! The emergency stop key on Wayland. A Wayland client can't listen for a
//! key while another app has the keyboard, so the compositor is asked to:
//! Hyprland and sway get a key binding (made through their IPC, removed
//! when the overlay ends) that sends this process SIGUSR1. Only that one
//! combination is ever bound, and never over a binding of the user's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::keys::KeyCombo;
use crate::linux::wayland::ipc::Compositor;

/// Set by the signal handler; taken by `pressed`.
static PRESSED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_usr1(_: libc::c_int) {
    PRESSED.store(true, Ordering::SeqCst);
}

/// Marks our bindings, so a stale one (left by a helper that crashed) is
/// told apart from the user's own.
pub const MARKER: &str = "computer-use-overlay";

pub struct StopKey {
    /// The binding made, to remove at the end.
    bound: Option<KeyCombo>,
    /// When it was last made.
    since: Instant,
}

/// How often the binding is made again: a compositor drops bindings made
/// through IPC when it reloads its config (Hyprland does whenever the file
/// changes).
const REBIND: Duration = Duration::from_secs(30);

impl StopKey {
    pub fn new() -> Self {
        // SAFETY: installs an async-signal-safe handler (one atomic store).
        unsafe {
            libc::signal(libc::SIGUSR1, on_usr1 as *const () as libc::sighandler_t);
        }
        Self {
            bound: None,
            since: Instant::now(),
        }
    }

    /// The command the compositor runs on the key: signal this process —
    /// only if it is still this program (a pid can be reused after a crash).
    fn command() -> String {
        let pid = std::process::id();
        let comm = std::fs::read_to_string("/proc/self/comm")
            .map(|c| c.trim().to_string())
            .unwrap_or_default();
        format!(
            "test \"$(cat /proc/{pid}/comm 2>/dev/null)\" = '{comm}' && kill -USR1 {pid} # {MARKER}"
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
            eprintln!("overlay: couldn't remove the stop key binding: {e}");
        }
        let Some(combo) = combo else {
            return false;
        };
        match comp.bind_key(&combo, &Self::command()) {
            Ok(true) => {
                self.bound = Some(combo);
                self.since = Instant::now();
                true
            }
            Ok(false) => {
                eprintln!(
                    "overlay: {combo} is already bound in {}; the stop key needs another combination (control.stop_hotkey)",
                    comp.name()
                );
                false
            }
            Err(e) => {
                eprintln!(
                    "overlay: couldn't bind the stop key in {}: {e}",
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
            && let Err(e) = comp.bind_key(&combo, &Self::command())
        {
            eprintln!("overlay: couldn't renew the stop key binding: {e}");
        }
    }

    /// The key was pressed since the last call.
    pub fn pressed(&self) -> bool {
        PRESSED.swap(false, Ordering::SeqCst)
    }
}

impl Drop for StopKey {
    fn drop(&mut self) {
        self.set(None);
    }
}
