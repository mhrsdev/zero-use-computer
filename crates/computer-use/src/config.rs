//! User configuration (`~/.computer-use/config.toml`).
//!
//! There is no access control here: which apps and actions the agent may
//! use is left to the agent's security skill (`skills/computer-use-security`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const HOME_ENV: &str = "COMPUTER_USE_HOME";

/// Several agents on one desktop ([hub]): every server (whichever client
/// started it, subagents included) joins one hub that draws a cursor for
/// each, has one stop key for all, shares the screen out and gives the
/// keyboard and mouse to one at a time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HubConfig {
    /// Join the hub (the first server to start runs it). Off: an overlay
    /// of this server's own, as before v3.9.
    pub enabled: bool,
    /// The port of this computer the hub listens on (only this computer
    /// can reach it, and only with the token in the server's folder).
    pub port: u16,
    /// With two agents or more, move the window an agent works with into
    /// its part of the screen.
    pub arrange: bool,
    /// Let the agents send each other short messages (`agents` send). Off
    /// by default: the user turns it on.
    pub chat: bool,
    /// Longest wait for a turn at the keyboard and mouse, in seconds.
    pub turn_wait_secs: u64,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 47_381,
            arrange: true,
            chat: false,
            turn_wait_secs: 60,
        }
    }
}

/// The settings panel ([panel]): a page this program serves on this
/// computer only, opened by the settings key or `computer-use-mcp settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelConfig {
    /// The port of this computer the panel listens on while it is open. If
    /// it is taken by another program, a free one is used.
    pub port: u16,
    /// The panel closes after this many minutes without a request.
    pub idle_minutes: u64,
    /// "system" (follow the computer), "light" or "dark".
    pub theme: String,
    /// The accent colour the panel's palette is made from (#RRGGBB).
    pub accent: String,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            port: 47_382,
            idle_minutes: 15,
            theme: "system".into(),
            accent: "#1A73E8".into(),
        }
    }
}

/// Updates ([update]): a while after the server starts it asks GitHub for
/// the latest release; a newer one is downloaded, checked and set aside,
/// never put in place while the agent may be working. It goes in when the
/// server starts again after the computer has restarted (see `install`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Look for updates and download them.
    pub enabled: bool,
    /// Minutes after the server starts before the first look.
    pub check_after_mins: u64,
    /// Hours between looks while the server runs (every server on the
    /// computer shares them).
    pub check_every_hours: u64,
    /// When a downloaded update goes in: "restart" (the first start after
    /// the computer restarts), "start" (the next time the server starts),
    /// or "manual" (only `computer-use-mcp update --install`).
    pub install: UpdateInstall,
    /// The GitHub repository releases come from (owner/name).
    pub repo: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_after_mins: 5,
            check_every_hours: 12,
            install: UpdateInstall::Restart,
            repo: "mhrsdev/zero-use-computer".into(),
        }
    }
}

/// When a downloaded update goes in ([`UpdateConfig::install`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateInstall {
    #[default]
    Restart,
    Start,
    Manual,
}

/// Optional JSONL audit log of every tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuditConfig {
    pub enabled: bool,
    /// File to append to. Defaults to `<home>/audit.log` when enabled.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

/// When get_app_state attaches a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AttachMode {
    /// Every get_app_state (Codex behaviour; most tokens).
    Always,
    /// Only when it adds information: the first view of a window, a large
    /// change, or a sparse tree (canvas / custom-drawn UI). Default.
    #[default]
    Auto,
    /// Never (use the `screenshot` tool when an image is needed).
    Never,
}

/// PNG compression effort: faster encoding vs. smaller files. File size does
/// not change the model's token cost (that depends on pixel dimensions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PngCompression {
    #[default]
    Fast,
    Default,
    Best,
}

/// Downscaling quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResizeFilter {
    /// Area averaging; fastest, good for UI text. Default.
    #[default]
    Fast,
    /// Bilinear (triangle) filter.
    Smooth,
    /// Lanczos3; sharpest, slowest.
    Sharp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotConfig {
    /// Allow screenshots at all (get_app_state images and the screenshot tool).
    pub enabled: bool,
    /// When get_app_state attaches an image: always | auto | never.
    pub attach: AttachMode,
    /// In `auto` mode, attach when the tree has fewer interactive elements
    /// than this (the UI is probably custom-drawn and needs pixels).
    pub auto_sparse_threshold: usize,
    /// Longest edge of the image sent to the model, in pixels. Image token cost
    /// grows with width x height, so this is the main image-token knob.
    pub max_dimension: u32,
    /// Opt-in: longest edge of a screenshot attached on its own (`attach =
    /// "auto"`) to a window whose tree already says what is there, as a
    /// smaller overview for fewer image tokens. It trades detail for tokens,
    /// so it is off (0 = always `max_dimension`). A screenshot asked for
    /// (`screenshot=true`), or of a window with little in its tree, always
    /// uses `max_dimension`.
    pub overview_max_dimension: u32,
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    pub png_compression: PngCompression,
    pub resize_filter: ResizeFilter,
    /// What a screenshot of a screen the model has already seen covers:
    /// `auto` = only the part that changed, when that is a small part of the
    /// window; `full` = always the whole window.
    pub scope: ShotScope,
    /// In `auto`, a change bigger than this share of the window area sends
    /// the whole window.
    pub region_max_ratio: f64,
    /// Margin around the changed part (pixels of the capture).
    pub region_padding: u32,
    /// The part sent is at least this many pixels on each side, for context.
    pub region_min_size: u32,
    /// Attach automatic screenshots by how the model works in an app: once
    /// it has looked a few times without ever using pixels there (no x/y,
    /// no screenshot asked for), well-described windows come without one
    /// (`screenshot=true` still gets one).
    pub adaptive: bool,
    /// Leave out an automatic screenshot when all that changed on screen
    /// is what the tree reports changed (a number, a line of text read off
    /// the screen): the text already says it. One is still sent after an
    /// action at x/y, an `expect` not met, and after 3 left out in a row.
    pub smart: bool,
    /// Keep a count of how often each app needed pixels (little in its
    /// tree, text read off the screen, screenshots sent and left out) in
    /// `apps.json` in the server's folder; `doctor` shows it.
    pub record_apps: bool,
    /// `locate` sends the window with the places it found numbered.
    pub locate_picture: bool,
    /// When no screenshot is attached, a small strip of the buttons that
    /// have no name, each numbered with its element_index (experimental).
    pub icon_sprite: bool,
}

/// How much of the window a follow-up screenshot covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ShotScope {
    /// Only the changed part when it's small. Default.
    #[default]
    Auto,
    /// Always the whole window.
    Full,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            attach: AttachMode::Auto,
            auto_sparse_threshold: 2,
            max_dimension: 1280,
            overview_max_dimension: 0,
            format: ImageFormat::Png,
            jpeg_quality: 85,
            png_compression: PngCompression::Fast,
            resize_filter: ResizeFilter::Fast,
            scope: ShotScope::Auto,
            region_max_ratio: 0.5,
            region_padding: 24,
            region_min_size: 200,
            adaptive: true,
            smart: true,
            record_apps: true,
            locate_picture: false,
            icon_sprite: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TreeConfig {
    /// Maximum elements rendered in one get_app_state.
    pub max_nodes: usize,
    /// Maximum elements the backend walks before pruning.
    pub max_walk: usize,
    pub max_depth: usize,
    /// Longest text/value shown per element before truncation.
    pub max_text_len: usize,
    /// Spaces of indentation per tree level.
    pub indent: usize,
    /// Show each element's secondary actions (actions=[...]).
    pub show_actions: bool,
    /// Show state flags such as (focused, disabled, checked).
    pub show_states: bool,
    /// Return a diff instead of the full tree when little changed.
    pub diff: bool,
    /// Fall back to the full tree when the diff touches more than this
    /// fraction of the elements.
    pub diff_full_ratio: f64,
    /// After a mutating action, re-snapshot and append what changed.
    pub report_changes: bool,
    /// Longest change report (lines) appended to an action result.
    pub report_changes_max_lines: usize,
    /// Token budget of one tree or diff (estimated; 0 = no limit).
    pub max_tokens: usize,
    /// How a tree over `max_tokens` is shortened (see [`Summarize`]).
    pub summarize: Summarize,
    /// Items kept at the start of a folded list (and two at its end).
    /// Higher keeps more of every long list.
    pub fold_keep: usize,
    /// Explanations (what a diff or a partial screenshot means) in full the
    /// first time and in a few words after that. false = in full every time.
    pub brief_repeats: bool,
    /// Say each thing once, losing nothing: look-alike siblings as records
    /// (roles once, one line a record), a table's cells a row a line,
    /// flags and actions the role implies left out, many removed elements
    /// as ranges of indices, added ones under their parent, a short header
    /// when the window is as before, no echo of a value just set.
    pub compact: bool,
    /// What an action's result says of what changed (see [`Report`]).
    pub report: Report,
    /// Elements that change on their own (clocks, progress, spinners):
    /// after a few reports in a row, summed up in one line instead of
    /// listed every time.
    pub quiet_volatile: bool,
}

