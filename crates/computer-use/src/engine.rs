//! The engine: platform-agnostic implementation of the tool contract on top
//! of a [`Backend`]. It resolves apps/windows/elements, enforces approvals,
//! renders app state (tree + screenshot), maintains per-app element indices
//! and diffs, and maps screenshot coordinates to the screen.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::backend::{Backend, Native};
use crate::config::{AttachMode, ConfigStore};
use crate::error::{Error, Result};
use crate::imaging::{self, CoordMap, EncodedImage};
use crate::keys::{self, Key, KeyCombo, NamedKey};
use crate::overlay::{Cmd as OverlayCmd, Launcher, Overlay, Status};
use crate::policy::{self, Verdict};
use crate::screens::{PixelSig, Screen, ScreenMemory, View};
use crate::tools::*;
use crate::tree::{self, IndexAllocator, Node};
use crate::types::*;

/// What the host must decide when an app needs approval.
#[derive(Debug, Clone)]
pub struct ApprovalRequest<'a> {
    pub app: &'a AppInfo,
    pub tool: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Allow for this call only.
    Once,
    /// Allow and persist to `always_allow`.
    Always,
    /// Allow for the rest of this session.
    Session,
    Deny,
}

/// Decides whether an app may be controlled when policy says to ask.
/// Implementations prompt the user (MCP elicitation, a desktop dialog, etc.).
pub trait Approver {
    fn request(&mut self, req: &ApprovalRequest<'_>) -> ApprovalDecision;

    /// Confirm a consequential on-screen action (the guard). `summary`
    /// describes what is about to happen, e.g. `press button "Send"`. The
    /// default assumes an interactive host and allows it; headless approvers
    /// should override this.
    fn confirm_action(&mut self, _summary: &str) -> bool {
        true
    }

    /// Whether this approver can actually put the question to the user (or
    /// has a policy that answers it). When it can't, the engine may ask on
    /// the screen instead (`overlay.confirm_on_screen`).
    fn interactive(&self) -> bool {
        true
    }
}

/// Approve nothing (used when no approver is wired up).
pub struct DenyApprover;
impl Approver for DenyApprover {
    fn request(&mut self, _req: &ApprovalRequest<'_>) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
    fn confirm_action(&mut self, _summary: &str) -> bool {
        false
    }
    fn interactive(&self) -> bool {
        false
    }
}

/// Approve everything (embedders that gate access themselves).
pub struct AllowApprover;
impl Approver for AllowApprover {
    fn request(&mut self, _req: &ApprovalRequest<'_>) -> ApprovalDecision {
        ApprovalDecision::Session
    }
}

/// Cached state for one app between tool calls.
#[derive(Default)]
struct AppState {
    /// Window of the latest snapshot.
    window_id: Option<u64>,
    alloc: IndexAllocator,
    /// Latest snapshot, numbered.
    nodes: Vec<Node>,
    /// Elements left out of `nodes` by `tree.max_nodes`.
    omitted: usize,
    /// Handle → screen bounds, for coordinate fallbacks.
    bounds: HashMap<ElementHandle, Rect>,
    /// Screen the latest snapshot shows (0 = none yet).
    screen: u32,
    /// The screen the model currently knows, as it was last shown to it.
    known: Option<Screen>,
    /// Coordinate map of the screenshot the model is working from.
    coord: Option<CoordMap>,
    /// When the snapshot was taken, and the action epoch at that time.
    snap_at: Option<Instant>,
    snap_epoch: u64,
    /// Window ids at the last lookup (to notice a newly opened dialog).
    seen_windows: Vec<u64>,
    stamped: bool,
    /// Screen areas of private data in the latest snapshot ([privacy]).
    private: Vec<Rect>,
    /// The last text read off the window (OCR), with the picture it was
    /// read from, so an unchanged picture isn't read again.
    ocr_cache: Option<(PixelSig, Vec<OcrLine>)>,
    /// Lines of OCR text in the latest snapshot.
    ocr_lines: usize,
}

/// A screenshot handed out but not yet known to have reached the model
/// (a batch keeps only its last image).
struct PendingImage {
    pid: u32,
    screen: u32,
    coord: CoordMap,
    pixels: Option<PixelSig>,
}

pub struct Engine<B: Backend> {
    backend: B,
    store: ConfigStore,
    session_allowed: HashSet<String>,
    states: HashMap<u32, AppState>,
    /// Screens the model has seen and moved away from.
    memory: ScreenMemory,
    /// Recently listed apps, reused for `timing.app_cache_ms`.
    app_cache: Option<(Instant, Vec<AppInfo>)>,
    /// Recently listed windows per app: (when, epoch, windows).
    window_cache: HashMap<u32, (Instant, u64, Vec<WindowInfo>)>,
    /// Bumped by every action; snapshots from an older epoch are stale.
    epoch: u64,
    pending_images: Vec<PendingImage>,
    /// Nesting of `call` (batch steps run inside a call).
    depth: u32,
    /// The on-screen indicator (a separate helper process), if running.
    overlay: Option<Overlay>,
    /// How to start it; set by the host (`with_overlay`).
    overlay_launcher: Option<Launcher>,
    /// Start attempts, so a helper that keeps failing is left alone.
    overlay_starts: u32,
    overlay_retry_at: Option<Instant>,
    /// Set by the user's stop key (through the overlay helper) or the host;
    /// while set, every tool call is refused.
    stop: Arc<AtomicBool>,
    /// When the engine's own synthesized input last ended, so the system
    /// idle time isn't mistaken for the user's input.
    last_input: Option<Instant>,
    /// The action epoch whose result has a fresh snapshot (after settling).
    settled: Option<u64>,
    /// The last full-screen screenshot sent: its fingerprint and scale.
    screen_shot: Option<(PixelSig, CoordMap)>,
    /// Read text off the screen in the next observe (get_app_state ocr=true).
    force_ocr: bool,
    /// Reuse the last OCR result instead of reading again (while settling).
    ocr_reuse: bool,
    /// Why OCR isn't available, once found out (told to the model once).
    ocr_note: Option<String>,
    ocr_note_shown: bool,
    /// A window capture taken for OCR at this epoch, reused as the screenshot.
    last_capture: Option<(u32, u64, u64, Capture)>,
    /// Config file modification time, for hot reload.
    config_mtime: Option<std::time::SystemTime>,
    /// Host-level overrides (e.g. command-line flags) re-applied on reload.
    overrides: Option<ConfigOverride>,
    clock: Box<dyn Fn() -> Instant + Send>,
    sleep: Box<dyn Fn(Duration) + Send>,
}

/// Host-level settings forced on top of the config file.
type ConfigOverride = Box<dyn Fn(&mut crate::config::Config) + Send>;

/// How the current view relates to what the model has seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// A screen the model hasn't seen.
    New,
    /// The screen the model is on.
    Same,
    /// A screen the model saw earlier and left.
    Revisit,
}

/// What a tree render produced.
struct Refreshed {
    seen: Seen,
    screen: u32,
    /// Rendered tree, diff, or "identical" note.
    text: String,
    /// The full tree was rendered.
    full: bool,
    /// The diff was too large and the full tree was rendered instead.
    large_change: bool,
    /// Elements added, changed or removed (0 for a new screen).
    changes: usize,
    /// Interactive elements in the pruned tree.
    interactive: usize,
}

/// A resolved click/scroll/drag anchor.
enum Anchor {
    Element(ElementHandle),
    Point(Point),
}

impl<B: Backend> Engine<B> {
    pub fn new(mut backend: B, store: ConfigStore) -> Self {
        backend.configure(&store.config);
        let config_mtime = store.path.as_deref().and_then(file_mtime);
        Self {
            backend,
            store,
            session_allowed: HashSet::new(),
            states: HashMap::new(),
            memory: ScreenMemory::default(),
            app_cache: None,
            window_cache: HashMap::new(),
            epoch: 0,
            pending_images: Vec::new(),
            depth: 0,
            overlay: None,
            overlay_launcher: None,
            overlay_starts: 0,
            overlay_retry_at: None,
            stop: Arc::new(AtomicBool::new(false)),
            last_input: None,
            settled: None,
            screen_shot: None,
            force_ocr: false,
            ocr_reuse: false,
            ocr_note: None,
            ocr_note_shown: false,
            last_capture: None,
            config_mtime,
            overrides: None,
            clock: Box::new(Instant::now),
            sleep: Box::new(std::thread::sleep),
        }
    }

    /// Re-read the config file if it changed on disk (`hot_reload`). Session
    /// approvals and cached UI state are kept.
    pub fn reload_if_changed(&mut self) {
        if !self.store.config.hot_reload {
            return;
        }
        let Some(path) = self.store.path.clone() else {
            return;
        };
        let mtime = file_mtime(&path);
        if mtime == self.config_mtime {
            return;
        }
        self.config_mtime = mtime;
        match ConfigStore::load(Some(&path)) {
            Ok(mut store) => {
                if let Some(f) = &self.overrides {
                    f(&mut store.config);
                }
                log::info!("reloaded settings from {}", path.display());
                self.backend.configure(&store.config);
                self.store = store;
                self.epoch += 1;
                self.overlay_reconfigure();
            }
            Err(e) => log::warn!("keeping previous settings; {e}"),
        }
    }

    /// Settings the host always forces (e.g. command-line flags). Applied now
    /// and again after every hot reload.
    pub fn with_overrides(
        mut self,
        f: impl Fn(&mut crate::config::Config) + Send + 'static,
    ) -> Self {
        f(&mut self.store.config);
        self.backend.configure(&self.store.config);
        self.overrides = Some(Box::new(f));
        self
    }

    /// Replace the settings (embedders that manage config themselves).
    pub fn set_config(&mut self, store: ConfigStore) {
        self.backend.configure(&store.config);
        self.store = store;
        self.epoch += 1;
        self.overlay_reconfigure();
    }

    /// Show the on-screen indicator while the agent works, using this helper
    /// program (normally `computer-use-mcp overlay`). Without this (or
    /// `overlay.command`), no overlay is shown.
    pub fn with_overlay(mut self, launcher: Launcher) -> Self {
        self.overlay_launcher = Some(launcher);
        self
    }

    /// Tell the overlay what the agent is doing, for hosts that know (e.g.
    /// "thinking" while the model generates, "done" when the task ends).
    pub fn set_status(&mut self, status: Status) {
        if matches!(status, Status::Done | Status::Hidden) && self.overlay.is_none() {
            return;
        }
        self.overlay_send(OverlayCmd::Status { state: status });
    }

    /// The running overlay helper, started on first use. Never fails: when
    /// it can't run, the engine simply works without it.
    fn overlay(&mut self) -> Option<&mut Overlay> {
        let cfg = &self.store.config.overlay;
        // The helper also listens for the stop key, so it runs when either
        // is wanted (with the overlay off it draws nothing).
        let hotkey = self.store.config.control.stop_hotkey.trim().to_string();
        if !cfg.enabled && hotkey.is_empty() {
            self.overlay = None;
            return None;
        }
        if self.overlay.as_ref().is_some_and(|o| !o.alive()) {
            // It died: try again a little later, a few times at most.
            self.overlay = None;
            self.overlay_retry_at = Some((self.clock)() + Duration::from_secs(10));
        }
        if self.overlay.is_none() {
            let launcher = self.overlay_launcher.clone().or_else(|| {
                (!cfg.command.trim().is_empty())
                    .then(|| crate::overlay::find_helper(cfg))
                    .flatten()
            })?;
            if self.overlay_starts >= 5 || self.overlay_retry_at.is_some_and(|t| (self.clock)() < t)
            {
                return None;
            }
            self.overlay_starts += 1;
            match Overlay::spawn(&launcher, cfg, &hotkey, self.stop.clone()) {
                Ok(o) => self.overlay = Some(o),
                Err(e) => {
                    log::warn!("overlay unavailable: {e}");
                    self.overlay_retry_at = Some((self.clock)() + Duration::from_secs(30));
                }
            }
        }
        self.overlay.as_mut()
    }

    fn overlay_send(&mut self, cmd: OverlayCmd) {
        if let Some(o) = self.overlay() {
            o.send(&cmd);
        }
    }

    fn overlay_reconfigure(&mut self) {
        let cfg = self.store.config.overlay.clone();
        let hotkey = self.store.config.control.stop_hotkey.trim().to_string();
        if !cfg.enabled && hotkey.is_empty() {
            self.overlay = None;
        } else if let Some(o) = &self.overlay {
            o.configure(&cfg, &hotkey);
        } else if !hotkey.is_empty() {
            // Listen for the stop key from now on, not only once work starts.
            self.overlay();
        }
    }

    /// Start listening for the user's stop key (and get the overlay ready)
    /// before the first tool call. Hosts call this once at start-up.
    pub fn arm(&mut self) {
        self.overlay();
    }

