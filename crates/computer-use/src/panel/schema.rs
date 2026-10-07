//! What the panel knows about each setting: its group, a line of help and
//! how to edit it. The type and the default come from the real default
//! settings (`Config::default()`), so they can't drift; a test fails when a
//! settable key has no entry here.

/// How the panel edits a setting, when its type alone doesn't say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// By its type: a switch, a number, a line of text or a list.
    Auto,
    /// One of these.
    Choice(&'static [&'static str]),
    /// A number with a slider.
    Range {
        min: f64,
        max: f64,
        step: f64,
        unit: &'static str,
    },
    /// A number with a unit, edited as a field.
    Number { unit: &'static str },
    /// #RRGGBB.
    Color,
    /// A key combination such as ctrl+alt+escape.
    Hotkey,
    /// Write-only text (an API key, a token): the panel never gets it back.
    Secret,
    /// Edited on a page of its own (the decision model).
    Custom,
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub key: &'static str,
    pub group: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// Takes effect when the server starts again, not at once.
    pub restart: bool,
    /// Asks the user to confirm before it is changed.
    pub confirm: bool,
    /// Only for people who know what it does.
    pub advanced: bool,
}

const fn e(key: &'static str, group: &'static str, help: &'static str) -> Entry {
    Entry {
        key,
        group,
        help,
        kind: Kind::Auto,
        restart: false,
        confirm: false,
        advanced: false,
    }
}

impl Entry {
    const fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
    const fn choice(self, c: &'static [&'static str]) -> Self {
        self.kind(Kind::Choice(c))
    }
    const fn range(self, min: f64, max: f64, step: f64, unit: &'static str) -> Self {
        self.kind(Kind::Range {
            min,
            max,
            step,
            unit,
        })
    }
    const fn number(self, unit: &'static str) -> Self {
        self.kind(Kind::Number { unit })
    }
    const fn restart(mut self) -> Self {
        self.restart = true;
        self
    }
    const fn confirm(mut self) -> Self {
        self.confirm = true;
        self
    }
    const fn advanced(mut self) -> Self {
        self.advanced = true;
        self
    }
}

pub const POINTER: &str = "Pointer";
pub const MOUSE: &str = "Real mouse";
pub const OVERLAY: &str = "Overlay";
pub const SCREENSHOTS: &str = "Screenshots";
pub const TREE: &str = "Accessibility tree";
pub const TOOLS: &str = "Tools and tokens";
pub const TIMING: &str = "Timing";
pub const MEMORY: &str = "Screen memory";
pub const OCR: &str = "Text on screen (OCR)";
pub const DECISION: &str = "Decision model";
pub const PRIVACY: &str = "Privacy";
pub const CONTROL: &str = "Your control";
pub const AGENTS: &str = "Several agents";
pub const NOTIFICATIONS: &str = "Notifications";
pub const SCRIPTS: &str = "Scripts";
pub const VERIFY: &str = "Checking actions";
pub const AUDIT: &str = "Audit log";
pub const SERVER: &str = "Server";
pub const UPDATES: &str = "Updates";
pub const PLATFORM: &str = "Platform";
pub const PANEL: &str = "Panel";
pub const GENERAL: &str = "General";

/// The groups, in the order the panel lists them.
pub const GROUPS: &[&str] = &[
    GENERAL,
    POINTER,
    MOUSE,
    OVERLAY,
    SCREENSHOTS,
    TREE,
    TOOLS,
    TIMING,
    MEMORY,
    OCR,
    DECISION,
    PRIVACY,
    CONTROL,
    AGENTS,
    NOTIFICATIONS,
    SCRIPTS,
    VERIFY,
    AUDIT,
    SERVER,
    UPDATES,
    PLATFORM,
    PANEL,
];

