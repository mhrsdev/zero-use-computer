//! The on-screen indicator shown while the agent uses the computer: the
//! agent's own cursor (the real mouse is never touched), a border around the
//! window it works on, a status label, a ripple where it clicks, and state
//! colours (thinking, working, waiting for approval, sensitive, error, done).
//!
//! It lives in a **separate helper process** (`computer-use-mcp overlay`)
//! that the engine feeds JSON lines over a pipe:
//!
//! * the engine never waits on it (writes go through a background thread) and
//!   keeps working if it fails to start or crashes;
//! * when the engine exits — even if it is killed — the pipe closes and the
//!   helper exits at once; and since the windows belong to the helper
//!   process, the OS removes them with it, so nothing is ever left on screen;
//! * its windows are click-through and left out of the agent's screenshots
//!   (excluded from capture on Windows and macOS, hidden for the moment of
//!   capture on X11).

pub mod draw;
pub mod helper;
pub mod text;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::OverlayConfig;

/// What the agent is doing, as shown by the overlay colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The model is working out its next step (gold).
    Thinking,
    /// Acting on the computer (blue).
    Working,
    /// The task is finished (green, then everything disappears).
    Done,
    /// Something failed (red).
    Error,
    /// Hide the overlay now.
    Hidden,
}

impl std::str::FromStr for Status {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "thinking" => Ok(Self::Thinking),
            "working" => Ok(Self::Working),
            "done" | "idle" | "finished" => Ok(Self::Done),
            "error" => Ok(Self::Error),
            "hidden" | "off" | "hide" => Ok(Self::Hidden),
            other => Err(format!(
                "unknown status `{other}` (thinking, working, done, error, hidden)"
            )),
        }
    }
}

/// Engine → helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Cmd {
    Config {
        config: Box<OverlayConfig>,
        /// The emergency stop key the helper listens for ("" = none).
        #[serde(default)]
        hotkey: String,
        /// Whether the agent is stopped right now.
        #[serde(default)]
        stopped: bool,
    },
    /// The agent waits (on) for the user to stop using the mouse/keyboard,
    /// or carries on (off).
    Paused {
        on: bool,
    },
    /// The agent is stopped (on) or may continue (off).
    Stopped {
        on: bool,
    },
    /// A tool call started.
    Begin,
    /// The tool call finished.
    End {
        ok: bool,
    },
    /// The window being worked on (x, y, width, height); None = whole screen.
    Target {
        rect: Option<[f64; 4]>,
    },
    /// Glide the agent cursor to a point, optionally clicking there.
    Pointer {
        x: f64,
        y: f64,
        #[serde(default)]
        click: bool,
    },
    /// A sensitive action is running (Some) or finished (None).
    Danger {
        action: Option<String>,
    },
    /// Waiting for the user's approval of `action`. With `ask`, the helper
    /// asks on screen and answers with [`Reply::Answer`].
    Approval {
        id: u64,
        action: String,
        ask: bool,
    },
    ApprovalDone,
    /// An explicit status from the host agent.
    Status {
        state: Status,
    },
    /// Hide everything for a screenshot; answered with [`Reply::Hidden`].
    Hide {
        id: u64,
    },
    Show,
    Quit,
}

/// Helper → engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Reply {
    Ready {
        excluded: bool,
        available: bool,
    },
    Hidden {
        id: u64,
        shown: bool,
    },
    Answer {
        id: u64,
        ok: bool,
    },
    /// The user pressed the stop key: the agent is now stopped (on) or may
    /// continue (off).
    Stop {
        on: bool,
    },
    /// Whether the system accepted the stop key (another program may own it).
    Hotkey {
        key: String,
        ok: bool,
    },
}

/// Most replies kept waiting to be picked up.
const MAX_BACKLOG: usize = 16;

/// How to start the helper process.
#[derive(Debug, Clone)]
pub struct Launcher {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Launcher {
    /// The `computer-use-mcp overlay` helper at `program`.
    pub fn helper(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec!["overlay".into()],
        }
    }
}

/// The engine's handle on the helper. Every method is best-effort and
/// non-blocking (except the explicit waits below, which are bounded).
pub struct Overlay {
    child: Option<Child>,
    tx: Option<mpsc::Sender<String>>,
    rx: mpsc::Receiver<Reply>,
    backlog: Vec<Reply>,
    alive: Arc<AtomicBool>,
    /// Set by the stop key (shared with the engine).
    stop: Arc<AtomicBool>,
    excluded: bool,
    /// Whether the helper can draw (known once it reports ready).
    available: Option<bool>,
    next_id: u64,
}