    /// Shared stop flag: the stop key sets and clears it; a host may too
    /// (e.g. its own Stop button). While set, every tool call is refused.
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Whether the user stopped the agent.
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Stop the agent (or let it continue), as the stop key does.
    pub fn set_stopped(&mut self, on: bool) {
        self.stop.store(on, Ordering::SeqCst);
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Stopped { on });
        }
    }

    fn stopped_error(&self) -> Error {
        let key = self.store.config.control.stop_hotkey.trim();
        Error::Stopped(if key.is_empty() {
            "the host's stop control".into()
        } else {
            crate::overlay::helper::pretty_key(key)
        })
    }

    /// Before an action: wait while the user is using the mouse or keyboard
    /// (`control.pause_on_user_input`). Only the system idle time is read.
    fn wait_for_user(&mut self) -> Result<()> {
        /// Allowance for the delivery of the engine's own input.
        const OWN_INPUT_MARGIN: Duration = Duration::from_millis(250);
        let ctl = self.store.config.control.clone();
        if !ctl.pause_on_user_input {
            return Ok(());
        }
        let resume = Duration::from_millis(ctl.resume_after_idle_ms.max(1));
        let limit = Duration::from_secs(ctl.max_pause_secs);
        let start = (self.clock)();
        let mut paused = false;
        let result = loop {
            if self.is_stopped() {
                break Err(self.stopped_error());
            }
            let Some(idle) = self.backend.user_idle() else {
                break Ok(());
            };
            let now = (self.clock)();
            // Input after the engine's own last input is the user's.
            let own = self.last_input.map(|t| now.saturating_duration_since(t));
            let user_active = idle < resume && own.is_none_or(|o| idle + OWN_INPUT_MARGIN < o);
            if !user_active {
                break Ok(());
            }
            if !paused {
                paused = true;
                log::info!("the user is using the computer; waiting");
                self.overlay_send(OverlayCmd::Paused { on: true });
            }
            let waited = now.saturating_duration_since(start);
            if waited >= limit {
                break Err(Error::UserBusy(waited.as_secs()));
            }
            (self.sleep)(
                (resume - idle).clamp(Duration::from_millis(50), Duration::from_millis(250)),
            );
        };
        if paused {
            self.overlay_send(OverlayCmd::Paused { on: false });
        }
        result
    }

    /// Black out private areas ([privacy]) of a capture: those found in the
    /// latest trees of every app (other windows can overlap a capture).
    /// Returns how many areas were covered.
    fn redact_capture(&self, cap: &mut Capture) -> usize {
        if !crate::privacy::active(&self.store.config.privacy) {
            return 0;
        }
        let rects: Vec<Rect> = self
            .states
            .values()
            .flat_map(|s| s.private.iter().copied())
            .collect();
        if rects.is_empty() {
            return 0;
        }
        imaging::redact(cap, &rects, self.store.config.privacy.style)
    }

    /// Point the agent cursor at an element (its centre), optionally clicking.
    fn overlay_point_element(&mut self, app: &AppInfo, handle: ElementHandle, click: bool) {
        let p = self
            .state(app.pid)
            .ok()
            .and_then(|s| s.bounds.get(&handle))
            .filter(|b| !b.is_empty())
            .map(|b| b.center());
        if let Some(p) = p {
            self.overlay_point(p, click);
        }
    }

    fn overlay_point(&mut self, p: Point, click: bool) {
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Pointer {
                x: p.x,
                y: p.y,
                click,
            });
        }
    }

    /// Whether to ask the user on the screen rather than through `approver`.
    fn ask_on_screen(&self, approver: &dyn Approver) -> bool {
        use crate::config::ScreenConfirm;
        if !self.store.config.overlay.enabled {
            return false;
        }
        match self.store.config.overlay.confirm_on_screen {
            ScreenConfirm::Never => false,
            ScreenConfirm::Always => true,
            ScreenConfirm::WhenNoClient => !approver.interactive(),
        }
    }

    /// Show that `action` waits for the user, and ask on screen when that is
    /// the way to ask. `Some(answer)` if the screen answered.
    fn overlay_approval(&mut self, action: &str, approver: &dyn Approver) -> Option<bool> {
        if !self.store.config.overlay.enabled {
            return None;
        }
        let ask = self.ask_on_screen(approver);
        let timeout = Duration::from_secs(self.store.config.overlay.confirm_timeout_secs.max(1));
        let o = self.overlay()?;
        if ask {
            if let Some(answer) = o.ask(action, timeout) {
                return Some(answer);
            }
        } else {
            o.send(&OverlayCmd::Approval {
                id: 0,
                action: action.to_string(),
                ask: false,
            });
        }
        None
    }

    /// Run a screen capture with the overlay out of the picture.
    fn capture_clean<T>(&mut self, f: impl FnOnce(&mut B) -> Result<T>) -> Result<T> {
        let pause = Duration::from_millis(self.store.config.overlay.capture_hide_ms);
        let hidden = self.overlay.as_mut().and_then(|o| o.hide_for_capture());
        if hidden == Some(true) {
            (self.sleep)(pause);
        }
        let r = f(&mut self.backend);
        if hidden.is_some()
            && let Some(o) = &self.overlay
        {
            o.send(&OverlayCmd::Show);
        }
        r
    }

    /// Forget every remembered screen (the next views are treated as new).
    pub fn clear_screen_memory(&mut self) {
        self.memory.clear();
        for st in self.states.values_mut() {
            st.known = None;
            st.screen = 0;
        }
    }

    /// Screens currently remembered and their approximate memory use.
    pub fn screen_memory_stats(&self) -> (usize, usize) {
        (self.memory.len(), self.memory.bytes())
    }

    /// Replace timing hooks (tests use instant clocks).
    pub fn with_time(
        mut self,
        clock: impl Fn() -> Instant + Send + 'static,
        sleep: impl Fn(Duration) + Send + 'static,
    ) -> Self {
        self.clock = Box::new(clock);
        self.sleep = Box::new(sleep);
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn store(&self) -> &ConfigStore {
        &self.store
    }

    pub fn permissions(&mut self) -> Vec<PermissionStatus> {
        self.backend.permissions()
    }

    /// Pre-approve an app for the session (e.g. from a host allow-list).
    pub fn allow_for_session(&mut self, app_id: &str) {
        self.session_allowed.insert(app_id.to_lowercase());
    }

    // -- app / window / element resolution ---------------------------------

    /// List running apps (always fresh), refreshing the cache and dropping
    /// cached UI state for apps that have quit.
    fn find_apps(&mut self) -> Result<Vec<AppInfo>> {
        let apps = self.backend.list_apps()?;
        let live: HashSet<u32> = apps.iter().map(|a| a.pid).collect();
        self.states.retain(|pid, _| live.contains(pid));
        self.window_cache.retain(|pid, _| live.contains(pid));
        self.memory.retain_pids(&live);
        self.app_cache = Some(((self.clock)(), apps.clone()));
        Ok(apps)
    }

    /// Resolve an app, reusing the recent app list when it is fresh enough.
    fn resolve_app(&mut self, query: &str) -> Result<AppInfo> {
        let ttl = Duration::from_millis(self.store.config.timing.app_cache_ms);
        if let Some((at, apps)) = &self.app_cache
            && (self.clock)().saturating_duration_since(*at) < ttl
            && let Ok(app) = resolve_app_in(apps, query)
        {
            return Ok(app);
        }
        let apps = self.find_apps()?;
        resolve_app_in(&apps, query)
    }

    /// Resolve an app and confirm it may be controlled.
    fn authorize(
        &mut self,
        query: &str,
        tool: &str,
        approver: &mut dyn Approver,
    ) -> Result<AppInfo> {
        let app = self.resolve_app(query)?;
        match policy::evaluate(&app, &self.store, &self.session_allowed) {
            Verdict::Allowed => {
                // Acting in a sensitive app the user opened up (a terminal,
                // a password manager…) shows as a sensitive action.
                if MUTATING_TOOLS.contains(&tool)
                    && self.overlay.is_some()
                    && let Some(category) = policy::classify(&app, &self.store)
                {
                    self.overlay_send(OverlayCmd::Danger {
                        action: Some(format!("{} in {} ({})", tool, app.name, category.label())),
                    });
                }
                Ok(app)
            }
            Verdict::Blocked(reason) => Err(Error::Blocked(app.name, reason)),
            Verdict::NeedsApproval => {
                let what = format!("let the agent control {}", app.name);
                let decision = match self.overlay_approval(&what, approver) {
                    Some(true) => ApprovalDecision::Session,
                    Some(false) => ApprovalDecision::Deny,
                    None => approver.request(&ApprovalRequest { app: &app, tool }),
                };
                self.overlay_send(OverlayCmd::ApprovalDone);
                match decision {
                    ApprovalDecision::Deny => Err(Error::Denied(app.name)),
                    ApprovalDecision::Once => Ok(app),
                    ApprovalDecision::Session => {
                        self.session_allowed.insert(app.id.to_lowercase());
                        Ok(app)
                    }
                    ApprovalDecision::Always => {
                        self.session_allowed.insert(app.id.to_lowercase());
                        if let Err(e) = self.store.add_always_allow(&app.id) {
                            log::warn!("could not persist approval for {}: {e}", app.id);
                        }
                        Ok(app)
                    }
                }
            }
        }
    }

    /// The app's windows, reusing a list taken within `cache.snapshot_ttl_ms`
    /// when no action ran since (unless `fresh`).
    fn list_windows(&mut self, app: &AppInfo, fresh: bool) -> Result<Vec<WindowInfo>> {
        let ttl = Duration::from_millis(self.store.config.cache.snapshot_ttl_ms);
        let now = (self.clock)();
        if !fresh
            && let Some((at, epoch, list)) = self.window_cache.get(&app.pid)
            && *epoch == self.epoch
            && now.saturating_duration_since(*at) < ttl
        {
            return Ok(list.clone());
        }
        let list = self.backend.list_windows(app)?;
        self.window_cache
            .insert(app.pid, ((self.clock)(), self.epoch, list.clone()));
        Ok(list)
    }

    fn resolve_window(
        &mut self,
        app: &AppInfo,
        query: Option<&str>,
        fresh: bool,
    ) -> Result<WindowInfo> {
        let w = self.pick_window(app, query, fresh)?;
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Target {
                rect: w.bounds.map(|b| [b.x, b.y, b.width, b.height]),
            });
        }
        Ok(w)
    }

    fn pick_window(
        &mut self,
        app: &AppInfo,
        query: Option<&str>,
        fresh: bool,
    ) -> Result<WindowInfo> {
        let mut windows = self.list_windows(app, fresh)?;
        if windows.is_empty() {
            return Err(Error::NoWindows {
                app: app.name.clone(),
            });
        }
        let state = self.states.entry(app.pid).or_default();
        let seen = std::mem::replace(
            &mut state.seen_windows,
            windows.iter().map(|w| w.id).collect(),
        );
        let remembered = state.window_id;
        if let Some(q) = query {
            let ql = q.to_lowercase();
            if let Some(w) = windows.iter().find(|w| w.id.to_string() == q) {
                return Ok(w.clone());
            }
            let matches: Vec<&WindowInfo> = windows
                .iter()
                .filter(|w| w.title.to_lowercase().contains(&ql))
                .collect();
            return match matches.as_slice() {
                [w] => Ok((*w).clone()),
                [] => Err(Error::WindowNotFound {
                    query: q.into(),
                    app: app.name.clone(),
                    available: window_list(&windows),
                }),
                many => {
                    // Prefer an exact title, else the focused one.
                    if let Some(w) = many.iter().find(|w| w.title.eq_ignore_ascii_case(q)) {
                        return Ok((*w).clone());
                    }
                    many.iter()
                        .find(|w| w.focused)
                        .map(|w| (*w).clone())
                        .ok_or_else(|| Error::WindowNotFound {
                            query: q.into(),
                            app: app.name.clone(),
                            available: window_list(&windows),
                        })
                }
            };
        }
        // No query: a window that just opened (a dialog or menu the last
        // action brought up) — the focused one, else the only one — then the
        // remembered window, the focused/main one, or the first.
        if !seen.is_empty() && self.store.config.follow_new_windows {
            let opened: Vec<&WindowInfo> = windows
                .iter()
                .filter(|w| !w.minimized && !seen.contains(&w.id))
                .collect();
            let pick = opened
                .iter()
                .find(|w| w.focused)
                .or(match opened.as_slice() {
                    [only] => Some(only),
                    _ => None,
                });
            if let Some(w) = pick {
                return Ok((*w).clone());
            }
        }
        if let Some(id) = remembered
            && let Some(w) = windows.iter().find(|w| w.id == id && !w.minimized)
        {
            return Ok(w.clone());
        }
        windows.sort_by_key(|w| (!w.focused, !w.main, w.minimized));
        Ok(windows.into_iter().next().unwrap())
    }

    /// Snapshot `window` (or reuse a snapshot from within
    /// `cache.snapshot_ttl_ms` when no action ran since, unless `fresh`), work
    /// out which screen it shows, and number its elements. What the model is
    /// known to have seen is left alone (see [`Self::commit`]).
    /// Whether to read text off this window: a sparse tree (`ocr.mode`), or
    /// the agent asked.
    fn ocr_wanted(&self, raw: &[RawNode]) -> bool {
        use crate::config::OcrMode;
        let cfg = &self.store.config;
        if !cfg.screenshot.enabled {
            return false;
        }
        if self.force_ocr {
            return true;
        }
        match cfg.ocr.mode {
            OcrMode::Off => false,
            OcrMode::Always => true,
            OcrMode::Auto => {
                raw.iter()
                    .filter(|n| crate::roles::is_interactive(&n.role) || n.states.editable)
                    .count()
                    < cfg.ocr.sparse_threshold
            }
        }
    }

    /// The text on a window (OCR), reusing the last result when the picture
    /// hasn't changed (or, while settling, without looking again).
    fn read_screen_text(&mut self, app: &AppInfo, window: &WindowInfo) -> Vec<OcrLine> {
        let cached = self.states.get(&app.pid).and_then(|s| s.ocr_cache.clone());
        if self.ocr_reuse
            && let Some((_, lines)) = &cached
        {
            return lines.clone();
        }
        let cap = match self.capture_clean(|b| b.capture(app, window)) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no capture for OCR: {e}");
                return Vec::new();
            }
        };
        let cache = &self.store.config.cache;
        let sig = PixelSig::of(&cap, cache.pixel_grid);
        let lines = match cached {
            Some((old, lines)) if old.same_as(&sig, cache.pixel_tolerance) => lines,
            _ => self.run_ocr(&cap),
        };
        self.states.entry(app.pid).or_default().ocr_cache = Some((sig, lines.clone()));
        self.last_capture = Some((app.pid, window.id, self.epoch, cap));
        lines
    }

    /// Recognise the text in a capture with the configured engine.
    fn run_ocr(&mut self, cap: &Capture) -> Vec<OcrLine> {
        use crate::config::OcrEngineChoice;
        let cfg = self.store.config.ocr.clone();
        let tesseract = || crate::ocr::tesseract(cap, &cfg.languages, &cfg.tesseract_path);
        let result = match cfg.engine {
            OcrEngineChoice::Native => self.backend.ocr(cap, &cfg.languages),
            OcrEngineChoice::Tesseract => tesseract(),
            OcrEngineChoice::Auto => match self.backend.ocr(cap, &cfg.languages) {
                Ok(lines) => Ok(lines),
                Err(e) => {
                    log::debug!("built-in OCR unavailable ({e}); trying Tesseract");
                    tesseract()
                }
            },
        };
        match result {
            Ok(lines) => {
                self.ocr_note = None;
                lines
            }
            Err(e) => {
                if self.ocr_note.is_none() {
                    log::warn!("text recognition unavailable: {e}");
                }
                self.ocr_note = Some(e.to_string());
                Vec::new()
            }
        }
    }

    fn observe(&mut self, app: &AppInfo, window: &WindowInfo, fresh: bool) -> Result<()> {
        let ttl = Duration::from_millis(self.store.config.cache.snapshot_ttl_ms);
        if !fresh
            && let Some(s) = self.states.get(&app.pid)
            && s.stamped
            && s.window_id == Some(window.id)
            && s.snap_epoch == self.epoch
            && s.snap_at
                .is_some_and(|t| (self.clock)().saturating_duration_since(t) < ttl)
        {
            return Ok(());
        }

        let tcfg = &self.store.config.tree;
        let opts = SnapshotOptions {
            max_nodes: tcfg.max_walk,
            max_depth: tcfg.max_depth,
        };
        let mut raw = self.backend.snapshot(app, window, &opts)?;
        let snap_at = (self.clock)();
        // Custom-drawn UI: add the text read off the window.
        let mut ocr_lines = 0;
        if !raw.is_empty() && self.ocr_wanted(&raw) {
            let lines = self.read_screen_text(app, window);
            let cfg = &self.store.config.ocr;
            let mut extra = crate::ocr::nodes(&lines, cfg.min_confidence, cfg.max_lines);
            // Leave out glyph noise and what the tree already says there.
            extra.retain(|o| {
                let text = crate::ocr::words(o.name.as_deref().unwrap_or(""));
                text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
                    && !raw.iter().any(|n| {
                        // The window itself overlaps everything: not a match.
                        let Some((a, b)) = n.bounds.zip(o.bounds) else {
                            return false;
                        };
                        if n.parent.is_none() || !a.intersects(&b) {
                            return false;
                        }
                        // A line-sized element (not a big container) whose
                        // text the line merely adds glyphs to also counts.
                        let line_sized = a.height <= 3.0 * b.height.max(8.0);
                        [&n.name, &n.value].into_iter().flatten().any(|t| {
                            let t = crate::ocr::words(t);
                            t.contains(&text) || (line_sized && t.len() >= 3 && text.contains(&t))
                        })
                    })
            });
            ocr_lines = extra.len();
            raw.extend(extra);
        }
        // Private data never reaches the model (or the screen memory).
        let private = if crate::privacy::active(&self.store.config.privacy) {
            crate::privacy::scrub(&mut raw, &self.store.config.privacy)
        } else {
            Vec::new()
        };
        let pruned = tree::prune(&raw, window.bounds, &self.store.config.tree);
        drop(raw);
        let mut nodes = pruned.nodes;

        let cache = &self.store.config.cache;
        let ratio = self.store.config.tree.diff_full_ratio;
        let mut st = self.states.remove(&app.pid).unwrap_or_default();
        let same_window = st.screen != 0 && st.window_id == Some(window.id);

        // Which screen is this? A small change keeps the current screen;
        // otherwise look for the most similar screen: the current one (a big
        // update of the same layout), the one the model knows, or one it saw
        // earlier.
        let small = same_window
            && (tree::change_count(&st.nodes, &nodes) as f64) < ratio * nodes.len().max(1) as f64;
        let screen = if small {
            st.screen
        } else {
            let shapes: HashSet<u64> = nodes.iter().map(|n| n.shape).collect();
            let threshold = cache.match_threshold;
            let mut best: Option<(u32, f64)> = None;
            if same_window {
                let sim = shape_similarity(&st.nodes, &shapes);
                if sim >= threshold {
                    best = Some((st.screen, sim));
                }
            }
            if cache.enabled {
                if let Some(k) = st.known.as_ref().filter(|k| k.id != st.screen) {
                    let sim = k.view.similarity(&shapes);
                    if sim >= threshold && best.is_none_or(|b| sim > b.1) {
                        best = Some((k.id, sim));
                    }
                }
                if let Some(m) = self
                    .memory
                    .best_match(app.pid, window.id, &shapes, threshold)
                    && best.is_none_or(|b| m.1 > b.1)
                {
                    best = Some(m);
                }
            }
            match best {
                Some((id, _)) => id,
                None => self.memory.new_id(),
            }
        };

        // Numbering: a screen seen before gets its old indices back.
        if screen != st.screen {
            let view = match &st.known {
                Some(k) if k.id == screen => Some(&k.view),
                _ => self.memory.get(screen).map(|s| &s.view),
            };
            match view {
                Some(v) => v.restore_indices(&mut st.alloc, &nodes),
                None if !same_window => st.alloc.clear_keys(),
                None => {}
            }
        }
        st.alloc.assign_stable(&mut nodes);

        st.bounds = nodes
            .iter()
            .filter_map(|n| n.bounds.map(|b| (n.handle, b)))
            .collect();
        st.window_id = Some(window.id);
        st.nodes = nodes;
        st.omitted = pruned.omitted;
        st.screen = screen;
        st.snap_at = Some(snap_at);
        st.snap_epoch = self.epoch;
        st.stamped = true;
        st.private = private;
        st.ocr_lines = ocr_lines;
        self.states.insert(app.pid, st);
        Ok(())
    }

    /// Render the latest snapshot against what the model has seen of that
    /// screen: a diff, the full tree, or a note that nothing changed.
    fn render(&self, pid: u32, full: bool) -> Result<Refreshed> {
        let st = self.state(pid)?;
        let tcfg = &self.store.config.tree;
        let (seen, base) = match &st.known {
            Some(k) if k.id == st.screen => (Seen::Same, Some(&k.view)),
            _ => match self.memory.get(st.screen) {
                Some(s) if self.store.config.cache.enabled => (Seen::Revisit, Some(&s.view)),
                _ => (Seen::New, None),
            },
        };
        let nodes = &st.nodes;
        let mut out = Refreshed {
            seen,
            screen: st.screen,
            text: String::new(),
            full: false,
            large_change: false,
            changes: 0,
            interactive: nodes
                .iter()
                .filter(|n| crate::roles::is_interactive(&n.role) || n.states.editable)
                .count(),
        };
        match base {
            None => {
                out.text = tree::render_full(nodes, tcfg.indent);
                out.full = true;
            }
            Some(view) => {
                let d = view.diff(nodes);
                out.changes = d.len();
                let large = out.changes as f64 >= tcfg.diff_full_ratio * nodes.len().max(1) as f64;
                if full || !tcfg.diff || large {
                    out.text = tree::render_full(nodes, tcfg.indent);
                    out.full = true;
                    out.large_change = large;
                } else if seen == Seen::Same {
                    out.text = tree::render_diff(&d, nodes);
                } else if d.is_empty() {
                    out.text = format!(
                        "Identical to when you last saw screen #{}; element indices are as they were then.\n",
                        st.screen
                    );
                } else {
                    let intro = format!(
                        "Changes since you last saw screen #{} (+ added, ~ changed, - removed). Other elements are as they were then, with the same indices.",
                        st.screen
                    );
                    out.text = tree::render_diff_with(&d, nodes, &intro);
                }
            }
        }
        if out.full && st.omitted > 0 {
            out.text
                .push_str(&format!("[{} more elements not shown]\n", st.omitted));
        }
        Ok(out)
    }

    /// The model has now been shown the latest snapshot: it becomes the known
    /// screen. Leaving a screen files it in the screen memory; returning to
    /// one takes it back out, with its screenshot's coordinate map. Returns
    /// whether a returning screen's window changed size (its old screenshot
    /// no longer lines up).
    fn commit(&mut self, pid: u32, window: &WindowInfo) -> bool {
        let Some(mut st) = self.states.remove(&pid) else {
            return false;
        };
        let cache = self.store.config.cache.clone();
        let view = View::from_nodes(&st.nodes);
        let size = window.bounds.map(|b| (b.width, b.height));
        let origin = window.bounds.map(|b| (b.x, b.y));
        let mut size_changed = false;
        match st.known.as_mut() {
            Some(k) if k.id == st.screen => {
                k.view = view;
                k.window = window.id;
                k.size = size;
                k.origin = origin;
            }
            _ => {
                let mut next = self
                    .memory
                    .take(st.screen)
                    .unwrap_or_else(|| Screen::new(st.screen, pid, window.id, View::default()));
                if let Some(old) = st.known.take()
                    && cache.enabled
                {
                    self.memory.remember(
                        old,
                        cache.max_screens,
                        cache.max_memory_kb.saturating_mul(1024),
                    );
                }
                // The old screenshot still lines up if the window kept its
                // size; follow the window if it moved.
                if next.shot {
                    match (next.size, size) {
                        (Some(a), Some(b))
                            if (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0 =>
                        {
                            if let (Some(o0), Some(o1), Some(c)) =
                                (next.origin, origin, next.coord.as_mut())
                            {
                                c.bounds.x += o1.0 - o0.0;
                                c.bounds.y += o1.1 - o0.1;
                            }
                        }
                        _ => {
                            next.coord = None;
                            next.pixels = None;
                            next.shot = false;
                            size_changed = true;
                        }
                    }
                }
                next.view = view;
                next.window = window.id;
                next.size = size;
                next.origin = origin;
                if let Some(c) = next.coord {
                    st.coord = Some(c);
                }
                st.known = Some(next);
            }
        }
        self.states.insert(pid, st);
        size_changed
    }

    /// Record that the images handed out by the finished top-level call
    /// reached the model: their screens now have that screenshot.
    fn commit_images(&mut self) {
        for p in std::mem::take(&mut self.pending_images) {
            let Some(st) = self.states.get_mut(&p.pid) else {
                continue;
            };
            st.coord = Some(p.coord);
            let screen = match st.known.as_mut() {
                Some(k) if k.id == p.screen => Some(k),
                _ => self.memory.get_mut(p.screen),
            };
            if let Some(s) = screen {
                s.coord = Some(p.coord);
                s.pixels = p.pixels;
                s.shot = true;
            }
        }
    }

    fn state(&self, pid: u32) -> Result<&AppState> {
        self.states
            .get(&pid)
            .filter(|s| s.stamped)
            .ok_or(Error::Internal("no cached state".into()))
    }

    /// Look up an element handle by index in the app's latest state.
    fn element_by_index(&self, app: &AppInfo, index: u32) -> Result<ElementHandle> {
        let state = self
            .states
            .get(&app.pid)
            .filter(|s| s.stamped)
            .ok_or_else(|| Error::NoState(app.name.clone()))?;
        state
            .nodes
            .iter()
            .find(|n| n.index == index)
            .map(|n| n.handle)
            .ok_or(Error::UnknownElement {
                app: app.name.clone(),
                index,
            })
    }

    fn node_by_index(&self, app: &AppInfo, index: u32) -> Result<&Node> {
        let state = self
            .state(app.pid)
            .map_err(|_| Error::NoState(app.name.clone()))?;
        state
            .nodes
            .iter()
            .find(|n| n.index == index)
            .ok_or(Error::UnknownElement {
                app: app.name.clone(),
                index,
            })
    }

    fn input_target(&self, app: &AppInfo) -> InputTarget {
        let (window_id, window_handle) = self
            .states
            .get(&app.pid)
            .and_then(|s| s.window_id)
            .map(|id| (Some(id), None))
            .unwrap_or((None, None));
        InputTarget {
            pid: app.pid,
            window_id,
            window_handle,
        }
    }

    /// Convert an element index / coordinate pair into a screen anchor.
    fn anchor(
        &self,
        app: &AppInfo,
        index: Option<u32>,
        x: Option<f64>,
        y: Option<f64>,
        what: &str,
    ) -> Result<Anchor> {
        if let Some(i) = index {
            return Ok(Anchor::Element(self.element_by_index(app, i)?));
        }
        match (x, y) {
            (Some(x), Some(y)) => {
                let map = self.state(app.pid).ok().and_then(|s| s.coord).ok_or_else(|| {
                    Error::InvalidArgs(format!(
                        "no current screenshot to map ({x}, {y}); call get_app_state first, or use element_index"
                    ))
                })?;
                Ok(Anchor::Point(map.to_screen(x, y)?))
            }
            _ => Err(Error::InvalidArgs(format!(
                "{what} needs either element_index or both x and y"
            ))),
        }
    }

    /// Screen point for an anchor (element center for element anchors).
    fn anchor_point(&self, app: &AppInfo, anchor: &Anchor) -> Result<Point> {
        match anchor {
            Anchor::Point(p) => Ok(*p),
            Anchor::Element(h) => self
                .state(app.pid)
                .ok()
                .and_then(|s| s.bounds.get(h))
                .filter(|b| !b.is_empty())
                .map(|b| b.center())
                .ok_or_else(|| {
                    Error::ActionFailed("this element has no on-screen position to click".into())
                }),
        }
    }

    fn settle(&self) {
        (self.sleep)(Duration::from_millis(self.store.config.timing.settle_ms));
    }

    /// After an action on `app`: pause `timing.settle_ms`, then (adaptive)
    /// re-read the app until two reads in a row agree — the UI has finished
    /// reacting — or `settle_max_ms` passes. Leaves a fresh snapshot for
    /// verification and the change report.
    fn settle_on(&mut self, app: &AppInfo) {
        use crate::config::SettleMode;
        self.settle();
        let cfg = &self.store.config;
        let adaptive = cfg.timing.settle == SettleMode::Adaptive;
        if !adaptive && !cfg.verify.enabled && !cfg.tree.report_changes {
            return;
        }
        let max = Duration::from_millis(cfg.timing.settle_max_ms);
        let poll = Duration::from_millis(cfg.timing.settle_poll_ms.max(5));
        let deadline = (self.clock)() + max;
        let mut last = None;
        // Text read off the screen isn't read again for every look.
        let reuse = std::mem::replace(&mut self.ocr_reuse, true);
        while let Ok(window) = self.pick_window(app, None, true) {
            if self.observe(app, &window, true).is_err() {
                break;
            }
            self.settled = Some(self.epoch);
            let now = self.tree_fingerprint(app.pid);
            if !adaptive || (now.is_some() && now == last) {
                break;
            }
            last = now;
            if self.is_stopped() || (self.clock)() >= deadline {
                break;
            }
            (self.sleep)(poll);
        }
        self.ocr_reuse = reuse;
    }

    /// Whether the current action's result was read back (verification on).
    fn verified(&self) -> bool {
        self.store.config.verify.enabled && self.settled == Some(self.epoch)
    }

    /// A cheap identity of what the latest snapshot of an app shows: its
    /// windows, and every element's text, state and position.
    fn tree_fingerprint(&self, pid: u32) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let st = self.states.get(&pid).filter(|s| s.stamped)?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        st.window_id.hash(&mut h);
        let mut windows = st.seen_windows.clone();
        windows.sort_unstable();
        windows.hash(&mut h);
        for n in &st.nodes {
            n.key.hash(&mut h);
            n.line.hash(&mut h);
            if let Some(b) = n.bounds {
                [b.x, b.y, b.width, b.height]
                    .map(|v| v.round() as i64)
                    .hash(&mut h);
            }
        }
        Some(h.finish())
    }

    /// Point the agent cursor at a click/scroll anchor.
    fn overlay_anchor(&mut self, app: &AppInfo, anchor: &Anchor, click: bool) {
        match anchor {
            Anchor::Element(h) => self.overlay_point_element(app, *h, click),
            Anchor::Point(p) => self.overlay_point(*p, click),
        }
    }

    // -- tool dispatch -----------------------------------------------------

    /// Run one tool call.
    pub fn call(&mut self, call: ToolCall, approver: &mut dyn Approver) -> Result<ToolOutput> {
        if self.is_stopped() {
            // Show it again so the user sees why nothing happens.
            if self.depth == 0 && self.overlay.is_some() {
                self.overlay_send(OverlayCmd::Stopped { on: true });
            }
            return Err(self.stopped_error());
        }
        self.depth += 1;
        if self.depth == 1 {
            self.overlay_send(OverlayCmd::Begin);
        }
        let out = self.dispatch(call, approver);
        if self.depth == 1 && self.overlay.is_some() {
            let ok = out.as_ref().is_ok_and(|o| !o.is_error);
            self.overlay_send(OverlayCmd::End { ok });
        }
        self.depth -= 1;
        if self.depth == 0 {
            // Only the image in the final result reaches the model.
            if out.as_ref().is_ok_and(|o| o.image.is_some()) {
                self.commit_images();
            } else {
                self.pending_images.clear();
            }
        }
        out
    }

    fn dispatch(&mut self, call: ToolCall, approver: &mut dyn Approver) -> Result<ToolOutput> {
        if !self.store.config.tools.is_enabled(call.name()) {
            return Err(Error::Blocked(
                call.name().into(),
                "this tool is disabled in settings ([tools])".into(),
            ));
        }
        // For mutating actions, remember which app to re-inspect afterwards.
        let acting = mutating_app(&call);
        let mutating = acting.is_some()
            || matches!(call, ToolCall::LaunchApp(_))
            || matches!(&call, ToolCall::Window(w) if w.action.mutating());
        if mutating {
            // Anything read before this action is stale now.
            self.epoch += 1;
            // Don't act while the user is using the mouse or keyboard.
            self.wait_for_user()?;
        }
        let report_app = acting.filter(|_| self.store.config.tree.report_changes);
        let out = match call {
            ToolCall::ListApps => self.list_apps(),
            ToolCall::LaunchApp(a) => self.launch_app(a, approver),
            ToolCall::GetAppState(a) => self.get_app_state(a, approver),
            ToolCall::Click(a) => self.click(a, approver),
            ToolCall::PerformSecondaryAction(a) => self.perform_secondary(a, approver),
            ToolCall::SetValue(a) => self.set_value(a, approver),
            ToolCall::SelectText(a) => self.select_text(a, approver),
            ToolCall::Scroll(a) => self.scroll(a, approver),
            ToolCall::Drag(a) => self.drag(a, approver),
            ToolCall::PressKey(a) => self.press_key(a, approver),
            ToolCall::TypeText(a) => self.type_text(a, approver),
            ToolCall::FindElement(a) => self.find_element(a, approver),
            ToolCall::WaitFor(a) => self.wait_for(a, approver),
            ToolCall::Screenshot(a) => self.screenshot(a, approver),
            ToolCall::Batch(a) => self.batch(a, approver),
            ToolCall::GetClipboard => self.get_clipboard(),
            ToolCall::SetClipboard(a) => self.set_clipboard(a),
            ToolCall::CreateFolder(a) => self.create_folder(a),
            ToolCall::Window(a) => self.window_tool(a, approver),
            ToolCall::GetNotifications(a) => self.get_notifications(a),
        };
        if mutating {
            self.last_input = Some((self.clock)());
        }
        let out = out?;
        match report_app {
            Some(app) => Ok(self.append_changes(&app, out)),
            None => Ok(out),
        }
    }

    /// Parse and run a raw tool call, returning a tool result even on error.
    pub fn call_tool(
        &mut self,
        name: &str,
        args: serde_json::Value,
        approver: &mut dyn Approver,
    ) -> ToolOutput {
        self.reload_if_changed();
        let app = args.get("app").and_then(|v| v.as_str()).map(str::to_string);
        let out = match ToolCall::parse(name, args).and_then(|c| self.call(c, approver)) {
            Ok(out) => out,
            Err(e) => ToolOutput::error(&e),
        };
        self.audit(name, app.as_deref(), &out);
        out
    }

    /// Append a bounded JSONL record of the call, if auditing is enabled. Only
    /// metadata is written — never arguments, tree text or screenshots.
    fn audit(&self, tool: &str, app: Option<&str>, out: &ToolOutput) {
        let audit = &self.store.config.audit;
        if !audit.enabled {
            return;
        }
        let path = audit
            .path
            .clone()
            .unwrap_or_else(|| crate::config::home_dir().join("audit.log"));
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let record = serde_json::json!({
            "ts": ts,
            "tool": tool,
            "app": app,
            "ok": !out.is_error,
            "summary": out.text.lines().next().unwrap_or("").chars().take(160).collect::<String>(),
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write as _;
            let _ = writeln!(f, "{record}");
        }
    }

    fn list_apps(&mut self) -> Result<ToolOutput> {
        let apps = self.find_apps()?;
        if apps.is_empty() {
            return Ok(ToolOutput::text("No GUI apps are currently running."));
        }
        let mut lines = vec![format!("{} running apps:", apps.len())];
        for a in &apps {
            let mut tags = Vec::new();
            if a.frontmost {
                tags.push("frontmost".to_string());
            }
            if a.hidden {
                tags.push("hidden".to_string());
            }
            if let Some(t) = policy::tag(a, &self.store, &self.session_allowed) {
                tags.push(t);
            }
            let tags = if tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", tags.join(", "))
            };
            lines.push(format!(
                "- {} (id: {}, pid: {}){}",
                a.name, a.id, a.pid, tags
            ));
        }
        Ok(ToolOutput::text(lines.join("\n")))
    }

    fn launch_app(
        &mut self,
        args: LaunchAppArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        // Enforce policy against a synthetic app record so a blocked category
        // (e.g. terminals) can't be launched-and-driven around the block.
        let probe = AppInfo {
            name: args.app.clone(),
            id: args.app.clone(),
            pid: 0,
            exe: Some(args.app.clone()),
            frontmost: false,
            hidden: false,
        };
        if let Verdict::Blocked(reason) =
            policy::evaluate(&probe, &self.store, &self.session_allowed)
        {
            return Err(Error::Blocked(args.app.clone(), reason));
        }

        let before: HashSet<u32> = self.find_apps()?.iter().map(|a| a.pid).collect();
        self.backend.launch_app(&args.app)?;

        let deadline = (self.clock)()
            + Duration::from_secs_f64(self.store.config.launch_timeout_secs.max(0.5));
        let ql = args.app.to_lowercase();
        loop {
            let apps = self.find_apps()?;
            // Prefer a newly-appeared app that matches the query.
            let found = apps
                .iter()
                .find(|a| {
                    !before.contains(&a.pid) && a.match_keys().iter().any(|k| k.contains(&ql))
                })
                .or_else(|| {
                    apps.iter()
                        .find(|a| a.match_keys().iter().any(|k| k.contains(&ql)))
                });
            if let Some(app) = found {
                let app = app.clone();
                // Authorize now so the model can act right away.
                let _ = self.authorize(&app.id, "launch_app", approver);
                return Ok(ToolOutput::text(format!(
                    "Launched {} (id: {}, pid: {}). Call get_app_state to see it.",
                    app.name, app.id, app.pid
                )));
            }
            if self.is_stopped() {
                return Err(self.stopped_error());
            }
            if (self.clock)() >= deadline {
                return Ok(ToolOutput::text(format!(
                    "Requested launch of `{}`. It hasn't shown a window yet; call list_apps or get_app_state shortly.",
                    args.app
                )));
            }
            (self.sleep)(Duration::from_millis(200));
        }
    }

    fn get_app_state(
        &mut self,
        args: GetAppStateArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "get_app_state", approver)?;
        let window = self.resolve_window(&app, args.window.as_deref(), false)?;
        self.force_ocr = args.ocr;
        let observed = self.observe(&app, &window, args.ocr);
        self.force_ocr = false;
        observed?;
        let r = self.render(app.pid, args.disable_diff)?;
        let size_changed = self.commit(app.pid, &window);

        let mut header = format!(
            "App: {} ({}, pid {}) · window \"{}\" (id {}",
            app.name, app.id, app.pid, window.title, window.id
        );
        if let Some(b) = window.bounds {
            header.push_str(&format!(
                ", {:.0}x{:.0} at {:.0},{:.0}",
                b.width, b.height, b.x, b.y
            ));
        }
        header.push_str(&format!(") · screen #{}", r.screen));
        match r.seen {
            Seen::New => header.push_str(" (new)"),
            Seen::Revisit => header.push_str(" (seen before)"),
            Seen::Same => {}
        }
        let ocr_lines = self.state(app.pid).map(|s| s.ocr_lines).unwrap_or(0);
        if ocr_lines > 0 {
            let how =
                "\"ocr text\" elements: click them by element_index; they can't be set or selected";
            header.push_str(&if args.ocr {
                format!("\nRead {ocr_lines} more line(s) of text off the screen ({how}).")
            } else {
                format!(
                    "\nThis window has little accessibility information, so {ocr_lines} line(s) of text were read off the screen ({how})."
                )
            });
        }
        if let Some(note) = &self.ocr_note
            && !self.ocr_note_shown
            && (args.ocr || r.interactive < self.store.config.ocr.sparse_threshold)
        {
            header.push_str(&format!("\n[Text recognition unavailable: {note}]"));
            self.ocr_note_shown = true;
        }

        // Decide whether this view needs pixels (screenshot.attach): a screen
        // the model has no picture of, a big change, a returning screen whose
        // window changed size, or custom-drawn UI with little in the tree.
        let shot = &self.store.config.screenshot;
        let cache = &self.store.config.cache;
        let allowed = shot.enabled && !self.store.config.text_only;
        let known = self
            .state(app.pid)?
            .known
            .as_ref()
            .ok_or(Error::Internal("no known screen".into()))?;
        let want = allowed
            && args.screenshot.unwrap_or(match shot.attach {
                AttachMode::Always => true,
                AttachMode::Never => false,
                AttachMode::Auto => {
                    !known.shot
                        || r.large_change
                        || size_changed
                        || r.interactive < shot.auto_sparse_threshold
                }
            });
        let (dedupe, grid, tolerance) = (
            cache.dedupe_screenshots && args.screenshot != Some(true),
            cache.pixel_grid,
            cache.pixel_tolerance,
        );
        // Pixel fingerprints tell unchanged pictures and changed parts apart.
        let fingerprint = cache.dedupe_screenshots || shot.scope == crate::config::ShotScope::Auto;
        let (known_pixels, known_coord, known_shot) =
            (known.pixels.clone(), known.coord, known.shot);

        let mut image = None;
        if want {
            // The picture just read for OCR, if any, is the screenshot.
            let reuse = match self.last_capture.take() {
                Some((pid, wid, epoch, cap))
                    if pid == app.pid && wid == window.id && epoch == self.epoch =>
                {
                    Some(cap)
                }
                _ => None,
            };
            let captured = match reuse {
                Some(cap) => Ok(cap),
                None => self.capture_clean(|b| b.capture(&app, &window)),
            };
            match captured {
                Ok(mut cap) => {
                    let redacted = self.redact_capture(&mut cap);
                    if redacted > 0 {
                        header.push_str(&format!(
                            "\n[{redacted} private area(s) blacked out of the screenshot]"
                        ));
                    }
                    let sig = fingerprint.then(|| PixelSig::of(&cap, grid));
                    let unchanged = dedupe
                        && known_coord.is_some_and(|c| c.bounds == cap.bounds)
                        && matches!((&sig, &known_pixels), (Some(a), Some(b)) if a.same_as(b, tolerance));
                    if unchanged {
                        // The model already has this picture.
                        if let Some(st) = self.states.get_mut(&app.pid) {
                            st.coord = known_coord;
                        }
                        header.push_str(
                            "\nScreenshot: unchanged since you last saw it, not re-sent (screenshot=true forces one).",
                        );
                    } else if let Some(part) = (args.screenshot != Some(true))
                        .then(|| {
                            self.changed_part(
                                &cap,
                                sig.as_ref(),
                                known_pixels.as_ref(),
                                known_coord,
                            )
                        })
                        .flatten()
                        && let Some(full) = known_coord
                        && let Ok((img, (ox, oy))) =
                            imaging::encode_part(&cap, part, &full, &self.store.config.screenshot)
                    {
                        // Only the part that changed, placed in the picture
                        // the model already has.
                        header.push_str(&format!(
                            "\nScreenshot: only the part that changed, {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of your earlier screenshot of this screen (same scale; the rest is unchanged, and x/y coordinates still refer to that whole screenshot).",
                            w = img.width,
                            h = img.height,
                            x1 = ox + img.width,
                            y1 = oy + img.height,
                        ));
                        self.pending_images.push(PendingImage {
                            pid: app.pid,
                            screen: r.screen,
                            coord: full,
                            pixels: sig,
                        });
                        image = Some(img);
                    } else {
                        match imaging::encode(cap, &self.store.config.screenshot) {
                            Ok((img, map)) => {
                                header.push_str(&format!(
                                    "\nScreenshot: {}x{} px.",
                                    img.width, img.height
                                ));
                                self.pending_images.push(PendingImage {
                                    pid: app.pid,
                                    screen: r.screen,
                                    coord: map,
                                    pixels: sig,
                                });
                                image = Some(img);
                            }
                            Err(e) => {
                                self.mark_shot(app.pid);
                                header.push_str(&format!("\n[screenshot encode failed: {e}]"));
                            }
                        }
                    }
                }
                Err(e) => {
                    self.mark_shot(app.pid);
                    header.push_str(&format!("\n[screenshot unavailable: {e}]"));
                }
            }
        } else if allowed && r.seen == Seen::Revisit && known_shot && known_coord.is_some() {
            header.push_str(&format!(
                "\nScreenshot: not re-sent (your earlier one of screen #{} still applies).",
                r.screen
            ));
        } else if allowed {
            header.push_str("\nScreenshot: not attached (pass screenshot=true for one).");
        }

        Ok(ToolOutput {
            text: format!("{header}\nTree:\n{}", r.text),
            image,
            is_error: false,
        })
    }

    /// With `screenshot.scope = "auto"`: the pixel area of a new capture worth
    /// sending, when the model already has a picture of this screen at this
    /// size and only a small part of it changed.
    fn changed_part(
        &self,
        cap: &Capture,
        sig: Option<&PixelSig>,
        known: Option<&PixelSig>,
        coord: Option<CoordMap>,
    ) -> Option<(u32, u32, u32, u32)> {
        let cfg = &self.store.config.screenshot;
        if cfg.scope != crate::config::ShotScope::Auto {
            return None;
        }
        coord.filter(|c| c.bounds == cap.bounds)?;
        let area = sig?.changed_area(known?, self.store.config.cache.pixel_tolerance)?;
        let part = imaging::widen(
            area,
            cfg.region_padding,
            cfg.region_min_size,
            cap.width,
            cap.height,
        );
        let share = f64::from(part.2) * f64::from(part.3)
            / (f64::from(cap.width) * f64::from(cap.height)).max(1.0);
        (share <= cfg.region_max_ratio).then_some(part)
    }

    /// A screenshot of the known screen was attempted and failed: don't
    /// retry it on every view.
    fn mark_shot(&mut self, pid: u32) {
        if let Some(k) = self.states.get_mut(&pid).and_then(|s| s.known.as_mut()) {
            k.shot = true;
        }
    }

    fn click(&mut self, args: ClickArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "click", approver)?;
        let count = args.click_count.clamp(1, 3);
        let anchor = self.anchor(&app, args.element_index, args.x, args.y, "click")?;
        // What things looked like, to tell whether the click did anything.
        let before = self.tree_fingerprint(app.pid);
        let what = self.describe_anchor(&app, &anchor);
        let point = self.anchor_point(&app, &anchor).ok();

        // The agent cursor goes there first, so the user sees what is next.
        self.overlay_anchor(&app, &anchor, false);
        // Confirm consequential presses (Send / Delete / Pay …) when guarded.
        let mut guarded = false;
        if let Anchor::Element(h) = &anchor {
            let label = self.describe(&app, *h);
            guarded = self.guard_action(&label, approver)?;
        }
        self.overlay_anchor(&app, &anchor, true);

        // A single left click on an element with a press action goes through
        // the accessibility API so it works in the background.
        let mut note = String::new();
        if let (Anchor::Element(h), MouseButton::Left, 1) = (&anchor, args.button, count) {
            let node = self.node_for_handle(&app, *h);
            if let Some(action) = node
                .and_then(|n| n.has_action("press"))
                .map(|a| a.native.clone())
            {
                match self.backend.perform_action(*h, &action) {
                    Ok(()) => {
                        self.settle_on(&app);
                        let unchanged = self.verified() && self.tree_fingerprint(app.pid) == before;
                        let v = &self.store.config.verify;
                        if unchanged
                            && v.retry
                            && v.retry_on_no_change
                            && !guarded
                            && let Some(p) = point
                        {
                            // Nothing happened: click it with the mouse.
                            let target = self.input_target(&app);
                            self.backend.click(&target, p, MouseButton::Left, 1)?;
                            self.settle_on(&app);
                            let mut msg = format!(
                                "Pressed {what}; nothing changed, so clicked it with the mouse too."
                            );
                            if self.verified() && self.tree_fingerprint(app.pid) == before {
                                msg.push_str(NO_CHANGE_NOTE);
                            }
                            return Ok(ToolOutput::text(msg));
                        }
                        let mut msg = format!("Pressed {what}.");
                        if unchanged {
                            msg.push_str(NO_CHANGE_NOTE);
                        }
                        return Ok(ToolOutput::text(msg));
                    }
                    Err(e) if self.store.config.verify.retry && point.is_some() => {
                        log::info!("accessibility press failed ({e}); clicking instead");
                        note = format!(
                            " (its accessibility action failed: {e}; clicked it with the mouse instead)"
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        let point = match point {
            Some(p) => p,
            None => self.anchor_point(&app, &anchor)?,
        };
        let target = self.input_target(&app);
        self.backend.click(&target, point, args.button, count)?;
        self.settle_on(&app);
        let verb = match (args.button, count) {
            (MouseButton::Right, _) => "Right-clicked",
            (_, 2) => "Double-clicked",
            (_, 3) => "Triple-clicked",
            _ => "Clicked",
        };
        let mut msg = format!("{verb} {what} at ({:.0}, {:.0}).{note}", point.x, point.y);
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    fn perform_secondary(
        &mut self,
        args: SecondaryActionArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "perform_secondary_action", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        ocr_can_only_be_clicked(handle, "perform_secondary_action")?;
        let action = node.has_action(&args.action).ok_or_else(|| {
            let available: Vec<&str> = node.actions.iter().map(|a| a.name.as_str()).collect();
            Error::InvalidArgs(format!(
                "{} has no action `{}`. Available: [{}]",
                node.label(),
                args.action,
                available.join(", ")
            ))
        })?;
        let native = action.native.clone();
        let before = self.tree_fingerprint(app.pid);
        self.overlay_point_element(&app, handle, false);
        self.guard_action(&node.label(), approver)?;
        self.overlay_point_element(&app, handle, true);
        self.backend.perform_action(handle, &native)?;
        self.settle_on(&app);
        let mut msg = format!("Performed `{}` on {}.", args.action, node.label());
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    fn set_value(&mut self, args: SetValueArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "set_value", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        ocr_can_only_be_clicked(handle, "set_value")?;
        self.overlay_point_element(&app, handle, true);
        // Text fields can be typed into when setting fails.
        let typable = node.states.editable && node.states.checked.is_none();
        let retry = self.store.config.verify.retry && typable;
        let mut how = String::new();
        if let Err(e) = self.backend.set_value(handle, &args.value) {
            if !retry {
                return Err(e);
            }
            log::info!("set_value failed ({e}); typing instead");
            self.retype(&app, handle, &node, &args.value)?;
            how = format!(" (setting it directly failed: {e}; typed it instead)");
        }
        self.settle_on(&app);
        let shown = tree::truncate(&args.value, 80);
        let mut msg = format!("Set {} to \"{shown}\".{how}", node.label());
        if self.verified() && self.value_took(&app, args.element_index, &args.value) == Some(false)
        {
            let fresh = self.node_by_index(&app, args.element_index).ok().cloned();
            if retry
                && how.is_empty()
                && let Some(n) = fresh
            {
                // The value didn't take: type it into the field instead.
                self.retype(&app, n.handle, &n, &args.value)?;
                self.settle_on(&app);
                msg = format!(
                    "Set {} to \"{shown}\" (the value didn't take at first; typed it instead).",
                    node.label()
                );
            }
            if self.verified()
                && self.value_took(&app, args.element_index, &args.value) == Some(false)
            {
                let now = self
                    .node_by_index(&app, args.element_index)
                    .ok()
                    .and_then(|n| n.value.clone())
                    .unwrap_or_default();
                msg.push_str(&format!(
                    " Note: it now shows \"{}\", not the value that was set; check it before going on.",
                    tree::truncate(&now, 80)
                ));
            }
        }
        Ok(ToolOutput::text(msg))
    }

    fn select_text(
        &mut self,
        args: SelectTextArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "select_text", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        ocr_can_only_be_clicked(handle, "select_text")?;
        self.overlay_point_element(&app, handle, false);
        self.backend
            .select_text(handle, args.text.as_deref(), args.occurrence.max(1))?;
        self.settle_on(&app);
        let what = match &args.text {
            Some(t) => format!("\"{}\"", tree::truncate(t, 60)),
            None => "all text".into(),
        };
        Ok(ToolOutput::text(format!(
            "Selected {what} in {}.",
            node.label()
        )))
    }

    fn scroll(&mut self, args: ScrollArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "scroll", approver)?;
        let pages = if args.amount.is_finite() && args.amount > 0.0 {
            args.amount
        } else {
            1.0
        };
        let anchor = self.anchor(&app, args.element_index, args.x, args.y, "scroll")?;
        let before = self.tree_fingerprint(app.pid);
        let what = self.describe_anchor(&app, &anchor);
        let point = self.anchor_point(&app, &anchor).ok();
        self.overlay_anchor(&app, &anchor, false);
        let (ux, uy) = args.direction.unit();
        // ~3 wheel lines per page.
        let lines = (pages * 3.0).round().max(1.0) as i32;

        if let Anchor::Element(h) = &anchor
            && !crate::ocr::is_ocr(*h)
            && let Native::Done(_) = self.backend.scroll_element(*h, args.direction, pages)?
        {
            self.settle_on(&app);
            let mut msg = format!("Scrolled {what} {:?} by {pages} page(s).", args.direction);
            if self.verified() && self.tree_fingerprint(app.pid) == before {
                // Scrolling again does no harm: try the mouse wheel.
                if self.store.config.verify.retry
                    && let Some(p) = point
                {
                    let target = self.input_target(&app);
                    self.backend
                        .scroll_wheel(&target, p, ux * lines, uy * lines)?;
                    self.settle_on(&app);
                    msg = format!(
                        "Scrolled {what} {:?} by {pages} page(s) (with the mouse wheel; the first try didn't move it).",
                        args.direction
                    );
                }
                if self.verified() && self.tree_fingerprint(app.pid) == before {
                    msg.push_str(" Nothing moved: it may already be at the end.");
                }
            }
            return Ok(ToolOutput::text(msg));
        }

        let point = match point {
            Some(p) => p,
            None => self.anchor_point(&app, &anchor)?,
        };
        let target = self.input_target(&app);
        self.backend
            .scroll_wheel(&target, point, ux * lines, uy * lines)?;
        self.settle_on(&app);
        let mut msg = format!(
            "Scrolled {:?} by {pages} page(s) at ({:.0}, {:.0}).",
            args.direction, point.x, point.y
        );
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(" Nothing moved: it may already be at the end, or not scrollable there.");
        }
        Ok(ToolOutput::text(msg))
    }

    fn drag(&mut self, args: DragArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "drag", approver)?;
        let from = self.anchor(
            &app,
            args.from_element_index,
            args.from_x,
            args.from_y,
            "drag source",
        )?;
        let to = self.anchor(
            &app,
            args.to_element_index,
            args.to_x,
            args.to_y,
            "drag target",
        )?;
        let (p0, p1) = (
            self.anchor_point(&app, &from)?,
            self.anchor_point(&app, &to)?,
        );
        let before = self.tree_fingerprint(app.pid);
        self.overlay_point(p0, true);
        self.overlay_point(p1, false);
        let target = self.input_target(&app);
        self.backend.drag(&target, p0, p1)?;
        self.settle_on(&app);
        let mut msg = format!(
            "Dragged from ({:.0}, {:.0}) to ({:.0}, {:.0}).",
            p0.x, p0.y, p1.x, p1.y
        );
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    fn press_key(&mut self, args: PressKeyArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "press_key", approver)?;
        let combos = keys::parse_sequence(&args.key)?;
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            let node = self.node_by_index(&app, i)?.clone();
            self.overlay_point_element(&app, h, false);
            self.focus_element(&app, h, &node)?;
        }
        let target = self.input_target(&app);
        for combo in &combos {
            if self.is_stopped() {
                return Err(self.stopped_error());
            }
            self.backend.press_key(&target, combo)?;
            (self.sleep)(Duration::from_millis(self.store.config.timing.key_delay_ms));
        }
        self.settle_on(&app);
        let shown: Vec<String> = combos.iter().map(|c| c.to_string()).collect();
        Ok(ToolOutput::text(format!("Pressed {}.", shown.join(" "))))
    }

    fn type_text(&mut self, args: TypeTextArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "type_text", approver)?;
        if args.text.is_empty() {
            return Err(Error::InvalidArgs("`text` must not be empty".into()));
        }
        let mut field = None;
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            let node = self.node_by_index(&app, i)?.clone();
            self.overlay_point_element(&app, h, true);
            self.focus_element(&app, h, &node)?;
            self.settle();
            field = Some((i, node));
        }
        self.type_into_focus(&app, &args.text)?;
        self.settle_on(&app);
        let mut msg = format!("Typed {} character(s).", args.text.chars().count());

        // Typed into a field whose text we can read: did it land?
        if let Some((i, node)) = field
            && self.verified()
            && node.states.editable
            && node.value.is_some()
        {
            let unchanged = |e: &Self| {
                e.node_by_index(&app, i)
                    .ok()
                    .is_some_and(|n| n.value.is_some() && n.value == node.value)
            };
            if unchanged(self) {
                let fresh = self.node_by_index(&app, i).ok().cloned();
                let point = fresh
                    .as_ref()
                    .and_then(|n| n.bounds)
                    .filter(|b| !b.is_empty())
                    .map(|b| b.center());
                if self.store.config.verify.retry
                    && let Some(p) = point
                {
                    // Click into the field and type again.
                    let target = self.input_target(&app);
                    self.backend.click(&target, p, MouseButton::Left, 1)?;
                    self.settle();
                    self.type_into_focus(&app, &args.text)?;
                    self.settle_on(&app);
                    msg.push_str(
                        " The text didn't land in the field at first, so it was clicked and the text typed again (the first attempt may have gone to another element).",
                    );
                }
                if self.verified() && unchanged(self) {
                    msg.push_str(
                        " Note: the field's text didn't change; the typing may have gone elsewhere. Check with get_app_state.",
                    );
                }
            }
        }
        Ok(ToolOutput::text(msg))
    }

    /// Type `text` into the app's focused element (newlines press Return).
    fn type_into_focus(&mut self, app: &AppInfo, text: &str) -> Result<()> {
        let target = self.input_target(app);
        // Split on newlines so each becomes a Return press (works everywhere).
        let mut first = true;
        for segment in text.split('\n') {
            if self.is_stopped() {
                return Err(self.stopped_error());
            }
            if !first {
                self.backend.press_key(
                    &target,
                    &KeyCombo {
                        modifiers: keys::Modifiers::default(),
                        key: Key::Named(NamedKey::Return),
                    },
                )?;
            }
            if !segment.is_empty() {
                self.backend.type_text(&target, segment)?;
            }
            first = false;
        }
        Ok(())
    }

    /// Give an element keyboard focus; a text field that won't take focus
    /// through accessibility is clicked instead (when retries are on).
    fn focus_element(&mut self, app: &AppInfo, handle: ElementHandle, node: &Node) -> Result<()> {
        let ocr = crate::ocr::is_ocr(handle);
        let focused = !ocr && matches!(self.backend.focus(handle), Ok(Native::Done(_)));
        // Text read off the screen is focused by clicking it.
        if !focused
            && (ocr || self.store.config.verify.retry && node.states.editable)
            && let Some(p) = node.bounds.filter(|b| !b.is_empty()).map(|b| b.center())
        {
            let target = self.input_target(app);
            self.backend.click(&target, p, MouseButton::Left, 1)?;
        }
        Ok(())
    }

    /// Replace a text field's contents by typing: focus it, select all, type.
    fn retype(
        &mut self,
        app: &AppInfo,
        handle: ElementHandle,
        node: &Node,
        text: &str,
    ) -> Result<()> {
        self.focus_element(app, handle, node)?;
        if self.backend.select_text(handle, None, 1).is_err() {
            let target = self.input_target(app);
            self.backend
                .press_key(&target, &keys::parse_combo("primary+a")?)?;
        }
        self.type_into_focus(app, text)
    }

    /// Whether element `index` now holds `want` (after a set_value). `None`
    /// when it can't be told (value not exposed, hidden for privacy, gone).
    fn value_took(&self, app: &AppInfo, index: u32, want: &str) -> Option<bool> {
        let n = self.node_by_index(app, index).ok()?;
        if crate::privacy::is_password(&n.role) {
            return None;
        }
        let truthy = |s: &str| {
            matches!(
                s.trim().to_lowercase().as_str(),
                "true" | "1" | "on" | "checked" | "yes"
            )
        };
        if let Some(c) = n.states.checked {
            return Some(c == truthy(want));
        }
        let got = n.value.as_deref()?;
        if let (Ok(a), Ok(b)) = (got.trim().parse::<f64>(), want.trim().parse::<f64>()) {
            return Some((a - b).abs() <= 1e-6_f64.max(b.abs() * 0.01));
        }
        let cfg = &self.store.config.privacy;
        // Compare the way the value is shown (card numbers are masked).
        let want = if cfg.redact_card_numbers {
            crate::privacy::mask_card_numbers(want).unwrap_or_else(|| want.to_string())
        } else {
            want.to_string()
        };
        if got.contains(crate::privacy::MASK) && !want.contains(crate::privacy::MASK) {
            return None;
        }
        Some(got.trim_end() == want.trim_end())
    }

    // -- new tools ---------------------------------------------------------

    fn find_element(
        &mut self,
        args: FindElementArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "find_element", approver)?;
        let window = self.resolve_window(&app, args.window.as_deref(), false)?;
        self.observe(&app, &window, false)?;
        let role = args.role.map(|r| r.to_lowercase());
        let name = args.name.map(|n| n.to_lowercase());
        let text = args.text.map(|t| t.to_lowercase());
        let state = self.state(app.pid)?;
        let mut hits: Vec<&Node> = state
            .nodes
            .iter()
            .filter(|n| {
                role.as_deref().is_none_or(|r| n.role == r)
                    && name.as_deref().is_none_or(|q| {
                        n.name
                            .as_deref()
                            .is_some_and(|nm| nm.to_lowercase().contains(q))
                    })
                    && text.as_deref().is_none_or(|q| node_text(n).contains(q))
                    && (!args.editable || n.states.editable)
            })
            .collect();
        hits.truncate(args.max_results.max(1));
        if hits.is_empty() {
            return Ok(ToolOutput::text(format!(
                "No elements in {} match. Try get_app_state to see the whole tree.",
                app.name
            )));
        }
        let mut out = format!("{} matching element(s) in {}:\n", hits.len(), app.name);
        for n in hits {
            out.push_str(&format!("{} {}\n", n.index, n.line));
        }
        Ok(ToolOutput::text(out))
    }

    fn wait_for(&mut self, args: WaitForArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "wait_for", approver)?;
        let role = args.role.clone().map(|r| r.to_lowercase());
        let name = args.name.clone().map(|n| n.to_lowercase());
        let text = args.text.clone().map(|t| t.to_lowercase());
        let timing = &self.store.config.timing;
        let timeout_ms = args.timeout_ms.unwrap_or(timing.wait_timeout_ms).max(1);
        let poll = Duration::from_millis(
            args.poll_ms
                .unwrap_or(timing.wait_poll_ms)
                .clamp(20, 60_000),
        );
        let timeout = Duration::from_millis(timeout_ms);
        let deadline = (self.clock)() + timeout;

        loop {
            if let Ok(window) = self.resolve_window(&app, args.window.as_deref(), true)
                && self.observe(&app, &window, true).is_ok()
            {
                let state = self.state(app.pid)?;
                let found = state.nodes.iter().find(|n| {
                    role.as_deref().is_none_or(|r| n.role == r)
                        && name.as_deref().is_none_or(|q| {
                            n.name
                                .as_deref()
                                .is_some_and(|nm| nm.to_lowercase().contains(q))
                        })
                        && text.as_deref().is_none_or(|q| node_text(n).contains(q))
                        && state_matches(n, args.state)
                });
                if let Some(n) = found {
                    return Ok(ToolOutput::text(format!(
                        "Found after waiting: {} {}",
                        n.index, n.line
                    )));
                }
            }
            if self.is_stopped() {
                return Err(self.stopped_error());
            }
            if (self.clock)() >= deadline {
                return Err(Error::ActionFailed(format!(
                    "timed out after {}ms waiting for an element matching {}",
                    timeout_ms,
                    describe_matcher(&args)
                )));
            }
            (self.sleep)(poll);
        }
    }

    fn screenshot(
        &mut self,
        args: ScreenshotArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        if self.store.config.text_only || !self.store.config.screenshot.enabled {
            return Err(Error::Blocked(
                "screenshot".into(),
                "screenshots are disabled (text_only / screenshot.enabled=false)".into(),
            ));
        }
        let mode = args.mode.unwrap_or(if args.app.is_some() {
            ScreenshotMode::Window
        } else {
            ScreenshotMode::Auto
        });
        // An element to zoom into (screen rect).
        let mut zoom: Option<Rect> = None;
        let (capture, marks, label) = match mode {
            ScreenshotMode::Auto | ScreenshotMode::Full => (
                self.capture_clean(|b| b.capture_screen(None))?,
                None,
                "full screen".to_string(),
            ),
            ScreenshotMode::Region => {
                let (x, y, w, h) = match (args.x, args.y, args.width, args.height) {
                    (Some(x), Some(y), Some(w), Some(h)) if w > 0.0 && h > 0.0 => (x, y, w, h),
                    _ => {
                        return Err(Error::InvalidArgs(
                            "region mode needs x, y, width and height".into(),
                        ));
                    }
                };
                (
                    self.capture_clean(|b| b.capture_screen(Some(Rect::new(x, y, w, h))))?,
                    None,
                    format!("region ({x:.0}, {y:.0}) {w:.0}x{h:.0}"),
                )
            }
            ScreenshotMode::Window => {
                let query = args
                    .app
                    .as_deref()
                    .ok_or_else(|| Error::InvalidArgs("window mode needs `app`".into()))?;
                let app = self.authorize(query, "screenshot", approver)?;
                let window = self.resolve_window(&app, args.window.as_deref(), false)?;
                if crate::privacy::active(&self.store.config.privacy)
                    || args.annotate
                    || args.element_index.is_some()
                {
                    // A current tree: where private data is, the marks, the element.
                    self.observe(&app, &window, false)?;
                }
                let mut label = format!("{} window \"{}\"", app.name, window.title);
                if let Some(i) = args.element_index {
                    let node = self.node_by_index(&app, i)?;
                    zoom = Some(node.bounds.filter(|b| !b.is_empty()).ok_or_else(|| {
                        Error::InvalidArgs(format!(
                            "element {i} ({}) has no on-screen area to show",
                            node.label()
                        ))
                    })?);
                    label = format!("{} in {}", node.label(), app.name);
                }
                let cap = self.capture_clean(|b| b.capture(&app, &window))?;
                let marks = if args.annotate {
                    Some(
                        self.state(app.pid)?
                            .nodes
                            .iter()
                            .filter_map(|n| n.bounds.map(|b| (n.index, b)))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                };
                (cap, marks, label)
            }
        };

        let mut capture = capture;
        let redacted = self.redact_capture(&mut capture);
        if let Some(marks) = marks {
            imaging::annotate(&mut capture, &marks);
        }
        let note = if redacted > 0 {
            format!(" [{redacted} private area(s) blacked out]")
        } else {
            String::new()
        };
        let cfg = self.store.config.screenshot.clone();
        let image = |img: EncodedImage, text: String| ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        };

        // Zoomed in on one element, at up to full resolution.
        if let Some(b) = zoom {
            let sx = f64::from(capture.width) / capture.bounds.width.max(1.0);
            let sy = f64::from(capture.height) / capture.bounds.height.max(1.0);
            let px = (
                ((b.x - capture.bounds.x) * sx).max(0.0) as u32,
                ((b.y - capture.bounds.y) * sy).max(0.0) as u32,
                (b.width * sx).ceil() as u32,
                (b.height * sy).ceil() as u32,
            );
            let px = imaging::widen(px, 8, 0, capture.width, capture.height);
            let (img, _) = imaging::encode(imaging::crop(&capture, px), &cfg)?;
            let text = format!(
                "Screenshot of {label}, zoomed in: {}x{} px. It is its own picture: x/y for actions still refer to get_app_state's screenshot.{note}",
                img.width, img.height
            );
            return Ok(image(img, text));
        }

        // The whole screen, or only what changed since the last one.
        if matches!(mode, ScreenshotMode::Auto | ScreenshotMode::Full) {
            let grid = self.store.config.cache.pixel_grid;
            let tolerance = self.store.config.cache.pixel_tolerance;
            let sig = PixelSig::of(&capture, grid);
            let smart = mode == ScreenshotMode::Auto && cfg.scope == crate::config::ShotScope::Auto;
            if smart && let Some((old, map)) = &self.screen_shot {
                let map = *map;
                if map.bounds == capture.bounds && sig.same_as(old, tolerance) {
                    return Ok(ToolOutput::text(
                        "The screen looks the same as in your last full-screen screenshot; not re-sent (mode=full sends it anyway).",
                    ));
                }
                if let Some(part) = self.changed_part(&capture, Some(&sig), Some(old), Some(map)) {
                    let (img, (ox, oy)) = imaging::encode_part(&capture, part, &map, &cfg)?;
                    let text = format!(
                        "Screenshot: only the part of the screen that changed since your last full-screen screenshot, {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of that screenshot (same scale; the rest is unchanged).{note}",
                        w = img.width,
                        h = img.height,
                        x1 = ox + img.width,
                        y1 = oy + img.height,
                    );
                    self.screen_shot = Some((sig, map));
                    return Ok(image(img, text));
                }
            }
            let (img, map) = imaging::encode(capture, &cfg)?;
            self.screen_shot = Some((sig, map));
            let text = format!(
                "Screenshot of {label}: {}x{} px.{note}",
                img.width, img.height
            );
            return Ok(image(img, text));
        }

        let (img, _map) = imaging::encode(capture, &cfg)?;
        let text = format!(
            "Screenshot of {label}: {}x{} px.{note}",
            img.width, img.height
        );
        Ok(image(img, text))
    }

    fn batch(&mut self, args: BatchArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        if args.steps.is_empty() {
            return Err(Error::InvalidArgs("batch needs at least one step".into()));
        }
        let mut report = String::new();
        let mut last_image = None;
        let mut any_error = false;
        for (i, step) in args.steps.iter().enumerate() {
            // Inject the default app when the step omits one.
            let mut step_args = match &step.arguments {
                serde_json::Value::Object(m) => m.clone(),
                serde_json::Value::Null => serde_json::Map::new(),
                other => {
                    report.push_str(&format!(
                        "{}. {} — bad arguments (expected an object, got {other})\n",
                        i + 1,
                        step.tool
                    ));
                    any_error = true;
                    break;
                }
            };
            if !step_args.contains_key("app")
                && let Some(app) = &args.app
            {
                step_args.insert("app".into(), serde_json::json!(app));
            }
            let parsed = ToolCall::parse(&step.tool, serde_json::Value::Object(step_args));
            let before = self.pending_images.len();
            let result = parsed.and_then(|c| self.call(c, approver));
            match result {
                Ok(out) => {
                    let first = out.text.lines().next().unwrap_or("");
                    report.push_str(&format!("{}. {} — {first}\n", i + 1, step.tool));
                    if out.image.is_some() {
                        // Earlier images are replaced by this one.
                        self.pending_images.drain(..before);
                        last_image = out.image;
                    }
                    if out.is_error {
                        any_error = true;
                        if !args.continue_on_error {
                            break;
                        }
                    }
                }
                Err(e) => {
                    report.push_str(&format!("{}. {} — ERROR: {e}\n", i + 1, step.tool));
                    any_error = true;
                    if !args.continue_on_error || matches!(e, Error::Stopped(_)) {
                        break;
                    }
                }
            }
        }
        Ok(ToolOutput {
            text: format!("Ran {} step(s):\n{report}", args.steps.len()),
            image: last_image,
            is_error: any_error,
        })
    }

    fn window_tool(&mut self, args: WindowArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        use crate::tools::WindowAction as A;
        if args.action == A::Displays {
            let displays = self.backend.displays()?;
            let mut out = format!("{} display(s), in screen coordinates:\n", displays.len());
            for d in &displays {
                out.push_str(&format!(
                    "- display {}{}: {} (usable area {})\n",
                    d.index,
                    if d.primary { " (primary)" } else { "" },
                    show_rect(d.bounds),
                    show_rect(d.work_area)
                ));
            }
            match self.backend.desktops() {
                Some((n, cur)) => out.push_str(&format!(
                    "Virtual desktops: {n} (current: {cur}; numbered from 0).\n"
                )),
                None => out.push_str("Virtual desktops: not available here.\n"),
            }
            return Ok(ToolOutput::text(out));
        }
        let query = args
            .app
            .as_deref()
            .ok_or_else(|| Error::InvalidArgs("this window action needs `app`".into()))?;
        let app = self.authorize(query, "window", approver)?;
        if args.action == A::List {
            let windows = self.list_windows(&app, true)?;
            if windows.is_empty() {
                return Ok(ToolOutput::text(format!(
                    "{} has no open windows.",
                    app.name
                )));
            }
            let mut out = format!("{} window(s) of {}:\n", windows.len(), app.name);
            for w in &windows {
                out.push_str(&format!("- {}\n", describe_window(w)));
            }
            return Ok(ToolOutput::text(out));
        }

        let window = self.resolve_window(&app, args.window.as_deref(), true)?;
        let bounds = window.bounds;
        // The display to place it on: the one asked for, else its own.
        let displays = self.backend.displays().unwrap_or_default();
        let display =
            match args.display {
                Some(i) => Some(displays.iter().find(|d| d.index == i).cloned().ok_or_else(
                    || Error::InvalidArgs(format!("no display {i}; see window action=displays")),
                )?),
                None => bounds
                    .and_then(|b| {
                        displays
                            .iter()
                            .find(|d| d.bounds.contains(b.center()))
                            .or_else(|| displays.iter().find(|d| d.primary))
                            .or(displays.first())
                    })
                    .cloned(),
            };
        let area = || {
            display.as_ref().map(|d| d.work_area).ok_or_else(|| {
                Error::Unsupported("the display layout is not known on this platform".into())
            })
        };
        let size =
            || bounds.ok_or_else(|| Error::ActionFailed("the window's size is not known".into()));
        let (op, what) = match args.action {
            A::Displays | A::List => unreachable!("handled above"),
            A::Focus => (WindowOp::Focus, "Focused".to_string()),
            A::Maximize if args.display.is_some() => {
                (WindowOp::SetBounds(area()?), "Maximized".to_string())
            }
            A::Maximize => (WindowOp::Maximize, "Maximized".to_string()),
            A::Minimize => (WindowOp::Minimize, "Minimized".to_string()),
            A::Restore => (WindowOp::Restore, "Restored".to_string()),
            A::Fullscreen => (WindowOp::Fullscreen(true), "Made full screen".to_string()),
            A::ExitFullscreen => (WindowOp::Fullscreen(false), "Left full screen".to_string()),
            A::Close => (WindowOp::Close, "Asked to close".to_string()),
            A::Move => {
                let (x, y) = match (args.x, args.y) {
                    (Some(x), Some(y)) => (x, y),
                    _ => return Err(Error::InvalidArgs("move needs x and y".into())),
                };
                let b = size()?;
                let r = Rect::new(
                    x,
                    y,
                    args.width.unwrap_or(b.width),
                    args.height.unwrap_or(b.height),
                );
                (WindowOp::SetBounds(r), "Moved".to_string())
            }
            A::Resize => {
                let b = size()?;
                let (w, h) = match (args.width, args.height) {
                    (None, None) => {
                        return Err(Error::InvalidArgs(
                            "resize needs width and/or height".into(),
                        ));
                    }
                    (w, h) => (w.unwrap_or(b.width), h.unwrap_or(b.height)),
                };
                if !(w >= 1.0 && h >= 1.0) {
                    return Err(Error::InvalidArgs(
                        "width and height must be positive".into(),
                    ));
                }
                (
                    WindowOp::SetBounds(Rect::new(b.x, b.y, w, h)),
                    "Resized".to_string(),
                )
            }
            A::TileLeft | A::TileRight | A::TileTop | A::TileBottom => {
                let a = area()?;
                let (hw, hh) = (a.width / 2.0, a.height / 2.0);
                let r = match args.action {
                    A::TileLeft => Rect::new(a.x, a.y, hw, a.height),
                    A::TileRight => Rect::new(a.x + hw, a.y, a.width - hw, a.height),
                    A::TileTop => Rect::new(a.x, a.y, a.width, hh),
                    _ => Rect::new(a.x, a.y + hh, a.width, a.height - hh),
                };
                (WindowOp::SetBounds(r), "Tiled".to_string())
            }
            A::Center | A::MoveToDisplay => {
                let a = area()?;
                let b = size()?;
                let (w, h) = (b.width.min(a.width), b.height.min(a.height));
                let r = Rect::new(a.x + (a.width - w) / 2.0, a.y + (a.height - h) / 2.0, w, h);
                let what = match (args.action, &display) {
                    (A::MoveToDisplay, Some(d)) => format!("Moved to display {}", d.index),
                    _ => "Centered".to_string(),
                };
                (WindowOp::SetBounds(r), what)
            }
            A::MoveToDesktop => {
                let d = args
                    .desktop
                    .ok_or_else(|| Error::InvalidArgs("move_to_desktop needs desktop".into()))?;
                (WindowOp::ToDesktop(d), format!("Moved to desktop {d}"))
            }
        };
        if let WindowOp::SetBounds(r) = op
            && !(r.width >= 1.0 && r.height >= 1.0 && r.x.is_finite() && r.y.is_finite())
        {
            return Err(Error::InvalidArgs(format!(
                "invalid window area {}",
                show_rect(r)
            )));
        }
        if let (WindowOp::SetBounds(r), Some(_)) = (op, &bounds) {
            self.overlay_send(OverlayCmd::Target {
                rect: Some([r.x, r.y, r.width, r.height]),
            });
        }
        self.backend.window_op(&app, &window, &op)?;
        self.settle();

        // The window's screenshot no longer lines up: the next
        // get_app_state takes a fresh one.
        if let Some(st) = self.states.get_mut(&app.pid) {
            st.coord = None;
            if let Some(k) = st.known.as_mut() {
                k.shot = false;
                k.coord = None;
                k.pixels = None;
            }
        }
        let now = self
            .list_windows(&app, true)
            .ok()
            .and_then(|ws| ws.into_iter().find(|w| w.id == window.id));
        let mut msg = format!("{what} \"{}\" of {}.", window.title, app.name);
        match now {
            Some(w) => msg.push_str(&format!(" Now: {}.", describe_window(&w))),
            None if op == WindowOp::Close => msg.push_str(" It is closed."),
            None => {}
        }
        if let (WindowOp::SetBounds(want), Some(got)) = (op, self.window_bounds(&app, window.id)) {
            let off = (got.x - want.x).abs().max((got.y - want.y).abs());
            let size_off = (got.width - want.width)
                .abs()
                .max((got.height - want.height).abs());
            if self.store.config.verify.enabled && (off > 16.0 || size_off > 16.0) {
                msg.push_str(" (The app or window manager adjusted it; it may have a minimum size or be maximized.)");
            }
        }
        Ok(ToolOutput::text(msg))
    }

    /// A window's current bounds, from the latest window list.
    fn window_bounds(&self, app: &AppInfo, id: u64) -> Option<Rect> {
        self.window_cache
            .get(&app.pid)
            .and_then(|(_, _, ws)| ws.iter().find(|w| w.id == id))
            .and_then(|w| w.bounds)
    }

    fn get_notifications(&mut self, args: NotificationsArgs) -> Result<ToolOutput> {
        let cfg = self.store.config.notifications.clone();
        if !cfg.enabled {
            return Err(Error::Blocked(
                "notifications".into(),
                "reading notifications is off in settings ([notifications] enabled = false)".into(),
            ));
        }
        let all = self.backend.notifications()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut hidden = 0;
        let mut shown = Vec::new();
        for n in all {
            if let Some(q) = &args.app
                && !n.app.to_lowercase().contains(&q.to_lowercase())
            {
                continue;
            }
            // The user's app rules apply to what their notifications say.
            let probe = AppInfo {
                name: n.app.clone(),
                id: n.app.clone(),
                pid: 0,
                exe: None,
                frontmost: false,
                hidden: false,
            };
            let allowed_app =
                cfg.apps.is_empty() || cfg.apps.iter().any(|a| a.eq_ignore_ascii_case(&n.app));
            if !allowed_app
                || matches!(
                    policy::evaluate(&probe, &self.store, &self.session_allowed),
                    Verdict::Blocked(_)
                )
            {
                hidden += 1;
                continue;
            }
            shown.push(n);
        }
        let limit = args.limit.unwrap_or(10).max(1);
        let skip = shown.len().saturating_sub(limit);
        let privacy = &self.store.config.privacy;
        let clean = |s: &str| -> String {
            let mut s = s.to_string();
            if privacy.redact_card_numbers
                && let Some(m) = crate::privacy::mask_card_numbers(&s)
            {
                s = m;
            }
            if cfg.mask_codes
                && let Some(m) = crate::privacy::mask_codes(&s)
            {
                s = m;
            }
            tree::truncate(&s.replace('\n', " / "), 300)
        };
        let mut out = if shown.is_empty() {
            "No recent notifications.".to_string()
        } else {
            format!(
                "{} recent notification(s), newest last:\n",
                shown.len() - skip
            )
        };
        for n in &shown[skip..] {
            let when = match n.time.map(|t| now.saturating_sub(t)) {
                Some(s) if s < 60 => "just now".to_string(),
                Some(s) if s < 3600 => format!("{} min ago", s / 60),
                Some(s) if s < 86_400 => format!("{} h ago", s / 3600),
                Some(s) => format!("{} d ago", s / 86_400),
                None => "on screen".to_string(),
            };
            let app = if n.app.is_empty() {
                "?"
            } else {
                n.app.as_str()
            };
            let body = clean(&n.body);
            out.push_str(&format!("- [{when}] {app} — {}", clean(&n.title)));
            if !body.is_empty() {
                out.push_str(&format!(": {body}"));
            }
            out.push('\n');
        }
        if hidden > 0 {
            out.push_str(&format!(
                "[{hidden} from apps blocked in settings not shown]\n"
            ));
        }
        Ok(ToolOutput::text(out))
    }

    fn get_clipboard(&mut self) -> Result<ToolOutput> {
        if !self.store.config.clipboard {
            return Err(Error::Blocked(
                "clipboard".into(),
                "clipboard access is disabled in config".into(),
            ));
        }
        let text = self.backend.clipboard_get()?;
        Ok(ToolOutput::text(if text.is_empty() {
            "The clipboard is empty.".into()
        } else {
            format!("Clipboard:\n{text}")
        }))
    }

    fn set_clipboard(&mut self, args: SetClipboardArgs) -> Result<ToolOutput> {
        if !self.store.config.clipboard {
            return Err(Error::Blocked(
                "clipboard".into(),
                "clipboard access is disabled in config".into(),
            ));
        }
        self.backend.clipboard_set(&args.text)?;
        Ok(ToolOutput::text(format!(
            "Copied {} character(s) to the clipboard.",
            args.text.chars().count()
        )))
    }

    fn create_folder(&mut self, args: CreateFolderArgs) -> Result<ToolOutput> {
        if !self.store.config.create_folder {
            return Err(Error::Blocked(
                "create_folder".into(),
                "creating folders is disabled in config".into(),
            ));
        }
        let raw = args.path.trim();
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        let path = match (raw.strip_prefix('~'), home) {
            (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
                std::path::PathBuf::from(h).join(rest.trim_start_matches(['/', '\\']))
            }
            _ => std::path::PathBuf::from(raw),
        };
        if raw.is_empty() || !path.is_absolute() {
            return Err(Error::ActionFailed(
                "create_folder needs an absolute path (or one starting with ~).".into(),
            ));
        }
        if path.is_dir() {
            return Ok(ToolOutput::text(format!(
                "The folder already exists: {}",
                path.display()
            )));
        }
        std::fs::create_dir_all(&path).map_err(|e| {
            Error::ActionFailed(format!("could not create {}: {e}", path.display()))
        })?;
        Ok(ToolOutput::text(format!(
            "Created the folder {}",
            path.display()
        )))
    }

    /// The action guard: confirm/refuse a consequential press on `label`.
    /// While such an action waits for the user the overlay turns to the
    /// approval colour; once it runs, to the sensitive-action colour.
    fn guard_action(&mut self, label: &str, approver: &mut dyn Approver) -> Result<bool> {
        use crate::config::SensitiveMode;
        let guard = &self.store.config.guard;
        let low = label.to_lowercase();
        if !guard
            .keywords
            .iter()
            .any(|k| low.contains(&k.to_lowercase()))
        {
            return Ok(false);
        }
        let action = format!("press {label}");
        match guard.mode {
            SensitiveMode::Allow => {}
            SensitiveMode::Block => {
                return Err(Error::Blocked(
                    label.to_string(),
                    "this looks like a consequential action; guard.mode is \"block\". Set guard.mode = \"ask\" or \"allow\" to permit it.".into(),
                ));
            }
            SensitiveMode::Ask => {
                let approved = match self.overlay_approval(&action, approver) {
                    Some(answer) => answer,
                    None => approver.confirm_action(&format!("perform: {label}")),
                };
                self.overlay_send(OverlayCmd::ApprovalDone);
                if !approved {
                    return Err(Error::Denied(format!(
                        "the user declined the action on {label}"
                    )));
                }
            }
        }
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Danger {
                action: Some(action),
            });
        }
        Ok(true)
    }

    /// Re-inspect an app after a mutating action and append what changed
    /// (capped at `tree.report_changes_max_lines`): a diff, the new screen,
    /// or that the model is back on a screen it has seen.
    fn append_changes(&mut self, app_query: &str, mut out: ToolOutput) -> ToolOutput {
        if out.is_error {
            return out;
        }
        let Ok(app) = self.resolve_app(app_query) else {
            return out;
        };
        // Settling already read the app back after the action (without
        // reading its text off the screen again).
        let ocr = self.states.get(&app.pid).is_some_and(|s| s.ocr_lines > 0);
        let fresh = self.settled != Some(self.epoch) || ocr;
        let Ok(window) = self.resolve_window(&app, None, fresh) else {
            return out;
        };
        if self.observe(&app, &window, fresh).is_err() {
            return out;
        }
        let Ok(r) = self.render(app.pid, false) else {
            return out;
        };
        if r.seen == Seen::Same && !r.full && r.changes == 0 {
            return out;
        }
        let title = match r.seen {
            Seen::Same if r.large_change => {
                "State after the action (large change, full tree):".to_string()
            }
            Seen::Same => "State after the action:".to_string(),
            Seen::New => format!(
                "State after the action: now on screen #{} (new), window \"{}\":",
                r.screen, window.title
            ),
            Seen::Revisit => format!(
                "State after the action: back on screen #{} (seen before), window \"{}\":",
                r.screen, window.title
            ),
        };
        let max = self.store.config.tree.report_changes_max_lines.max(1);
        let lines: Vec<&str> = r.text.lines().collect();
        out.text.push_str("\n\n");
        out.text.push_str(&title);
        out.text.push('\n');
        if lines.len() > max {
            out.text.push_str(&lines[..max].join("\n"));
            out.text.push_str(&format!(
                "\n[+{} more lines; call get_app_state for the rest]",
                lines.len() - max
            ));
            // The model hasn't seen all of it: the next get_app_state
            // reports against what it had seen before.
        } else {
            out.text.push_str(r.text.trim_end());
            self.commit(app.pid, &window);
        }
        out
    }

    // -- describers --------------------------------------------------------

    fn node_for_handle(&self, app: &AppInfo, handle: ElementHandle) -> Option<&Node> {
        self.states
            .get(&app.pid)
            .and_then(|s| s.nodes.iter().find(|n| n.handle == handle))
    }

    fn describe(&self, app: &AppInfo, handle: ElementHandle) -> String {
        self.node_for_handle(app, handle)
            .map(|n| n.label())
            .unwrap_or_else(|| "the element".into())
    }

    fn describe_anchor(&self, app: &AppInfo, anchor: &Anchor) -> String {
        match anchor {
            Anchor::Element(h) => self.describe(app, *h),
            Anchor::Point(_) => "the point".into(),
        }
    }
}

