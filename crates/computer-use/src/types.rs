use serde::{Deserialize, Serialize};

/// A point in global screen coordinates (points on macOS, physical pixels on
/// Windows and X11). The same space as [`Rect`] bounds reported by backends.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.width > 0.0 && self.height > 0.0)
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// A running application with at least one window (or a regular UI app).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    /// Human-readable name ("TextEdit", "Notepad", "gedit").
    pub name: String,
    /// Stable identifier used for approvals: bundle id on macOS, executable
    /// name on Windows, executable/application name on Linux.
    pub id: String,
    pub pid: u32,
    /// Full executable path when known.
    pub exe: Option<String>,
    pub frontmost: bool,
    pub hidden: bool,
}

impl AppInfo {
    /// Names this app can be matched by (lowercased).
    pub fn match_keys(&self) -> Vec<String> {
        let mut keys = vec![self.name.to_lowercase(), self.id.to_lowercase()];
        if let Some(exe) = &self.exe {
            let file = exe.rsplit(['/', '\\']).next().unwrap_or(exe).to_lowercase();
            if let Some(stem) = file.strip_suffix(".exe") {
                keys.push(stem.to_string());
            }
            keys.push(file);
        }
        keys.dedup();
        keys
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Platform window id (CGWindowID, HWND, X11 window) or a backend-assigned id.
    pub id: u64,
    pub title: String,
    pub bounds: Option<Rect>,
    pub focused: bool,
    pub main: bool,
    pub minimized: bool,
    /// Opaque backend handle for the window's accessibility element.
    pub handle: ElementHandle,
}

/// Opaque handle to a UI element, meaningful only to the backend that
/// produced it and only until that backend's next snapshot of the same app.
pub type ElementHandle = u64;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeStates {
    pub enabled: bool,
    pub focused: bool,
    pub selected: bool,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    /// Text can be edited (text fields, text areas).
    pub editable: bool,
    /// The element's value can be set directly (set_value will work).
    pub value_settable: bool,
    /// The platform reports the element as not currently shown.
    pub hidden: bool,
}

/// An accessibility action the element supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionDesc {
    /// Normalized name shown to the model ("press", "show_menu", "increment").
    pub name: String,
    /// Native name ("AXPress", "click", "Invoke").
    pub native: String,
}

impl ActionDesc {
    pub fn new(name: impl Into<String>, native: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            native: native.into(),
        }
    }
}

/// One element of the raw accessibility tree, as produced by a backend.
/// Nodes are returned flat in pre-order; `parent` indexes into the same vec.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawNode {
    pub handle: ElementHandle,
    pub parent: Option<usize>,
    /// Stable identity across snapshots when the platform provides one.
    pub key: Option<String>,
    /// Normalized role ("button", "text field", "window").
    pub role: String,
    /// Native role ("AXButton", "push button", "Button").
    pub native_role: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub placeholder: Option<String>,
    pub identifier: Option<String>,
    /// Bounds in global screen coordinates.
    pub bounds: Option<Rect>,
    pub actions: Vec<ActionDesc>,
    pub states: NodeStates,
}

/// Limits a backend should respect while walking the tree.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotOptions {
    pub max_nodes: usize,
    pub max_depth: usize,
}

/// A captured window image.
#[derive(Debug, Clone)]
pub struct Capture {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8.
    pub rgba: Vec<u8>,
    /// The screen-space rectangle the image covers.
    pub bounds: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirection {
    /// (dx, dy) unit vector; positive dy scrolls content down (reveals content below).
    pub fn unit(self) -> (i32, i32) {
        match self {
            ScrollDirection::Up => (0, -1),
            ScrollDirection::Down => (0, 1),
            ScrollDirection::Left => (-1, 0),
            ScrollDirection::Right => (1, 0),
        }
    }
}

/// Where input events go: the app process and, when known, the window.
#[derive(Debug, Clone, Copy)]
pub struct InputTarget {
    pub pid: u32,
    pub window_id: Option<u64>,
    pub window_handle: Option<ElementHandle>,
}

/// A screen (monitor), in screen coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Display {
    pub index: u32,
    pub bounds: Rect,
    /// The part not taken by task bars, docks and menu bars.
    pub work_area: Rect,
    pub primary: bool,
}

/// A change to a top-level window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowOp {
    /// Bring it to the front and give it focus.
    Focus,
    /// Move and/or resize (screen coordinates).
    SetBounds(Rect),
    Maximize,
    Minimize,
    /// Un-minimize / un-maximize / leave full screen.
    Restore,
    Fullscreen(bool),
    /// Ask it to close (as its close button does; the app may ask first).
    Close,
    /// Move it to virtual desktop n (0-based).
    ToDesktop(u32),
}

/// Status of an OS-level permission the backend needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionStatus {
    pub name: String,
    pub granted: bool,
    pub detail: String,
}