/// How much of what changed an action's result reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Report {
    /// Every change, up to `report_changes_max_lines`.
    Full,
    /// Changes in and around what the action acted on, new windows and
    /// added elements; how many others there are. Default.
    #[default]
    Relevant,
    /// How many changes there are, and a new screen or window.
    Brief,
}

/// How much a tree over the token budget is shortened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Summarize {
    /// Never: every tree is sent whole, whatever its size.
    Off,
    /// Only long lists (list items, rows, menu items…) are folded to their
    /// first and last items; nothing is cut.
    Light,
    /// Long lists first, then any long run of look-alike elements, and the
    /// rest cut if it still doesn't fit. Default.
    #[default]
    Normal,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            max_nodes: 1200,
            max_walk: 6000,
            max_depth: 64,
            max_text_len: 160,
            indent: 1,
            show_actions: true,
            show_states: true,
            diff: true,
            diff_full_ratio: 0.33,
            report_changes: true,
            report_changes_max_lines: 25,
            max_tokens: 10_000,
            summarize: Summarize::Normal,
            fold_keep: 5,
            brief_repeats: true,
            compact: true,
            report: Report::Relevant,
            quiet_volatile: true,
        }
    }
}

/// How tool definitions are presented to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DescriptionStyle {
    /// Detailed descriptions for every tool and parameter.
    Full,
    /// One-line tool descriptions, no per-parameter prose.
    Compact,
    /// Compact, and lighter schemas still: the keys of nested objects (a
    /// design's layers, a drawing's strokes) listed by name instead of
    /// typed one by one, no `window` (still accepted) and no defaults;
    /// `decide` only once a decision model is set up. Default: the tool
    /// list is sent with every model request.
    #[default]
    Lean,
}

/// Which tools the model sees at first ([tools] manager).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolManager {
    /// Every tool, always.
    Off,
    /// The base tools and `find_tools`; the others are found by name or
    /// category and run through `use_tool`. The tool list never changes,
    /// so a client's prompt cache keeps working, with any client. Default.
    #[default]
    Dispatch,
    /// The base tools and `find_tools`; a category it finds is added to the
    /// tool list (the client is told the list changed) and stays.
    ListChanged,
}

/// A ready-made set of tools ([tools] preset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolPreset {
    /// Every tool the other settings allow. Default.
    #[default]
    Full,
    /// A small fixed set for smaller models: look, find, wait, click, set,
    /// type, keys, scroll, list and launch apps.
    Small,
}

/// The tools of [`ToolPreset::Small`].
pub const SMALL_TOOLS: &[&str] = &[
    "list_apps",
    "launch_app",
    "get_app_state",
    "find_element",
    "wait_for",
    "click",
    "set_value",
    "type_text",
    "press_key",
    "scroll",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Tools to hide from the model entirely (e.g. ["drag", "batch"]).
    pub disabled: Vec<String>,
    /// If non-empty, ONLY these tools are exposed.
    pub enabled: Vec<String>,
    pub descriptions: DescriptionStyle,
    /// Which tools the model sees at first (see [`ToolManager`]).
    pub manager: ToolManager,
    /// A ready-made set of tools (see [`ToolPreset`]); `enabled` wins.
    pub preset: ToolPreset,
    /// A call without `app` acts on the app of the last call that named
    /// one, and `app` isn't required in the tools' schemas.
    pub default_app: bool,
    /// launch_app answers with the app's first state (its tree, as
    /// get_app_state would), saving the call that always follows.
    pub launch_look: bool,
    /// When `design` lists the steps to paint a design (see
    /// [`DesignSteps`]).
    pub design_steps: DesignSteps,
}

/// When `design` lists the steps to paint a design ([tools] design_steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DesignSteps {
    /// With every answer.
    Always,
    /// When asked (show steps=true); otherwise only how many there are.
    /// Default.
    #[default]
    Asked,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            disabled: Vec::new(),
            enabled: Vec::new(),
            descriptions: DescriptionStyle::default(),
            manager: ToolManager::default(),
            preset: ToolPreset::default(),
            default_app: false,
            launch_look: true,
            design_steps: DesignSteps::Asked,
        }
    }
}

impl ToolsConfig {
    pub fn is_enabled(&self, name: &str) -> bool {
        if self.disabled.iter().any(|d| d == name) {
            return false;
        }
        if !self.enabled.is_empty() {
            return self.enabled.iter().any(|e| e == name);
        }
        self.preset == ToolPreset::Full || SMALL_TOOLS.contains(&name)
    }
}

/// What the overlay border goes around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BorderTarget {
    /// A glow along the edges of the whole screen.
    Screen,
    /// A glow around the window being worked on.
    Window,
}

/// The on-screen indicator shown while the agent uses the computer: its own
/// cursor, a border around the window it works on, and a status label (see
/// `overlay/`). It runs in a separate helper process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    pub enabled: bool,
    pub show_border: bool,
    pub show_cursor: bool,
    pub show_label: bool,
    /// A ripple where the agent clicks.
    pub click_effect: bool,
    /// Where the border glows: around the whole `screen` or the `window`
    /// being worked on.
    pub border_target: BorderTarget,
    /// Thickness of the border's bright core line (px at 100% scale).
    pub border_width: u32,
    /// How far the border's glow fades out (px at 100% scale; 0 = no glow).
    pub glow_size: u32,
    /// Size multiplier; 0 = follow the display's scaling.
    pub scale: f64,
    /// Label texts per state; `{action}` is replaced by what is pending.
    pub label_working: String,
    pub label_thinking: String,
    pub label_error: String,
    pub label_done: String,
    /// While the agent waits for the user to stop using the mouse/keyboard.
    pub label_paused: String,
    /// After the emergency stop key; `{hotkey}` is replaced by the key.
    pub label_stopped: String,
    /// Colours (#RRGGBB or #RRGGBBAA) per state.
    pub color_thinking: String,
    pub color_working: String,
    pub color_error: String,
    pub color_done: String,
    pub color_paused: String,
    pub color_stopped: String,
    /// The agent cursor's own colour (its ring follows the state).
    pub cursor_color: String,
    /// Name tag shown beside the agent cursor ("" = none).
    pub cursor_tag: String,
    /// The agent cursor's look: "random" (one of the pointers, another for
    /// each agent working at once), "classic" (the plain arrow), or one:
    /// crystal, paper, jelly, ice, metal, orbit.
    pub cursor_style: String,
    /// A pointer for an agent by its name: the MCP client's ("claude-code",
    /// "codex") or its `cursor_tag`, e.g. { codex = "metal" }. Matched
    /// without regard to case, before `cursor_style`.
    pub agent_cursors: std::collections::BTreeMap<String, String>,
    /// The pointer leans into its moves and leaves a trail of its own
    /// material, breathes while it waits, draws a line where it drags and
    /// shows the way it scrolls.
    pub cursor_motion: bool,
    /// The way the pointer glides to where it acts (as `mouse_path`):
    /// "mixed", "hand", "sine", "arc", "spring" or "spiral".
    pub cursor_path: String,
    /// Keys the agent presses show as keycaps by its pointer, and what it
    /// types runs out beside it.
    pub show_keys: bool,
    /// No new action for this long after the last one: done (green), then hidden.
    pub done_after_ms: u64,
    /// How long "done" stays on screen before everything disappears.
    pub done_linger_ms: u64,
    /// How long an error stays red before returning to "thinking".
    pub error_hold_ms: u64,
    /// Duration of the cursor's glide to a new point. An action waits for
    /// the cursor to get there first (at most this long, plus a little).
    pub move_ms: u64,
    /// How long the overlay takes to fade in when it appears.
    pub fade_in_ms: u64,
    /// How long it takes to fade out when the work is done (never abrupt).
    pub fade_out_ms: u64,
    /// How long a change of state colour takes.
    pub transition_ms: u64,
    /// Where the overlay can't be excluded from screenshots (X11): wait this
    /// long after hiding it before capturing.
    pub capture_hide_ms: u64,
    /// Font file for the label ("" = a system font).
    pub font: String,
    /// Overlay helper program for embedders ("" = the host decides; the MCP
    /// server uses itself).
    pub command: String,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show_border: true,
            show_cursor: true,
            show_label: true,
            click_effect: true,
            border_target: BorderTarget::Screen,
            border_width: 3,
            glow_size: 36,
            scale: 0.0,
            label_working: "Zero is using the computer".into(),
            label_thinking: "Zero is thinking…".into(),
            label_error: "Zero hit an error".into(),
            label_done: "Zero is done".into(),
            label_paused: "Paused while you use the computer".into(),
            label_stopped: "Zero stopped. Press {hotkey} to let it continue".into(),
            color_thinking: "#D4A017".into(),
            color_working: "#1E88E5".into(),
            color_error: "#E53935".into(),
            color_done: "#2E7D32".into(),
            color_paused: "#78909C".into(),
            color_stopped: "#FF6D00".into(),
            cursor_color: "#9C27B0".into(),
            cursor_tag: "Zero".into(),
            cursor_style: "random".into(),
            agent_cursors: Default::default(),
            cursor_motion: true,
            cursor_path: "mixed".into(),
            show_keys: true,
            done_after_ms: 20_000,
            done_linger_ms: 1_500,
            error_hold_ms: 2_500,
            move_ms: 220,
            fade_in_ms: 250,
            fade_out_ms: 1200,
            transition_ms: 300,
            capture_hide_ms: 40,
            font: String::new(),
            command: String::new(),
        }
    }
}

