//! The engine: platform-agnostic implementation of the tool contract on top
//! of a [`Backend`]. It resolves apps/windows/elements, renders app state
//! (tree + screenshot), maintains per-app element indices and diffs, maps
//! screenshot coordinates to the screen, and keeps synthesized input on the
//! app it is meant for. It does no access control (see the security skill).

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
use crate::screens::{PixelSig, Screen, ScreenMemory, View};
use crate::tools::*;
use crate::tree::{self, IndexAllocator, Node};
use crate::types::*;

mod deciding;
mod scripting;

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
    /// Areas of the window the tree says nothing about ([ocr]
    /// blind_regions), in the latest snapshot (screen coordinates).
    blind: Vec<Rect>,
    /// The last picture those areas were found and read in: its
    /// fingerprint, the areas and the text read in them.
    blind_cache: Option<(PixelSig, Vec<Rect>, Vec<OcrLine>)>,
    /// The text last read in each blind area, by its place and a hash of
    /// its exact pixels: an area that didn't change isn't read again when
    /// another part of the window did (a caret, a clock).
    area_reads: Vec<(Rect, u64, Vec<OcrLine>)>,
    /// The header of the last get_app_state (app, window, place): the next
    /// one is short if it is the same ([tree] compact).
    header_seen: Option<String>,
    /// Elements whose line changed in the last reports, by key, and in how
    /// many of them in a row ([tree] quiet_volatile).
    volatile: HashMap<u64, u8>,
    /// Looks at this app, and calls that used its pixels (x/y, a
    /// screenshot asked for): [screenshot] adaptive.
    looks: u32,
    pixel_uses: u32,
    /// Unnamed buttons already shown in an icon strip, by key.
    icons_shown: HashSet<u64>,
    /// `sent_tokens` when this app's tree was last sent whole ([cache]
    /// rebase_after_tokens).
    full_at: usize,
}

/// What the model was last shown of a design or a scene: the next answer
/// says only what changed ([tree] compact).
#[derive(Default, Clone)]
struct DraftSeen {
    head: String,
    items: Vec<(String, String)>,
    checks: String,
    steps: String,
    picture: u64,
    /// The cells named on a design's picture, as said.
    cells: String,
}

/// What the result of the current top-level call holds, for hosts that
/// drop results a later one repeats ([server] result_meta).
#[derive(Default)]
struct ResultNote {
    /// Apps whose whole tree it shows (a look), and apps it looked at
    /// with a diff.
    looks_full: Vec<u32>,
    looks_diff: Vec<u32>,
    /// Apps it has a whole picture of, and a changed part of.
    pictures_whole: Vec<u32>,
    pictures_part: Vec<u32>,
    /// Designs and scenes ("design:<name>") it shows whole, or what
    /// changed in them, and those it has a picture of.
    drafts_full: Vec<String>,
    drafts_diff: Vec<String>,
    drafts_picture: Vec<String>,
}

/// The results that showed something, by what they showed, for
/// [`ResultNote`].
#[derive(Default)]
struct Results {
    looks: HashMap<u32, Vec<u64>>,
    pictures: HashMap<u32, Vec<u64>>,
    drafts: HashMap<String, Vec<u64>>,
    draft_pictures: HashMap<String, Vec<u64>>,
}

/// What an action's `expect` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Confirmed,
    NotSeen,
    Uncertain,
}

/// An app as it was before an action that expects something.
struct Before {
    window: Option<u64>,
    windows: Vec<u64>,
    nodes: Vec<Node>,
    fingerprint: Option<u64>,
}

/// A screenshot handed out but not yet known to have reached the model
/// (a batch keeps only its last image).
struct PendingImage {
    pid: u32,
    screen: u32,
    coord: CoordMap,
    pixels: Option<PixelSig>,
    /// The number of the whole screenshot x/y refer to (a changed part
    /// patches that one).
    id: u32,
}

/// What goes back of a window's pixels ([`Engine::window_picture`]).
enum Picture {
    /// The model's picture of this screen is current (numbered `base`).
    Unchanged { base: Option<u32> },
    /// Only the part that changed, at `at` in the picture numbered `base`.
    Part {
        img: EncodedImage,
        id: u32,
        base: Option<u32>,
        at: (u32, u32),
        changed: Option<Changed>,
    },
    /// The whole window (an overview when `overview`).
    Whole {
        img: EncodedImage,
        id: u32,
        overview: bool,
        changed: Option<Changed>,
    },
}

/// Where a window's pixels changed since the model's last picture of it.
#[derive(Clone, Copy)]
struct Changed {
    /// On screen.
    screen: Rect,
    /// In the screenshot the model has, as x/y take them.
    shot: (u32, u32, u32, u32),
    /// Share of the window.
    share: f64,
}

/// The last full-screen screenshot the model got: its fingerprint, scale
/// and number.
#[derive(Clone)]
struct ScreenShot {
    pixels: PixelSig,
    coord: CoordMap,
    id: u32,
}

/// How deep calls may nest (batch steps and a script's tools run inside the
/// call that started them).
const MAX_DEPTH: u32 = 8;

/// Failed starts or crashes of the overlay helper before the engine stops
/// trying (until the server restarts).
const MAX_OVERLAY_FAILURES: u32 = 5;

/// `draw`: the farthest the pointer moves in one step (screen units), so
/// apps see a continuous line.
const DRAW_STEP: f64 = 3.0;
/// `draw`: most pointer positions in one call.
const DRAW_MAX_POINTS: usize = 40_000;
/// `draw`: most strokes in one call.
const DRAW_MAX_STROKES: usize = 1000;
/// `draw`: default pointer speed, screen units per second.
const DRAW_SPEED: f64 = 800.0;
/// `draw`: longest drawing in one call.
const DRAW_MAX_SECS: f64 = 120.0;
/// `scroll`: most pages in one call.
const MAX_SCROLL_PAGES: f64 = 50.0;
/// `type_text`: most characters in one call.
const MAX_TYPED_CHARS: usize = 100_000;
/// `type_text`: characters sent at once, so the stop key works mid-text.
const TYPE_CHUNK: usize = 200;
/// `press_key`: most key presses in one call.
const MAX_KEY_PRESSES: usize = 500;
/// `get_clipboard`: most characters returned.
const MAX_CLIPBOARD_CHARS: usize = 30_000;

pub struct Engine<B: Backend> {
    backend: B,
    store: ConfigStore,
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
    /// The call depth of a batch's steps: their reports are never shown
    /// (the batch reports once, at the end), so they aren't made.
    quiet_depth: Option<u32>,
    /// The on-screen indicator (a separate helper process), if running.
    overlay: Option<Overlay>,
    /// How to start it; set by the host (`with_overlay`).
    overlay_launcher: Option<Launcher>,
    /// Failed starts and crashes, so a helper that keeps failing is left
    /// alone. (Stopping it because the settings turned it off isn't one.)
    overlay_failures: u32,
    overlay_retry_at: Option<Instant>,
    /// Why the helper (and with it the stop key) last failed to start.
    overlay_error: Option<String>,
    /// The user was told the stop key doesn't work (told once).
    stop_note_shown: bool,
    /// Explanations already given in full.
    hints: Hints,
    /// Set by the user's stop key (through the overlay helper) or the host;
    /// while set, every tool call is refused.
    stop: Arc<AtomicBool>,
    /// Set when the client cancels the call in progress: it ends as the stop
    /// key would end it, and is cleared when the call ends. (A client that
    /// goes away closes the server's input; MCP then ends the server.)
    cancel: Arc<AtomicBool>,
    /// When the engine's own synthesized input last ended, so the system
    /// idle time isn't mistaken for the user's input.
    last_input: Option<Instant>,
    /// The action epoch whose result has a fresh snapshot (after settling).
    settled: Option<u64>,
    /// The last full-screen screenshot sent.
    screen_shot: Option<ScreenShot>,
    /// One taken during this call: it becomes `screen_shot` only if it is
    /// the image the call returns (a batch returns only its last image).
    pending_screen_shot: Option<ScreenShot>,
    /// Screenshots handed out so far: each gets the next number, so a
    /// changed part can name the picture it patches.
    shots: u32,
    /// The element the current action acts on (app pid, index), for a
    /// report of the changes around it ([tree] report = "relevant").
    target: Option<(u32, u32)>,
    /// Tool categories `find_tools` has added to the tool list ([tools]
    /// manager = "list_changed"); they stay.
    active_tools: HashSet<&'static str>,
    /// The tool lists, until what they depend on changes.
    tools_cache: Option<scripting::ToolsCache>,
    /// Tools whose arguments the tool manager has shown, with a hash of
    /// what it showed: shown again only if they changed, or when asked.
    schemas_shown: HashMap<String, u64>,
    /// The app the last call named ([tools] default_app).
    last_app: Option<String>,
    /// What the last action's `expect` found (a batch stops on anything
    /// but confirmed).
    last_expect: Option<Outcome>,
    /// Estimated tokens of every result handed out so far ([cache]
    /// rebase_after_tokens).
    sent_tokens: usize,
    /// What each design and scene looked like in its last answer, by
    /// "design:<name>" / "scene:<name>".
    drafts_seen: HashMap<String, DraftSeen>,
    /// Top-level results so far, what the current one holds, what earlier
    /// ones held, and the `_meta` for the last one ([server] result_meta).
    result_id: u64,
    note: ResultNote,
    results: Results,
    result_meta: Option<serde_json::Value>,
    /// Read text off the screen in the next observe (get_app_state ocr=true).
    force_ocr: bool,
    /// Reuse the last OCR result instead of reading again (while settling).
    ocr_reuse: bool,
    /// Why OCR isn't available, once found out (told to the model once).
    ocr_note: Option<String>,
    ocr_note_shown: bool,
    /// The start of a tree an action's result showed (cut short): its app,
    /// screen, a hash of the whole text and the lines shown. If the next
    /// get_app_state renders the same text, those lines aren't sent again.
    partial_report: Option<(u32, u32, u64, usize)>,
    /// A window capture taken for OCR at this epoch, reused as the screenshot.
    last_capture: Option<(u32, u64, u64, Capture)>,
    /// Config file modification time, for hot reload.
    config_mtime: Option<std::time::SystemTime>,
    /// The file changed but didn't load: read it once more on the next call.
    config_retry: bool,
    /// Host-level overrides (e.g. command-line flags) re-applied on reload.
    overrides: Option<ConfigOverride>,
    /// Pictures traced with trace_image, by name (newest last).
    traces: Vec<(String, crate::paint::Trace)>,
    /// Designs on the design board, by name (newest last).
    designs: Vec<(String, crate::design::Design)>,
    /// 3D scenes, by name (newest last).
    scenes: Vec<(String, crate::scene::Scene)>,
    fonts: crate::design::FontCache,
    /// Exported files (temporary).
    exports: crate::design::TempFiles,
    /// Saved scripts.
    scripts: crate::script::Library,
    /// The decision layer: the model's answers kept, its failures, what
    /// it cost (see [`crate::decision::judge`]).
    judge: crate::decision::judge::Judge,
    /// A script is running: its calls can't start another (by any route,
    /// `batch` included), or scripts could nest until the stack runs out.
    in_script: bool,
    clock: Box<dyn Fn() -> Instant + Send>,
    sleep: Box<dyn Fn(Duration) + Send>,
}

/// What the model had seen before a batch or a script ran.
pub(super) struct SeenBefore {
    known: HashMap<u32, Option<Screen>>,
    memory: HashMap<u32, crate::screens::View>,
    hints: HashSet<&'static str>,
}

/// Host-level settings forced on top of the config file.
type ConfigOverride = Box<dyn Fn(&mut crate::config::Config) + Send>;

/// Explanations the model gets in full the first time and in a short form
/// after that: the full one is already in its context, and repeating it on
/// every call only costs tokens.
#[derive(Default)]
struct Hints(std::cell::RefCell<HashSet<&'static str>>);

/// What the model had been shown before a call ([`Engine::shown`]).
pub struct Shown {
    hints: HashSet<&'static str>,
    /// Per app: the screen the model knows, and the screenshot coordinates
    /// it works from.
    apps: HashMap<u32, (Option<Screen>, Option<CoordMap>)>,
    screen_shot: Option<ScreenShot>,
    partial: Option<(u32, u32, u64, usize)>,
}

impl Hints {
    /// Whether `key` is being explained for the first time (it then counts
    /// as explained).
    fn first(&self, key: &'static str) -> bool {
        self.0.borrow_mut().insert(key)
    }
}

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
    /// Where the elements a diff reports added or changed are (screen).
    touched: Vec<Rect>,
    /// Elements a diff reports removed.
    removed: usize,
    /// Changes left out: elements that keep changing on their own.
    restless: Vec<u32>,
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
        let scripts = crate::script::Library::new(store.config.script.library());
        Self {
            backend,
            store,
            states: HashMap::new(),
            memory: ScreenMemory::default(),
            app_cache: None,
            window_cache: HashMap::new(),
            epoch: 0,
            pending_images: Vec::new(),
            depth: 0,
            quiet_depth: None,
            overlay: None,
            overlay_launcher: None,
            overlay_failures: 0,
            overlay_retry_at: None,
            overlay_error: None,
            stop_note_shown: false,
            hints: Hints::default(),
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            last_input: None,
            settled: None,
            screen_shot: None,
            pending_screen_shot: None,
            shots: 0,
            target: None,
            active_tools: HashSet::new(),
            tools_cache: None,
            schemas_shown: HashMap::new(),
            last_app: None,
            last_expect: None,
            sent_tokens: 0,
            drafts_seen: HashMap::new(),
            result_id: 0,
            note: ResultNote::default(),
            results: Results::default(),
            result_meta: None,
            force_ocr: false,
            ocr_reuse: false,
            traces: Vec::new(),
            designs: Vec::new(),
            scenes: Vec::new(),
            fonts: crate::design::FontCache::default(),
            exports: crate::design::TempFiles::default(),
            scripts,
            judge: Default::default(),
            in_script: false,
            ocr_note: None,
            ocr_note_shown: false,
            partial_report: None,
            last_capture: None,
            config_mtime,
            config_retry: false,
            overrides: None,
            clock: Box::new(Instant::now),
            sleep: Box::new(std::thread::sleep),
        }
    }

    /// Re-read the config file if it changed on disk (`hot_reload`). Cached
    /// UI state is kept.
    pub fn reload_if_changed(&mut self) {
        if !self.store.config.hot_reload {
            return;
        }
        let Some(path) = self.store.path.clone() else {
            return;
        };
        let mtime = file_mtime(&path);
        let changed = mtime != self.config_mtime;
        if !changed && !self.config_retry {
            return;
        }
        self.config_mtime = mtime;
        self.config_retry = false;
        // An editor saving the file may empty it first: an empty file is
        // taken only when it is still empty the next time.
        if changed && std::fs::metadata(&path).is_ok_and(|m| m.len() == 0) {
            self.config_retry = true;
            return;
        }
        match ConfigStore::load(Some(&path)) {
            Ok(mut store) => {
                if let Some(f) = &self.overrides {
                    f(&mut store.config);
                }
                log::info!("reloaded settings from {}", path.display());
                self.backend.configure(&store.config);
                self.store = store;
                self.scripts.set_dir(self.store.config.script.library());
                self.epoch += 1;
                self.overlay_reconfigure();
            }
            Err(e) => {
                log::warn!("keeping previous settings; {e}");
                // Maybe read half-written: look once more next time, even if
                // the finished file keeps this modification time.
                self.config_retry = changed;
            }
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

    /// Replace the settings (embedders that manage config themselves). The
    /// host's overrides still apply.
    pub fn set_config(&mut self, mut store: ConfigStore) {
        if let Some(f) = &self.overrides {
            f(&mut store.config);
        }
        self.backend.configure(&store.config);
        self.store = store;
        self.scripts.set_dir(self.store.config.script.library());
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
        let keys = self.global_keys();
        let cfg = &self.store.config.overlay;
        // The helper also listens for the global keys, so it runs when any
        // is wanted (with the overlay off it draws nothing).
        if !cfg.enabled && keys == crate::overlay::Keys::default() {
            self.overlay = None;
            return None;
        }
        if self.overlay.as_ref().is_some_and(|o| !o.alive()) {
            // It died: try again a little later, a few times at most.
            self.overlay = None;
            self.overlay_failures += 1;
            self.overlay_error = Some("it stopped unexpectedly".into());
            self.overlay_retry_at = Some((self.clock)() + Duration::from_secs(10));
        }
        if self.overlay.is_none() {
            let launcher = self.overlay_launcher.clone().or_else(|| {
                (!cfg.command.trim().is_empty())
                    .then(|| crate::overlay::find_helper(cfg))
                    .flatten()
            })?;
            if self.overlay_failures >= MAX_OVERLAY_FAILURES {
                if let Some(e) = &self.overlay_error
                    && !e.contains("gave up")
                {
                    self.overlay_error = Some(format!(
                        "{e}; gave up after {MAX_OVERLAY_FAILURES} tries, restart the server to try again"
                    ));
                }
                return None;
            }
            if self.overlay_retry_at.is_some_and(|t| (self.clock)() < t) {
                return None;
            }
            let on_settings = self.settings_handler();
            match Overlay::spawn(&launcher, cfg, &keys, self.stop.clone(), Some(on_settings)) {
                Ok(o) => {
                    self.overlay = Some(o);
                    self.overlay_error = None;
                }
                Err(e) => {
                    log::warn!("overlay unavailable: {e}");
                    self.overlay_failures += 1;
                    self.overlay_error = Some(e.to_string());
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
        let keys = self.global_keys();
        if !cfg.enabled && keys == crate::overlay::Keys::default() {
            self.overlay = None;
        } else if let Some(o) = &self.overlay {
            o.configure(&cfg, &keys);
        } else if keys != crate::overlay::Keys::default() {
            // Listen for the keys from now on, not only once work starts.
            self.overlay();
        }
    }

    /// The global keys the helper listens for.
    fn global_keys(&self) -> crate::overlay::Keys {
        let c = &self.store.config.control;
        crate::overlay::Keys {
            stop: c.stop_hotkey.trim().to_string(),
            settings: c.settings_hotkey.trim().to_string(),
        }
    }

    /// What the settings key does: open the decision model's settings page
    /// (from the helper's reader thread, whatever the engine is doing).
    fn settings_handler(&self) -> crate::overlay::OnSettings {
        let path = self.store.path.clone();
        Arc::new(move || {
            if let Err(e) = crate::decision::page::open(path.clone()) {
                log::warn!("couldn't open the settings page: {e}");
            }
        })
    }

    /// Whether the settings key works: `None` when there is none, or the
    /// helper hasn't said yet.
    pub fn settings_key_ok(&mut self, timeout: Duration) -> Option<bool> {
        let key = self.store.config.control.settings_hotkey.trim().to_string();
        if key.is_empty() {
            return None;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self.overlay.is_none() {
                self.overlay();
            }
            match &self.overlay {
                Some(o) if !o.alive() => return Some(false),
                Some(o) => {
                    if let Some(ok) = o.settings_key_ok(&key) {
                        return Some(ok);
                    }
                }
                None => return Some(false),
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Start listening for the user's stop key (and get the overlay ready)
    /// before the first tool call. Hosts call this once at start-up.
    pub fn arm(&mut self) {
        self.overlay();
    }

    /// Why the emergency stop key isn't working, if it is configured and
    /// known not to work: the helper that listens for it couldn't start or
    /// stopped, or the system refused the key combination.
    pub fn stop_key_problem(&mut self) -> Option<String> {
        let key = self.store.config.control.stop_hotkey.trim().to_string();
        if key.is_empty() {
            return None; // the user chose to have none
        }
        if self.overlay.is_none() {
            // Try to start it (cheap if it is already known not to start).
            self.overlay();
        }
        match &self.overlay {
            Some(o) if !o.alive() => Some("the helper that listens for it stopped".into()),
            Some(o) if o.hotkey_ok(&key) == Some(false) => Some(
                "the system refused it (another program may use this key, or there is no display the helper can use); set control.stop_hotkey to another combination".into(),
            ),
            Some(_) => None,
            None => self
                .overlay_error
                .as_ref()
                .map(|e| format!("the helper that listens for it could not start ({e})")),
        }
    }

    /// Wait up to `timeout` for the stop key to be confirmed working
    /// (`computer-use-mcp doctor`).
    pub fn check_stop_key(&mut self, timeout: Duration) -> std::result::Result<(), String> {
        let key = self.store.config.control.stop_hotkey.trim().to_string();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(problem) = self.stop_key_problem() {
                return Err(problem);
            }
            if self.overlay.as_ref().and_then(|o| o.hotkey_ok(&key)) == Some(true) {
                return Ok(());
            }
            if self.overlay.is_none() && self.overlay_error.is_none() {
                return Err("no overlay helper is set up to listen for it".into());
            }
            if Instant::now() >= deadline {
                return Err("the helper didn't confirm it in time".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether to give the explanation `key` in full: the first time, or
    /// every time when `tree.brief_repeats` is off.
    fn explain_first(&self, key: &'static str) -> bool {
        let first = self.hints.first(key);
        first || !self.store.config.tree.brief_repeats
    }

    /// `long` the first time (or always, without `tree.brief_repeats`),
    /// `short` afterwards.
    fn explain<'a>(&self, key: &'static str, long: &'a str, short: &'a str) -> &'a str {
        if self.explain_first(key) { long } else { short }
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

    /// Cancels the call in progress when set (from another thread, e.g. the
    /// server's reader when the client sends `notifications/cancelled`).
    /// Cleared when the call ends.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// Whether the call in progress must end now: the user stopped the
    /// agent, or the client cancelled the call.
    fn halted(&self) -> bool {
        self.is_stopped() || self.cancel.load(Ordering::SeqCst)
    }

    /// Stop the agent (or let it continue), as the stop key does.
    pub fn set_stopped(&mut self, on: bool) {
        self.stop.store(on, Ordering::SeqCst);
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Stopped { on });
        }
    }

    fn stopped_error(&self) -> Error {
        if !self.is_stopped() && self.cancel.load(Ordering::SeqCst) {
            return Error::Cancelled;
        }
        Error::Stopped(self.stop_control_name())
    }

    /// The stop key as the user knows it ("Ctrl+Alt+Esc").
    fn stop_control_name(&self) -> String {
        let key = self.store.config.control.stop_hotkey.trim();
        if key.is_empty() {
            "the host's stop control".into()
        } else {
            crate::overlay::helper::pretty_key(key)
        }
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
            if self.halted() {
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

    /// The part of `r` (screen coordinates) that is on a display, so a
    /// region from the model can't ask for an impossible capture.
    fn clip_to_screens(&mut self, r: Rect) -> Result<Rect> {
        /// Largest side accepted when the displays aren't known.
        const MAX_SIDE: f64 = 16_384.0;
        let Ok(displays) = self.backend.displays() else {
            return Ok(Rect::new(
                r.x,
                r.y,
                r.width.min(MAX_SIDE),
                r.height.min(MAX_SIDE),
            ));
        };
        let on = |d: &Rect| {
            let (x0, y0) = (r.x.max(d.x), r.y.max(d.y));
            let x1 = (r.x + r.width).min(d.x + d.width);
            let y1 = (r.y + r.height).min(d.y + d.height);
            (x1 - x0 >= 1.0 && y1 - y0 >= 1.0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
        };
        // The bounding box of the region's parts on every display it touches.
        let parts: Vec<Rect> = displays.iter().filter_map(|d| on(&d.bounds)).collect();
        if parts.is_empty() {
            return Err(Error::InvalidArgs(format!(
                "the area ({:.0}, {:.0}) {:.0}x{:.0} is not on any display; window(action=\"displays\") lists them",
                r.x, r.y, r.width, r.height
            )));
        }
        let x0 = parts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let y0 = parts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let x1 = parts
            .iter()
            .map(|p| p.x + p.width)
            .fold(f64::NEG_INFINITY, f64::max);
        let y1 = parts
            .iter()
            .map(|p| p.y + p.height)
            .fold(f64::NEG_INFINITY, f64::max);
        Ok(Rect::new(x0, y0, x1 - x0, y1 - y0))
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
            let ql = crate::text::fold(q);
            if let Some(w) = windows.iter().find(|w| w.id.to_string() == q) {
                return Ok(w.clone());
            }
            let matches: Vec<&WindowInfo> = windows
                .iter()
                .filter(|w| crate::text::fold(&w.title).contains(&ql))
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
            _ => {
                let mut lines = self.run_ocr(&cap);
                lines.retain(|l| !self.overlay_text(&l.text));
                lines
            }
        };
        self.states.entry(app.pid).or_default().ocr_cache = Some((sig, lines.clone()));
        self.last_capture = Some((app.pid, window.id, self.epoch, cap));
        lines
    }

    /// Whether text read off the screen is the on-screen indicator's own
    /// label (a picture can catch it where the platform can't leave it
    /// out): never the app's text.
    fn overlay_text(&self, text: &str) -> bool {
        let o = &self.store.config.overlay;
        if !o.enabled || self.overlay.is_none() {
            return false;
        }
        let read = crate::text::fold(text.trim().trim_end_matches(['…', '.']));
        if read.chars().count() < 4 {
            return false;
        }
        [
            &o.label_working,
            &o.label_thinking,
            &o.label_error,
            &o.label_done,
            &o.label_paused,
            &o.label_stopped,
        ]
        .iter()
        .any(|label| reads_as_start_of(&read, &crate::text::fold(label)))
    }

    /// Recognise the text in a capture with the configured engine.
    fn run_ocr(&mut self, cap: &Capture) -> Vec<OcrLine> {
        use crate::config::OcrEngineChoice;
        let cfg = self.store.config.ocr.clone();
        // As it is and enlarged, without grid lines: enlarging alone turns
        // big framed labels into one glyph.
        let tesseract = || {
            crate::ocr::tesseract_both(
                cap,
                &cfg.languages,
                &cfg.tesseract_path,
                crate::ocr::Layout::Sparse,
            )
        };
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
        // Custom-drawn UI: add the text read off the window, or off the
        // areas of it the tree says nothing about.
        let mut ocr_lines = 0;
        let mut blind = Vec::new();
        let lines = if raw.is_empty() {
            Vec::new()
        } else if self.ocr_wanted(&raw) {
            self.read_screen_text(app, window)
        } else if self.blind_wanted() {
            let (areas, lines) = self.read_blind_areas(app, window, &raw);
            blind = areas;
            lines
        } else {
            Vec::new()
        };
        if !lines.is_empty() {
            let cfg = &self.store.config.ocr;
            let mut extra = crate::ocr::nodes(&lines, cfg.min_confidence, cfg.max_lines);
            // Leave out glyph noise and what the tree already says there.
            extra.retain(|o| {
                let line = OcrLine {
                    text: o.name.clone().unwrap_or_default(),
                    bounds: o.bounds.unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0)),
                    confidence: 1.0,
                };
                // Shapes read as glyphs, and rulers' numbers, aren't text.
                if !crate::ocr::plausible(&line) || crate::ocr::ruler(&line) {
                    return false;
                }
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
        st.blind = blind;
        self.states.insert(app.pid, st);
        Ok(())
    }

    /// Look for areas the tree says nothing about ([ocr] blind_regions):
    /// pixels are needed to tell them from empty background.
    fn blind_wanted(&self) -> bool {
        let cfg = &self.store.config;
        cfg.ocr.blind_regions && cfg.screenshot.enabled && !cfg.text_only
    }

    /// The areas of a window no informative element covers that show
    /// something, and the text read off them (unless OCR is off). An
    /// unchanged picture isn't read again, and while settling after an
    /// action the last reading is kept.
    fn read_blind_areas(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        raw: &[RawNode],
    ) -> (Vec<Rect>, Vec<OcrLine>) {
        let Some(bounds) = window.bounds else {
            return Default::default();
        };
        let candidates = crate::coverage::uncovered(raw, bounds);
        if candidates.is_empty() {
            return Default::default();
        }
        let cached = self
            .states
            .get(&app.pid)
            .and_then(|s| s.blind_cache.clone());
        if self.ocr_reuse
            && let Some((_, areas, lines)) = &cached
        {
            return (areas.clone(), lines.clone());
        }
        let cap = match self.capture_clean(|b| b.capture(app, window)) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no capture for blind areas: {e}");
                return Default::default();
            }
        };
        let cache = &self.store.config.cache;
        let sig = PixelSig::of(&cap, cache.pixel_grid);
        let (areas, lines) = match cached {
            Some((old, areas, lines)) if old.same_as(&sig, cache.pixel_tolerance) => (areas, lines),
            _ => {
                let areas: Vec<Rect> = candidates
                    .into_iter()
                    .filter(|a| crate::coverage::shows_something(&cap, a))
                    .map(|a| a.rect)
                    .collect();
                let mut lines = Vec::new();
                if self.store.config.ocr.mode != crate::config::OcrMode::Off {
                    let before = self
                        .states
                        .get_mut(&app.pid)
                        .map(|s| std::mem::take(&mut s.area_reads))
                        .unwrap_or_default();
                    let mut reads = Vec::new();
                    for r in &areas {
                        if let Some(px) = crate::coverage::pixels_of(&cap, *r) {
                            let crop = imaging::crop(&cap, px);
                            let hash = pixels_hash(&crop);
                            let kept = before
                                .iter()
                                .find(|(rect, h, _)| rect == r && *h == hash)
                                .map(|(_, _, read)| read.clone());
                            let read: Vec<OcrLine> = match kept {
                                Some(read) => read,
                                None => self
                                    .run_area_ocr(&crop)
                                    .into_iter()
                                    // Only what is in the area (an engine
                                    // may read around it).
                                    .filter(|l| {
                                        let b = l.bounds;
                                        r.contains(Point::new(
                                            b.x + b.width / 2.0,
                                            b.y + b.height / 2.0,
                                        )) && crate::ocr::plausible(l)
                                            && !self.overlay_text(&l.text)
                                    })
                                    .collect(),
                            };
                            lines.extend(read.iter().cloned());
                            reads.push((*r, hash, read));
                        }
                    }
                    self.states.entry(app.pid).or_default().area_reads = reads;
                }
                (areas, lines)
            }
        };
        self.states.entry(app.pid).or_default().blind_cache =
            Some((sig, areas.clone(), lines.clone()));
        self.last_capture = Some((app.pid, window.id, self.epoch, cap));
        (areas, lines)
    }

    /// Read a part of a window. Tesseract reads it both enlarged and as it
    /// is: a part can hold big painted labels as well as small text.
    fn run_area_ocr(&mut self, cap: &Capture) -> Vec<OcrLine> {
        use crate::config::OcrEngineChoice;
        let cfg = self.store.config.ocr.clone();
        let layout = crate::ocr::Layout::for_height(cap.bounds.height);
        let tesseract =
            || crate::ocr::tesseract_both(cap, &cfg.languages, &cfg.tesseract_path, layout);
        let result = match cfg.engine {
            OcrEngineChoice::Native => self.backend.ocr(cap, &cfg.languages),
            OcrEngineChoice::Tesseract => tesseract(),
            OcrEngineChoice::Auto => self
                .backend
                .ocr(cap, &cfg.languages)
                .or_else(|_| tesseract()),
        };
        match result {
            Ok(lines) => lines,
            Err(e) => {
                if self.ocr_note.is_none() {
                    log::warn!("text recognition unavailable: {e}");
                }
                self.ocr_note = Some(e.to_string());
                Vec::new()
            }
        }
    }

    /// Render the latest snapshot against what the model has seen of that
    /// screen: a diff, the full tree, or a note that nothing changed.
    fn render(&self, pid: u32, full: bool, max_tokens: Option<usize>) -> Result<Refreshed> {
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
            touched: Vec::new(),
            removed: 0,
            restless: Vec::new(),
        };
        let mut budget = tree::Budget::from_config(tcfg);
        if let Some(tokens) = max_tokens {
            budget.tokens = tokens;
        }
        match base {
            None => {
                out.text = tree::render_full_within(nodes, tcfg.indent, budget);
                out.full = true;
            }
            Some(view) => {
                let mut d = view.diff(nodes);
                // Elements that keep changing on their own: one line.
                let mut restless: Vec<u32> = Vec::new();
                if tcfg.quiet_volatile && seen == Seen::Same {
                    d.changed.retain(|(pos, _)| {
                        let n = &nodes[*pos];
                        let busy = st.volatile.get(&n.key).is_some_and(|c| *c >= 2);
                        if busy {
                            restless.push(n.index);
                        }
                        !busy
                    });
                }
                out.changes = d.len();
                out.removed = d.removed.len();
                out.touched = d
                    .added
                    .iter()
                    .chain(d.changed.iter().map(|(p, _)| p))
                    .filter_map(|&p| nodes[p].bounds)
                    .collect();
                let large = out.changes as f64 >= tcfg.diff_full_ratio * nodes.len().max(1) as f64;
                if full || !tcfg.diff || large {
                    out.text = tree::render_full_within(nodes, tcfg.indent, budget);
                    out.full = true;
                    out.large_change = large;
                } else if seen == Seen::Same {
                    out.text = if d.is_empty() {
                        tree::render_diff(&d, nodes)
                    } else {
                        // In full once, the legend once more, then a word.
                        let intro = if self.explain_first("diff") {
                            tree::DIFF_INTRO
                        } else {
                            self.explain(
                                "diff-legend",
                                "Changes (+ added, ~ changed, - removed):",
                                "Changes:",
                            )
                        };
                        tree::render_diff_with(&d, nodes, intro, tcfg.compact)
                    };
                    if !restless.is_empty() {
                        out.text.push_str(&format!(
                            "~ {} element(s) that keep changing on their own left out: {}\n",
                            restless.len(),
                            tree::ranges(&mut restless.clone())
                        ));
                    }
                    out.restless = restless;
                } else if d.is_empty() {
                    out.text = if self.explain_first("revisit") {
                        format!(
                            "Identical to when you last saw screen #{}; element indices are as they were then.\n",
                            st.screen
                        )
                    } else {
                        format!("Identical to screen #{} as you saw it.\n", st.screen)
                    };
                } else {
                    let intro = if self.explain_first("revisit") {
                        format!(
                            "Changes since you last saw screen #{} (+ added, ~ changed, - removed). Other elements are as they were then, with the same indices.",
                            st.screen
                        )
                    } else {
                        format!("Changes since you saw screen #{} (+/~/-):", st.screen)
                    };
                    out.text = tree::render_diff_with(&d, nodes, &intro, tcfg.compact);
                }
                // A diff gets the same budget as a whole tree.
                if !out.full
                    && budget.active()
                    && budget.level == crate::config::Summarize::Normal
                    && crate::text::estimate_tokens(&out.text) > budget.tokens
                {
                    out.text = tree::cut_to_budget(&out.text, budget.tokens);
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
        let target_key = self
            .target
            .filter(|(p, _)| *p == pid)
            .and_then(|(_, i)| st.nodes.iter().find(|n| n.index == i))
            .map(|n| n.key);
        let quiet_volatile = self.store.config.tree.quiet_volatile;
        let size = window.bounds.map(|b| (b.width, b.height));
        let origin = window.bounds.map(|b| (b.x, b.y));
        let mut size_changed = false;
        match st.known.as_mut() {
            Some(k) if k.id == st.screen => {
                // Which elements changed since the model's last look, and
                // how many looks in a row they did (the acted-on element's
                // change isn't its own doing).
                if quiet_volatile {
                    let d = k.view.diff(&st.nodes);
                    // The focused element changes with the model's own keys.
                    let changed: HashSet<u64> = d
                        .changed
                        .iter()
                        .map(|(p, _)| &st.nodes[*p])
                        .filter(|n| !n.states.focused)
                        .map(|n| n.key)
                        .collect();
                    st.volatile.retain(|key, _| changed.contains(key));
                    for key in changed {
                        if Some(key) != target_key {
                            let c = st.volatile.entry(key).or_default();
                            *c = c.saturating_add(1);
                        } else {
                            st.volatile.remove(&key);
                        }
                    }
                }
                // The same screen in a window that moved or changed size: the
                // model's screenshot still names the same places only if the
                // size is the same, shifted by the move. (The tree's text
                // doesn't change, so nothing else would notice.)
                match (k.size, size) {
                    (Some(a), Some(b)) if (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0 => {
                        if let (Some(o0), Some(o1)) = (k.origin, origin) {
                            let (dx, dy) = (o1.0 - o0.0, o1.1 - o0.1);
                            if dx != 0.0 || dy != 0.0 {
                                for c in [k.coord.as_mut(), st.coord.as_mut()].into_iter().flatten()
                                {
                                    c.bounds.x += dx;
                                    c.bounds.y += dy;
                                }
                            }
                        }
                    }
                    (Some(_), Some(_)) => {
                        k.coord = None;
                        k.pixels = None;
                        k.shot = false;
                        k.shot_id = None;
                        st.coord = None;
                        size_changed = true;
                    }
                    _ => {}
                }
                k.view = view;
                k.window = window.id;
                k.size = size;
                k.origin = origin;
            }
            _ => {
                // Where the window was when the model last saw it (another
                // window's place says nothing about this one).
                let before = st
                    .known
                    .as_ref()
                    .filter(|k| k.window == window.id)
                    .map(|k| (k.origin, k.size));
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
                            next.shot_id = None;
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
                } else if let Some(c) = st.coord.as_mut() {
                    // No picture of this screen yet: x/y still refer to the
                    // last one the model saw, which follows the window if it
                    // moved and is no use if it changed size.
                    match (before, origin, size) {
                        (Some((Some(o0), Some(s0))), Some(o1), Some(s1))
                            if (s0.0 - s1.0).abs() < 1.0 && (s0.1 - s1.1).abs() < 1.0 =>
                        {
                            c.bounds.x += o1.0 - o0.0;
                            c.bounds.y += o1.1 - o0.1;
                        }
                        (Some((_, Some(_))), _, Some(_)) => {
                            st.coord = None;
                            size_changed = true;
                        }
                        _ => {}
                    }
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
        if let Some(shot) = self.pending_screen_shot.take() {
            self.screen_shot = Some(shot);
        }
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
                s.shot_id = Some(p.id);
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
    fn element_by_index(&mut self, app: &AppInfo, index: u32) -> Result<ElementHandle> {
        self.target = Some((app.pid, index));
        self.handle_of(app, index)
    }

    /// The handle of an element by index, without making it the action's
    /// target.
    fn handle_of(&self, app: &AppInfo, index: u32) -> Result<ElementHandle> {
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

    /// Where synthesized keyboard/mouse input for `app` goes, after making
    /// sure it really goes there. Such input lands in whatever window is in
    /// front (on backends where [`Backend::input_needs_front`]), so the app's
    /// window is brought to the front first, and nothing is sent if it can't
    /// be: keys meant for one app must never land in another (the terminal
    /// the agent runs in, a chat window…).
    fn input_target(&mut self, app: &AppInfo) -> Result<InputTarget> {
        if self.backend.input_needs_front() {
            self.bring_to_front(app)?;
        }
        Ok(self.target_of(app))
    }

    fn bring_to_front(&mut self, app: &AppInfo) -> Result<()> {
        let front = |e: &mut Self| -> Result<Option<AppInfo>> {
            Ok(e.find_apps()?.into_iter().find(|a| a.frontmost))
        };
        match front(self)? {
            // In front already, or the platform can't tell what is in front
            // (no window manager): nothing to do.
            None => return Ok(()),
            Some(f) if f.pid == app.pid => return Ok(()),
            Some(_) => {}
        }
        let windows = self.list_windows(app, true)?;
        let current = self.states.get(&app.pid).and_then(|s| s.window_id);
        let window = windows
            .iter()
            .find(|w| Some(w.id) == current)
            .or_else(|| windows.first())
            .cloned()
            .ok_or_else(|| Error::NoWindows {
                app: app.name.clone(),
            })?;
        let focus = self.backend.window_op(app, &window, &WindowOp::Focus);
        self.epoch += 1;
        let deadline = (self.clock)() + Duration::from_millis(1000);
        loop {
            match front(self)? {
                None => return Ok(()),
                Some(f) if f.pid == app.pid => return Ok(()),
                Some(f) if (self.clock)() >= deadline => {
                    let why = focus.err().map(|e| format!(" ({e})")).unwrap_or_default();
                    return Err(Error::ActionFailed(format!(
                        "{} could not be brought to the front{why}, so the keyboard/mouse input was not sent: it would have gone to {}, which is in front. Use element_index actions (they work in the background), or ask the user to bring {} forward.",
                        app.name, f.name, app.name
                    )));
                }
                Some(_) => (self.sleep)(Duration::from_millis(50)),
            }
        }
    }

    fn target_of(&self, app: &AppInfo) -> InputTarget {
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
        &mut self,
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
    ///
    /// Reads that still show exactly what was there before the action are
    /// not trusted at once: many apps (browsers, Electron apps) report a
    /// change a little after making it. Only after `NO_CHANGE_GRACE` of
    /// reads like that does the action count as having changed nothing.
    fn settle_on(&mut self, app: &AppInfo) {
        use crate::config::SettleMode;
        /// How long reads may keep showing the old state before "nothing
        /// changed" is believed.
        const NO_CHANGE_GRACE: Duration = Duration::from_millis(500);
        // The latest read is still the one from before the action.
        let before = self.tree_fingerprint(app.pid);
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
        // Time waited, counted in poll intervals (independent of the clock).
        let mut waited = Duration::from_millis(cfg.timing.settle_ms);
        // Text read off the screen isn't read again for every look.
        let reuse = std::mem::replace(&mut self.ocr_reuse, true);
        // While reads still show the state from before, they come further
        // apart (up to 2 polls): a change is still seen within one of them,
        // and an action that changed nothing costs half the reads.
        let mut interval = poll;
        while let Ok(window) = self.pick_window(app, None, true) {
            if self.observe(app, &window, true).is_err() {
                break;
            }
            self.settled = Some(self.epoch);
            let now = self.tree_fingerprint(app.pid);
            let steady = now.is_some() && now == last;
            if !adaptive || (steady && (now != before || waited >= NO_CHANGE_GRACE)) {
                break;
            }
            let unchanged = now.is_some() && now == before;
            last = now;
            if self.halted() || (self.clock)() >= deadline || waited >= max {
                break;
            }
            (self.sleep)(interval);
            waited += interval;
            interval = if unchanged {
                (interval * 2).min(poll * 2)
            } else {
                poll
            };
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
    pub fn call(&mut self, call: ToolCall) -> Result<ToolOutput> {
        if self.halted() {
            // Show it again so the user sees why nothing happens.
            if self.depth == 0 && self.is_stopped() && self.overlay.is_some() {
                self.overlay_send(OverlayCmd::Stopped { on: true });
            }
            let err = self.stopped_error();
            if self.depth == 0 {
                // A cancel is for this call only.
                self.cancel.store(false, Ordering::SeqCst);
            }
            return Err(err);
        }
        // Calls inside calls (batch steps, a script's tools) stay shallow.
        if self.depth >= MAX_DEPTH {
            return Err(Error::InvalidArgs(format!(
                "calls nest at most {MAX_DEPTH} deep (batch steps and scripts run tools inside a call)"
            )));
        }
        if self.in_script && matches!(call, ToolCall::Script(_)) {
            return Err(Error::InvalidArgs(
                "a script can't start another script, through batch or otherwise: run(name, args) runs a saved script inside it".into(),
            ));
        }
        if self.depth == 0 {
            self.target = None;
            // A picture is reused within the call that took it, never by a
            // later one (the screen may have moved on by itself).
            self.last_capture = None;
        }
        self.depth += 1;
        if self.depth == 1 {
            self.overlay_send(OverlayCmd::Begin);
        }
        // Calls that make big pictures or run on a thread of their own.
        let heavy = matches!(
            call,
            ToolCall::Design(_)
                | ToolCall::Scene(_)
                | ToolCall::TraceImage(_)
                | ToolCall::Script(_)
                | ToolCall::Screenshot(_)
                | ToolCall::Draw(_)
                | ToolCall::Locate(_)
        );
        let out = self.dispatch(call);
        if self.depth == 1 && self.overlay.is_some() {
            let ok = out.as_ref().is_ok_and(|o| !o.is_error);
            self.overlay_send(OverlayCmd::End { ok });
        }
        self.depth -= 1;
        if self.depth == 0 {
            self.cancel.store(false, Ordering::SeqCst);
            // Only the image in the final result reaches the model.
            let imaged = out.as_ref().is_ok_and(|o| o.image.is_some());
            if imaged {
                self.commit_images();
            } else {
                self.pending_images.clear();
                self.pending_screen_shot = None;
            }
            if imaged || heavy {
                trim_heap();
            }
        }
        out
    }

    fn dispatch(&mut self, call: ToolCall) -> Result<ToolOutput> {
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
        let report_app = acting.filter(|_| {
            self.store.config.tree.report_changes && self.quiet_depth != Some(self.depth)
        });
        let pixels_of_app = pixel_use(&call).map(str::to_string);
        // `expect`: the app as it was, to tell what the action did.
        let expecting = expectation(&call);
        let before = expecting.as_ref().and_then(|(query, ..)| {
            let pid = self.resolve_app(query).ok()?.pid;
            let st = self.states.get(&pid).filter(|s| s.stamped)?;
            Some(Before {
                window: st.window_id,
                windows: st.seen_windows.clone(),
                nodes: st.nodes.clone(),
                fingerprint: self.tree_fingerprint(pid),
            })
        });
        let out = match call {
            ToolCall::ListApps => self.list_apps(),
            ToolCall::LaunchApp(a) => self.launch_app(a),
            ToolCall::GetAppState(a) => self.get_app_state(a),
            ToolCall::Click(a) => self.click(a),
            ToolCall::PerformSecondaryAction(a) => self.perform_secondary(a),
            ToolCall::SetValue(a) => self.set_value(a),
            ToolCall::SelectText(a) => self.select_text(a),
            ToolCall::Scroll(a) => self.scroll(a),
            ToolCall::Drag(a) => self.drag(a),
            ToolCall::Draw(a) => self.draw(a),
            ToolCall::TraceImage(a) => self.trace_image(a),
            ToolCall::Design(a) => self.design(a),
            ToolCall::Scene(a) => self.scene(a),
            ToolCall::Locate(a) => self.locate(a),
            ToolCall::PressKey(a) => self.press_key(a),
            ToolCall::TypeText(a) => self.type_text(a),
            ToolCall::FindElement(a) => self.find_element(a),
            ToolCall::WaitFor(a) => self.wait_for(a),
            ToolCall::Screenshot(a) => self.screenshot(a),
            ToolCall::Batch(a) => self.batch(a),
            ToolCall::GetClipboard => self.get_clipboard(),
            ToolCall::SetClipboard(a) => self.set_clipboard(a),
            ToolCall::Window(a) => self.window_tool(a),
            ToolCall::GetNotifications(a) => self.get_notifications(a),
            ToolCall::Script(a) => self.script(a),
            ToolCall::Decide(a) => self.decide(a),
        };
        if mutating {
            self.last_input = Some((self.clock)());
        }
        let out = out?;
        if let Some(query) = pixels_of_app
            && let Ok(app) = self.resolve_app(&query)
        {
            let st = self.states.entry(app.pid).or_default();
            st.pixel_uses = st.pixel_uses.saturating_add(1);
        }
        self.last_expect = None;
        let checked = match expecting {
            Some((query, what, index)) if !out.is_error => {
                let (outcome, note) = self.check_expect(&query, &what, index, before.as_ref());
                self.last_expect = Some(outcome);
                Some(note)
            }
            _ => None,
        };
        let mut out = match report_app {
            Some(app) => self.append_changes(&app, out),
            None => out,
        };
        // Said with the action's line (the one a batch keeps).
        if let Some(note) = checked {
            let end = out.text.find('\n').unwrap_or(out.text.len());
            out.text.insert_str(end, &format!(" {note}"));
        }
        Ok(out)
    }

    /// Wait (up to [timing] expect_wait_ms) for what an action was expected
    /// to bring, and say what was found: confirmed, not seen, or uncertain.
    fn check_expect(
        &mut self,
        query: &str,
        what: &str,
        index: Option<u32>,
        before: Option<&Before>,
    ) -> (Outcome, String) {
        let timing = &self.store.config.timing;
        let wait = Duration::from_millis(timing.expect_wait_ms);
        let poll = Duration::from_millis(timing.settle_poll_ms.clamp(50, 1000) * 3);
        let start = (self.clock)();
        let mut fresh = self.settled != Some(self.epoch);
        let label = tree::truncate(what.trim().trim_matches('"'), 60);
        loop {
            let (mut outcome, mut detail, settled) =
                self.expect_now(query, what, index, before, fresh);
            let waited = (self.clock)().saturating_duration_since(start);
            if outcome == Outcome::Confirmed || settled || waited >= wait || self.halted() {
                // A text not there word for word may be there in other
                // words ("Saved" for "saved successfully"): the decision
                // model, when there is one, reads the window once. Without
                // one, "not seen" stands, and the agent looks.
                if outcome != Outcome::Confirmed
                    && !self.halted()
                    && let Some(yes) = self.expect_in_other_words(query, what)
                {
                    outcome = Outcome::Confirmed;
                    detail = format!("in other words, as the decision model reads it: {yes:.2}");
                }
                let detail = if detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({detail})")
                };
                let note = match outcome {
                    Outcome::Confirmed => format!("Expected {label}: confirmed{detail}."),
                    Outcome::NotSeen => format!(
                        "Expected {label}: not seen{detail}{}. Look before doing it again.",
                        if waited.as_millis() >= 100 {
                            format!(" after {:.1} s", waited.as_secs_f64())
                        } else {
                            String::new()
                        }
                    ),
                    Outcome::Uncertain => format!(
                        "Expected {label}: uncertain{detail}. Don't repeat the action; look first."
                    ),
                };
                return (outcome, note);
            }
            (self.sleep)(poll);
            fresh = true;
        }
    }

    /// The probability (at least 0.7) that the window shows an expected
    /// text in other words, as the decision model reads it; `None` when it
    /// doesn't, when `what` isn't a text, or when no model may be asked.
    fn expect_in_other_words(&mut self, query: &str, what: &str) -> Option<f64> {
        use crate::decision::{Answer, Question, judge::Use};
        let text = expected_text(what)?;
        let decider = self.auto_decider()?;
        let window = self.window_text(query, None, false).ok()?;
        let q = Question::yes_no(
            "shows",
            &format!(
                "Does the window show this, in these words or in others that mean the same: \"{text}\"?"
            ),
        );
        let (answers, _) = self.judged_one(&decider, Use::Auto, &window, &[q]).ok()?;
        match answers.first() {
            Some((_, Answer::YesNo { yes })) if *yes >= 0.7 => Some(*yes),
            _ => None,
        }
    }

    /// Whether what was expected shows now, and whether waiting longer
    /// could change the answer.
    fn expect_now(
        &mut self,
        query: &str,
        what: &str,
        index: Option<u32>,
        before: Option<&Before>,
        fresh: bool,
    ) -> (Outcome, String, bool) {
        use Outcome::*;
        let kind = crate::text::fold(what.trim().trim_matches('"'));
        let kind = kind.trim();
        let gone = matches!(
            kind,
            "gone" | "goes" | "closed" | "close" | "closes" | "disappear" | "disappears"
        );
        let Ok(app) = self.resolve_app(query) else {
            return if gone {
                (Confirmed, "the app closed".into(), true)
            } else {
                (Uncertain, "the app can't be found now".into(), true)
            };
        };
        let window = match self.resolve_window(&app, None, fresh) {
            Ok(w) => w,
            Err(_) if gone => return (Confirmed, "its window closed".into(), true),
            Err(_) => return (Uncertain, "the app shows no window".into(), false),
        };
        if self.observe(&app, &window, fresh).is_err() {
            return (Uncertain, "the app couldn't be read after it".into(), false);
        }
        let target = index.or_else(|| self.target.filter(|(p, _)| *p == app.pid).map(|t| t.1));
        let fingerprint = self.tree_fingerprint(app.pid);
        let Some(st) = self.states.get(&app.pid).filter(|s| s.stamped) else {
            return (Uncertain, "the app couldn't be read after it".into(), false);
        };
        let unseen = || {
            (
                Uncertain,
                "the app wasn't looked at before it, so there's nothing to compare".to_string(),
                true,
            )
        };
        match kind {
            "dialog" | "window" | "new window" | "popup" | "pop-up" | "sheet" | "alert" => {
                let Some(b) = before else { return unseen() };
                let opened = st.window_id != b.window
                    || st.seen_windows.iter().any(|w| !b.windows.contains(w));
                if opened {
                    (Confirmed, format!("window \"{}\"", window.title), true)
                } else {
                    (NotSeen, "no other window came up".into(), false)
                }
            }
            "menu" | "a menu" => {
                let Some(b) = before else { return unseen() };
                let menus = |nodes: &[Node]| {
                    nodes
                        .iter()
                        .filter(|n| n.role == "menu" || n.role == "menu item")
                        .filter(|n| !n.states.hidden)
                        .count()
                };
                if menus(&st.nodes) > menus(&b.nodes) || st.window_id != b.window {
                    (Confirmed, String::new(), true)
                } else {
                    (NotSeen, "no menu came up".into(), false)
                }
            }
            "change" | "changes" | "a change" | "anything" => {
                let Some(b) = before else { return unseen() };
                if fingerprint != b.fingerprint {
                    (Confirmed, String::new(), true)
                } else {
                    (NotSeen, "nothing changed".into(), false)
                }
            }
            "value" | "new value" | "value changes" | "checked" | "selected" => {
                let Some(b) = before else { return unseen() };
                let Some(t) = target else {
                    return (Uncertain, "no element_index to watch".into(), true);
                };
                let old = b.nodes.iter().find(|n| n.index == t);
                match (old, st.nodes.iter().find(|n| n.index == t)) {
                    (_, None) => (Uncertain, "the element is gone".into(), true),
                    (None, Some(_)) => unseen(),
                    (Some(o), Some(n))
                        if o.value != n.value
                            || o.states.checked != n.states.checked
                            || o.states.selected != n.states.selected =>
                    {
                        (Confirmed, String::new(), true)
                    }
                    _ => (NotSeen, "its value is as it was".into(), false),
                }
            }
            _ if gone => {
                let Some(b) = before else { return unseen() };
                if b.window.is_some_and(|w| !st.seen_windows.contains(&w)) {
                    return (Confirmed, "its window closed".into(), true);
                }
                match target {
                    Some(t) if !st.nodes.iter().any(|n| n.index == t) => {
                        (Confirmed, String::new(), true)
                    }
                    Some(_) => (NotSeen, "it is still there".into(), false),
                    None if st.window_id != b.window => {
                        (Confirmed, "another window is in front".into(), true)
                    }
                    None => (NotSeen, "the window is still there".into(), false),
                }
            }
            text => {
                // A text that should be on screen.
                let text = text
                    .strip_prefix("text ")
                    .unwrap_or(text)
                    .trim()
                    .trim_matches('"')
                    .to_string();
                let found = st
                    .nodes
                    .iter()
                    .filter(|n| !crate::privacy::is_password(&n.role))
                    .find(|n| node_text(n).contains(&text));
                match found {
                    Some(n) => (
                        Confirmed,
                        format!("{} {}", n.index, tree::truncate(&n.line, 80)),
                        true,
                    ),
                    None if !st.blind.is_empty() || st.ocr_lines > 0 => (
                        Uncertain,
                        "the tree doesn't show it, and part of the window is only a picture".into(),
                        false,
                    ),
                    None => (NotSeen, "no element shows it".into(), false),
                }
            }
        }
    }

    /// The one element with this name (and role), for a click by name.
    fn element_named(
        &mut self,
        app: &AppInfo,
        window: Option<&str>,
        name: Option<&str>,
        role: Option<&str>,
    ) -> Result<u32> {
        let w = self.resolve_window(app, window, false)?;
        self.observe(app, &w, false)?;
        let st = self.state(app.pid)?;
        let role = role.map(|r| r.trim().to_lowercase());
        let want = name.map(crate::text::fold);
        let of_role: Vec<&Node> = st
            .nodes
            .iter()
            .filter(|n| !n.states.hidden && role.as_deref().is_none_or(|r| n.role == r))
            .collect();
        let named = |exact: bool| -> Vec<&Node> {
            of_role
                .iter()
                .copied()
                .filter(|n| match &want {
                    None => true,
                    Some(q) => n.name.as_deref().map(crate::text::fold).is_some_and(|nm| {
                        if exact {
                            nm.trim() == q.trim()
                        } else {
                            nm.contains(q.trim())
                        }
                    }),
                })
                .collect()
        };
        let mut hits = named(true);
        let exact = !hits.is_empty();
        if !exact {
            hits = named(false);
        }
        // Of several with that very name, the ones that do something when
        // pressed (a part of a name must name one element).
        if exact && hits.len() > 1 {
            let active: Vec<&Node> = hits
                .iter()
                .copied()
                .filter(|n| !n.actions.is_empty() && n.states.enabled)
                .collect();
            if !active.is_empty() {
                hits = active;
            }
        }
        match hits.as_slice() {
            [one] => Ok(one.index),
            [] => Err(Error::InvalidArgs(format!(
                "no element in {} is named \"{}\"{}; find_element or get_app_state shows what is there",
                app.name,
                name.unwrap_or(""),
                role.map(|r| format!(" with the role {r}"))
                    .unwrap_or_default()
            ))),
            many => {
                let list: Vec<String> = many
                    .iter()
                    .take(6)
                    .map(|n| format!("{} {}", n.index, n.line))
                    .collect();
                Err(Error::InvalidArgs(format!(
                    "{} elements match, so nothing was clicked; click one by element_index (or give its role):\n{}",
                    many.len(),
                    list.join("\n")
                )))
            }
        }
    }

    /// Parse and run a raw tool call, returning a tool result even on error.
    pub fn call_tool(&mut self, name: &str, args: serde_json::Value) -> ToolOutput {
        self.reload_if_changed();
        self.result_id += 1;
        self.note = ResultNote::default();
        self.result_meta = None;
        let manager = self.store.config.tools.manager != crate::config::ToolManager::Off;
        // use_tool runs the tool it names, as if called directly.
        let (name, mut args) = if manager && name == "use_tool" {
            let inner = args
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if inner.is_empty() || crate::tools::MANAGER_TOOLS.contains(&inner.as_str()) {
                return ToolOutput::error(&Error::InvalidArgs(
                    "use_tool needs the name of a tool find_tools showed".into(),
                ));
            }
            if !self.has_tool(&inner) {
                return ToolOutput::error(&Error::InvalidArgs(format!(
                    "unknown tool: {inner} (find_tools lists them)"
                )));
            }
            let a = args
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            (inner, a)
        } else {
            (name.to_string(), args)
        };
        let name = name.as_str();
        if manager && name == "find_tools" {
            let out = self.find_tools(&args);
            self.audit(name, None, &out);
            return out;
        }
        // [tools] default_app: the last app named stands in for none.
        if self.store.config.tools.default_app
            && let serde_json::Value::Object(m) = &mut args
        {
            match m.get("app").and_then(serde_json::Value::as_str) {
                Some(a) if !a.trim().is_empty() => self.last_app = Some(a.to_string()),
                _ => {
                    if crate::tools::needs_app(name)
                        && let Some(last) = &self.last_app
                    {
                        m.insert("app".into(), serde_json::json!(last));
                    }
                }
            }
        }
        let app = args.get("app").and_then(|v| v.as_str()).map(str::to_string);
        let mut wrong_args = false;
        // A bug in one tool call must not take the whole server down.
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // A saved script is a tool of its own.
            let call = match self.saved_tool(name) {
                Some(_) => Ok(ToolCall::Script(ScriptArgs {
                    run: Some(name.to_string()),
                    args: Some(args),
                    ..Default::default()
                })),
                None => ToolCall::parse(name, args),
            };
            wrong_args = matches!(call, Err(Error::InvalidArgs(_)));
            call.and_then(|c| self.call(c))
        }));
        let mut out = match run {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                let mut out = ToolOutput::error(&e);
                if wrong_args && let Some(schema) = self.schema_for_wrong_call(name) {
                    out.text.push_str(&schema);
                }
                out
            }
            Err(panic) => {
                let what = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                log::error!("{name} panicked: {what}");
                // Undo what the interrupted call left half-done.
                self.depth = 0;
                self.in_script = false;
                self.force_ocr = false;
                self.ocr_reuse = false;
                self.pending_images.clear();
                self.pending_screen_shot = None;
                self.partial_report = None;
                self.cancel.store(false, Ordering::SeqCst);
                self.epoch += 1;
                self.overlay_send(OverlayCmd::End { ok: false });
                ToolOutput::error(&Error::Internal(format!(
                    "{name} failed unexpectedly ({what}); the screen may have changed, call get_app_state before going on"
                )))
            }
        };
        // The user is counting on the stop key: if it doesn't work, say so
        // (once), so the agent can tell them.
        if !self.stop_note_shown
            && let Some(problem) = self.stop_key_problem()
        {
            self.stop_note_shown = true;
            let key = crate::overlay::helper::pretty_key(&self.store.config.control.stop_hotkey);
            log::warn!("the emergency stop key {key} is not working: {problem}");
            out.text.push_str(&format!(
                "\n\nNote: the user's emergency stop key ({key}) is not working: {problem}. Tell the user now, so they know they can't stop you with it."
            ));
        }
        self.sent_tokens = self.sent_tokens.saturating_add(out.estimated_tokens());
        if !out.is_error {
            self.note_result();
        }
        self.audit(name, app.as_deref(), &out);
        out
    }

    /// The `_meta` of the last result for an MCP host ([server]
    /// result_meta): its number, and the earlier results it makes
    /// redundant: `supersedes` (looks at an app, or answers about a design
    /// or scene, that this one repeats whole) and `supersedes-images`
    /// (pictures of the same window or design that this one's picture
    /// replaces). A host may drop those from its context.
    pub fn take_result_meta(&mut self) -> Option<serde_json::Value> {
        self.result_meta.take()
    }

    fn note_result(&mut self) {
        let id = self.result_id;
        let note = std::mem::take(&mut self.note);
        let r = &mut self.results;
        let mut sup: Vec<u64> = Vec::new();
        let mut images: Vec<u64> = Vec::new();
        for pid in note.looks_full {
            sup.extend(r.looks.insert(pid, vec![id]).unwrap_or_default());
        }
        for pid in note.looks_diff {
            r.looks.entry(pid).or_default().push(id);
        }
        for pid in note.pictures_whole {
            images.extend(r.pictures.insert(pid, vec![id]).unwrap_or_default());
        }
        for pid in note.pictures_part {
            r.pictures.entry(pid).or_default().push(id);
        }
        for k in note.drafts_full {
            sup.extend(r.drafts.insert(k, vec![id]).unwrap_or_default());
        }
        for k in note.drafts_diff {
            r.drafts.entry(k).or_default().push(id);
        }
        for k in note.drafts_picture {
            images.extend(r.draft_pictures.insert(k, vec![id]).unwrap_or_default());
        }
        sup.retain(|x| *x != id);
        sup.sort_unstable();
        sup.dedup();
        images.retain(|x| *x != id && !sup.contains(x));
        images.sort_unstable();
        images.dedup();
        if self.store.config.server.result_meta {
            self.result_meta = Some(serde_json::json!({
                "zero-use-computer/result": id,
                "zero-use-computer/supersedes": sup,
                "zero-use-computer/supersedes-images": images,
            }));
        }
    }

    /// The tools of a category, or whose name or description has the words
    /// asked for, with their arguments. With [tools] manager =
    /// "list_changed", their categories join the tool list for good.
    fn find_tools(&mut self, args: &serde_json::Value) -> ToolOutput {
        use crate::config::ToolManager;
        /// Tools a query returns at most (a category or names return all).
        const MOST: usize = 3;
        let category = args.get("category").and_then(serde_json::Value::as_str);
        let again = args
            .get("again")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let names: Vec<String> = args
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(|n| {
                n.split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|w| !w.is_empty())
                    .map(|w| w.to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let words = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(crate::tools::query_words)
            .unwrap_or_default();
        let query = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let all = self.all_tool_definitions();
        let hidden = |d: &&ToolDefinition| !crate::tools::BASE_TOOLS.contains(&&*d.name);
        let in_category =
            |d: &&ToolDefinition| category.is_none_or(|c| crate::tools::category_of(&d.name) == c);
        let mut more = Vec::new();
        // How well the best tool matched the words (3: a word in its name).
        let mut best = 0;
        let mut found: Vec<&ToolDefinition> = if !names.is_empty() {
            all.iter()
                .filter(|d| names.contains(&d.name.to_string()))
                .collect()
        } else if words.is_empty() {
            // A category on its own: all of it.
            all.iter()
                .filter(hidden)
                .filter(in_category)
                .filter(|_| category.is_some())
                .collect()
        } else {
            // By what they do: the best few, words in the name counting most.
            let mut scored: Vec<(usize, &ToolDefinition)> = all
                .iter()
                .filter(hidden)
                .filter(in_category)
                .map(|d| {
                    (
                        crate::tools::query_score(&words, &d.name, &d.description),
                        d,
                    )
                })
                .filter(|(score, _)| *score > 0)
                .collect();
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            best = scored.first().map_or(0, |s| s.0);
            more = scored
                .iter()
                .skip(MOST)
                .map(|(_, d)| d.name.to_string())
                .collect();
            scored.into_iter().take(MOST).map(|(_, d)| d).collect()
        };
        // Words that name no tool, or only show up in descriptions: the
        // decision model, when there is one, knows "make a logo" means
        // design. Without one the words decide, as before.
        let mut by_model = false;
        if names.is_empty()
            && let Some(q) = query
            && best < 3
        {
            let cands: Vec<&ToolDefinition> =
                all.iter().filter(hidden).filter(in_category).collect();
            if let Some(picked) = self.tools_by_model(q, &cands) {
                found = picked.into_iter().map(|i| cands[i]).collect();
                more.clear();
                by_model = true;
            }
        }
        if found.is_empty() {
            let cats: Vec<&str> = crate::tools::CATEGORIES.iter().map(|c| c.0).collect();
            return ToolOutput::text(format!("No tools match. Categories: {}.", cats.join(", ")));
        }
        let dispatch = self.store.config.tools.manager == ToolManager::Dispatch;
        let mut out = String::new();
        for d in &found {
            out.push_str(&format!("{}: {}\n", d.name, d.description));
            // With list_changed the arguments come with the tool list.
            if !dispatch {
                continue;
            }
            let schema = d.input_schema.to_string();
            let hash = text_hash(&schema);
            if !again && self.schemas_shown.get(&*d.name) == Some(&hash) {
                out.push_str("  arguments: as shown before (again=true repeats them)\n");
            } else {
                out.push_str(&format!("  arguments: {schema}\n"));
                self.schemas_shown.insert(d.name.to_string(), hash);
            }
        }
        if !more.is_empty() {
            out.push_str(&format!("Also matching: {}.\n", more.join(", ")));
        }
        if by_model {
            out.push_str("(Chosen by the decision model.)\n");
        }
        if dispatch {
            out.push_str("Run one with use_tool(name, arguments).");
        } else {
            // Base tools are listed already: no category joins for them.
            let added: Vec<&str> = found
                .iter()
                .copied()
                .filter(hidden)
                .map(|d| &*d.name)
                .collect();
            for name in &added {
                self.active_tools.insert(crate::tools::category_of(name));
            }
            if added.is_empty() {
                out.push_str("Already in your tools (call them directly).");
            } else {
                out.push_str(&format!(
                    "Added to your tools: {} (call them directly).",
                    added.join(", ")
                ));
            }
        }
        ToolOutput::text(out)
    }

    /// A tool the model hasn't been shown was called with arguments it
    /// doesn't take: its arguments, once, so the next call can be right
    /// without a find_tools first.
    fn schema_for_wrong_call(&mut self, name: &str) -> Option<String> {
        if self.store.config.tools.manager == crate::config::ToolManager::Off
            || self.tool_definitions().iter().any(|d| d.name == name)
        {
            return None;
        }
        let d = self
            .all_tool_definitions()
            .into_iter()
            .find(|d| d.name == name)?;
        let schema = d.input_schema.to_string();
        let hash = text_hash(&schema);
        if self.schemas_shown.get(name) == Some(&hash) {
            return None;
        }
        self.schemas_shown.insert(name.to_string(), hash);
        Some(format!("\n{name} takes: {schema}"))
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
            // Estimated tokens the result costs the model (text + image).
            "tokens": out.estimated_tokens(),
            "summary": out.text.lines().next().unwrap_or("").chars().take(160).collect::<String>(),
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // The log never grows without end: past 10 MB it starts again, and
        // the previous one is kept beside it.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 10 * 1024 * 1024) {
            let mut old = path.clone().into_os_string();
            old.push(".1");
            let _ = std::fs::rename(&path, old);
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
        // What this desktop doesn't allow, said once.
        if let Some(note) = self.backend.session_note()
            && self.hints.first("session-note")
        {
            lines.push(format!("\nNote: {note}"));
        }
        Ok(ToolOutput::text(lines.join("\n")))
    }

    fn launch_app(&mut self, mut args: LaunchAppArgs) -> Result<ToolOutput> {
        // An app to open, not a command line: backends start exactly this
        // program with no arguments, and nothing that could read as an option.
        args.app = args.app.trim().to_string();
        if args.app.is_empty() || args.app.starts_with('-') || args.app.contains(char::is_control) {
            return Err(Error::InvalidArgs(
                "`app` must be an app name, bundle id or executable (no options or arguments)"
                    .into(),
            ));
        }
        // A web address: open it in the default browser.
        if crate::launch::is_url(&args.app) {
            crate::launch::open_url(&args.app)?;
            return Ok(ToolOutput::text(format!(
                "Opened {} in the default browser. Call list_apps, then get_app_state on the browser, to see it.",
                args.app
            )));
        }
        let before: HashSet<u32> = self.find_apps()?.iter().map(|a| a.pid).collect();
        let program = self.backend.launch_app(&args.app)?;

        let deadline = (self.clock)()
            + Duration::from_secs_f64(self.store.config.launch_timeout_secs.max(0.5));
        // The app shows up under the name asked for, or (when the name was
        // looked up in the OS's app list) under the program that was run.
        let mut names = vec![args.app.to_lowercase()];
        names.extend(program.as_deref().map(crate::launch::program_key));
        let matches = |a: &AppInfo| {
            a.match_keys()
                .iter()
                .any(|k| names.iter().any(|n| k.contains(n.as_str())))
        };
        loop {
            let apps = self.find_apps()?;
            // Prefer a newly-appeared app that matches the query.
            let found = apps
                .iter()
                .find(|a| !before.contains(&a.pid) && matches(a))
                .or_else(|| apps.iter().find(|a| matches(a)));
            if let Some(app) = found {
                let app = app.clone();
                let launched = format!("Launched {} (id: {}, pid: {}).", app.name, app.id, app.pid);
                // Its first state, saving the get_app_state that follows.
                if self.store.config.tools.launch_look
                    && let Some(state) = self.first_look(&app, deadline)
                {
                    return Ok(ToolOutput {
                        text: format!("{launched}\n\n{}", state.text),
                        ..state
                    });
                }
                return Ok(ToolOutput::text(format!(
                    "{launched} Call get_app_state to see it."
                )));
            }
            if self.halted() {
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

    /// A just-launched app's state, once it shows a window (by `deadline`).
    fn first_look(&mut self, app: &AppInfo, deadline: Instant) -> Option<ToolOutput> {
        loop {
            if self.list_windows(app, true).is_ok_and(|w| !w.is_empty()) {
                // Give it a moment to fill the window.
                self.settle_on(app);
                return self
                    .get_app_state(GetAppStateArgs {
                        app: app.pid.to_string(),
                        ..Default::default()
                    })
                    .ok();
            }
            if self.halted() || (self.clock)() >= deadline {
                return None;
            }
            (self.sleep)(Duration::from_millis(200));
        }
    }

    fn get_app_state(&mut self, mut args: GetAppStateArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        // [cache] rebase_after_tokens: much said since this app's tree was
        // last sent whole, so the model may have lost what a diff refers to.
        let threshold = self.store.config.cache.rebase_after_tokens;
        let rebase = threshold > 0
            && !args.disable_diff
            && args.within.is_none()
            && self.states.get(&app.pid).is_some_and(|s| {
                s.stamped
                    && s.known.is_some()
                    && self.sent_tokens.saturating_sub(s.full_at) >= threshold
            });
        if rebase {
            args.disable_diff = true;
            args.screenshot.get_or_insert(true);
        }
        let window = self.resolve_window(&app, args.window.as_deref(), false)?;
        self.force_ocr = args.ocr;
        let observed = self.observe(&app, &window, args.ocr);
        self.force_ocr = false;
        observed?;
        if let Some(index) = args.within {
            return self.part_of_tree(&app, &window, index, args.max_tokens);
        }
        if let Some(about) = args.about.as_deref().filter(|a| !a.trim().is_empty()) {
            return self.about_parts(&app, &window, about, args.max_tokens);
        }
        let mut r = self.render(app.pid, args.disable_diff, args.max_tokens)?;
        if self.depth == 1 {
            if r.full {
                self.note.looks_full.push(app.pid);
            } else {
                self.note.looks_diff.push(app.pid);
            }
        }
        if r.full {
            let sent = self.sent_tokens;
            if let Some(st) = self.states.get_mut(&app.pid) {
                st.full_at = sent;
            }
        }
        // The action before this one showed the start of this very tree:
        // only the rest is sent.
        if let Some((pid, screen, hash, shown)) = self.partial_report.take()
            && !args.disable_diff
            && self.depth == 1
            && pid == app.pid
            && screen == r.screen
            && hash == text_hash(&r.text)
        {
            let rest: Vec<&str> = r.text.lines().skip(shown).collect();
            let intro = self.explain(
                "partial-rest",
                "The first lines of this tree are as the action's result showed them (unchanged since); the rest:",
                "Rest of the tree (the start is as the action's result showed it):",
            );
            r.text = format!("{intro}\n{}\n", rest.join("\n"));
        }
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
        header.push(')');
        // The app and window as the last look gave them: named only.
        let compact = self.store.config.tree.compact;
        if let Some(st) = self.states.get_mut(&app.pid) {
            let same = st.header_seen.as_deref() == Some(header.as_str());
            st.header_seen = Some(header.clone());
            if compact && same {
                header = format!("App: {} · window \"{}\"", app.name, window.title);
            }
        }
        header.push_str(&format!(" · screen #{}", r.screen));
        match r.seen {
            Seen::New => header.push_str(" (new)"),
            Seen::Revisit => header.push_str(" (seen before)"),
            Seen::Same => {}
        }
        if rebase {
            header.push_str(" · sent whole again (much has been said since it last was)");
        }
        let head_len = header.len();
        let (ocr_lines, blind) = self
            .state(app.pid)
            .map(|s| (s.ocr_lines, s.blind.len()))
            .unwrap_or((0, 0));
        if blind > 0 && !args.ocr {
            // Said on a screen's first view (and the first time at all).
            if r.seen != Seen::Same || self.explain_first("blind") {
                header.push_str("\nPart of this window has no accessibility information (a canvas or a picture): ");
                if ocr_lines > 0 {
                    let how = self.explain(
                        "ocr-elements",
                        " (\"ocr text\" elements: click them by element_index; they can't be set or selected)",
                        " (\"ocr text\")",
                    );
                    header.push_str(&format!(
                        "{ocr_lines} line(s) of its text were read off the screen{how}; the screenshot shows the rest."
                    ));
                } else {
                    header.push_str("the screenshot shows it.");
                }
            }
        } else if ocr_lines > 0 {
            // What OCR elements are is said once.
            let how = self.explain(
                "ocr-elements",
                " (\"ocr text\" elements: click them by element_index; they can't be set or selected)",
                " (\"ocr text\")",
            );
            header.push_str(&if args.ocr {
                format!("\nRead {ocr_lines} more line(s) of text off the screen{how}.")
            } else {
                format!(
                    "\nThis window has little accessibility information, so {ocr_lines} line(s) of text were read off the screen{how}."
                )
            });
        }
        if let Some(note) = &self.ocr_note
            && !self.ocr_note_shown
            && (args.ocr || blind > 0 || r.interactive < self.store.config.ocr.sparse_threshold)
        {
            header.push_str(&format!("\n[Text recognition unavailable: {note}]"));
            self.ocr_note_shown = true;
        }

        // Decide whether this view needs pixels (screenshot.attach): a screen
        // the model has no picture of, a big change, a returning screen whose
        // window changed size, or custom-drawn UI with little in the tree.
        // [screenshot] adaptive: no automatic picture of a well-described
        // window in an app where the model hasn't used pixels.
        let (looks, pixel_uses) = self
            .states
            .get_mut(&app.pid)
            .map(|s| {
                s.looks = s.looks.saturating_add(1);
                (s.looks, s.pixel_uses)
            })
            .unwrap_or((1, 0));
        let unneeded = self.store.config.screenshot.adaptive
            && looks > 3
            && pixel_uses == 0
            && r.interactive >= self.store.config.screenshot.auto_sparse_threshold.max(3)
            && blind == 0
            && ocr_lines == 0;
        let shot = &self.store.config.screenshot;
        let allowed = shot.enabled && !self.store.config.text_only;
        let known = self
            .state(app.pid)?
            .known
            .as_ref()
            .ok_or(Error::Internal("no known screen".into()))?;
        let auto = match shot.attach {
            AttachMode::Always => true,
            AttachMode::Never => false,
            AttachMode::Auto => {
                !known.shot
                    || r.large_change
                    || size_changed
                    || r.interactive < shot.auto_sparse_threshold
                    // Something changed, or the app came back to an
                    // earlier screen: look at the pixels rather than
                    // assume the model's picture still fits (an unchanged
                    // picture isn't sent again).
                    || r.changes > 0
                    || r.seen == Seen::Revisit
                    // What changes in an area the tree says nothing
                    // about only the pixels show.
                    || blind > 0
            }
        };
        let want = allowed && args.screenshot.unwrap_or(auto && !unneeded);
        // A picture held back only because the model hasn't needed any here.
        let withheld = allowed && args.screenshot.is_none() && auto && unneeded;
        let force = args.screenshot == Some(true);
        let mut image = None;
        let mut stale: Option<Changed> = None;
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
                    if imaging::uniform(&cap) {
                        header.push_str(
                            "\n[The screenshot is one flat colour: the app may not draw while its window is in the background or minimized. Use the tree, or bring it forward (window action=focus) and look again.]",
                        );
                    }
                    let redacted = self.redact_capture(&mut cap);
                    if redacted > 0 {
                        header.push_str(&format!(
                            "\n[{redacted} private area(s) blacked out of the screenshot]"
                        ));
                    }
                    // Attached on its own to a window the tree already
                    // describes well: an overview is enough, and costs a
                    // fraction of the image tokens.
                    let shot_cfg = &self.store.config.screenshot;
                    let overview = args.screenshot.is_none()
                        && shot_cfg.overview_max_dimension > 0
                        && shot_cfg.overview_max_dimension < shot_cfg.max_dimension
                        && r.interactive >= shot_cfg.auto_sparse_threshold.max(1)
                        && self.state(app.pid).is_ok_and(|s| s.ocr_lines == 0);
                    match self.window_picture(app.pid, r.screen, cap, force, overview) {
                        Ok(Picture::Unchanged { base }) => {
                            let at = base.map(|b| format!(" (#{b})")).unwrap_or_default();
                            header.push_str(&if self.explain_first("shot-unchanged") {
                                format!(
                                    "\nScreenshot: unchanged since you last saw it{at}, not re-sent (screenshot=true forces one)."
                                )
                            } else {
                                format!("\nScreenshot: unchanged{at}, not re-sent.")
                            });
                        }
                        Ok(Picture::Part {
                            img,
                            id,
                            base,
                            at,
                            changed,
                        }) => {
                            stale = changed;
                            if self.depth == 1 {
                                self.note.pictures_part.push(app.pid);
                            }
                            let (w, h, (ox, oy)) = (img.width, img.height, at);
                            let (x1, y1) = (ox + w, oy + h);
                            header.push_str(&match base {
                                Some(b) if self.explain_first("shot-part") => format!(
                                    "\nScreenshot #{id}: only the part that changed, {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of screenshot #{b}, your earlier one of this screen (same scale; the rest is unchanged, and x/y coordinates still refer to that whole screenshot)."
                                ),
                                Some(b) => format!(
                                    "\nScreenshot #{id}: changed part only, the area x {ox}–{x1}, y {oy}–{y1} of #{b} (x/y still refer to #{b})."
                                ),
                                None => format!(
                                    "\nScreenshot #{id}: only the part that changed, {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of your earlier screenshot of this screen (same scale; the rest is unchanged, and x/y coordinates still refer to that whole screenshot)."
                                ),
                            });
                            image = Some(img);
                        }
                        Ok(Picture::Whole {
                            img,
                            id,
                            overview,
                            changed,
                        }) => {
                            stale = changed;
                            if self.depth == 1 {
                                self.note.pictures_whole.push(app.pid);
                            }
                            header.push_str(&format!(
                                "\nScreenshot #{id}: {}x{} px.",
                                img.width, img.height
                            ));
                            if overview {
                                header.push_str(self.explain(
                                    "overview",
                                    " (An overview; pass screenshot=true for full detail, or screenshot(element_index) to zoom into one element.)",
                                    " (overview)",
                                ));
                            }
                            image = Some(img);
                        }
                        Err(e) => {
                            self.mark_shot(app.pid);
                            header.push_str(&format!("\n[screenshot encode failed: {e}]"));
                        }
                    }
                }
                Err(e) => {
                    self.mark_shot(app.pid);
                    header.push_str(&format!("\n[screenshot unavailable: {e}]"));
                }
            }
        } else if allowed {
            header.push_str(if withheld && self.explain_first("shot-unneeded") {
                "\nScreenshot: not attached (you haven't needed pictures in this app; screenshot=true for one)."
            } else {
                self.explain(
                    "shot-none",
                    "\nScreenshot: not attached (pass screenshot=true for one).",
                    "\nScreenshot: not attached.",
                )
            });
        }

        // [screenshot] icon_sprite: no screenshot, but buttons without a
        // name: a strip of what they look like.
        if image.is_none()
            && allowed
            && self.store.config.screenshot.icon_sprite
            && let Some((img, listed)) = self.icon_strip(&app, &window)
        {
            header.push_str(&format!(
                "\nIcons of buttons without a name, numbered with their element_index: {listed}."
            ));
            image = Some(img);
        }
        // The pixels changed where the tree reports nothing (a toolkit that
        // doesn't tell accessibility what it redrew): say where.
        if let Some(c) = stale
            && r.seen == Seen::Same
            && !r.full
            && r.removed == 0
            && blind == 0
            && c.share >= 0.1
            && !r.touched.iter().any(|t| t.intersects(&c.screen))
        {
            let (x, y, w, h) = c.shot;
            header.push_str(&format!(
                "\n[The picture changed at x {x}–{}, y {y}–{} where the tree reports no change: the tree may be out of date there; trust the screenshot.]",
                x + w,
                y + h
            ));
        }
        // Nothing new at all: one line says so.
        let quiet = compact
            && r.seen == Seen::Same
            && r.changes == 0
            && !r.full
            && image.is_none()
            && header[head_len..]
                .lines()
                .filter(|l| !l.is_empty())
                .all(|l| {
                    l.starts_with("Screenshot: unchanged")
                        || l.starts_with("Screenshot: not attached")
                });
        if quiet {
            let mut text = format!(
                "{}: nothing changed since your last look (tree and screenshot)",
                &header[..head_len]
            );
            if !r.restless.is_empty() {
                text.push_str(&format!(
                    ", but for {} element(s) that keep changing on their own: {}",
                    r.restless.len(),
                    tree::ranges(&mut r.restless.clone())
                ));
            }
            text.push('.');
            return Ok(ToolOutput::text(text));
        }
        Ok(ToolOutput {
            text: format!("{header}\nTree:\n{}", r.text),
            image,
            is_error: false,
        })
    }

    /// A strip of the buttons in the latest snapshot that have no name and
    /// haven't been shown yet, each numbered with its index.
    fn icon_strip(&mut self, app: &AppInfo, window: &WindowInfo) -> Option<(EncodedImage, String)> {
        let st = self.states.get(&app.pid)?;
        let icons: Vec<(u32, Rect, u64)> = st
            .nodes
            .iter()
            .filter(|n| {
                crate::roles::is_interactive(&n.role)
                    && n.name.is_none()
                    && !n.line.contains(" desc=")
                    && !st.icons_shown.contains(&n.key)
            })
            .filter_map(|n| {
                let b = n.bounds?;
                (b.width >= 4.0 && b.height >= 4.0 && b.width <= 96.0 && b.height <= 96.0)
                    .then_some((n.index, b, n.key))
            })
            .take(24)
            .collect();
        if icons.is_empty() {
            return None;
        }
        let reuse = match self.last_capture.take() {
            Some((pid, wid, epoch, cap))
                if pid == app.pid && wid == window.id && epoch == self.epoch =>
            {
                Some(cap)
            }
            _ => None,
        };
        let mut cap = match reuse {
            Some(c) => c,
            None => self.capture_clean(|b| b.capture(app, window)).ok()?,
        };
        self.redact_capture(&mut cap);
        let crops: Vec<(u32, Capture)> = icons
            .iter()
            .filter_map(|(i, b, _)| {
                crate::coverage::pixels_of(&cap, *b).map(|px| (*i, imaging::crop(&cap, px)))
            })
            .collect();
        if crops.is_empty() {
            return None;
        }
        let strip = imaging::strip(&crops, 28);
        let (img, _) = imaging::encode(strip, &self.store.config.screenshot).ok()?;
        let mut listed: Vec<u32> = crops.iter().map(|(i, _)| *i).collect();
        if let Some(st) = self.states.get_mut(&app.pid) {
            st.icons_shown.extend(icons.iter().map(|(_, _, k)| *k));
        }
        Some((img, tree::ranges(&mut listed)))
    }

    /// One element and what is in it (get_app_state within=index): a look
    /// that leaves what later diffs are against as it was.
    fn part_of_tree(
        &self,
        app: &AppInfo,
        window: &WindowInfo,
        index: u32,
        max_tokens: Option<usize>,
    ) -> Result<ToolOutput> {
        let st = self.state(app.pid)?;
        let pos = st
            .nodes
            .iter()
            .position(|n| n.index == index)
            .ok_or_else(|| {
                Error::InvalidArgs(format!(
                    "unknown element_index {index}: call get_app_state for the current indices"
                ))
            })?;
        let base = st.nodes[pos].depth;
        let end = pos
            + 1
            + st.nodes[pos + 1..]
                .iter()
                .take_while(|n| n.depth > base)
                .count();
        let part: Vec<Node> = st.nodes[pos..end]
            .iter()
            .map(|n| {
                let mut n = n.clone();
                n.depth -= base;
                n.parent = n.parent.filter(|p| *p >= pos).map(|p| p - pos);
                n
            })
            .collect();
        let tcfg = &self.store.config.tree;
        let mut budget = tree::Budget::from_config(tcfg);
        if let Some(tokens) = max_tokens {
            budget.tokens = tokens;
        }
        let text = tree::render_full_within(&part, tcfg.indent, budget);
        Ok(ToolOutput::text(format!(
            "App: {} · window \"{}\" · element {index} and what is in it ({} element(s)):\n{text}",
            app.name,
            window.title,
            part.len()
        )))
    }

    /// What the model gets of a window's pixels, given what it already has
    /// of `screen`: nothing when that picture is current, only the part
    /// that changed when it is small, else the whole window, which then is
    /// the picture x/y refer to. get_app_state and screenshot(app) share it.
    /// `force` (asked for explicitly): always the whole window.
    fn window_picture(
        &mut self,
        pid: u32,
        screen: u32,
        cap: Capture,
        force: bool,
        overview: bool,
    ) -> Result<Picture> {
        let cache = self.store.config.cache.clone();
        let mut shot_cfg = self.store.config.screenshot.clone();
        let (known_pixels, known_coord, base) = self
            .states
            .get(&pid)
            .and_then(|s| s.known.as_ref())
            .filter(|k| k.id == screen)
            .map(|k| (k.pixels.clone(), k.coord, k.shot_id))
            .unwrap_or_default();
        // Pixel fingerprints tell unchanged pictures and changed parts apart.
        let fingerprint =
            cache.dedupe_screenshots || shot_cfg.scope == crate::config::ShotScope::Auto;
        let sig = fingerprint.then(|| PixelSig::of(&cap, cache.pixel_grid));
        let unchanged = !force
            && cache.dedupe_screenshots
            && known_coord.is_some_and(|c| c.bounds == cap.bounds)
            && matches!((&sig, &known_pixels), (Some(a), Some(b)) if a.same_as(b, cache.pixel_tolerance));
        // Where it changed, in screen and screenshot terms.
        let changed = match (&sig, &known_pixels, known_coord) {
            (Some(a), Some(b), Some(c)) if c.bounds == cap.bounds => a
                .changed_area(b, cache.pixel_tolerance)
                .map(|(x, y, w, h)| {
                    let sx = cap.bounds.width / f64::from(cap.width.max(1));
                    let sy = cap.bounds.height / f64::from(cap.height.max(1));
                    let screen = Rect::new(
                        cap.bounds.x + f64::from(x) * sx,
                        cap.bounds.y + f64::from(y) * sy,
                        f64::from(w) * sx,
                        f64::from(h) * sy,
                    );
                    let kx = f64::from(c.width) / f64::from(cap.width.max(1));
                    let ky = f64::from(c.height) / f64::from(cap.height.max(1));
                    let shot = (
                        (f64::from(x) * kx) as u32,
                        (f64::from(y) * ky) as u32,
                        (f64::from(w) * kx).ceil() as u32,
                        (f64::from(h) * ky).ceil() as u32,
                    );
                    let share = f64::from(w) * f64::from(h)
                        / (f64::from(cap.width) * f64::from(cap.height)).max(1.0);
                    Changed {
                        screen,
                        shot,
                        share,
                    }
                }),
            _ => None,
        };
        if unchanged {
            // The model already has this picture.
            if let Some(st) = self.states.get_mut(&pid) {
                st.coord = known_coord;
            }
            return Ok(Picture::Unchanged { base });
        }
        if !force
            && let Some(part) =
                self.changed_part(&cap, sig.as_ref(), known_pixels.as_ref(), known_coord)
            && let Some(full) = known_coord
            && let Ok((img, at)) = imaging::encode_part(&cap, part, &full, &shot_cfg)
        {
            // Only the part that changed, placed in the picture the model
            // already has.
            let id = self.next_shot();
            self.pending_images.push(PendingImage {
                pid,
                screen,
                coord: full,
                pixels: sig,
                id: base.unwrap_or(id),
            });
            return Ok(Picture::Part {
                img,
                id,
                base,
                at,
                changed,
            });
        }
        if overview {
            shot_cfg.max_dimension = shot_cfg.overview_max_dimension;
        }
        let (img, map) = imaging::encode(cap, &shot_cfg)?;
        let id = self.next_shot();
        self.pending_images.push(PendingImage {
            pid,
            screen,
            coord: map,
            pixels: sig,
            id,
        });
        Ok(Picture::Whole {
            img,
            id,
            overview,
            changed,
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

    /// The next screenshot's number.
    fn next_shot(&mut self) -> u32 {
        self.shots += 1;
        self.shots
    }

    /// A screenshot of the known screen was attempted and failed: don't
    /// retry it on every view.
    fn mark_shot(&mut self, pid: u32) {
        if let Some(k) = self.states.get_mut(&pid).and_then(|s| s.known.as_mut()) {
            k.shot = true;
        }
    }

    /// Actions act on the window of the latest `get_app_state` (their
    /// element indices and x/y belong to it). A `window` argument naming
    /// another window is an error, not silently ignored.
    fn check_window(&mut self, app: &AppInfo, window: Option<&str>) -> Result<()> {
        let Some(w) = window.map(str::trim).filter(|w| !w.is_empty()) else {
            return Ok(());
        };
        let wanted = self.resolve_window(app, Some(w), false)?;
        match self.states.get(&app.pid).and_then(|s| s.window_id) {
            Some(id) if id != wanted.id => Err(Error::InvalidArgs(format!(
                "the latest get_app_state of {} shows another window; call get_app_state with window=\"{w}\" first, then act on it.",
                app.name
            ))),
            _ => Ok(()),
        }
    }

    fn click(&mut self, args: ClickArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let count = args.click_count.clamp(1, 3);
        let (x, y, snapped) = match &args.snap {
            None => (args.x, args.y, String::new()),
            Some(how) => {
                let (Some(x), Some(y)) = (args.x, args.y) else {
                    return Err(Error::InvalidArgs(
                        "snap moves an x/y point: give x and y".into(),
                    ));
                };
                let (nx, ny, note) =
                    self.snap_xy(&app, args.window.as_deref(), x, y, how, args.snap_radius)?;
                (Some(nx), Some(ny), note)
            }
        };
        // By name: the one element that has it.
        let mut named = String::new();
        let index = match args.element_index {
            None if x.is_none() && (args.name.is_some() || args.role.is_some()) => {
                let i = self.element_named(
                    &app,
                    args.window.as_deref(),
                    args.name.as_deref(),
                    args.role.as_deref(),
                )?;
                named = format!(" ({i})");
                Some(i)
            }
            i => i,
        };
        let anchor = self.anchor(&app, index, x, y, "click")?;
        // What things looked like, to tell whether the click did anything.
        let before = self.tree_fingerprint(app.pid);
        let what = format!("{}{named}", self.describe_anchor(&app, &anchor));
        let point = self.anchor_point(&app, &anchor).ok();

        // The agent cursor goes there first, so the user sees what is next.
        self.overlay_anchor(&app, &anchor, false);
        self.overlay_anchor(&app, &anchor, true);

        // A single left click on an element with a press action goes through
        // the accessibility API so it works in the background.
        let mut note = String::new();
        if let (Anchor::Element(h), MouseButton::Left, 1) = (&anchor, args.button, count) {
            let node = self.node_for_handle(&app, *h);
            // A table cell's press may not select its row (GTK): clicking it
            // again with the mouse only selects it, so that is safe.
            let cell = node.is_some_and(|n| crate::roles::is_cell(&n.role));
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
                            && (v.retry_on_no_change || cell)
                            && let Some(p) = point
                        {
                            // Nothing happened: click it with the mouse.
                            let target = self.input_target(&app)?;
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
                    // Sent, but unanswered: clicking again could press it twice.
                    Err(e @ Error::Unanswered(_)) => {
                        self.settle_on(&app);
                        return Ok(ToolOutput::text(format!("Pressed {what}, but {e}")));
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
        let target = self.input_target(&app)?;
        self.backend.click(&target, point, args.button, count)?;
        self.settle_on(&app);
        let verb = match (args.button, count) {
            (MouseButton::Right, _) => "Right-clicked",
            (_, 2) => "Double-clicked",
            (_, 3) => "Triple-clicked",
            _ => "Clicked",
        };
        let mut msg = format!(
            "{verb} {what} at ({:.0}, {:.0}).{note}{snapped}",
            point.x, point.y
        );
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    fn perform_secondary(&mut self, args: SecondaryActionArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
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
        self.overlay_point_element(&app, handle, true);
        match self.backend.perform_action(handle, &native) {
            Ok(()) => {}
            Err(e @ Error::Unanswered(_)) => {
                self.settle_on(&app);
                return Ok(ToolOutput::text(format!(
                    "Performed `{}` on {}, but {e}",
                    args.action,
                    node.label()
                )));
            }
            Err(e) => return Err(e),
        }
        self.settle_on(&app);
        let mut msg = format!("Performed `{}` on {}.", args.action, node.label());
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    fn set_value(&mut self, args: SetValueArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let handle = self.element_by_index(&app, args.element_index)?;
        let node = self.node_by_index(&app, args.element_index)?.clone();
        ocr_can_only_be_clicked(handle, "set_value")?;
        self.overlay_point_element(&app, handle, true);
        // Text fields can be typed into when setting fails.
        let typable = node.states.editable && node.states.checked.is_none();
        let retry = self.store.config.verify.retry && typable;
        let mut how = String::new();
        if let Err(e) = self.backend.set_value(handle, &args.value) {
            // It may have been set: typing it as well could enter it twice,
            // into whatever the busy app shows next.
            if let Error::Unanswered(_) = e {
                self.settle_on(&app);
                return Ok(ToolOutput::text(format!(
                    "Sent the new value to {}, but {e}",
                    node.label()
                )));
            }
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
        // The field shows it: the value is the one just sent, not news.
        if self.store.config.tree.compact
            && how.is_empty()
            && self.verified()
            && self.value_took(&app, args.element_index, &args.value) == Some(true)
        {
            msg = format!("Set {}; it shows the new value.", node.label());
        }
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

    fn select_text(&mut self, args: SelectTextArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
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

    fn scroll(&mut self, args: ScrollArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        // A wheel turn per line: an amount in the millions would keep the
        // backend busy for hours.
        let pages = if args.amount.is_finite() && args.amount > 0.0 {
            args.amount.min(MAX_SCROLL_PAGES)
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
                    let target = self.input_target(&app)?;
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
        let target = self.input_target(&app)?;
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

    fn drag(&mut self, args: DragArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let (mut fx, mut fy, mut tx, mut ty) = (args.from_x, args.from_y, args.to_x, args.to_y);
        let mut snapped = String::new();
        if let Some(how) = &args.snap {
            let mut any = false;
            for (x, y, end) in [(&mut fx, &mut fy, "start"), (&mut tx, &mut ty, "end")] {
                if let (Some(px), Some(py)) = (*x, *y) {
                    let (nx, ny, note) =
                        self.snap_xy(&app, args.window.as_deref(), px, py, how, args.snap_radius)?;
                    (*x, *y) = (Some(nx), Some(ny));
                    snapped.push_str(&note.replace(" Snapped", &format!(" The {end} snapped")));
                    any = true;
                }
            }
            if !any {
                return Err(Error::InvalidArgs(
                    "snap moves x/y points: give from_x/from_y or to_x/to_y".into(),
                ));
            }
        }
        let from = self.anchor(&app, args.from_element_index, fx, fy, "drag source")?;
        let to = self.anchor(&app, args.to_element_index, tx, ty, "drag target")?;
        let (p0, p1) = (
            self.anchor_point(&app, &from)?,
            self.anchor_point(&app, &to)?,
        );
        let before = self.tree_fingerprint(app.pid);
        self.overlay_point(p0, true);
        self.overlay_point(p1, false);
        let target = self.input_target(&app)?;
        self.backend.drag(&target, p0, p1)?;
        self.settle_on(&app);
        let mut msg = format!(
            "Dragged from ({:.0}, {:.0}) to ({:.0}, {:.0}).{snapped}",
            p0.x, p0.y, p1.x, p1.y
        );
        if self.verified() && self.tree_fingerprint(app.pid) == before {
            msg.push_str(NO_CHANGE_NOTE);
        }
        Ok(ToolOutput::text(msg))
    }

    /// The coordinates a drawing (or a canvas-labelled screenshot) uses:
    /// the latest screenshot's pixels, an element's box (fractions), or a
    /// document on screen in its own units or as a math range.
    fn draw_frame(
        &self,
        app: &AppInfo,
        element_index: Option<u32>,
        canvas: Option<&DrawCanvas>,
    ) -> Result<(crate::draw::Frame, &'static str)> {
        use crate::draw::Frame;
        let map = self.state(app.pid).ok().and_then(|s| s.coord);
        let no_map = || {
            Error::InvalidArgs(
                "no current screenshot to draw on: call get_app_state first, or pass element_index"
                    .into(),
            )
        };
        // The element's box (screen coordinates).
        let element = match element_index {
            Some(i) => {
                let h = self.handle_of(app, i)?;
                let b = self
                    .state(app.pid)
                    .ok()
                    .and_then(|s| s.bounds.get(&h))
                    .copied()
                    .filter(|b| !b.is_empty())
                    .ok_or_else(|| {
                        Error::ActionFailed("this element has no on-screen box to draw in".into())
                    })?;
                Some(b)
            }
            None => None,
        };
        let Some(c) = canvas else {
            return Ok(match element {
                Some(b) => (Frame::fractions(b), "the element"),
                None => {
                    let map = map.ok_or_else(no_map)?;
                    (
                        Frame::pixels(map.bounds, map.width, map.height),
                        "the screenshot",
                    )
                }
            });
        };
        let b = match (c.area, element) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidArgs(
                    "give canvas.box or element_index, not both".into(),
                ));
            }
            (Some([l, t, r, btm]), None) => {
                let map = map.ok_or_else(no_map)?;
                let (a, z) = (map.to_screen(l, t)?, map.to_screen(r, btm)?);
                let b = Rect::new(a.x, a.y, z.x - a.x, z.y - a.y);
                if b.is_empty() {
                    return Err(Error::InvalidArgs(
                        "canvas.box is [left, top, right, bottom], right of left and below top"
                            .into(),
                    ));
                }
                b
            }
            (None, Some(b)) => b,
            (None, None) => {
                return Err(Error::InvalidArgs(
                    "canvas needs box (where the document is in the screenshot) or element_index"
                        .into(),
                ));
            }
        };
        match (c.size, c.range) {
            (Some(_), Some(_)) => Err(Error::InvalidArgs(
                "canvas takes size (document units) or range (math), not both".into(),
            )),
            (Some([w, h]), None) => {
                if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
                    return Err(Error::InvalidArgs(
                        "canvas.size is [width, height], both positive".into(),
                    ));
                }
                Ok((Frame::units(b, w, h), "the document"))
            }
            (None, Some([x0, x1, y0, y1])) => {
                if ![x0, x1, y0, y1].iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
                    return Err(Error::InvalidArgs(
                        "canvas.range is [x min, x max, y min, y max], each max above its min"
                            .into(),
                    ));
                }
                Ok((Frame::range(b, x0, x1, y0, y1), "the plot"))
            }
            (None, None) => Err(Error::InvalidArgs(
                "canvas needs size [width, height] or range [x min, x max, y min, y max]".into(),
            )),
        }
    }

    fn draw(&mut self, args: DrawArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        if args.strokes.is_empty() || args.strokes.len() > DRAW_MAX_STROKES {
            return Err(Error::InvalidArgs(format!(
                "`strokes` needs 1 to {DRAW_MAX_STROKES} strokes"
            )));
        }
        let (frame, area) = self.draw_frame(&app, args.element_index, args.canvas.as_ref())?;
        // A cell size the model chose is checked before anything is drawn.
        frame_cells(&frame, None, args.cell_size)?;
        let mut shapes: Vec<(String, crate::draw::Shape)> = Vec::new();
        // Per shape: painted solid with a brush this wide on screen, and
        // how far past its edge (traces go about one of their pixels past,
        // so no gaps show between neighbours).
        let mut fills: Vec<Option<(f64, f64)>> = Vec::new();
        let unit = {
            let (sx, sy) = frame.scale();
            sx.abs().min(sy.abs())
        };
        for (i, s) in args.strokes.iter().enumerate() {
            let bad = |e: String| Error::InvalidArgs(format!("stroke {}: {e}", i + 1));
            // Shapes, and whether they must be painted solid (a trace or
            // a design's solid step), may not be (a design's line step), or
            // either.
            let (made, solid) = match (&s.trace, &s.design) {
                (Some(_), Some(_)) => return Err(bad("give trace or design, not both".into())),
                (Some(name), None) => (
                    self.trace_shapes(s, name, &frame, area).map_err(bad)?,
                    Some(true),
                ),
                (None, Some(name)) => {
                    let (shapes, solid) = self.design_shapes(s, name, &frame, area).map_err(bad)?;
                    (shapes, Some(solid))
                }
                (None, None) => (draw_shapes(s, &frame).map_err(bad)?, None),
            };
            let what = if s.trace.is_some() {
                "a trace step"
            } else {
                "this design step"
            };
            let fill = match (s.fill, solid) {
                (_, Some(false)) => None,
                (None, Some(true)) => {
                    return Err(bad(format!(
                        "{what} is painted solid: give fill, the brush width"
                    )));
                }
                (None, None) => None,
                (Some(w), _) if w.is_finite() && w > 0.0 => {
                    if w * unit < 2.0 {
                        return Err(bad(format!(
                            "fill {w} is under 2 pixels on screen; use a wider brush"
                        )));
                    }
                    let bleed = match &s.trace {
                        Some(name) => self.trace_pixel(name, &frame),
                        None => 0.0,
                    };
                    Some((w * unit, bleed))
                }
                (Some(_), _) => {
                    return Err(bad("fill is the brush width, a positive number".into()));
                }
            };
            let copies = made.len();
            for (k, shape) in made.into_iter().enumerate() {
                let name = if copies == 1 {
                    format!("stroke {}", i + 1)
                } else {
                    format!("stroke {} (part {})", i + 1, k + 1)
                };
                shapes.push((name, shape));
                fills.push(fill);
            }
            if shapes.len() > DRAW_MAX_STROKES {
                return Err(Error::InvalidArgs(format!(
                    "more than {DRAW_MAX_STROKES} strokes once repeated; draw it in parts"
                )));
            }
        }
        let plan = crate::draw::plan_labelled(&shapes, &frame, DRAW_STEP, DRAW_MAX_POINTS)
            .map_err(Error::InvalidArgs)?;
        let names: Vec<String> = shapes.iter().map(|s| s.0.clone()).collect();
        // Solid shapes narrower than the brush come out bigger.
        let thin: Vec<f64> = plan
            .strokes
            .iter()
            .zip(&plan.shape)
            .filter_map(|(s, &k)| {
                let (w, _) = fills.get(k).copied().flatten()?;
                let width = crate::draw::shape_width(s);
                (width < w).then_some(width)
            })
            .collect();
        let plan = crate::draw::fill_plan(plan, &fills, &names, DRAW_STEP, DRAW_MAX_POINTS)
            .map_err(|e| {
                if e.contains("pointer positions") {
                    Error::InvalidArgs(format!("{e}, or paint with a wider brush (fill)"))
                } else {
                    Error::InvalidArgs(e)
                }
            })?;
        let speed = args
            .speed
            .filter(|s| s.is_finite())
            .unwrap_or(DRAW_SPEED)
            .clamp(50.0, 5000.0);
        let secs = plan.length / speed;
        if secs > DRAW_MAX_SECS {
            return Err(Error::InvalidArgs(format!(
                "this drawing would take about {secs:.0} s at {speed:.0} pixels per second; raise speed or draw it in parts"
            )));
        }
        let (Some(first), Some(last)) = (
            plan.strokes.first().and_then(|s| s.first()).copied(),
            plan.strokes.last().and_then(|s| s.last()).copied(),
        ) else {
            return Err(Error::InvalidArgs("nothing to draw".into()));
        };
        let mut summary = self.draw_summary(&frame, area, &plan, args.cell_size);
        if let Some(w) = fills.iter().flatten().map(|f| f.0).min_by(f64::total_cmp) {
            summary.push_str(&format!(
                " Solid shapes are painted for a brush {w:.0} px wide on screen; a smaller brush leaves stripes."
            ));
        }
        if let Some(narrowest) = thin.iter().copied().min_by(f64::total_cmp) {
            summary.push_str(&format!(
                " {} of the solid shapes are narrower than the brush and come out bigger (the narrowest is about {:.0} wide): for fine detail use a smaller brush.",
                thin.len(),
                narrowest / unit.max(1e-9)
            ));
        }
        if args.preview {
            let cap = self.window_capture(&app, args.window.as_deref())?;
            let fill_note = self.fill_report(&app, cap.clone(), &plan, &shapes, &fills, true);
            return self.draw_preview(
                cap,
                &frame,
                &plan,
                args.cell_size,
                format!("{summary}{fill_note}"),
            );
        }

        self.overlay_point(first, true);
        let target = self.input_target(&app)?;
        // Paced to `speed`, and stoppable between any two moves: the stop
        // key ends the drawing (the backend lets go of the button).
        let (stop, cancel) = (self.stop.clone(), self.cancel.clone());
        let stop_name = self.stop_control_name();
        let sleep = &self.sleep;
        let mut owed = 0.0f64;
        let mut pace = |d: f64| -> Result<()> {
            if stop.load(Ordering::SeqCst) {
                return Err(Error::Stopped(stop_name.clone()));
            }
            if cancel.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            owed += d / speed;
            if owed >= 0.004 {
                sleep(Duration::from_secs_f64(owed.min(0.25)));
                owed = 0.0;
            }
            Ok(())
        };
        let drawn = self
            .backend
            .draw(&target, &plan.strokes, args.button, &mut pace);
        self.last_input = Some((self.clock)());
        self.overlay_point(last, false);
        drawn?;
        self.settle_on(&app);

        let cap = self
            .window_capture(&app, args.window.as_deref())
            .ok()
            .flatten();
        let fill_note = self.fill_report(&app, cap, &plan, &shapes, &fills, false);
        let mut msg = format!("Drew {summary}{fill_note}");
        msg.push_str(self.explain(
            "draw-check",
            " Drawing changes pixels, which the accessibility tree doesn't show: check the result with a screenshot (screenshot: true).",
            " Check it with a screenshot.",
        ));
        Ok(ToolOutput::text(msg))
    }

    /// "3 strokes (…) on the plot: x … to …, y … to …." plus what fell
    /// outside.
    fn draw_summary(
        &self,
        frame: &crate::draw::Frame,
        area: &str,
        plan: &crate::draw::Plan,
        cell_size: Option<f64>,
    ) -> String {
        // Where it goes, in the coordinates the model used.
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in plan.strokes.iter().flatten() {
            let (x, y) = frame.to_frame(*p);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let digits = match area {
            "the element" | "the plot" => 2,
            _ => 0,
        };
        let n = plan.strokes.len();
        let mut msg = format!(
            "{n} stroke{} ({} pointer positions, {:.0} px of line) on {area}: x {x0:.digits$} to {x1:.digits$}, y {y0:.digits$} to {y1:.digits$}.",
            if n == 1 { "" } else { "s" },
            plan.points(),
            plan.length,
        );
        if let Some(t) = plan_span(frame, plan) {
            let (cells, _) = drawing_cells(frame, Some(t), cell_size);
            msg.push_str(&format!(
                " It covers cells {} (cells of {}, A1 top-left).",
                cells.covering(t),
                imaging::grid_label(cells.step, cells.step)
            ));
        }
        if plan.skipped > 0 {
            msg.push_str(&format!(
                " {} sample(s) of a curve were outside {area} and were not drawn.",
                plan.skipped
            ));
        }
        // Circles come out as ellipses when a unit of x and of y differ on
        // screen: say so, with the numbers to fix it.
        let (sx, sy) = frame.scale();
        let (ux, uy) = (sx.abs(), sy.abs());
        if area != "the screenshot" && ux > 0.0 && uy > 0.0 && (ux / uy - 1.0).abs() > 0.02 {
            msg.push_str(&format!(
                " On screen 1 unit of x is {ux:.1} px and 1 unit of y is {uy:.1} px, so circles look like ellipses; for equal units make the canvas's width/height match its box's."
            ));
        }
        msg
    }

    /// A design on the design board, by name.
    fn design_named(&self, name: &str) -> std::result::Result<&crate::design::Design, String> {
        let key = design_key(name);
        if let Some((_, d)) = self.designs.iter().rev().find(|(n, _)| *n == key) {
            return Ok(d);
        }
        let known: Vec<&str> = self.designs.iter().map(|(n, _)| n.as_str()).collect();
        Err(if known.is_empty() {
            format!("no design called \"{name}\": make it with the design tool first")
        } else {
            format!(
                "no design called \"{name}\" (designs: {})",
                known.join(", ")
            )
        })
    }

    /// The shapes of one step of a design, fitted into the drawing area
    /// with its proportions kept, and whether they are painted solid.
    fn design_shapes(
        &mut self,
        s: &DrawStroke,
        name: &str,
        frame: &crate::draw::Frame,
        area: &str,
    ) -> std::result::Result<(Vec<crate::draw::Shape>, bool), String> {
        use crate::design::StepKind;
        if shape_given(s) {
            return Err(
                "give design on its own (with step, and fill for a solid step), not with a shape"
                    .into(),
            );
        }
        if s.rotate.is_some() || s.repeat.is_some() {
            return Err("a design step can't be turned or repeated: change the design".into());
        }
        if area == "the screenshot" {
            return Err("a design needs canvas or element_index: where it goes".into());
        }
        let d = self.design_named(name)?.clone();
        let steps = d.steps();
        let n = steps.len();
        let step = match s.step {
            Some(k) if k >= 1 && k as usize <= n => &steps[k as usize - 1],
            _ => return Err(format!("\"{name}\" has steps 1 to {n}: give step")),
        };
        if step.kind == StepKind::Text {
            let l = &d.layers[step.layers[0]];
            let (at, size) = l
                .text
                .as_ref()
                .map_or(((0.0, 0.0), 0.0), |t| (t.at, t.size));
            return Err(format!(
                "step {} is the text of {}: type it with the app's text tool at ({:.0}, {:.0}) in the design's units, size {:.0}",
                s.step.unwrap_or(0),
                l.id,
                at.0,
                at.1,
                size
            ));
        }
        let place = crate::paint::fit_in(
            frame.screen_rect(),
            d.width.round().max(1.0) as u32,
            d.height.round().max(1.0) as u32,
        );
        let (sx, sy) = (place.width / d.width, place.height / d.height);
        let (xa, xb) = (frame.x0.min(frame.x1), frame.x0.max(frame.x1));
        let (ya, yb) = (frame.y0.min(frame.y1), frame.y0.max(frame.y1));
        let lines = d.step_lines(step)?;
        let shapes = lines
            .into_iter()
            .map(|(pts, closed)| {
                let pts = pts
                    .iter()
                    .map(|&(x, y)| {
                        let (fx, fy) =
                            frame.to_frame(Point::new(place.x + x * sx, place.y + y * sy));
                        (fx.clamp(xa, xb), fy.clamp(ya, yb))
                    })
                    .collect();
                crate::draw::Shape::points(pts, closed)
            })
            .collect();
        Ok((shapes, step.kind == StepKind::Solid))
    }

    fn scene(&mut self, args: SceneArgs) -> Result<ToolOutput> {
        use crate::scene::num;
        let key = design_key(&args.name);
        if key.is_empty() {
            return Err(Error::InvalidArgs("give the scene a name".into()));
        }
        let seen_key = format!("scene:{key}");
        let mut s = match self.scenes.iter().find(|(n, _)| *n == key) {
            Some((_, s)) => s.clone(),
            None => {
                self.drafts_seen.remove(&seen_key);
                Default::default()
            }
        };
        // All or nothing: the stored scene changes only if every part works.
        s.apply(&args).map_err(Error::InvalidArgs)?;
        self.scenes.retain(|(n, _)| *n != key);
        self.scenes.push((key.clone(), s.clone()));
        if self.scenes.len() > 8 {
            self.scenes.remove(0);
        }
        // Said once, then only what changed ([tree] compact); a call that
        // changes nothing is a look, and gets all of it.
        let looking = args.add.is_none()
            && args.change.is_none()
            && args.remove.is_none()
            && args.mirror.is_none()
            && args.repeat.is_none()
            && args.ground.is_none();
        let prev = self
            .drafts_seen
            .get(&seen_key)
            .filter(|_| self.store.config.tree.compact && !looking)
            .cloned();
        let items = s.items();
        let mut text = match s.bounds() {
            None => format!(
                "Scene \"{key}\": empty. Add objects: add=[{{\"id\", \"shape\", \"size\", \"at\"}}]."
            ),
            Some((lo, hi)) => {
                let head = format!(
                    "Scene \"{key}\": {} object{}, {} x {} x {} (x {} to {}, y {} to {}, z {} to {}), Z up{}.",
                    s.objects.len(),
                    if s.objects.len() == 1 { "" } else { "s" },
                    num(hi[0] - lo[0]),
                    num(hi[1] - lo[1]),
                    num(hi[2] - lo[2]),
                    num(lo[0]),
                    num(hi[0]),
                    num(lo[1]),
                    num(hi[1]),
                    num(lo[2]),
                    num(hi[2]),
                    if s.ground { ", the ground at z 0" } else { "" },
                );
                match &prev {
                    Some(p) => match items_diff(&p.items, &items) {
                        Some(diff) => format!("{head} Since the last answer: {diff}."),
                        None => format!("{head} Objects as before."),
                    },
                    None => format!("{head} {}.", s.listing()),
                }
            }
        };
        let mut checks_said = String::new();
        if !s.objects.is_empty() {
            let checks = s.checks();
            checks_said = checks.join("; ");
            if prev.as_ref().is_some_and(|p| p.checks == checks_said) {
                if !checks.is_empty() {
                    text.push_str("\nChecks as before.");
                }
            } else if checks.is_empty() {
                text.push_str(if s.ground {
                    "\nChecks: everything rests on the ground or on something; nothing runs into anything."
                } else {
                    "\nChecks: all parts are joined; nothing runs into anything."
                });
            } else {
                text.push_str(&format!("\nChecks: {}.", checks.join("; ")));
            }
            let build = self.explain(
                "scene-build",
                "\nTo build it: at is each object's centre, size its full extent on its own x, y, z before rotate turns it (degrees about x, then y, then z). In Blender: Add > Mesh > Cube (box), Cylinder, UV Sphere, Cone, Torus or Plane, then in the sidebar (N) > Item type Location = at, Rotation = rotate and Dimensions = size. Other apps take the same numbers (a box's corner is at minus half its size). Or export=\"obj\" and import the file (File > Import > Wavefront .obj).",
                "\nBuild: Location = at, Rotation = rotate, Dimensions = size; or export=\"obj\".",
            );
            text.push_str(build);
        }
        let view = args.view.unwrap_or_default();
        let look = args.look.unwrap_or([35.0, 25.0]);
        if look.iter().any(|v| !v.is_finite()) {
            return Err(Error::InvalidArgs("look is [turn, tilt] in degrees".into()));
        }
        let ids = args.ids.unwrap_or(true);
        let cfg = self.store.config.screenshot.clone();
        let picture = (!self.store.config.text_only && cfg.enabled
            || args.export == Some(SceneExport::Png))
        .then(|| s.render(view, look, ids));
        if let Some(format) = args.export {
            let written = match format {
                SceneExport::Obj => {
                    // The colours first: the model names their file.
                    let (_, mtl) = s.obj(&key, "");
                    let mtl_path = self
                        .exports
                        .write(&key, "mtl", mtl.as_bytes())
                        .map_err(Error::InvalidArgs)?;
                    let mtl_name = mtl_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let (obj, _) = s.obj(&key, &mtl_name);
                    let path = self
                        .exports
                        .write(&key, "obj", obj.as_bytes())
                        .map_err(Error::InvalidArgs)?;
                    (path, obj.len() + mtl.len(), " with its colours beside it")
                }
                SceneExport::Png => {
                    let pic = picture
                        .clone()
                        .ok_or_else(|| Error::InvalidArgs("nothing to write".into()))?;
                    let mut pm = tiny_skia::Pixmap::new(pic.width, pic.height)
                        .ok_or_else(|| Error::InvalidArgs("nothing to write".into()))?;
                    pm.data_mut().copy_from_slice(&pic.rgba);
                    let bytes = pm
                        .encode_png()
                        .map(imaging::signed_png)
                        .map_err(|e| Error::InvalidArgs(format!("could not write the PNG: {e}")))?;
                    let path = self
                        .exports
                        .write(&key, "png", &bytes)
                        .map_err(Error::InvalidArgs)?;
                    (path, bytes.len(), "")
                }
            };
            text.push_str(&format!(
                "\nExported to {}{} ({} KB). It is temporary: import it into the app now (it is deleted when the server stops).",
                written.0.display(),
                written.2,
                written.1.div_ceil(1024)
            ));
        }
        let mut seen = DraftSeen {
            head: String::new(),
            items,
            checks: checks_said,
            steps: String::new(),
            picture: prev.as_ref().map(|p| p.picture).unwrap_or(0),
            cells: String::new(),
        };
        if self.depth == 1 {
            if prev.is_none() {
                self.note.drafts_full.push(seen_key.clone());
            } else {
                self.note.drafts_diff.push(seen_key.clone());
            }
        }
        let Some(picture) = picture.filter(|_| !self.store.config.text_only && cfg.enabled) else {
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        };
        // The very picture the model has: not sent again.
        let hash = picture_hash(&picture);
        if prev.as_ref().is_some_and(|p| p.picture == hash) {
            text.push_str("\nThe picture is as before.");
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        }
        seen.picture = hash;
        if self.depth == 1 {
            self.note.drafts_picture.push(seen_key.clone());
        }
        self.drafts_seen.insert(seen_key, seen);
        if !s.objects.is_empty() {
            let long = format!(
                "\nThe picture: front (x right, z up), right (y right, z up) and top (x right, y up) to one scale, a grid line every {}; and a perspective view (look [{}, {}]) with shadows straight down onto the ground. view=\"front\" (or right, top, perspective) shows one bigger.",
                num(s.step()),
                num(look[0]),
                num(look[1])
            );
            let short = format!("\nGrid: a line every {}.", num(s.step()));
            text.push_str(self.explain("scene-picture", &long, &short));
        }
        let (img, _) = imaging::encode(picture, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    fn design(&mut self, args: DesignArgs) -> Result<ToolOutput> {
        use crate::design::{Design, Extras, StepKind, hex};
        let key = design_key(&args.name);
        if key.is_empty() {
            return Err(Error::InvalidArgs("give the design a name".into()));
        }
        let seen_key = format!("design:{key}");
        let mut d = match self.designs.iter().position(|(n, _)| *n == key) {
            Some(i) => self.designs[i].1.clone(),
            None => {
                self.drafts_seen.remove(&seen_key);
                let [w, h] = args.size.ok_or_else(|| {
                    Error::InvalidArgs(format!(
                        "no design called \"{key}\" yet: give size [width, height] to start one"
                    ))
                })?;
                Design::new(w, h)
            }
        };
        // All or nothing: the stored design changes only if every part works.
        d.apply(&args, &mut self.fonts)
            .map_err(Error::InvalidArgs)?;
        self.designs.retain(|(n, _)| *n != key);
        self.designs.push((key.clone(), d.clone()));
        if self.designs.len() > 8 {
            self.designs.remove(0);
        }
        // Said once, then only what changed ([tree] compact); a call that
        // changes nothing is a look, and gets all of it.
        let looking = args.size.is_none()
            && args.background.is_none()
            && args.margin.is_none()
            && args.cell_size.is_none()
            && args.add.is_none()
            && args.change.is_none()
            && args.remove.is_none()
            && args.mirror.is_none()
            && args.align.is_none()
            && args.distribute.is_none()
            && args.order.is_none();
        let prev = self
            .drafts_seen
            .get(&seen_key)
            .filter(|_| self.store.config.tree.compact && !looking)
            .cloned();
        let head = format!(
            "Design \"{key}\": {} x {}, background {}, margin {}.",
            d.width,
            d.height,
            hex(d.background),
            d.margin,
        );
        let items = d.items(&mut self.fonts);
        let count = format!(
            "{} layer{}",
            d.layers.len(),
            if d.layers.len() == 1 { "" } else { "s" }
        );
        let mut text = match &prev {
            Some(p) => {
                let head = if p.head == head {
                    format!("Design \"{key}\":")
                } else {
                    head.clone()
                };
                match items_diff(&p.items, &items) {
                    Some(diff) => format!("{head} {count}; since the last answer: {diff}."),
                    None => format!("{head} {count}, as before."),
                }
            }
            None => format!(
                "{head} {count}, back to front: {}.",
                if d.layers.is_empty() {
                    "none yet".to_string()
                } else {
                    d.listing(&mut self.fonts)
                }
            ),
        };
        let checks = d.checks(&mut self.fonts);
        let checks_said = checks.join("; ");
        if prev.as_ref().is_some_and(|p| p.checks == checks_said) {
            if !checks.is_empty() {
                text.push_str("\nChecks as before.");
            }
        } else if checks.is_empty() {
            if !d.layers.is_empty() {
                text.push_str("\nChecks: nothing off.");
            }
        } else {
            text.push_str(&format!("\nChecks: {}.", checks.join("; ")));
        }
        let steps = d.steps();
        let show = args.show.clone().unwrap_or_default();
        let list_steps = show.steps
            || self.store.config.tools.design_steps == crate::config::DesignSteps::Always;
        let mut steps_said = prev.as_ref().map(|p| p.steps.clone()).unwrap_or_default();
        if !steps.is_empty() && !list_steps {
            text.push_str(&format!(
                "\n{} step(s) to paint it (show steps=true lists them).",
                steps.len()
            ));
        } else if !steps.is_empty() {
            let list: Vec<String> = steps
                .iter()
                .enumerate()
                .map(|(k, st)| {
                    let ids: Vec<&str> =
                        st.layers.iter().map(|&i| d.layers[i].id.as_str()).collect();
                    let how = match st.kind {
                        StepKind::Solid => "solid".to_string(),
                        StepKind::Outline(w) => format!("lines {w}"),
                        StepKind::Text => "text".to_string(),
                    };
                    format!("{} {} {how} ({})", k + 1, hex(st.color), ids.join(", "))
                })
                .collect();
            // How to paint it is said once; then only the steps.
            let how = self.explain(
                "design-paint",
                " Each: set the colour, then draw(strokes=[{\"design\": <name>, \"step\": n, \"fill\": <brush size>}], canvas=...); a lines step uses a brush that wide; type text steps with the text tool. Or export=\"svg\" / \"png\" and import the file.",
                "",
            );
            let said = format!(
                "background {} first, then steps {}.",
                hex(d.background),
                list.join("; ")
            );
            if prev.as_ref().is_some_and(|p| p.steps == said) {
                text.push_str("\nThe steps to paint it are as before.");
            } else {
                text.push_str(&format!("\nTo paint it in an app: {said}{how}"));
            }
            steps_said = said;
        }
        let mut seen = DraftSeen {
            head,
            items,
            checks: checks_said,
            steps: steps_said,
            picture: prev.as_ref().map(|p| p.picture).unwrap_or(0),
            cells: prev.as_ref().map(|p| p.cells.clone()).unwrap_or_default(),
        };
        if self.depth == 1 {
            if prev.is_none() {
                self.note.drafts_full.push(seen_key.clone());
            } else {
                self.note.drafts_diff.push(seen_key.clone());
            }
        }
        if let Some(format) = args.export {
            let (bytes, ext) = match format {
                ExportFormat::Png => (d.png(&mut self.fonts).map_err(Error::InvalidArgs)?, "png"),
                ExportFormat::Svg => (
                    d.svg(&mut self.fonts)
                        .map_err(Error::InvalidArgs)?
                        .into_bytes(),
                    "svg",
                ),
            };
            let path = self
                .exports
                .write(&key, ext, &bytes)
                .map_err(Error::InvalidArgs)?;
            text.push_str(&format!(
                "\nExported to {} ({} KB). It is temporary: import it into the app now (it is deleted when the server stops).",
                path.display(),
                bytes.len().div_ceil(1024)
            ));
        }
        let cfg = self.store.config.screenshot.clone();
        if self.store.config.text_only || !cfg.enabled {
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        }
        // One cell, magnified: its picture and what is in it.
        if let Some(cell) = &show.cell {
            self.drafts_seen.insert(seen_key, seen);
            let (picture, about) = d
                .render_cell(cell, &mut self.fonts)
                .map_err(Error::InvalidArgs)?;
            text.push_str(&format!("\n{about}"));
            let (img, _) = imaging::encode(picture, &cfg)?;
            return Ok(ToolOutput {
                text,
                image: Some(img),
                is_error: false,
            });
        }
        let extras = Extras {
            grid: show.grid,
            ids: show.ids,
            guides: show.guides,
            cells: show.cells.unwrap_or(true),
        };
        let cells = extras.cells;
        let picture = d
            .render(cfg.max_dimension.clamp(256, 1024), extras, &mut self.fonts)
            .map_err(Error::InvalidArgs)?;
        // The very picture the model has: not sent again.
        let hash = picture_hash(&picture);
        if prev.as_ref().is_some_and(|p| p.picture == hash) {
            text.push_str("\nThe picture is as before.");
            self.drafts_seen.insert(seen_key, seen);
            return Ok(ToolOutput::text(text));
        }
        // The cells are as they were: said with the first picture only.
        let described = d.cells().describe();
        if cells && prev.as_ref().is_none_or(|p| p.cells != described) {
            text.push_str(&format!(
                "\nOn the picture, {described} (A1 top-left); show {{\"cell\": \"C4\"}} looks at one closely."
            ));
            seen.cells = described;
        }
        seen.picture = hash;
        if self.depth == 1 {
            self.note.drafts_picture.push(seen_key.clone());
        }
        self.drafts_seen.insert(seen_key, seen);
        let (img, _) = imaging::encode(picture, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    /// A picture traced with trace_image, by name.
    fn trace_named(&self, name: &str) -> std::result::Result<&crate::paint::Trace, String> {
        if let Some((_, t)) = self
            .traces
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
        {
            return Ok(t);
        }
        let known: Vec<&str> = self.traces.iter().map(|(n, _)| n.as_str()).collect();
        Err(if known.is_empty() {
            format!("no picture called \"{name}\": trace it with trace_image first")
        } else {
            format!(
                "no picture called \"{name}\" (traced: {})",
                known.join(", ")
            )
        })
    }

    /// How big one of a traced picture's working pixels is on screen,
    /// fitted into `frame` (how far apart its neighbouring shapes' edges
    /// may be).
    fn trace_pixel(&self, name: &str, frame: &crate::draw::Frame) -> f64 {
        self.trace_named(name).map_or(0.0, |t| {
            let place = crate::paint::fit_in(frame.screen_rect(), t.width, t.height);
            place.width / f64::from(t.target.width.max(1))
        })
    }

    /// The shapes of one step of a traced picture, fitted into the
    /// drawing area with its proportions kept.
    fn trace_shapes(
        &self,
        s: &DrawStroke,
        name: &str,
        frame: &crate::draw::Frame,
        area: &str,
    ) -> std::result::Result<Vec<crate::draw::Shape>, String> {
        if shape_given(s) {
            return Err("give trace on its own (with step and fill), not with a shape".into());
        }
        if s.rotate.is_some() || s.repeat.is_some() {
            return Err("a trace can't be turned or repeated".into());
        }
        if area == "the screenshot" {
            return Err("a trace needs canvas or element_index: where the picture goes".into());
        }
        let t = self.trace_named(name)?;
        let n = t.steps.len();
        let step = match s.step {
            Some(k) if k >= 1 && k as usize <= n => k as usize,
            _ => return Err(format!("\"{name}\" has steps 1 to {n}: give step")),
        };
        let place = crate::paint::fit_in(frame.screen_rect(), t.width, t.height);
        let (xa, xb) = (frame.x0.min(frame.x1), frame.x0.max(frame.x1));
        let (ya, yb) = (frame.y0.min(frame.y1), frame.y0.max(frame.y1));
        Ok(t.steps[step - 1]
            .regions
            .iter()
            .map(|r| {
                let pts = r
                    .iter()
                    .map(|&(u, v)| {
                        let (x, y) = frame.to_frame(Point::new(
                            place.x + u * place.width,
                            place.y + v * place.height,
                        ));
                        (x.clamp(xa, xb), y.clamp(ya, yb))
                    })
                    .collect();
                crate::draw::Shape::points(pts, true)
            })
            .collect())
    }

    fn trace_image(&mut self, args: TraceImageArgs) -> Result<ToolOutput> {
        use crate::tools::TraceDetail;
        let colors = args.colors.unwrap_or(8);
        if !(2..=16).contains(&colors) {
            return Err(Error::InvalidArgs("colors is 2 to 16".into()));
        }
        let (src, source, default_name) = match (&args.path, &args.app) {
            (Some(_), Some(_)) => {
                return Err(Error::InvalidArgs(
                    "give path (an image file) or app (what its window shows), not both".into(),
                ));
            }
            (None, None) => {
                return Err(Error::InvalidArgs(
                    "give path (an image file) or app (trace what its window shows)".into(),
                ));
            }
            (Some(path), None) => {
                let p = std::path::Path::new(path.trim());
                let meta = std::fs::metadata(p)
                    .map_err(|e| Error::InvalidArgs(format!("can't read {path}: {e}")))?;
                if !meta.is_file() {
                    return Err(Error::InvalidArgs(format!("{path} is not a regular file")));
                }
                let size = meta.len();
                if size > 64 * 1024 * 1024 {
                    return Err(Error::InvalidArgs(format!(
                        "{path} is {} MB; give an image under 64 MB",
                        size / (1024 * 1024)
                    )));
                }
                // A small file can still unpack to a huge picture.
                let (w, h) = image::image_dimensions(p).map_err(|e| {
                    Error::InvalidArgs(format!(
                        "{path} isn't a picture I can read (PNG or JPEG): {e}"
                    ))
                })?;
                if u64::from(w) * u64::from(h) > 50_000_000 {
                    return Err(Error::InvalidArgs(format!(
                        "{path} is {w} x {h} pixels; trace pictures up to 50 megapixels"
                    )));
                }
                let img = image::open(p)
                    .map_err(|e| {
                        Error::InvalidArgs(format!(
                            "{path} isn't a picture I can read (PNG or JPEG): {e}"
                        ))
                    })?
                    .to_rgba8();
                let (w, h) = img.dimensions();
                let file = p
                    .file_name()
                    .map_or_else(|| path.clone(), |f| f.to_string_lossy().into_owned());
                let stem = p
                    .file_stem()
                    .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
                (
                    Capture {
                        width: w,
                        height: h,
                        rgba: img.into_raw(),
                        bounds: Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
                    },
                    file,
                    stem,
                )
            }
            (None, Some(query)) => {
                if self.store.config.text_only || !self.store.config.screenshot.enabled {
                    return Err(Error::Blocked(
                        "trace_image".into(),
                        "screenshots are disabled (text_only / screenshot.enabled=false)".into(),
                    ));
                }
                let app = self.resolve_app(query)?;
                let window = self.resolve_window(&app, args.window.as_deref(), false)?;
                let part = match (args.area, args.element_index) {
                    (Some(_), Some(_)) => {
                        return Err(Error::InvalidArgs(
                            "give box or element_index, not both".into(),
                        ));
                    }
                    (Some([l, t, r, b]), None) => {
                        let map = self.state(app.pid).ok().and_then(|s| s.coord).ok_or_else(|| {
                            Error::InvalidArgs(
                                "box is in screenshot pixels: call get_app_state (or screenshot) first"
                                    .into(),
                            )
                        })?;
                        let (a, z) = (map.to_screen(l, t)?, map.to_screen(r, b)?);
                        let rect = Rect::new(a.x, a.y, z.x - a.x, z.y - a.y);
                        if rect.is_empty() {
                            return Err(Error::InvalidArgs(
                                "box is [left, top, right, bottom], right of left and below top"
                                    .into(),
                            ));
                        }
                        Some(rect)
                    }
                    (None, Some(i)) => {
                        self.observe(&app, &window, false)?;
                        let node = self.node_by_index(&app, i)?;
                        Some(node.bounds.filter(|b| !b.is_empty()).ok_or_else(|| {
                            Error::InvalidArgs(format!(
                                "element {i} ({}) has no on-screen area",
                                node.label()
                            ))
                        })?)
                    }
                    (None, None) => None,
                };
                let mut cap = self.capture_clean(|b| b.capture(&app, &window))?;
                self.redact_capture(&mut cap);
                if let Some(r) = part {
                    let sx = f64::from(cap.width) / cap.bounds.width.max(1e-9);
                    let sy = f64::from(cap.height) / cap.bounds.height.max(1e-9);
                    let x0 = ((r.x - cap.bounds.x) * sx).max(0.0);
                    let y0 = ((r.y - cap.bounds.y) * sy).max(0.0);
                    let x1 = ((r.x + r.width - cap.bounds.x) * sx).min(f64::from(cap.width));
                    let y1 = ((r.y + r.height - cap.bounds.y) * sy).min(f64::from(cap.height));
                    if x1 - x0 < 4.0 || y1 - y0 < 4.0 {
                        return Err(Error::InvalidArgs(
                            "that part of the window is (almost) off the picture".into(),
                        ));
                    }
                    cap = imaging::crop(
                        &cap,
                        (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32),
                    );
                }
                (
                    cap,
                    format!("{} window \"{}\"", app.name, window.title),
                    app.name.clone(),
                )
            }
        };
        let detail = match args.detail.unwrap_or_default() {
            TraceDetail::Low => crate::paint::Detail::LOW,
            TraceDetail::Medium => crate::paint::Detail::MEDIUM,
            TraceDetail::High => crate::paint::Detail::HIGH,
        };
        let t = crate::paint::trace(&src, colors as usize, detail);
        // A short name the model can repeat.
        let name: String = args
            .name
            .clone()
            .unwrap_or(default_name)
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(40)
            .collect();
        let name = if name.is_empty() {
            "picture".to_string()
        } else {
            name
        };
        let regions: usize = t.steps.iter().map(|s| s.regions.len()).sum();
        let mut text = format!(
            "Traced \"{name}\" ({source}, {} x {} px) as {} steps of flat colour, {regions} shapes in all; the picture shows the result. Paint the steps in order, back to front: for each, set the app's colour to its hex, then draw.",
            t.width,
            t.height,
            t.steps.len()
        );
        for (k, s) in t.steps.iter().enumerate() {
            text.push_str(&format!(
                "\n{}. {}: {} shape{}, {:.0}% of the picture",
                k + 1,
                crate::paint::hex(s.color),
                s.regions.len(),
                if s.regions.len() == 1 { "" } else { "s" },
                s.share * 100.0
            ));
            if k == 0 && s.share > 0.9 {
                text.push_str(" (all of it: on an empty canvas one bucket click does this step)");
            }
        }
        // How to paint and check it is said once; then only the call.
        if self.explain_first("trace-paint") {
            text.push_str(&format!(
                "\nEach step: draw(app, canvas=<the document's box and size>, strokes=[{{\"trace\": \"{name}\", \"step\": n, \"fill\": <the app's brush size>}}]). The picture keeps its proportions ({} x {}): give the canvas the same, or it is centred with margins. Afterwards screenshot(app, canvas=..., compare=\"{name}\") shows where the canvas still differs.",
                t.width, t.height
            ));
        } else {
            text.push_str(&format!(
                "\nEach step: draw strokes=[{{\"trace\": \"{name}\", \"step\": n, \"fill\": ...}}] on a {} x {} canvas; check with compare=\"{name}\".",
                t.width, t.height
            ));
        }
        let cfg = self.store.config.screenshot.clone();
        let (pw, ph) = imaging::fit(t.width, t.height, 512);
        let picture = crate::paint::render(&t, pw, ph);
        self.traces.retain(|(n, _)| n != &name);
        self.traces.push((name, t));
        if self.traces.len() > 8 {
            self.traces.remove(0);
        }
        if self.store.config.text_only || !cfg.enabled {
            return Ok(ToolOutput::text(text));
        }
        let (img, _) = imaging::encode(picture, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    /// How the document in `cap` compares with a traced picture: how many
    /// cells of an 8 x 8 grid look alike, and the most different ones.
    fn compare_note(
        &self,
        name: &str,
        frame: &crate::draw::Frame,
        cap: &Capture,
    ) -> std::result::Result<String, String> {
        const CELLS: usize = 8;
        /// ΔE under this looks alike.
        const ALIKE: f64 = 12.0;
        let t = self.trace_named(name)?;
        let place = crate::paint::fit_in(frame.screen_rect(), t.width, t.height);
        let (sx, sy) = (
            f64::from(cap.width) / cap.bounds.width.max(1e-9),
            f64::from(cap.height) / cap.bounds.height.max(1e-9),
        );
        let area = Rect::new(
            (place.x - cap.bounds.x) * sx,
            (place.y - cap.bounds.y) * sy,
            place.width * sx,
            place.height * sy,
        );
        let mut cells = crate::paint::compare(t, cap, area, CELLS);
        if cells.is_empty() {
            return Err("the canvas is not in this picture".into());
        }
        let alike = cells.iter().filter(|c| c.delta < ALIKE).count();
        let mean = cells.iter().map(|c| c.delta).sum::<f64>() / cells.len() as f64;
        let mut msg = format!(
            "\nCompared with \"{name}\": {alike} of {} cells look alike (mean colour difference {mean:.0}; under {ALIKE:.0} looks alike).",
            cells.len()
        );
        cells.sort_by(|a, b| b.delta.total_cmp(&a.delta));
        let worst: Vec<String> = cells
            .iter()
            .filter(|c| c.delta >= ALIKE)
            .take(4)
            .map(|c| {
                let at = |i: usize, j: usize| {
                    frame.to_frame(Point::new(
                        place.x + place.width * i as f64 / CELLS as f64,
                        place.y + place.height * j as f64 / CELLS as f64,
                    ))
                };
                let (a, b) = (at(c.col, c.row), at(c.col + 1, c.row + 1));
                format!(
                    "x {:.0} to {:.0}, y {:.0} to {:.0} should be {} but is {}",
                    a.0.min(b.0),
                    a.0.max(b.0),
                    a.1.min(b.1),
                    a.1.max(b.1),
                    crate::paint::hex(c.want),
                    crate::paint::hex(c.got)
                )
            })
            .collect();
        if !worst.is_empty() {
            msg.push_str(&format!(" Most different: {}.", worst.join("; ")));
        }
        Ok(msg)
    }

    /// A fresh, clean picture of the window to aim with, and the map
    /// between the x/y actions take (the latest get_app_state screenshot)
    /// and the screen.
    fn aim_capture(&mut self, app: &AppInfo, window: Option<&str>) -> Result<(Capture, CoordMap)> {
        if self.store.config.text_only || !self.store.config.screenshot.enabled {
            return Err(Error::Blocked(
                "pixel targeting".into(),
                "it reads the screen, and screenshots are disabled (text_only / screenshot.enabled=false)"
                    .into(),
            ));
        }
        let map = self
            .state(app.pid)
            .ok()
            .and_then(|s| s.coord)
            .ok_or_else(|| {
                Error::InvalidArgs(
                    "x/y are in the pixels of get_app_state's screenshot: call get_app_state first"
                        .into(),
                )
            })?;
        let window = self.resolve_window(app, window, false)?;
        let mut cap = self.capture_clean(|b| b.capture(app, &window))?;
        self.redact_capture(&mut cap);
        Ok((cap, map))
    }

    /// The x/y point snapped to `how` ("corner", "edge", "center", a
    /// colour) within `radius` screenshot pixels, and a note saying how
    /// far it moved.
    fn snap_xy(
        &mut self,
        app: &AppInfo,
        window: Option<&str>,
        x: f64,
        y: f64,
        how: &str,
        radius: Option<f64>,
    ) -> Result<(f64, f64, String)> {
        use crate::target::{Feature, snap};
        let feature = Feature::parse(how).map_err(Error::InvalidArgs)?;
        let (cap, map) = self.aim_capture(app, window)?;
        let r = radius.unwrap_or(10.0).clamp(1.0, 100.0);
        let at = to_capture(&cap, &map, x, y)?;
        let per = to_capture(&cap, &map, x + 1.0, y)?.0 - at.0;
        let found = snap(&cap, at, r * per.abs().max(1e-9), feature).ok_or_else(|| {
            Error::InvalidArgs(format!(
                "no {} within {r} pixels of ({x}, {y}), so nothing was done. Look with screenshot(app, zoom=[{x}, {y}]), or leave out snap",
                feature.name()
            ))
        })?;
        let (nx, ny) = from_capture(&cap, &map, found);
        Ok((
            nx,
            ny,
            format!(
                " Snapped to the {} at ({nx:.1}, {ny:.1}), {}.",
                feature.name(),
                moved(nx - x, ny - y)
            ),
        ))
    }

    fn locate(&mut self, args: LocateArgs) -> Result<ToolOutput> {
        use crate::target::{Feature, colour_blobs, look_alikes, snap};
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let asked = [
            args.color.is_some(),
            args.like.is_some(),
            args.near.is_some(),
        ];
        if asked.iter().filter(|a| **a).count() != 1 {
            return Err(Error::InvalidArgs(
                "give one of color (areas of a colour), like (look-alikes of a box) or near with feature (a corner, edge or centre next to a point)".into(),
            ));
        }
        let (cap, map) = self.aim_capture(&app, args.window.as_deref())?;
        let whole = Rect::new(0.0, 0.0, f64::from(cap.width), f64::from(cap.height));
        let to_rect = |b: [f64; 4]| -> Result<Rect> {
            let [l, t, r, bt] = b;
            let a = to_capture(&cap, &map, l, t)?;
            let z = to_capture(&cap, &map, r, bt)?;
            let rect = Rect::new(
                a.0.min(z.0),
                a.1.min(z.1),
                (z.0 - a.0).abs(),
                (z.1 - a.1).abs(),
            );
            if rect.is_empty() {
                return Err(Error::InvalidArgs(
                    "a box is [left, top, right, bottom], right of left and below top".into(),
                ));
            }
            Ok(rect)
        };
        let area = match args.area {
            Some(b) => to_rect(b)?,
            None => whole,
        };
        let back = |p: (f64, f64)| from_capture(&cap, &map, p);
        let span = |r: Rect| {
            let (a, z) = (back((r.x, r.y)), back((r.x + r.width, r.y + r.height)));
            format!("box {:.0},{:.0} to {:.0},{:.0}", a.0, a.1, z.0, z.1)
        };
        // Results: (text, screen box to mark).
        let mut marks: Vec<Rect> = Vec::new();
        let screen_rect = |r: Rect| {
            let b = cap.bounds;
            let (sx, sy) = (
                f64::from(cap.width) / b.width.max(1e-9),
                f64::from(cap.height) / b.height.max(1e-9),
            );
            Rect::new(b.x + r.x / sx, b.y + r.y / sy, r.width / sx, r.height / sy)
        };
        let text = if let Some(c) = &args.color {
            let colour = crate::design::parse_colour(c)
                .map_err(Error::InvalidArgs)?
                .ok_or_else(|| Error::InvalidArgs("color must be a colour, not none".into()))?;
            let tol = args.tolerance.unwrap_or(16.0).clamp(0.0, 255.0) as i32;
            let blobs = colour_blobs(&cap, area, colour, tol);
            let shown: Vec<String> = blobs
                .iter()
                .take(10)
                .enumerate()
                .map(|(k, b)| {
                    marks.push(screen_rect(b.bbox));
                    let (x, y) = back(b.center);
                    format!("{} at ({x:.1}, {y:.1}), {}", k + 1, span(b.bbox))
                })
                .collect();
            if shown.is_empty() {
                format!(
                    "No {} (within {tol} per channel) here.",
                    crate::design::hex(colour)
                )
            } else {
                format!(
                    "{}{} area{} of {} (within {tol}), biggest first: {}{}.",
                    // Only the biggest are kept of a picture full of specks.
                    if blobs.len() >= crate::target::MAX_BLOBS {
                        "At least "
                    } else {
                        ""
                    },
                    blobs.len(),
                    if blobs.len() == 1 { "" } else { "s" },
                    crate::design::hex(colour),
                    shown.join("; "),
                    if blobs.len() > 10 {
                        "; and smaller ones"
                    } else {
                        ""
                    }
                )
            }
        } else if let Some(b) = args.like {
            let template = to_rect(b)?;
            let found = look_alikes(&cap, template, area, 0.85).map_err(Error::InvalidArgs)?;
            let shown: Vec<String> = found
                .iter()
                .enumerate()
                .map(|(k, m)| {
                    marks.push(screen_rect(m.bbox));
                    let (x, y) = back((
                        m.bbox.x + m.bbox.width / 2.0,
                        m.bbox.y + m.bbox.height / 2.0,
                    ));
                    let same =
                        (m.bbox.x - template.x).abs() < 2.0 && (m.bbox.y - template.y).abs() < 2.0;
                    format!(
                        "{} at ({x:.1}, {y:.1}), match {:.0}%{}",
                        k + 1,
                        m.score * 100.0,
                        if same { " (the one you gave)" } else { "" }
                    )
                })
                .collect();
            if shown.is_empty() {
                "Nothing here looks like that box.".to_string()
            } else {
                format!(
                    "{} place{} look like it, best first (centres): {}.",
                    found.len(),
                    if found.len() == 1 { "" } else { "s" },
                    shown.join("; ")
                )
            }
        } else {
            let p = args.near.map(|p| p.xy()).unwrap_or_default();
            let feature = Feature::parse(args.feature.as_deref().unwrap_or("center"))
                .map_err(Error::InvalidArgs)?;
            let r = args.radius.unwrap_or(12.0).clamp(1.0, 100.0);
            let at = to_capture(&cap, &map, p.0, p.1)?;
            let per = to_capture(&cap, &map, p.0 + 1.0, p.1)?.0 - at.0;
            match snap(&cap, at, r * per.abs().max(1e-9), feature) {
                Some(q) => {
                    let (x, y) = back(q);
                    let mark = 4.0 * per.abs().max(1.0);
                    marks.push(screen_rect(Rect::new(
                        q.0 - mark,
                        q.1 - mark,
                        2.0 * mark,
                        2.0 * mark,
                    )));
                    format!(
                        "The {} near ({}, {}): ({x:.1}, {y:.1}), {}.",
                        feature.name(),
                        p.0,
                        p.1,
                        moved(x - p.0, y - p.1)
                    )
                }
                None => format!(
                    "No {} within {r} pixels of ({}, {}).",
                    feature.name(),
                    p.0,
                    p.1
                ),
            }
        };
        let text = format!("{text} Coordinates are the x/y click takes.");
        let cfg = self.store.config.screenshot.clone();
        if marks.is_empty() || !args.picture.unwrap_or(cfg.locate_picture) {
            return Ok(ToolOutput::text(text));
        }
        // The window with each place found numbered, to check before acting.
        let mut shown = cap;
        let numbered: Vec<(u32, Rect)> = marks
            .into_iter()
            .enumerate()
            .map(|(k, r)| (k as u32 + 1, r))
            .collect();
        imaging::annotate(&mut shown, &numbered);
        let (img, _) = imaging::encode(shown, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    /// A clean picture of the window (private areas blacked out), or
    /// `None` when screenshots are off.
    fn window_capture(&mut self, app: &AppInfo, window: Option<&str>) -> Result<Option<Capture>> {
        if self.store.config.text_only || !self.store.config.screenshot.enabled {
            return Ok(None);
        }
        let window = self.resolve_window(app, window, false)?;
        let mut cap = self.capture_clean(|b| b.capture(app, &window))?;
        self.redact_capture(&mut cap);
        Ok(Some(cap))
    }

    /// Where a bucket click (or magic wand) fills each closed outline of
    /// the drawing, in the x/y click takes: read off the pixels of `cap`
    /// (with the strokes added first when they are only `planned`), so it
    /// knows when other lines cut a shape into pieces or a gap lets a
    /// fill run out. Without a picture, from the shapes alone.
    fn fill_report(
        &self,
        app: &AppInfo,
        cap: Option<Capture>,
        plan: &crate::draw::Plan,
        shapes: &[(String, crate::draw::Shape)],
        fills: &[Option<(f64, f64)>],
        planned: bool,
    ) -> String {
        let solid = |i: usize| {
            plan.shape
                .get(i)
                .and_then(|&k| fills.get(k))
                .is_some_and(Option::is_some)
        };
        let targets = crate::draw::fill_targets(&plan.strokes, 20, |i| !solid(i));
        let Some(map) = self.state(app.pid).ok().and_then(|s| s.coord) else {
            return String::new();
        };
        if targets.is_empty() {
            return String::new();
        }
        let name = |i: usize| {
            plan.shape
                .get(i)
                .and_then(|&k| shapes.get(k))
                .map_or("a stroke", |(n, _)| n.as_str())
        };
        let mut cap = cap;
        let (to_cap, to_screen) = match &cap {
            Some(c) => {
                let (b, sx, sy) = (
                    c.bounds,
                    f64::from(c.width) / c.bounds.width.max(1e-9),
                    f64::from(c.height) / c.bounds.height.max(1e-9),
                );
                (
                    Some(move |p: Point| Point::new((p.x - b.x) * sx, (p.y - b.y) * sy)),
                    Some(move |p: Point| Point::new(b.x + p.x / sx, b.y + p.y / sy)),
                )
            }
            None => (None, None),
        };
        // Not drawn yet: put the strokes on the picture, as paint.
        if planned && let (Some(c), Some(to_cap)) = (cap.as_mut(), to_cap) {
            let sx = f64::from(c.width) / c.bounds.width.max(1e-9);
            for (i, stroke) in plan.strokes.iter().enumerate() {
                let width = plan
                    .shape
                    .get(i)
                    .and_then(|&k| fills.get(k).copied().flatten())
                    .map_or(2.0, |(w, _)| w * sx);
                let pts: Vec<(f64, f64)> = stroke
                    .iter()
                    .map(|p| {
                        let q = to_cap(*p);
                        (q.x, q.y)
                    })
                    .collect();
                imaging::draw_path(c, &pts, [0, 0, 0], width.round().max(2.0) as i64);
            }
        }
        let show = |p: Point| {
            let (x, y) = map.to_image(p);
            format!("({x:.0}, {y:.0})")
        };
        let mut clicks: Vec<String> = Vec::new();
        let mut leaks: Vec<String> = Vec::new();
        for t in targets {
            let checked = match (&cap, to_cap, to_screen) {
                (Some(c), Some(to_cap), Some(_)) => {
                    let outline: Vec<Point> =
                        plan.strokes[t.stroke].iter().map(|p| to_cap(*p)).collect();
                    let holes: Vec<Vec<Point>> = t
                        .holes
                        .iter()
                        .map(|&h| plan.strokes[h].iter().map(|p| to_cap(*p)).collect())
                        .collect();
                    let hole_refs: Vec<&[Point]> = holes.iter().map(Vec::as_slice).collect();
                    crate::paint::fill_check(c, &outline, &hole_refs, to_cap(t.point))
                }
                _ => None,
            };
            let back = |p: Point| to_screen.map_or(p, |f| f(p));
            match checked {
                Some(f) if f.leak.is_some() => {
                    let at = back(f.leak.unwrap_or(t.point));
                    leaks.push(format!(
                        "{} would leak out through a gap near {}: close the outline first",
                        name(t.stroke),
                        show(at)
                    ));
                }
                Some(f) if f.clicks.len() > 1 => {
                    let n = f.clicks.len();
                    let at: Vec<String> = f.clicks.iter().take(5).map(|&p| show(back(p))).collect();
                    let more = if n > 5 {
                        format!(" and {} smaller", n - 5)
                    } else {
                        String::new()
                    };
                    clicks.push(format!(
                        "{}, cut into {n} pieces by other lines, at {}{more}",
                        name(t.stroke),
                        at.join(", ")
                    ));
                }
                _ => clicks.push(format!("{} at {}", name(t.stroke), show(t.point))),
            }
        }
        let mut msg = String::new();
        if !clicks.is_empty() {
            msg.push_str(&format!(
                " To fill a closed outline (bucket or magic wand), click {}.",
                clicks.join("; ")
            ));
        }
        for l in leaks {
            let mut l = l;
            if let Some(first) = l.get(..1) {
                l = first.to_uppercase() + &l[1..];
            }
            msg.push_str(&format!(" {l}."));
        }
        msg
    }

    /// The strokes in red over a screenshot of the window, with a grid in
    /// the coordinates the call used; nothing is drawn.
    fn draw_preview(
        &mut self,
        cap: Option<Capture>,
        frame: &crate::draw::Frame,
        plan: &crate::draw::Plan,
        cell_size: Option<f64>,
        summary: String,
    ) -> Result<ToolOutput> {
        let legend = self.explain(
            "draw-preview",
            " Red: the strokes (green: where each starts); the blue cells (A1 top-left, their lines labelled) are in the coordinates you gave. Call draw again without preview to draw them.",
            "",
        );
        let text = format!("Preview only, nothing was drawn: {summary}{legend}");
        let Some(mut cap) = cap else {
            return Ok(ToolOutput::text(text));
        };
        let cfg = self.store.config.screenshot.clone();
        let (out_w, _) = imaging::fit(cap.width, cap.height, cfg.max_dimension.max(64));
        let out_scale = f64::from(cap.width) / f64::from(out_w.max(1));
        let (bounds, cw, ch) = (cap.bounds, f64::from(cap.width), f64::from(cap.height));
        let to_cap = |p: Point| {
            (
                (p.x - bounds.x) * cw / bounds.width.max(1e-9),
                (p.y - bounds.y) * ch / bounds.height.max(1e-9),
            )
        };
        let r = frame.screen_rect();
        let (cx0, cy0) = to_cap(Point::new(r.x, r.y));
        let (cx1, cy1) = to_cap(Point::new(r.x + r.width, r.y + r.height));
        let (ax, ay) = LabelSpace::Frame(*frame).axes(&cap, out_w);
        // Graph paper sized to the drawing: over it (and a cell round it)
        // when it is small in the area, else over the whole area.
        let (cells, sized) = drawing_cells(frame, plan_span(frame, plan), cell_size);
        let mut clip = Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0);
        if let (true, Some(t)) = (sized, plan_span(frame, plan)) {
            let e = cells.step;
            let a = to_cap(frame.to_screen(t.x0 - e, t.y0 - e));
            let b = to_cap(frame.to_screen(t.x1 + e, t.y1 + e));
            let (x0, x1) = (a.0.min(b.0).max(cx0), a.0.max(b.0).min(cx1));
            let (y0, y1) = (a.1.min(b.1).max(cy0), a.1.max(b.1).min(cy1));
            clip = Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
        }
        cells.draw(&mut cap, ax, ay, out_scale, Some(clip));
        let width = (2.0 * out_scale).round().max(2.0) as i64;
        for stroke in &plan.strokes {
            let pts: Vec<(f64, f64)> = stroke.iter().map(|p| to_cap(*p)).collect();
            imaging::draw_path(&mut cap, &pts, [255, 30, 60], width);
            if let Some(&start) = pts.first() {
                imaging::draw_path(&mut cap, &[start], [0, 200, 70], width * 3);
            }
        }
        let (img, _) = imaging::encode(cap, &cfg)?;
        Ok(ToolOutput {
            text,
            image: Some(img),
            is_error: false,
        })
    }

    /// Point the mouse at screenshot pixel (x, y) of `app`, for apps that
    /// send keys to what is under the pointer. Returns how to put the
    /// pointer back afterwards ([`Engine::unhover`]).
    fn hover(
        &mut self,
        app: &AppInfo,
        x: Option<f64>,
        y: Option<f64>,
    ) -> Result<Option<(InputTarget, Point)>> {
        if x.is_none() && y.is_none() {
            return Ok(None);
        }
        let anchor = self.anchor(app, None, x, y, "the pointer position")?;
        let at = self.anchor_point(app, &anchor)?;
        let target = self.input_target(app)?;
        self.overlay_point(at, false);
        let back = self.backend.move_pointer(&target, at)?;
        // Let the app see where the pointer is before the keys arrive.
        (self.sleep)(Duration::from_millis(40));
        Ok(back.map(|p| (target, p)))
    }

    fn unhover(&mut self, back: Option<(InputTarget, Point)>) {
        if let Some((target, p)) = back {
            let _ = self.backend.move_pointer(&target, p);
        }
    }

    fn press_combos(&mut self, app: &AppInfo, combos: &[KeyCombo]) -> Result<()> {
        let target = self.input_target(app)?;
        for combo in combos {
            if self.halted() {
                return Err(self.stopped_error());
            }
            self.backend.press_key(&target, combo)?;
            (self.sleep)(Duration::from_millis(self.store.config.timing.key_delay_ms));
        }
        Ok(())
    }

    fn press_key(&mut self, args: PressKeyArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        let combos = keys::parse_sequence(&args.key)?;
        if combos.len() > MAX_KEY_PRESSES {
            return Err(Error::InvalidArgs(format!(
                "`key` has {} presses; at most {MAX_KEY_PRESSES} per call (type text with type_text)",
                combos.len()
            )));
        }
        if let Some(i) = args.element_index {
            let h = self.element_by_index(&app, i)?;
            let node = self.node_by_index(&app, i)?.clone();
            self.overlay_point_element(&app, h, false);
            self.focus_element(&app, h, &node)?;
        }
        let back = self.hover(&app, args.x, args.y)?;
        let pressed = self.press_combos(&app, &combos);
        self.unhover(back);
        pressed?;
        self.settle_on(&app);
        let shown: Vec<String> = combos.iter().map(|c| c.to_string()).collect();
        Ok(ToolOutput::text(format!("Pressed {}.", shown.join(" "))))
    }

    fn type_text(&mut self, args: TypeTextArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        self.check_window(&app, args.window.as_deref())?;
        if args.text.is_empty() {
            return Err(Error::InvalidArgs("`text` must not be empty".into()));
        }
        let count = args.text.chars().count();
        if count > MAX_TYPED_CHARS {
            return Err(Error::InvalidArgs(format!(
                "`text` has {count} characters; at most {MAX_TYPED_CHARS} per call (paste long text: set_clipboard, then press_key ctrl+v / cmd+v)"
            )));
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
        let back = self.hover(&app, args.x, args.y)?;
        let typed = self.type_into_focus(&app, &args.text);
        self.unhover(back);
        typed?;
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
            // Never typed again automatically: many apps update what they
            // report a moment late, and typing twice would enter the text
            // twice (or send a message twice).
            if unchanged(self) {
                msg.push_str(TYPED_UNCONFIRMED_NOTE);
            }
        }
        Ok(ToolOutput::text(msg))
    }

    /// Type `text` into the app's focused element (newlines press Return).
    fn type_into_focus(&mut self, app: &AppInfo, text: &str) -> Result<()> {
        // Every path that types (set_value's fallback too) has the limit.
        let count = text.chars().count();
        if count > MAX_TYPED_CHARS {
            return Err(Error::InvalidArgs(format!(
                "{count} characters is too many to type; at most {MAX_TYPED_CHARS} (paste long text: set_clipboard, then press_key ctrl+v / cmd+v)"
            )));
        }
        let target = self.input_target(app)?;
        // A Windows line break is one Return, not two.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        // Split on newlines so each becomes a Return press (works everywhere).
        let mut first = true;
        for segment in text.split('\n') {
            if !first {
                if self.halted() {
                    return Err(self.stopped_error());
                }
                self.backend.press_key(
                    &target,
                    &KeyCombo {
                        modifiers: keys::Modifiers::default(),
                        key: Key::Named(NamedKey::Return),
                    },
                )?;
            }
            // Long text goes in pieces, checking the stop key between them.
            let mut rest = segment;
            while !rest.is_empty() {
                let cut = rest
                    .char_indices()
                    .nth(TYPE_CHUNK)
                    .map_or(rest.len(), |(i, _)| i);
                let (piece, after) = rest.split_at(cut);
                if self.halted() {
                    return Err(self.stopped_error());
                }
                self.backend.type_text(&target, piece)?;
                rest = after;
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
            let target = self.input_target(app)?;
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
            let target = self.input_target(app)?;
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

    fn find_element(&mut self, args: FindElementArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        let window = self.resolve_window(&app, args.window.as_deref(), false)?;
        self.observe(&app, &window, false)?;
        let role = args.role.map(|r| r.to_lowercase());
        let name = args.name.map(|n| crate::text::fold(&n));
        let text = args.text.map(|t| crate::text::fold(&t));
        let state = self.state(app.pid)?;
        let hits: Vec<&Node> = state
            .nodes
            .iter()
            .filter(|n| {
                role.as_deref().is_none_or(|r| n.role == r)
                    && name.as_deref().is_none_or(|q| {
                        n.name
                            .as_deref()
                            .is_some_and(|nm| crate::text::fold(nm).contains(q))
                    })
                    && text.as_deref().is_none_or(|q| node_text(n).contains(q))
                    && (!args.editable || n.states.editable)
            })
            .collect();
        let total = hits.len();
        let offset = args.offset.unwrap_or(0);
        let hits: Vec<&Node> = hits
            .into_iter()
            .skip(offset)
            .take(args.max_results.max(1))
            .collect();
        if hits.is_empty() {
            return Ok(ToolOutput::text(if total > 0 {
                format!(
                    "{total} element(s) in {} match; none after offset {offset}.",
                    app.name
                )
            } else {
                format!(
                    "No elements in {} match. Try get_app_state to see the whole tree.",
                    app.name
                )
            }));
        }
        let next = offset + hits.len();
        let mut out = if offset == 0 && next == total {
            format!("{} matching element(s) in {}:\n", hits.len(), app.name)
        } else {
            format!(
                "Matches {}–{next} of {total} in {}{}:\n",
                offset + 1,
                app.name,
                if next < total {
                    format!(" (offset={next} for the next ones)")
                } else {
                    String::new()
                }
            )
        };
        for n in hits {
            out.push_str(&format!("{} {}\n", n.index, n.line));
        }
        Ok(ToolOutput::text(out))
    }

    fn wait_for(&mut self, args: WaitForArgs) -> Result<ToolOutput> {
        let app = self.resolve_app(&args.app)?;
        let role = args.role.clone().map(|r| r.to_lowercase());
        let name = args.name.clone().map(|n| crate::text::fold(&n));
        let text = args.text.clone().map(|t| crate::text::fold(&t));
        let timing = &self.store.config.timing;
        // At most two minutes: the server answers nothing else while it waits.
        let timeout_ms = args
            .timeout_ms
            .unwrap_or(timing.wait_timeout_ms)
            .clamp(1, 120_000);
        let poll = Duration::from_millis(
            args.poll_ms
                .unwrap_or(timing.wait_poll_ms)
                .clamp(20, 60_000),
        );
        let timeout = Duration::from_millis(timeout_ms);
        let start = (self.clock)();
        let deadline = start + timeout;
        // `until`: the decision model judges the window each time.
        let until = match args.until.as_deref().map(str::trim) {
            Some(q) if !q.is_empty() => Some((q.to_string(), self.decider()?)),
            _ => None,
        };
        let matchers = role.is_some() || name.is_some() || text.is_some();
        // The text the decision model last said no about.
        let mut asked = None;

        loop {
            if let Ok(window) = self.resolve_window(&app, args.window.as_deref(), true)
                && self.observe(&app, &window, true).is_ok()
            {
                let state = self.state(app.pid)?;
                let found = state
                    .nodes
                    .iter()
                    .find(|n| {
                        role.as_deref().is_none_or(|r| n.role == r)
                            && name.as_deref().is_none_or(|q| {
                                n.name
                                    .as_deref()
                                    .is_some_and(|nm| crate::text::fold(nm).contains(q))
                            })
                            && text.as_deref().is_none_or(|q| node_text(n).contains(q))
                            && state_matches(n, args.state)
                    })
                    .map(|n| (n.index, n.line.clone()));
                match (&until, found) {
                    (None, Some((index, line))) => {
                        return Ok(ToolOutput::text(format!(
                            "Found after waiting: {index} {line}"
                        )));
                    }
                    (Some((question, decider)), found) if found.is_some() || !matchers => {
                        let decider = decider.clone();
                        let window = args.window.clone();
                        if let Some(yes) = self.until_yes(
                            &decider,
                            question,
                            &args.app,
                            window.as_deref(),
                            &mut asked,
                        )? {
                            let waited = (self.clock)().saturating_duration_since(start);
                            let mut out = format!(
                                "Yes after {:.1} s ({yes:.2}): {question}",
                                waited.as_secs_f64()
                            );
                            if let Some((index, line)) = found.filter(|_| matchers) {
                                out.push_str(&format!("\nFound: {index} {line}"));
                            }
                            return Ok(ToolOutput::text(out));
                        }
                    }
                    _ => {}
                }
            }
            if self.halted() {
                return Err(self.stopped_error());
            }
            if (self.clock)() >= deadline {
                return Err(Error::ActionFailed(match &until {
                    Some((q, _)) => format!(
                        "timed out after {timeout_ms}ms: the decision model never answered yes to \"{q}\""
                    ),
                    None => format!(
                        "timed out after {}ms waiting for an element matching {}",
                        timeout_ms,
                        describe_matcher(&args)
                    ),
                }));
            }
            (self.sleep)(poll);
        }
    }

    fn screenshot(&mut self, args: ScreenshotArgs) -> Result<ToolOutput> {
        if self.store.config.text_only || !self.store.config.screenshot.enabled {
            return Err(Error::Blocked(
                "screenshot".into(),
                "screenshots are disabled (text_only / screenshot.enabled=false)".into(),
            ));
        }
        // x/y/width/height are a part of the screen: they make the mode
        // region, and are never silently ignored.
        let region = [args.x, args.y, args.width, args.height]
            .iter()
            .any(Option::is_some);
        let mode = match args.mode {
            Some(m) if region && m != ScreenshotMode::Region => {
                return Err(Error::InvalidArgs(format!(
                    "x, y, width and height give a part of the screen (mode=region); mode={} doesn't take them. For part of a window, use element_index (one element) or zoom=[x, y].",
                    mode_name(m)
                )));
            }
            Some(m) => m,
            None if region && args.app.is_some() => {
                return Err(Error::InvalidArgs(
                    "x, y, width and height are a part of the screen in screen coordinates (mode=region, without app). For part of a window, use element_index (one element) or zoom=[x, y].".into(),
                ));
            }
            None if region => ScreenshotMode::Region,
            None if args.app.is_some() => ScreenshotMode::Window,
            None => ScreenshotMode::Auto,
        };
        // An element to zoom into (screen rect).
        let mut zoom: Option<Rect> = None;
        // A plain picture of a window whose screen the model knows: (pid,
        // screen), so an unchanged picture isn't sent again.
        let mut known_window: Option<(u32, u32)> = None;
        // What grid labels and `pick` points are in: screen coordinates,
        // or the x/y actions use for the window.
        let mut space = LabelSpace::Screen;
        if args.canvas.is_some() && mode != ScreenshotMode::Window {
            return Err(Error::InvalidArgs(
                "canvas needs a window screenshot (app)".into(),
            ));
        }
        let (capture, marks, label) = match mode {
            ScreenshotMode::Auto | ScreenshotMode::Full => (
                self.capture_clean(|b| b.capture_screen(None))?,
                None,
                "full screen".to_string(),
            ),
            ScreenshotMode::Region => {
                let region = match (args.x, args.y, args.width, args.height) {
                    (Some(x), Some(y), Some(w), Some(h))
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w > 0.0 && h > 0.0 =>
                    {
                        self.clip_to_screens(Rect::new(x, y, w, h))?
                    }
                    _ => {
                        return Err(Error::InvalidArgs(
                            "region mode needs x, y, width and height".into(),
                        ));
                    }
                };
                let (x, y, w, h) = (region.x, region.y, region.width, region.height);
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
                let app = self.resolve_app(query)?;
                let window = self.resolve_window(&app, args.window.as_deref(), false)?;
                if crate::privacy::active(&self.store.config.privacy)
                    || args.annotate
                    || args.element_index.is_some()
                {
                    // A current tree: where private data is, the marks, the element.
                    self.observe(&app, &window, false)?;
                }
                let plain = !args.annotate
                    && args.element_index.is_none()
                    && args.grid.is_none()
                    && !args.palette
                    && args.pick.is_none()
                    && args.canvas.is_none()
                    && args.compare.is_none()
                    && !args.cells
                    && args.cell.is_none()
                    && args.zoom.is_none();
                let cache = &self.store.config.cache;
                if plain
                    && (cache.dedupe_screenshots
                        || self.store.config.screenshot.scope == crate::config::ShotScope::Auto)
                {
                    // Which screen this is, to compare with the picture the
                    // model has of it. (An app whose tree can't be read
                    // still gets its picture, in full.)
                    let read = crate::privacy::active(&self.store.config.privacy)
                        || self.observe(&app, &window, false).is_ok();
                    if read
                        && let Some(st) = self.states.get(&app.pid)
                        && st.window_id == Some(window.id)
                        && st.known.as_ref().is_some_and(|k| k.id == st.screen)
                    {
                        known_window = Some((app.pid, st.screen));
                    }
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
                // The picture this call's look just took, if any.
                let cap = match self.last_capture.take() {
                    Some((pid, wid, epoch, cap))
                        if pid == app.pid && wid == window.id && epoch == self.epoch =>
                    {
                        cap
                    }
                    _ => self.capture_clean(|b| b.capture(&app, &window))?,
                };
                space = match self.states.get(&app.pid) {
                    Some(st) if st.window_id == Some(window.id) => {
                        st.coord.map_or(LabelSpace::Image, LabelSpace::Map)
                    }
                    _ => LabelSpace::Image,
                };
                // A document's own units or a math range, as draw takes them.
                if let Some(c) = &args.canvas {
                    let element = if c.area.is_none() {
                        args.element_index
                    } else {
                        None
                    };
                    space = LabelSpace::Frame(self.draw_frame(&app, element, Some(c))?.0);
                }
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
        let mut note = if redacted > 0 {
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

        // A loupe: magnified around a point, to aim.
        if let Some(z) = args.zoom {
            let LabelSpace::Map(map) = space else {
                return Err(Error::InvalidArgs(
                    "zoom is for a window screenshot (app) after get_app_state: its x/y are in that screenshot's pixels".into(),
                ));
            };
            let (x, y) = z.xy();
            let at = to_capture(&capture, &map, x, y)?;
            let per_x = to_capture(&capture, &map, x + 1.0, y)?.0 - at.0;
            let per_y = to_capture(&capture, &map, x, y + 1.0)?.1 - at.1;
            if at.0 < 0.0
                || at.1 < 0.0
                || at.0 >= f64::from(capture.width)
                || at.1 >= f64::from(capture.height)
            {
                return Err(Error::InvalidArgs(format!(
                    "({x}, {y}) is outside the window"
                )));
            }
            let per_screen = f64::from(capture.width) / capture.bounds.width.max(1e-9);
            let r = (args.radius.unwrap_or(12.0).clamp(3.0, 64.0) * per_screen)
                .round()
                .max(2.0) as usize;
            let k = (480 / (2 * r + 1)).clamp(2, 32);
            let (mut pic, origin) = crate::target::loupe(&capture, at.0, at.1, r, k)
                .ok_or_else(|| Error::InvalidArgs("nothing to magnify there".into()))?;
            let (ox, oy) = from_capture(&capture, &map, (origin.0 as f64, origin.1 as f64));
            let ax = imaging::Axis {
                offset: ox,
                scale: 1.0 / (k as f64 * per_x.abs().max(1e-9)),
            };
            let ay = imaging::Axis {
                offset: oy,
                scale: 1.0 / (k as f64 * per_y.abs().max(1e-9)),
            };
            let used = imaging::draw_grid(&mut pic, ax, ay, 0.0, 1.0, None);
            let under = imaging::color_at(&capture, at.0, at.1).unwrap_or_default();
            let (img, _) = imaging::encode(pic, &cfg)?;
            let text = if self.explain_first("loupe") {
                format!(
                    "Magnified around ({x}, {y}) in {label}: each square is one pixel of the screen picture ({:.2} of the x/y click takes); the crosshair is the point, on {under}. The grid is labelled in the x/y click takes, a line every {used}: read an exact point off it.{note}",
                    1.0 / per_x.abs().max(1e-9)
                )
            } else {
                format!(
                    "Magnified around ({x}, {y}) in {label}: a square a pixel ({:.2} of x/y); crosshair on {under}; grid in x/y, a line every {used}.{note}",
                    1.0 / per_x.abs().max(1e-9)
                )
            };
            return Ok(image(img, text));
        }

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
            capture = imaging::crop(&capture, px);
        }

        // Exact colours and coordinates, read before anything is drawn on it.
        let extras = args.grid.is_some() || args.palette || args.pick.is_some();
        let sig = matches!(mode, ScreenshotMode::Auto | ScreenshotMode::Full)
            .then(|| PixelSig::of(&capture, self.store.config.cache.pixel_grid));
        let (out_w, _) = imaging::fit(capture.width, capture.height, cfg.max_dimension.max(64));
        let (ax, ay) = space.axes(&capture, out_w);
        if args.palette {
            let colours: Vec<String> = imaging::palette(&capture, 8)
                .iter()
                .map(|(hex, share)| format!("{hex} {:.0}%", share * 100.0))
                .collect();
            note.push_str(&format!("\nMain colours: {}.", colours.join(", ")));
        }
        if let Some(points) = &args.pick {
            let read: Vec<String> = points
                .iter()
                .take(50)
                .map(|p| {
                    let (x, y) = p.xy();
                    match imaging::color_at(&capture, ax.pixel(x), ay.pixel(y)) {
                        Some(hex) => format!("({x}, {y}) {hex}"),
                        None => format!("({x}, {y}) is outside the image"),
                    }
                })
                .collect();
            note.push_str(&format!("\nColours: {}.", read.join("; ")));
        }
        if let Some(name) = &args.compare {
            let LabelSpace::Frame(f) = space else {
                return Err(Error::InvalidArgs(
                    "compare needs canvas: where the picture is on screen".into(),
                ));
            };
            note.push_str(
                &self
                    .compare_note(name, &f, &capture)
                    .map_err(Error::InvalidArgs)?,
            );
        }
        // Graph paper over a document: its cells, or one cell up close.
        let mut cells = None;
        if args.cells || args.cell.is_some() {
            let LabelSpace::Frame(f) = space else {
                return Err(Error::InvalidArgs(
                    "cells need canvas: where the document is on screen".into(),
                ));
            };
            let c = frame_cells(&f, None, args.cell_size)?;
            if let Some(name) = &args.cell {
                let (col, row) = c.parse(name).map_err(Error::InvalidArgs)?;
                let (pic, text) = cell_view(&capture, ax, ay, &c, col, row)?;
                let (img, _) = imaging::encode(pic, &cfg)?;
                return Ok(image(img, format!("{text}{note}")));
            }
            cells = Some((c, f));
        }
        if let Some(marks) = marks {
            imaging::annotate(&mut capture, &marks);
        }
        if let Some(step) = args.grid {
            let out_scale = f64::from(capture.width) / f64::from(out_w.max(1));
            // A document's grid covers just the document.
            let clip = match space {
                LabelSpace::Frame(f) => Some(capture_rect(&capture, f.screen_rect())),
                _ => None,
            };
            let used = imaging::draw_grid(&mut capture, ax, ay, step, out_scale, clip);
            note.push_str(&format!(
                "\nGrid: a line every {}, labelled in {}.",
                used,
                space.describe()
            ));
        }
        if let Some((c, f)) = cells {
            let out_scale = f64::from(capture.width) / f64::from(out_w.max(1));
            let clip = capture_rect(&capture, f.screen_rect());
            c.draw(&mut capture, ax, ay, out_scale, Some(clip));
            note.push_str(&format!(
                "\nCells over the document: {} (A1 top-left); cell=\"C4\" looks at one closely.",
                c.describe()
            ));
        }

        if zoom.is_some() {
            let (img, _) = imaging::encode(capture, &cfg)?;
            let how = self.explain(
                "zoomed",
                " It is its own picture: x/y for actions still refer to get_app_state's screenshot.",
                "",
            );
            let text = format!(
                "Screenshot of {label}, zoomed in: {}x{} px.{how}{note}",
                img.width, img.height
            );
            return Ok(image(img, text));
        }

        // The whole screen, or only what changed since the last one.
        if let Some(sig) = sig {
            let tolerance = self.store.config.cache.pixel_tolerance;
            let smart = mode == ScreenshotMode::Auto
                && cfg.scope == crate::config::ShotScope::Auto
                && !extras;
            if smart && let Some(last) = self.screen_shot.clone() {
                let (map, base) = (last.coord, last.id);
                if map.bounds == capture.bounds && sig.same_as(&last.pixels, tolerance) {
                    return Ok(ToolOutput::text(format!(
                        "The screen looks the same as in your last full-screen screenshot (#{base}); not re-sent (mode=full sends it anyway)."
                    )));
                }
                if let Some(part) =
                    self.changed_part(&capture, Some(&sig), Some(&last.pixels), Some(map))
                {
                    let (img, (ox, oy)) = imaging::encode_part(&capture, part, &map, &cfg)?;
                    let (w, h, x1, y1) = (img.width, img.height, ox + img.width, oy + img.height);
                    let id = self.next_shot();
                    let text = if self.explain_first("screen-part") {
                        format!(
                            "Screenshot #{id}: only the part of the screen that changed since your last full-screen screenshot (#{base}), {w}x{h} px: the area x {ox}–{x1}, y {oy}–{y1} of that screenshot (same scale; the rest is unchanged).{note}"
                        )
                    } else {
                        format!(
                            "Screenshot #{id}: changed part only, the area x {ox}–{x1}, y {oy}–{y1} of #{base} (x/y still refer to that whole screenshot).{note}"
                        )
                    };
                    self.pending_screen_shot = Some(ScreenShot {
                        pixels: sig,
                        coord: map,
                        id: base,
                    });
                    return Ok(image(img, text));
                }
            }
            let (img, map) = imaging::encode(capture, &cfg)?;
            let id = self.next_shot();
            self.pending_screen_shot = Some(ScreenShot {
                pixels: sig,
                coord: map,
                id,
            });
            let text = format!(
                "Screenshot #{id} of {label}: {}x{} px.{note}",
                img.width, img.height
            );
            return Ok(image(img, text));
        }

        // A window whose screen the model knows: as get_app_state sends it
        // (nothing if unchanged, else the part that changed or all of it),
        // and it becomes the picture x/y refer to. mode=window asks for all
        // of it.
        if let Some((pid, screen)) = known_window {
            let force = args.mode == Some(ScreenshotMode::Window);
            return Ok(
                match self.window_picture(pid, screen, capture, force, false)? {
                    Picture::Unchanged { base } => {
                        let at = base.map(|b| format!(" (#{b})")).unwrap_or_default();
                        ToolOutput::text(format!(
                            "Screenshot of {label}: unchanged since your last screenshot of it{at}; not re-sent (mode=\"window\" sends it anyway).{note}"
                        ))
                    }
                    Picture::Part {
                        img, id, base, at, ..
                    } => {
                        if self.depth == 1 {
                            self.note.pictures_part.push(pid);
                        }
                        let (ox, oy) = at;
                        let (x1, y1) = (ox + img.width, oy + img.height);
                        let of = base
                            .map(|b| format!("#{b}"))
                            .unwrap_or_else(|| "your earlier screenshot of it".into());
                        let text = format!(
                            "Screenshot #{id} of {label}: only the part that changed, {}x{} px: the area x {ox}–{x1}, y {oy}–{y1} of {of} (same scale; x/y still refer to that whole screenshot).{note}",
                            img.width, img.height
                        );
                        image(img, text)
                    }
                    Picture::Whole { img, id, .. } => {
                        if self.depth == 1 {
                            self.note.pictures_whole.push(pid);
                        }
                        let text = format!(
                            "Screenshot #{id} of {label}: {}x{} px.{note}",
                            img.width, img.height
                        );
                        image(img, text)
                    }
                },
            );
        }

        let (img, _map) = imaging::encode(capture, &cfg)?;
        let text = format!(
            "Screenshot of {label}: {}x{} px.{note}",
            img.width, img.height
        );
        Ok(image(img, text))
    }

    fn batch(&mut self, args: BatchArgs) -> Result<ToolOutput> {
        if args.steps.is_empty() {
            return Err(Error::InvalidArgs("batch needs at least one step".into()));
        }
        let total = args.steps.len();
        let mut report = String::new();
        let mut last_image = None;
        let mut any_error = false;
        let mut ran = 0;
        let mut stopped = String::new();
        // The app the steps acted on last: the report at the end is of it.
        let mut acted_on: Option<String> = None;
        // The report shows one line per step, so the trees the steps render
        // never reach the model: what it has seen of each app stays what it
        // saw before the batch (restored below).
        let seen_before = self.known_screens();
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
            let step_app = step_args
                .get("app")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let expects = step_args
                .get("expect")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|e| !e.trim().is_empty());
            let more = i + 1 < total;
            // The window in front before the step, to notice one coming up.
            let window_before = match &step_app {
                Some(a) if more && !args.through_windows && !expects => self.front_window(a),
                _ => None,
            };
            let parsed = ToolCall::parse(&step.tool, serde_json::Value::Object(step_args));
            let acting = parsed.as_ref().ok().and_then(mutating_app);
            let before = self.pending_images.len();
            let shot_before = self.pending_screen_shot.take();
            let quiet = self.quiet_depth.replace(self.depth + 1);
            let result = parsed.and_then(|c| self.call(c));
            self.quiet_depth = quiet;
            ran += 1;
            if acting.is_some() {
                acted_on = acting;
            }
            let imaged = result.as_ref().is_ok_and(|o| o.image.is_some());
            if !imaged {
                // No image from this step: an earlier step's stays the one
                // the batch returns.
                self.pending_screen_shot = shot_before;
            }
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
                        continue;
                    }
                    if !more {
                        continue;
                    }
                    // The steps after this one were planned on what it
                    // expected.
                    if expects && self.last_expect != Some(Outcome::Confirmed) {
                        stopped = format!(
                            "Stopped after step {}: what it expected wasn't confirmed; the {} step(s) after it were not run.",
                            i + 1,
                            total - i - 1
                        );
                        break;
                    }
                    if let (Some((id, _)), Some(a)) = (window_before, &step_app)
                        && let Some((now, title)) = self.front_window(a)
                        && now != id
                    {
                        stopped = format!(
                            "Stopped after step {}: window \"{title}\" came up, which it didn't expect; the {} step(s) after it were not run (they were meant for the window before; add expect dialog to a step that opens one, or through_windows=true).",
                            i + 1,
                            total - i - 1
                        );
                        break;
                    }
                }
                Err(e) => {
                    report.push_str(&format!("{}. {} — ERROR: {e}\n", i + 1, step.tool));
                    any_error = true;
                    if !args.continue_on_error || matches!(e, Error::Stopped(_) | Error::Cancelled)
                    {
                        break;
                    }
                }
            }
        }
        self.restore_known(seen_before);
        let head = if ran == total {
            format!("Ran {total} step(s):\n")
        } else {
            format!("Ran {ran} of {total} step(s):\n")
        };
        let mut text = format!("{head}{report}");
        if !stopped.is_empty() {
            text.push_str(&stopped);
            text.push('\n');
        }
        let mut out = ToolOutput {
            text,
            image: last_image,
            is_error: false,
        };
        // One report of what the steps changed, against what the model saw
        // before them.
        if self.store.config.tree.report_changes
            && let Some(app) = acted_on
        {
            let at = out.text.len();
            out = self.append_changes(&app, out);
            let tail = out.text.split_off(at);
            out.text
                .push_str(&tail.replacen("State after the action", "State after the steps", 1));
        }
        out.is_error = any_error;
        Ok(out)
    }

    /// The window an app shows in front now (its id and title).
    fn front_window(&mut self, query: &str) -> Option<(u64, String)> {
        let app = self.resolve_app(query).ok()?;
        let w = self.resolve_window(&app, None, false).ok()?;
        Some((w.id, w.title))
    }

    /// What the model has seen of each app, to put back after calls whose
    /// trees it never saw (batch steps, a script's tool calls).
    /// What the model has been shown so far, to put back with
    /// [`Self::not_delivered`] if the answer to the next call never reaches
    /// it (the client cancelled the call).
    pub fn shown(&self) -> Shown {
        Shown {
            hints: self.hints.0.borrow().clone(),
            apps: self
                .states
                .iter()
                .map(|(pid, st)| (*pid, (st.known.clone(), st.coord)))
                .collect(),
            screen_shot: self.screen_shot.clone(),
            partial: self.partial_report,
        }
    }

    /// The answer to the last call never reached the model: what it has
    /// seen is what it had seen before (`shown`), so the next look sends
    /// in full what that call would have shown, and no explanation counts
    /// as given.
    pub fn not_delivered(&mut self, shown: Shown) {
        *self.hints.0.borrow_mut() = shown.hints;
        self.partial_report = shown.partial;
        self.screen_shot = shown.screen_shot;
        for (pid, st) in self.states.iter_mut() {
            let (known, coord) = shown.apps.get(pid).cloned().unwrap_or_default();
            st.known = known;
            st.coord = coord;
        }
    }

    fn known_screens(&self) -> SeenBefore {
        SeenBefore {
            known: self
                .states
                .iter()
                .map(|(pid, st)| (*pid, st.known.clone()))
                .collect(),
            memory: self.memory.views(),
            hints: self.hints.0.borrow().clone(),
        }
    }

    fn restore_known(&mut self, seen: SeenBefore) {
        let SeenBefore {
            known: seen_before,
            memory,
            hints,
        } = seen;
        *self.hints.0.borrow_mut() = hints;
        self.memory.restore_views(memory);
        let mut refile = Vec::new();
        for (pid, st) in self.states.iter_mut() {
            let before = seen_before.get(pid).cloned().flatten();
            st.known = match (before, st.known.take()) {
                // Same screen: the model's tree is the one from before; the
                // screenshot state (only the returned image counts) is kept.
                (Some(b), Some(mut now)) if b.id == now.id => {
                    now.view = b.view;
                    Some(now)
                }
                // A screen reached meanwhile: its tree hasn't been shown, so
                // the next look sends all of it.
                (before, Some(mut now)) => {
                    refile.extend(before);
                    now.view = crate::screens::View::default();
                    Some(now)
                }
                (before, None) => before,
            };
        }
        let cache = self.store.config.cache.clone();
        if cache.enabled {
            for s in refile {
                self.memory.remember(
                    s,
                    cache.max_screens,
                    cache.max_memory_kb.saturating_mul(1024),
                );
            }
        }
    }

    fn window_tool(&mut self, args: WindowArgs) -> Result<ToolOutput> {
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
        let app = self.resolve_app(query)?;
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
                k.shot_id = None;
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
            let allowed_app =
                cfg.apps.is_empty() || cfg.apps.iter().any(|a| a.eq_ignore_ascii_case(&n.app));
            if !allowed_app {
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
                "[{hidden} from apps not in notifications.apps not shown]\n"
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
        if text.is_empty() {
            return Ok(ToolOutput::text("The clipboard is empty."));
        }
        // Someone may have copied a whole book.
        let count = text.chars().count();
        Ok(ToolOutput::text(if count > MAX_CLIPBOARD_CHARS {
            let head: String = text.chars().take(MAX_CLIPBOARD_CHARS).collect();
            format!("Clipboard (first {MAX_CLIPBOARD_CHARS} of {count} characters):\n{head}")
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
        let Ok(r) = self.render(app.pid, false, None) else {
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
        // How much of it to report ([tree] report). The model then hasn't
        // seen all of it: the next get_app_state reports it.
        use crate::config::Report;
        let text = match self.store.config.tree.report {
            Report::Brief => {
                let count = if r.full {
                    format!(
                        "{} element(s)",
                        self.state(app.pid).map(|s| s.nodes.len()).unwrap_or(0)
                    )
                } else {
                    format!("{} change(s)", r.changes)
                };
                let how = self.explain("report-brief", "; get_app_state shows them.", ".");
                out.text.push_str(&format!("\n\n{title} {count}{how}"));
                return out;
            }
            Report::Relevant if !r.full => {
                let (text, others) = self.relevant_changes(app.pid, &r.text);
                if others == 0 {
                    r.text.clone()
                } else {
                    let max = self.store.config.tree.report_changes_max_lines.max(1);
                    let lines: Vec<&str> = text.lines().take(max).collect();
                    let how = self.explain("report-others", "; get_app_state shows them", "");
                    out.text.push_str(&format!(
                        "\n\n{title}\n{}\n[{others} other change(s) elsewhere{how}]",
                        lines.join("\n")
                    ));
                    return out;
                }
            }
            _ => r.text.clone(),
        };
        let max = self.store.config.tree.report_changes_max_lines.max(1);
        let lines: Vec<&str> = text.lines().collect();
        out.text.push_str("\n\n");
        out.text.push_str(&title);
        out.text.push('\n');
        if lines.len() > max {
            out.text.push_str(&lines[..max].join("\n"));
            let how = self.explain("report-more", "; call get_app_state for the rest", "");
            out.text
                .push_str(&format!("\n[+{} more lines{how}]", lines.len() - max));
            // The model hasn't seen all of it: the next get_app_state
            // reports against what it had seen before, and sends only the
            // rest if nothing changed (when the model reads this result
            // itself, not a script).
            if self.depth == 1 && !self.in_script {
                self.partial_report = Some((app.pid, r.screen, text_hash(&r.text), max));
            }
        } else {
            out.text.push_str(r.text.trim_end());
            self.commit(app.pid, &window);
        }
        out
    }

    /// The lines of a change report about what the action acted on: the
    /// changes in its container (its parent, or grandparent when the parent
    /// holds little), added elements, the focused element; and how many
    /// other changes there are.
    fn relevant_changes(&self, pid: u32, text: &str) -> (String, usize) {
        let Ok(st) = self.state(pid) else {
            return (text.to_string(), 0);
        };
        let nodes = &st.nodes;
        let mut keep: HashSet<u32> = nodes
            .iter()
            .filter(|n| n.states.focused)
            .map(|n| n.index)
            .collect();
        if let Some((_, t)) = self.target.filter(|(p, _)| *p == pid)
            && let Some(pos) = nodes.iter().position(|n| n.index == t)
        {
            let size = |p: usize| {
                nodes[p + 1..]
                    .iter()
                    .take_while(|n| n.depth > nodes[p].depth)
                    .count()
            };
            let mut container = nodes[pos].parent.unwrap_or(pos);
            if size(container) < 4
                && let Some(g) = nodes[container].parent
            {
                container = g;
            }
            let end = container + 1 + size(container);
            keep.extend(nodes[container..end].iter().map(|n| n.index));
        }
        let mut out = String::new();
        let mut others = 0;
        for (i, line) in text.lines().enumerate() {
            // render()'s note on elements that keep changing: not an element.
            let restless = line.contains(" element(s) that keep changing on their own");
            let changed = line
                .strip_prefix("~ ")
                .filter(|_| !restless)
                .and_then(|l| l.split_whitespace().next())
                .and_then(|t| t.parse::<u32>().ok());
            let removed = line.starts_with("- ");
            let shown = i == 0
                || restless
                || line.starts_with("+ ")
                || line.starts_with("in ")
                || changed.is_some_and(|x| keep.contains(&x))
                || (changed.is_none() && !removed && !line.starts_with("~ "));
            if shown {
                out.push_str(line);
                out.push('\n');
            } else if let Some(rest) = line.strip_prefix("- ")
                && let Some((n, _)) = rest.split_once(" removed:")
            {
                others += n.parse::<usize>().unwrap_or(1);
            } else {
                others += 1;
            }
        }
        (out, others)
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

/// Appended when typed text doesn't show in the field (yet).
const TYPED_UNCONFIRMED_NOTE: &str = " Note: the field doesn't show the new text yet. Look (get_app_state) before typing again: typing again could enter the text twice.";

/// Appended when an action changed nothing that can be seen.
const NO_CHANGE_NOTE: &str = " Nothing on screen changed after it; check (get_app_state, screenshot=true) before repeating it.";

/// What the coordinates on a screenshot's grid (and `pick` points) are.
#[derive(Debug, Clone, Copy)]
enum LabelSpace {
    /// Screen coordinates (full screen and region screenshots).
    Screen,
    /// The x/y actions use for a window: pixels of its latest get_app_state
    /// screenshot.
    Map(CoordMap),
    /// This image's own pixels (no get_app_state of the window yet).
    Image,
    /// A document's own units or a math range (`canvas`).
    Frame(crate::draw::Frame),
}

impl LabelSpace {
    /// Label = offset + scale * capture pixel, per axis; `out_w` is the
    /// width the capture will be sent at.
    fn axes(&self, cap: &Capture, out_w: u32) -> (imaging::Axis, imaging::Axis) {
        let (cw, ch) = (f64::from(cap.width.max(1)), f64::from(cap.height.max(1)));
        let (px_x, px_y) = (cap.bounds.width / cw, cap.bounds.height / ch);
        match self {
            LabelSpace::Screen => (
                imaging::Axis {
                    offset: cap.bounds.x,
                    scale: px_x,
                },
                imaging::Axis {
                    offset: cap.bounds.y,
                    scale: px_y,
                },
            ),
            LabelSpace::Map(m) => {
                let kx = f64::from(m.width) / m.bounds.width.max(f64::MIN_POSITIVE);
                let ky = f64::from(m.height) / m.bounds.height.max(f64::MIN_POSITIVE);
                (
                    imaging::Axis {
                        offset: (cap.bounds.x - m.bounds.x) * kx,
                        scale: px_x * kx,
                    },
                    imaging::Axis {
                        offset: (cap.bounds.y - m.bounds.y) * ky,
                        scale: px_y * ky,
                    },
                )
            }
            LabelSpace::Frame(f) => {
                let (fx, fy) = f.scale();
                let (ox, oy) = f.to_frame(Point::new(cap.bounds.x, cap.bounds.y));
                (
                    imaging::Axis {
                        offset: ox,
                        scale: px_x / fx,
                    },
                    imaging::Axis {
                        offset: oy,
                        scale: px_y / fy,
                    },
                )
            }
            LabelSpace::Image => {
                let k = f64::from(out_w.max(1)) / cw;
                (
                    imaging::Axis {
                        offset: 0.0,
                        scale: k,
                    },
                    imaging::Axis {
                        offset: 0.0,
                        scale: k,
                    },
                )
            }
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            LabelSpace::Screen => {
                "screen coordinates (as region screenshots and window moves take them)"
            }
            LabelSpace::Map(_) => "the x/y that click, drag and draw use for this window",
            LabelSpace::Image => "this image's pixels (call get_app_state before acting with x/y)",
            LabelSpace::Frame(f) if f.y_up() => "the canvas range (math, y up), as draw takes it",
            LabelSpace::Frame(_) => "the document's units, as draw takes them with canvas",
        }
    }
}

/// A `draw` stroke as the drawing module takes it: one shape, or several
/// (axes and ticks, repeated copies).
pub(crate) fn draw_shapes(
    s: &DrawStroke,
    frame: &crate::draw::Frame,
) -> std::result::Result<Vec<crate::draw::Shape>, String> {
    use crate::draw::{self, Affine, Expr, Shape};
    let curve = s.x.is_some() || s.y.is_some();
    let kinds = [
        ("points", s.points.is_some()),
        ("x/y", curve),
        ("rect", s.rect.is_some()),
        ("ellipse", s.ellipse.is_some()),
        ("polygon", s.polygon.is_some()),
        ("star", s.star.is_some()),
        ("arc", s.arc.is_some()),
        ("bezier", s.bezier.is_some()),
        ("axes", s.axes.is_some()),
    ];
    let given: Vec<&str> = kinds.iter().filter(|k| k.1).map(|k| k.0).collect();
    match given.len() {
        0 => {
            return Err(
                "give a shape (rect, ellipse, polygon, star, arc, bezier), points, axes, or x and/or y expressions in t"
                    .into(),
            );
        }
        1 => {}
        _ => {
            return Err(format!(
                "give one kind of stroke, not {}",
                given.join(" and ")
            ));
        }
    }
    let finite = |v: &[f64], what: &str| -> std::result::Result<(), String> {
        if v.iter().all(|x| x.is_finite()) {
            Ok(())
        } else {
            Err(format!("{what} has a number that isn't finite"))
        }
    };
    let whole = |v: f64, what: &str, lo: u32, hi: u32| -> std::result::Result<u32, String> {
        if v.fract() == 0.0 && v >= f64::from(lo) && v <= f64::from(hi) {
            Ok(v as u32)
        } else {
            Err(format!("{what} must be a whole number from {lo} to {hi}"))
        }
    };
    let circle = |cx: f64, cy: f64, rx: f64, ry: f64, t0: f64, t1: f64| {
        Ok::<Shape, String>(Shape::Curve {
            x: Expr::parse(&format!("({cx:?}) + ({rx:?})*cos(t)"))?,
            y: Expr::parse(&format!("({cy:?}) + ({ry:?})*sin(t)"))?,
            t0,
            t1,
            steps: None,
            transform: Affine::IDENTITY,
        })
    };
    let base: Shape = if let Some(r) = &s.rect {
        if !(r.len() == 4 || r.len() == 5) {
            return Err(
                "rect is [x, y, width, height] or [x, y, width, height, corner radius]".into(),
            );
        }
        finite(r, "rect")?;
        let (x, y, w, h) = (r[0], r[1], r[2], r[3]);
        let radius = r.get(4).copied().unwrap_or(0.0);
        if w <= 0.0 || h <= 0.0 || radius < 0.0 {
            return Err("rect needs a positive width and height (and radius 0 or more)".into());
        }
        Shape::points(draw::rect(x, y, w, h, radius), true)
    } else if let Some(e) = s.ellipse {
        let [cx, cy, rx, ry] = e;
        finite(&e, "ellipse")?;
        if rx <= 0.0 || ry <= 0.0 {
            return Err(
                "ellipse is [center x, center y, radius x, radius y] with positive radii".into(),
            );
        }
        // Drawn from its rightmost point, all the way round.
        circle(cx, cy, rx, ry, 0.0, std::f64::consts::TAU)?
    } else if let Some(p) = s.polygon {
        let [cx, cy, r, n] = p;
        finite(&p, "polygon")?;
        if r <= 0.0 {
            return Err("polygon is [center x, center y, radius, corners], radius positive".into());
        }
        let n = whole(n, "polygon corners", 3, 1000)?;
        Shape::points(draw::polygon(cx, cy, r, n, frame.y_up()), true)
    } else if let Some(p) = s.star {
        let [cx, cy, outer, inner, n] = p;
        finite(&p, "star")?;
        if outer <= 0.0 || inner <= 0.0 {
            return Err(
                "star is [center x, center y, outer radius, inner radius, points], radii positive"
                    .into(),
            );
        }
        let n = whole(n, "star points", 2, 500)?;
        Shape::points(draw::star(cx, cy, outer, inner, n, frame.y_up()), true)
    } else if let Some(a) = s.arc {
        let [cx, cy, r, from, to] = a;
        finite(&a, "arc")?;
        if r <= 0.0 {
            return Err(
                "arc is [center x, center y, radius, from degrees, to degrees], radius positive"
                    .into(),
            );
        }
        circle(cx, cy, r, r, from.to_radians(), to.to_radians())?
    } else if let Some(b) = &s.bezier {
        let pts: Vec<(f64, f64)> = b.iter().map(|p| p.xy()).collect();
        finite(
            &pts.iter().flat_map(|p| [p.0, p.1]).collect::<Vec<_>>(),
            "bezier",
        )?;
        Shape::Points {
            points: draw::bezier(&pts)?,
            closed: s.closed,
            smooth: false,
            transform: Affine::IDENTITY,
        }
    } else if let Some(a) = s.axes {
        if !frame.y_up() {
            return Err("axes need canvas.range (math coordinates)".into());
        }
        if s.rotate.is_some() || s.repeat.is_some() {
            return Err("axes can't be turned or repeated".into());
        }
        finite(&a, "axes")?;
        return draw::axes(frame, a[0].max(0.0), a[1].max(0.0), 10.0);
    } else if let Some(points) = &s.points {
        Shape::Points {
            points: points.iter().map(|p| p.xy()).collect(),
            closed: s.closed,
            smooth: s.smooth,
            transform: Affine::IDENTITY,
        }
    } else {
        // A curve: x and y in t, or a plot of y in x (x in y) over the
        // whole area unless t says otherwise.
        let (x, y, default_t) = match (&s.x, &s.y) {
            (Some(x), Some(y)) => (
                Expr::parse(x).map_err(|e| format!("x: {e}"))?,
                Expr::parse(y).map_err(|e| format!("y: {e}"))?,
                (0.0, 1.0),
            ),
            (None, Some(y)) => (
                Expr::parse("t")?,
                Expr::parse_in(y, Some("x")).map_err(|e| format!("y: {e}"))?,
                (frame.x0, frame.x1),
            ),
            (Some(x), None) => (
                Expr::parse_in(x, Some("y")).map_err(|e| format!("x: {e}"))?,
                Expr::parse("t")?,
                (frame.y0, frame.y1),
            ),
            (None, None) => unreachable!("one of x and y is given"),
        };
        let bound = |v: &DrawNumber, which: &str| -> std::result::Result<f64, String> {
            match v {
                DrawNumber::Num(n) => Ok(*n),
                DrawNumber::Expr(e) => Expr::parse(e)
                    .map(|e| e.eval(0.0))
                    .map_err(|err| format!("t {which}: {err}")),
            }
        };
        let (t0, t1) = match &s.t {
            Some([a, b]) => (bound(a, "from")?, bound(b, "to")?),
            None => default_t,
        };
        if let Some(n) = s.steps
            && !(1..=10_000).contains(&n)
        {
            return Err("steps must be 1 to 10000".into());
        }
        Shape::Curve {
            x,
            y,
            t0,
            t1,
            steps: s.steps,
            transform: Affine::IDENTITY,
        }
    };
    let center = base.center().unwrap_or((0.0, 0.0));
    let mut shape = base;
    if let Some(deg) = s.rotate {
        let about = s.about.map(|p| p.xy()).unwrap_or(center);
        finite(&[deg, about.0, about.1], "rotate")?;
        shape = shape.then(frame.rotation(about, deg));
    }
    let Some(r) = &s.repeat else {
        return Ok(vec![shape]);
    };
    if r.count == 0 || r.count > 500 {
        return Err("repeat.count must be 1 to 500".into());
    }
    let [dx, dy] = r.offset.unwrap_or([0.0, 0.0]);
    let turn = r.rotate.unwrap_or(0.0);
    let about = r.about.map(|p| p.xy()).unwrap_or(center);
    finite(&[dx, dy, turn, about.0, about.1], "repeat")?;
    Ok((0..r.count)
        .map(|k| {
            let k = f64::from(k);
            shape
                .then(Affine::translate(k * dx, k * dy))
                .then(frame.rotation(about, k * turn))
        })
        .collect())
}

/// The box a drawing covers, in its frame's units.
fn plan_span(frame: &crate::draw::Frame, plan: &crate::draw::Plan) -> Option<crate::cells::Span> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in plan.strokes.iter().flatten() {
        let (x, y) = frame.to_frame(*p);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    (x0 <= x1).then_some(crate::cells::Span { x0, x1, y0, y1 })
}

/// Cells for a drawing on `frame`: sized to the drawing when it is small
/// in the area (and then `true`), else to the whole area.
fn drawing_cells(
    frame: &crate::draw::Frame,
    target: Option<crate::cells::Span>,
    cell_size: Option<f64>,
) -> (crate::cells::Cells, bool) {
    // The size the model chose (checked before drawing) is kept as is.
    if let Ok(c) = frame_cells(frame, None, cell_size)
        && cell_size.is_some()
    {
        return (c, false);
    }
    let (w, h) = ((frame.x1 - frame.x0).abs(), (frame.y1 - frame.y0).abs());
    let small = target.filter(|t| {
        let (tw, th) = (t.x1 - t.x0, t.y1 - t.y0);
        tw.max(th) > 0.0 && tw < w * 0.35 && th < h * 0.35
    });
    (
        crate::cells::Cells::new(frame.x0, frame.x1, frame.y0, frame.y1, frame.y_up(), small),
        small.is_some(),
    )
}

/// Cells over a drawing area: `cell_size` units square when given, else
/// about eight across `target` (or the whole area).
fn frame_cells(
    frame: &crate::draw::Frame,
    target: Option<crate::cells::Span>,
    cell_size: Option<f64>,
) -> Result<crate::cells::Cells> {
    let Some(s) = cell_size else {
        return Ok(crate::cells::Cells::new(
            frame.x0,
            frame.x1,
            frame.y0,
            frame.y1,
            frame.y_up(),
            target,
        ));
    };
    if !(s.is_finite() && s > 0.0) {
        return Err(Error::InvalidArgs(
            "cell_size is the cells' size in the canvas's units, above 0".into(),
        ));
    }
    let c = crate::cells::Cells::with_step(frame.x0, frame.x1, frame.y0, frame.y1, frame.y_up(), s);
    let most = crate::design::MAX_CELLS;
    if c.cols as f64 > most || c.rows as f64 > most {
        return Err(Error::InvalidArgs(format!(
            "cells of {s} make {} x {} over the canvas; at most {most} across (give a bigger cell_size)",
            c.cols, c.rows
        )));
    }
    Ok(c)
}

/// A screen rectangle in a capture's pixels.
fn capture_rect(cap: &Capture, r: Rect) -> Rect {
    let sx = f64::from(cap.width) / cap.bounds.width.max(1e-9);
    let sy = f64::from(cap.height) / cap.bounds.height.max(1e-9);
    Rect::new(
        (r.x - cap.bounds.x) * sx,
        (r.y - cap.bounds.y) * sy,
        r.width * sx,
        r.height * sy,
    )
}

/// One cell of a document up close: the cell and a little around it,
/// magnified, with a fine grid in the document's units and the cell's edges
/// in blue, and what it says about it.
fn cell_view(
    cap: &Capture,
    ax: imaging::Axis,
    ay: imaging::Axis,
    cells: &crate::cells::Cells,
    col: usize,
    row: usize,
) -> Result<(Capture, String)> {
    let sp = cells.span(col, row);
    let pad = cells.step * 0.15;
    let px = |a: imaging::Axis, v0: f64, v1: f64| {
        let (p0, p1) = (a.pixel(v0), a.pixel(v1));
        (p0.min(p1), p0.max(p1))
    };
    let (x0, x1) = px(ax, sp.x0 - pad, sp.x1 + pad);
    let (y0, y1) = px(ay, sp.y0 - pad, sp.y1 + pad);
    let (w, h) = (f64::from(cap.width), f64::from(cap.height));
    let (x0, y0) = (x0.floor().clamp(0.0, w), y0.floor().clamp(0.0, h));
    let (x1, y1) = (x1.ceil().clamp(0.0, w), y1.ceil().clamp(0.0, h));
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return Err(Error::InvalidArgs(format!(
            "cell {} is not on the screen",
            crate::cells::Cells::name(col, row)
        )));
    }
    let part = imaging::crop(
        cap,
        (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32),
    );
    // Its colours, from the cell alone.
    let (cx0, cx1) = px(ax, sp.x0, sp.x1);
    let (cy0, cy1) = px(ay, sp.y0, sp.y1);
    let inner = imaging::crop(
        cap,
        (
            cx0.max(0.0) as u32,
            cy0.max(0.0) as u32,
            (cx1 - cx0).max(1.0) as u32,
            (cy1 - cy0).max(1.0) as u32,
        ),
    );
    let k = (512 / part.width.max(part.height).max(1)).clamp(1, 16);
    let mut pic = imaging::magnify(&part, k);
    let kf = f64::from(k);
    let (bx, by) = (ax.label(x0), ay.label(y0));
    let (fx, fy) = (
        imaging::Axis {
            offset: bx,
            scale: ax.scale / kf,
        },
        imaging::Axis {
            offset: by,
            scale: ay.scale / kf,
        },
    );
    let fine = imaging::nice_step(cells.step, 10.0);
    let used = imaging::draw_grid(&mut pic, fx, fy, fine, 1.0, None);
    let (ex0, ex1) = px(fx, sp.x0, sp.x1);
    let (ey0, ey1) = px(fy, sp.y0, sp.y1);
    for t in 0..2 {
        for x in ex0 as i64..=ex1 as i64 {
            imaging::blend(&mut pic, x, ey0 as i64 + t, [40, 100, 210], 0.9);
            imaging::blend(&mut pic, x, ey1 as i64 - t, [40, 100, 210], 0.9);
        }
        for y in ey0 as i64..=ey1 as i64 {
            imaging::blend(&mut pic, ex0 as i64 + t, y, [40, 100, 210], 0.9);
            imaging::blend(&mut pic, ex1 as i64 - t, y, [40, 100, 210], 0.9);
        }
    }
    let colours: Vec<String> = imaging::palette(&inner, 5)
        .iter()
        .map(|(hex, share)| format!("{hex} {:.0}%", share * 100.0))
        .collect();
    let n = |v: f64| imaging::grid_label(v, fine);
    let text = format!(
        "Cell {} of the document: x {} to {}, y {} to {} (shown {k} times bigger, a grid line every {}, in the document's units; its edges in blue). Main colours: {}.",
        crate::cells::Cells::name(col, row),
        n(sp.x0),
        n(sp.x1),
        n(sp.y0),
        n(sp.y1),
        n(used),
        colours.join(", ")
    );
    Ok((pic, text))
}

/// The capture pixel an x/y point (screenshot pixels) falls on.
fn to_capture(cap: &Capture, map: &CoordMap, x: f64, y: f64) -> Result<(f64, f64)> {
    let p = map.to_screen(x, y)?;
    let b = cap.bounds;
    Ok((
        (p.x - b.x) * f64::from(cap.width) / b.width.max(1e-9),
        (p.y - b.y) * f64::from(cap.height) / b.height.max(1e-9),
    ))
}

/// Back from a capture pixel to the x/y actions take.
fn from_capture(cap: &Capture, map: &CoordMap, p: (f64, f64)) -> (f64, f64) {
    let b = cap.bounds;
    map.to_image(Point::new(
        b.x + p.0 * b.width / f64::from(cap.width.max(1)),
        b.y + p.1 * b.height / f64::from(cap.height.max(1)),
    ))
}

/// "2.5 right and 1.0 up", or "not moved".
fn moved(dx: f64, dy: f64) -> String {
    let mut parts = Vec::new();
    if dx.abs() >= 0.05 {
        parts.push(format!(
            "{:.1} {}",
            dx.abs(),
            if dx > 0.0 { "right" } else { "left" }
        ));
    }
    if dy.abs() >= 0.05 {
        parts.push(format!(
            "{:.1} {}",
            dy.abs(),
            if dy > 0.0 { "down" } else { "up" }
        ));
    }
    if parts.is_empty() {
        "where you pointed".to_string()
    } else {
        format!("{} from where you pointed", parts.join(" and "))
    }
}

/// Whether a `draw` stroke gives a shape of its own.
fn shape_given(s: &DrawStroke) -> bool {
    s.points.is_some()
        || s.x.is_some()
        || s.y.is_some()
        || s.rect.is_some()
        || s.ellipse.is_some()
        || s.polygon.is_some()
        || s.star.is_some()
        || s.arc.is_some()
        || s.bezier.is_some()
        || s.axes.is_some()
}

/// A design's name as it is stored: lower case, letters, digits and '-'.
fn design_key(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(40)
        .collect()
}

/// For a mutating tool, the app to re-inspect afterwards (change reporting).
/// What changed between two listings of a design's layers or a scene's
/// objects (by id, in order): None when nothing did.
fn items_diff(old: &[(String, String)], new: &[(String, String)]) -> Option<String> {
    let before: HashMap<&str, &str> = old.iter().map(|(i, l)| (i.as_str(), l.as_str())).collect();
    let now: HashSet<&str> = new.iter().map(|(i, _)| i.as_str()).collect();
    let changed: Vec<&str> = new
        .iter()
        .filter(|(i, l)| before.get(i.as_str()).is_some_and(|o| o != l))
        .map(|(_, l)| l.as_str())
        .collect();
    let added: Vec<&str> = new
        .iter()
        .filter(|(i, _)| !before.contains_key(i.as_str()))
        .map(|(_, l)| l.as_str())
        .collect();
    let removed: Vec<&str> = old
        .iter()
        .filter(|(i, _)| !now.contains(i.as_str()))
        .map(|(i, _)| i.as_str())
        .collect();
    let mut parts = Vec::new();
    if !changed.is_empty() {
        parts.push(format!("changed {}", changed.join("; ")));
    }
    if !added.is_empty() {
        parts.push(format!("added {}", added.join("; ")));
    }
    if !removed.is_empty() {
        parts.push(format!("removed {}", removed.join(", ")));
    }
    // The order, when it isn't the old one with the new ones on top.
    let expected: Vec<&str> = old
        .iter()
        .map(|(i, _)| i.as_str())
        .filter(|i| now.contains(i))
        .chain(
            new.iter()
                .map(|(i, _)| i.as_str())
                .filter(|i| !before.contains_key(i)),
        )
        .collect();
    let order: Vec<&str> = new.iter().map(|(i, _)| i.as_str()).collect();
    if order != expected {
        parts.push(format!("back to front now {}", order.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join(". "))
}

/// A picture's identity, to tell whether it is the one sent last.
fn picture_hash(c: &Capture) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (c.width, c.height).hash(&mut h);
    c.rgba.hash(&mut h);
    h.finish()
}

/// The app, what is expected and the element acted on, of an action with
/// `expect`.
fn expectation(call: &ToolCall) -> Option<(String, String, Option<u32>)> {
    let (app, expect, index) = match call {
        ToolCall::Click(a) => (&a.app, a.expect.as_ref()?, a.element_index),
        ToolCall::PerformSecondaryAction(a) => (&a.app, a.expect.as_ref()?, Some(a.element_index)),
        ToolCall::SetValue(a) => (&a.app, a.expect.as_ref()?, Some(a.element_index)),
        ToolCall::PressKey(a) => (&a.app, a.expect.as_ref()?, a.element_index),
        ToolCall::TypeText(a) => (&a.app, a.expect.as_ref()?, a.element_index),
        _ => return None,
    };
    let expect = expect.trim();
    (!expect.is_empty()).then(|| (app.clone(), expect.to_string(), index))
}

fn mutating_app(call: &ToolCall) -> Option<String> {
    match call {
        ToolCall::Click(a) => Some(a.app.clone()),
        ToolCall::PerformSecondaryAction(a) => Some(a.app.clone()),
        ToolCall::SetValue(a) => Some(a.app.clone()),
        ToolCall::SelectText(a) => Some(a.app.clone()),
        ToolCall::Scroll(a) => Some(a.app.clone()),
        ToolCall::Drag(a) => Some(a.app.clone()),
        ToolCall::Draw(a) if !a.preview => Some(a.app.clone()),
        ToolCall::PressKey(a) => Some(a.app.clone()),
        ToolCall::TypeText(a) => Some(a.app.clone()),
        _ => None,
    }
}

/// A hash of a rendered tree, to tell whether it is the same text.
/// Whether `read` (text read off the screen) is the start of `label`, with
/// at most one misread character in six ("zero is thir" for "zero is
/// thinking").
fn reads_as_start_of(read: &str, label: &str) -> bool {
    let a: Vec<char> = read.chars().collect();
    let b: Vec<char> = label.chars().take(a.len()).collect();
    if b.len() < a.len() {
        return false;
    }
    // Levenshtein distance between the two.
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()] <= (a.len() / 6).max(usize::from(a.len() >= 6))
}

/// A hash of a picture's exact pixels (and size).
fn pixels_hash(cap: &Capture) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (cap.width, cap.height).hash(&mut h);
    cap.rgba.hash(&mut h);
    h.finish()
}

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Give memory freed by a big call back to the system. glibc keeps freed
/// heap (and a finished thread's arena) for reuse, so without this the
/// server would stay at its largest size; elsewhere the allocator returns
/// it by itself.
fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only releases free memory; it is safe to call at
    // any time.
    unsafe {
        libc::malloc_trim(0);
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

/// The text an `expect` waits for, when it waits for a text rather than
/// for one of the kinds `expect_now` knows (a dialog, a menu, a change, a
/// value, gone).
fn expected_text(what: &str) -> Option<String> {
    const KINDS: &[&str] = &[
        "gone",
        "goes",
        "closed",
        "close",
        "closes",
        "disappear",
        "disappears",
        "dialog",
        "window",
        "new window",
        "popup",
        "pop-up",
        "sheet",
        "alert",
        "menu",
        "a menu",
        "change",
        "changes",
        "a change",
        "anything",
        "value",
        "new value",
        "value changes",
        "checked",
        "selected",
    ];
    let raw = what.trim().trim_matches('"').trim();
    let folded = crate::text::fold(raw);
    if KINDS.contains(&folded.trim()) {
        return None;
    }
    let text = match raw.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("text ") => &raw[5..],
        _ => raw,
    };
    let text = text.trim().trim_matches('"').trim();
    (!text.is_empty()).then(|| text.to_string())
}
/// Lowercased name + value of a node, for text matching.
/// An element's name and value, [folded](crate::text::fold) for matching.
fn node_text(n: &Node) -> String {
    let mut s = n.name.clone().unwrap_or_default();
    if let Some(v) = &n.value {
        s.push(' ');
        s.push_str(v);
    }
    crate::text::fold(&s)
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

/// The app whose pixels a call uses (x/y, a picture asked for), if any:
/// [screenshot] adaptive keeps sending automatic screenshots there.
fn pixel_use(call: &ToolCall) -> Option<&str> {
    let xy = |x: Option<f64>, i: Option<u32>| x.is_some() && i.is_none();
    match call {
        ToolCall::Click(a) if xy(a.x, a.element_index) => Some(&a.app),
        ToolCall::Scroll(a) if xy(a.x, a.element_index) => Some(&a.app),
        ToolCall::PressKey(a) if a.x.is_some() => Some(&a.app),
        ToolCall::TypeText(a) if a.x.is_some() => Some(&a.app),
        ToolCall::Drag(a) if a.from_x.is_some() || a.to_x.is_some() => Some(&a.app),
        ToolCall::Draw(a) => Some(&a.app),
        ToolCall::Locate(a) => Some(&a.app),
        ToolCall::GetAppState(a) if a.screenshot == Some(true) => Some(&a.app),
        ToolCall::Screenshot(a) => a.app.as_deref(),
        _ => None,
    }
}

fn mode_name(m: ScreenshotMode) -> &'static str {
    match m {
        ScreenshotMode::Auto => "auto",
        ScreenshotMode::Full => "full",
        ScreenshotMode::Region => "region",
        ScreenshotMode::Window => "window",
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
/// Of several processes of one app, the frontmost one (else the first).
fn many_of_one(apps: &[&AppInfo]) -> AppInfo {
    let a = apps.iter().find(|a| a.frontmost).unwrap_or(&apps[0]);
    (*a).clone()
}

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
        // Same name twice: one app, several processes.
        return Ok(many_of_one(&exact));
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
        .filter(|a| {
            let q = crate::text::fold(&ql);
            crate::text::fold(&a.name).contains(&q) || a.id.to_lowercase().contains(&ql)
        })
        .collect();
    match sub.as_slice() {
        [a] => Ok((*a).clone()),
        [] => Err(Error::AppNotFound(query.into())),
        // Several windows of one app (two Notepads): the frontmost one.
        [first, rest @ ..]
            if rest
                .iter()
                .all(|a| a.name.eq_ignore_ascii_case(&first.name)) =>
        {
            Ok(many_of_one(&sub))
        }
        // Several different apps: never guess, ask for the exact one.
        many => Err(Error::AmbiguousApp {
            query: query.into(),
            candidates: many
                .iter()
                .map(|a| format!("{} (pid {})", a.name, a.pid))
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::mock::{Event, MockBackend};
    use crate::tree::{expand, index_of};

    /// An engine whose `design` lists its paint steps with every answer.
    fn steps_engine() -> Engine<MockBackend> {
        let mut e = engine();
        let mut cfg = e.store().config.clone();
        cfg.tools.design_steps = crate::config::DesignSteps::Always;
        e.set_config(ConfigStore::in_memory(cfg));
        e
    }

    fn engine() -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let cfg = Config::default();
        let mut e = Engine::new(backend, ConfigStore::in_memory(cfg));
        // Deterministic, instant time.
        e = e.with_time(Instant::now, |_| {});
        e
    }

    #[test]
    fn list_apps_and_state() {
        let mut e = engine();
        let out = e.call(ToolCall::ListApps).unwrap();
        assert!(out.text.contains("TextEdit"));

        let out = e
            .call(ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            }))
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
        e.call(ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        }))
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
        e.call(ToolCall::Click(ClickArgs {
            app: "TextEdit".into(),
            element_index: Some(bold),
            ..Default::default()
        }))
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Action(3, a) if a == "AXPress"
        ));

        // Right click -> coordinate path.
        e.call(ToolCall::Click(ClickArgs {
            app: "TextEdit".into(),
            element_index: Some(bold),
            button: MouseButton::Right,
            ..Default::default()
        }))
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, _, MouseButton::Right, 1)
        ));
    }

    #[test]
    fn coordinate_click_maps_through_screenshot() {
        let mut e = engine();
        e.call(ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        }))
        .unwrap();
        e.call(ToolCall::Click(ClickArgs {
            app: "TextEdit".into(),
            x: Some(400.0),
            y: Some(300.0),
            ..Default::default()
        }))
        .unwrap();
        // Window is 800x600; screenshot fits to 800x600 (<=1280) so 1:1.
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, p, _, _) if (p.x-400.0).abs()<1.0 && (p.y-300.0).abs()<1.0
        ));
    }

    #[test]
    fn coordinates_follow_a_window_that_moved() {
        let mut e = engine();
        let mut cfg = e.store().config.clone();
        cfg.cache.snapshot_ttl_ms = 0;
        cfg.timing.app_cache_ms = 0;
        e.set_config(ConfigStore::in_memory(cfg));
        let look = || {
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                ..Default::default()
            })
        };
        e.call(look()).unwrap();
        // The user drags the window 300 px right: the tree reads the same.
        e.backend_mut().app_mut(4242).unwrap().windows[0].bounds.x += 300.0;
        // No new screenshot comes with this look: the old one must follow.
        let out = e
            .call(ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                screenshot: Some(false),
                ..Default::default()
            }))
            .unwrap();
        assert!(out.image.is_none());
        e.call(ToolCall::Click(ClickArgs {
            app: "TextEdit".into(),
            x: Some(400.0),
            y: Some(300.0),
            ..Default::default()
        }))
        .unwrap();
        assert!(
            matches!(
                e.backend().events.last().unwrap(),
                Event::Click(4242, p, _, _) if (p.x - 700.0).abs() < 1.0 && (p.y - 300.0).abs() < 1.0
            ),
            "{:?}",
            e.backend().events.last()
        );
    }

    #[test]
    fn looking_at_another_window_leaves_the_screenshot_coordinates_alone() {
        let mut e = engine();
        {
            let app = e.backend_mut().app_mut(4242).unwrap();
            let mut other = app.windows[0].clone();
            other.id = 2;
            other.title = "Other".into();
            other.root = 99;
            other.focused = false;
            other.bounds.x += 300.0;
            other.bounds.y += 200.0;
            app.elements
                .push(MockElement::new(99, "window", "Other", other.bounds));
            app.windows.push(other);
        }
        let look = |window: &str, shot: bool| {
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                window: Some(window.into()),
                screenshot: Some(shot),
                ..Default::default()
            })
        };
        let out = e.call(look("Untitled", true)).unwrap();
        assert!(out.image.is_some(), "{}", out.text);
        e.call(look("Other", false)).unwrap();
        // x/y still mean the picture of "Untitled".
        e.call(ToolCall::Click(ClickArgs {
            app: "TextEdit".into(),
            x: Some(400.0),
            y: Some(300.0),
            ..Default::default()
        }))
        .unwrap();
        assert!(
            matches!(
                e.backend().events.last().unwrap(),
                Event::Click(4242, p, _, _) if (p.x - 400.0).abs() < 1.0 && (p.y - 300.0).abs() < 1.0
            ),
            "{:?}",
            e.backend().events.last()
        );
    }

    #[test]
    fn a_call_whose_answer_was_dropped_counts_as_unseen() {
        let mut backend = MockBackend::new();
        backend.add_app(page_a(7));
        backend.on_press.insert(20, page_b(7));
        let mut cfg = Config::default();
        cfg.tree.report_changes_max_lines = 4;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        // The client cancels the click; its answer never reaches the model.
        let shown = e.shown();
        press_named(&mut e, 7, "Next");
        e.not_delivered(shown);
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(!out.text.contains("the rest:"), "{}", out.text);
        assert!(out.text.contains("button \"Back\""), "{}", out.text);
        assert!(
            out.image.is_some(),
            "the model has no picture of it: {}",
            out.text
        );
    }

    #[test]
    fn set_value_and_type_and_select() {
        let mut e = engine();
        e.call(ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        }))
        .unwrap();
        let doc = e
            .state(4242)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.role == "text area")
            .unwrap()
            .index;
        e.call(ToolCall::SetValue(SetValueArgs {
            app: "TextEdit".into(),
            element_index: doc,
            value: "World".into(),
            ..Default::default()
        }))
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::SetValue(5, v) if v == "World"
        ));
        e.call(ToolCall::SelectText(SelectTextArgs {
            app: "TextEdit".into(),
            element_index: doc,
            text: Some("or".into()),
            occurrence: 1,
            ..Default::default()
        }))
        .unwrap();
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::SelectText(5, Some(t), 1) if t == "or"
        ));
    }

    #[test]
    fn press_key_sequences() {
        let mut e = engine();
        e.call(ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        }))
        .unwrap();
        e.call(ToolCall::PressKey(PressKeyArgs {
            app: "TextEdit".into(),
            key: "cmd+a Delete".into(),
            ..Default::default()
        }))
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
        // "cmd" is the shortcut key: Cmd on a Mac, Ctrl elsewhere.
        let select_all = if cfg!(target_os = "macos") {
            "meta+a"
        } else {
            "ctrl+a"
        };
        assert_eq!(keys, vec![select_all, "Delete"]);
    }

    /// An engine with TextEdit (pid 4242) behind another app that is in front.
    fn behind_engine() -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        let mut app = MockBackend::text_editor(4242);
        app.info.frontmost = false;
        backend.add_app(app);
        let mut front = MockBackend::text_editor(77);
        front.info.name = "Terminal".into();
        front.info.id = "com.apple.Terminal".into();
        backend.add_app(front);
        Engine::new(backend, ConfigStore::in_memory(Config::default()))
            .with_time(Instant::now, |_| {})
    }

    fn keys_sent(e: &Engine<MockBackend>) -> Vec<(u32, String)> {
        e.backend()
            .events
            .iter()
            .filter_map(|ev| match ev {
                Event::Key(pid, k) => Some((*pid, k.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn keys_go_to_the_app_only_once_it_is_in_front() {
        let mut e = behind_engine();
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "Return"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            e.backend()
                .window_ops
                .iter()
                .any(|(_, op)| *op == WindowOp::Focus),
            "brought to the front first"
        );
        assert_eq!(keys_sent(&e), vec![(4242, "Return".to_string())]);
    }

    #[test]
    fn no_input_when_the_app_cannot_come_to_the_front() {
        let mut e = behind_engine();
        e.backend_mut().refuse_focus = true;
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "text": "rm -rf ~\n"}),
        );
        assert!(out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Terminal, which is in front"),
            "{}",
            out.text
        );
        assert!(keys_sent(&e).is_empty(), "nothing typed anywhere");
        assert!(
            !e.backend()
                .events
                .iter()
                .any(|ev| matches!(ev, Event::Type(..))),
            "nothing typed anywhere"
        );
    }

    #[test]
    fn unknown_index_is_a_tool_error() {
        let mut e = engine();
        e.call(ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        }))
        .unwrap();
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": 999}),
        );
        assert!(out.is_error);
        assert!(out.text.contains("Element indices are only valid"));
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
        let cfg = Config::default();
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let out = e
            .call(ToolCall::LaunchApp(LaunchAppArgs {
                app: "Notes".into(),
            }))
            .unwrap();
        assert!(out.text.contains("Launched Notes"));
        // Its first state comes with it ([tools] launch_look).
        assert!(out.text.contains("text area \"Document\""), "{}", out.text);
        assert!(out.image.is_some());
        let out = e.call_tool("get_app_state", serde_json::json!({"app": "Notes"}));
        assert!(!out.is_error);
        assert!(!out.text.contains("Document"), "already sent: {}", out.text);
    }

    #[test]
    fn launch_by_catalog_name_finds_the_program_that_ran() {
        let mut backend = MockBackend::new();
        // "Visual Studio Code" is the menu name; the app runs as "Code".
        backend.add_launchable("visual studio code", {
            let mut a = MockBackend::text_editor(56);
            a.info.name = "Code".into();
            a.info.id = "code".into();
            a.info.exe = Some("/usr/share/code/code".into());
            a
        });
        let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
            .with_time(Instant::now, |_| {});
        let out = e.call_tool(
            "launch_app",
            serde_json::json!({"app": "Visual Studio Code"}),
        );
        assert!(out.text.contains("Launched Code (id: code"), "{}", out.text);
    }

    #[test]
    fn launch_app_never_takes_a_command_line() {
        let mut e = engine();
        // The whole string is the program; it is never split into arguments.
        let out = e.call_tool(
            "launch_app",
            serde_json::json!({"app": "xterm -e sh -c id"}),
        );
        assert!(out.is_error);
        assert!(
            e.backend()
                .events
                .iter()
                .any(|ev| matches!(ev, Event::Launch(q) if q == "xterm -e sh -c id"))
        );
        // Nothing that could read as an option.
        for app in [" --args x", "notes\n--args"] {
            let out = e.call_tool("launch_app", serde_json::json!({ "app": app }));
            assert!(
                out.is_error && out.text.contains("no options"),
                "{app:?}: {}",
                out.text
            );
        }
    }

    #[test]
    fn screenshot_regions_off_the_screen_are_refused_not_fatal() {
        let mut e = engine();
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "region", "x": 1.0e9, "y": 5, "width": 1.0e12, "height": 10}),
        );
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("not on any display"), "{}", out.text);
        // Partly on a display: the visible part is captured.
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "region", "x": -50, "y": -50, "width": 100, "height": 100}),
        );
        assert!(!out.is_error, "{}", out.text);
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
        // Part of two different apps' names: never a guess, even for the
        // frontmost one.
        assert!(matches!(
            resolve_app_in(&apps, "afari"),
            Err(Error::AmbiguousApp { .. })
        ));
        assert!(resolve_app_in(&apps, "Firefox").is_err());
        // Two processes of one app: the frontmost.
        let note = |pid, frontmost| AppInfo {
            name: "Notepad".into(),
            id: format!("notepad-{pid}"),
            pid,
            exe: None,
            frontmost,
            hidden: false,
        };
        let two = vec![note(7, false), note(8, true)];
        assert_eq!(resolve_app_in(&two, "notep").unwrap().pid, 8);
        assert_eq!(resolve_app_in(&two, "Notepad").unwrap().pid, 8);
    }

    #[test]
    fn find_element_filters() {
        let mut e = engine();
        let out = e
            .call_tool(
                "find_element",
                serde_json::json!({"app": "TextEdit", "role": "button"}),
            )
            .text;
        assert!(out.contains("Bold"), "{out}");
        assert!(!out.contains("text area"), "{out}");

        let out = e.call_tool(
            "find_element",
            serde_json::json!({"app": "TextEdit", "editable": true}),
        );
        assert!(out.text.contains("text area"), "{}", out.text);
    }

    #[test]
    fn wait_for_finds_immediately() {
        let mut e = engine();
        let out = e.call_tool(
            "wait_for",
            serde_json::json!({"app": "TextEdit", "name": "Bold", "timeout_ms": 500}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Found after waiting"));

        let out = e.call_tool(
            "wait_for",
            serde_json::json!({"app": "TextEdit", "name": "Nonexistent", "timeout_ms": 60, "poll_ms": 20})
        );
        assert!(out.is_error);
        assert!(out.text.contains("timed out"));
    }

    #[test]
    fn clipboard_round_trips() {
        let mut e = engine();
        let out = e.call_tool("set_clipboard", serde_json::json!({"text": "hello clip"}));
        assert!(!out.is_error);
        let out = e.call_tool("get_clipboard", serde_json::json!({}));
        assert!(out.text.contains("hello clip"), "{}", out.text);
    }

    #[test]
    fn screenshot_full_and_region() {
        let mut e = engine();
        let out = e.call_tool("screenshot", serde_json::json!({"mode": "full"}));
        assert!(out.image.is_some());
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "region", "x": 0, "y": 0, "width": 100, "height": 50}),
        );
        assert!(out.image.is_some());
        // Region without full coords is an error.
        let out = e.call_tool("screenshot", serde_json::json!({"mode": "region", "x": 0}));
        assert!(out.is_error);
    }

    #[test]
    fn screenshot_window_annotated() {
        let mut e = engine();
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"mode": "window", "app": "TextEdit", "annotate": true}),
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
        );
        assert!(out.is_error);
        assert!(
            !out.text.contains("2. get_app_state"),
            "should stop early: {}",
            out.text
        );
    }

    #[test]
    fn change_report_appended_after_action() {
        let mut e = engine();
        e.call_tool("get_app_state", serde_json::json!({"app": "TextEdit"}));
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
        let out = e.call_tool("get_app_state", args);
        assert!(!out.is_error, "{}", out.text);
        out
    }

    fn press(e: &mut Engine<MockBackend>, index: u32) -> ToolOutput {
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": index}),
        );
        assert!(!out.is_error, "{}", out.text);
        out
    }

    fn press_named(e: &mut Engine<MockBackend>, pid: u32, name: &str) -> ToolOutput {
        let i = index_named(e, pid, name);
        press(e, i)
    }

    #[test]
    fn lines_an_action_already_showed_are_not_sent_again() {
        let mut backend = MockBackend::new();
        backend.add_app(page_a(7));
        backend.on_press.insert(20, page_b(7));
        let mut cfg = Config::default();
        cfg.tree.report_changes_max_lines = 4;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        let out = press_named(&mut e, 7, "Next");
        assert!(
            out.text.contains("more lines; call get_app_state"),
            "{}",
            out.text
        );
        let shown: Vec<String> = out
            .text
            .lines()
            .skip_while(|l| !l.starts_with("State after the action"))
            .skip(1)
            .take(4)
            .map(String::from)
            .collect();
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("the rest:"), "{}", out.text);
        for line in &shown {
            assert!(
                !out.text.contains(line.as_str()),
                "{line} again:\n{}",
                out.text
            );
        }
        assert!(out.text.contains("Result 3"), "{}", out.text);

        // Asked for in full, or changed meanwhile: all of it.
        let mut e2 = nav_engine(true);
        e2.store.config.tree.report_changes_max_lines = 4;
        state_of(&mut e2, serde_json::json!({}));
        press_named(&mut e2, 7, "Next");
        let out = state_of(&mut e2, serde_json::json!({"disable_diff": true}));
        assert!(!out.text.contains("the rest:"), "{}", out.text);
    }

    #[test]
    fn a_settings_file_caught_half_saved_is_not_taken() {
        let dir = std::env::temp_dir().join(format!("cu-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[tools]\ndisabled = [\"drag\"]\n").unwrap();
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut e = Engine::new(backend, ConfigStore::load(Some(&path)).unwrap())
            .with_time(Instant::now, |_| {});
        e.reload_if_changed();
        assert!(!e.store.config.tools.is_enabled("drag"));
        let touch = |secs: u64| {
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(std::time::SystemTime::now() + Duration::from_secs(secs))
                .unwrap();
        };
        // An editor empties the file before writing it.
        std::fs::write(&path, "").unwrap();
        touch(5);
        e.reload_if_changed();
        assert!(!e.store.config.tools.is_enabled("drag"), "taken half-saved");
        // Still empty the next time: the user emptied it.
        e.reload_if_changed();
        assert!(e.store.config.tools.is_enabled("drag"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_returning_screen_that_looks_different_gets_a_new_picture() {
        let mut e = nav_engine(false);
        state_of(&mut e, serde_json::json!({}));
        let next = index_named(&e, 7, "Next");
        press(&mut e, next);
        state_of(&mut e, serde_json::json!({}));
        let back = index_named(&e, 7, "Back");
        press(&mut e, back);
        // Same elements as before, different pixels (say, a new image).
        e.backend_mut().fill = 40;
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("screen #1 (seen before)"), "{}", out.text);
        assert!(out.image.is_some(), "{}", out.text);
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
        // The pixels are checked, not assumed: the same picture isn't sent.
        assert!(out.image.is_none(), "no screenshot re-sent");
        assert_eq!(e.backend().captures, captures + 1, "checked once");
        assert!(
            out.text.contains("unchanged since you last saw it"),
            "{}",
            out.text
        );
        // The indices the model analysed are back.
        assert_eq!(index_named(&e, 7, "Document"), doc);
        assert_eq!(index_named(&e, 7, "Next"), next);
        // An index from page B is refused rather than hitting something else.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": back}),
        );
        assert!(out.is_error, "{}", out.text);

        // Coordinates from page A's screenshot still map (1:1 here).
        e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "x": 100, "y": 100}),
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
        assert!(out.text.contains("nothing changed"), "{}", out.text);
        assert!(out.image.is_none());
    }

    fn pointer_events(e: &Engine<MockBackend>) -> Vec<Event> {
        e.backend()
            .events
            .iter()
            .filter(|ev| {
                matches!(
                    ev,
                    Event::PointerDown(..) | Event::PointerMove(..) | Event::PointerUp(..)
                )
            })
            .cloned()
            .collect()
    }

    #[test]
    fn draw_follows_a_parametric_curve() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [
                {"x": "400 + 100*cos(t)", "y": "300 + 100*sin(t)", "t": [0, "2*pi"]}
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Drew 1 stroke ("), "{}", out.text);
        assert!(
            out.text.contains("x 300 to 500, y 200 to 400"),
            "{}",
            out.text
        );
        let events = pointer_events(&e);
        let Event::PointerDown(4242, start, MouseButton::Left) = events[0] else {
            panic!("{:?}", events[0]);
        };
        assert!((start.x - 500.0).abs() < 1e-6 && (start.y - 300.0).abs() < 1e-6);
        let Event::PointerUp(_, end, _) = events.last().unwrap() else {
            panic!("ends with the button up");
        };
        assert!((end.x - start.x).abs() < 1e-6 && (end.y - start.y).abs() < 1e-6);
        let moves: Vec<Point> = events
            .iter()
            .filter_map(|ev| match ev {
                Event::PointerMove(_, p) => Some(*p),
                _ => None,
            })
            .collect();
        assert!(moves.len() > 150, "{}", moves.len());
        for p in &moves {
            let r = (p.x - 400.0).hypot(p.y - 300.0);
            assert!((r - 100.0).abs() < 0.5, "{p:?}");
        }
    }

    #[test]
    fn draw_in_an_element_uses_fractions_of_its_box() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let doc = index_named(&e, 4242, "Document");
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "button": "right",
                "strokes": [{"points": [[0, 0], {"x": 1, "y": 1}]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        let events = pointer_events(&e);
        // The text area is (0, 40, 800, 560) on screen.
        assert_eq!(
            events[0],
            Event::PointerDown(4242, Point::new(0.0, 40.0), MouseButton::Right)
        );
        assert_eq!(
            *events.last().unwrap(),
            Event::PointerUp(4242, Point::new(800.0, 600.0), MouseButton::Right)
        );
        assert!(out.text.contains("x 0.00 to 1.00"), "{}", out.text);
    }

    #[test]
    fn the_stop_key_ends_a_drawing_with_the_button_up() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        // The first pause in the drawing is when the user presses stop.
        let stop = e.stop_handle();
        let mut e = e.with_time(Instant::now, move |_| stop.store(true, Ordering::SeqCst));
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "speed": 50,
                "strokes": [{"points": [[10, 100], [700, 100]]}]}),
        );
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.to_lowercase().contains("stop"), "{}", out.text);
        let events = pointer_events(&e);
        assert!(events.len() < 20, "stopped early: {}", events.len());
        assert!(
            matches!(events.last(), Some(Event::PointerUp(..))),
            "{events:?}"
        );
    }

    #[test]
    fn rects_and_ellipses_need_no_formulas() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [
                {"rect": [100, 100, 200, 100]},
                {"ellipse": [400, 300, 100, 50]}
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        let downs: Vec<Point> = pointer_events(&e)
            .iter()
            .filter_map(|ev| match ev {
                Event::PointerDown(_, p, _) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(downs, [Point::new(100.0, 100.0), Point::new(500.0, 300.0)]);
        assert!(out.text.contains("Drew 2 strokes"), "{}", out.text);
        assert!(
            out.text.contains("x 100 to 500, y 100 to 350"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [{"rect": [1, 1, 5, 5], "ellipse": [1, 1, 1, 1]}]}),
        );
        assert!(
            out.text
                .contains("give one kind of stroke, not rect and ellipse"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [{"ellipse": [1, 1, 0, 1]}]}),
        );
        assert!(out.text.contains("positive radii"), "{}", out.text);
    }

    #[test]
    fn draw_in_document_units() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        // A 1000x500 px document shown at (100, 100)-(500, 300).
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "canvas": {"box": [100, 100, 500, 300], "size": [1000, 500]},
                "strokes": [{"points": [[0, 0], [1000, 500]]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        let events = pointer_events(&e);
        assert_eq!(
            events[0],
            Event::PointerDown(4242, Point::new(100.0, 100.0), MouseButton::Left)
        );
        assert_eq!(
            *events.last().unwrap(),
            Event::PointerUp(4242, Point::new(500.0, 300.0), MouseButton::Left)
        );
        assert!(
            out.text
                .contains("on the document: x 0 to 1000, y 0 to 500"),
            "{}",
            out.text
        );
        // The element's box with the document's size.
        let doc = index_named(&e, 4242, "Document");
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "canvas": {"size": [800, 560]},
                "strokes": [{"points": [[400, 280]]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(pointer_events(&e).contains(&Event::PointerDown(
            4242,
            Point::new(400.0, 320.0),
            MouseButton::Left
        )));
        for (canvas, want) in [
            (serde_json::json!({"size": [10, 10]}), "needs box"),
            (
                serde_json::json!({"box": [1, 1, 5, 5], "size": [0, 10]}),
                "both positive",
            ),
            (
                serde_json::json!({"box": [50, 50, 10, 10], "size": [10, 10]}),
                "right of left",
            ),
        ] {
            let out = e.call_tool(
                "draw",
                serde_json::json!({"app": "TextEdit", "canvas": canvas, "strokes": [{"points": [[1, 1]]}]}),
            );
            assert!(
                out.is_error && out.text.contains(want),
                "{want}: {}",
                out.text
            );
        }
    }

    fn downs(e: &Engine<MockBackend>) -> Vec<Point> {
        pointer_events(e)
            .iter()
            .filter_map(|ev| match ev {
                Event::PointerDown(_, p, _) => Some(*p),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn plots_use_math_coordinates_with_axes() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        // x -pi..pi, y -1.5..1.5 across (100, 100)-(700, 400).
        let pi = std::f64::consts::PI;
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit",
                "canvas": {"box": [100, 100, 700, 400], "range": ["-3", 3, -1.5, 1.5]},
                "strokes": [{"axes": [1, 0.5]}, {"y": "sin(x)"}]}),
        );
        // A range must be numbers.
        assert!(out.is_error, "{}", out.text);
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit",
                "canvas": {"box": [100, 100, 700, 400], "range": [-pi, pi, -1.5, 1.5]},
                "strokes": [{"axes": [1, 0.5]}, {"y": "sin(x)"}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Drew 17 strokes"), "{}", out.text);
        assert!(out.text.contains("on the plot"), "{}", out.text);
        // The sine starts at x = -pi, y = 0: the left edge, half way down.
        let d = downs(&e);
        let start = d.last().unwrap();
        assert!(
            (start.x - 100.0).abs() < 0.01 && (start.y - 250.0).abs() < 0.01,
            "{start:?}"
        );
        // Its top (x = pi/2, y = 1) is 100 px above the middle.
        let events = pointer_events(&e);
        let sine_start = events
            .iter()
            .rposition(|ev| matches!(ev, Event::PointerDown(..)))
            .unwrap();
        let top = events[sine_start..]
            .iter()
            .filter_map(|ev| match ev {
                Event::PointerMove(_, p) => Some(p.y),
                _ => None,
            })
            .fold(f64::MAX, f64::min);
        assert!((top - 150.0).abs() < 0.5, "{top}");
    }

    #[test]
    fn shapes_turn_repeat_and_report_where_to_fill() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [
                {"rect": [100, 100, 200, 100], "rotate": 90},
                {"star": [500, 300, 60, 25, 5], "repeat": {"count": 3, "offset": [10, 0]}}
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Drew 4 strokes"), "{}", out.text);
        // Turned a quarter about its centre (200, 150): now 100 wide, 200 tall.
        assert!(
            out.text
                .contains("To fill a closed outline (bucket or magic wand), click stroke 1 at (200, 150); stroke 2 (part 1), cut into 5 pieces by other lines, at (500, 300), "),
            "{}",
            out.text
        );
        let d = downs(&e);
        assert_eq!(d.len(), 4);
        // The rect's first corner (100, 100) turned clockwise about (200, 150).
        assert!(
            (d[0].x - 250.0).abs() < 1e-6 && (d[0].y - 50.0).abs() < 1e-6,
            "{:?}",
            d[0]
        );
        // Stars start at their top point, each copy 10 px to the right.
        assert!(
            (d[1].x - 500.0).abs() < 1e-6 && (d[1].y - 240.0).abs() < 1e-6,
            "{:?}",
            d[1]
        );
        assert!((d[3].x - 520.0).abs() < 1e-6);
        // Polygons, arcs, bezier.
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [
                {"polygon": [300, 300, 50, 6]},
                {"arc": [300, 300, 80, 0, 90]},
                {"bezier": [[100, 500], [150, 400], [250, 400], [300, 500]]}
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("Drew 3 strokes"), "{}", out.text);
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [{"polygon": [300, 300, 50, 2.5]}]}),
        );
        assert!(out.text.contains("whole number from 3"), "{}", out.text);
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [{"axes": [1, 1]}]}),
        );
        assert!(out.text.contains("axes need canvas.range"), "{}", out.text);
    }

    #[test]
    fn previews_show_the_strokes_without_drawing() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "preview": true,
                "canvas": {"box": [0, 0, 800, 600], "size": [1600, 1200]},
                "strokes": [{"ellipse": [800, 600, 300, 300]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .starts_with("Preview only, nothing was drawn: 1 stroke"),
            "{}",
            out.text
        );
        assert!(out.text.contains("on the document"), "{}", out.text);
        assert!(out.image.is_some());
        assert!(pointer_events(&e).is_empty(), "nothing drawn");
    }

    #[test]
    fn fill_paints_shapes_solid_with_the_brush() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit",
                "canvas": {"box": [0, 0, 800, 600], "size": [800, 600]},
                "strokes": [{"rect": [100, 100, 200, 100], "fill": 10},
                            {"ellipse": [500, 300, 60, 60]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("Solid shapes are painted for a brush 10 px wide on screen"),
            "{}",
            out.text
        );
        // Only the outline needs a bucket.
        assert!(
            out.text.contains("click stroke 2 at (500, 300)"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("stroke 1 at"), "{}", out.text);
        // The brush stays half its width inside the rectangle, and its
        // rows are close enough to leave no stripes.
        let mut rows: Vec<f64> = Vec::new();
        for ev in pointer_events(&e) {
            let (Event::PointerDown(_, p, _)
            | Event::PointerMove(_, p)
            | Event::PointerUp(_, p, _)) = ev
            else {
                continue;
            };
            if p.x < 400.0 {
                assert!(
                    (104.9..=295.1).contains(&p.x) && (104.9..=195.1).contains(&p.y),
                    "{p:?}"
                );
                rows.push(p.y);
            }
        }
        rows.sort_by(f64::total_cmp);
        rows.dedup_by(|a, b| (*a - *b).abs() < 0.01);
        assert!(rows.first().is_some_and(|y| *y < 106.0), "{rows:?}");
        assert!(rows.last().is_some_and(|y| *y > 194.0), "{rows:?}");
        assert!(rows.windows(2).all(|w| w[1] - w[0] <= 6.01), "{rows:?}");
        // An open line can't be painted solid.
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit",
                "strokes": [{"points": [[10, 10], [50, 50]], "fill": 5}]}),
        );
        assert!(
            out.is_error && out.text.contains("fill needs a closed shape"),
            "{}",
            out.text
        );
    }

    #[test]
    fn previews_find_shapes_that_other_lines_cut() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "preview": true,
                "strokes": [{"ellipse": [400, 300, 100, 100]},
                            {"points": [[250, 300], [550, 300]]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("stroke 1, cut into 2 pieces by other lines, at "),
            "{}",
            out.text
        );
        assert!(pointer_events(&e).is_empty(), "nothing drawn");
    }

    #[test]
    fn traced_pictures_are_painted_step_by_step_and_compared() {
        // A picture file: a red disc on white, twice as wide as high.
        let dir = std::env::temp_dir().join(format!("cu-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Logo Mark.png");
        let (w, h) = (200u32, 100u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let d = (f64::from(x) - 100.0).hypot(f64::from(y) - 50.0);
                rgba.extend_from_slice(if d < 30.0 {
                    &[220, 30, 30, 255]
                } else {
                    &[255, 255, 255, 255]
                });
            }
        }
        image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).unwrap();

        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "trace_image",
            serde_json::json!({"path": path.to_string_lossy(), "colors": 2}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("Traced \"logo-mark\" (Logo Mark.png, 200 x 100 px) as 2 steps"),
            "{}",
            out.text
        );
        assert!(
            out.text
                .contains("\n1. #FEFEFE: 1 shape, 100% of the picture (all of it")
                || out
                    .text
                    .contains("\n1. #FFFFFF: 1 shape, 100% of the picture (all of it"),
            "{}",
            out.text
        );
        assert!(out.text.contains("\n2. #DC1E1E: 1 shape"), "{}", out.text);
        assert!(out.image.is_some());

        // Step 2 into a 400 x 200 document shown at (0, 0)-(800, 400).
        let canvas = serde_json::json!({"box": [0, 0, 800, 400], "size": [400, 200]});
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "canvas": canvas,
                "strokes": [{"trace": "logo-mark", "step": 2, "fill": 4}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        // The disc: centre (400, 200), radius 120 on screen; the brush runs
        // on its edge, traced from a 160-pixel-wide working copy (one of
        // its pixels is 5 on screen).
        for ev in pointer_events(&e) {
            if let Event::PointerMove(_, p) = ev {
                assert!((p.x - 400.0).hypot(p.y - 200.0) <= 130.0, "{p:?}");
            }
        }
        for (strokes, want) in [
            (
                serde_json::json!([{"trace": "logo-mark", "step": 2}]),
                "give fill",
            ),
            (
                serde_json::json!([{"trace": "logo", "step": 2, "fill": 4}]),
                "no picture called \"logo\" (traced: logo-mark)",
            ),
            (
                serde_json::json!([{"trace": "logo-mark", "step": 3, "fill": 4}]),
                "has steps 1 to 2",
            ),
            (
                serde_json::json!([{"trace": "logo-mark", "step": 1, "fill": 4, "rect": [0, 0, 5, 5]}]),
                "on its own",
            ),
        ] {
            let out = e.call_tool(
                "draw",
                serde_json::json!({"app": "TextEdit", "canvas": canvas, "strokes": strokes}),
            );
            assert!(
                out.is_error && out.text.contains(want),
                "{want}: {}",
                out.text
            );
        }
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit",
                "strokes": [{"trace": "logo-mark", "step": 1, "fill": 4}]}),
        );
        assert!(
            out.is_error && out.text.contains("needs canvas or element_index"),
            "{}",
            out.text
        );

        // Compared with the canvas: grey where white should be.
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "canvas": canvas, "compare": "logo-mark"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Compared with \"logo-mark\": "),
            "{}",
            out.text
        );
        // Grey canvas: nothing alike, the red disc's cells most of all.
        assert!(
            out.text.contains("0 of 64 cells look alike")
                && out.text.contains(
                    "Most different: x 150 to 200, y 75 to 100 should be #DC1E1E but is "
                ),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "compare": "logo-mark"}),
        );
        assert!(
            out.is_error && out.text.contains("compare needs canvas"),
            "{}",
            out.text
        );

        // What a window shows can be traced too.
        let out = e.call_tool(
            "trace_image",
            serde_json::json!({"app": "TextEdit", "box": [0, 450, 800, 600], "name": "Strip"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Traced \"strip\" (TextEdit window"),
            "{}",
            out.text
        );
        assert!(out.text.contains("800 x 150 px"), "{}", out.text);
        let out = e.call_tool(
            "trace_image",
            serde_json::json!({"path": dir.join("nope.png").to_string_lossy()}),
        );
        assert!(
            out.is_error && out.text.contains("can't read"),
            "{}",
            out.text
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn designs_are_composed_seen_and_painted_step_by_step() {
        let mut e = steps_engine();
        state_of(&mut e, serde_json::json!({}));
        // A name nobody started yet needs a size.
        let out = e.call_tool("design", serde_json::json!({"name": "Badge"}));
        assert!(
            out.is_error && out.text.contains("give size"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "Badge", "size": [400, 200], "background": "#FFFFFF",
                "add": [{"id": "disc", "ellipse": [100, 100, 60, 60], "fill": "#CC2222"},
                        {"id": "ring", "ellipse": [100, 100, 80, 80], "fill": "none", "stroke": "#222222", "width": 4},
                        {"id": "label", "text": "OK", "at": [280, 80], "size": 40}],
                "mirror": [{"id": "disc", "as": "disc-2"}],
                "show": {"guides": true, "ids": true, "grid": 50}}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.image.is_some());
        assert!(
            out.text.starts_with("Design \"badge\": 400 x 200, background #FFFFFF, margin 10. 4 layers, back to front: disc ellipse x 40 y 40 w 120 h 120 fill #CC2222; disc-2 ellipse x 240 y 40"),
            "{}",
            out.text
        );
        assert!(out.text.contains("steps 1 #CC2222 solid (disc, disc-2); 2 #222222 lines 4 (ring); 3 #000000 text (label)"), "{}", out.text);

        // A change that fails leaves the design as it was.
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "badge", "change": [{"id": "disc", "fill": "#00FF00"}], "remove": ["nope"]}),
        );
        assert!(
            out.is_error && out.text.contains("no layer \"nope\""),
            "{}",
            out.text
        );
        let out = e.call_tool("design", serde_json::json!({"name": "badge"}));
        assert!(
            out.text
                .contains("disc ellipse x 40 y 40 w 120 h 120 fill #CC2222"),
            "{}",
            out.text
        );

        // Painted into an 800 x 400 document shown at (0, 0)-(800, 400):
        // twice the size.
        let canvas = serde_json::json!({"box": [0, 0, 800, 400], "size": [800, 400]});
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "canvas": canvas,
                "strokes": [{"design": "badge", "step": 1, "fill": 8}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        for ev in pointer_events(&e) {
            if let Event::PointerMove(_, p) = ev {
                let d = (p.x - 200.0)
                    .hypot(p.y - 200.0)
                    .min((p.x - 600.0).hypot(p.y - 200.0));
                assert!(d <= 120.5, "{p:?}");
            }
        }
        // The ring is a line step: no fill needed; the text step is typed.
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "canvas": canvas,
                "strokes": [{"design": "badge", "step": 2}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        for (strokes, want) in [
            (
                serde_json::json!([{"design": "badge", "step": 1}]),
                "give fill",
            ),
            (
                serde_json::json!([{"design": "badge", "step": 3}]),
                "step 3 is the text of label: type it with the app's text tool at (280, 80)",
            ),
            (
                serde_json::json!([{"design": "badge", "step": 9}]),
                "has steps 1 to 3",
            ),
            (
                serde_json::json!([{"design": "nope", "step": 1}]),
                "no design called \"nope\" (designs: badge)",
            ),
            (
                serde_json::json!([{"design": "badge", "trace": "x", "step": 1}]),
                "trace or design, not both",
            ),
        ] {
            let out = e.call_tool(
                "draw",
                serde_json::json!({"app": "TextEdit", "canvas": canvas, "strokes": strokes}),
            );
            assert!(
                out.is_error && out.text.contains(want),
                "{want}: {}",
                out.text
            );
        }

        // Exports are temporary files.
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "badge", "export": "svg"}),
        );
        let path = out
            .text
            .split("Exported to ")
            .nth(1)
            .and_then(|r| r.split(" (").next())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| panic!("{}", out.text));
        let svg = std::fs::read_to_string(&path).unwrap();
        assert!(svg.contains("<ellipse id=\"disc-2\" cx=\"300\""), "{svg}");
        assert!(out.text.contains("It is temporary"), "{}", out.text);
        drop(e);
        assert!(!path.exists(), "deleted with the server");
    }

    #[test]
    fn scenes_are_planned_seen_checked_and_exported() {
        let mut e = engine();
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "Stool",
                "add": [{"id": "seat", "shape": "cylinder", "size": [0.4, 0.04], "at": [0, 0, 0.47], "color": "#8B5A2B"},
                        {"id": "leg", "shape": "box", "size": [0.04, 0.04, 0.45], "at": [0.12, 0.12, 0.225]},
                        {"id": "lamp", "shape": "sphere", "size": [0.1], "at": [0, 0, 0.6]}],
                "mirror": [{"id": "leg", "as": "leg-b"}],
                "repeat": [{"id": "leg", "count": 2, "offset": [0, -0.24, 0]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.image.is_some());
        assert!(
            out.text.starts_with("Scene \"stool\": 5 objects, 0.4 x 0.4 x 0.65 (x -0.2 to 0.2, y -0.2 to 0.2, z 0 to 0.65), Z up, the ground at z 0. seat cylinder 0.4 x 0.4 x 0.04 at (0, 0, 0.47), z 0.45 to 0.49, #8B5A2B;"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("lamp floats in the air: nothing holds it; the bottom of lamp is 0.06 above seat (move it down 0.06"),
            "{}",
            out.text
        );
        assert!(out.text.contains("In Blender: Add > Mesh"), "{}", out.text);
        assert!(out.text.contains("a grid line every 0.1"), "{}", out.text);

        // A bad change leaves the scene as it was; a good one fixes it.
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "change": [{"id": "lamp", "on": "seat"}], "remove": ["nope"]}),
        );
        assert!(
            out.is_error && out.text.contains("no object \"nope\""),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "change": [{"id": "lamp", "on": "seat"}], "view": "top"}),
        );
        assert!(
            out.text
                .contains("Checks: everything rests on the ground or on something"),
            "{}",
            out.text
        );
        assert!(out.text.contains("Build: Location = at"), "{}", out.text);

        // Exports are temporary: the model and its colours.
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "export": "obj"}),
        );
        let path = out
            .text
            .split("Exported to ")
            .nth(1)
            .and_then(|r| r.split(" with").next())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| panic!("{}", out.text));
        let obj = std::fs::read_to_string(&path).unwrap();
        let mtl_name = obj
            .lines()
            .find_map(|l| l.strip_prefix("mtllib "))
            .unwrap()
            .to_string();
        let mtl = path.with_file_name(&mtl_name);
        assert!(
            std::fs::read_to_string(&mtl)
                .unwrap()
                .contains("newmtl c8B5A2B")
        );
        assert!(
            obj.contains("o seat\nusemtl c8B5A2B\nv "),
            "{}",
            &obj[..200]
        );
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "export": "png", "view": "perspective"}),
        );
        let png = out
            .text
            .split("Exported to ")
            .nth(1)
            .and_then(|r| r.split(" (").next())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| panic!("{}", out.text));
        assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
        drop(e);
        assert!(
            !path.exists() && !mtl.exists() && !png.exists(),
            "deleted with the server"
        );
    }

    #[test]
    fn pixel_targeting_snaps_finds_and_magnifies() {
        let mut e = engine();
        let mut cfg = e.store().config.clone();
        cfg.screenshot.locate_picture = true;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        // A dark square drawn on the (mock) window: 200..260 x 150..210.
        e.backend_mut().patch = Some((Rect::new(200.0, 150.0, 60.0, 60.0), 20));
        // A click near the square's corner lands on it exactly.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "x": 204, "y": 147, "snap": "corner"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Snapped to the corner at ("),
            "{}",
            out.text
        );
        let clicked = e
            .backend()
            .events
            .iter()
            .rev()
            .find_map(|ev| match ev {
                Event::Click(_, p, _, _) => Some(*p),
                _ => None,
            })
            .unwrap();
        assert!(
            (clicked.x - 200.5).abs() <= 1.5 && (clicked.y - 150.5).abs() <= 1.5,
            "{clicked:?}"
        );
        // Nothing to snap to: nothing is clicked.
        let before = e.backend().events.len();
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "x": 500, "y": 400, "snap": "edge", "snap_radius": 5}),
        );
        assert!(
            out.is_error
                && out
                    .text
                    .contains("no edge within 5 pixels of (500, 400), so nothing was done"),
            "{}",
            out.text
        );
        assert_eq!(e.backend().events.len(), before);

        // Areas of a colour, look-alikes, and the centre near a point.
        let out = e.call_tool(
            "locate",
            serde_json::json!({"app": "TextEdit", "color": "#141414"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("1 area of #141414 (within 16), biggest first: 1 at (230.0, 180.0), box 200,150 to 260,210"), "{}", out.text);
        assert!(out.image.is_some());
        let out = e.call_tool(
            "locate",
            serde_json::json!({"app": "TextEdit", "near": [238, 191], "feature": "center", "radius": 40}),
        );
        assert!(
            out.text
                .contains("The shape centre near (238, 191): (230.0, 180.0), 8.0 left and 11.0 up"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "locate",
            serde_json::json!({"app": "TextEdit", "color": "#141414", "like": [0, 0, 5, 5]}),
        );
        assert!(
            out.is_error && out.text.contains("give one of"),
            "{}",
            out.text
        );

        // The loupe: a magnified view with a crosshair.
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "zoom": [200, 150], "radius": 6}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.starts_with("Magnified around (200, 150)"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("the crosshair is the point, on #141414"),
            "{}",
            out.text
        );
        assert!(out.image.is_some());
    }

    #[test]
    fn screenshot_grids_and_picks_follow_the_canvas() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "grid": true,
                "canvas": {"box": [0, 0, 800, 600], "range": [-4, 4, -3, 3]},
                "pick": [[0, 0]]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("Grid: a line every 1, labelled in the canvas range"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("Colours: (0, 0) #C8C8C8."),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"grid": true, "canvas": {"box": [0, 0, 8, 6], "size": [8, 6]}}),
        );
        assert!(
            out.text.contains("canvas needs a window screenshot"),
            "{}",
            out.text
        );
    }

    #[test]
    fn cells_name_the_page_and_show_one_cell_close() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let canvas = serde_json::json!({"box": [0, 0, 800, 600], "size": [800, 600]});
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "canvas": canvas, "cells": true}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains(
                "Cells over the document: cells of 100: columns A to H from x 0, rows 1 to 6 from y 0 down"
            ),
            "{}",
            out.text
        );

        // One cell up close, with its colours.
        e.backend_mut().patch = Some((Rect::new(210.0, 110.0, 80.0, 80.0), 20));
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "canvas": canvas, "cell": "c2"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.starts_with(
                "Cell C2 of the document: x 200 to 300, y 100 to 200 (shown 3 times bigger"
            ),
            "{}",
            out.text
        );
        assert!(out.text.contains("#141414 64%"), "{}", out.text);
        let img = out.image.unwrap();
        assert!(img.width >= 390 && img.width <= 512, "{}", img.width);

        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "canvas": canvas, "cell": "J9"}),
        );
        assert!(
            out.is_error && out.text.contains("cells are A1 to H6"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "cells": true}),
        );
        assert!(
            out.is_error && out.text.contains("cells need canvas"),
            "{}",
            out.text
        );

        // Drawing says which cells a drawing covers; a small one gets
        // smaller cells.
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "preview": true, "canvas": canvas,
                "strokes": [{"rect": [100, 100, 300, 200]}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text
                .contains("It covers cells B2 to D3 (cells of 100, A1 top-left)."),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "preview": true, "canvas": canvas,
                "strokes": [{"rect": [100, 100, 40, 40]}]}),
        );
        assert!(
            out.text.contains("(cells of 5, A1 top-left)"),
            "{}",
            out.text
        );

        // The design board names its cells and opens one.
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "card", "size": [400, 200], "background": "#FFFFFF",
                "add": [{"id": "disc", "ellipse": [100, 100, 60, 60], "fill": "#CC2222"}]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains(
                "On the picture, cells of 50: columns A to H from x 0, rows 1 to 4 from y 0 down"
            ),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "card", "show": {"cell": "B2"}}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Cell B2: x 50 to 100, y 50 to 100")
                && out.text.contains("Layers in it, back to front: disc."),
            "{}",
            out.text
        );
        assert!(out.image.is_some());
    }

    #[test]
    fn keys_can_go_to_what_is_under_the_pointer() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        e.backend_mut().pointer = Some(Point::new(5.0, 5.0));
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "g x 2 Return", "x": 300, "y": 200}),
        );
        assert!(!out.is_error, "{}", out.text);
        let ev = &e.backend().events;
        let first_key = ev.iter().position(|v| matches!(v, Event::Key(..))).unwrap();
        let last_key = ev
            .iter()
            .rposition(|v| matches!(v, Event::Key(..)))
            .unwrap();
        // Pointed there before the first key, put back after the last.
        assert!(ev[..first_key].contains(&Event::Hover(4242, Point::new(300.0, 200.0))));
        assert!(ev[last_key..].contains(&Event::Hover(4242, Point::new(5.0, 5.0))));
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "text": "1.5", "x": 300}),
        );
        assert!(
            out.is_error && out.text.contains("both x and y"),
            "{}",
            out.text
        );
    }

    #[test]
    fn screenshots_read_coordinates_and_colours() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "screenshot",
            serde_json::json!({"app": "TextEdit", "grid": 100, "palette": true,
                "pick": [[10, 10], [5000, 5]]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.image.is_some());
        let t = &out.text;
        assert!(t.contains("Main colours: #C8C8C8 100%."), "{t}");
        assert!(
            t.contains("Colours: (10, 10) #C8C8C8; (5000, 5) is outside the image."),
            "{t}"
        );
        assert!(
            t.contains("Grid: a line every 100, labelled in the x/y that click"),
            "{t}"
        );
        // A full-screen grid is in screen coordinates, and always sent.
        let out = e.call_tool("screenshot", serde_json::json!({"grid": true}));
        assert!(out.image.is_some());
        assert!(
            out.text.contains("labelled in screen coordinates"),
            "{}",
            out.text
        );
        let again = e.call_tool("screenshot", serde_json::json!({"grid": true}));
        assert!(again.image.is_some(), "{}", again.text);
    }

    #[test]
    fn bad_drawings_are_explained() {
        let mut e = engine();
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [{"points": [[1, 1]]}]}),
        );
        assert!(
            out.text.contains("call get_app_state first"),
            "{}",
            out.text
        );
        state_of(&mut e, serde_json::json!({}));
        for (stroke, want) in [
            (
                serde_json::json!({"x": "foo(t)", "y": "t"}),
                "stroke 1: x: unknown name `foo`",
            ),
            (
                serde_json::json!({"y": "sin(z)"}),
                "y: unknown name `z`: use t or x",
            ),
            (
                serde_json::json!({}),
                "give a shape (rect, ellipse, polygon, star, arc, bezier), points, axes",
            ),
            (
                serde_json::json!({"points": [[1, 1]], "x": "t", "y": "t"}),
                "give one kind of stroke",
            ),
            (
                serde_json::json!({"points": [[1, 1], [5000, 1]]}),
                "outside the drawing area",
            ),
            (
                serde_json::json!({"x": "t", "y": "t", "t": [0, "2*pie"]}),
                "t to: unknown name",
            ),
        ] {
            let out = e.call_tool(
                "draw",
                serde_json::json!({"app": "TextEdit", "strokes": [stroke]}),
            );
            assert!(
                out.is_error && out.text.contains(want),
                "{want}: {}",
                out.text
            );
        }
        assert!(pointer_events(&e).is_empty(), "nothing was drawn");
    }

    #[test]
    fn an_action_naming_another_window_is_refused() {
        let mut app = MockBackend::text_editor(9);
        app.windows.push(MockWindow {
            id: 2,
            title: "Preferences".into(),
            bounds: Rect::new(900.0, 0.0, 300.0, 200.0),
            root: 50,
            focused: false,
        });
        app.elements.push(MockElement::new(
            50,
            "window",
            "Preferences",
            Rect::new(900.0, 0.0, 300.0, 200.0),
        ));
        let mut backend = MockBackend::new();
        backend.add_app(app);
        let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
            .with_time(Instant::now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        let bold = index_named(&e, 9, "Bold");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold, "window": "Preferences"}),
        );
        assert!(out.is_error, "{}", out.text);
        assert!(out.text.contains("window=\"Preferences\""), "{}", out.text);
        // Naming the window it shows is fine.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold, "window": "Untitled"}),
        );
        assert!(!out.is_error, "{}", out.text);
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
        let cfg = Config::default();
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
        );
        state_of(&mut e, serde_json::json!({}));
        assert_eq!(e.backend().snapshots, snaps, "tree read reused");
        assert_eq!(e.backend().window_lists, lists, "window list reused");
        // An action makes them stale: settling reads the app until two reads
        // agree, and the next get_app_state reuses the last of them.
        press_named(&mut e, 7, "Next");
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
        );
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_none(), "{}", out.text);
    }

    // -- overlay hooks (a stand-in helper records what it is told) ---------

    #[cfg(unix)]
    fn recording_helper(log: &std::path::Path) -> Launcher {
        Launcher {
            program: "sh".into(),
            args: vec!["-c".into(), format!("cat > '{}'", log.display())],
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_stop_key_that_does_not_work_is_reported_once() {
        let dir = std::env::temp_dir().join(format!("cu-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("stop.log");
        // A helper that says the system refused the key.
        let helper = Launcher {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    r#"echo '{{"t":"hotkey","key":"ctrl+alt+escape","ok":false}}'; cat > '{}'"#,
                    log.display()
                ),
            ],
        };
        let mut e = engine().with_overlay(helper);
        e.arm();
        assert!(e.check_stop_key(Duration::from_secs(5)).is_err());
        let first = e.call_tool("list_apps", serde_json::json!({}));
        assert!(
            first.text.contains("emergency stop key") && first.text.contains("not working"),
            "{}",
            first.text
        );
        let second = e.call_tool("list_apps", serde_json::json!({}));
        assert!(!second.text.contains("emergency stop key"), "told once");
        drop(e);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Everything the recording helper got, once the engine has let it go
    /// (its last message is `quit`; dropping the engine doesn't wait).
    #[cfg(unix)]
    fn read_log(path: &std::path::Path) -> String {
        for _ in 0..250 {
            if let Ok(t) = std::fs::read_to_string(path)
                && t.contains("\"t\":\"quit\"")
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
        let mut e = nav_engine(false).with_overlay(recording_helper(&log));
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
            let out = e.call_tool(tool, args);
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
        );
        assert!(out.is_error);
        e.set_stopped(false);
        assert!(!e.is_stopped());
        assert!(!e.call_tool("list_apps", serde_json::json!({})).is_error);
    }

    #[test]
    fn actions_wait_while_the_user_uses_the_computer() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        // The user is typing (input 0.1 s and 0.4 s ago), then stops.
        backend.idle_script = [100, 400].map(Duration::from_millis).into();
        backend.idle = Some(Duration::from_secs(5));
        let cfg = Config::default();
        let (mut e, clock) = timed_engine(backend, cfg);
        state_of(&mut e, serde_json::json!({}));
        let t0 = *clock.lock().unwrap();
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "Return"}),
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
        assert!(!e.call_tool("list_apps", serde_json::json!({})).is_error);
    }

    #[test]
    fn a_busy_user_makes_the_action_give_up_and_say_why() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        backend.idle = Some(Duration::from_millis(100));
        let mut cfg = Config::default();
        cfg.control.max_pause_secs = 3;
        let (mut e, _) = timed_engine(backend, cfg);
        state_of(&mut e, serde_json::json!({}));
        let before = e.backend().events.len();
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "Return"}),
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
        );
        assert!(!out.is_error, "{}", out.text);
    }

    #[test]
    fn own_input_is_not_taken_for_the_user() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        // Only the pause for the user is measured here, not settling.
        cfg.timing.settle = crate::config::SettleMode::Fixed;
        let (mut e, clock) = timed_engine(backend, cfg);
        state_of(&mut e, serde_json::json!({}));
        let press = |e: &mut Engine<MockBackend>| {
            e.call_tool(
                "press_key",
                serde_json::json!({"app": "TextEdit", "key": "Tab"}),
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
        let cfg = Config::default();
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
        let out = e.call_tool("screenshot", serde_json::json!({"app": "TextEdit"}));
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
        let out = e.call_tool("list_apps", serde_json::json!({}));
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
        index_of(out, needle).unwrap_or_else(|| panic!("{needle} not in:\n{out}"))
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
        );
        assert!(out.is_error, "{}", out.text);
    }

    #[test]
    fn scripts_cannot_start_scripts_through_batch() {
        let dir = std::env::temp_dir().join(format!("cu-nest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = engine();
        let mut cfg = e.store().config.clone();
        cfg.script.dir = Some(dir.clone());
        e.set_config(ConfigStore::in_memory(cfg));
        let code = r#"tool("batch", #{steps: [#{tool: "script", arguments: #{run: "rec"}}]})"#;
        let saved = e.call_tool(
            "script",
            serde_json::json!({"save": "rec", "code": code, "description": "recurse"}),
        );
        assert!(!saved.is_error, "{}", saved.text);
        let out = e.call_tool("script", serde_json::json!({"run": "rec"}));
        assert!(out.is_error, "{}", out.text);
        assert!(
            out.text.contains("can't start another script"),
            "{}",
            out.text
        );
        // And the engine is fine afterwards.
        assert!(!e.call_tool("list_apps", serde_json::json!({})).is_error);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn long_text_is_typed_in_pieces_and_too_much_is_refused() {
        let mut e = engine();
        let text: String = "ab€d".repeat(150);
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "text": text}),
        );
        assert!(!out.is_error, "{}", out.text);
        let pieces: Vec<&String> = e
            .backend()
            .events
            .iter()
            .filter_map(|ev| match ev {
                Event::Type(_, t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(pieces.len(), 3, "{pieces:?}");
        assert!(pieces.iter().all(|p| p.chars().count() <= TYPE_CHUNK));
        assert_eq!(pieces.iter().map(|p| p.as_str()).collect::<String>(), text);

        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "text": "x".repeat(MAX_TYPED_CHARS + 1)}),
        );
        assert!(
            out.is_error && out.text.contains("set_clipboard"),
            "{}",
            out.text
        );
        let keys = vec!["a"; MAX_KEY_PRESSES + 1].join(" ");
        let out = e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": keys}),
        );
        assert!(out.is_error && out.text.contains("at most"), "{}", out.text);
    }

    #[test]
    fn a_cancelled_call_ends_and_the_next_one_runs() {
        let mut e = engine();
        e.cancel_handle().store(true, Ordering::SeqCst);
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "text": "hello"}),
        );
        assert!(
            out.is_error && out.text.contains("cancelled"),
            "{}",
            out.text
        );
        assert!(
            !e.backend()
                .events
                .iter()
                .any(|ev| matches!(ev, Event::Type(..))),
        );
        // The cancel was for that call only.
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "text": "hello"}),
        );
        assert!(!out.is_error, "{}", out.text);
    }

    #[test]
    fn a_secondary_action_that_fails_is_an_error() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let style = index_of_name(&out.text, "\"Style\"");
        e.backend_mut().fail_actions.insert(4);
        let out = e.call_tool(
            "perform_secondary_action",
            serde_json::json!({"app": "TextEdit", "element_index": style, "action": "show_menu"}),
        );
        assert!(out.is_error, "{}", out.text);
    }

    #[test]
    fn an_unanswered_press_is_never_clicked_again() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        e.backend_mut().unanswered_actions.insert(3);
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("may or may not have happened"),
            "{}",
            out.text
        );
        let events = &e.backend().events;
        assert!(events.contains(&Event::Action(3, "AXPress".into())));
        assert!(
            !events.iter().any(|ev| matches!(ev, Event::Click(..))),
            "{events:?}"
        );
    }

    #[test]
    fn a_press_that_changes_nothing_is_reported_not_repeated() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
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
        );
        assert!(
            out.text.contains("clicked it with the mouse too"),
            "{}",
            out.text
        );
        assert_eq!(presses(&e), 3);
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
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("typed it instead"), "{}", out.text);
        let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
        assert!(out.text.contains("Replaced"), "{}", out.text);
        assert!(!out.text.contains("HelloReplaced"), "{}", out.text);
    }

    #[test]
    fn typing_that_does_not_show_is_never_typed_twice() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let doc = index_of_name(&out.text, "\"Document\"");
        // Focus claims success but does nothing: the field doesn't change.
        e.backend_mut().fake_focus.insert(5);
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "text": "y"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("could enter the text twice"),
            "{}",
            out.text
        );
        let typed = e
            .backend()
            .events
            .iter()
            .filter(|ev| matches!(ev, Event::Type(..)))
            .count();
        assert_eq!(typed, 1, "typed once, never again on its own");
    }

    #[test]
    fn a_change_reported_late_is_not_taken_for_no_change() {
        let mut e = engine();
        let out = state_of(&mut e, serde_json::json!({}));
        let bold = index_of_name(&out.text, "\"Bold\"");
        // The app shows the change only after a few reads.
        e.backend_mut().snapshot_script.extend([
            (5, "Hello".to_string()),
            (5, "Hello".to_string()),
            (5, "Hello".to_string()),
            (5, "Changed".to_string()),
        ]);
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            !out.text.contains("Nothing on screen changed"),
            "{}",
            out.text
        );
    }

    // -- smart screenshots ----------------------------------------------------

    fn always_shot_engine() -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.screenshot.attach = AttachMode::Always;
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
    }

    #[test]
    fn auto_screenshots_of_well_described_windows_can_be_overviews() {
        // Off by default: full detail.
        let mut e = engine();
        let img = state_of(&mut e, serde_json::json!({})).image.unwrap();
        assert_eq!((img.width, img.height), (800, 600));
        // Opted in.
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.screenshot.overview_max_dimension = 768;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        // Attached on its own: an overview at overview_max_dimension.
        let out = state_of(&mut e, serde_json::json!({}));
        let img = out.image.expect("first view");
        assert_eq!(img.width.max(img.height), 768, "{}", out.text);
        assert!(out.text.contains("An overview"), "{}", out.text);
        // Coordinates in the overview still map to the screen.
        e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "x": 384, "y": 288}),
        );
        assert!(matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, p, _, _) if (p.x - 400.0).abs() < 1.0 && (p.y - 300.0).abs() < 1.0
        ));
        // Asked for: full detail.
        let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
        let img = out.image.expect("asked for");
        assert_eq!((img.width, img.height), (800, 600));
    }

    /// TextEdit with a 300-item list, and a small token budget.
    fn long_list_engine(cfg: impl FnOnce(&mut Config)) -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        let mut app = MockBackend::text_editor(4242);
        for i in 0..300u64 {
            app.elements.push(
                crate::mock::MockElement::new(
                    1000 + i,
                    "list item",
                    &format!("Item {i}"),
                    Rect::new(0.0, 40.0 + i as f64, 100.0, 1.0),
                )
                .child_of(1),
            );
        }
        backend.add_app(app);
        let mut c = Config::default();
        c.tree.max_tokens = 400;
        cfg(&mut c);
        Engine::new(backend, ConfigStore::in_memory(c)).with_time(Instant::now, |_| {})
    }

    #[test]
    fn summarizing_can_be_turned_down_or_off() {
        // Over the budget: the list is folded.
        let mut e = long_list_engine(|_| {});
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("folded"), "{}", out.text);
        // The model can ask for one whole tree.
        let out = state_of(
            &mut e,
            serde_json::json!({"disable_diff": true, "max_tokens": 0}),
        );
        assert!(!out.text.contains("folded"), "{}", out.text);
        assert!(out.text.contains("Item 150"), "{}", out.text);
        // The user can turn it off.
        let mut e = long_list_engine(|c| c.tree.summarize = crate::config::Summarize::Off);
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(!out.text.contains("folded") && out.text.contains("Item 150"));
        // Or keep more of each list.
        let mut e = long_list_engine(|c| c.tree.fold_keep = 40);
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("Item 39") && !out.text.contains("Item 40\""));
    }

    #[test]
    fn compact_looks_say_each_thing_once() {
        let mut e = engine();
        let first = state_of(&mut e, serde_json::json!({}));
        assert!(
            first
                .text
                .starts_with("App: TextEdit (com.apple.TextEdit, pid 4242) · window"),
            "{}",
            first.text
        );
        // A value the field now shows isn't echoed back.
        let doc = index_of_name(&first.text, "\"Document\"");
        let out = e.call_tool(
            "set_value",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "value": "Hi there"}),
        );
        assert!(
            out.text
                .starts_with("Set text area \"Document\"; it shows the new value."),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("value=\"Hi there\""),
            "the report shows it: {}",
            out.text
        );
        // Nothing new since: one line, the app and window named only.
        let quiet = state_of(&mut e, serde_json::json!({}));
        assert_eq!(
            quiet.text,
            "App: TextEdit · window \"Untitled\" · screen #1: nothing changed since your last look (tree and screenshot)."
        );
        // Part of the tree, without moving the baseline.
        let toolbar = e
            .state(4242)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.role == "toolbar")
            .unwrap()
            .index;
        let part = state_of(&mut e, serde_json::json!({"within": toolbar}));
        assert!(
            part.text.contains(&format!(
                "element {toolbar} and what is in it (3 element(s))"
            )),
            "{}",
            part.text
        );
        assert!(part.text.contains("Bold") && !part.text.contains("Document"));
        let again = state_of(&mut e, serde_json::json!({}));
        assert!(again.text.contains("nothing changed"), "{}", again.text);
        let bad = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": "TextEdit", "within": 999}),
        );
        assert!(bad.is_error && bad.text.contains("unknown element_index 999"));
    }

    #[test]
    fn find_element_pages_through_many_matches() {
        let mut e = long_list_engine(|_| {});
        let out = e.call_tool(
            "find_element",
            serde_json::json!({"app": "TextEdit", "role": "list item", "max_results": 5, "offset": 5}),
        );
        assert!(out.text.starts_with("Matches 6–10 of 300"), "{}", out.text);
        assert!(out.text.contains("offset=10"), "{}", out.text);
        assert!(out.text.contains("Item 5\"") && out.text.contains("Item 9\""));
        let out = e.call_tool(
            "find_element",
            serde_json::json!({"app": "TextEdit", "role": "list item", "offset": 400}),
        );
        assert!(out.text.contains("none after offset 400"), "{}", out.text);
    }

    #[test]
    fn reports_can_be_brief_or_about_what_was_acted_on() {
        // Brief: how many, and the new screen.
        let mut e = nav_engine(true);
        let mut cfg = e.store().config.clone();
        cfg.tree.report = crate::config::Report::Brief;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        let out = press_named(&mut e, 7, "Next");
        assert!(out.text.contains("now on screen #2 (new)"), "{}", out.text);
        assert!(
            out.text.contains("element(s); get_app_state shows them"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("Result 3"), "{}", out.text);
        // The next look shows all of it.
        let look = state_of(&mut e, serde_json::json!({}));
        assert!(expand(&look.text).contains("Result 3"), "{}", look.text);

        // Relevant: a toolbar of five buttons, a status line elsewhere.
        let mut backend = MockBackend::new();
        let mut app = MockBackend::text_editor(9);
        for (k, name) in ["Italic", "Underline", "Strike"].iter().enumerate() {
            app.elements.push(
                MockElement::new(
                    40 + k as u64,
                    "button",
                    name,
                    Rect::new(200.0 + 70.0 * k as f64, 8.0, 60.0, 24.0),
                )
                .child_of(2)
                .with_actions(&["AXPress"]),
            );
        }
        app.elements.push(
            MockElement::new(
                50,
                "text",
                "Status: ready",
                Rect::new(0.0, 580.0, 300.0, 20.0),
            )
            .child_of(1),
        );
        let mut after = app.clone();
        for el in after.elements.iter_mut() {
            if el.handle == 50 {
                el.name = Some("Status: bold on".into());
            }
            if el.handle == 41 {
                el.name = Some("Underline (on)".into());
            }
        }
        backend.add_app(app);
        backend.on_press.insert(3, after);
        let mut cfg = Config::default();
        cfg.tree.report = crate::config::Report::Relevant;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        let first = e.call_tool("get_app_state", serde_json::json!({"app": "TextEdit"}));
        let bold = index_of(&first.text, "\"Bold\"").unwrap();
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold}),
        );
        assert!(
            out.text.contains("Underline (on)"),
            "in the toolbar: {}",
            out.text
        );
        assert!(!out.text.contains("bold on"), "elsewhere: {}", out.text);
        assert!(
            out.text.contains("[1 other change(s) elsewhere"),
            "{}",
            out.text
        );
    }

    #[test]
    fn what_changes_on_its_own_is_summed_up() {
        let mut backend = MockBackend::new();
        let mut app = MockBackend::text_editor(4242);
        app.elements
            .push(MockElement::new(60, "text", "", Rect::new(700.0, 8.0, 80.0, 20.0)).child_of(2));
        backend.add_app(app);
        for t in 0..8 {
            backend
                .snapshot_script
                .push_back((60, format!("12:00:0{t}")));
        }
        let mut cfg = Config::default();
        cfg.tree.quiet_volatile = true;
        cfg.cache.snapshot_ttl_ms = 0;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        let mut last = String::new();
        for _ in 0..4 {
            last = state_of(&mut e, serde_json::json!({})).text;
        }
        assert!(
            last.contains("1 element(s) that keep changing on their own"),
            "{last}"
        );
        assert!(!last.contains("value=\"12:00"), "{last}");
    }

    #[test]
    fn pictures_follow_how_the_model_works() {
        // Adaptive: after a few looks without pixels, a new screen of a
        // well-described window comes without a picture.
        let mut e = nav_engine(true);
        let mut cfg = e.store().config.clone();
        cfg.screenshot.adaptive = true;
        cfg.cache.snapshot_ttl_ms = 0;
        e.set_config(ConfigStore::in_memory(cfg));
        let first = state_of(&mut e, serde_json::json!({}));
        assert!(first.image.is_some(), "the first look has one");
        for _ in 0..3 {
            state_of(&mut e, serde_json::json!({}));
        }
        // The document changes: a reason to look at the pixels, but not
        // here.
        e.backend_mut()
            .snapshot_script
            .push_back((5, "Changed".into()));
        e.backend_mut().fill = 90;
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_none(), "{}", out.text);
        assert!(out.text.contains("haven't needed pictures"), "{}", out.text);
        // Once the model uses pixels there, pictures come again.
        let r = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "x": 5, "y": 5}),
        );
        assert!(!r.is_error, "{}", r.text);
        e.backend_mut()
            .snapshot_script
            .push_back((5, "Changed again".into()));
        e.backend_mut().fill = 120;
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_some(), "{}", out.text);

        // Icon strip: unnamed buttons, shown once, numbered.
        let mut backend = MockBackend::new();
        let mut app = MockBackend::text_editor(4242);
        for k in 0..3u64 {
            app.elements.push(
                MockElement::new(
                    70 + k,
                    "button",
                    "",
                    Rect::new(300.0 + 40.0 * k as f64, 8.0, 24.0, 24.0),
                )
                .child_of(2)
                .with_actions(&["AXPress"]),
            );
        }
        backend.add_app(app);
        let mut cfg = Config::default();
        cfg.screenshot.icon_sprite = true;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        let out = state_of(&mut e, serde_json::json!({"screenshot": false}));
        let strip = out.image.as_ref().expect("the strip");
        assert!(
            strip.height < 80 && strip.width < 300,
            "{}x{}",
            strip.width,
            strip.height
        );
        assert!(
            out.text.contains("Icons of buttons without a name"),
            "{}",
            out.text
        );
        let again = state_of(&mut e, serde_json::json!({"screenshot": false}));
        assert!(again.image.is_none(), "shown once: {}", again.text);

        // locate without its picture.
        let mut e = always_shot_engine();
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "locate",
            serde_json::json!({"app": "TextEdit", "near": [400, 300], "feature": "center", "picture": false}),
        );
        assert!(out.image.is_none(), "{}", out.text);
    }

    #[test]
    fn the_indicator_label_is_never_the_apps_text() {
        assert!(reads_as_start_of("zero is thir", "zero is thinking…"));
        assert!(reads_as_start_of("zero", "zero is done"));
        assert!(!reads_as_start_of("zero balance", "zero is done"));
        assert!(!reads_as_start_of("save", "zero is done"));
    }

    #[test]
    fn the_tool_manager_shows_the_base_and_finds_the_rest() {
        use crate::config::ToolManager;
        let mut e = engine();
        // The default.
        assert_eq!(e.store().config.tools.manager, ToolManager::Dispatch);
        let mut cfg = e.store().config.clone();
        cfg.tools.manager = ToolManager::Off;
        e.set_config(ConfigStore::in_memory(cfg.clone()));
        let all = e.tool_definitions().len();
        cfg.tools.manager = ToolManager::Dispatch;
        e.set_config(ConfigStore::in_memory(cfg.clone()));
        let names = |e: &mut Engine<MockBackend>| -> Vec<String> {
            e.tool_definitions()
                .iter()
                .map(|d| d.name.to_string())
                .collect()
        };
        let shown = names(&mut e);
        assert!(
            shown.contains(&"find_tools".to_string()) && shown.contains(&"use_tool".to_string())
        );
        assert!(
            !shown.contains(&"design".to_string()) && shown.len() < all,
            "{shown:?}"
        );
        // Found by category, run through use_tool; the list never changes.
        let out = e.call_tool("find_tools", serde_json::json!({"category": "windows"}));
        assert!(
            out.text.starts_with("window: ") && out.text.contains("use_tool"),
            "{}",
            out.text
        );
        assert_eq!(names(&mut e), shown);
        let out = e.call_tool(
            "use_tool",
            serde_json::json!({"name": "window", "arguments": {"app": "TextEdit", "action": "list"}}),
        );
        assert!(!out.is_error, "{}", out.text);
        // Found by what it does.
        let out = e.call_tool("find_tools", serde_json::json!({"query": "clipboard"}));
        assert!(out.text.contains("get_clipboard"), "{}", out.text);
        // Hidden is not forbidden: a direct call still runs.
        assert!(!e.call_tool("get_clipboard", serde_json::json!({})).is_error);
        // use_tool can't run the manager itself, or an unknown tool.
        assert!(
            e.call_tool("use_tool", serde_json::json!({"name": "find_tools"}))
                .is_error
        );
        assert!(
            e.call_tool("use_tool", serde_json::json!({"name": "nope"}))
                .is_error
        );

        // Words that name nothing find nothing; a query brings the best few.
        let out = e.call_tool("find_tools", serde_json::json!({"query": "the a to"}));
        assert!(out.text.starts_with("No tools match"), "{}", out.text);
        let out = e.call_tool("find_tools", serde_json::json!({"query": "draw a star"}));
        assert!(out.text.starts_with("draw: "), "{}", out.text);
        assert!(
            out.text.matches("arguments: {").count() <= 3,
            "{}",
            out.text
        );
        // Arguments once: asked again, a line; again=true repeats them.
        let out = e.call_tool("find_tools", serde_json::json!({"name": "window"}));
        assert!(
            out.text.contains("arguments: as shown before"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "find_tools",
            serde_json::json!({"name": "window", "again": true}),
        );
        assert!(out.text.contains("arguments: {"), "{}", out.text);
        // A tool never shown, called wrong: its arguments come with the
        // error, once.
        let wrong = serde_json::json!({"name": "locate", "arguments": {"nope": 1}});
        let out = e.call_tool("use_tool", wrong.clone());
        assert!(
            out.is_error && out.text.contains("locate takes: {"),
            "{}",
            out.text
        );
        let out = e.call_tool("use_tool", wrong);
        assert!(!out.text.contains("locate takes"), "{}", out.text);

        // list_changed: what is found joins the list, and stays.
        cfg.tools.manager = ToolManager::ListChanged;
        e.set_config(ConfigStore::in_memory(cfg));
        assert!(!names(&mut e).contains(&"use_tool".to_string()));
        // A base tool asked for by name changes nothing.
        let before = e.tools_signature();
        let out = e.call_tool("find_tools", serde_json::json!({"name": "click"}));
        assert!(out.text.contains("Already in your tools"), "{}", out.text);
        assert_eq!(e.tools_signature(), before);
        let out = e.call_tool("find_tools", serde_json::json!({"category": "design"}));
        assert!(out.text.contains("Added to your tools"), "{}", out.text);
        let now = names(&mut e);
        assert!(now.contains(&"design".to_string()) && now.contains(&"locate".to_string()));
    }

    #[test]
    fn the_last_app_named_stands_in_for_a_missing_one() {
        let mut e = engine();
        let mut cfg = e.store().config.clone();
        cfg.tools.default_app = true;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool("find_element", serde_json::json!({"name": "Bold"}));
        assert!(
            !out.is_error && out.text.contains("\"Bold\""),
            "{}",
            out.text
        );
        // Off: an error, as before.
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        assert!(
            e.call_tool("find_element", serde_json::json!({"name": "Bold"}))
                .is_error
        );
    }

    #[test]
    fn explanations_can_always_be_given_in_full() {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.tree.brief_repeats = false;
        // (A compact look that changed nothing is one line.)
        cfg.tree.compact = false;
        let mut e =
            Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        for _ in 0..2 {
            let out = state_of(&mut e, serde_json::json!({"screenshot": false}));
            assert!(
                out.text
                    .contains("not attached (pass screenshot=true for one)"),
                "{}",
                out.text
            );
        }
    }

    #[test]
    fn explanations_are_given_once_then_kept_short() {
        let mut e = nav_engine(false);
        let mut cfg = e.store().config.clone();
        cfg.tree.compact = false;
        e.set_config(ConfigStore::in_memory(cfg));
        state_of(&mut e, serde_json::json!({}));
        let first = state_of(&mut e, serde_json::json!({"screenshot": false}));
        assert!(
            first
                .text
                .contains("not attached (pass screenshot=true for one)"),
            "{}",
            first.text
        );
        let again = state_of(&mut e, serde_json::json!({"screenshot": false}));
        assert!(
            again.text.contains("Screenshot: not attached."),
            "{}",
            again.text
        );
        assert!(again.text.len() < first.text.len());
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
        let shot =
            |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("screenshot", args);
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

    #[test]
    fn region_parameters_are_never_ignored() {
        let mut e = always_shot_engine();
        let shot =
            |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("screenshot", args);
        // x/y/width/height alone: a region of the screen.
        let out = shot(
            &mut e,
            serde_json::json!({"x": 10, "y": 20, "width": 300, "height": 200}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("region (10, 20) 300x200"), "{}", out.text);
        assert_eq!(out.image.unwrap().width, 300);
        // Some of them missing: said, not a full screen instead.
        let out = shot(&mut e, serde_json::json!({"x": 10, "y": 20}));
        assert!(
            out.is_error && out.text.contains("width and height"),
            "{}",
            out.text
        );
        // With another mode, or with an app: an error that says why.
        let out = shot(
            &mut e,
            serde_json::json!({"mode": "full", "x": 1, "y": 1, "width": 9, "height": 9}),
        );
        assert!(
            out.is_error && out.text.contains("mode=full"),
            "{}",
            out.text
        );
        let out = shot(
            &mut e,
            serde_json::json!({"app": "TextEdit", "x": 1, "y": 1, "width": 9, "height": 9}),
        );
        assert!(
            out.is_error && out.text.contains("element_index"),
            "{}",
            out.text
        );
    }

    #[test]
    fn a_window_screenshot_is_not_sent_again_and_pictures_are_numbered() {
        let mut e = always_shot_engine();
        let shot =
            |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("screenshot", args);
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(
            out.text.contains("Screenshot #1: 800x600 px."),
            "{}",
            out.text
        );
        // The window looks as in screenshot #1: not sent again.
        let out = shot(&mut e, serde_json::json!({"app": "TextEdit"}));
        assert!(out.image.is_none(), "{}", out.text);
        assert!(
            out.text.contains("unchanged") && out.text.contains("#1"),
            "{}",
            out.text
        );
        // A small change: only that part, placed in #1.
        e.backend_mut().patch = Some((Rect::new(500.0, 400.0, 60.0, 30.0), 20));
        let out = shot(&mut e, serde_json::json!({"app": "TextEdit"}));
        let part = out.image.as_ref().expect("the changed part");
        assert!(part.width < 400, "{}", out.text);
        assert!(
            out.text.contains("Screenshot #2") && out.text.contains("of #1"),
            "{}",
            out.text
        );
        // get_app_state now knows the model has it.
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.image.is_none(), "{}", out.text);
        assert!(
            out.text.contains("unchanged") && out.text.contains("(#1)"),
            "{}",
            out.text
        );
        // mode=window: all of it, always, and x/y then refer to it.
        let out = shot(
            &mut e,
            serde_json::json!({"app": "TextEdit", "mode": "window"}),
        );
        assert_eq!(out.image.as_ref().unwrap().width, 800, "{}", out.text);
        assert!(out.text.contains("Screenshot #3 of"), "{}", out.text);
        e.backend_mut().patch = Some((Rect::new(100.0, 100.0, 40.0, 40.0), 60));
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(
            out.text.contains("Screenshot #4") && out.text.contains("#3"),
            "{}",
            out.text
        );
    }

    // -- window management ----------------------------------------------------

    #[test]
    fn windows_can_be_arranged() {
        let mut e = engine();
        let win =
            |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("window", args);
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
        let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
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
        );
        assert!(r.is_error && r.text.contains("OCR"), "{}", r.text);
        // An unchanged picture isn't read again.
        let runs = e.backend().ocr_runs;
        e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
        assert_eq!(e.backend().ocr_runs, runs);
        e.backend_mut().fill = 90;
        e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
        assert_eq!(e.backend().ocr_runs, runs + 1);
    }

    /// A drawing app: a toolbar full of buttons over a canvas the tree
    /// knows nothing about, with something drawn on it.
    fn board_engine(blind_regions: bool) -> Engine<MockBackend> {
        let win = Rect::new(0.0, 0.0, 800.0, 600.0);
        let mut elements = vec![MockElement::new(1, "window", "Board", win)];
        for i in 0..20u64 {
            elements.push(
                MockElement::new(
                    10 + i,
                    "button",
                    &format!("Tool {i}"),
                    Rect::new(i as f64 * 40.0, 0.0, 38.0, 30.0),
                )
                .child_of(1),
            );
        }
        elements.push(
            MockElement::new(2, "canvas", "", Rect::new(0.0, 40.0, 800.0, 560.0)).child_of(1),
        );
        let app = MockApp {
            info: AppInfo {
                name: "Board".into(),
                id: "board".into(),
                pid: 78,
                exe: None,
                frontmost: true,
                hidden: false,
            },
            windows: vec![MockWindow {
                id: 9,
                title: "Board".into(),
                bounds: win,
                root: 1,
                focused: true,
            }],
            elements,
        };
        let mut backend = MockBackend::new();
        backend.add_app(app);
        backend.ocr_text = Some(vec![
            line("DELTA", 500.0, 400.0),
            line("Tool 3", 120.0, 5.0),
        ]);
        // Four framed boxes, as the canvas draws them.
        backend.ink = [(60.0, 80.0), (460.0, 80.0), (60.0, 360.0), (460.0, 360.0)]
            .iter()
            .map(|&(x, y)| {
                vec![
                    Point::new(x, y),
                    Point::new(x + 200.0, y),
                    Point::new(x + 200.0, y + 120.0),
                    Point::new(x, y + 120.0),
                    Point::new(x, y),
                ]
            })
            .collect();
        let mut cfg = Config::default();
        cfg.ocr.tesseract_path = "/nonexistent/tesseract".into();
        cfg.ocr.blind_regions = blind_regions;
        cfg.cache.snapshot_ttl_ms = 0;
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
    }

    #[test]
    fn a_canvas_under_a_full_toolbar_is_read_and_watched() {
        let look = |e: &mut Engine<MockBackend>| {
            e.call_tool("get_app_state", serde_json::json!({"app": "Board"}))
        };
        // Off (the default): 20 buttons look like a well-described window.
        let mut e = board_engine(false);
        let out = look(&mut e);
        assert!(!out.text.contains("DELTA"), "{}", out.text);

        let mut e = board_engine(true);
        let out = look(&mut e);
        assert!(out.text.contains("ocr text \"DELTA\""), "{}", out.text);
        assert!(
            out.text.contains("no accessibility information"),
            "{}",
            out.text
        );
        // Only what is in the area, and not what the tree already says.
        assert!(!out.text.contains("ocr text \"Tool 3\""), "{}", out.text);
        assert!(out.image.is_some());
        let delta = index_of_name(&out.text, "\"DELTA\"");
        let r = e.call_tool(
            "click",
            serde_json::json!({"app": "Board", "element_index": delta}),
        );
        assert!(!r.is_error, "{}", r.text);

        // Nothing changed: not read again, the picture not sent again.
        let runs = e.backend().ocr_runs;
        let out = look(&mut e);
        assert_eq!(e.backend().ocr_runs, runs, "{}", out.text);
        assert!(out.image.is_none(), "{}", out.text);
        // Something drawn on the canvas: the tree can't tell, the pixels
        // can.
        e.backend_mut()
            .ink
            .push(vec![Point::new(200.0, 500.0), Point::new(260.0, 520.0)]);
        let out = look(&mut e);
        assert!(out.image.is_some(), "{}", out.text);
        assert!(out.text.contains("Screenshot #"), "{}", out.text);

        // An empty canvas is just background: nothing to read.
        let mut e = board_engine(true);
        e.backend_mut().ink.clear();
        let out = look(&mut e);
        assert!(
            !out.text.contains("no accessibility information"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("DELTA"), "{}", out.text);
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
        let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
        assert!(
            out.text.contains("Text recognition unavailable"),
            "{}",
            out.text
        );
        let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
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
        let out = e.call_tool("get_notifications", serde_json::json!({}));
        assert!(
            out.is_error && out.text.contains("off in settings"),
            "{}",
            out.text
        );

        let mut cfg = e.store().config.clone();
        cfg.notifications.enabled = true;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = e.call_tool("get_notifications", serde_json::json!({}));
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("[10 min ago] Slack — Ada: Lunch at 1?"),
            "{}",
            out.text
        );
        assert!(out.text.contains("code is ••••••"), "{}", out.text);
        assert!(!out.text.contains("482913"), "{}", out.text);
        assert!(out.text.contains("•••• •••• •••• 1111"), "{}", out.text);
        assert!(out.text.contains("4 recent notification"), "{}", out.text);
        // notifications.apps narrows it to a list.
        let mut cfg = e.store().config.clone();
        cfg.notifications.apps = vec!["Slack".into(), "Bank".into(), "Shop".into()];
        e.set_config(ConfigStore::in_memory(cfg));
        let out = e.call_tool("get_notifications", serde_json::json!({}));
        assert!(!out.text.contains("Vault"), "{}", out.text);
        assert!(out.text.contains("1 from apps not in"), "{}", out.text);
        let out = e.call_tool("get_notifications", serde_json::json!({"app": "slack"}));
        assert!(out.text.contains("1 recent notification"), "{}", out.text);
        let out = e.call_tool("get_notifications", serde_json::json!({"limit": 1}));
        assert!(
            out.text.contains("Receipt") && !out.text.contains("Ada"),
            "{}",
            out.text
        );
    }

    // -- expect, click by name, batch lines, rebase -------------------------

    /// The editor with a "Save As" button that opens a dialog (a second,
    /// focused window).
    fn dialog_engine(cfg: Config) -> Engine<MockBackend> {
        let mut main = MockBackend::text_editor(4242);
        main.elements
            .push(button(20, "Save As", 2, 200.0).with_actions(&["AXPress"]));
        let mut with_dialog = main.clone();
        with_dialog.windows[0].focused = false;
        with_dialog.windows.push(MockWindow {
            id: 2,
            title: "Save As".into(),
            bounds: Rect::new(100.0, 100.0, 400.0, 200.0),
            root: 40,
            focused: true,
        });
        with_dialog.elements.extend([
            MockElement::new(
                40,
                "window",
                "Save As",
                Rect::new(100.0, 100.0, 400.0, 200.0),
            ),
            MockElement::new(
                41,
                "text field",
                "Name",
                Rect::new(120.0, 130.0, 200.0, 24.0),
            )
            .child_of(40)
            .editable(),
            MockElement::new(42, "button", "Save", Rect::new(120.0, 170.0, 60.0, 24.0))
                .child_of(40)
                .with_actions(&["AXPress"]),
        ]);
        for el in &mut with_dialog.elements {
            el.states.enabled = true;
        }
        let mut backend = MockBackend::new();
        backend.add_app(main);
        backend.on_press.insert(20, with_dialog);
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
    }

    fn no_wait() -> Config {
        let mut cfg = Config::default();
        cfg.timing.expect_wait_ms = 0;
        cfg
    }

    #[test]
    fn expect_says_confirmed_not_seen_or_uncertain() {
        let mut e = dialog_engine(no_wait());
        // Nothing seen of the app before: nothing to compare with.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "name": "Bold", "expect": "change"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.contains("Expected change: uncertain"),
            "{}",
            out.text
        );
        state_of(&mut e, serde_json::json!({}));
        let bold = index_named(&e, 4242, "Bold");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": bold, "expect": "dialog"}),
        );
        let first = out.text.lines().next().unwrap();
        assert!(first.contains("Expected dialog: not seen"), "{}", out.text);
        assert!(first.contains("Look before"), "{}", out.text);
        let save_as = index_named(&e, 4242, "Save As");
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": save_as, "expect": "dialog"}),
        );
        let first = out.text.lines().next().unwrap();
        assert!(
            first.contains("Expected dialog: confirmed (window \"Save As\")"),
            "{}",
            out.text
        );
        // A text that should be on screen, and a value.
        let name = index_named(&e, 4242, "Name");
        let out = e.call_tool(
            "set_value",
            serde_json::json!({"app": "TextEdit", "element_index": name, "value": "report.txt", "expect": "value"}),
        );
        assert!(
            out.text.contains("Expected value: confirmed"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "set_value",
            serde_json::json!({"app": "TextEdit", "element_index": name, "value": "notes.txt", "expect": "notes.txt"}),
        );
        assert!(
            out.text.contains("Expected notes.txt: confirmed (") && out.text.contains("text field"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "set_value",
            serde_json::json!({"app": "TextEdit", "element_index": name, "value": "a", "expect": "Saved!"}),
        );
        assert!(
            out.text.contains("Expected Saved!: not seen"),
            "{}",
            out.text
        );
    }

    #[test]
    fn click_by_name_needs_one_element() {
        let mut e = dialog_engine(no_wait());
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "name": "bold"}),
        );
        assert!(!out.is_error, "{}", out.text);
        let bold = index_named(&e, 4242, "Bold");
        assert!(
            out.text
                .starts_with(&format!("Pressed button \"Bold\" ({bold})")),
            "{}",
            out.text
        );
        assert!(
            e.backend()
                .events
                .contains(&Event::Action(3, "AXPress".into()))
        );
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "name": "Print"}),
        );
        assert!(
            out.is_error && out.text.contains("no element"),
            "{}",
            out.text
        );
        // "Save" is part of "Save As" only: found by its part.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "name": "save", "role": "button"}),
        );
        assert!(!out.is_error, "{}", out.text);
        // Now "Save" names two buttons ("Save" exactly, and "Save As"):
        // the exact one wins.
        let out = e.call_tool(
            "click",
            serde_json::json!({"app": "TextEdit", "name": "Save"}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("button \"Save\" ("), "{}", out.text);
        let out = e.call_tool("click", serde_json::json!({"app": "TextEdit", "name": "a"}));
        assert!(out.is_error, "{}", out.text);
        assert!(
            out.text.contains("elements match, so nothing was clicked"),
            "{}",
            out.text
        );
    }

    #[test]
    fn batch_lines_run_and_one_report_ends_them() {
        let mut e = dialog_engine(no_wait());
        state_of(&mut e, serde_json::json!({}));
        let doc = index_named(&e, 4242, "Document");
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                format!("set {doc} \"Dear Ada\""),
                "click \"Bold\"",
                "key cmd+a",
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Ran 3 step(s):"), "{}", out.text);
        assert!(out.text.contains("1. set_value"), "{}", out.text);
        assert!(
            out.text.contains("2. click — Pressed button \"Bold\""),
            "{}",
            out.text
        );
        assert!(out.text.contains("3. press_key"), "{}", out.text);
        assert!(out.text.contains("State after the steps"), "{}", out.text);
        assert!(out.text.contains("Dear Ada"), "{}", out.text);
        // A bad line is refused before anything runs.
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": ["jump 3"]}),
        );
        assert!(
            out.is_error && out.text.contains("unknown step"),
            "{}",
            out.text
        );
    }

    #[test]
    fn batch_stops_on_a_window_it_did_not_expect() {
        let mut e = dialog_engine(no_wait());
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": ["click \"Save As\"", "type \"x\""]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.starts_with("Ran 1 of 2 step(s):"), "{}", out.text);
        assert!(
            out.text
                .contains("Stopped after step 1: window \"Save As\" came up"),
            "{}",
            out.text
        );
        assert!(
            !e.backend()
                .events
                .iter()
                .any(|ev| matches!(ev, Event::Type(..)))
        );
        assert!(out.text.contains("State after the steps"), "{}", out.text);

        // Expected, it goes on.
        let mut e = dialog_engine(no_wait());
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                "click \"Save As\" expect dialog",
                "set 99 \"x\""
            ]}),
        );
        assert!(out.text.contains("2. set_value"), "{}", out.text);
        assert!(
            out.text.contains("Expected dialog: confirmed"),
            "{}",
            out.text
        );

        // An expectation not met stops it too.
        let mut e = dialog_engine(no_wait());
        state_of(&mut e, serde_json::json!({}));
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                "click \"Bold\" expect dialog",
                "type \"x\""
            ]}),
        );
        assert!(
            out.text.contains("what it expected wasn't confirmed"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("2. type_text"), "{}", out.text);
    }

    #[test]
    fn a_long_conversation_gets_the_whole_tree_again() {
        let mut cfg = Config::default();
        cfg.cache.rebase_after_tokens = 50;
        let mut e = dialog_engine(cfg);
        state_of(&mut e, serde_json::json!({}));
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("sent whole again"), "{}", out.text);
        assert!(out.text.contains("text area \"Document\""), "{}", out.text);
        assert!(out.image.is_some());
        // Right after, nothing much has been said: a short answer again.
        let mut cfg = e.store().config.clone();
        cfg.cache.rebase_after_tokens = 100_000;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(!out.text.contains("sent whole again"), "{}", out.text);
        assert!(!out.text.contains("Document"), "{}", out.text);
    }

    #[test]
    fn a_table_cell_press_that_selects_nothing_is_clicked() {
        let mut app = MockBackend::text_editor(4242);
        app.elements.push(
            MockElement::new(60, "table", "Stock", Rect::new(0.0, 100.0, 400.0, 200.0)).child_of(1),
        );
        app.elements.push(
            MockElement::new(61, "cell", "SKU-1", Rect::new(0.0, 100.0, 100.0, 20.0))
                .child_of(60)
                .with_actions(&["AXPress"]),
        );
        let mut backend = MockBackend::new();
        backend.add_app(app);
        let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
            .with_time(Instant::now, |_| {});
        state_of(&mut e, serde_json::json!({}));
        let cell = index_named(&e, 4242, "SKU-1");
        let out = press(&mut e, cell);
        assert!(
            out.text.contains("clicked it with the mouse too"),
            "{}",
            out.text
        );
        assert!(
            e.backend()
                .events
                .iter()
                .any(|ev| matches!(ev, Event::Click(..)))
        );
        // Not so for a button: pressing it again could repeat what it does.
        let bold = index_named(&e, 4242, "Bold");
        let n = e.backend().events.len();
        press(&mut e, bold);
        assert!(
            !e.backend().events[n..]
                .iter()
                .any(|ev| matches!(ev, Event::Click(..)))
        );
    }

    #[test]
    fn designs_and_scenes_say_what_changed() {
        let mut e = steps_engine();
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "logo", "size": [200, 100], "background": "#FFFFFF",
                "add": ["sun ellipse 50 50 20 20 fill #FFCC00",
                        "sky rect 0 0 200 40 fill #3366FF",
                        "title text \"Hi\" at 150 60 size 20 fill #222222"]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.image.is_some());
        assert!(
            out.text.contains("3 layers, back to front: sun ellipse"),
            "{}",
            out.text
        );
        assert!(out.text.contains("To paint it in an app"), "{}", out.text);
        // One layer changes: only it is said, with a new picture.
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "logo", "change": ["sun fill #FF0000"]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            out.text.starts_with("Design \"logo\": 3 layers; since the last answer: changed sun ellipse x 30 y 30 w 40 h 40 fill #FF0000."),
            "{}",
            out.text
        );
        assert!(!out.text.contains("sky rect"), "{}", out.text);
        assert!(!out.text.contains("On the picture, cells"), "{}", out.text);
        assert!(out.image.is_some());
        // A change that changes nothing: no picture.
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "logo", "change": ["sun fill #FF0000"]}),
        );
        assert!(out.text.contains("3 layers, as before."), "{}", out.text);
        assert!(
            out.text.contains("The picture is as before."),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("The steps to paint it are as before."),
            "{}",
            out.text
        );
        assert!(out.image.is_none());
        // Removing and reordering.
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "logo", "remove": ["title"], "order": [{"id": "sun", "to": "front"}]}),
        );
        assert!(out.text.contains("removed title"), "{}", out.text);
        assert!(
            out.text.contains("back to front now sky, sun"),
            "{}",
            out.text
        );
        // A look gets all of it.
        let out = e.call_tool("design", serde_json::json!({"name": "logo"}));
        assert!(out.text.contains("back to front: sky rect"), "{}", out.text);
        assert!(out.image.is_some());
        // Steps only when asked.
        let mut cfg = e.store().config.clone();
        cfg.tools.design_steps = crate::config::DesignSteps::Asked;
        e.set_config(ConfigStore::in_memory(cfg));
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "logo", "change": ["sky fill #2255EE"]}),
        );
        assert!(
            out.text.contains("2 step(s) to paint it (show steps=true"),
            "{}",
            out.text
        );
        let out = e.call_tool(
            "design",
            serde_json::json!({"name": "logo", "show": {"steps": true}}),
        );
        assert!(out.text.contains("To paint it in an app"), "{}", out.text);

        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "add": [
                "seat cylinder 0.4 0.05 at 0 0 0.475 color #884422",
                "leg box 0.05 0.05 0.45 at 0 0 0.225"
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(out.text.contains("seat cylinder"), "{}", out.text);
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "change": ["seat color #000000"]}),
        );
        assert!(
            out.text
                .contains("Since the last answer: changed seat cylinder"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("leg box"), "{}", out.text);
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "change": ["seat color #000000"]}),
        );
        assert!(out.text.contains("Objects as before."), "{}", out.text);
        assert!(
            out.text.contains("The picture is as before."),
            "{}",
            out.text
        );
        assert!(out.image.is_none());
        let out = e.call_tool(
            "scene",
            serde_json::json!({"name": "stool", "add": ["x wobble 1 1 1"]}),
        );
        assert!(
            out.is_error && out.text.contains("`wobble` is not a field"),
            "{}",
            out.text
        );
    }

    #[test]
    fn results_say_which_earlier_ones_they_repeat() {
        let mut cfg = Config::default();
        cfg.server.result_meta = true;
        let mut e = dialog_engine(cfg);
        let meta = |e: &mut Engine<MockBackend>| e.take_result_meta().expect("meta");
        state_of(&mut e, serde_json::json!({}));
        let m = meta(&mut e);
        assert_eq!(m["zero-use-computer/result"], 1);
        assert_eq!(m["zero-use-computer/supersedes"], serde_json::json!([]));
        let bold = index_named(&e, 4242, "Bold");
        press(&mut e, bold);
        assert_eq!(meta(&mut e)["zero-use-computer/result"], 2);
        // A diff with a whole new picture: the old picture is replaced.
        let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
        assert!(out.image.is_some());
        let m = meta(&mut e);
        assert_eq!(m["zero-use-computer/supersedes"], serde_json::json!([]));
        assert_eq!(
            m["zero-use-computer/supersedes-images"],
            serde_json::json!([1])
        );
        // The whole tree again: both looks are repeated by it.
        state_of(&mut e, serde_json::json!({"rebase": true}));
        let m = meta(&mut e);
        assert_eq!(m["zero-use-computer/supersedes"], serde_json::json!([1, 3]));
        // Off: no meta.
        assert!(Config::default().server.result_meta);
        let mut off = Config::default();
        off.server.result_meta = false;
        let mut e = dialog_engine(off);
        state_of(&mut e, serde_json::json!({}));
        assert!(e.take_result_meta().is_none());
    }

    // -- what batches and scripts saw stays theirs ------------------------
    #[test]
    fn a_batch_leaves_no_unseen_view_in_screen_memory() {
        let mut e = nav_engine(false);
        state_of(&mut e, serde_json::json!({})); // the model sees page A
        let next = index_named(&e, 7, "Next");
        // The document changes; the model hasn't looked.
        e.backend_mut()
            .app_mut(7)
            .unwrap()
            .elements
            .iter_mut()
            .find(|el| el.handle == 5)
            .unwrap()
            .value = Some("Edited".into());
        // A batch looks (the batch shows one line per step, never the tree),
        // then moves on to page B and looks there.
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                {"tool": "wait_for", "arguments": {"name": "Bold"}},
                {"tool": "get_app_state", "arguments": {}},
                {"tool": "click", "arguments": {"element_index": next}},
                {"tool": "get_app_state", "arguments": {}},
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(
            !out.text.contains("Edited"),
            "the batch never shows the tree"
        );
        // Page A comes back, still edited.
        let mut edited = page_a(7);
        edited
            .elements
            .iter_mut()
            .find(|el| el.handle == 5)
            .unwrap()
            .value = Some("Edited".into());
        e.backend_mut().on_press.insert(30, edited);
        state_of(&mut e, serde_json::json!({}));
        press_named(&mut e, 7, "Back");
        let out = state_of(&mut e, serde_json::json!({}));
        // The model never saw "Edited": it must be reported.
        assert!(
            out.text.contains("Edited"),
            "model told page A is as it saw it:\n{}",
            out.text
        );
    }

    #[test]
    fn batch_steps_dont_use_up_first_time_explanations() {
        let mut e = nav_engine(false);
        state_of(&mut e, serde_json::json!({}));
        e.backend_mut()
            .app_mut(7)
            .unwrap()
            .elements
            .iter_mut()
            .find(|el| el.handle == 5)
            .unwrap()
            .value = Some("Edited".into());
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                {"tool": "wait_for", "arguments": {"name": "Bold"}},
                {"tool": "get_app_state", "arguments": {}},
            ]}),
        );
        assert!(!out.text.contains("Changes since"), "{}", out.text);
        // The model's first diff: the intro in full.
        let out = state_of(&mut e, serde_json::json!({}));
        assert!(out.text.contains("Edited"), "{}", out.text);
        assert!(out.text.contains(tree::DIFF_INTRO), "{}", out.text);
    }

    #[test]
    fn the_restless_note_is_no_change_elsewhere() {
        let mut e = engine();
        state_of(&mut e, serde_json::json!({}));
        // What render() appends for 2 elements that keep changing (indices 7, 8)
        // after the one relevant change.
        let text = "Changes:\n~ 4 text area \"Document\" value=\"x\"  (was: …)\n~ 12 element(s) that keep changing on their own left out: 7–8\n";
        let doc = index_named(&e, 4242, "Document");
        e.target = Some((4242, doc));
        let (shown, others) = e.relevant_changes(4242, text);
        assert_eq!(others, 0, "the note is not a change elsewhere:\n{shown}");
    }

    #[test]
    fn a_batch_doesnt_make_a_never_seen_screen_count_as_seen() {
        let mut e = nav_engine(false);
        state_of(&mut e, serde_json::json!({})); // page A
        let next = index_named(&e, 7, "Next");
        let out = e.call_tool(
            "batch",
            serde_json::json!({"app": "TextEdit", "steps": [
                {"tool": "click", "arguments": {"element_index": next}},
                {"tool": "get_app_state", "arguments": {}},
                {"tool": "click", "arguments": {"name": "Back"}},
                {"tool": "get_app_state", "arguments": {}},
            ]}),
        );
        assert!(!out.is_error, "{}", out.text);
        assert!(!out.text.contains("Result 0"), "{}", out.text);
        press_named(&mut e, 7, "Next");
        let out = state_of(&mut e, serde_json::json!({}));
        // The model has never been shown page B's tree.
        assert!(
            out.text.contains("Result 0"),
            "page B said to be known:\n{}",
            out.text
        );
    }

    #[test]
    fn own_typing_is_never_taken_for_a_restless_element() {
        let mut e = engine();
        assert!(e.store().config.tree.quiet_volatile, "on by default");
        e.backend_mut()
            .app_mut(4242)
            .unwrap()
            .elements
            .iter_mut()
            .find(|el| el.handle == 5)
            .unwrap()
            .states
            .focused = true;
        state_of(&mut e, serde_json::json!({}));
        let mut last = String::new();
        for t in ["1", "2", "3", "4"] {
            let out = e.call_tool(
                "type_text",
                serde_json::json!({"app": "TextEdit", "text": t}),
            );
            assert!(!out.is_error, "{}", out.text);
            last = out.text;
        }
        assert!(
            last.contains("Hello1234"),
            "the model's own typing hidden:\n{last}"
        );
    }
}