const PATHS: &[&str] = &["mixed", "hand", "sine", "arc", "spring", "spiral"];
const POINTERS: &[&str] = &[
    "random", "classic", "crystal", "paper", "jelly", "ice", "metal", "orbit",
];
const ONOFF: &[&str] = &["auto", "always", "never"];

pub const ENTRIES: &[Entry] = &[
    // General
    e("clipboard", GENERAL, "Let the agent read and write the clipboard (the get_clipboard and set_clipboard tools)."),
    e("text_only", GENERAL, "Never attach screenshots anywhere. The lowest token use, but the agent can't see pictures."),
    e("follow_new_windows", GENERAL, "When an action opens a dialog or a menu, switch to it for the change report and the next look."),
    e("hot_reload", GENERAL, "Apply changes to the settings file to running servers without a restart."),
    e("launch_timeout_secs", GENERAL, "How long launch_app waits for a new app to show a window.").number("seconds"),
    // Real mouse
    e("natural_mouse", MOUSE, "Move the real mouse like a hand does: along a curve, speeding up and slowing down, sometimes a touch past the target and back. Off: it jumps."),
    e("mouse_path", MOUSE, "The way the real mouse goes. Mixed picks one at random for each move. Drags always go straight.").choice(PATHS),
    e("mouse_speed", MOUSE, "How fast the real mouse moves, as a multiple of a hand's pace. 2 is twice as quick; some apps are slow to notice a very fast pointer.").range(0.5, 2.0, 0.1, "×"),
    e("mouse_overshoot", MOUSE, "How often a long reach goes a touch past its target and back, as a share of how often a hand does. Only the hand style does this; the spring style always does.").range(0.0, 100.0, 5.0, "%"),
    e("mouse_jitter", MOUSE, "How much the path trembles, as a share of a hand's. 0 is a clean curve.").range(0.0, 200.0, 10.0, "%"),
    e("restore_pointer", MOUSE, "Put your mouse pointer back where it was after the agent clicks, scrolls or drags, so it never takes your mouse away."),
    // Pointer
    e("overlay.cursor_style", POINTER, "The agent's pointer. Random gives each session one, and a different one to each agent at once.").choice(POINTERS),
    e("overlay.cursor_color", POINTER, "The colour of the plain arrow and the base of the pointer's ring.").kind(Kind::Color),
    e("overlay.cursor_tag", POINTER, "The name beside the pointer. Empty: no name."),
    e("overlay.cursor_motion", POINTER, "The pointer leans as it moves, leaves a trail, breathes, and shows drags and scrolls."),
    e("overlay.trail", POINTER, "The trail the pointer leaves as it moves, made of its own material (sparkles, flecks, frost, chrome drops). Needs cursor motion."),
    e("overlay.trail_strength", POINTER, "How long the trail is. 100 is as it was made; 0 leaves none.").range(0.0, 100.0, 5.0, "%"),
    e("overlay.lean_strength", POINTER, "How far the pointer leans into a move. 0 keeps it upright.").range(0.0, 100.0, 5.0, "%"),
    e("overlay.breathe", POINTER, "The pointer breathes while it waits: the jelly swells, the paper sways. Needs cursor motion."),
    e("overlay.breathe_strength", POINTER, "How deeply it breathes.").range(0.0, 100.0, 5.0, "%"),
    e("overlay.cursor_path", POINTER, "How the pointer glides to where it acts.").choice(PATHS),
    e("overlay.show_keys", POINTER, "Show pressed keys as keycaps by the pointer, and typed text running out beside it."),
    e("overlay.click_effect", POINTER, "A ripple where the agent clicks."),
    e("overlay.show_cursor", POINTER, "Draw the agent's own pointer. The real mouse is never moved for this."),
    e("overlay.move_ms", POINTER, "How long the pointer takes to glide. An action waits for it to arrive.").range(0.0, 5000.0, 10.0, "ms"),
    e("overlay.scale", POINTER, "Size multiplier for everything the overlay draws. 0 follows the display scaling.").range(0.0, 8.0, 0.1, "×"),
    // Overlay
    e("overlay.enabled", OVERLAY, "Show the on-screen indicator while the agent works. It is click-through and left out of the agent's screenshots."),
    e("overlay.show_border", OVERLAY, "A glowing border around the screen or window the agent works on."),
    e("overlay.show_label", OVERLAY, "A status label."),
    e("overlay.border_target", OVERLAY, "Whether the border goes around the whole screen or only the window worked on.").choice(&["screen", "window"]),
    e("overlay.border_width", OVERLAY, "The bright core line of the border.").range(0.0, 40.0, 1.0, "px"),
    e("overlay.glow_size", OVERLAY, "How far the glow fades out. 0 is a plain line.").range(0.0, 200.0, 1.0, "px"),
    e("overlay.label_working", OVERLAY, "Label while the agent acts on the computer."),
    e("overlay.label_thinking", OVERLAY, "Label while the model is thinking."),
    e("overlay.label_error", OVERLAY, "Label after an error."),
    e("overlay.label_done", OVERLAY, "Label when the work is finished."),
    e("overlay.label_paused", OVERLAY, "Label while it waits for you to stop using the mouse and keyboard."),
    e("overlay.label_stopped", OVERLAY, "Label after the emergency stop. {hotkey} becomes the stop key."),
    e("overlay.color_thinking", OVERLAY, "Colour while the model is thinking.").kind(Kind::Color),
    e("overlay.color_working", OVERLAY, "Colour while the agent acts.").kind(Kind::Color),
    e("overlay.color_error", OVERLAY, "Colour after an error.").kind(Kind::Color),
    e("overlay.color_done", OVERLAY, "Colour when the work is finished.").kind(Kind::Color),
    e("overlay.color_paused", OVERLAY, "Colour while paused for you.").kind(Kind::Color),
    e("overlay.color_stopped", OVERLAY, "Colour after the emergency stop.").kind(Kind::Color),
    e("overlay.done_after_ms", OVERLAY, "No new action for this long: the work counts as done.").number("ms"),
    e("overlay.done_linger_ms", OVERLAY, "How long \"done\" stays before everything disappears.").number("ms"),
    e("overlay.error_hold_ms", OVERLAY, "How long an error stays.").number("ms"),
    e("overlay.fade_in_ms", OVERLAY, "Appear gradually over this time.").number("ms"),
    e("overlay.fade_out_ms", OVERLAY, "Disappear gradually over this time.").number("ms"),
    e("overlay.transition_ms", OVERLAY, "Blend from one state colour to the next over this time.").number("ms"),
    e("overlay.capture_hide_ms", OVERLAY, "X11 only: the pause after hiding the overlay before a screenshot.").range(0.0, 2000.0, 10.0, "ms").advanced(),
    e("overlay.font", OVERLAY, "A font file for the label. Empty: a system font.").advanced(),
    e("overlay.command", OVERLAY, "A helper program for the overlay, for programs that embed the library.").advanced(),
    // Screenshots
    e("screenshot.enabled", SCREENSHOTS, "Allow screenshots at all."),
    e("screenshot.attach", SCREENSHOTS, "When a look includes a picture: auto (first view, a big change, or a sparse tree), always (most tokens) or never.").choice(ONOFF),
    e("screenshot.auto_sparse_threshold", SCREENSHOTS, "In auto mode, attach a picture when the tree has fewer interactive elements than this.").number("elements"),
    e("screenshot.max_dimension", SCREENSHOTS, "The longest edge of a picture. This is the main image-token setting: 1024 costs about 35% less than 1280.").range(256.0, 4096.0, 64.0, "px"),
    e("screenshot.overview_max_dimension", SCREENSHOTS, "A smaller size for pictures of windows whose tree already says what is there. 0: always the size above.").range(0.0, 4096.0, 64.0, "px"),
    e("screenshot.format", SCREENSHOTS, "Picture format.").choice(&["png", "jpeg"]),
    e("screenshot.jpeg_quality", SCREENSHOTS, "JPEG quality.").range(1.0, 100.0, 1.0, "%"),
    e("screenshot.png_compression", SCREENSHOTS, "PNG compression. It changes the file size only.").choice(&["fast", "default", "best"]),
    e("screenshot.resize_filter", SCREENSHOTS, "How a picture is shrunk.").choice(&["fast", "smooth", "sharp"]),
    e("screenshot.scope", SCREENSHOTS, "Follow-up pictures of a screen the model has seen: only the part that changed (auto), or the whole window (full).").choice(&["auto", "full"]),
    e("screenshot.region_max_ratio", SCREENSHOTS, "A change bigger than this share of the window sends the whole window.").range(0.0, 1.0, 0.05, ""),
    e("screenshot.region_padding", SCREENSHOTS, "Margin around the changed part.").number("px"),
    e("screenshot.region_min_size", SCREENSHOTS, "The changed part is at least this big on each side, for context.").number("px"),
    e("screenshot.adaptive", SCREENSHOTS, "Leave pictures out of well-described windows once the model has looked a few times without using pixels there."),
    e("screenshot.smart", SCREENSHOTS, "Leave out a picture when all that changed is what the tree already reports."),
    e("screenshot.record_apps", SCREENSHOTS, "Count how often each app needed pixels (the doctor command shows it)."),
    e("screenshot.locate_picture", SCREENSHOTS, "Locate sends the window with the places it found numbered."),
    e("screenshot.icon_sprite", SCREENSHOTS, "Experimental: when no picture is attached, a small strip of the buttons that have no name, numbered."),
    // Tree
    e("tree.max_nodes", TREE, "Elements shown per look.").number("elements"),
    e("tree.max_walk", TREE, "Elements read from the app before pruning.").number("elements").advanced(),
    e("tree.max_depth", TREE, "How deep the tree goes.").number("levels").advanced(),
    e("tree.max_text_len", TREE, "Characters of text or value shown per element.").number("characters"),
    e("tree.indent", TREE, "Spaces per tree level.").number("spaces"),
    e("tree.show_actions", TREE, "Show the actions each element offers."),
    e("tree.show_states", TREE, "Show states such as focused, disabled and checked."),
    e("tree.diff", TREE, "Later looks return only what changed."),
    e("tree.diff_full_ratio", TREE, "Show the whole tree when more than this share changed.").range(0.0, 1.0, 0.01, ""),
    e("tree.report_changes", TREE, "Add the state after the action to an action's result."),
    e("tree.report_changes_max_lines", TREE, "At most this many lines of changes in a report.").number("lines"),
    e("tree.report", TREE, "What a report says: full (every change), relevant (around what was acted on) or brief (counts).").choice(&["full", "relevant", "brief"]),
    e("tree.max_tokens", TREE, "Token budget of one tree. Bigger trees are shortened. 0: no limit.").number("tokens"),
    e("tree.summarize", TREE, "How a tree over the budget is shortened: normal (fold, then cut), light (only fold lists) or off.").choice(&["normal", "light", "off"]),
    e("tree.fold_keep", TREE, "Items kept at the start of a folded list (and 2 at its end).").number("items"),
    e("tree.brief_repeats", TREE, "Explain notes in full once, then briefly."),
    e("tree.compact", TREE, "Say each thing once without losing anything: look-alike siblings as records, ranges of indices, and so on."),
    e("tree.quiet_volatile", TREE, "Things that change on their own (clocks, spinners) are summed up in one line after a few reports."),
    // Tools
    e("tools.disabled", TOOLS, "Tools the model never sees, for example drag or batch."),
    e("tools.enabled", TOOLS, "If not empty, only these tools are offered."),
    e("tools.descriptions", TOOLS, "How long the tool descriptions are. Lean uses the fewest tokens.").choice(&["lean", "compact", "full"]),
    e("tools.manager", TOOLS, "Which tools the model sees at first. Dispatch: the base tools plus find_tools, the rest on demand, and the list never changes.").choice(&["off", "dispatch", "list_changed"]),
    e("tools.preset", TOOLS, "Full: every tool the other settings allow. Small: a fixed small set for smaller models.").choice(&["full", "small"]),
    e("tools.default_app", TOOLS, "A call without an app acts on the app of the last call that named one."),
    e("tools.launch_look", TOOLS, "launch_app answers with the app's first state."),
    e("tools.design_steps", TOOLS, "When design lists the steps to paint a design.").choice(&["asked", "always"]),
    // Timing
    e("timing.settle_ms", TIMING, "Pause after each action before looking again.").range(0.0, 10000.0, 10.0, "ms"),
    e("timing.settle", TIMING, "Adaptive waits until the screen stops changing; fixed only pauses.").choice(&["adaptive", "fixed"]),
    e("timing.settle_max_ms", TIMING, "The longest an adaptive wait lasts.").range(0.0, 60000.0, 50.0, "ms"),
    e("timing.settle_poll_ms", TIMING, "How often the screen is checked while waiting.").range(0.0, 10000.0, 10.0, "ms"),
    e("timing.key_delay_ms", TIMING, "Pause between keys in a key sequence.").range(0.0, 5000.0, 5.0, "ms"),
    e("timing.app_cache_ms", TIMING, "Reuse the running-app list for this long.").number("ms"),
    e("timing.wait_timeout_ms", TIMING, "The default timeout of wait_for.").number("ms"),
    e("timing.wait_poll_ms", TIMING, "The default poll interval of wait_for.").range(0.0, 60000.0, 50.0, "ms"),
    e("timing.expect_wait_ms", TIMING, "How long an action with expect waits for the result to show.").range(0.0, 60000.0, 100.0, "ms"),
    e("timing.adaptive_grace", TIMING, "Say \"nothing changed\" after 200 ms instead of 500 for apps that show changes at once."),
    // Screen memory
    e("cache.enabled", MEMORY, "Remember screens the model has seen: coming back to one sends only what differs."),
    e("cache.max_screens", MEMORY, "Screens remembered, all apps together.").number("screens"),
    e("cache.max_memory_kb", MEMORY, "Memory the remembered screens may use.").number("KB"),
    e("cache.match_threshold", MEMORY, "Share of common elements that counts as the same screen.").range(0.0, 1.0, 0.01, ""),
    e("cache.dedupe_screenshots", MEMORY, "Never send an unchanged picture of a screen twice."),
    e("cache.pixel_grid", MEMORY, "Cells across the longer side for the picture fingerprint.").range(4.0, 256.0, 1.0, "cells").advanced(),
    e("cache.pixel_tolerance", MEMORY, "Brightness drift per cell still counted as unchanged.").advanced(),
    e("cache.snapshot_ttl_ms", MEMORY, "Reuse a read this recent if nothing ran since. 0: off.").number("ms").advanced(),
    e("cache.rebase_after_tokens", MEMORY, "After this many tokens of results, send a tree whole again with a picture. 0: never.").number("tokens"),
    // OCR
    e("ocr.mode", OCR, "Read text off the screen for apps whose tree says little: auto (sparse trees only), always or off.").choice(&["auto", "always", "off"]),
    e("ocr.engine", OCR, "The reader: the system's own, or Tesseract.").choice(&["auto", "native", "tesseract"]),
    e("ocr.sparse_threshold", OCR, "In auto mode, read when the tree has fewer interactive elements than this.").number("elements"),
    e("ocr.languages", OCR, "Languages to read, for example en or de. Empty: your own languages."),
    e("ocr.min_confidence", OCR, "Leave out lines recognised with less confidence.").range(0.0, 1.0, 0.05, ""),
    e("ocr.max_lines", OCR, "At most this many lines.").number("lines"),
    e("ocr.tesseract_path", OCR, "The Tesseract program."),
    e("ocr.blind_regions", OCR, "Also read areas the tree says nothing about (a canvas, a picture) in windows that have plenty of other elements."),
    // Decision
    e("decision.provider", DECISION, "The kind of decision model.").kind(Kind::Custom),
    e("decision.base_url", DECISION, "The model's address.").kind(Kind::Custom),
    e("decision.model", DECISION, "The model's name.").kind(Kind::Custom),
    e("decision.api_key", DECISION, "The model's API key.").kind(Kind::Custom),
    e("decision.api_key_env", DECISION, "An environment variable that holds the key.").kind(Kind::Custom),
    e("decision.auto", DECISION, "The server asks the model on its own where that saves the agent a turn."),
    e("decision.auto_timeout_ms", DECISION, "The longest wait for those questions.").number("ms"),
    e("decision.timeout_ms", DECISION, "The longest wait for one answer.").number("ms"),
    e("decision.max_state_chars", DECISION, "A longer state keeps its start and its end.").number("characters"),
    e("decision.parallel", DECISION, "Requests at once when judging several items.").range(1.0, 64.0, 1.0, ""),
    e("decision.cache_seconds", DECISION, "The same question about the same state is answered once for this long.").number("seconds"),
    // Privacy
    e("privacy.redact_passwords", PRIVACY, "Mask password fields in text and black them out of pictures.").confirm(),
    e("privacy.redact_card_numbers", PRIVACY, "Mask card numbers (only the last 4 digits stay).").confirm(),
    e("privacy.redact_labels", PRIVACY, "Fields whose label contains one of these are masked too.").confirm(),
    e("privacy.style", PRIVACY, "How private areas are hidden in pictures.").choice(&["fill", "pixelate"]).confirm(),
    // Control
    e("control.stop_hotkey", CONTROL, "The emergency stop: stops the agent at once from any app. Empty: no stop key.").kind(Kind::Hotkey).confirm(),
    e("control.pause_on_user_input", CONTROL, "Wait while you use the mouse or keyboard. Only the system idle time is read, never what you type.").confirm(),
    e("control.resume_after_idle_ms", CONTROL, "How long you must be idle before the agent continues.").number("ms").confirm(),
    e("control.max_pause_secs", CONTROL, "After this long the action fails and the agent is told why.").number("seconds").confirm(),
    e("control.settings_hotkey", CONTROL, "Opens this panel from any app. Empty: no key.").kind(Kind::Hotkey).confirm(),
    // Agents
    e("hub.enabled", AGENTS, "Several agents on one desktop share one hub: a cursor each, one stop key, the screen shared out, turns at the keyboard and mouse.").restart(),
    e("hub.port", AGENTS, "The hub's port on this computer.").number("port").restart().advanced(),
    e("hub.arrange", AGENTS, "With two agents or more, move each agent's window into its part of the screen."),
    e("hub.chat", AGENTS, "Let the agents send each other short messages."),
    e("hub.turn_wait_secs", AGENTS, "The longest an agent waits for its turn at the keyboard and mouse.").range(1.0, 600.0, 1.0, "s"),
    // Notifications
    e("notifications.enabled", NOTIFICATIONS, "Let the agent read desktop notifications. They carry other apps' messages and codes."),
    e("notifications.apps", NOTIFICATIONS, "Only notifications from these apps. Empty: any app."),
    e("notifications.mask_codes", NOTIFICATIONS, "Mask one-time and verification codes."),
    e("notifications.keep", NOTIFICATIONS, "How many are kept (Linux).").number("notifications"),
    // Scripts
    e("script.saved_as_tools", SCRIPTS, "Each saved script is also a tool of its own."),
    e("script.files", SCRIPTS, "Files scripts may use: none, workspace (only the scripts folder's files), read (read any file) or all.").choice(&["none", "workspace", "read", "all"]).confirm(),
    e("script.web", SCRIPTS, "Let scripts fetch and download over http(s).").confirm(),
    e("script.max_seconds", SCRIPTS, "The longest run of one script, tool calls included.").range(1.0, 3600.0, 1.0, "s"),
    e("script.dir", SCRIPTS, "Where saved scripts are kept. Empty: the default folder."),
    // Verify
    e("verify.enabled", VERIFY, "Check each action's result and tell the agent when it didn't work."),
    e("verify.retry", VERIFY, "An action that clearly failed is tried once more another way."),
    e("verify.retry_on_no_change", VERIFY, "Also retry presses after which nothing visible changed. Off: some actions (pay, send) show their effect late and must not be repeated."),
    // Audit
    e("audit.enabled", AUDIT, "Write a line of metadata per call to a log file.").confirm(),
    e("audit.path", AUDIT, "The log file. Empty: audit.log in the server's folder.").confirm(),
    // Server
    e("server.log", SERVER, "How much the server logs.").choice(&["off", "error", "warn", "info", "debug", "trace"]).restart(),
    e("server.instructions", SERVER, "The instructions sent when a client connects: full, short (for clients that load the skills) or off.").choice(&["full", "short", "off"]).restart(),
    e("server.result_meta", SERVER, "For hosts that trim their context: results say which earlier results they repeat."),
    e("server.structured_output", SERVER, "Also return some results as data, for clients that use it."),
    e("server.http_addr", SERVER, "Serve MCP over HTTP at this address, for example 127.0.0.1:8787. Empty: stdio only.").confirm().restart(),
    e("server.http_token", SERVER, "The bearer token HTTP clients must send.").kind(Kind::Secret).confirm().restart(),
    // Updates
    e("update.enabled", UPDATES, "Look for updates and download them. Off: it never updates by itself.").confirm(),
    e("update.check_after_mins", UPDATES, "Minutes after the server starts before the first look.").number("minutes").confirm(),
    e("update.check_every_hours", UPDATES, "Hours between looks while the server runs. Used when the minutes below are 0.").number("hours").confirm().advanced(),
    e("update.check_every_mins", UPDATES, "Minutes between looks, for a shorter wait than an hour. At least 5. 0: use the hours.").number("minutes").confirm(),
    e("update.channel", UPDATES, "Stable takes only proper releases. Prerelease also takes releases GitHub marks as pre-releases (their tags are plain numbers, such as v4.9.0).").choice(&["stable", "prerelease"]).confirm(),
    e("update.pin", UPDATES, "A version to stay on, for example 5.0.2: only that release is taken, and nothing newer. Empty: the newest.").confirm(),
    e("update.skip_version", UPDATES, "A version never to take, for example one that went wrong here.").confirm(),
    e("update.install", UPDATES, "When a downloaded update goes in: restart (after the computer restarts), start (the next server start) or manual.").choice(&["restart", "start", "manual"]).confirm(),
    e("update.repo", UPDATES, "The GitHub repository releases come from (owner/name).").confirm().advanced(),
    // Platform
    e("linux.batch_size", PLATFORM, "Elements whose AT-SPI queries run at once.").number("elements").advanced(),
    e("linux.text_max_chars", PLATFORM, "The longest text read from a text element.").number("characters").advanced(),
    e("macos.batch_attributes", PLATFORM, "Read all of an element's attributes in one call.").advanced(),
    e("macos.messaging_timeout_secs", PLATFORM, "The accessibility call timeout.").number("seconds").advanced(),
    e("windows.use_cache_request", PLATFORM, "Fetch a whole window's tree in one UIA call.").advanced(),
    // Panel
    e("panel.theme", PANEL, "Light, dark, or follow the computer.").choice(&["system", "light", "dark"]),
    e("panel.accent", PANEL, "The colour the panel's palette is made from.").kind(Kind::Color),
    e("panel.port", PANEL, "The port the panel listens on while it is open. A free one is used if this one is taken.").number("port").restart().advanced(),
    e("panel.idle_minutes", PANEL, "The panel closes after this long without a request.").range(1.0, 240.0, 1.0, "min"),
];

