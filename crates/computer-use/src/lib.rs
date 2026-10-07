//! # computer-use
//!
//! Codex-style computer use as a reusable library. It gives an agent the same
//! desktop-control surface OpenAI's Codex app exposes — the ten tools
//! `list_apps`, `get_app_state`, `click`, `perform_secondary_action`,
//! `set_value`, `select_text`, `scroll`, `drag`, `press_key`, `type_text`
//! (plus `launch_app`) — built on each platform's native accessibility API
//! plus screenshots.
//!
//! The design mirrors Codex's:
//!
//! * **Accessibility-first.** `get_app_state` returns a pruned, numbered
//!   accessibility tree *and* a screenshot of the window. Actions target
//!   `element_index` values from that tree and go through the platform's AX
//!   actions, so they work on background/occluded windows. `x`/`y` screenshot
//!   coordinates are the fallback.
//! * **Turn-scoped indices with diffs.** Element indices are only valid until
//!   the next `get_app_state`; subsequent calls return a diff unless
//!   `disable_diff` is set.
//! * **Screen memory.** Screens the model has seen are remembered compactly;
//!   when the app returns to one, its old element indices come back and only
//!   what changed since is sent — no fresh tree or screenshot to re-analyse
//!   (see [`screens`]). Unchanged screenshots are never sent twice, and recent
//!   reads are reused until the next action (`[cache]` settings).
//! * **On-screen indicator.** A separate helper process ([`overlay`]) shows
//!   the agent's own cursor, a glow and a status label in state colours;
//!   it is click-through, left out of screenshots, and gone the moment the
//!   work ends or the process dies. The real mouse is never taken.
//! * **Scripts.** The `script` tool runs small programs (Rhai, sandboxed)
//!   for loops over tools, maths, data and graph-paper pages; saved ones
//!   become tools of their own (see [`script`]).
//! * **No access control.** The engine does not decide which apps or actions
//!   are allowed; that is up to the agent (see the security skill in
//!   `skills/computer-use-security`) or the embedding host.
//!
//! ## Embedding
//!
//! ```no_run
//! use computer_use::{Engine, tools};
//! # fn main() -> computer_use::Result<()> {
//! let mut engine = computer_use::platform_engine()?;
//! // Hand `tools::definitions()` to your model, then route its tool calls:
//! let out = engine.call_tool("list_apps", serde_json::json!({}));
//! println!("{}", out.text);
//! # let _ = tools::definitions();
//! # Ok(())
//! # }
//! ```
//!
//! Or run it as an MCP stdio server (see the `computer-use-mcp` crate) so any
//! MCP-capable agent — Codex, Claude Code, or your own — can use it.

pub mod apps_log;
pub mod backend;
pub mod cells;
pub mod config;
pub mod connect;
pub mod coverage;
pub mod decision;
pub mod design;
pub mod draw;
pub mod engine;
pub mod error;
pub mod imaging;
pub mod keys;
pub mod launch;
pub mod mock;
pub mod motion;
pub mod ocr;
pub mod overlay;
pub mod paint;
pub mod panel;
pub mod privacy;
pub mod roles;
pub mod scene;
pub mod screens;
pub mod script;
pub mod target;
pub mod text;
pub mod tools;
pub mod tree;
pub mod types;
pub mod update;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub use backend::Backend;
pub use config::{Config, ConfigStore};
pub use engine::Engine;
pub use error::{Error, Result};
pub use tools::{ToolCall, ToolDefinition, ToolOutput};

/// The compiled-in platform name.
pub const PLATFORM: &str = if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(target_os = "windows") {
    "windows"
} else if cfg!(target_os = "linux") {
    "linux"
} else {
    "unsupported"
};

/// Build the native backend for this platform.
pub fn platform_backend() -> Result<Box<dyn Backend>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::MacBackend::new()?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::WindowsBackend::new()?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxBackend::new()?))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err(Error::Unsupported(format!(
            "no computer-use backend for this platform ({})",
            std::env::consts::OS
        )))
    }
}

/// Build an [`Engine`] on the native backend with config loaded from disk.
pub fn platform_engine() -> Result<Engine<Box<dyn Backend>>> {
    let store = ConfigStore::load(None)?;
    Ok(Engine::new(platform_backend()?, store))
}