/// Elements read by OCR exist only as pictures: they can be clicked, not
/// set, selected or asked to do something.
fn ocr_can_only_be_clicked(handle: ElementHandle, tool: &str) -> Result<()> {
    if crate::ocr::is_ocr(handle) {
        return Err(Error::InvalidArgs(format!(
            "this element was read off the screen (OCR), so {tool} can't work on it; click it by element_index (and type after clicking) instead"
        )));
    }
    Ok(())
}

/// Appended when an action changed nothing that can be seen.
const NO_CHANGE_NOTE: &str = " Nothing on screen changed after it; check (get_app_state, screenshot=true) before repeating it.";

/// Tools that change something on screen.
const MUTATING_TOOLS: &[&str] = &[
    "click",
    "perform_secondary_action",
    "set_value",
    "select_text",
    "scroll",
    "drag",
    "press_key",
    "type_text",
    "window",
];

/// For a mutating tool, the app to re-inspect afterwards (change reporting).
fn mutating_app(call: &ToolCall) -> Option<String> {
    match call {
        ToolCall::Click(a) => Some(a.app.clone()),
        ToolCall::PerformSecondaryAction(a) => Some(a.app.clone()),
        ToolCall::SetValue(a) => Some(a.app.clone()),
        ToolCall::SelectText(a) => Some(a.app.clone()),
        ToolCall::Scroll(a) => Some(a.app.clone()),
        ToolCall::Drag(a) => Some(a.app.clone()),
        ToolCall::PressKey(a) => Some(a.app.clone()),
        ToolCall::TypeText(a) => Some(a.app.clone()),
        _ => None,
    }
}