/// A short line under each group's name.
pub fn blurbs() -> serde_json::Value {
    serde_json::json!({
        "General": "Switches that apply to the whole program.",
        "Pointer": "How the agent's own pointer looks and moves on screen. Your real mouse is not touched.",
        "Real mouse": "How the real mouse moves when an action has to use it.",
        "Overlay": "The border, label and colours shown while the agent works.",
        "Screenshots": "When pictures are sent to the model, and how big. The biggest lever on image tokens.",
        "Accessibility tree": "How much of the app's tree the model reads, and how it is shortened.",
        "Tools and tokens": "Which tools the model sees. The tool list is sent with every request.",
        "Timing": "Pauses and waits around actions.",
        "Screen memory": "Remembering screens the model has already seen.",
        "Text on screen (OCR)": "Reading text off the screen for apps whose tree says little.",
        "Decision model": "A fast model that answers small questions about what is on screen.",
        "Privacy": "What is masked before anything reaches the model. Changes ask you to confirm.",
        "Your control": "The emergency stop and the pause while you use the computer. Changes ask you to confirm.",
        "Several agents": "Subagents, or Claude Code beside Codex, on one desktop.",
        "Notifications": "Reading desktop notifications.",
        "Scripts": "Small programs the agent writes and the server runs.",
        "Checking actions": "Verifying that an action did what it should.",
        "Audit log": "A record of every call, without the data.",
        "Server": "Logging, the instructions sent to clients and the optional HTTP transport.",
        "Updates": "How the program keeps itself up to date. Changes ask you to confirm.",
        "Platform": "Settings for one operating system.",
        "Panel": "This page.",
    })
}