impl Backend for Box<dyn Backend> {
    fn name(&self) -> &'static str {
        (**self).name()
    }
    fn configure(&mut self, config: &Config) {
        (**self).configure(config)
    }
    fn permissions(&mut self) -> Vec<types::PermissionStatus> {
        (**self).permissions()
    }
    fn list_apps(&mut self) -> Result<Vec<types::AppInfo>> {
        (**self).list_apps()
    }
    fn launch_app(&mut self, query: &str) -> Result<Option<String>> {
        (**self).launch_app(query)
    }
    fn list_windows(&mut self, app: &types::AppInfo) -> Result<Vec<types::WindowInfo>> {
        (**self).list_windows(app)
    }
    fn snapshot(
        &mut self,
        app: &types::AppInfo,
        window: &types::WindowInfo,
        opts: &types::SnapshotOptions,
    ) -> Result<Vec<types::RawNode>> {
        (**self).snapshot(app, window, opts)
    }
    fn capture(
        &mut self,
        app: &types::AppInfo,
        window: &types::WindowInfo,
    ) -> Result<types::Capture> {
        (**self).capture(app, window)
    }
    fn capture_screen(&mut self, region: Option<types::Rect>) -> Result<types::Capture> {
        (**self).capture_screen(region)
    }
    fn input_needs_front(&self) -> bool {
        (**self).input_needs_front()
    }
    fn session_note(&mut self) -> Option<String> {
        (**self).session_note()
    }
    fn user_idle(&mut self) -> Option<std::time::Duration> {
        (**self).user_idle()
    }
    fn ocr(&mut self, cap: &types::Capture, languages: &[String]) -> Result<Vec<types::OcrLine>> {
        (**self).ocr(cap, languages)
    }
    fn notifications(&mut self) -> Result<Vec<types::Notification>> {
        (**self).notifications()
    }
    fn displays(&mut self) -> Result<Vec<types::Display>> {
        (**self).displays()
    }
    fn desktops(&mut self) -> Option<(u32, u32)> {
        (**self).desktops()
    }
    fn window_op(
        &mut self,
        app: &types::AppInfo,
        window: &types::WindowInfo,
        op: &types::WindowOp,
    ) -> Result<()> {
        (**self).window_op(app, window, op)
    }
    fn clipboard_get(&mut self) -> Result<String> {
        (**self).clipboard_get()
    }
    fn clipboard_set(&mut self, text: &str) -> Result<()> {
        (**self).clipboard_set(text)
    }
    fn perform_action(&mut self, element: types::ElementHandle, native_action: &str) -> Result<()> {
        (**self).perform_action(element, native_action)
    }
    fn set_value(&mut self, element: types::ElementHandle, value: &str) -> Result<()> {
        (**self).set_value(element, value)
    }
    fn select_text(
        &mut self,
        element: types::ElementHandle,
        text: Option<&str>,
        occurrence: usize,
    ) -> Result<()> {
        (**self).select_text(element, text, occurrence)
    }
    fn focus(&mut self, element: types::ElementHandle) -> Result<backend::Native> {
        (**self).focus(element)
    }
    fn scroll_element(
        &mut self,
        element: types::ElementHandle,
        direction: types::ScrollDirection,
        pages: f64,
    ) -> Result<backend::Native> {
        (**self).scroll_element(element, direction, pages)
    }
    fn click(
        &mut self,
        target: &types::InputTarget,
        at: types::Point,
        button: types::MouseButton,
        count: u8,
    ) -> Result<()> {
        (**self).click(target, at, button, count)
    }
    fn drag(
        &mut self,
        target: &types::InputTarget,
        from: types::Point,
        to: types::Point,
    ) -> Result<()> {
        (**self).drag(target, from, to)
    }
    fn move_pointer(
        &mut self,
        target: &types::InputTarget,
        at: types::Point,
    ) -> Result<Option<types::Point>> {
        (**self).move_pointer(target, at)
    }
    fn draw(
        &mut self,
        target: &types::InputTarget,
        strokes: &[Vec<types::Point>],
        button: types::MouseButton,
        pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        (**self).draw(target, strokes, button, pace)
    }
    fn scroll_wheel(
        &mut self,
        target: &types::InputTarget,
        at: types::Point,
        dx: i32,
        dy: i32,
    ) -> Result<()> {
        (**self).scroll_wheel(target, at, dx, dy)
    }
    fn press_key(&mut self, target: &types::InputTarget, combo: &keys::KeyCombo) -> Result<()> {
        (**self).press_key(target, combo)
    }
    fn type_text(&mut self, target: &types::InputTarget, text: &str) -> Result<()> {
        (**self).type_text(target, text)
    }
}

#[cfg(test)]
mod tests {
    /// The names of the `fn`s between `start` and the `}` that closes it.
    fn fns_in(source: &str, start: &str) -> std::collections::BTreeSet<String> {
        // (A Windows checkout may have CRLF line ends.)
        let source = source.replace("\r\n", "\n");
        let body = &source[source.find(start).expect(start)..];
        let body = &body[..body.find("\n}\n").expect("end of block")];
        body.split("fn ")
            .skip(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_boxed_backend_passes_every_method_on() {
        let declared = fns_in(include_str!("backend.rs"), "pub trait Backend");
        let forwarded = fns_in(include_str!("lib.rs"), "impl Backend for Box<dyn Backend>");
        let missing: Vec<_> = declared.difference(&forwarded).collect();
        assert!(missing.is_empty(), "not passed on: {missing:?}");
    }
}
