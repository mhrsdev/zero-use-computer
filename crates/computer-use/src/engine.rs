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

mod actions;
mod agents;
mod batch;
mod boards;
mod deciding;

/// How many things `decide` judges at a time, at most (scripts too).
pub(crate) fn deciding_max_items() -> usize {
    deciding::MAX_ITEMS
}
mod drawing;
mod expect;
mod finding;
mod looking;
mod manager;
mod observe;
mod overlay;
mod progress;
mod screenshot;
mod scripting;
mod system;

pub(crate) use drawing::draw_shapes;
pub use progress::{Progress, ProgressSink};

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
    /// `Engine::inputs` at the last look committed: changes since are the
    /// model's own doing when it has moved.
    volatile_inputs: u64,
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
    /// The last picture of a design sent whole or in part, as the model
    /// now has it, and the marks drawn over it: a change then sends only
    /// the part that changed.
    shot: Option<std::sync::Arc<Capture>>,
    marks: Option<crate::design::Extras>,
}

/// What the result of the current top-level call holds, for hosts that
/// drop results a later one repeats ([server] result_meta).
#[derive(Default)]
struct ResultNote {
    /// Apps whose whole tree it shows (a look), and apps it looked at
    /// with a diff.
    looks_full: Vec<(u32, u32)>,
    looks_diff: Vec<(u32, u32)>,
    /// Apps it has a whole picture of, and a changed part of.
    pictures_whole: Vec<(u32, u32)>,
    pictures_part: Vec<(u32, u32)>,
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
    /// Results that brought other agents' messages: never superseded (a
    /// later look doesn't repeat them).
    kept: HashSet<u64>,
    /// By app and screen: a look stands in only for looks of the same
    /// screen (going back to another shows it as it was then).
    looks: HashMap<(u32, u32), Vec<u64>>,
    pictures: HashMap<(u32, u32), Vec<u64>>,
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
/// Hubs that went away and were joined again at once, at most.
const MAX_HUB_LOSSES: u32 = 3;

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

/// What one tool call (with the calls it runs: batch steps, a script's
/// tools) is doing. Nothing in it outlives the call, so a call that fails
/// unexpectedly is undone by starting it afresh.
#[derive(Default)]
struct CallState {
    pending_images: Vec<PendingImage>,
    /// Nesting of `call` (batch steps run inside a call).
    depth: u32,
    /// The call depth of a batch's steps: their reports are never shown
    /// (the batch reports once, at the end), so they aren't made.
    quiet_depth: Option<u32>,
    /// The element the current action acts on (app pid, index), for a
    /// report of the changes around it ([tree] report = "relevant").
    target: Option<(u32, u32)>,
    /// What the last action's `expect` found (a batch stops on anything
    /// but confirmed).
    last_expect: Option<Outcome>,
    /// Read text off the screen in the next observe (get_app_state ocr=true).
    force_ocr: bool,
    /// Reuse the last OCR result instead of reading again (while settling).
    ocr_reuse: bool,
    /// A window capture taken for OCR at this epoch, reused as the screenshot.
    last_capture: Option<(u32, u64, u64, Capture)>,
    /// A script is running: its calls can't start another (by any route,
    /// `batch` included), or scripts could nest until the stack runs out.
    in_script: bool,
    /// One taken during this call: it becomes `screen_shot` only if it is
    /// the image the call returns (a batch returns only its last image).
    pending_screen_shot: Option<ScreenShot>,
    /// Tools the running script has called (its progress).
    script_calls: u32,
    /// The keyboard and mouse are this agent's (the hub's turn).
    turn: bool,
}

pub struct Engine<B: Backend> {
    /// What the call in progress is doing, reset as a whole when one
    /// fails unexpectedly.
    ctx: CallState,
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
    /// What is wrong with the settings file, while it can't be used; told
    /// to the agent once.
    settings_problem: Option<String>,
    settings_problem_shown: bool,
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
    /// Actions sent so far (counted as each starts).
    inputs: u64,
    /// The action epoch whose result has a fresh snapshot (after settling).
    settled: Option<u64>,
    /// Per app (pid): how its tree has shown what an action changed, so
    /// an action that changed nothing is waited on no longer than needed.
    promptness: HashMap<u32, Promptness>,
    /// The last full-screen screenshot sent.
    screen_shot: Option<ScreenShot>,
    /// Screenshots handed out so far: each gets the next number, so a
    /// changed part can name the picture it patches.
    shots: u32,
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
    /// The last result as data, for its `structuredContent` ([server]
    /// structured_output).
    structured: Option<serde_json::Value>,
    /// Why OCR isn't available, once found out (told to the model once).
    ocr_note: Option<String>,
    ocr_note_shown: bool,
    /// The start of a tree an action's result showed (cut short): its app,
    /// screen, a hash of the whole text and the lines shown. If the next
    /// get_app_state renders the same text, those lines aren't sent again.
    partial_report: Option<(u32, u32, u64, usize)>,
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
    /// Where the progress of the calls goes, set by the host.
    progress: Option<progress::Reporter>,
    /// The MCP client this engine serves, for the other agents.
    client: String,
    /// This engine's number on the desktop, kept for a hub started again.
    hub_agent: Option<u32>,
    /// Hubs lost and joined again at once (a few, then the usual waits).
    hub_losses: u32,
    /// Where the hub's token is, instead of the server's folder (tests).
    hub_home: Option<std::path::PathBuf>,
    /// Windows put in this agent's part of the screen, and the part.
    arranged: HashMap<u64, crate::types::Rect>,
    /// The app the other agents were told this one works with.
    doing: Option<String>,
    /// How many agents the model was last told share the desktop.
    agents_told: usize,
    /// Messages sent to other agents in the last minute.
    sent: Vec<Instant>,
    clock: Box<dyn Fn() -> Instant + Send>,
    sleep: Box<dyn Fn(Duration) + Send>,
}

/// What the model had seen before a batch or a script ran.
pub(super) struct SeenBefore {
    known: HashMap<u32, Option<Screen>>,
    memory: HashMap<u32, crate::screens::View>,
    hints: HashSet<&'static str>,
    /// What the model was shown of each app besides its tree: the header
    /// it has, and the icons it was shown (a look inside a batch or a
    /// script, whose result the model never sees, mustn't count).
    shown: HashMap<u32, (Option<String>, HashSet<u64>)>,
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
            ctx: CallState::default(),
            backend,
            store,
            states: HashMap::new(),
            memory: ScreenMemory::default(),
            app_cache: None,
            window_cache: HashMap::new(),
            epoch: 0,
            overlay: None,
            overlay_launcher: None,
            overlay_failures: 0,
            overlay_retry_at: None,
            overlay_error: None,
            stop_note_shown: false,
            settings_problem: None,
            settings_problem_shown: false,
            hints: Hints::default(),
            stop: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            last_input: None,
            inputs: 0,
            settled: None,
            promptness: HashMap::new(),
            screen_shot: None,
            shots: 0,
            active_tools: HashSet::new(),
            tools_cache: None,
            schemas_shown: HashMap::new(),
            last_app: None,
            sent_tokens: 0,
            drafts_seen: HashMap::new(),
            result_id: 0,
            note: ResultNote::default(),
            results: Results::default(),
            result_meta: None,
            structured: None,
            traces: Vec::new(),
            designs: Vec::new(),
            scenes: Vec::new(),
            fonts: crate::design::FontCache::default(),
            exports: crate::design::TempFiles::default(),
            scripts,
            judge: Default::default(),
            progress: None,
            client: String::new(),
            hub_agent: None,
            hub_losses: 0,
            hub_home: None,
            arranged: HashMap::new(),
            doing: None,
            agents_told: 0,
            sent: Vec::new(),
            ocr_note: None,
            ocr_note_shown: false,
            partial_report: None,
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
                self.settings_problem = None;
                self.backend.configure(&store.config);
                if store.config.decision != self.store.config.decision {
                    self.judge.forget();
                }
                // Another hub (or none): leave this one, the next call joins.
                let hub_changed = {
                    let (old, new) = (&self.store.config.hub, &store.config.hub);
                    old.enabled != new.enabled || old.port != new.port
                };
                if hub_changed && self.overlay.take().is_some() {
                    self.hub_losses = 0;
                    self.agents_told = 0;
                }
                self.store = store;
                self.scripts.set_dir(self.store.config.script.library());
                self.epoch += 1;
                self.overlay_reconfigure();
            }
            Err(e) => {
                log::warn!("keeping previous settings; {e}");
                let problem =
                    format!("{e}; the settings in use until it is fixed are the ones before");
                if self.settings_problem.as_deref() != Some(&problem) {
                    self.settings_problem = Some(problem);
                    self.settings_problem_shown = false;
                }
                // Maybe read half-written: look once more next time, even if
                // the finished file keeps this modification time.
                self.config_retry = changed;
            }
        }
    }

    /// The settings file couldn't be used when the server started (`problem`
    /// says why), so the defaults are in use: the agent is told once, and a
    /// fixed file is picked up as soon as it is saved.
    pub fn with_settings_problem(mut self, problem: impl Into<String>) -> Self {
        self.settings_problem = Some(format!(
            "{}; the default settings are in use until it is fixed",
            problem.into()
        ));
        self.settings_problem_shown = false;
        self
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
        // Another model: the old one's failures and answers aren't its.
        if store.config.decision != self.store.config.decision {
            self.judge.forget();
        }
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
        self.is_stopped() || self.cancel.load(Ordering::SeqCst) || self.turn_lost()
    }

    /// The hub gave this agent's turn at the keyboard and mouse to another
    /// (it held them too long without a word): it must not act on.
    fn turn_lost(&self) -> bool {
        self.ctx.turn && self.hub_link().is_some_and(|l| l.revoked())
    }

    /// Stop the agent (or let it continue), as the stop key does.
    pub fn set_stopped(&mut self, on: bool) {
        self.stop.store(on, Ordering::SeqCst);
        if self.overlay.is_some() {
            self.overlay_send(OverlayCmd::Stopped { on });
        }
    }

    fn stopped_error(&self) -> Error {
        if !self.is_stopped() && self.turn_lost() {
            return Error::ActionFailed(
                "another agent on this desktop was given the keyboard and mouse (this action held them too long): it stopped part way; look again before going on".into(),
            );
        }
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
            // Input after the engine's own last input (or another agent's
            // on this desktop) is the user's.
            let own = self
                .last_input
                .into_iter()
                .chain(self.others_input())
                .max()
                .map(|t| now.saturating_duration_since(t));
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

    fn state(&self, pid: u32) -> Result<&AppState> {
        self.states
            .get(&pid)
            .filter(|s| s.stamped)
            .ok_or(Error::Internal("no cached state".into()))
    }

    /// Look up an element handle by index in the app's latest state.
    fn element_by_index(&mut self, app: &AppInfo, index: u32) -> Result<ElementHandle> {
        self.ctx.target = Some((app.pid, index));
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
    /// change a little after making it. Only after a while of reads like
    /// that (`Promptness::grace`) does the action count as having changed
    /// nothing.
    fn settle_on(&mut self, app: &AppInfo) {
        use crate::config::SettleMode;
        // How long reads may keep showing the old state before "nothing
        // changed" is believed.
        let grace = if self.store.config.timing.adaptive_grace {
            self.promptness
                .get(&app.pid)
                .copied()
                .unwrap_or_default()
                .grace()
        } else {
            Promptness::default().grace()
        };
        // The latest read is still the one from before the action.
        let before = self.tree_fingerprint(app.pid);
        let mut reads = 0;
        let mut change_seen = false;
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
        let reuse = std::mem::replace(&mut self.ctx.ocr_reuse, true);
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
            reads += 1;
            // (Only against a read from before the action: with none, any
            // read would look like a change.)
            if !change_seen && before.is_some() && now.is_some() && now != before {
                change_seen = true;
                self.promptness
                    .entry(app.pid)
                    .or_default()
                    .saw_change(reads == 1);
            }
            let steady = now.is_some() && now == last;
            if !adaptive || (steady && (now != before || waited >= grace)) {
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
        self.ctx.ocr_reuse = reuse;
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

    // -- tool dispatch -----------------------------------------------------

    /// Run one tool call.
    pub fn call(&mut self, call: ToolCall) -> Result<ToolOutput> {
        if self.halted() {
            // Show it again so the user sees why nothing happens.
            if self.ctx.depth == 0 && self.is_stopped() && self.overlay.is_some() {
                self.overlay_send(OverlayCmd::Stopped { on: true });
            }
            let err = self.stopped_error();
            if self.ctx.depth == 0 {
                // A cancel is for this call only.
                self.cancel.store(false, Ordering::SeqCst);
            }
            return Err(err);
        }
        // Calls inside calls (batch steps, a script's tools) stay shallow.
        if self.ctx.depth >= MAX_DEPTH {
            return Err(Error::InvalidArgs(format!(
                "calls nest at most {MAX_DEPTH} deep (batch steps and scripts run tools inside a call)"
            )));
        }
        if self.ctx.in_script && matches!(call, ToolCall::Script(_)) {
            return Err(Error::InvalidArgs(
                "a script can't start another script, through batch or otherwise: run(name, args) runs a saved script inside it".into(),
            ));
        }
        if self.ctx.depth == 0 {
            self.ctx.target = None;
            // A picture is reused within the call that took it, never by a
            // later one (the screen may have moved on by itself).
            self.ctx.last_capture = None;
        }
        self.ctx.depth += 1;
        if self.ctx.depth == 1 {
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
        if self.ctx.depth == 1 && self.overlay.is_some() {
            let ok = out.as_ref().is_ok_and(|o| !o.is_error);
            self.overlay_send(OverlayCmd::End { ok });
        }
        self.ctx.depth -= 1;
        if self.ctx.depth == 0 {
            self.cancel.store(false, Ordering::SeqCst);
            // Only the image in the final result reaches the model.
            let imaged = out.as_ref().is_ok_and(|o| o.image.is_some());
            if imaged {
                self.commit_images();
            } else {
                self.ctx.pending_images.clear();
                self.ctx.pending_screen_shot = None;
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
            self.inputs += 1;
            // Don't act while the user is using the mouse or keyboard…
            self.wait_for_user()?;
            // …nor while another agent on the desktop is: one at a time.
            self.take_turn()?;
        }
        let report_app = acting.filter(|_| {
            self.store.config.tree.report_changes && self.ctx.quiet_depth != Some(self.ctx.depth)
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
            ToolCall::Agents(a) => self.agents(a),
        };
        if mutating {
            self.last_input = Some((self.clock)());
            self.end_turn(true);
        }
        let out = out?;
        if let Some(query) = pixels_of_app
            && let Ok(app) = self.resolve_app(&query)
        {
            let st = self.states.entry(app.pid).or_default();
            st.pixel_uses = st.pixel_uses.saturating_add(1);
        }
        self.ctx.last_expect = None;
        let checked = match expecting {
            Some((query, what, index)) if !out.is_error => {
                let (outcome, note) = self.check_expect(&query, &what, index, before.as_ref());
                self.ctx.last_expect = Some(outcome);
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
        self.structured = None;
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
                // Undo what the interrupted call left half-done: all of
                // its state (a batch step's quiet reports, the element it
                // aimed at, what its `expect` found…), and the tree start
                // a result may have shown only in part.
                self.end_turn(true);
                self.ctx = CallState::default();
                self.partial_report = None;
                self.cancel.store(false, Ordering::SeqCst);
                self.epoch += 1;
                self.overlay_send(OverlayCmd::End { ok: false });
                ToolOutput::error(&Error::Internal(format!(
                    "{name} failed unexpectedly ({what}); the screen may have changed, call get_app_state before going on"
                )))
            }
        };
        if out.is_error {
            self.structured = None;
        }
        // The other agents on the desktop: how many, and what they said.
        if let Some(note) = self.hub_notes(name == "agents") {
            if let Some(data) = self.structured.as_mut() {
                data["notes"] = serde_json::json!(note.trim());
            }
            out.text.push_str(&note);
        }
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
        if !self.settings_problem_shown
            && let Some(problem) = &self.settings_problem
        {
            self.settings_problem_shown = true;
            out.text.push_str(&format!(
                "\n\nNote: the settings file has an error: {problem}. Tell the user, so they can fix it (it is read again when saved)."
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

    /// The last call's result as data ([`crate::tools::output_schema`]),
    /// when [server] structured_output is on and the call succeeded.
    pub fn take_structured(&mut self) -> Option<serde_json::Value> {
        self.structured.take()
    }

    /// Keep the top-level call's result as data (made only when asked for).
    fn set_structured(&mut self, data: impl FnOnce() -> serde_json::Value) {
        if self.ctx.depth == 1 && self.store.config.server.structured_output {
            self.structured = Some(data());
        }
    }

    /// An app and the screen it shows now, for [`ResultNote`].
    fn screen_key(&self, pid: u32) -> (u32, u32) {
        (pid, self.states.get(&pid).map_or(0, |s| s.screen))
    }

    fn note_result(&mut self) {
        let id = self.result_id;
        let note = std::mem::take(&mut self.note);
        let r = &mut self.results;
        let mut sup: Vec<u64> = Vec::new();
        let mut images: Vec<u64> = Vec::new();
        for key in note.looks_full {
            sup.extend(r.looks.insert(key, vec![id]).unwrap_or_default());
        }
        for key in note.looks_diff {
            r.looks.entry(key).or_default().push(id);
        }
        for key in note.pictures_whole {
            images.extend(r.pictures.insert(key, vec![id]).unwrap_or_default());
        }
        for key in note.pictures_part {
            r.pictures.entry(key).or_default().push(id);
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
        sup.retain(|x| *x != id && !r.kept.contains(x));
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

    /// Append a bounded JSONL record of the call, if auditing is enabled. Only
    /// metadata is written — never arguments, tree text or screenshots.
    fn audit(&self, tool: &str, app: Option<&str>, out: &ToolOutput) {
        let audit = &self.store.config.audit;
        if !audit.enabled {
            return;
        }
        let path = audit
            .path
            .as_deref()
            .and_then(crate::config::settings_path)
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
}

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

fn show_rect(r: Rect) -> String {
    format!("{:.0}x{:.0} at {:.0},{:.0}", r.width, r.height, r.x, r.y)
}

/// How an app's tree has shown the changes actions made: many apps
/// (browsers, Electron apps) show one a little after making it, so reads
/// that still show the state from before are not believed at once.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Promptness {
    /// Changes shown by the first read after the action.
    prompt: u32,
    /// A change was first shown by a later read.
    late: bool,
}

impl Promptness {
    /// Changes shown at once, this many times and never late, make an
    /// app prompt.
    const PROMPT_AFTER: u32 = 3;

    fn saw_change(&mut self, at_once: bool) {
        if at_once {
            self.prompt = self.prompt.saturating_add(1);
        } else {
            self.late = true;
        }
    }

    /// How long reads that still show the state from before are waited
    /// on: 500 ms, or 200 for an app that has always shown its changes at
    /// once (most native apps), so an action that changed nothing (a click
    /// on a canvas) is done sooner.
    fn grace(self) -> Duration {
        if !self.late && self.prompt >= Self::PROMPT_AFTER {
            Duration::from_millis(200)
        } else {
            Duration::from_millis(500)
        }
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
mod tests;
