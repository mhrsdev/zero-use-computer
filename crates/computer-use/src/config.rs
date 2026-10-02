//! User configuration (`~/.computer-use/config.toml`).
//!
//! There is no access control here: which apps and actions the agent may
//! use is left to the agent's security skill (`skills/computer-use-security`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const HOME_ENV: &str = "COMPUTER_USE_HOME";

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
            adaptive: false,
            locate_picture: true,
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
    /// Every change, up to `report_changes_max_lines`. Default.
    #[default]
    Full,
    /// Changes in and around what the action acted on, new windows and
    /// added elements; how many others there are.
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
            report: Report::Full,
            quiet_volatile: false,
        }
    }
}

/// How tool definitions are presented to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DescriptionStyle {
    /// Detailed descriptions for every tool and parameter.
    Full,
    /// One-line tool descriptions, no per-parameter prose. Default; the tool
    /// list is sent with every model request, so this saves the most tokens.
    #[default]
    Compact,
    /// Compact, and lighter schemas still: the keys of nested objects (a
    /// design's layers, a drawing's strokes) listed by name instead of
    /// typed one by one, no `window` (still accepted) and no defaults;
    /// `decide` only once a decision model is set up.
    Lean,
}

/// Which tools the model sees at first ([tools] manager).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolManager {
    /// Every tool, always. Default.
    #[default]
    Off,
    /// The base tools and `find_tools`; the others are found by name or
    /// category and run through `use_tool`. The tool list never changes,
    /// so a client's prompt cache keeps working, with any client.
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
    /// With every answer (default).
    #[default]
    Always,
    /// When asked (show steps=true); otherwise only how many there are.
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
            design_steps: DesignSteps::Always,
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
    /// No new action for this long after the last one: done (green), then hidden.
    pub done_after_ms: u64,
    /// How long "done" stays on screen before everything disappears.
    pub done_linger_ms: u64,
    /// How long an error stays red before returning to "thinking".
    pub error_hold_ms: u64,
    /// Duration of the cursor's glide to a new point.
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
            .clone()
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
            blind_regions: false,
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
    /// Global key combination that opens the settings page for the decision
    /// model in the browser. "" = none.
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
            result_meta: false,
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
            linux: LinuxConfig::default(),
            macos: MacosConfig::default(),
            windows: WindowsConfig::default(),
            clipboard: true,
            text_only: false,
            follow_new_windows: true,
            restore_pointer: true,
            hot_reload: true,
            launch_timeout_secs: 15.0,
        }
    }
}

impl Config {
    /// Range checks the types can't express.
    pub fn validate(&self) -> std::result::Result<(), String> {
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
        if !(0.0..=1.0).contains(&self.ocr.min_confidence) {
            return Err(format!(
                "ocr.min_confidence must be between 0 and 1 (got {})",
                self.ocr.min_confidence
            ));
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

pub fn home_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(HOME_ENV) {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".computer-use")
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

/// Several settings changed at once (all or none), as `edit_file` does
/// one: validated as a whole before anything is written. A file that
/// holds an API key is kept readable by its owner only.
pub fn edit_file_many(path: &Path, edits: &[(&str, Edit)]) -> Result<()> {
    let mut text = match std::fs::read_to_string(path) {
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
    let private = !config.decision.api_key.trim().is_empty();
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
    let mut table = doc.as_table_mut();
    for t in tables {
        let entry = table
            .entry(t)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        table = entry
            .as_table_mut()
            .ok_or_else(|| Error::Config(format!("`{t}` is not a table")))?;
    }

    match edit {
        Edit::Set(raw) => {
            table[last] = toml_edit::value(parse_value(&raw));
        }
        Edit::SetText(raw) => {
            table[last] = toml_edit::value(raw);
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
            table[last] = toml_edit::value(arr);
        }
        Edit::Remove(item) => {
            let current = current_list(&doc_to_config(text)?, key)?;
            let mut arr = toml_edit::Array::new();
            for v in current.iter().filter(|v| !v.eq_ignore_ascii_case(&item)) {
                arr.push(v.as_str());
            }
            table[last] = toml_edit::value(arr);
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
/// stays), and keeps its permissions (it may hold `server.http_token`);
/// `private` (it holds an API key) makes it readable by its owner only,
/// from the moment it is created.
fn write_atomic(path: &Path, text: &str, private: bool) -> Result<()> {
    let fail = |e: std::io::Error| Error::Config(format!("{}: {e}", path.display()));
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
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
    if let Ok(meta) = std::fs::metadata(&path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if private {
        owner_only(&tmp);
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        fail(e)
    })
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
        let config = match std::fs::read_to_string(&path) {
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
    use super::*;

    #[test]
    fn defaults_when_missing() {
        let dir = std::env::temp_dir().join(format!("cu-cfg-missing-{}", std::process::id()));
        let store = ConfigStore::load(Some(&dir.join("config.toml"))).unwrap();
        assert_eq!(store.config, Config::default());
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
