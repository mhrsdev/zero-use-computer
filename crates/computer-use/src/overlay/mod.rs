//! The on-screen indicator shown while the agent uses the computer: the
//! agent's own cursor (the real mouse is never touched), a border around the
//! window it works on, a status label, a ripple where it clicks, and state
//! colours (thinking, working, paused, stopped, error, done).
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
pub mod hub;
pub mod text;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod wayland_stop;
#[cfg(target_os = "windows")]
mod windows;

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
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
        /// The key that opens the decision model's settings page ("" = none).
        #[serde(default)]
        settings_key: String,
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
    /// Glide the agent cursor to a point, optionally clicking there. With
    /// an `id`, the helper answers [`Reply::Arrived`] once the cursor is
    /// shown there, so the action can wait for it.
    Pointer {
        x: f64,
        y: f64,
        #[serde(default)]
        click: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
    },
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
    // -- to the hub only (a helper of its own ignores these) --
    /// The first line to the hub: the token it wrote, who this is, and the
    /// number it had before (a hub started again gives it back if free).
    Hello {
        token: String,
        #[serde(default)]
        client: String,
        #[serde(default)]
        pid: u32,
        #[serde(default)]
        want: Option<u32>,
        /// The screen's work area, for sharing it out.
        #[serde(default)]
        screen: Option<[f64; 4]>,
        #[serde(default)]
        proto: u32,
    },
    /// The MCP client this engine serves ("claude-code", "codex"…).
    Client {
        name: String,
    },
    /// The app this agent works with now.
    Doing {
        app: String,
    },
    /// The part of the screen this agent would like.
    Area {
        want: AreaWant,
    },
    /// Ask for the keyboard and mouse (answered with [`Reply::Granted`]).
    Lock {
        id: u64,
    },
    /// Done with the keyboard and mouse.
    Unlock,
    /// A message for one agent, or all (`to` None).
    Send {
        #[serde(default)]
        to: Option<u32>,
        text: String,
    },
}

/// A part of the screen an agent asks the hub for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AreaWant {
    /// Whatever the hub shares out.
    #[default]
    Auto,
    Full,
    Half,
    Third,
    Quarter,
}

impl AreaWant {
    /// The share of the screen asked for (None: whatever is left).
    pub fn fraction(self) -> Option<f64> {
        match self {
            AreaWant::Auto => None,
            AreaWant::Full => Some(1.0),
            AreaWant::Half => Some(0.5),
            AreaWant::Third => Some(1.0 / 3.0),
            AreaWant::Quarter => Some(0.25),
        }
    }
}

impl std::str::FromStr for AreaWant {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(Self::Auto),
            "full" | "whole" | "all" => Ok(Self::Full),
            "half" => Ok(Self::Half),
            "third" => Ok(Self::Third),
            "quarter" => Ok(Self::Quarter),
            other => Err(format!(
                "unknown area `{other}` (full, half, third, quarter, auto)"
            )),
        }
    }
}

/// Another agent on this desktop, as the hub knows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    pub agent: u32,
    #[serde(default)]
    pub client: String,
    /// The app it works with, when it said.
    #[serde(default)]
    pub app: String,
    /// Its part of the screen.
    #[serde(default)]
    pub area: Option<[f64; 4]>,
}

/// A message from another agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub from: u32,
    #[serde(default)]
    pub client: String,
    pub text: String,
}