pub fn entry(key: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_an_entry_and_nothing_else_does() {
        let known = crate::config::known_keys();
        for key in &known {
            assert!(
                entry(key).is_some(),
                "`{key}` has no entry in panel/schema.rs"
            );
        }
        for e in ENTRIES {
            assert!(
                known.iter().any(|k| k == e.key),
                "panel/schema.rs lists `{}`, which is not a setting",
                e.key
            );
        }
        let mut keys: Vec<_> = ENTRIES.iter().map(|e| e.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(n, keys.len(), "a key is listed twice");
    }

    #[test]
    fn every_entry_has_a_known_group_and_a_help_line() {
        for e in ENTRIES {
            assert!(GROUPS.contains(&e.group), "{}: group {}", e.key, e.group);
            assert!(e.help.len() > 8, "{}: no help", e.key);
        }
    }

    #[test]
    fn choices_match_what_the_settings_accept() {
        for e in ENTRIES {
            if let Kind::Choice(c) = e.kind {
                let cfg = toml::Table::try_from(crate::config::Config::default()).unwrap();
                for v in c {
                    let mut t = cfg.clone();
                    let mut cur = &mut t;
                    let parts: Vec<_> = e.key.split('.').collect();
                    for p in &parts[..parts.len() - 1] {
                        cur = cur.get_mut(*p).unwrap().as_table_mut().unwrap();
                    }
                    cur.insert(
                        parts[parts.len() - 1].into(),
                        toml::Value::String((*v).into()),
                    );
                    let parsed: Result<crate::config::Config, _> = t.try_into();
                    let cfgv = parsed.unwrap_or_else(|err| panic!("{}={v}: {err}", e.key));
                    cfgv.validate()
                        .unwrap_or_else(|err| panic!("{}={v}: {err}", e.key));
                }
            }
        }
    }
}