impl OverlayConfig {
    /// These settings for an agent called any of `names` (its MCP client,
    /// its tag): the pointer `agent_cursors` gives it, if any.
    pub fn for_agent(&self, names: &[&str]) -> OverlayConfig {
        let mut cfg = self.clone();
        let picked = names.iter().find_map(|n| {
            let n = n.trim();
            (!n.is_empty())
                .then(|| {
                    self.agent_cursors
                        .iter()
                        .find(|(k, _)| k.trim().eq_ignore_ascii_case(n))
                })
                .flatten()
        });
        if let Some((_, style)) = picked {
            cfg.cursor_style = style.clone();
        }
        cfg
    }
}

/// Reading desktop notifications. Off by default: notifications carry other
/// apps' content (messages, codes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    /// Offer the get_notifications tool (and, on Linux, listen for them).
    pub enabled: bool,
    /// Only these apps' notifications (names, case-insensitive; [] = any app).
    pub apps: Vec<String>,
    /// Mask what looks like a one-time or verification code.
    pub mask_codes: bool,
    /// Notifications kept (Linux listens from the start of the server).
    pub keep: usize,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            apps: Vec::new(),
            mask_codes: true,
            keep: 50,
        }
    }
}

/// Which files scripts may read and write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScriptFiles {
    /// No files at all.
    None,
    /// Only the scripts' own folder (`<home>/scripts/files`).
    Workspace,
    /// Read any file; write only in the scripts' own folder.
    #[default]
    Read,
    /// Read and write any file.
    All,
}

/// The `script` tool: programs the agent writes and runs inside the server,
/// and saved scripts that become tools of their own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptConfig {
    /// Saved scripts are listed as tools of their own (the tool list changes
    /// when one is saved or deleted).
    pub saved_as_tools: bool,
    /// Files scripts may use.
    pub files: ScriptFiles,
    /// fetch() and download(): web pages and data over http(s).
    pub web: bool,
    /// Longest run of one script, in seconds (its tool calls included).
    pub max_seconds: u64,
    /// Where saved scripts live (default `<home>/scripts`).
    pub dir: Option<PathBuf>,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            saved_as_tools: true,
            files: ScriptFiles::Read,
            web: true,
            max_seconds: 300,
            dir: None,
        }
    }
}

impl ScriptConfig {
    /// The folder of saved scripts.
    pub fn library(&self) -> PathBuf {
        self.dir
            .as_deref()
            .and_then(settings_path)
            .unwrap_or_else(|| home_dir().join("scripts"))
    }
}

/// When text is read off the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OcrMode {
    /// When a window's accessibility tree has almost nothing to act on
    /// (fewer interactive elements than `sparse_threshold`). Default.
    #[default]
    Auto,
    /// For every window read (slow).
    Always,
    /// Never (the agent can still ask with get_app_state ocr=true).
    Off,
}

/// Which OCR engine reads the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngineChoice {
    /// The OS's own (Windows.Media.Ocr, Vision), else Tesseract. Default.
    #[default]
    Auto,
    Native,
    Tesseract,
}

/// Reading text off the screen for apps with little accessibility
/// information; the lines become clickable `ocr text` elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    pub mode: OcrMode,
    pub engine: OcrEngineChoice,
    /// In `auto`, windows with fewer interactive elements than this.
    pub sparse_threshold: usize,
    /// Languages to read ("en", "de", …); [] = the user's languages
    /// (Tesseract: English).
    pub languages: Vec<String>,
    /// Lines recognised with less confidence (0–1) are left out.
    pub min_confidence: f64,
    /// Most lines added per window.
    pub max_lines: usize,
    /// The Tesseract program.
    pub tesseract_path: String,
    /// Find the areas of a window the tree says nothing about (a canvas
    /// next to a full toolbar), read their text, and check their pixels
    /// on every look, however many elements the rest of the window has.
    pub blind_regions: bool,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            mode: OcrMode::Auto,
            engine: OcrEngineChoice::Auto,
            sparse_threshold: 3,
            languages: Vec::new(),
            min_confidence: 0.4,
            max_lines: 150,
            tesseract_path: "tesseract".into(),
            blind_regions: true,
        }
    }
}

/// Checking that actions worked, and retrying another way when they
/// clearly didn't.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VerifyConfig {
    /// Check the result of each action (a value really set, text really
    /// typed, something changed) and tell the agent when it didn't work.
    pub enabled: bool,
    /// When an action clearly failed (the accessibility call errored, a
    /// value didn't take, typed text didn't land), try once more another way
    /// (mouse click, focus + select + type…).
    pub retry: bool,
    /// Also retry a press after which nothing visible changed. Off by
    /// default: some actions (pay, send…) show their effect late, and a
    /// retry could repeat them.
    pub retry_on_no_change: bool,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retry: true,
            retry_on_no_change: false,
        }
    }
}

/// The user's controls over a running agent: an emergency stop key and
/// pausing while the user works.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    /// Global key combination that stops the agent at once (every tool call
    /// is refused); pressing it again lets it continue. "" = no stop key.
    pub stop_hotkey: String,
    /// Before each action, wait while the user is using the mouse or
    /// keyboard, and continue once they have stopped. Only *whether* there
    /// was input is checked (the system's idle time), never what it was.
    pub pause_on_user_input: bool,
    /// How long the user must leave the mouse and keyboard alone before the
    /// agent continues (ms).
    pub resume_after_idle_ms: u64,
    /// Give up (the action fails with a message to the agent) after waiting
    /// this long for the user (seconds).
    pub max_pause_secs: u64,
    /// Global key combination that opens the settings panel in the browser.
    /// "" = none.
    pub settings_hotkey: String,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            stop_hotkey: "ctrl+alt+escape".into(),
            pause_on_user_input: true,
            resume_after_idle_ms: 1500,
            max_pause_secs: 120,
            settings_hotkey: "ctrl+alt+j".into(),
        }
    }
}