/// The hub protocol's version: a member and a hub that differ don't mix
/// (the member draws its own overlay instead).
pub const HUB_PROTO: u32 = 1;

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
    /// Whether the system accepted the settings key.
    SettingsKey {
        key: String,
        ok: bool,
    },
    /// The user pressed the settings key.
    Settings,
    /// The cursor is shown where [`Cmd::Pointer`] `id` sent it.
    Arrived {
        id: u64,
    },
    // -- from the hub only --
    /// Joined: this agent's number.
    Welcome {
        agent: u32,
        proto: u32,
    },
    /// Not joined (a wrong token, another protocol).
    Refused {
        why: String,
    },
    /// Every agent now on this desktop.
    Agents {
        agents: Vec<Peer>,
    },
    /// This agent's part of the screen (None: all of it), and whether it
    /// is what it asked for.
    Region {
        rect: Option<[f64; 4]>,
        granted: bool,
    },
    /// The keyboard and mouse are this agent's ([`Cmd::Lock`] `id`).
    Granted {
        id: u64,
    },
    /// Another agent has just used the keyboard or mouse (so the input
    /// isn't taken for the user's).
    Input {
        agent: u32,
    },
    Message(Message),
}

/// Replies kept for a later `wait_for` at most.
const MAX_BACKLOG: usize = 16;
/// Commands waiting for the helper to read them, at most.
const MAX_QUEUED: usize = 1024;

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

/// What to do when the user presses the settings key (called on the
/// helper's reader thread, so it runs even while the engine is busy).
pub type OnSettings = Arc<dyn Fn() + Send + Sync>;

/// The global keys the helper listens for ("" = none).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keys {
    /// The emergency stop key.
    pub stop: String,
    /// Opens the decision model's settings page.
    pub settings: String,
}

/// The engine's handle on the helper. Every method is best-effort and
/// non-blocking (except the explicit waits below, which are bounded).
pub struct Overlay {
    child: Option<Child>,
    tx: Option<mpsc::SyncSender<String>>,
    rx: mpsc::Receiver<Reply>,
    backlog: Vec<Reply>,
    alive: Arc<AtomicBool>,
    /// Set by the stop key (shared with the engine).
    stop: Arc<AtomicBool>,
    /// The stop key the helper last reported on, and whether the system
    /// accepted it (`None` until the helper says).
    hotkey: Arc<Mutex<Option<(String, bool)>>>,
    /// The same for the settings key.
    settings_key: Arc<Mutex<Option<(String, bool)>>>,
    excluded: bool,
    /// The helper has answered a `Hide` in time once: it is up (until then
    /// it may still be starting, and the first pictures wait for it).
    hide_answered: bool,
    /// Whether the helper says when the cursor arrives: unknown until it
    /// is first asked.
    arrivals: Option<bool>,
    next_id: u64,
    /// What the hub said, when this overlay is the hub's.
    hub: Option<Arc<HubLink>>,
}

/// What the hub has told this engine: its number, the other agents, its
/// part of the screen, their messages. Kept up to date by the reader thread.
#[derive(Default)]
pub struct HubLink {
    agent: std::sync::atomic::AtomicU32,
    peers: Mutex<Vec<Peer>>,
    /// Its part of the screen and whether it was what it asked for; None
    /// until the hub says.
    region: Mutex<Option<(Option<crate::types::Rect>, bool)>>,
    inbox: Mutex<std::collections::VecDeque<Message>>,
    /// When another agent last used the keyboard or mouse.
    others_input: Mutex<Option<Instant>>,
}

/// Messages kept for the engine at most (the oldest go first).
const MAX_INBOX: usize = 50;

impl HubLink {
    fn take(&self, r: Reply) {
        match r {
            Reply::Welcome { agent, .. } => self.agent.store(agent, Ordering::SeqCst),
            Reply::Agents { agents } => *lock(&self.peers) = agents,
            Reply::Region { rect, granted } => {
                let rect = rect.map(|r| crate::types::Rect::new(r[0], r[1], r[2], r[3]));
                *lock(&self.region) = Some((rect, granted));
            }
            Reply::Input { .. } => *lock(&self.others_input) = Some(Instant::now()),
            Reply::Message(m) => {
                let mut inbox = lock(&self.inbox);
                if inbox.len() >= MAX_INBOX {
                    inbox.pop_front();
                }
                inbox.push_back(m);
            }
            _ => {}
        }
    }

