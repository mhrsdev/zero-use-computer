//! The platform abstraction. Each OS implements [`Backend`] on top of its
//! native accessibility API (AX on macOS, UI Automation on Windows, AT-SPI on
//! Linux) plus its input and window-capture APIs.
//!
//! Backends stay small and mechanical. Pruning, indexing, diffing,
//! coordinate mapping and fallbacks live in the engine so every platform
//! behaves the same way.

use crate::error::Result;
use crate::keys::KeyCombo;
use crate::types::{
    AppInfo, Capture, Display, ElementHandle, InputTarget, MouseButton, Notification, OcrLine,
    PermissionStatus, Point, RawNode, Rect, ScrollDirection, SnapshotOptions, WindowInfo, WindowOp,
};

/// Start a launched app or helper program cut off from this process's
/// stdin/stdout/stderr: in the MCP server those carry the JSON-RPC stream,
/// which a child must neither read nor write to. The child is reaped in the
/// background when it exits.
pub fn spawn_detached(mut cmd: std::process::Command) -> std::io::Result<()> {
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::Builder::new()
        .name("reap-child".into())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}

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

    /// Apply user settings (called on start and whenever the config reloads).
    fn configure(&mut self, _config: &crate::config::Config) {}

    /// OS permissions this backend depends on (Accessibility, Screen Recording…).
    fn permissions(&mut self) -> Vec<PermissionStatus>;

    /// Running GUI apps.
    fn list_apps(&mut self) -> Result<Vec<AppInfo>>;

    /// Start an app by name, bundle id or executable, with no arguments.
    /// Returns once the launch request has been issued; the engine waits for
    /// the app to appear. When the name was looked up in the OS's list of
    /// installed apps (see [`crate::launch`]), returns the program that was
    /// started, so the engine can recognise the app by it.
    fn launch_app(&mut self, query: &str) -> Result<Option<String>>;

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

    /// Read the text in a capture with the OS's own OCR, as lines in screen
    /// coordinates. `languages` are BCP-47 / ISO codes ("en", "de"); empty
    /// means the user's languages. Unsupported where the OS has none (the
    /// engine then uses Tesseract).
    fn ocr(&mut self, _cap: &Capture, _languages: &[String]) -> Result<Vec<OcrLine>> {
        Err(crate::error::Error::Unsupported(
            "no built-in OCR on this platform".into(),
        ))
    }

    /// Recent desktop notifications, oldest first (only while
    /// `[notifications]` is enabled; see each platform for what it can see).
    fn notifications(&mut self) -> Result<Vec<Notification>> {
        Err(crate::error::Error::Unsupported(
            "reading notifications is not available on this platform".into(),
        ))
    }

    /// The screens (monitors).
    fn displays(&mut self) -> Result<Vec<Display>> {
        Err(crate::error::Error::Unsupported(
            "listing displays is not implemented on this platform".into(),
        ))
    }

    /// Virtual desktops as (count, current index), where the OS exposes them.
    fn desktops(&mut self) -> Option<(u32, u32)> {
        None
    }

    /// Move, resize, maximize, minimize, focus or close a top-level window.
    fn window_op(&mut self, _app: &AppInfo, _window: &WindowInfo, op: &WindowOp) -> Result<()> {
        Err(crate::error::Error::Unsupported(format!(
            "{op:?} is not available for windows on this platform"
        )))
    }

    /// Whether synthesized keyboard and mouse input goes to whichever window
    /// is in front (X11 XTest, Windows `SendInput`) rather than to the app it
    /// is meant for (macOS posts events to the app's process). When it does,
    /// the engine brings the app to the front first, and sends nothing if it
    /// can't.
    fn input_needs_front(&self) -> bool {
        true
    }

    /// How long ago anyone last used the mouse or keyboard: the system's
    /// idle time (synthesized input may count too; the engine allows for
    /// its own). Never what the input was. `None` if unknown.
    fn user_idle(&mut self) -> Option<std::time::Duration> {
        None
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

    /// Point the mouse at a screen point without clicking, for apps that
    /// send keys to what is under the pointer (Blender). Returns where the
    /// pointer was when it should be put back afterwards
    /// (`restore_pointer`); the engine then calls this again with it.
    fn move_pointer(&mut self, _target: &InputTarget, _at: Point) -> Result<Option<Point>> {
        Err(crate::error::Error::Unsupported(
            "moving the pointer is not implemented on this platform".into(),
        ))
    }

    /// Synthesized drawing (`draw`): for each stroke, press `button` at its
    /// first screen point, move through the others with it held, release
    /// at the last. `pace` is called with the distance of each move before
    /// making it (it waits to keep the speed, and fails when the user
    /// stopped the agent): on any error the button is released before
    /// returning it, never left down.
    fn draw(
        &mut self,
        _target: &InputTarget,
        _strokes: &[Vec<Point>],
        _button: MouseButton,
        _pace: &mut dyn FnMut(f64) -> Result<()>,
    ) -> Result<()> {
        Err(crate::error::Error::Unsupported(
            "drawing with the mouse is not implemented on this platform".into(),
        ))
    }

    /// Synthesized scroll-wheel input at a screen point, in wheel lines.
    fn scroll_wheel(&mut self, target: &InputTarget, at: Point, dx: i32, dy: i32) -> Result<()>;

    /// Synthesized key press (with modifiers) delivered to the target app.
    fn press_key(&mut self, target: &InputTarget, combo: &KeyCombo) -> Result<()>;

    /// Synthesized text input delivered to the target app's focused element.
    fn type_text(&mut self, target: &InputTarget, text: &str) -> Result<()>;
}