/// A decision model: a fast model that answers typed questions (yes/no,
/// one of some options, a score on a scale) about a state, for the `decide`
/// tool, `wait_for`'s `until` and scripts. TypeSafe's Jev (its System One
/// API, or a server speaking it), or any OpenAI-compatible chat model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DecisionConfig {
    /// "jev" (the System One API: TypeSafe, or a compatible server),
    /// "openai" (any OpenAI-compatible chat completions API), or "" = none.
    pub provider: String,
    /// The API's address; "" = the provider's own (https://api.typesafe.ai,
    /// https://api.openai.com/v1).
    pub base_url: String,
    /// "" = jev-latest (jev); an OpenAI-compatible API needs one.
    pub model: String,
    /// The API key ("" = none, or `api_key_env`). Kept in this file, which
    /// is then readable by its owner only.
    pub api_key: String,
    /// Or the name of an environment variable holding the key.
    pub api_key_env: String,
    /// Longest wait for one answer (ms).
    pub timeout_ms: u64,
    /// Longest state sent (characters); a longer one keeps its start and end.
    pub max_state_chars: usize,
    /// Requests at once when several items are judged.
    pub parallel: usize,
    /// Let the server ask the model on its own where that saves the agent
    /// a turn or a read: whether an `expect`ed text shows in other words,
    /// which parts of a window `about` means, which tool a `find_tools`
    /// query means. Without a model (or with this off) the server judges
    /// those itself, as before.
    pub auto: bool,
    /// Longest wait for a question the server asks on its own (ms); then
    /// it judges by itself.
    pub auto_timeout_ms: u64,
    /// How long an answer is kept for the same question about the same
    /// state (seconds; 0 = never kept).
    pub cache_seconds: u64,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            base_url: String::new(),
            model: String::new(),
            api_key: String::new(),
            api_key_env: String::new(),
            timeout_ms: 15_000,
            max_state_chars: 20_000,
            parallel: 8,
            auto: true,
            auto_timeout_ms: 3_000,
            cache_seconds: 300,
        }
    }
}

/// How redacted areas look in screenshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RedactStyle {
    /// A solid grey box. Default; nothing of the original survives.
    #[default]
    Fill,
    /// Coarse pixelation (keeps the layout recognisable).
    Pixelate,
}

/// Private data kept away from the model: masked in element text and
/// blacked out of screenshots before anything is sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    /// Password fields.
    pub redact_passwords: bool,
    /// Payment card numbers (13–19 digits passing the Luhn check); element
    /// text keeps only the last four digits.
    pub redact_card_numbers: bool,
    /// Fields whose label contains one of these (case-insensitive) have
    /// their contents masked too.
    pub redact_labels: Vec<String>,
    pub style: RedactStyle,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            redact_passwords: true,
            redact_card_numbers: true,
            redact_labels: [
                "cvv",
                "cvc",
                "security code",
                "card number",
                "one-time code",
                "verification code",
            ]
            .map(String::from)
            .to_vec(),
            style: RedactStyle::Fill,
        }
    }
}

/// Screen memory and caches (see `screens.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Recognise screens the model has already seen: restore their element
    /// indices and report only what differs, with no new screenshot.
    pub enabled: bool,
    /// Screens remembered across all apps (least recently used go first).
    pub max_screens: usize,
    /// Memory budget for remembered screens, in KiB.
    pub max_memory_kb: usize,
    /// How alike (0–1, share of common elements) a view must be to a
    /// remembered screen to count as the same screen.
    pub match_threshold: f64,
    /// Don't re-send a screenshot whose pixels haven't changed since the
    /// model last received one of that screen (an explicit screenshot=true
    /// always sends).
    pub dedupe_screenshots: bool,
    /// Cells across the longer side of the pixel fingerprint.
    pub pixel_grid: u32,
    /// Per-cell brightness drift (0–255) still counted as unchanged.
    pub pixel_tolerance: u8,
    /// Reuse a window list / tree snapshot this recent (ms) when no action
    /// ran in between. 0 turns it off.
    pub snapshot_ttl_ms: u64,
    /// After this many (estimated) tokens of results since an app's tree
    /// was last sent whole, the next get_app_state sends it whole again,
    /// with a picture: in a long conversation the model (or a host that
    /// trims its context) may have lost what diffs refer to. 0 = never.
    pub rebase_after_tokens: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_screens: 32,
            max_memory_kb: 8192,
            match_threshold: 0.8,
            dedupe_screenshots: true,
            pixel_grid: 64,
            pixel_tolerance: 2,
            snapshot_ttl_ms: 200,
            rebase_after_tokens: 0,
        }
    }
}

/// How the engine waits for the UI after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SettleMode {
    /// Just pause `settle_ms`.
    Fixed,
    /// Pause `settle_ms`, then re-read the app until it stops changing (two
    /// reads in a row agree), at most `settle_max_ms`. Default.
    #[default]
    Adaptive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimingConfig {
    /// Pause after each action before looking at the UI again.
    pub settle_ms: u64,
    /// `adaptive`: wait until the UI stops changing; `fixed`: only settle_ms.
    pub settle: SettleMode,
    /// Longest adaptive wait after an action.
    pub settle_max_ms: u64,
    /// Interval between checks while waiting.
    pub settle_poll_ms: u64,
    /// Pause between keys of a press_key sequence.
    pub key_delay_ms: u64,
    /// How long the running-app list is reused between calls.
    pub app_cache_ms: u64,
    /// Default wait_for timeout and poll interval.
    pub wait_timeout_ms: u64,
    pub wait_poll_ms: u64,
    /// How long an action with `expect` waits for what it expects to show.
    pub expect_wait_ms: u64,
    /// After an action, reads that still show the state from before are
    /// believed ("nothing changed") after 500 ms; after 200 for an app
    /// that has shown every change at once so far (3 or more, none late).
    /// `false`: 500 ms for every app.
    pub adaptive_grace: bool,
}

