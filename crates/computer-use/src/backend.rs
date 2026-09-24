//! The platform abstraction. Each OS implements [`Backend`] on top of its
//! native accessibility API (AX on macOS, UI Automation on Windows, AT-SPI on
//! Linux) plus its input and window-capture APIs.
//!
//! Backends stay small and mechanical. Pruning, indexing, diffing, approvals,
//! coordinate mapping and fallbacks live in the engine so every platform
//! behaves the same way.

use crate::error::Result;
use crate::keys::KeyCombo;
use crate::types::{
    AppInfo, Capture, ElementHandle, InputTarget, MouseButton, PermissionStatus, Point, RawNode,
    Rect, ScrollDirection, SnapshotOptions, WindowInfo,
};

/// Result of an attempt to handle an element-level operation natively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Native {
    /// Done; the string describes how (for the tool result).
    Done(String),
    /// The element does not support this natively; the engine falls back to
    /// synthesized mouse or keyboard input.
    Unsupported,
}

pub trait Backend {
    /// Short platform name ("macos", "windows", "linux", "mock").
    fn name(&self) -> &'static str;

    /// OS permissions this backend depends on (Accessibility, Screen Recording…).
    fn permissions(&mut self) -> Vec<PermissionStatus>;

    /// Running GUI apps.
    fn list_apps(&mut self) -> Result<Vec<AppInfo>>;

    /// Start an app by name, bundle id or executable. Returns once the launch
    /// request has been issued; the engine waits for the app to appear.
    fn launch_app(&mut self, query: &str) -> Result<()>;

    /// Top-level windows of an app, most relevant first is not required.
    fn list_windows(&mut self, app: &AppInfo) -> Result<Vec<WindowInfo>>;

    /// Walk the accessibility tree of one window. Returns nodes in pre-order,
    /// the window itself first. Handles stay valid until the next snapshot of
    /// the same app. Backends may also append the app's menu bar.
    fn snapshot(
        &mut self,
        app: &AppInfo,
        window: &WindowInfo,
        opts: &SnapshotOptions,
    ) -> Result<Vec<RawNode>>;

    /// Capture the window's pixels (works for background windows where the
    /// platform allows it).
    fn capture(&mut self, app: &AppInfo, window: &WindowInfo) -> Result<Capture>;

    /// Capture the whole (virtual) screen, or a screen-space rectangle of it.
    /// Not tied to any app window.
    fn capture_screen(&mut self, _region: Option<Rect>) -> Result<Capture> {
        Err(crate::error::Error::Unsupported(
            "screen capture is not implemented on this platform".into(),
        ))
    }

    /// Read the system clipboard as text.
    fn clipboard_get(&mut self) -> Result<String> {
        Err(crate::error::Error::Unsupported(
            "clipboard access is not implemented on this platform".into(),
        ))
    }

    /// Write text to the system clipboard.
    fn clipboard_set(&mut self, _text: &str) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "clipboard access is not implemented on this platform".into(),
        ))
    }

    /// Invoke a named accessibility action (native name) on an element.
    fn perform_action(&mut self, element: ElementHandle, native_action: &str) -> Result<()>;

    /// Set the element's value (text, number, toggle state).
    fn set_value(&mut self, element: ElementHandle, value: &str) -> Result<()>;

    /// Select `text` inside a text element (the `occurrence`-th match, 1-based;
    /// `None` selects everything).
    fn select_text(
        &mut self,
        element: ElementHandle,
        text: Option<&str>,
        occurrence: usize,
    ) -> Result<()>;

    /// Give keyboard focus to an element.
    fn focus(&mut self, element: ElementHandle) -> Result<Native>;

    /// Scroll the element's content semantically (scroll bars, scroll patterns).
    fn scroll_element(
        &mut self,
        element: ElementHandle,
        direction: ScrollDirection,
        pages: f64,
    ) -> Result<Native>;

    /// Synthesized mouse click at a screen point.
    fn click(
        &mut self,
        target: &InputTarget,
        at: Point,
        button: MouseButton,
        count: u8,
    ) -> Result<()>;

    /// Synthesized mouse drag between two screen points.
    fn drag(&mut self, target: &InputTarget, from: Point, to: Point) -> Result<()>;

    /// Synthesized scroll-wheel input at a screen point, in wheel lines.
    fn scroll_wheel(&mut self, target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()>;

    /// Synthesized key press (with modifiers) delivered to the target app.
    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()>;

    /// Synthesized text input delivered to the target app's focused element.
    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()>;
}
