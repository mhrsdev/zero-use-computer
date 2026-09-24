//! The engine: platform-agnostic implementation of the tool contract on top
//! of a [`Backend`]. It resolves apps/windows/elements, enforces approvals,
//! renders app state (tree + screenshot), maintains per-app element indices
//! and diffs, and maps screenshot coordinates to the screen.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::backend::{Backend, Native};
use crate::config::{AttachMode, ConfigStore};
use crate::error::{Error, Result};
use crate::imaging::{self, CoordMap};
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
        if !cfg.enabled {
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
            match Overlay::spawn(&launcher, cfg) {
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
        if !cfg.enabled {
            self.overlay = None;
        } else if let Some(o) = &self.overlay {
            o.send(&OverlayCmd::Config {
                config: Box::new(cfg),
            });
        }
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
        match self.store.config.overlay.confirm_on_screen {
            ScreenConfirm::Never => false,
            ScreenConfirm::Always => true,
            ScreenConfirm::WhenNoClient => !approver.interactive(),
        }
    }

    /// Show that `action` waits for the user, and ask on screen when that is
    /// the way to ask. `Some(answer)` if the screen answered.
    fn overlay_approval(&mut self, action: &str, approver: &dyn Approver) -> Option<bool> {
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
        let raw = self.backend.snapshot(app, window, &opts)?;
        let snap_at = (self.clock)();
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

    // -- tool dispatch -----------------------------------------------------

    /// Run one tool call.
    pub fn call(&mut self, call: ToolCall, approver: &mut dyn Approver) -> Result<ToolOutput> {
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
        if acting.is_some() || matches!(call, ToolCall::LaunchApp(_)) {
            // Anything read before this action is stale now.
            self.epoch += 1;
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
        }?;
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
        self.observe(&app, &window, false)?;
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
        let fingerprint = cache.dedupe_screenshots;
        let (known_pixels, known_coord, known_shot) =
            (known.pixels.clone(), known.coord, known.shot);

        let mut image = None;
        if want {
            match self.capture_clean(|b| b.capture(&app, &window)) {
                Ok(cap) => {
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

        // The agent cursor goes there first, so the user sees what is next.
        match &anchor {
            Anchor::Element(h) => self.overlay_point_element(&app, *h, false),
            Anchor::Point(p) => self.overlay_point(*p, false),
        }
        // Confirm consequential presses (Send / Delete / Pay …) when guarded.
        if let Anchor::Element(h) = &anchor {
            let label = self.describe(&app, *h);
            self.guard_action(&label, approver)?;
        }
        match &anchor {
            Anchor::Element(h) => self.overlay_point_element(&app, *h, true),
            Anchor::Point(p) => self.overlay_point(*p, true),
        }

        // A single left click on an element with a press action goes through
        // the accessibility API so it works in the background.
        if let (Anchor::Element(h), MouseButton::Left, 1) = (&anchor, args.button, count) {
            let node = self.node_for_handle(&app, *h);
            if let Some(action) = node
                .and_then(|n| n.has_action("press"))
                .map(|a| a.native.clone())
            {
                self.backend.perform_action(*h, &action)?;
                self.settle();
                return Ok(ToolOutput::text(format!(
                    "Pressed {}.",
                    self.describe(&app, *h)
                )));
            }
        }

        let point = self.anchor_point(&app, &anchor)?;
        let target = self.input_target(&app);
        self.backend.click(&target, point, args.button, count)?;
        self.settle();
        let verb = match (args.button, count) {
            (MouseButton::Right, _) => "Right-clicked",
            (_, 2) => "Double-clicked",
            (_, 3) => "Triple-clicked",
            _ => "Clicked",
        };
        Ok(ToolOutput::text(format!(
            "{verb} {} at ({:.0}, {:.0}).",
            self.describe_anchor(&app, &anchor),
            point.x,
            point.y
        )))
    }

    fn perform_secondary(
        &mut self,
        args: SecondaryActionArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "perform_secondary_action", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
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
        self.overlay_point_element(&app, handle, false);
        self.guard_action(&node.label(), approver)?;
        self.overlay_point_element(&app, handle, true);
        self.backend.perform_action(handle, &native)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Performed `{}` on {}.",
            args.action,
            node.label()
        )))
    }

    fn set_value(&mut self, args: SetValueArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "set_value", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        self.overlay_point_element(&app, handle, true);
        self.backend.set_value(handle, &args.value)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Set {} to \"{}\".",
            node.label(),
            tree::truncate(&args.value, 80)
        )))
    }

    fn select_text(
        &mut self,
        args: SelectTextArgs,
        approver: &mut dyn Approver,
    ) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "select_text", approver)?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        self.overlay_point_element(&app, handle, false);
        self.backend
            .select_text(handle, args.text.as_deref(), args.occurrence.max(1))?;
        self.settle();
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
        match &anchor {
            Anchor::Element(h) => self.overlay_point_element(&app, *h, false),
            Anchor::Point(p) => self.overlay_point(*p, false),
        }

        if let Anchor::Element(h) = &anchor
            && let Native::Done(_) = self.backend.scroll_element(*h, args.direction, pages)?
        {
            self.settle();
            return Ok(ToolOutput::text(format!(
                "Scrolled {} {:?} by {pages} page(s).",
                self.describe(&app, *h),
                args.direction
            )));
        }

        let point = self.anchor_point(&app, &anchor)?;
        let (ux, uy) = args.direction.unit();
        // ~3 wheel lines per page.
        let lines = (pages * 3.0).round().max(1.0) as i32;
        let target = self.input_target(&app);
        self.backend
            .scroll_wheel(&target, point, ux * lines, uy * lines)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Scrolled {:?} by {pages} page(s) at ({:.0}, {:.0}).",
            args.direction, point.x, point.y
        )))
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
        self.overlay_point(p0, true);
        self.overlay_point(p1, false);
        let target = self.input_target(&app);
        self.backend.drag(&target, p0, p1)?;
        self.settle();
        Ok(ToolOutput::text(format!(
            "Dragged from ({:.0}, {:.0}) to ({:.0}, {:.0}).",
            p0.x, p0.y, p1.x, p1.y
        )))
    }

    fn press_key(&mut self, args: PressKeyArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "press_key", approver)?;
        let combos = keys::parse_sequence(&args.key)?;
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            self.overlay_point_element(&app, h, false);
            let _ = self.backend.focus(h);
        }
        let target = self.input_target(&app);
        for combo in &combos {
            self.backend.press_key(&target, combo)?;
            (self.sleep)(Duration::from_millis(self.store.config.timing.key_delay_ms));
        }
        self.settle();
        let shown: Vec<String> = combos.iter().map(|c| c.to_string()).collect();
        Ok(ToolOutput::text(format!("Pressed {}.", shown.join(" "))))
    }

    fn type_text(&mut self, args: TypeTextArgs, approver: &mut dyn Approver) -> Result<ToolOutput> {
        let app = self.authorize(&args.app, "type_text", approver)?;
        if args.text.is_empty() {
            return Err(Error::InvalidArgs("`text` must not be empty".into()));
        }
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            self.overlay_point_element(&app, h, true);
            let _ = self.backend.focus(h);
            self.settle();
        }
        let target = self.input_target(&app);
        // Split on newlines so each becomes a Return press (works everywhere).
        let mut first = true;
        for segment in args.text.split('\n') {
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
        self.settle();
        Ok(ToolOutput::text(format!(
            "Typed {} character(s).",
            args.text.chars().count()
        )))
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
            ScreenshotMode::Full
        });
        let (capture, marks, label) = match mode {
            ScreenshotMode::Full => (
                self.capture_clean(|b| b.capture_screen(None))?,
                None,
                "full screen".into(),
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
                let cap = self.capture_clean(|b| b.capture(&app, &window))?;
                let marks = if args.annotate {
                    self.observe(&app, &window, false)?;
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
                (
                    cap,
                    marks,
                    format!("{} window \"{}\"", app.name, window.title),
                )
            }
        };

        let mut capture = capture;
        if let Some(marks) = marks {
            imaging::annotate(&mut capture, &marks);
        }
        let (img, _map) = imaging::encode(capture, &self.store.config.screenshot)?;
        Ok(ToolOutput {
            text: format!("Screenshot of {label}: {}x{} px.", img.width, img.height),
            image: Some(img),
            is_error: false,
        })
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
                    if !args.continue_on_error {
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

    /// The action guard: confirm/refuse a consequential press on `label`.
    /// While such an action waits for the user the overlay turns to the
    /// approval colour; once it runs, to the sensitive-action colour.
    fn guard_action(&mut self, label: &str, approver: &mut dyn Approver) -> Result<()> {
        use crate::config::SensitiveMode;
        let guard = &self.store.config.guard;
        let low = label.to_lowercase();
        if !guard
            .keywords
            .iter()
            .any(|k| low.contains(&k.to_lowercase()))
        {
            return Ok(());
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
        Ok(())
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
        let Ok(window) = self.resolve_window(&app, None, true) else {
            return out;
        };
        if self.observe(&app, &window, true).is_err() {
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
        // An action makes them stale.
        press_named(&mut e, 7, "Bold");
        state_of(&mut e, serde_json::json!({}));
        assert_eq!(e.backend().snapshots, snaps + 1);
        // Turned off: every call reads again.
        let mut cfg = e.store().config.clone();
        cfg.cache.snapshot_ttl_ms = 0;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        state_of(&mut e, serde_json::json!({}));
        assert_eq!(e.backend().snapshots, snaps + 3);
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
}