impl Default for TimingConfig {
    fn default() -> Self {
        Self {
            settle_ms: 40,
            settle: SettleMode::Adaptive,
            settle_max_ms: 2000,
            settle_poll_ms: 50,
            key_delay_ms: 10,
            app_cache_ms: 1500,
            wait_timeout_ms: 10_000,
            wait_poll_ms: 400,
            expect_wait_ms: 2000,
            adaptive_grace: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinuxConfig {
    /// Elements whose AT-SPI queries are sent concurrently in one batch.
    pub batch_size: usize,
    /// Longest text read from a text element.
    pub text_max_chars: usize,
}

impl Default for LinuxConfig {
    fn default() -> Self {
        Self {
            batch_size: 48,
            text_max_chars: 2000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MacosConfig {
    /// Fetch all of an element's attributes in one AX call.
    pub batch_attributes: bool,
    /// Seconds an AX call may block on an unresponsive app.
    pub messaging_timeout_secs: f32,
}

impl Default for MacosConfig {
    fn default() -> Self {
        Self {
            batch_attributes: true,
            messaging_timeout_secs: 2.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsConfig {
    /// Walk the tree with a UI Automation CacheRequest (one cross-process
    /// call for the whole window instead of several per element).
    pub use_cache_request: bool,
}

impl Default for WindowsConfig {
    fn default() -> Self {
        Self {
            use_cache_request: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Log level: error, warn, info, debug, trace.
    pub log: String,
    /// Serve MCP over HTTP on this address instead of stdio (needs the `http`
    /// build feature). Empty = stdio.
    pub http_addr: String,
    /// Bearer token required on the HTTP endpoint (the server refuses to
    /// start over HTTP without one).
    pub http_token: String,
    /// The server's MCP instructions: "full", "short" (the loop and the
    /// safety rules in a few lines, for clients that load the skills) or
    /// "off".
    pub instructions: Instructions,
    /// Add `_meta` to tool results for hosts that trim their context: a
    /// number for each result and the earlier ones it repeats whole
    /// (`zero-use-computer/supersedes`) or whose pictures it replaces
    /// (`zero-use-computer/supersedes-images`).
    pub result_meta: bool,
    /// Also return the results of list_apps, find_element and
    /// get_clipboard as data (MCP `structuredContent`, with an
    /// `outputSchema` in the tool list), for clients that use it. Off: the
    /// text is what models read, and some clients would send both.
    pub structured_output: bool,
}

/// How much the MCP `instructions` say ([server] instructions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Instructions {
    #[default]
    Full,
    Short,
    Off,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            log: "warn".into(),
            http_addr: String::new(),
            http_token: String::new(),
            instructions: Instructions::Full,
            result_meta: true,
            structured_output: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub tools: ToolsConfig,
    pub screenshot: ScreenshotConfig,
    pub tree: TreeConfig,
    pub timing: TimingConfig,
    pub cache: CacheConfig,
    pub overlay: OverlayConfig,
    pub control: ControlConfig,
    pub privacy: PrivacyConfig,
    pub verify: VerifyConfig,
    pub ocr: OcrConfig,
    pub notifications: NotificationsConfig,
    pub script: ScriptConfig,
    pub decision: DecisionConfig,
    pub audit: AuditConfig,
    pub server: ServerConfig,
    pub hub: HubConfig,
    pub panel: PanelConfig,
    pub update: UpdateConfig,
    pub linux: LinuxConfig,
    pub macos: MacosConfig,
    pub windows: WindowsConfig,
    /// Expose the clipboard tools (get_clipboard / set_clipboard).
    pub clipboard: bool,
    /// Never attach screenshots to any tool result (tree-only operation).
    pub text_only: bool,
    /// When an action opens a new window (a dialog, a menu), switch to it
    /// for the change report and the next get_app_state without a `window`.
    pub follow_new_windows: bool,
    /// Put the real mouse pointer back where it was after a synthesized
    /// click, scroll or drag (Linux, Windows; macOS never moves it).
    pub restore_pointer: bool,
    /// Move the mouse the way a hand does when an action uses it: along a
    /// gentle curve, speeding up and slowing down, rather than jumping or
    /// going in a straight line (some apps notice). Off: it jumps there.
    pub natural_mouse: bool,
    /// The way the mouse goes (with `natural_mouse`): "mixed" (one at random
    /// for each move), "hand" (a hand's curve), "sine" (a wave), "arc" (a
    /// circle's arc), "spring" (past the target and back) or "spiral" (in
    /// to the target). Drags go straight whatever this says.
    pub mouse_path: String,
    /// Re-read this file when it changes, without restarting the server.
    pub hot_reload: bool,
    /// Seconds launch_app waits for the app to show a window.
    pub launch_timeout_secs: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tools: ToolsConfig::default(),
            screenshot: ScreenshotConfig::default(),
            tree: TreeConfig::default(),
            timing: TimingConfig::default(),
            cache: CacheConfig::default(),
            overlay: OverlayConfig::default(),
            control: ControlConfig::default(),
            privacy: PrivacyConfig::default(),
            verify: VerifyConfig::default(),
            ocr: OcrConfig::default(),
            notifications: NotificationsConfig::default(),
            script: ScriptConfig::default(),
            decision: DecisionConfig::default(),
            audit: AuditConfig::default(),
            server: ServerConfig::default(),
            hub: HubConfig::default(),
            panel: PanelConfig::default(),
            update: UpdateConfig::default(),
            linux: LinuxConfig::default(),
            macos: MacosConfig::default(),
            windows: WindowsConfig::default(),
            clipboard: true,
            text_only: false,
            follow_new_windows: true,
            restore_pointer: true,
            natural_mouse: true,
            mouse_path: "mixed".into(),
            hot_reload: true,
            launch_timeout_secs: 15.0,
        }
    }
}

impl Config {
    /// Range checks the types can't express.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.hub.port == 0 {
            return Err("hub.port must be a port number (1 to 65535)".into());
        }
        if !(1..=600).contains(&self.hub.turn_wait_secs) {
            return Err(format!(
                "hub.turn_wait_secs must be between 1 and 600 (got {})",
                self.hub.turn_wait_secs
            ));
        }
        let p = &self.panel;
        if p.port == 0 {
            return Err("panel.port must be a port number (1 to 65535)".into());
        }
        if !(1..=240).contains(&p.idle_minutes) {
            return Err(format!(
                "panel.idle_minutes must be between 1 and 240 (got {})",
                p.idle_minutes
            ));
        }
        if !matches!(p.theme.trim(), "system" | "light" | "dark") {
            return Err(format!(
                "panel.theme must be system, light or dark (got \"{}\")",
                p.theme
            ));
        }
        let accent = p.accent.trim();
        if !(accent.len() == 7
            && accent.starts_with('#')
            && accent[1..].chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(format!(
                "panel.accent must be a colour like \"#1A73E8\" (got \"{accent}\")"
            ));
        }
        let c = &self.cache;
        if !(0.0..=1.0).contains(&c.match_threshold) {
            return Err(format!(
                "cache.match_threshold must be between 0 and 1 (got {})",
                c.match_threshold
            ));
        }
        let o = &self.overlay;
        for (key, value) in [
            ("overlay.color_thinking", &o.color_thinking),
            ("overlay.color_working", &o.color_working),
            ("overlay.color_error", &o.color_error),
            ("overlay.color_done", &o.color_done),
            ("overlay.color_paused", &o.color_paused),
            ("overlay.color_stopped", &o.color_stopped),
            ("overlay.cursor_color", &o.cursor_color),
        ] {
            if crate::overlay::draw::parse_color(value).is_none() {
                return Err(format!(
                    "{key} must be a colour like \"#1E88E5\" (got \"{value}\")"
                ));
            }
        }
        let style = o.cursor_style.trim().to_ascii_lowercase();
        if !crate::overlay::draw::CURSOR_STYLES.contains(&style.as_str()) {
            return Err(format!(
                "overlay.cursor_style must be one of {} (got \"{}\")",
                crate::overlay::draw::CURSOR_STYLES.join(", "),
                o.cursor_style
            ));
        }
        let repo = self.update.repo.trim();
        let part_ok = |p: &str| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        if !matches!(repo.split_once('/'), Some((o, n)) if part_ok(o) && part_ok(n)) {
            return Err(format!(
                "update.repo must be a GitHub repository as owner/name (got \"{repo}\")"
            ));
        }
        for (key, value) in [
            ("mouse_path", &self.mouse_path),
            ("overlay.cursor_path", &o.cursor_path),
        ] {
            let v = value.trim().to_ascii_lowercase();
            if !crate::motion::PATH_STYLES.contains(&v.as_str()) {
                return Err(format!(
                    "{key} must be one of {} (got \"{value}\")",
                    crate::motion::PATH_STYLES.join(", ")
                ));
            }
        }
        for (agent, style) in &o.agent_cursors {
            let style = style.trim().to_ascii_lowercase();
            if !crate::overlay::draw::CURSOR_STYLES.contains(&style.as_str()) {
                return Err(format!(
                    "overlay.agent_cursors.{agent} must be one of {} (got \"{style}\")",
                    crate::overlay::draw::CURSOR_STYLES.join(", ")
                ));
            }
        }
        if !(0.0..=1.0).contains(&self.ocr.min_confidence) {
            return Err(format!(
                "ocr.min_confidence must be between 0 and 1 (got {})",
                self.ocr.min_confidence
            ));
        }
        // A level misspelt ("verbose") would turn logging off unnoticed.
        let log = self.server.log.trim().to_ascii_lowercase();
        if !(matches!(
            log.as_str(),
            "" | "off" | "error" | "warn" | "info" | "debug" | "trace"
        ) || log.contains('=')
            || log.contains(','))
        {
            return Err(format!(
                "server.log must be off, error, warn, info, debug or trace (got \"{}\")",
                self.server.log
            ));
        }
        // Waits the stop key can't end: a typo (600000 for 600) would hold
        // every action for minutes.
        let t = &self.timing;
        for (key, value, max) in [
            ("timing.settle_ms", t.settle_ms, 10_000),
            ("timing.settle_max_ms", t.settle_max_ms, 60_000),
            ("timing.settle_poll_ms", t.settle_poll_ms, 10_000),
            ("timing.key_delay_ms", t.key_delay_ms, 5_000),
            ("timing.wait_poll_ms", t.wait_poll_ms, 60_000),
            ("timing.expect_wait_ms", t.expect_wait_ms, 60_000),
            ("overlay.move_ms", self.overlay.move_ms, 5_000),
            (
                "overlay.capture_hide_ms",
                self.overlay.capture_hide_ms,
                2_000,
            ),
        ] {
            if value > max {
                return Err(format!("{key} must be at most {max} (got {value})"));
            }
        }
        let r = self.screenshot.region_max_ratio;
        if !(0.0..=1.0).contains(&r) {
            return Err(format!(
                "screenshot.region_max_ratio must be between 0 and 1 (got {r})"
            ));
        }
        let hotkey = self.control.stop_hotkey.trim();
        if !hotkey.is_empty()
            && let Err(e) = crate::keys::parse_combo(hotkey)
        {
            return Err(format!("control.stop_hotkey: {e}"));
        }
        let settings = self.control.settings_hotkey.trim();
        if !settings.is_empty() {
            let combo = crate::keys::parse_combo(settings)
                .map_err(|e| format!("control.settings_hotkey: {e}"))?;
            if crate::keys::parse_combo(hotkey).is_ok_and(|stop| stop == combo) {
                return Err("control.settings_hotkey must differ from control.stop_hotkey".into());
            }
        }
        let d = &self.decision;
        if !matches!(d.provider.trim(), "" | "jev" | "openai") {
            return Err(format!(
                "decision.provider must be \"jev\", \"openai\" or \"\" (got \"{}\")",
                d.provider
            ));
        }
        let url = d.base_url.trim().to_ascii_lowercase();
        if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!(
                "decision.base_url must start with http:// or https:// (got \"{}\")",
                d.base_url
            ));
        }
        for (key, value) in [
            ("decision.base_url", &d.base_url),
            ("decision.model", &d.model),
            ("decision.api_key", &d.api_key),
            ("decision.api_key_env", &d.api_key_env),
        ] {
            if value.chars().any(char::is_control) {
                return Err(format!("{key} must be one line of text"));
            }
        }
        if !(100..=600_000).contains(&d.timeout_ms) {
            return Err(format!(
                "decision.timeout_ms must be between 100 and 600000 (got {})",
                d.timeout_ms
            ));
        }
        if !(100..=1_000_000).contains(&d.max_state_chars) {
            return Err(format!(
                "decision.max_state_chars must be between 100 and 1000000 (got {})",
                d.max_state_chars
            ));
        }
        if !(1..=64).contains(&d.parallel) {
            return Err(format!(
                "decision.parallel must be between 1 and 64 (got {})",
                d.parallel
            ));
        }
        if !(4..=256).contains(&c.pixel_grid) {
            return Err(format!(
                "cache.pixel_grid must be between 4 and 256 (got {})",
                c.pixel_grid
            ));
        }
        // Numbers that become durations or drawing sizes: out of range (or
        // inf / NaN) they would panic or stall.
        for (key, value, max) in [
            ("launch_timeout_secs", self.launch_timeout_secs, 3600.0),
            ("overlay.scale", o.scale, 8.0),
            (
                "macos.messaging_timeout_secs",
                f64::from(self.macos.messaging_timeout_secs),
                120.0,
            ),
            ("tree.diff_full_ratio", self.tree.diff_full_ratio, 1.0),
            ("script.max_seconds", self.script.max_seconds as f64, 3600.0),
        ] {
            if !(0.0..=max).contains(&value) {
                return Err(format!("{key} must be between 0 and {max} (got {value})"));
            }
        }
        for (key, value) in [
            ("overlay.border_width", o.border_width),
            ("overlay.glow_size", o.glow_size),
        ] {
            if value > 2000 {
                return Err(format!("{key} must be at most 2000 (got {value})"));
            }
        }
        Ok(())
    }
}

/// The server's own folder: `$COMPUTER_USE_HOME`, else `~/.computer-use`.
/// Never relative: the folder a host starts the server in differs from
/// host to host (`/` for some, the project for others).
pub fn home_dir() -> PathBuf {
    let user = dirs::home_dir();
    let env = std::env::var_os(HOME_ENV).map(PathBuf::from);
    home_from(env.as_deref(), user.as_deref())
}

/// `home_dir` from the environment variable and the user's home: an empty
/// variable counts as unset (as the install scripts take it), `~` is the
/// user's home, and a relative path is taken in it. With no home known,
/// the system's temporary folder.
fn home_from(env: Option<&Path>, user: Option<&Path>) -> PathBuf {
    let user = user
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    match env.filter(|p| !p.as_os_str().is_empty()) {
        Some(p) => in_folder(p, &user, &user),
        None => user.join(".computer-use"),
    }
}

/// `p` with a leading `~` (`~`, `~/…`, `~\…`) as `user`, and taken in
/// `base` when it is still relative.
fn in_folder(p: &Path, user: &Path, base: &Path) -> PathBuf {
    let text = p.to_string_lossy();
    let expanded = if text == "~" {
        user.to_path_buf()
    } else if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        user.join(rest)
    } else {
        p.to_path_buf()
    };
    if expanded.is_relative() {
        base.join(expanded)
    } else {
        expanded
    }
}

/// A path from the settings (`script.dir`, `audit.path`): `~` is the
/// user's home and a relative path is in the server's folder
/// (`home_dir`); empty is no path.
pub fn settings_path(p: &Path) -> Option<PathBuf> {
    if p.as_os_str().is_empty() {
        return None;
    }
    let user = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    Some(in_folder(p, &user, &home_dir()))
}

pub fn default_config_path() -> PathBuf {
    home_dir().join("config.toml")
}

/// A fully commented config file with every option at its default value.
pub const TEMPLATE: &str = include_str!("config_template.toml");

/// Keys that are valid even though they don't appear in a serialized default
/// config (optional values that default to unset).
const OPTIONAL_KEYS: &[&str] = &["audit.path", "script.dir"];

fn collect_keys(prefix: &str, table: &toml::Table, out: &mut Vec<String>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        if let toml::Value::Table(t) = v {
            collect_keys(&key, t, out);
        } else {
            out.push(key);
        }
    }
}

/// Every settable key, as dotted paths (e.g. `screenshot.attach`).
pub fn known_keys() -> Vec<String> {
    let table: toml::Table = toml::Table::try_from(Config::default()).unwrap_or_default();
    let mut keys = Vec::new();
    collect_keys("", &table, &mut keys);
    keys.extend(OPTIONAL_KEYS.iter().map(|k| k.to_string()));
    keys.sort();
    keys
}

/// Keys present in `text` that computer-use doesn't recognise (typos).
pub fn unknown_keys(text: &str) -> Vec<String> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let known = known_keys();
    let mut found = Vec::new();
    collect_keys("", &table, &mut found);
    found.retain(|k| !known.contains(k));
    found
}

/// Look up one setting by dotted key in the effective config.
pub fn get_value(config: &Config, key: &str) -> Option<toml::Value> {
    let mut v = toml::Value::try_from(config).ok()?;
    for part in key.split('.') {
        v = v.get(part)?.clone();
    }
    Some(v)
}

/// A change to one setting.
#[derive(Debug, Clone)]
pub enum Edit {
    /// Set a value (parsed as TOML; bare words become strings).
    Set(String),
    /// Set a text value exactly as given (never parsed: an API key).
    SetText(String),
    /// Remove the key so it falls back to its default.
    Unset,
    /// Append an item to a list setting (no duplicates).
    Add(String),
    /// Remove an item from a list setting.
    Remove(String),
}

fn parse_value(raw: &str) -> toml_edit::Value {
    // Accept any TOML literal (numbers, booleans, arrays, quoted strings) and
    // treat anything else as a bare string: `config set screenshot.attach auto`.
    let doc = format!("v = {raw}");
    if let Ok(parsed) = doc.parse::<toml_edit::DocumentMut>()
        && let Some(v) = parsed.get("v").and_then(|i| i.as_value())
    {
        return v.clone();
    }
    toml_edit::Value::from(raw)
}

/// Apply an edit to a config file (created if missing), preserving comments.
/// The result is validated as a whole before anything is written.
pub fn edit_file(path: &Path, key: &str, edit: Edit) -> Result<()> {
    edit_file_many(path, &[(key, edit)])
}

/// A settings file as text: UTF-8 (with or without a byte-order mark), or
/// UTF-16 with its mark, as Windows PowerShell 5.1 writes with `>` and
/// Notepad can save. Written back, it is UTF-8.
pub fn read_text(path: &Path) -> std::io::Result<String> {
    decode_text(&std::fs::read(path)?).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not text in UTF-8 or UTF-16 (save the file as UTF-8)",
        )
    })
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if le {
                    u16::from_le_bytes(c)
                } else {
                    u16::from_be_bytes(c)
                }
            })
            .collect();
        rest.len()
            .is_multiple_of(2)
            .then(|| String::from_utf16(&units).ok())
            .flatten()
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
}