impl Overlay {
    /// Start the helper. `hotkey` is the emergency stop key it listens for;
    /// pressing it sets (and pressing it again clears) `stop`.
    pub fn spawn(
        launcher: &Launcher,
        config: &OverlayConfig,
        hotkey: &str,
        stop: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(&launcher.program);
        cmd.args(&launcher.args)
            .arg("--parent")
            .arg(std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            // No console window for the helper, even from a GUI host.
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let alive = Arc::new(AtomicBool::new(true));

        let (tx, lines) = mpsc::channel::<String>();
        let a = alive.clone();
        std::thread::Builder::new()
            .name("overlay-writer".into())
            .spawn(move || {
                for line in lines {
                    if writeln!(stdin, "{line}")
                        .and_then(|_| stdin.flush())
                        .is_err()
                    {
                        a.store(false, Ordering::Relaxed);
                        break;
                    }
                }
                // Dropping stdin here tells the helper to exit.
            })?;

        let (rtx, rx) = mpsc::channel::<Reply>();
        let a = alive.clone();
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("overlay-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(r) = serde_json::from_str::<Reply>(&line) else {
                        continue;
                    };
                    match r {
                        // Acted on here, so it works even while the engine
                        // is busy (waiting for an app, a long batch…).
                        Reply::Stop { on } => {
                            flag.store(on, Ordering::SeqCst);
                            if on {
                                log::warn!("stopped by the user (stop key)");
                            } else {
                                log::info!("the user let the agent continue");
                            }
                        }
                        Reply::Hotkey { key, ok: false } => {
                            log::warn!(
                                "the stop key {key} could not be registered (another program may use it); set control.stop_hotkey to another combination"
                            );
                        }
                        Reply::Hotkey { .. } => {}
                        r => {
                            if rtx.send(r).is_err() {
                                break;
                            }
                        }
                    }
                }
                a.store(false, Ordering::Relaxed);
            })?;

        let o = Self {
            child: Some(child),
            tx: Some(tx),
            rx,
            backlog: Vec::new(),
            alive,
            stop,
            excluded: false,
            available: None,
            next_id: 0,
        };
        o.configure(config, hotkey);
        Ok(o)
    }

    /// Send (new) settings.
    pub fn configure(&self, config: &OverlayConfig, hotkey: &str) {
        self.send(&Cmd::Config {
            config: Box::new(config.clone()),
            hotkey: hotkey.to_string(),
            stopped: self.stop.load(Ordering::SeqCst),
        });
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn send(&self, cmd: &Cmd) {
        if !self.alive() {
            return;
        }
        if let (Some(tx), Ok(line)) = (&self.tx, serde_json::to_string(cmd)) {
            let _ = tx.send(line);
        }
    }

    /// File a reply from the helper. Only the answer to the latest request
    /// is kept (one waits for at most one at a time): late answers to
    /// requests that timed out are dropped, so nothing piles up.
    fn take(&mut self, r: Reply) {
        match r {
            Reply::Ready {
                excluded,
                available,
            } => {
                self.excluded = excluded;
                self.available = Some(available);
            }
            Reply::Hidden { id, .. } | Reply::Answer { id, .. } if id != self.next_id => {}
            r => {
                if self.backlog.len() >= MAX_BACKLOG {
                    self.backlog.remove(0);
                }
                self.backlog.push(r);
            }
        }
    }

    fn drain(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            self.take(r);
        }
    }

    /// Wait up to `timeout` for a reply matching `pred`, or (with no
    /// predicate) until the helper has said whether it can draw.
    fn wait_for(
        &mut self,
        timeout: Duration,
        pred: Option<&dyn Fn(&Reply) -> bool>,
    ) -> Option<Reply> {
        let deadline = Instant::now() + timeout;
        loop {
            self.drain();
            match pred {
                Some(p) => {
                    if let Some(i) = self.backlog.iter().position(p) {
                        return Some(self.backlog.remove(i));
                    }
                }
                None if self.available.is_some() => return None,
                None => {}
            }
            if !self.alive() {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match self.rx.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(r) => self.take(r),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Hide the overlay before a screenshot when the platform can't leave it
    /// out of captures. `None` when nothing needed hiding; otherwise whether
    /// anything was on screen (the caller then waits `capture_hide_ms` for
    /// the screen to repaint, and sends [`Cmd::Show`] afterwards).
    pub fn hide_for_capture(&mut self) -> Option<bool> {
        self.drain();
        if self.excluded || !self.alive() {
            return None;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.send(&Cmd::Hide { id });
        let r = self.wait_for(
            Duration::from_millis(150),
            Some(&|r: &Reply| matches!(r, Reply::Hidden { id: i, .. } if *i == id)),
        );
        Some(matches!(r, Some(Reply::Hidden { shown: true, .. })))
    }

    /// Ask the user on screen to approve `action`; `None` if the helper can't
    /// (not running, or no answer within `timeout`).
    pub fn ask(&mut self, action: &str, timeout: Duration) -> Option<bool> {
        if !self.alive() {
            return None;
        }
        // Only a helper that can show things can ask (it says so as soon
        // as it has started).
        self.drain();
        if self.available.is_none() {
            let _ = self.wait_for(Duration::from_secs(2), None);
        }
        if self.available != Some(true) {
            return None;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.send(&Cmd::Approval {
            id,
            action: action.to_string(),
            ask: true,
        });
        let deadline = Instant::now() + timeout;
        loop {
            // The stop key answers "no" to anything pending.
            if self.stop.load(Ordering::SeqCst) {
                self.send(&Cmd::ApprovalDone);
                return Some(false);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !self.alive() {
                return None;
            }
            if let Some(Reply::Answer { ok, .. }) = self.wait_for(
                left.min(Duration::from_millis(100)),
                Some(&|r: &Reply| matches!(r, Reply::Answer { id: i, .. } if *i == id)),
            ) {
                return Some(ok);
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.send(&Cmd::Quit);
        self.tx = None; // closes the helper's stdin
        if let Some(mut child) = self.child.take() {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            // Give it time to fade out before it is stopped, without holding
            // up the engine. (If this process exits first, the helper sees
            // its stdin close and goes by itself.)
            let reap = move || {
                let deadline = Instant::now() + Duration::from_millis(1200);
                while Instant::now() < deadline {
                    if let Ok(Some(_)) = child.try_wait() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = child.kill();
                let _ = child.wait();
            };
            let _ = std::thread::Builder::new()
                .name("overlay-reaper".into())
                .spawn(reap);
        }
    }
}

/// The helper for an embedding program: `overlay.command` if set, else a
/// `computer-use-mcp` next to the current executable or on `PATH`.
pub fn find_helper(config: &OverlayConfig) -> Option<Launcher> {
    if !config.command.trim().is_empty() {
        return Some(Launcher::helper(config.command.trim()));
    }
    let name = if cfg!(windows) {
        "computer-use-mcp.exe"
    } else {
        "computer-use-mcp"
    };
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file());
    let on_path = || {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(name))
                .find(|p| p.is_file())
        })
    };
    beside.or_else(on_path).map(Launcher::helper)
}

/// Whether process `pid` is still running (the helper's parent watchdog).
pub(crate) fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: signal 0 only checks that the process exists.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(target_os = "windows")]
    {
        windows::process_alive(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips() {
        let cmds = [
            Cmd::Begin,
            Cmd::End { ok: false },
            Cmd::Target {
                rect: Some([1.0, 2.0, 3.0, 4.0]),
            },
            Cmd::Pointer {
                x: 5.0,
                y: 6.0,
                click: true,
            },
            Cmd::Approval {
                id: 3,
                action: "press \"Send\"".into(),
                ask: true,
            },
            Cmd::Status {
                state: Status::Done,
            },
            Cmd::Paused { on: true },
            Cmd::Stopped { on: false },
            Cmd::Config {
                config: Box::default(),
                hotkey: "ctrl+alt+escape".into(),
                stopped: true,
            },
        ];
        for c in cmds {
            let line = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<Cmd>(&line).unwrap(), c, "{line}");
        }
        assert_eq!("idle".parse::<Status>().unwrap(), Status::Done);
        assert!("dancing".parse::<Status>().is_err());
    }

    #[test]
    fn a_missing_helper_is_harmless() {
        let l = Launcher::helper("/nonexistent/computer-use-mcp");
        let stop = Arc::new(AtomicBool::new(false));
        assert!(Overlay::spawn(&l, &OverlayConfig::default(), "", stop).is_err());
        // Old helpers' config lines (no hotkey fields) still parse.
        let line = r#"{"t":"config","config":{}}"#;
        assert!(matches!(
            serde_json::from_str::<Cmd>(line).unwrap(),
            Cmd::Config { stopped: false, .. }
        ));
    }

    #[test]
    fn late_replies_do_not_pile_up() {
        let (rtx, rx) = mpsc::channel();
        let mut o = Overlay {
            child: None,
            tx: None,
            rx,
            backlog: Vec::new(),
            alive: Arc::new(AtomicBool::new(true)),
            stop: Arc::new(AtomicBool::new(false)),
            excluded: false,
            available: None,
            next_id: 5,
        };
        for id in 0..100 {
            rtx.send(Reply::Hidden { id, shown: true }).unwrap();
        }
        rtx.send(Reply::Ready {
            excluded: true,
            available: true,
        })
        .unwrap();
        o.drain();
        assert_eq!(o.backlog, vec![Reply::Hidden { id: 5, shown: true }]);
        assert_eq!(o.available, Some(true));
        // Readiness already known: no wait.
        let t = Instant::now();
        assert_eq!(o.wait_for(Duration::from_secs(2), None), None);
        assert!(t.elapsed() < Duration::from_millis(500));
    }
}
