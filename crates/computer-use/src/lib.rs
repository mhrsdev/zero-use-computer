//! # computer-use
//!
//! Codex-style computer use as a reusable library. It gives an agent the same
//! desktop-control surface OpenAI's Codex app exposes — the ten tools
//! `list_apps`, `get_app_state`, `click`, `perform_secondary_action`,
//! `set_value`, `select_text`, `scroll`, `drag`, `press_key`, `type_text`
//! (plus `launch_app`) — built on each platform's native accessibility API
//! plus screenshots, with per-app approvals.
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
//! * **Approvals.** Each app is approved before it is controlled (once,
//!   for the session, or always), and terminals, credential/security prompts
//!   and the agent's own host app can never be controlled.
//!
//! ## Embedding
//!
//! ```no_run
//! use computer_use::{Engine, AllowApprover, tools};
//! # fn main() -> computer_use::Result<()> {
//! let mut engine = computer_use::platform_engine()?;
//! // Hand `tools::definitions()` to your model, then route its tool calls:
//! let out = engine.call_tool("list_apps", serde_json::json!({}), &mut AllowApprover);
//! println!("{}", out.text);
//! # let _ = tools::definitions();
//! # Ok(())
//! # }
//! ```
//!
//! Or run it as an MCP stdio server (see the `computer-use-mcp` crate) so any
//! MCP-capable agent — Codex, Claude Code, or your own — can use it.

pub mod backend;
pub mod config;
pub mod engine;
pub mod error;
pub mod imaging;
pub mod keys;
pub mod mock;
pub mod policy;
pub mod roles;
pub mod tools;
pub mod tree;
pub mod types;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub use backend::Backend;
pub use config::{Config, ConfigStore};
pub use engine::{
    AllowApprover, ApprovalDecision, ApprovalRequest, Approver, DenyApprover, Engine,
};
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
    fn launch_app(&mut self, query: &str) -> Result<()> {
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