/// Several settings changed at once (all or none), as `edit_file` does
/// one: validated as a whole before anything is written. A file that
/// holds an API key is kept readable by its owner only.
pub fn edit_file_many(path: &Path, edits: &[(&str, Edit)]) -> Result<()> {
    // One edit at a time in this process: each reads the file, changes it
    // and writes it back, so two at once would lose one of them.
    static EDITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut text = match read_text(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::Config(format!("{}: {e}", path.display()))),
    };
    for (key, edit) in edits {
        text = edit_text(&text, key, edit.clone()).map_err(|e| match e {
            Error::Config(m) if m.starts_with("unknown setting") => Error::Config(m),
            Error::Config(m) => Error::Config(format!("{}: {m}", path.display())),
            e => e,
        })?;
    }
    // Validate the whole file before writing.
    let key = edits.first().map(|(k, _)| *k).unwrap_or_default();
    let config = toml::from_str::<Config>(&text)
        .map_err(|e| Error::Config(format!("`{key}`: {}", e.message())))?;
    config.validate().map_err(Error::Config)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::Config(format!("{}: {e}", dir.display())))?;
    }
    let private =
        !config.decision.api_key.trim().is_empty() || !config.server.http_token.trim().is_empty();
    write_atomic(path, &text, private)
}