/// Jaccard similarity between the shapes of `nodes` and `shapes`.
fn shape_similarity(nodes: &[Node], shapes: &HashSet<u64>) -> f64 {
    let mine: HashSet<u64> = nodes.iter().map(|n| n.shape).collect();
    if mine.is_empty() && shapes.is_empty() {
        return 1.0;
    }
    let common = mine.intersection(shapes).count();
    common as f64 / (mine.len() + shapes.len() - common).max(1) as f64
}

/// Lowercased name + value of a node, for text matching.
fn node_text(n: &Node) -> String {
    let mut s = n.name.clone().unwrap_or_default();
    if let Some(v) = &n.value {
        s.push(' ');
        s.push_str(v);
    }
    s.to_lowercase()
}

fn state_matches(n: &Node, want: crate::tools::ElementState) -> bool {
    use crate::tools::ElementState::*;
    match want {
        Present => true,
        Visible => !n.states.hidden,
        Enabled => n.states.enabled,
        Focused => n.states.focused,
        Checked => n.states.checked == Some(true),
    }
}

fn describe_matcher(args: &WaitForArgs) -> String {
    let mut parts = Vec::new();
    if let Some(r) = &args.role {
        parts.push(format!("role={r}"));
    }
    if let Some(n) = &args.name {
        parts.push(format!("name~\"{n}\""));
    }
    if let Some(t) = &args.text {
        parts.push(format!("text~\"{t}\""));
    }
    if parts.is_empty() {
        "any element".into()
    } else {
        parts.join(", ")
    }
}