    /// This engine's number on the desktop.
    pub fn agent(&self) -> u32 {
        self.agent.load(Ordering::SeqCst)
    }

    /// Every agent on the desktop, this one included.
    pub fn peers(&self) -> Vec<Peer> {
        lock(&self.peers).clone()
    }

    /// This engine's part of the screen (None: the whole of it, or not
    /// told yet), and whether it is what it asked for.
    pub fn region(&self) -> (Option<crate::types::Rect>, bool) {
        lock(&self.region).unwrap_or((None, true))
    }

    /// The messages that came since the last call.
    pub fn take_messages(&self) -> Vec<Message> {
        lock(&self.inbox).drain(..).collect()
    }

    /// Whether a message is waiting.
    pub fn has_messages(&self) -> bool {
        !lock(&self.inbox).is_empty()
    }

    /// When another agent last used the keyboard or mouse.
    pub fn others_input(&self) -> Option<Instant> {
        *lock(&self.others_input)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Overlay {
    /// Start the helper. It listens for `keys`: pressing the stop key sets
    /// (and pressing it again clears) `stop`; the settings key calls
    /// `on_settings`.
    pub fn spawn(
        launcher: &Launcher,
        config: &OverlayConfig,
        keys: &Keys,
        stop: Arc<AtomicBool>,
        on_settings: Option<OnSettings>,
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
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Self::start(
            Some(child),
            Box::new(stdin),
            Box::new(BufReader::new(stdout)),
            config,
            keys,
            stop,
            on_settings,
            None,
        )
    }

    /// Join the hub every engine on this desktop shares (starting it with
    /// `launcher` if none runs): one overlay with a cursor for each agent,
    /// one stop key for all of them, their numbers and parts of the screen,
    /// turns at the keyboard and mouse, and their messages.
    pub fn join_hub(
        launcher: &Launcher,
        opts: &hub::JoinOptions,
        config: &OverlayConfig,
        keys: &Keys,
        stop: Arc<AtomicBool>,
        on_settings: Option<OnSettings>,
    ) -> std::io::Result<Self> {
        let (stream, reader, agent) = hub::connect(launcher, opts)?;
        let link = Arc::new(HubLink::default());
        link.agent.store(agent, Ordering::SeqCst);
        Self::start(
            None,
            Box::new(stream),
            Box::new(reader),
            config,
            keys,
            stop,
            on_settings,
            Some(link),
        )
    }

    /// Talk to a helper (or the hub) over `writer` and `reader`.
    #[allow(clippy::too_many_arguments)]
    fn start(
        child: Option<Child>,
        mut writer: Box<dyn Write + Send>,
        reader: Box<dyn BufRead + Send>,
        config: &OverlayConfig,
        keys: &Keys,
        stop: Arc<AtomicBool>,
        on_settings: Option<OnSettings>,
        hub: Option<Arc<HubLink>>,
    ) -> std::io::Result<Self> {
        let alive = Arc::new(AtomicBool::new(true));

        // Bounded: a helper that stops reading can't make it grow forever.
        let (tx, lines) = mpsc::sync_channel::<String>(MAX_QUEUED);
        let a = alive.clone();
        std::thread::Builder::new()
            .name("overlay-writer".into())
            .spawn(move || {
                for line in lines {
                    if writeln!(writer, "{line}")
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        a.store(false, Ordering::Relaxed);
                        break;
                    }
                }
                // Dropping the writer here tells the helper to exit.
            })?;

        let (rtx, rx) = mpsc::channel::<Reply>();
        let a = alive.clone();
        let flag = stop.clone();
        let hotkey_state = Arc::new(Mutex::new(None));
        let hk = hotkey_state.clone();
        let settings_state = Arc::new(Mutex::new(None));
        let sk = settings_state.clone();
        let link = hub.clone();
        std::thread::Builder::new()
            .name("overlay-reader".into())
            .spawn(move || {
                for line in reader.lines() {
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
                        Reply::Hotkey { key, ok } => {
                            if !ok {
                                log::warn!(
                                    "the stop key {key} could not be registered (another program may use it, or there is no display); set control.stop_hotkey to another combination"
                                );
                            }
                            if let Ok(mut h) = hk.lock() {
                                *h = Some((key, ok));
                            }
                        }
                        Reply::SettingsKey { key, ok } => {
                            if !ok {
                                log::info!(
                                    "the settings key {key} could not be registered (another program may use it); `computer-use-mcp settings` opens the page too"
                                );
                            }
                            if let Ok(mut h) = sk.lock() {
                                *h = Some((key, ok));
                            }
                        }
                        Reply::Settings => {
                            log::info!("the user pressed the settings key");
                            if let Some(f) = &on_settings {
                                f();
                            }
                        }
                        r @ (Reply::Welcome { .. }
                        | Reply::Refused { .. }
                        | Reply::Agents { .. }
                        | Reply::Region { .. }
                        | Reply::Input { .. }
                        | Reply::Message(_)) => {
                            if let Some(link) = &link {
                                link.take(r);
                            }
                        }
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
            child,
            tx: Some(tx),
            rx,
            backlog: Vec::new(),
            alive,
            stop,
            hotkey: hotkey_state,
            settings_key: settings_state,
            excluded: false,
            hide_answered: false,
            arrivals: None,
            next_id: 0,
            hub,
        };
        o.configure(config, keys);
        Ok(o)
    }

    /// Send (new) settings.
    pub fn configure(&self, config: &OverlayConfig, keys: &Keys) {
        self.send(&Cmd::Config {
            config: Box::new(config.clone()),
            hotkey: keys.stop.clone(),
            settings_key: keys.settings.clone(),
            stopped: self.stop.load(Ordering::SeqCst),
        });
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Whether the system accepted the stop key `key` (`None` until the
    /// helper has said, or if it last reported on another key).
    pub fn hotkey_ok(&self, key: &str) -> Option<bool> {
        let h = self.hotkey.lock().ok()?;
        h.as_ref()
            .filter(|(k, _)| k.trim().eq_ignore_ascii_case(key.trim()))
            .map(|(_, ok)| *ok)
    }

    /// The same for the settings key.
    pub fn settings_key_ok(&self, key: &str) -> Option<bool> {
        let h = self.settings_key.lock().ok()?;
        h.as_ref()
            .filter(|(k, _)| k.trim().eq_ignore_ascii_case(key.trim()))
            .map(|(_, ok)| *ok)
    }

    pub fn send(&self, cmd: &Cmd) {
        if !self.alive() {
            return;
        }
        if let (Some(tx), Ok(line)) = (&self.tx, serde_json::to_string(cmd))
            && let Err(mpsc::TrySendError::Full(_)) = tx.try_send(line)
        {
            // It stopped reading (hung): the engine starts a new one.
            log::warn!("the overlay helper stopped responding; replacing it");
            self.alive.store(false, Ordering::Relaxed);
        }
    }

    /// Keep a reply for a later `wait_for`. Late answers nobody waits for
    /// any more (a `Hidden` after its wait timed out) are dropped, oldest
    /// first, so the backlog can't grow without bound.
    fn take(&mut self, r: Reply) {
        if let Reply::Ready { excluded, .. } = r {
            self.excluded = excluded;
            return;
        }
        if self.backlog.len() >= MAX_BACKLOG {
            self.backlog.remove(0);
        }
        self.backlog.push(r);
    }

    fn drain(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            self.take(r);
        }
    }

    fn wait_for(&mut self, timeout: Duration, pred: impl Fn(&Reply) -> bool) -> Option<Reply> {
        let deadline = Instant::now() + timeout;
        loop {
            self.drain();
            if let Some(i) = self.backlog.iter().position(&pred) {
                return Some(self.backlog.remove(i));
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
    /// Glide the agent cursor to a point (clicking there when `click`)
    /// and wait until it is shown there, at most `limit`: an action then
    /// happens where the user sees the cursor, not before it gets there.
    /// A helper that never answers is given the glide's time instead.
    pub fn point(&mut self, x: f64, y: f64, click: bool, glide: Duration) -> bool {
        self.drain();
        if !self.alive() {
            return false;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.send(&Cmd::Pointer {
            x,
            y,
            click,
            id: Some(id),
        });
        if self.arrivals == Some(false) {
            std::thread::sleep(glide);
            return false;
        }
        // A helper just started takes longer to answer at first.
        let limit = glide
            + if self.arrivals == Some(true) {
                Duration::from_millis(300)
            } else {
                Duration::from_millis(1000)
            };
        let arrived = self
            .wait_for(limit, |r| matches!(r, Reply::Arrived { id: i } if *i == id))
            .is_some();
        self.arrivals = Some(arrived);
        arrived
    }

    /// What the hub said, when this overlay is the hub's.
    pub fn hub(&self) -> Option<&Arc<HubLink>> {
        self.hub.as_ref()
    }

    /// Wait for a turn at the keyboard and mouse (the hub gives them to one
    /// agent at a time), at most `limit`, or until `give_up` says to.
    /// True once it is this engine's.
    pub fn lock_input(&mut self, limit: Duration, give_up: impl Fn() -> bool) -> bool {
        if self.hub.is_none() || !self.alive() {
            return true;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.send(&Cmd::Lock { id });
        let deadline = Instant::now() + limit;
        loop {
            if self
                .wait_for(
                    Duration::from_millis(50),
                    |r| matches!(r, Reply::Granted { id: i } if *i == id),
                )
                .is_some()
            {
                return true;
            }
            // A hub that went away gives no turns: the engine goes on alone.
            if !self.alive() {
                return true;
            }
            if give_up() || Instant::now() >= deadline {
                // Not waiting any longer: the turn, if it comes, goes back.
                self.send(&Cmd::Unlock);
                return false;
            }
        }
    }

    /// Done with the keyboard and mouse.
    pub fn unlock_input(&self) {
        if self.hub.is_some() {
            self.send(&Cmd::Unlock);
        }
    }

    pub fn hide_for_capture(&mut self) -> Option<bool> {
        self.drain();
        if self.excluded || !self.alive() {
            return None;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.send(&Cmd::Hide { id });
        // A helper that has just started can take longer to answer than
        // one that is up: a picture taken meanwhile would have it in it
        // (and its label read as the app's text).
        let wait = if self.hide_answered { 150 } else { 1000 };
        let r = self.wait_for(
            Duration::from_millis(wait),
            |r| matches!(r, Reply::Hidden { id: i, .. } if *i == id),
        );
        self.hide_answered |= r.is_some();
        Some(matches!(r, Some(Reply::Hidden { shown: true, .. })))
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
                    std::thread::sleep(Duration::from_millis(10));
                }
                let _ = child.kill();
                let _ = child.wait();
            };
            let _ = std::thread::Builder::new()
                .name("overlay-reap".into())
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
    // (Not /proc: some containers don't mount it, and the helper would
    // take its parent for gone and quit.)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
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
                id: Some(3),
            },
            Cmd::Status {
                state: Status::Done,
            },
            Cmd::Paused { on: true },
            Cmd::Stopped { on: false },
            Cmd::Config {
                config: Box::default(),
                hotkey: "ctrl+alt+escape".into(),
                settings_key: "ctrl+alt+j".into(),
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
        assert!(
            Overlay::spawn(&l, &OverlayConfig::default(), &Keys::default(), stop, None).is_err()
        );
        // Old helpers' config lines (no hotkey fields) still parse.
        let line = r#"{"t":"config","config":{}}"#;
        assert!(matches!(
            serde_json::from_str::<Cmd>(line).unwrap(),
            Cmd::Config { stopped: false, .. }
        ));
    }
}