/// One edit to a config file's text, comments kept (not validated).
fn edit_text(text: &str, key: &str, edit: Edit) -> Result<String> {
    if !known_keys().iter().any(|k| k == key) {
        return Err(Error::Config(format!(
            "unknown setting `{key}` (see `computer-use-mcp config keys`)"
        )));
    }
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| Error::Config(e.to_string()))?;

    let parts: Vec<&str> = key.split('.').collect();
    let (last, tables) = parts.split_last().expect("non-empty key");
    // Table-like: a section may be written as an inline table too
    // (`screenshot = { attach = "always" }`).
    let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for t in tables {
        let entry = table
            .entry(t)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        table = entry
            .as_table_like_mut()
            .ok_or_else(|| Error::Config(format!("`{t}` is not a table")))?;
    }

    match edit {
        Edit::Set(raw) => {
            // A text setting keeps text that reads like a number or a
            // boolean as text (`server.http_token 123456789`).
            let text_key = matches!(
                get_value(&Config::default(), key),
                Some(toml::Value::String(_))
            );
            let v = parse_value(&raw);
            let v = if text_key && !v.is_str() {
                toml_edit::Value::from(raw.trim())
            } else {
                v
            };
            table.insert(last, toml_edit::value(v));
        }
        Edit::SetText(raw) => {
            table.insert(last, toml_edit::value(raw));
        }
        Edit::Unset => {
            table.remove(last);
        }
        Edit::Add(item) => {
            // Start from the current effective list so defaults are kept.
            let current = current_list(&doc_to_config(text)?, key)?;
            let mut arr = toml_edit::Array::new();
            for v in current.iter().chain(std::iter::once(&item)) {
                if !arr.iter().any(|e| e.as_str() == Some(v.as_str())) {
                    arr.push(v.as_str());
                }
            }
            table.insert(last, toml_edit::value(arr));
        }
        Edit::Remove(item) => {
            let current = current_list(&doc_to_config(text)?, key)?;
            let mut arr = toml_edit::Array::new();
            for v in current.iter().filter(|v| !v.eq_ignore_ascii_case(&item)) {
                arr.push(v.as_str());
            }
            table.insert(last, toml_edit::value(arr));
        }
    }
    Ok(doc.to_string())
}

/// Make a settings file readable and writable by its owner only (it holds
/// an API key). Windows keeps a user's profile private already.
pub fn owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            log::warn!("{}: couldn't make it private: {e}", path.display());
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// An API key as it may be shown: its last four characters.
pub fn masked_key(key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    let n = key.chars().count();
    if n <= 8 {
        return "••••".into();
    }
    let tail: String = key.chars().skip(n - 4).collect();
    format!("••••{tail}")
}

/// Write `text` to `path` all at once: a reader (the server reloading its
/// settings) sees the old file or the new one, never half of it.
/// A settings file that is a link is written where it points (the link
/// stays, even before what it points to exists), and keeps its
/// permissions; `private` (it holds an API key or the HTTP token) makes it
/// readable by its owner only, from the moment it is created.
fn write_atomic(path: &Path, text: &str, private: bool) -> Result<()> {
    let fail = |e: std::io::Error| Error::Config(format!("{}: {e}", path.display()));
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| match std::fs::read_link(path) {
        // A link to a file not made yet: write that file.
        Ok(to) => path.parent().map(|dir| dir.join(&to)).unwrap_or(to),
        Err(_) => path.to_path_buf(),
    });
    // Unique per save: two saves at once (the settings page and a tool)
    // never share a temporary file.
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    {
        use std::io::Write as _;
        let mut f = opts.open(&tmp).map_err(fail)?;
        f.write_all(text.as_bytes()).map_err(fail)?;
    }
    // (On Windows a permission is only "read-only", which would stop the
    // file being replaced.)
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(&path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if private {
        owner_only(&tmp);
    }
    // Windows refuses to replace a file another program has open for a
    // moment (an antivirus scan, the search indexer): try a few times.
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, &path) {
            Ok(()) => return Ok(()),
            Err(e)
                if cfg!(windows)
                    && e.kind() == std::io::ErrorKind::PermissionDenied
                    && tries < 10 =>
            {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(fail(e));
            }
        }
    }
}

fn doc_to_config(text: &str) -> Result<Config> {
    toml::from_str(text).map_err(|e| Error::Config(e.message().to_string()))
}

fn current_list(config: &Config, key: &str) -> Result<Vec<String>> {
    match get_value(config, key) {
        Some(toml::Value::Array(a)) => Ok(a
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()),
        Some(_) => Err(Error::Config(format!("`{key}` is not a list"))),
        None => Ok(Vec::new()),
    }
}

/// Write the documented template to `path` (refusing to overwrite unless `force`).
pub fn write_template(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(Error::Config(format!(
            "{} already exists (use --force to overwrite)",
            path.display()
        )));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::Config(format!("{}: {e}", dir.display())))?;
    }
    write_atomic(path, TEMPLATE, false)
}

/// Configuration plus where it lives (for hot reload).
#[derive(Debug, Clone)]
pub struct ConfigStore {
    pub config: Config,
    /// `None` keeps everything in memory (tests, embedders).
    pub path: Option<PathBuf>,
}

impl ConfigStore {
    pub fn in_memory(config: Config) -> Self {
        Self { config, path: None }
    }