fn show_rect(r: Rect) -> String {
    format!("{:.0}x{:.0} at {:.0},{:.0}", r.width, r.height, r.x, r.y)
}

fn describe_window(w: &WindowInfo) -> String {
    let mut s = format!("\"{}\" (id {})", w.title, w.id);
    if let Some(b) = w.bounds {
        s.push_str(&format!(", {}", show_rect(b)));
    }
    let mut flags = Vec::new();
    if w.focused {
        flags.push("focused");
    }
    if w.minimized {
        flags.push("minimized");
    }
    if !flags.is_empty() {
        s.push_str(&format!(" [{}]", flags.join(", ")));
    }
    s
}

fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn window_list(windows: &[WindowInfo]) -> String {
    windows
        .iter()
        .map(|w| format!("\"{}\" (id: {})", w.title, w.id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve an app from a name / id / pid query against a snapshot of apps.
fn resolve_app_in(apps: &[AppInfo], query: &str) -> Result<AppInfo> {
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::InvalidArgs("`app` must not be empty".into()));
    }
    if let Ok(pid) = q.parse::<u32>()
        && let Some(a) = apps.iter().find(|a| a.pid == pid)
    {
        return Ok(a.clone());
    }
    let ql = q.to_lowercase();
    // Exact id or name.
    let exact: Vec<&AppInfo> = apps
        .iter()
        .filter(|a| a.id.eq_ignore_ascii_case(q) || a.name.eq_ignore_ascii_case(q))
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    if exact.len() > 1 {
        // Same name twice: prefer frontmost.
        if let Some(a) = exact.iter().find(|a| a.frontmost) {
            return Ok((*a).clone());
        }
        return Ok(exact[0].clone());
    }
    // Key match (exe stem, bundle id, etc.).
    let key: Vec<&AppInfo> = apps
        .iter()
        .filter(|a| a.match_keys().contains(&ql))
        .collect();
    if key.len() == 1 {
        return Ok(key[0].clone());
    }
    // Substring on name/id.
    let sub: Vec<&AppInfo> = apps
        .iter()
        .filter(|a| a.name.to_lowercase().contains(&ql) || a.id.to_lowercase().contains(&ql))
        .collect();
    match sub.as_slice() {
        [a] => Ok((*a).clone()),
        [] => Err(Error::AppNotFound(query.into())),
        many => {
            if let Some(a) = many.iter().find(|a| a.frontmost) {
                return Ok((*a).clone());
            }
            Err(Error::AmbiguousApp {
                query: query.into(),
                candidates: many
                    .iter()
                    .map(|a| format!("{} (pid {})", a.name, a.pid))
                    .collect::<Vec<_>>()
                    .join(", "),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApprovalMode, Config};
    use crate::mock::{Event, MockBackend};

    fn engine() -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e = Engine::new(backend, ConfigStore::in_memory(cfg));
        // Deterministic, instant time.
        e = e.with_time(Instant::now, |_| {});
        e
    }

    fn allow() -> AllowApprover {
        AllowApprover
    }

    #[test]
    fn list_apps_and_state() {
        let mut e = engine();
        let out = e.call(ToolCall::ListApps, &mut allow()).unwrap();
        assert!(out.text.contains("TextEdit"));

        let out = e
            .call(
                ToolCall::GetAppState(GetAppStateArgs {
                    app: "TextEdit".into(),
                    ..Default::default()
                }),
                &mut allow(),
            )
            .unwrap();
        assert!(out.image.is_some());
        assert!(out.text.contains("button \"Bold\""));
        assert!(out.text.contains("text area \"Document\""));
        // Bold has only press (hidden); Style keeps show_menu as a listed action.
        assert!(out.text.contains("actions=[show_menu]"));
        assert!(!out.text.contains("actions=[press"));
    }

    #[test]
    fn click_uses_accessibility_then_falls_back_to_coords() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        // Find Bold's index.
        let bold = e
            .state(4242)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.name.as_deref() == Some("Bold"))
            .unwrap()
            .index;
        e.call(
            ToolCall::Click(ClickArgs {
                app: "TextEdit".into(),
                element_index: Some(bold),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Action(3, a) if a == "AXPress"
        ));

        // Right click -> coordinate path.
        e.call(
            ToolCall::Click(ClickArgs {
                app: "TextEdit".into(),
                element_index: Some(bold),
                button: MouseButton::Right,
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, _, MouseButton::Right, 1)
        ));
    }

    #[test]
    fn coordinate_click_maps_through_screenshot() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        e.call(
            ToolCall::Click(ClickArgs {
                app: "TextEdit".into(),
                x: Some(400.0),
                y: Some(300.0),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        // Window is 800x600; screenshot fits to 800x600 (<=1280) so 1:1.
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, p, _, _) if (p.x-400.0).abs()<1.0 && (p.y-300.0).abs()<1.0
        ));
    }

    #[test]
    fn set_value_and_type_and_select() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        let doc = e
            .state(4242)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.role == "text area")
            .unwrap()
            .index;
        e.call(
            ToolCall::SetValue(SetValueArgs {
                app: "TextEdit".into(),
                element_index: doc,
                value: "World".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::SetValue(5, v) if v == "World"
        ));
        e.call(
            ToolCall::SelectText(SelectTextArgs {
                app: "TextEdit".into(),
                element_index: doc,
                text: Some("or".into()),
                occurrence: 1,
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::SelectText(5, Some(t), 1) if t == "or"
        ));
    }

    #[test]
    fn press_key_sequences() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        e.call(
            ToolCall::PressKey(PressKeyArgs {
                app: "TextEdit".into(),
                key: "cmd+a Delete".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        let keys: Vec<_> = e
            .backend()
            .events
            .iter()
            .filter_map(|ev| match ev {
                Event::Key(_, k) => Some(k.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(keys, vec!["meta+a", "Delete"]);
    }

    #[test]
    fn unknown_index_is_a_tool_error() {
        let mut e = engine();
        e.call(
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }),
            &mut allow(),
        )
        .unwrap();
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": 999}),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(out.text.contains("Element indices are only valid"));
    }

    #[test]
    fn blocked_app_is_refused() {
        let mut backend = MockBackend::new();
        let mut term = MockBackend::text_editor(10);
        term.info.name = "iTerm2".into();
        term.info.id = "com.googlecode.iterm2".into();
        backend.add_app(term);
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "iTerm2"}),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(out.text.contains("terminal"));
    }

    #[test]
    fn approval_denied_blocks() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(7));
        let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
            .with_time(Instant::now, |_| {});
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "TextEdit"}),
            &mut DenyApprover,
        );
        assert!(out.is_error);
        assert!(out.text.contains("denied"));
    }

    #[test]
    fn launch_makes_app_available() {
        let mut backend = MockBackend::new();
        backend.add_launchable("notes", {
            let mut a = MockBackend::text_editor(55);
            a.info.name = "Notes".into();
            a.info.id = "com.apple.Notes".into();
            a.info.exe = Some("/System/Applications/Notes.app".into());
            a
        });
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let out = e
            .call(
                ToolCall::LaunchApp(LaunchAppArgs {
                    app: "Notes".into(),
                }),
                &mut allow(),
            )
            .unwrap();
        assert!(out.text.contains("Launched Notes"));
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "Notes"}),
            &mut allow(),
        );
        assert!(!out.is_error);
    }

    #[test]
    fn cannot_launch_a_terminal() {
        let mut e = engine();
        let out = e.call_tool(
            "launch_app",
            serde_json::json!({"app": "xterm"}),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(out.text.contains("terminal"));
    }

    #[test]
    fn resolve_by_pid_and_ambiguity() {
        let a = AppInfo {
            name: "Safari".into(),
            id: "com.apple.Safari".into(),
            pid: 1,
            exe: None,
            frontmost: false,
            hidden: false,
        };
        let b = AppInfo {
            name: "Safari Technology Preview".into(),
            id: "com.apple.SafariTechnologyPreview".into(),
            pid: 2,
            exe: None,
            frontmost: true,
            hidden: false,
        };
        let apps = vec![a.clone(), b.clone()];
        assert_eq!(resolve_app_in(&apps, "1").unwrap().pid, 1);
        assert_eq!(resolve_app_in(&apps, "com.apple.Safari").unwrap().pid, 1);
        // Exact name beats the frontmost tiebreak.
        assert_eq!(resolve_app_in(&apps, "Safari").unwrap().pid, 1);
        // "safari" (lowercased) is only a substring of both -> frontmost wins.
        assert_eq!(resolve_app_in(&apps, "afari").unwrap().pid, 2);
        assert!(resolve_app_in(&apps, "Firefox").is_err());
    }

    #[test]
    fn find_element_filters() {
        let mut e = engine();
        let out = e
            .call_tool(
                "find_element",
                serde_json::json!({"app": "TextEdit", "role": "button"}),
                &mut allow(),
            )
            .text;
        assert!(out.contains("Bold"), "{out}");
        assert!(!out.contains("text area"), "{out}");

        let out = e.call_tool(
            "find_element",
            serde_json::json!({"app": "TextEdit", "editable": true}),
            &mut allow(),
        );
        assert!(out.text.contains("text area"), "{}", out.text);
    }

    #[test]
    fn wait_for_finds_immediately() {
        let mut e = engine();
        let out = e.call_tool(
            "wait_for",
            serde_json::json!({"app": "TextEdit", "name": "Bold", "timeout_ms": 500}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Found after waiting"));

        let out = e.call_tool(
            "wait_for",
            serde_json::json!({"app": "TextEdit", "name": "Nonexistent", "timeout_ms": 60, "poll_ms": 20}),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(out.text.contains("timed out"));
    }

    #[test]
    fn clipboard_round_trips() {
        let mut e = engine();
        let out = e.call_tool(
            "set_clipboard",
            serde_json::json!({"text": "hello clip"}),
            &mut allow(),
        );
        assert!(!out.is_error);
        let out = e.call_tool("get_clipboard", serde_json::json!({}), &mut allow());
        assert!(out.text.contains("hello clip"), "{}", out.text);
    }

    #[test]
    fn create_folder_makes_nested_folders_and_is_idempotent() {
        let mut e = engine();
        let dir = std::env::temp_dir().join(format!("cu-folder-{}", std::process::id()));
        let nested = dir.join("a").join("b");
        let args = serde_json::json!({"path": nested.to_string_lossy()});
        let out = e.call_tool("create_folder", args.clone(), &mut allow());
        assert!(!out.is_error, "{}", out.text);
        assert!(nested.is_dir());
        let out = e.call_tool("create_folder", args, &mut allow());
        assert!(
            !out.is_error && out.text.contains("already exists"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "create_folder",
            serde_json::json!({"path": "relative/dir"}),
            &mut allow(),
        );
        assert!(out.is_error);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn screenshot_full_and_region() {
        let mut e = engine();
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "full"}),
            &mut allow(),
        );
        assert!(out.image.is_some());
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "region", "x": 0, "y": 0, "width": 100, "height": 50}),
            &mut allow(),
        );
        assert!(out.image.is_some());
        // Region without full coords is an error.
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "region", "x": 0}),
            &mut allow(),
        );
        assert!(out.is_error);
    }

    #[test]
    fn screenshot_window_annotated() {
        let mut e = engine();
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "window", "app": "TextEdit", "annotate": true}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.image.is_some());
    }

    #[test]
    fn batch_runs_steps_and_stops_on_error() {
        let mut e = engine();
        let out = e.call_tool(
            "batch",
            serde_json::json!({
                "app": "TextEdit",
                "steps": [
                    {"tool": "get_app_state", "arguments": {}},
                    {"tool": "set_value", "arguments": {"element_index": 4, "value": "hi"}}
                ]
            }),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("1. get_app_state"));
        assert!(out.text.contains("2. set_value"));

        // A failing step stops the batch.
        let out = e.call_tool(
            "batch",
            serde_json::json!({
                "app": "TextEdit",
                "steps": [
                    {"tool": "click", "arguments": {"element_index": 999}},
                    {"tool": "get_app_state", "arguments": {}}
                ]
            }),
            &mut allow(),
        );
        assert!(out.is_error);
        assert!(
            !out.text.contains("2. get_app_state"),
            "should stop early: {}",
            out.text
        );
    }

    #[test]
    fn guard_blocks_and_confirms_sensitive_press() {
        // A backend with a "Send" button that has a press action.
        let mut backend = MockBackend::new();
        let mut app = MockBackend::text_editor(50);
        app.elements.push(
            crate::mock::MockElement::new(9, "button", "Send", Rect::new(0.0, 0.0, 40.0, 20.0))
                .child_of(1)
                .with_actions(&["AXPress"]),
        );
        backend.add_app(app);
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "TextEdit"}),
            &mut allow(),
        );
        let send = e
            .state(50)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.name.as_deref() == Some("Send"))
            .unwrap()
            .index;
        // DenyApprover.confirm_action() returns false -> blocked.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": send}),
            &mut DenyApprover,
        );
        assert!(out.is_error, "guard should block: {}", out.text);
        // AllowApprover confirms -> allowed.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": send}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
    }

    #[test]
    fn change_report_appended_after_action() {
        let mut e = engine();
        e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "TextEdit"}),
            &mut allow(),
        );
        let doc = e
            .state(4242)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.role == "text area")
            .unwrap()
            .index;
        let out = e.call_tool(
            "set_value",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "value": "changed!"}),
            &mut allow(),
        );
        assert!(out.text.contains("State after the action"), "{}", out.text);
        assert!(out.text.contains("changed!"), "{}", out.text);
    }
    // -- screen memory & caches ------------------------------------------

    use crate::mock::{MockApp, MockElement, MockWindow};

    fn button(h: u64, name: &str, parent: u64, x: f64) -> MockElement {
        MockElement::new(h, "button", name, Rect::new(x, 8.0, 50.0, 24.0))
            .child_of(parent)
            .with_actions(&["AXPress"])
    }

    /// Page A: the editor plus a "Next" button.
    fn page_a(pid: u32) -> MockApp {
        let mut a = MockBackend::text_editor(pid);
        a.elements.push(button(20, "Next", 2, 200.0));
        a
    }

    /// Page B: same toolbar, a list instead of the document, and "Back".
    fn page_b(pid: u32) -> MockApp {
        let mut a = MockBackend::text_editor(pid);
        a.elements.retain(|e| e.handle != 5);
        a.elements.push(button(30, "Back", 2, 200.0));
        a.elements.push(
            MockElement::new(31, "list", "Results", Rect::new(0.0, 40.0, 800.0, 500.0)).child_of(1),
        );
        for i in 0..4u64 {
            a.elements.push(
                MockElement::new(
                    32 + i,
                    "list item",
                    &format!("Result {i}"),
                    Rect::new(0.0, 40.0 + 30.0 * i as f64, 800.0, 30.0),
                )
                .child_of(31)
                .with_actions(&["AXPress"]),
            );
        }
        a
    }

    fn nav_engine(report: bool) -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(page_a(7));
        backend.on_press.insert(20, page_b(7));
        backend.on_press.insert(30, page_a(7));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        cfg.tree.report_changes = report;
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
    }

    fn index_named(e: &Engine<MockBackend>, pid: u32, name: &str) -> u32 {
        e.state(pid)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no {name}"))
            .index
    }

    fn state_of(e: &mut Engine<MockBackend>, extra: serde_json::Value) -> ToolOutput {
        let mut args = serde_json::json!({"app": "TextEdit"});
        if let (Some(a), Some(x)) = (args.as_object_mut(), extra.as_object()) {
            a.extend(x.clone());
        }
        let out = e.call_tool("get_app_state", args, &mut allow());
        assert!(!out.is_error, "{}", out.text);
        out
    }

    fn press(e: &mut Engine<MockBackend>, index: u32) -> ToolOutput {
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": index}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        out
    }

    fn press_named(e: &mut Engine<MockBackend>, pid: u32, name: &str) -> ToolOutput {
        let i = index_named(e, pid, name);
        press(e, i)
    }

    #[test]
    fn returning_to_a_seen_screen_skips_tree_and_screenshot() {
        let mut e = nav_engine(false);
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("screen #1 (new)"), "{}", out.text);
        assert!(out.image.is_some());
        let (doc, next) = (index_named(&e, 7, "Document"), index_named(&e, 7, "Next"));

        press(&mut e, next);
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("screen #2 (new)"), "{}", out.text);
        assert!(out.text.contains("Result 0"), "{}", out.text);
        assert!(out.image.is_some(), "a new screen gets a picture");
        let back = index_named(&e, 7, "Back");
        assert!(back > next, "numbers are never reused: {back} vs {next}");

        press(&mut e, back);
        let captures = e.backend().captures;
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("screen #1 (seen before)"), "{}", out.text);
        assert!(
            out.text
                .contains("Identical to when you last saw screen #1"),
            "{}",
            out.text
        );
        assert!(
            !out.text.contains("text area"),
            "no tree re-sent: {}",
            out.text
        );
        assert!(out.image.is_none(), "no screenshot re-sent");
        assert_eq!(e.backend().captures, captures, "not even captured");
        // The indices the model analysed are back.
        assert_eq!(index_named(&e, 7, "Document"), doc);
        assert_eq!(index_named(&e, 7, "Next"), next);
        // An index from page B is refused rather than hitting something else.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": back}),
            &mut allow(),
        );
        assert!(out.is_error, "{}", out.text);

        // Coordinates from page A's screenshot still map (1:1 here).
        e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "x": 100, "y": 100}),
            &mut allow(),
        );
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(7, p, _, _) if (p.x - 100.0).abs() < 1.0 && (p.y - 100.0).abs() < 1.0
        ));
    }

    #[test]
    fn returning_screen_reports_only_what_changed_since() {
        let mut e = nav_engine(false);
        state_of(&mut e, serde_json::json!({}));
        let next = index_named(&e, 7, "Next");
        let doc = index_named(&e, 7, "Document");
        press(&mut e, next);
        state_of(&mut e, serde_json::json!({}));
        // Going back finds the document edited meanwhile.
        let mut edited = page_a(7);
        edited
            .elements
            .iter_mut()
            .find(|el| el.handle == 5)
            .unwrap()
            .value = Some("Edited".into());
        e.backend_mut().on_press.insert(30, edited);
        press_named(&mut e, 7, "Back");
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(
            out.text.contains("Changes since you last saw screen #1"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains(&format!("~ {doc} text area")),
            "{}",
            out.text
        );
        assert!(!out.text.contains("button \"Bold\""), "{}", out.text);
    }

    #[test]
    fn change_report_announces_new_and_returning_screens() {
        let mut e = nav_engine(true);
        state_of(&mut e, serde_json::json!({}));
        let out = press_named(&mut e, 7, "Next");
        assert!(out.text.contains("now on screen #2 (new)"), "{}", out.text);
        assert!(out.text.contains("Result 3"), "{}", out.text);
        let out = press_named(&mut e, 7, "Back");
        assert!(
            out.text.contains("back on screen #1 (seen before)"),
            "{}",
            out.text
        );
        assert!(
            out.text
                .contains("Identical to when you last saw screen #1")
        );
        // The report was the model's view of it: nothing new to say now.
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("No changes"), "{}", out.text);
        assert!(out.image.is_none());
    }

    #[test]
    fn dialog_is_followed_and_main_window_recognised_after() {
        let pid = 9;
        let main = || {
            let mut a = MockBackend::text_editor(pid);
            a.elements.push(button(20, "Delete", 2, 200.0));
            a
        };
        let with_dialog = {
            let mut a = main();
            a.windows[0].focused = false;
            a.windows.push(MockWindow {
                id: 2,
                title: "Confirm".into(),
                bounds: Rect::new(200.0, 200.0, 300.0, 120.0),
                root: 40,
                focused: true,
            });
            a.elements.push(MockElement::new(
                40,
                "dialog",
                "Confirm",
                Rect::new(200.0, 200.0, 300.0, 120.0),
            ));
            a.elements.push(
                MockElement::new(
                    42,
                    "text",
                    "Delete the document?",
                    Rect::new(210.0, 210.0, 280.0, 20.0),
                )
                .child_of(40),
            );
            a.elements.push(button(41, "OK", 40, 400.0));
            a.elements.last_mut().unwrap().bounds = Rect::new(400.0, 280.0, 60.0, 24.0);
            a
        };
        let mut backend = MockBackend::new();
        backend.add_app(main());
        backend.on_press.insert(20, with_dialog);
        backend.on_press.insert(41, main());
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});

        state_of(&mut e, serde_json::json!({}));
        let delete = index_named(&e, pid, "Delete");
        let out = press(&mut e, delete);
        assert!(
            out.text
                .contains("now on screen #2 (new), window \"Confirm\""),
            "{}",
            out.text
        );
        let ok = index_named(&e, pid, "OK");
        assert!(ok > delete, "dialog numbers don't collide: {ok}");
        let out = press(&mut e, ok);
        assert!(
            out.text
                .contains("back on screen #1 (seen before), window \"Untitled\""),
            "{}",
            out.text
        );
        assert_eq!(index_named(&e, pid, "Delete"), delete);
    }

    #[test]
    fn unchanged_screenshots_are_not_resent() {
        // A custom-drawn app: almost nothing in the tree, so every view wants
        // pixels — but identical pixels are not sent twice.
        let mut backend = MockBackend::new();
        let mut canvas = MockBackend::text_editor(3);
        canvas.elements.retain(|el| el.handle == 1);
        backend.add_app(canvas);
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        cfg.ocr.mode = crate::config::OcrMode::Off;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        assert!(state_of(&mut e, serde_json::json!({})).image.is_some());
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_none(), "{}", out.text);
        assert!(
            out.text.contains("unchanged since you last saw"),
            "{}",
            out.text
        );
        // Explicitly asked for: always sent.
        assert!(
            state_of(&mut e, serde_json::json!({"screenshot": true}))
                .image
                .is_some()
        );
        // The picture changed: sent again.
        e.backend_mut().fill = 10;
        assert!(state_of(&mut e, serde_json::json!({})).image.is_some());
        // Dedupe off: sent every time.
        let mut cfg = e.store().config.clone();
        cfg.cache.dedupe_screenshots = false;
        e.set_config(ConfigStore::in_memory(cfg));
        assert!(state_of(&mut e, serde_json::json!({})).image.is_some());
    }

    #[test]
    fn recent_snapshots_are_reused_until_an_action() {
        // A clock that stands still: reuse depends on actions, not on how
        // long the (debug-mode) screenshot encoding took.
        let now = Instant::now();
        let mut e = nav_engine(false).with_time(move || now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        let (snaps, lists) = (e.backend().snapshots, e.backend().window_lists);
        e.call_tool(
            "find_element",
            serde_json::json!({"app": "TextEdit", "name": "Next"}),
            &mut allow(),
        );
        state_of(&mut e, serde_json::json!({}));
        assert_eq!(e.backend().snapshots, snaps, "tree read reused");
        assert_eq!(e.backend().window_lists, lists, "window list reused");
        // An action makes them stale: settling reads the app until two reads
        // agree, and the next get_app_state reuses the last of them.
        press_named(&mut e, 7, "Bold");
        state_of(&mut e, serde_json::json!({}));
        assert_eq!(e.backend().snapshots, snaps + 2);
        // Turned off: every call reads again.
        let mut cfg = e.store().config.clone();
        cfg.cache.snapshot_ttl_ms = 0;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        state_of(&mut e, serde_json::json!({}));
        assert_eq!(e.backend().snapshots, snaps + 4);
    }

    #[test]
    fn screen_memory_can_be_disabled() {
        let mut e = nav_engine(false);
        let mut cfg = e.store().config.clone();
        cfg.cache.enabled = false;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        press_named(&mut e, 7, "Next");
        state_of(&mut e, serde_json::json!({}));
        press_named(&mut e, 7, "Back");
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("(new)"), "{}", out.text);
        assert!(out.text.contains("text area"), "{}", out.text);
        assert_eq!(e.screen_memory_stats().0, 0);
    }

    #[test]
    fn find_element_keeps_indices_the_model_knows() {
        let mut e = nav_engine(false);
        state_of(&mut e, serde_json::json!({}));
        let next = index_named(&e, 7, "Next");
        let out = e.call_tool(
            "find_element",
            serde_json::json!({"app": "TextEdit", "name": "Next"}),
            &mut allow(),
        );
        assert!(
            out.text.contains(&format!("{next} button \"Next\"")),
            "{}",
            out.text
        );
    }

    #[test]
    fn batch_only_counts_the_screenshot_that_is_returned() {
        // Step 1's window screenshot is replaced by step 2's full-screen one,
        // so the model never got a picture of the window: the next view sends it.
        let mut e = nav_engine(false);
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                {"tool": "get_app_state", "arguments": {}},
                {"tool": "screenshot", "arguments": {"mode": "full"}}
            ]}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.image.is_some());
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_some(), "{}", out.text);

        // A batch whose last image is the window's: it counts.
        let mut e = nav_engine(false);
        e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                {"tool": "get_app_state", "arguments": {}}
            ]}),
            &mut allow(),
        );
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_none(), "{}", out.text);
    }

    // -- overlay hooks (a stand-in helper records what it is told) ---------

    #[cfg(unix)]
    fn recording_helper(log: &std::path::Path, answer: Option<bool>) -> Launcher {
        let reply = match answer {
            Some(ok) => format!(
                r#"echo '{{"t":"ready","excluded":true,"available":true}}'; while IFS= read -r l; do echo "$l" >> '{log}'; case "$l" in *'"ask":true'*) id=$(echo "$l" | sed 's/.*"id":\([0-9]*\).*/\1/'); echo "{{\"t\":\"answer\",\"id\":$id,\"ok\":{ok}}}";; esac; done"#,
                log = log.display()
            ),
            None => format!("cat > '{}'", log.display()),
        };
        Launcher {
            program: "sh".into(),
            args: vec!["-c".into(), reply],
        }
    }

    #[cfg(unix)]
    fn read_log(path: &std::path::Path) -> String {
        for _ in 0..100 {
            if let Ok(t) = std::fs::read_to_string(path)
                && t.contains("\"t\":\"end\"")
            {
                return t;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[cfg(unix)]
    #[test]
    fn overlay_follows_the_work() {
        let dir = std::env::temp_dir().join(format!("cu-ov-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("follow.log");
        let mut e = nav_engine(false).with_overlay(recording_helper(&log, None));
        state_of(&mut e, serde_json::json!({}));
        press_named(&mut e, 7, "Next");
        drop(e); // closes the helper's stdin
        let t = read_log(&log);
        for want in [
            r#""t":"config""#,
            r#""t":"begin""#,
            r#""t":"target","rect":[0.0,0.0,800.0,600.0]"#,
            r#""t":"pointer""#,
            r#""click":true"#,
            r#""t":"end","ok":true"#,
        ] {
            assert!(t.contains(want), "missing {want} in:\n{t}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn sensitive_click_waits_for_the_screen_when_no_client_can_ask() {
        let dir = std::env::temp_dir().join(format!("cu-ov2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (answer, allowed) in [(true, true), (false, false)] {
            let log = dir.join(format!("ask-{answer}.log"));
            let mut backend = MockBackend::new();
            let mut app = MockBackend::text_editor(60);
            app.elements.push(button(9, "Send", 2, 200.0));
            backend.add_app(app);
            let mut cfg = Config::default();
            cfg.approvals.mode = ApprovalMode::AllowAll;
            let mut e = Engine::new(backend, ConfigStore::in_memory(cfg))
                .with_time(Instant::now, |_| {})
                .with_overlay(recording_helper(&log, Some(answer)));
            state_of(&mut e, serde_json::json!({}));
            let send = index_named(&e, 60, "Send");
            // DenyApprover can't ask anyone: the screen is asked instead.
            let out = e.call_tool(
                "click",
                serde_json::json!({"app": "TextEdit", "element_index": send}),
                &mut DenyApprover,
            );
            assert_eq!(!out.is_error, allowed, "{}", out.text);
            drop(e);
            let t = read_log(&log);
            assert!(t.contains(r#""ask":true"#), "{t}");
            if allowed {
                assert!(
                    t.contains(r#""t":"danger","action":"press button \"Send\"""#),
                    "{t}"
                );
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_overlay_without_a_helper() {
        // Engines only show an overlay when the host provides the helper.
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        assert!(e.overlay.is_none());
    }

    // -- the user's controls ------------------------------------------------

    /// An engine on a fake clock that sleeping advances.
    fn timed_engine(
        backend: MockBackend,
        cfg: Config,
    ) -> (Engine<MockBackend>, Arc<std::sync::Mutex<Instant>>) {
        let now = Arc::new(std::sync::Mutex::new(Instant::now()));
        let (c, s) = (now.clone(), now.clone());
        let e = Engine::new(backend, ConfigStore::in_memory(cfg))
            .with_time(move || *c.lock().unwrap(), move |d| *s.lock().unwrap() += d);
        (e, now)
    }

    #[test]
    fn stop_refuses_every_call_until_the_user_lets_it_continue() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let stop = e.stop_handle();
        stop.store(true, Ordering::SeqCst);
        for (tool, args) in [
            ("list_apps", serde_json::json!({})),
            ("get_app_state", serde_json::json!({"app": "TextEdit"})),
            (
                "click",
                serde_json::json!({"app": "TextEdit", "element_index": 1}),
            ),
        ] {
            let out = e.call_tool(tool, args, &mut allow());
            assert!(out.is_error, "{tool}");
            // "Ctrl+Alt+Esc" (Ctrl+Option+Esc on a Mac).
            let key = crate::overlay::helper::pretty_key("ctrl+alt+escape");
            assert!(out.text.contains(&key), "{}", out.text);
        }
        // A batch stops at once, even with continue_on_error.
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "continue_on_error": true,
                "steps": [{"tool": "list_apps"}, {"tool": "list_apps"}]}),
            &mut allow(),
        );
        assert!(out.is_error);
        e.set_stopped(false);
        assert!(!e.is_stopped());
        assert!(
            !e.call_tool("list_apps", serde_json::json!({}), &mut allow())
                .is_error
        );
    }

    #[test]
    fn actions_wait_while_the_user_uses_the_computer() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        // The user is typing (input 0.1 s and 0.4 s ago), then stops.
        backend.idle_script = [100, 400].map(Duration::from_millis).into();
        backend.idle = Some(Duration::from_secs(5));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let (mut e, clock) = timed_engine(backend, cfg);
        state_of(&mut e, serde_json::json!({}));
        let t0 = *clock.lock().unwrap();
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "Return"}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        let waited = clock.lock().unwrap().saturating_duration_since(t0);
        assert!(waited >= Duration::from_millis(100), "{waited:?}");
        assert!(
            e.backend()
                .events
                .contains(&Event::Key(4242, "Return".into()))
        );

        // Reading is never held up.
        e.backend_mut().idle = Some(Duration::ZERO);
        assert!(
            !e.call_tool("list_apps", serde_json::json!({}), &mut allow())
                .is_error
        );
    }

    #[test]
    fn a_busy_user_makes_the_action_give_up_and_say_why() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        backend.idle = Some(Duration::from_millis(100));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        cfg.control.max_pause_secs = 3;
        let (mut e, _) = timed_engine(backend, cfg);
        state_of(&mut e, serde_json::json!({}));
        let before = e.backend().events.len();
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "Return"}),
            &mut allow(),
        );
        assert!(
            out.is_error && out.text.contains("using the mouse or keyboard"),
            "{}",
            out.text
        );
        assert_eq!(e.backend().events.len(), before, "nothing was done");

        // Switched off: no waiting at all.
        let mut cfg = e.store().config.clone();
        cfg.control.pause_on_user_input = false;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "Return"}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
    }

    #[test]
    fn own_input_is_not_taken_for_the_user() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let (mut e, clock) = timed_engine(backend, cfg);
        state_of(&mut e, serde_json::json!({}));
        let press = |e: &mut Engine<MockBackend>| {
            e.call_tool(
                "press_key",
                serde_json::json!({"app": "TextEdit", "key": "Tab"}),
                &mut allow(),
            )
        };
        assert!(!press(&mut e).is_error);
        // 1 s later the system saw input 1 s ago: the engine's own.
        *clock.lock().unwrap() += Duration::from_secs(1);
        e.backend_mut().idle = Some(Duration::from_secs(1));
        let t = *clock.lock().unwrap();
        assert!(!press(&mut e).is_error);
        let took = clock.lock().unwrap().saturating_duration_since(t);
        // Only the action's own settle/key delays, no pause.
        assert!(
            took < Duration::from_millis(200),
            "waited {took:?} for its own input"
        );
    }

    fn login_app(pid: u32) -> MockApp {
        let mut app = MockBackend::text_editor(pid);
        let mut pw = MockElement::new(
            20,
            "secure text field",
            "Password",
            Rect::new(100.0, 100.0, 200.0, 30.0),
        )
        .child_of(1)
        .editable();
        pw.value = Some("hunter2".into());
        let mut card = MockElement::new(
            21,
            "text field",
            "Payment",
            Rect::new(100.0, 200.0, 200.0, 30.0),
        )
        .child_of(1)
        .editable();
        card.value = Some("4111 1111 1111 1111".into());
        app.elements.extend([pw, card]);
        app
    }

    #[test]
    fn private_data_never_reaches_the_model() {
        let mut backend = MockBackend::new();
        backend.add_app(login_app(4242));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
        assert!(!out.text.contains("hunter2"), "{}", out.text);
        assert!(!out.text.contains("4111 1111 1111 1111"), "{}", out.text);
        assert!(out.text.contains("•••• •••• •••• 1111"), "{}", out.text);
        assert!(
            out.text.contains("2 private area(s) blacked out"),
            "{}",
            out.text
        );
        // The password field's pixels are grey in the image.
        let img = out.image.unwrap();
        let rgb = image::load_from_memory(&img.data).unwrap().to_rgb8();
        let at = |x: u32, y: u32| rgb.get_pixel(x, y).0;
        assert_eq!(at(150, 110), [128, 128, 128]);
        assert_eq!(at(700, 500), [200, 200, 200]);

        // The screenshot tool too.
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit"}),
            &mut allow(),
        );
        assert!(out.text.contains("blacked out"), "{}", out.text);

        // Settings can switch it off.
        let mut cfg = e.store().config.clone();
        cfg.privacy.redact_passwords = false;
        cfg.privacy.redact_card_numbers = false;
        cfg.privacy.redact_labels.clear();
        e.set_config(ConfigStore::in_memory(cfg));
        let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
        assert!(out.text.contains("4111 1111 1111 1111"), "{}", out.text);
    }

    #[cfg(unix)]
    #[test]
    fn the_stop_key_in_the_helper_stops_the_engine() {
        let dir = std::env::temp_dir().join(format!("cu-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("stop.log");
        // A stand-in helper whose user presses the stop key at once.
        let script = format!(
            r#"echo '{{"t":"ready","excluded":true,"available":true}}'; echo '{{"t":"stop","on":true}}'; cat > '{}'"#,
            log.display()
        );
        let mut e = engine().with_overlay(Launcher {
            program: "sh".into(),
            args: vec!["-c".into(), script],
        });
        e.arm();
        for _ in 0..100 {
            if e.is_stopped() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(e.is_stopped());
        let out = e.call_tool("list_apps", serde_json::json!({}), &mut allow());
        assert!(
            out.is_error && out.text.contains("stop key"),
            "{}",
            out.text
        );
        drop(e);
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- smart waiting and verification -------------------------------------

    fn index_of_name(out: &str, needle: &str) -> u32 {
        out.lines()
            .find(|l| l.contains(needle))
            .and_then(|l| l.split_whitespace().next())
            .and_then(|t| t.parse().ok())
            .unwrap_or_else(|| panic!("{needle} not in:\n{out}"))
    }

    #[test]
    fn waits_until_the_ui_stops_changing() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        // The document keeps updating for three more reads, then settles.
        let before = e.backend().snapshots;
        e.backend_mut().snapshot_script = ["Loading.", "Loading..", "Done"]
            .map(|v| (5, v.to_string()))
            .into();
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        // Read until two reads agreed (after "Done"), and the report shows it.
        assert!(
            e.backend().snapshots >= before + 4,
            "{}",
            e.backend().snapshots
        );
        assert!(out.text.contains("Done"), "{}", out.text);

        // Fixed mode: one read (for the report), no waiting for stability.
        let mut cfg = e.store().config.clone();
        cfg.timing.settle = crate::config::SettleMode::Fixed;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        let before = e.backend().snapshots;
        e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
            &mut allow(),
        );
        assert_eq!(e.backend().snapshots, before + 1);
    }

    #[test]
    fn a_press_that_fails_is_clicked_with_the_mouse() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        e.backend_mut().fail_actions.insert(3);
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("clicked it with the mouse instead"),
            "{}",
            out.text
        );
        assert!(e.backend().events.contains(&Event::Click(
            4242,
            Point::new(40.0, 20.0),
            MouseButton::Left,
            1
        )));

        // With retries off, the failure is reported instead.
        let mut cfg = e.store().config.clone();
        cfg.verify.retry = false;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
            &mut allow(),
        );
        assert!(out.is_error, "{}", out.text);
    }

    #[test]
    fn a_press_that_changes_nothing_is_reported_not_repeated() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
            &mut allow(),
        );
        assert!(
            out.text.contains("Nothing on screen changed"),
            "{}",
            out.text
        );
        let presses = |e: &Engine<MockBackend>| {
            e.backend()
                .events
                .iter()
                .filter(|ev| matches!(ev, Event::Action(3, _) | Event::Click(..)))
                .count()
        };
        assert_eq!(presses(&e), 1, "not repeated by default");

        // Opted in: tried once more with the mouse.
        let mut cfg = e.store().config.clone();
        cfg.verify.retry_on_no_change = true;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
            &mut allow(),
        );
        assert!(
            out.text.contains("clicked it with the mouse too"),
            "{}",
            out.text
        );
        assert_eq!(presses(&e), 3);
    }

    #[test]
    fn guarded_presses_are_never_repeated() {
        let mut backend = MockBackend::new();
        let mut app = MockBackend::text_editor(4242);
        app.elements.push(button(9, "Send", 2, 200.0));
        backend.add_app(app);
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        cfg.verify.retry_on_no_change = true;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let out = state_of(&mut e, serde_json::json!({}));
        let send = index_of_name(&out.text, "\"Send\"");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": send}),
            &mut allow(),
        );
        assert!(
            out.text.contains("Nothing on screen changed"),
            "{}",
            out.text
        );
        assert!(
            !e.backend()
                .events
                .iter()
                .any(|ev| matches!(ev, Event::Click(..)))
        );
    }

    #[test]
    fn a_value_that_does_not_take_is_typed_instead() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let doc = index_of_name(&out.text, "\"Document\"");
        e.backend_mut().ignore_set_value.insert(5);
        let out = e.call_tool(
            "set_value",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "value": "Replaced"}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("typed it instead"), "{}", out.text);
        let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
        assert!(out.text.contains("Replaced"), "{}", out.text);
        assert!(!out.text.contains("HelloReplaced"), "{}", out.text);
    }

    #[test]
    fn typing_that_does_not_land_clicks_the_field_and_types_again() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let doc = index_of_name(&out.text, "\"Document\"");
        // Focus claims success but does nothing: the text goes nowhere.
        e.backend_mut().fake_focus.insert(5);
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "text": " world"}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("didn't land"), "{}", out.text);
        let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
        assert!(out.text.contains("Hello world"), "{}", out.text);
    }

    // -- smart screenshots ----------------------------------------------------

    fn always_shot_engine() -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        cfg.screenshot.attach = AttachMode::Always;
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
    }

    #[test]
    fn a_follow_up_screenshot_sends_only_the_part_that_changed() {
        let mut e = always_shot_engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let full = out.image.expect("first view: the whole window");
        assert_eq!((full.width, full.height), (800, 600));

        // A small change (a tooltip, a menu): only that part, with its place.
        e.backend_mut().patch = Some((Rect::new(500.0, 400.0, 60.0, 30.0), 20));
        let out = state_of(&mut e, serde_json::json!({}));
        let part = out.image.expect("the changed part");
        assert!(
            out.text.contains("only the part that changed"),
            "{}",
            out.text
        );
        assert!(
            part.width < 400 && part.height < 400,
            "{}x{}",
            part.width,
            part.height
        );
        assert!(
            out.text.contains("x 4"),
            "offset in the earlier image: {}",
            out.text
        );
        // x/y still refer to the whole window.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "x": 700, "y": 550}),
            &mut allow(),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(e.backend().events.iter().any(|ev| matches!(
            ev,
            Event::Click(_, p, _, _) if (p.x - 700.0).abs() < 1.0 && (p.y - 550.0).abs() < 1.0
        )));

        // A big change: the whole window again.
        e.backend_mut().patch = Some((Rect::new(0.0, 0.0, 800.0, 500.0), 60));
        let out = state_of(&mut e, serde_json::json!({}));
        let img = out.image.expect("whole window");
        assert_eq!((img.width, img.height), (800, 600), "{}", out.text);

        // Asking explicitly, or scope = "full": the whole window.
        e.backend_mut().patch = Some((Rect::new(10.0, 10.0, 20.0, 20.0), 90));
        let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
        assert_eq!(out.image.unwrap().width, 800);
        let mut cfg = e.store().config.clone();
        cfg.screenshot.scope = crate::config::ShotScope::Full;
        e.set_config(ConfigStore::in_memory(cfg));
        e.backend_mut().patch = Some((Rect::new(10.0, 10.0, 20.0, 20.0), 140));
        let out = state_of(&mut e, serde_json::json!({}));
        assert_eq!(out.image.unwrap().width, 800);
    }

    #[test]
    fn the_screenshot_tool_sends_what_changed_and_zooms_into_elements() {
        let mut e = always_shot_engine();
        let shot = |e: &mut Engine<MockBackend>, args: serde_json::Value| {
            e.call_tool("screenshot", args, &mut allow())
        };
        let out = shot(&mut e, serde_json::json!({}));
        assert_eq!(out.image.as_ref().unwrap().width, 1280, "{}", out.text);
        // Nothing changed: said, not re-sent.
        let out = shot(&mut e, serde_json::json!({}));
        assert!(
            out.image.is_none() && out.text.contains("looks the same"),
            "{}",
            out.text
        );
        // A small change: that part only.
        e.backend_mut().patch = Some((Rect::new(1000.0, 700.0, 40.0, 40.0), 10));
        let out = shot(&mut e, serde_json::json!({}));
        let img = out.image.unwrap();
        assert!(img.width < 640, "{}", out.text);
        assert!(
            out.text.contains("changed since your last full-screen"),
            "{}",
            out.text
        );
        // mode=full: all of it.
        let out = shot(&mut e, serde_json::json!({"mode": "full"}));
        assert_eq!(out.image.unwrap().width, 1280);

        // Zoom into one element.
        let tree = state_of(&mut e, serde_json::json!({"disable_diff": true}));
        let style = index_of_name(&tree.text, "\"Style\"");
        let out = shot(
            &mut e,
            serde_json::json!({"app": "TextEdit", "element_index": style}),
        );
        let img = out.image.unwrap();
        assert!(out.text.contains("zoomed in"), "{}", out.text);
        // The 100x24 pop-up button plus a small margin.
        assert!(
            (100..=130).contains(&img.width) && img.height <= 50,
            "{}x{}",
            img.width,
            img.height
        );
    }

    // -- window management ----------------------------------------------------

    #[test]
    fn windows_can_be_arranged() {
        let mut e = engine();
        let win = |e: &mut Engine<MockBackend>, args: serde_json::Value| {
            e.call_tool("window", args, &mut allow())
        };
        let out = win(&mut e, serde_json::json!({"action": "displays"}));
        assert!(
            out.text.contains("display 0 (primary): 1280x800 at 0,0"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("display 1: 1920x1080 at 1280,0"),
            "{}",
            out.text
        );
        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "list"}),
        );
        assert!(
            out.text.contains("\"Untitled\" (id 1), 800x600 at 0,0"),
            "{}",
            out.text
        );

        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "move", "x": 100, "y": 50}),
        );
        assert!(
            out.text
                .contains("Now: \"Untitled\" (id 1), 800x600 at 100,50"),
            "{}",
            out.text
        );
        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "tile_right"}),
        );
        assert!(out.text.contains("640x760 at 640,0"), "{}", out.text);
        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "move_to_display", "display": 1}),
        );
        assert!(out.text.contains("at 1920,"), "{}", out.text);
        // The app's minimum size is reported.
        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "resize", "width": 50, "height": 50}),
        );
        assert!(out.text.contains("adjusted"), "{}", out.text);
        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "minimize"}),
        );
        assert!(out.text.contains("minimized]"), "{}", out.text);
        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "restore"}),
        );
        assert!(!out.text.contains("minimized]"), "{}", out.text);
        // Unsupported operations say so; bad arguments are errors.
        assert!(
            win(
                &mut e,
                serde_json::json!({"app": "TextEdit", "action": "move_to_desktop", "desktop": 2})
            )
            .is_error
        );
        assert!(
            win(
                &mut e,
                serde_json::json!({"app": "TextEdit", "action": "move"})
            )
            .is_error
        );
        assert!(win(&mut e, serde_json::json!({"action": "maximize"})).is_error);
        // A moved window needs a fresh screenshot for coordinates.
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_some(), "{}", out.text);
        let out = win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "close"}),
        );
        assert!(out.text.contains("It is closed"), "{}", out.text);
    }

    // -- OCR ------------------------------------------------------------------

    /// A custom-drawn app: a window with nothing but a canvas in its tree.
    fn canvas_engine(lines: Option<Vec<OcrLine>>) -> Engine<MockBackend> {
        let win = Rect::new(0.0, 0.0, 640.0, 480.0);
        let app = MockApp {
            info: AppInfo {
                name: "Game".into(),
                id: "game".into(),
                pid: 77,
                exe: None,
                frontmost: true,
                hidden: false,
            },
            windows: vec![MockWindow {
                id: 9,
                title: "Game".into(),
                bounds: win,
                root: 1,
                focused: true,
            }],
            elements: vec![
                MockElement::new(1, "window", "Game", win),
                MockElement::new(2, "canvas", "", win).child_of(1),
            ],
        };
        let mut backend = MockBackend::new();
        backend.add_app(app);
        backend.ocr_text = lines;
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        cfg.ocr.tesseract_path = "/nonexistent/tesseract".into();
        // Every read looks again (the tests change the picture).
        cfg.cache.snapshot_ttl_ms = 0;
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
    }

    fn line(text: &str, x: f64, y: f64) -> OcrLine {
        OcrLine {
            text: text.into(),
            bounds: Rect::new(x, y, 80.0, 20.0),
            confidence: 0.9,
        }
    }

    #[test]
    fn custom_drawn_apps_get_their_text_read_and_clickable() {
        let mut e = canvas_engine(Some(vec![
            line("New Game", 100.0, 100.0),
            line("Options", 100.0, 140.0),
            OcrLine {
                confidence: 0.1,
                ..line("~~noise~~", 300.0, 300.0)
            },
        ]));
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "Game"}),
            &mut allow(),
        );
        assert!(out.text.contains("ocr text \"New Game\""), "{}", out.text);
        assert!(
            out.text.contains("2 line(s) of text were read"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("noise"), "low confidence left out");
        assert!(out.image.is_some(), "sparse tree: a screenshot too");
        let options = index_of_name(&out.text, "\"Options\"");
        // Clicked at its place on screen.
        let r = e.call_tool(
            "click",
            serde_json::json!({"app": "Game", "element_index": options}),
            &mut allow(),
        );
        assert!(!r.is_error, "{}", r.text);
        assert!(e.backend().events.iter().any(|ev| matches!(
            ev,
            Event::Click(77, p, MouseButton::Left, 1) if *p == Point::new(140.0, 150.0)
        )));
        // It can't be set or selected.
        let r = e.call_tool(
            "set_value",
            serde_json::json!({"app": "Game", "element_index": options, "value": "x"}),
            &mut allow(),
        );
        assert!(r.is_error && r.text.contains("OCR"), "{}", r.text);
        // An unchanged picture isn't read again.
        let runs = e.backend().ocr_runs;
        e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "Game"}),
            &mut allow(),
        );
        assert_eq!(e.backend().ocr_runs, runs);
        e.backend_mut().fill = 90;
        e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "Game"}),
            &mut allow(),
        );
        assert_eq!(e.backend().ocr_runs, runs + 1);
    }

    #[test]
    fn apps_with_a_real_tree_are_not_read_unless_asked() {
        let mut e = engine();
        e.backend_mut().ocr_text = Some(vec![line("Bold", 10.0, 8.0), line("Extra", 300.0, 300.0)]);
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(!out.text.contains("ocr text"), "{}", out.text);
        assert_eq!(e.backend().ocr_runs, 0);
        // Asked: read, minus what the tree already says there.
        let out = state_of(
            &mut e,
            serde_json::json!({"ocr": true, "disable_diff": true}),
        );
        assert!(out.text.contains("ocr text \"Extra\""), "{}", out.text);
        assert!(!out.text.contains("ocr text \"Bold\""), "{}", out.text);
    }

    #[test]
    fn missing_ocr_is_explained_once() {
        let mut e = canvas_engine(None);
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "Game"}),
            &mut allow(),
        );
        assert!(
            out.text.contains("Text recognition unavailable"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "Game"}),
            &mut allow(),
        );
        assert!(
            !out.text.contains("Text recognition unavailable"),
            "{}",
            out.text
        );
    }

    // -- notifications --------------------------------------------------------

    #[test]
    fn notifications_are_read_only_when_enabled_and_private_bits_masked() {
        let mut e = engine();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let note = |app: &str, title: &str, body: &str, ago: u64| Notification {
            app: app.into(),
            title: title.into(),
            body: body.into(),
            time: Some(now - ago),
        };
        e.backend_mut().notes = vec![
            note("Slack", "Ada", "Lunch at 1?", 600),
            note("Bank", "Sign-in", "Your verification code is 482913", 30),
            note("1Password", "Vault", "unlocked", 20),
            note("Shop", "Receipt", "Card 4111 1111 1111 1111 charged", 5),
        ];
        // Off by default: the tool isn't even offered.
        assert!(
            !crate::tools::definitions_from(&e.store().config)
                .iter()
                .any(|d| d.name == "get_notifications")
        );
        let out = e.call_tool("get_notifications", serde_json::json!({}), &mut allow());
        assert!(
            out.is_error && out.text.contains("off in settings"),
            "{}",
            out.text
        );

        let mut cfg = e.store().config.clone();
        cfg.notifications.enabled = true;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = e.call_tool("get_notifications", serde_json::json!({}), &mut allow());
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("[10 min ago] Slack — Ada: Lunch at 1?"),
            "{}",
            out.text
        );
        assert!(out.text.contains("code is ••••••"), "{}", out.text);
        assert!(!out.text.contains("482913"), "{}", out.text);
        assert!(out.text.contains("•••• •••• •••• 1111"), "{}", out.text);
        // A password manager is a blocked category.
        assert!(!out.text.contains("Vault"), "{}", out.text);
        assert!(out.text.contains("1 from apps blocked"), "{}", out.text);
        let out = e.call_tool(
            "get_notifications",
            serde_json::json!({"app": "slack"}),
            &mut allow(),
        );
        assert!(out.text.contains("1 recent notification"), "{}", out.text);
        let out = e.call_tool(
            "get_notifications",
            serde_json::json!({"limit": 1}),
            &mut allow(),
        );
        assert!(
            out.text.contains("Receipt") && !out.text.contains("Ada"),
            "{}",
            out.text
        );
    }
}