    /// Load the user config (missing file = defaults).
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path
            .map(Path::to_path_buf)
            .unwrap_or_else(default_config_path);
        let config = match read_text(&path) {
            Ok(text) => {
                for key in unknown_keys(&text) {
                    log::warn!("{}: unknown setting `{key}` (ignored)", path.display());
                }
                let config: Config = toml::from_str(&text)
                    .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
                config
                    .validate()
                    .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
                config
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(Error::Config(format!("{}: {e}", path.display()))),
        };
        Ok(Self {
            config,
            path: Some(path),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_agent_can_have_its_own_pointer() {
        let mut o = OverlayConfig::default();
        o.agent_cursors.insert("Codex".into(), "metal".into());
        o.agent_cursors.insert("Zero".into(), "ice".into());
        assert_eq!(o.for_agent(&["codex", "Zero"]).cursor_style, "metal");
        assert_eq!(o.for_agent(&["claude-code", "zero"]).cursor_style, "ice");
        assert_eq!(o.for_agent(&["claude-code", "Ada"]).cursor_style, "random");
        let mut c = Config::default();
        c.overlay
            .agent_cursors
            .insert("codex".into(), "velvet".into());
        assert!(
            c.validate()
                .unwrap_err()
                .contains("overlay.agent_cursors.codex")
        );
    }

    use super::*;

    #[test]
    fn waits_the_stop_key_cant_end_are_refused() {
        let mut c = Config::default();
        assert!(c.validate().is_ok());
        c.timing.settle_ms = 600_000;
        assert!(c.validate().unwrap_err().contains("timing.settle_ms"));
        c.timing.settle_ms = 40;
        c.overlay.move_ms = 60_000;
        assert!(c.validate().unwrap_err().contains("overlay.move_ms"));
        c.overlay.move_ms = 220;
        c.server.log = "verbose".into();
        assert!(c.validate().unwrap_err().contains("server.log"));
        c.server.log = "computer_use=debug".into();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn folders_never_depend_on_where_the_server_started() {
        // Absolute on every system (a path like /home/u isn't on Windows).
        let user_dir = std::env::temp_dir().join("u");
        let user = user_dir.as_path();
        let home = |env: Option<&str>| home_from(env.map(Path::new), Some(user));
        assert_eq!(home(None), user.join(".computer-use"));
        assert_eq!(home(Some("")), user.join(".computer-use"), "empty is unset");
        assert_eq!(home(Some("~/.cu")), user.join(".cu"));
        assert_eq!(home(Some("~")), user.to_path_buf());
        assert_eq!(home(Some("cu")), user.join("cu"));
        let abs = std::env::temp_dir().join("cu-abs");
        assert_eq!(home_from(Some(&abs), Some(user)), abs);
        assert!(
            home_from(None, None).is_absolute(),
            "no home: still absolute"
        );
        let base_dir = user.join(".computer-use");
        let base = base_dir.as_path();
        assert_eq!(in_folder(Path::new("~/s"), user, base), user.join("s"));
        assert_eq!(in_folder(Path::new("s"), user, base), base.join("s"));
        assert_eq!(settings_path(Path::new("")), None);
        assert!(settings_path(Path::new("scripts")).unwrap().is_absolute());
    }

    #[test]
    fn settings_saved_as_utf16_or_with_a_mark_are_read() {
        let text = "[tree]\nmax_nodes = 300 # é\n";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend(text.as_bytes());
        for bytes in [le, be, bom, text.as_bytes().to_vec()] {
            let t = decode_text(&bytes).unwrap();
            assert_eq!(t, text);
            let c: Config = toml::from_str(&t).unwrap();
            assert_eq!(c.tree.max_nodes, 300);
        }
        assert_eq!(decode_text(&[0xFF, 0xFE, 0x41]), None, "odd length");
        assert_eq!(decode_text(&[0xC3, 0x28]), None, "not UTF-8");
    }

    #[test]
    fn defaults_when_missing() {
        let dir = std::env::temp_dir().join(format!("cu-cfg-missing-{}", std::process::id()));
        let store = ConfigStore::load(Some(&dir.join("config.toml"))).unwrap();
        assert_eq!(store.config, Config::default());
    }

    #[test]
    fn edits_reach_into_inline_tables() {
        let text = "screenshot = { attach = \"always\", max_dimension = 1024 }\n";
        let out = edit_text(text, "screenshot.attach", Edit::Unset).unwrap();
        assert!(!out.contains("attach") && out.contains("1024"), "{out}");
        let out = edit_text(text, "screenshot.max_dimension", Edit::Set("800".into())).unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.screenshot.max_dimension, 800);
        // Text that looks like a number stays text where text is meant.
        let out = edit_text("", "server.http_token", Edit::Set("123456789".into())).unwrap();
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.server.http_token, "123456789");
    }

    #[test]
    fn parses_partial_config() {
        let cfg: Config = toml::from_str(
            r#"
            [tree]
            max_nodes = 300
            [screenshot]
            max_dimension = 1024
            "#,
        )
        .unwrap();
        assert_eq!(cfg.tree.max_nodes, 300);
        assert_eq!(cfg.screenshot.max_dimension, 1024);
        assert!(cfg.screenshot.enabled);
        assert!(!cfg.privacy.redact_labels.is_empty());
    }

    #[test]
    fn template_matches_defaults_and_covers_every_key() {
        let parsed: Config = toml::from_str(TEMPLATE).expect("template parses");
        assert_eq!(parsed, Config::default(), "template defaults drifted");
        assert!(
            unknown_keys(TEMPLATE).is_empty(),
            "{:?}",
            unknown_keys(TEMPLATE)
        );
        // Every known key (except optional ones) is spelled out in the template.
        let mut in_template = Vec::new();
        collect_keys(
            "",
            &TEMPLATE.parse::<toml::Table>().unwrap(),
            &mut in_template,
        );
        for key in known_keys() {
            if !OPTIONAL_KEYS.contains(&key.as_str()) {
                assert!(in_template.contains(&key), "template is missing `{key}`");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_edit_keeps_the_link_and_the_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("cu-edit-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.toml");
        std::fs::write(&real, "[server]\nhttp_token = \"secret\"\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        edit_file(&link, "tree.max_nodes", Edit::Set("300".into())).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("max_nodes = 300")
        );
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_with_the_http_token_is_its_owners_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("cu-edit-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        edit_file(&path, "server.http_token", Edit::Set("123456789".into())).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_file_not_made_yet_stays_a_link() {
        let dir = std::env::temp_dir().join(format!("cu-edit-dangling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("config.toml");
        std::os::unix::fs::symlink("real.toml", &link).unwrap();
        edit_file(&link, "tree.max_nodes", Edit::Set("300".into())).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(dir.join("real.toml"))
                .unwrap()
                .contains("max_nodes = 300")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_set_add_remove_unset() {
        let dir = std::env::temp_dir().join(format!("cu-edit-{}", std::process::id()));
        let path = dir.join("config.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "# keep me\n").unwrap();

        edit_file(&path, "screenshot.attach", Edit::Set("always".into())).unwrap();
        edit_file(&path, "tree.max_nodes", Edit::Set("300".into())).unwrap();
        edit_file(&path, "verify.retry", Edit::Set("false".into())).unwrap();
        edit_file(&path, "tools.disabled", Edit::Add("drag".into())).unwrap();
        edit_file(&path, "tools.disabled", Edit::Add("scroll".into())).unwrap();
        edit_file(&path, "tools.disabled", Edit::Add("scroll".into())).unwrap();
        edit_file(&path, "privacy.redact_labels", Edit::Remove("cvc".into())).unwrap();

        let store = ConfigStore::load(Some(&path)).unwrap();
        let c = &store.config;
        assert_eq!(c.screenshot.attach, AttachMode::Always);
        assert_eq!(c.tree.max_nodes, 300);
        assert!(!c.verify.retry);
        assert_eq!(c.tools.disabled, vec!["drag", "scroll"]);
        assert!(!c.privacy.redact_labels.contains(&"cvc".to_string()));
        assert!(
            c.privacy.redact_labels.contains(&"cvv".to_string()),
            "defaults kept"
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# keep me")
        );

        // Invalid values and unknown keys are rejected without writing.
        assert!(edit_file(&path, "screenshot.attach", Edit::Set("sometimes".into())).is_err());
        assert!(edit_file(&path, "tree.max_nodes", Edit::Set("lots".into())).is_err());
        assert!(edit_file(&path, "screenshot.atach", Edit::Set("auto".into())).is_err());
        assert_eq!(
            ConfigStore::load(Some(&path))
                .unwrap()
                .config
                .screenshot
                .attach,
            AttachMode::Always
        );

        edit_file(&path, "screenshot.attach", Edit::Unset).unwrap();
        assert_eq!(
            ConfigStore::load(Some(&path))
                .unwrap()
                .config
                .screenshot
                .attach,
            AttachMode::Auto
        );
        assert_eq!(
            unknown_keys("[tree]\nmax_nodez = 1\n"),
            vec!["tree.max_nodez"]
        );
        assert_eq!(
            get_value(&Config::default(), "timing.settle_ms"),
            Some(toml::Value::Integer(40))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn out_of_range_cache_settings_are_rejected() {
        assert!(Config::default().validate().is_ok());
        let dir = std::env::temp_dir().join(format!("cu-cfg-range-{}", std::process::id()));
        let path = dir.join("config.toml");
        let err = edit_file(&path, "cache.match_threshold", Edit::Set("2".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("between 0 and 1"), "{err}");
        edit_file(&path, "cache.match_threshold", Edit::Set("0.6".into())).unwrap();
        std::fs::write(&path, "[cache]\npixel_grid = 1\n").unwrap();
        assert!(ConfigStore::load(Some(&path)).is_err());
        // Numbers that would panic or stall as durations or drawing sizes.
        for bad in [
            "launch_timeout_secs = inf\n",
            "launch_timeout_secs = 1e20\n",
            "launch_timeout_secs = -1\n",
            "[overlay]\nscale = nan\n",
            "[overlay]\nglow_size = 100000\n",
            "[tree]\ndiff_full_ratio = 3\n",
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(ConfigStore::load(Some(&path)).is_err(), "{bad}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
