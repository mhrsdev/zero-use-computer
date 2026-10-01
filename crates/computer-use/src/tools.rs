//! The tool surface: the same ten tools Codex's Computer Use plugin exposes
//! (list_apps, get_app_state, click, perform_secondary_action, set_value,
//! select_text, scroll, drag, press_key, type_text) plus launch_app, draw
//! and helpers.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::imaging::EncodedImage;
use crate::types::{MouseButton, ScrollDirection};

/// A tool definition in MCP shape (`name`, `description`, `inputSchema`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    pub annotations: Value,
}

/// What a tool call returns: text for the model and, for get_app_state, a screenshot.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub text: String,
    pub image: Option<EncodedImage>,
    pub is_error: bool,
}

impl ToolOutput {
    /// About how many tokens this result costs the model: its text, plus
    /// an image at roughly width × height / 750.
    pub fn estimated_tokens(&self) -> usize {
        crate::text::estimate_tokens(&self.text)
            + self
                .image
                .as_ref()
                .map_or(0, |i| (i.width as usize * i.height as usize).div_ceil(750))
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image: None,
            is_error: false,
        }
    }

    pub fn error(err: &Error) -> Self {
        Self {
            text: err.to_string(),
            image: None,
            is_error: true,
        }
    }

    /// MCP `CallToolResult.content`.
    pub fn content(&self) -> Value {
        let mut content = vec![json!({"type": "text", "text": self.text})];
        if let Some(img) = &self.image {
            content.push(json!({"type": "image", "data": img.base64(), "mimeType": img.mime}));
        }
        Value::Array(content)
    }

    /// Full MCP `CallToolResult`.
    pub fn to_mcp_result(&self) -> Value {
        json!({"content": self.content(), "isError": self.is_error})
    }
}

// ---------------------------------------------------------------------------
// Arguments

fn de_opt_string<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(other) => {
            return Err(serde::de::Error::custom(format!(
                "expected a string or number, got {other}"
            )));
        }
    })
}

fn de_opt_index<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<u32>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => Some(n.as_u64().and_then(|v| u32::try_from(v).ok()).ok_or_else(
            || serde::de::Error::custom("element_index must be a non-negative integer"),
        )?),
        Some(Value::String(s)) => Some(s.trim().parse().map_err(|_| {
            serde::de::Error::custom("element_index must be a non-negative integer")
        })?),
        Some(other) => {
            return Err(serde::de::Error::custom(format!(
                "element_index must be an integer, got {other}"
            )));
        }
    })
}

fn de_index<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u32, D::Error> {
    de_opt_index(d)?.ok_or_else(|| serde::de::Error::custom("element_index is required"))
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct LaunchAppArgs {
    pub app: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct GetAppStateArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(default, alias = "disableDiff")]
    pub disable_diff: bool,
    /// Force (true) or suppress (false) the screenshot; default follows
    /// `screenshot.attach`.
    #[serde(default, alias = "include_screenshot")]
    pub screenshot: Option<bool>,
    /// Also read the window's text off the screen (OCR), whatever settings say.
    #[serde(default)]
    pub ocr: bool,
    /// Token budget for this tree, instead of `tree.max_tokens`; 0 = the
    /// whole tree, nothing folded or cut.
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ClickArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(default, deserialize_with = "de_opt_index")]
    pub element_index: Option<u32>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    #[serde(default)]
    pub button: MouseButton,
    #[serde(default = "one", alias = "clicks")]
    pub click_count: u8,
}

fn one() -> u8 {
    1
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct SecondaryActionArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(deserialize_with = "de_index")]
    pub element_index: u32,
    pub action: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct SetValueArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(deserialize_with = "de_index")]
    pub element_index: u32,
    #[serde(deserialize_with = "de_value")]
    pub value: String,
}

fn de_value<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => s,
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => {
            return Err(serde::de::Error::custom(format!(
                "value must be a string, number or boolean, got {other}"
            )));
        }
    })
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct SelectTextArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(deserialize_with = "de_index")]
    pub element_index: u32,
    /// Text to select; omitted selects all.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default = "one_usize")]
    pub occurrence: usize,
}

fn one_usize() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ScrollArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(default, deserialize_with = "de_opt_index")]
    pub element_index: Option<u32>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub direction: ScrollDirection,
    /// Pages (viewport heights/widths); fractions allowed.
    #[serde(default = "one_f64", alias = "pages")]
    pub amount: f64,
}

fn one_f64() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DragArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(default, deserialize_with = "de_opt_index")]
    pub from_element_index: Option<u32>,
    pub from_x: Option<f64>,
    pub from_y: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_index")]
    pub to_element_index: Option<u32>,
    pub to_x: Option<f64>,
    pub to_y: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DrawArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    pub strokes: Vec<DrawStroke>,
    /// Coordinates are fractions of this element's box instead of
    /// screenshot pixels.
    #[serde(default, deserialize_with = "de_opt_index")]
    pub element_index: Option<u32>,
    /// Coordinates are in the document's own units: the document's
    /// `size` is shown at `box` (or at `element_index`'s box).
    #[serde(default)]
    pub canvas: Option<DrawCanvas>,
    #[serde(default)]
    pub button: MouseButton,
    /// Show the strokes over a screenshot instead of drawing them.
    #[serde(default)]
    pub preview: bool,
    /// Pointer speed while drawing, in screen pixels per second.
    #[serde(default)]
    pub speed: Option<f64>,
}

/// Where a document is on screen and how big it is in its own units.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DrawCanvas {
    /// `[left, top, right, bottom]` of the document in screenshot pixels.
    #[serde(default, rename = "box")]
    pub area: Option<[f64; 4]>,
    /// `[width, height]` of the document in its own units (pixels, mm…),
    /// y down.
    #[serde(default)]
    pub size: Option<[f64; 2]>,
    /// `[x min, x max, y min, y max]`: math coordinates across the box,
    /// y up.
    #[serde(default)]
    pub range: Option<[f64; 4]>,
}

/// One press-move-release of a `draw`: points, or a parametric curve.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DrawStroke {
    #[serde(default)]
    pub points: Option<Vec<DrawPoint>>,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub smooth: bool,
    #[serde(default, deserialize_with = "de_opt_expr")]
    pub x: Option<String>,
    #[serde(default, deserialize_with = "de_opt_expr")]
    pub y: Option<String>,
    /// `[from, to]`: numbers or expressions such as "2*pi".
    #[serde(default)]
    pub t: Option<[DrawNumber; 2]>,
    #[serde(default)]
    pub steps: Option<u32>,
    /// A rectangle: `[x, y, width, height]`, optionally a corner radius.
    #[serde(default)]
    pub rect: Option<Vec<f64>>,
    /// An ellipse: `[center x, center y, radius x, radius y]`.
    #[serde(default)]
    pub ellipse: Option<[f64; 4]>,
    /// A regular polygon: `[center x, center y, radius, corners]`.
    #[serde(default)]
    pub polygon: Option<[f64; 4]>,
    /// A star: `[center x, center y, outer radius, inner radius, points]`.
    #[serde(default)]
    pub star: Option<[f64; 5]>,
    /// A circular arc: `[center x, center y, radius, from°, to°]`.
    #[serde(default)]
    pub arc: Option<[f64; 5]>,
    /// Cubic Bézier: start, then two controls and an end per segment.
    #[serde(default)]
    pub bezier: Option<Vec<DrawPoint>>,
    /// Axes through 0 with ticks every `[x step, y step]` (math range).
    #[serde(default)]
    pub axes: Option<[f64; 2]>,
    /// Turn the stroke by this many degrees (clockwise on screen).
    #[serde(default)]
    pub rotate: Option<f64>,
    /// The point to turn about (default: the stroke's centre).
    #[serde(default)]
    pub about: Option<DrawPoint>,
    /// Draw the stroke several times, moved and/or turned each time.
    #[serde(default)]
    pub repeat: Option<DrawRepeat>,
    /// Paint the closed shape solid with a round brush this wide (in the
    /// stroke's units) instead of its outline.
    #[serde(default)]
    pub fill: Option<f64>,
    /// A picture traced with trace_image: draw this step of it.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub trace: Option<String>,
    /// A design from the design board: draw this step of it.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub design: Option<String>,
    #[serde(default)]
    pub step: Option<u32>,
}

/// `repeat`: copy k (from 0) is moved by k × offset and turned by k ×
/// rotate about `about`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DrawRepeat {
    pub count: u32,
    #[serde(default)]
    pub offset: Option<[f64; 2]>,
    #[serde(default)]
    pub rotate: Option<f64>,
    #[serde(default)]
    pub about: Option<DrawPoint>,
}

/// A point as `[x, y]` or `{"x": .., "y": ..}`.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum DrawPoint {
    Pair([f64; 2]),
    Named { x: f64, y: f64 },
}

impl DrawPoint {
    pub fn xy(self) -> (f64, f64) {
        match self {
            DrawPoint::Pair([x, y]) | DrawPoint::Named { x, y } => (x, y),
        }
    }
}

/// A number, or an expression that gives one (`"2*pi"`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum DrawNumber {
    Num(f64),
    Expr(String),
}

/// An expression given as a string or a plain number.
fn de_opt_expr<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(other) => {
            return Err(serde::de::Error::custom(format!(
                "x and y must be expressions in t (strings), got {other}"
            )));
        }
    })
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct PressKeyArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(alias = "keys")]
    pub key: String,
    #[serde(default, deserialize_with = "de_opt_index")]
    pub element_index: Option<u32>,
    /// Point the mouse here (screenshot pixels) while pressing.
    pub x: Option<f64>,
    pub y: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct TypeTextArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    pub text: String,
    #[serde(default, deserialize_with = "de_opt_index")]
    pub element_index: Option<u32>,
    /// Point the mouse here (screenshot pixels) while typing.
    pub x: Option<f64>,
    pub y: Option<f64>,
}

/// A state an element can be matched on, for find_element / wait_for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ElementState {
    /// Present in the tree at all (default).
    #[default]
    Present,
    Visible,
    Enabled,
    Focused,
    Checked,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct FindElementArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    /// Normalized role to match (e.g. "button", "text field").
    #[serde(default, deserialize_with = "de_opt_string")]
    pub role: Option<String>,
    /// Substring of the element's name/label (case-insensitive).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub name: Option<String>,
    /// Substring of the element's name or value (case-insensitive).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub text: Option<String>,
    /// Only match editable elements.
    #[serde(default)]
    pub editable: bool,
    #[serde(default = "twenty")]
    pub max_results: usize,
}

fn twenty() -> usize {
    20
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct WaitForArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub role: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub text: Option<String>,
    #[serde(default)]
    pub state: ElementState,
    /// Defaults to `timing.wait_timeout_ms`.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Defaults to `timing.wait_poll_ms`.
    #[serde(default)]
    pub poll_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScreenshotMode {
    /// The whole screen, or only the part that changed since the last
    /// full-screen screenshot (default without `app`).
    Auto,
    /// Always the whole (virtual) screen.
    Full,
    /// A screen-space rectangle (x, y, width, height).
    Region,
    /// A specific app window (default when `app` is given).
    #[default]
    Window,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ScreenshotArgs {
    #[serde(default)]
    pub mode: Option<ScreenshotMode>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub app: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    /// Draw each element's index over the window image (set-of-marks).
    #[serde(default)]
    pub annotate: bool,
    /// Zoom into this element of the window (from get_app_state).
    #[serde(default, deserialize_with = "de_opt_index")]
    pub element_index: Option<u32>,
    /// Draw a labelled coordinate grid, a line every this many units
    /// (0 = a round step that suits the image).
    #[serde(default, deserialize_with = "de_grid")]
    pub grid: Option<f64>,
    /// List the image's main colours.
    #[serde(default)]
    pub palette: bool,
    /// Exact colours at these points.
    #[serde(default)]
    pub pick: Option<Vec<DrawPoint>>,
    /// Label the grid and read `pick` points in a document's own units or
    /// a math range, as `draw` takes them.
    #[serde(default)]
    pub canvas: Option<DrawCanvas>,
    /// Compare the document (`canvas`) with a picture traced by
    /// trace_image: where it differs most.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub compare: Option<String>,
}

/// The design board: compose a picture from layers, see it, then put it
/// into an app.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignArgs {
    pub name: String,
    /// `[width, height]` in the design's units (pixels of the result).
    #[serde(default)]
    pub size: Option<[f64; 2]>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub background: Option<String>,
    /// Space kept clear at the edges (default 5% of the short side).
    #[serde(default)]
    pub margin: Option<f64>,
    #[serde(default)]
    pub add: Option<Vec<DesignLayer>>,
    #[serde(default)]
    pub change: Option<Vec<DesignLayer>>,
    #[serde(default)]
    pub remove: Option<Vec<String>>,
    #[serde(default)]
    pub mirror: Option<Vec<DesignMirror>>,
    #[serde(default)]
    pub align: Option<Vec<DesignAlign>>,
    #[serde(default)]
    pub distribute: Option<Vec<DesignDistribute>>,
    #[serde(default)]
    pub order: Option<Vec<DesignOrder>>,
    /// Extras on the picture: a grid, the layers' names, the guides.
    #[serde(default)]
    pub show: Option<DesignShow>,
    /// Write the design to a temporary file to import into an app.
    #[serde(default)]
    pub export: Option<ExportFormat>,
}

/// A layer: a shape (as `draw` takes it) or a text, with its colours.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignLayer {
    #[serde(default, deserialize_with = "de_opt_string")]
    pub id: Option<String>,
    #[serde(default)]
    pub points: Option<Vec<DrawPoint>>,
    #[serde(default)]
    pub closed: Option<bool>,
    #[serde(default)]
    pub smooth: Option<bool>,
    #[serde(default, deserialize_with = "de_opt_expr")]
    pub x: Option<String>,
    #[serde(default, deserialize_with = "de_opt_expr")]
    pub y: Option<String>,
    #[serde(default)]
    pub t: Option<[DrawNumber; 2]>,
    #[serde(default)]
    pub steps: Option<u32>,
    #[serde(default)]
    pub rect: Option<Vec<f64>>,
    #[serde(default)]
    pub ellipse: Option<[f64; 4]>,
    #[serde(default)]
    pub polygon: Option<[f64; 4]>,
    #[serde(default)]
    pub star: Option<[f64; 5]>,
    #[serde(default)]
    pub arc: Option<[f64; 5]>,
    #[serde(default)]
    pub bezier: Option<Vec<DrawPoint>>,
    #[serde(default)]
    pub rotate: Option<f64>,
    #[serde(default)]
    pub about: Option<DrawPoint>,
    #[serde(default)]
    pub repeat: Option<DrawRepeat>,
    /// A text layer: the text (lines split at "\n").
    #[serde(default, deserialize_with = "de_opt_string")]
    pub text: Option<String>,
    /// Where the text starts: its top, and its left, centre or right
    /// (`align`).
    #[serde(default)]
    pub at: Option<DrawPoint>,
    /// Text height in the design's units.
    #[serde(default)]
    pub size: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub font: Option<String>,
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub align: Option<TextAlign>,
    /// Colours as "#RRGGBB", or "none".
    #[serde(default, deserialize_with = "de_opt_string")]
    pub fill: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub stroke: Option<String>,
    /// Outline width.
    #[serde(default)]
    pub width: Option<f64>,
    /// 0 (clear) to 1.
    #[serde(default)]
    pub opacity: Option<f64>,
    /// Move by [dx, dy].
    #[serde(default, rename = "move")]
    pub shift: Option<[f64; 2]>,
    /// Move so the layer's box starts at [x, y].
    #[serde(default)]
    pub to: Option<[f64; 2]>,
    /// Put the new layer just below / above this one (default: on top).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub below: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub above: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// A mirrored copy of a layer (the other ear, the other eye).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignMirror {
    pub id: String,
    /// The copy's id.
    #[serde(rename = "as")]
    pub copy: String,
    /// "x": left-right (about a vertical line, default); "y": top-bottom.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub axis: Option<String>,
    /// Where the mirror line is (default: the middle of the page).
    #[serde(default)]
    pub line: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignAlign {
    pub ids: Vec<String>,
    /// "left", "center" or "right".
    #[serde(default, deserialize_with = "de_opt_string")]
    pub x: Option<String>,
    /// "top", "middle" or "bottom".
    #[serde(default, deserialize_with = "de_opt_string")]
    pub y: Option<String>,
    /// "page" (default), "margins", "each other", or a layer's id.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub to: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignDistribute {
    pub ids: Vec<String>,
    /// "x" (side by side, default) or "y" (one above another).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub axis: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignOrder {
    pub id: String,
    /// "front", "back", "up" (one step) or "down".
    pub to: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DesignShow {
    #[serde(default, deserialize_with = "de_grid")]
    pub grid: Option<f64>,
    /// Write each layer's id on the picture.
    #[serde(default)]
    pub ids: bool,
    /// Margins, centre lines and thirds.
    #[serde(default)]
    pub guides: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Png,
    Svg,
}

/// Turn a reference picture into a few flat colours and shapes to paint.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TraceImageArgs {
    /// An image file (PNG or JPEG).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub path: Option<String>,
    /// Or what an app's window shows.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub app: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    /// The part of the window: [left, top, right, bottom] in screenshot
    /// pixels.
    #[serde(default, rename = "box")]
    pub area: Option<[f64; 4]>,
    /// Or one element of the window.
    #[serde(default, deserialize_with = "de_opt_index")]
    pub element_index: Option<u32>,
    #[serde(default)]
    pub colors: Option<u32>,
    #[serde(default)]
    pub detail: Option<TraceDetail>,
    /// What to call it in draw and screenshot (default: from the file).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TraceDetail {
    Low,
    #[default]
    Medium,
    High,
}

/// `grid`: a spacing, or `true` for a round step that suits the image.
fn de_grid<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<f64>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(Value::Bool(true)) => Some(0.0),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(v) if v > 0.0 && v.is_finite() => Some(v),
            _ => None,
        },
        Some(other) => {
            return Err(serde::de::Error::custom(format!(
                "grid must be a spacing in pixels or true, got {other}"
            )));
        }
    })
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct BatchStep {
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct BatchArgs {
    /// Applied to any step that does not name its own `app`.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub app: Option<String>,
    pub steps: Vec<BatchStep>,
    /// Continue running after a step fails (default: stop).
    #[serde(default)]
    pub continue_on_error: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct NotificationsArgs {
    /// Only this app's notifications (name substring).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub app: Option<String>,
    /// Most notifications returned (newest).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// What the `window` tool does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowAction {
    /// The screens (and virtual desktops).
    Displays,
    /// The app's windows with their positions and states.
    List,
    Focus,
    /// To x, y (and width, height if given), in screen coordinates.
    Move,
    Resize,
    Maximize,
    Minimize,
    Restore,
    Fullscreen,
    ExitFullscreen,
    Close,
    /// Fill the left/right/top/bottom half of a display, or center it.
    TileLeft,
    TileRight,
    TileTop,
    TileBottom,
    Center,
    MoveToDisplay,
    MoveToDesktop,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct WindowArgs {
    #[serde(default, deserialize_with = "de_opt_string")]
    pub app: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    pub action: WindowAction,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    /// Display index (from action=displays).
    #[serde(default)]
    pub display: Option<u32>,
    /// Virtual desktop index, from 0.
    #[serde(default)]
    pub desktop: Option<u32>,
}

impl WindowAction {
    /// Whether it changes anything.
    pub fn mutating(self) -> bool {
        !matches!(self, WindowAction::Displays | WindowAction::List)
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct SetClipboardArgs {
    pub text: String,
}

/// A parsed tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    ListApps,
    LaunchApp(LaunchAppArgs),
    GetAppState(GetAppStateArgs),
    Click(ClickArgs),
    PerformSecondaryAction(SecondaryActionArgs),
    SetValue(SetValueArgs),
    SelectText(SelectTextArgs),
    Scroll(ScrollArgs),
    Drag(DragArgs),
    Draw(DrawArgs),
    Design(DesignArgs),
    TraceImage(TraceImageArgs),
    PressKey(PressKeyArgs),
    TypeText(TypeTextArgs),
    FindElement(FindElementArgs),
    WaitFor(WaitForArgs),
    Screenshot(ScreenshotArgs),
    Batch(BatchArgs),
    GetClipboard,
    SetClipboard(SetClipboardArgs),
    Window(WindowArgs),
    GetNotifications(NotificationsArgs),
}

fn parse_args<T: for<'de> Deserialize<'de>>(tool: &str, args: Value) -> Result<T> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| Error::InvalidArgs(format!("{tool}: {e}")))
}

impl ToolCall {
    pub fn parse(name: &str, args: Value) -> Result<Self> {
        Ok(match name {
            "list_apps" => ToolCall::ListApps,
            "launch_app" => ToolCall::LaunchApp(parse_args(name, args)?),
            "get_app_state" => ToolCall::GetAppState(parse_args(name, args)?),
            "click" => ToolCall::Click(parse_args(name, args)?),
            "perform_secondary_action" => ToolCall::PerformSecondaryAction(parse_args(name, args)?),
            "set_value" => ToolCall::SetValue(parse_args(name, args)?),
            "select_text" => ToolCall::SelectText(parse_args(name, args)?),
            "scroll" => ToolCall::Scroll(parse_args(name, args)?),
            "drag" => ToolCall::Drag(parse_args(name, args)?),
            "draw" => ToolCall::Draw(parse_args(name, args)?),
            "trace_image" => ToolCall::TraceImage(parse_args(name, args)?),
            "design" => ToolCall::Design(parse_args(name, args)?),
            "press_key" => ToolCall::PressKey(parse_args(name, args)?),
            "type_text" => ToolCall::TypeText(parse_args(name, args)?),
            "find_element" => ToolCall::FindElement(parse_args(name, args)?),
            "wait_for" => ToolCall::WaitFor(parse_args(name, args)?),
            "screenshot" => ToolCall::Screenshot(parse_args(name, args)?),
            "batch" => ToolCall::Batch(parse_args(name, args)?),
            "get_clipboard" => ToolCall::GetClipboard,
            "set_clipboard" => ToolCall::SetClipboard(parse_args(name, args)?),
            "window" => ToolCall::Window(parse_args(name, args)?),
            "get_notifications" => ToolCall::GetNotifications(parse_args(name, args)?),
            other => return Err(Error::UnknownTool(other.to_string())),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            ToolCall::ListApps => "list_apps",
            ToolCall::LaunchApp(_) => "launch_app",
            ToolCall::GetAppState(_) => "get_app_state",
            ToolCall::Click(_) => "click",
            ToolCall::PerformSecondaryAction(_) => "perform_secondary_action",
            ToolCall::SetValue(_) => "set_value",
            ToolCall::SelectText(_) => "select_text",
            ToolCall::Scroll(_) => "scroll",
            ToolCall::Drag(_) => "drag",
            ToolCall::Draw(_) => "draw",
            ToolCall::TraceImage(_) => "trace_image",
            ToolCall::Design(_) => "design",
            ToolCall::PressKey(_) => "press_key",
            ToolCall::TypeText(_) => "type_text",
            ToolCall::FindElement(_) => "find_element",
            ToolCall::WaitFor(_) => "wait_for",
            ToolCall::Screenshot(_) => "screenshot",
            ToolCall::Batch(_) => "batch",
            ToolCall::GetClipboard => "get_clipboard",
            ToolCall::SetClipboard(_) => "set_clipboard",
            ToolCall::Window(_) => "window",
            ToolCall::GetNotifications(_) => "get_notifications",
        }
    }
}

// ---------------------------------------------------------------------------
// Definitions

fn app_props() -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "app".into(),
        json!({"type": "string", "description": "App name, bundle id / executable name, or pid (from list_apps)."}),
    );
    m.insert(
        "window".into(),
        json!({"type": "string", "description": "Window id or title substring. Defaults to the focused/main window, or the window of the latest get_app_state."}),
    );
    m
}

fn schema(mut props: serde_json::Map<String, Value>, extra: Value, required: &[&str]) -> Value {
    if let Value::Object(extra) = extra {
        props.extend(extra);
    }
    let mut req = vec!["app"];
    req.extend_from_slice(required);
    json!({"type": "object", "properties": props, "required": req, "additionalProperties": false})
}

fn index_prop(desc: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": desc})
}

fn coord_prop(axis: &str) -> Value {
    json!({"type": "number", "description": format!("{axis} coordinate in screenshot pixels (from the latest get_app_state image).")})
}

fn canvas_prop() -> Value {
    json!({
        "type": "object",
        "description": "Coordinates of a document: box = [left, top, right, bottom] where it is in the screenshot (or element_index's box), with size = [width, height] in its own units (y down) or range = [x min, x max, y min, y max] (math, y up).",
        "properties": {
            "box": {"type": "array", "items": {"type": "number"}},
            "size": {"type": "array", "items": {"type": "number"}},
            "range": {"type": "array", "items": {"type": "number"}}
        },
        "additionalProperties": false
    })
}

/// The shape of a stroke or design layer, as draw takes it.
fn shape_props() -> serde_json::Map<String, Value> {
    let v = json!({
                                "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "[[x, y], ...]"},
                                "closed": {"type": "boolean"},
                                "smooth": {"type": "boolean"},
                                "x": {"type": "string", "description": "x(t)"},
                                "y": {"type": "string", "description": "y(t)"},
                                "t": {"type": "array", "items": {"type": ["number", "string"]}, "description": "[from, to]: numbers or expressions like \"2*pi\"."},
                                "steps": {"type": "integer", "minimum": 1},
                                "rect": {"type": "array", "items": {"type": "number"}, "description": "[x, y, width, height] or [x, y, width, height, corner radius]"},
                                "ellipse": {"type": "array", "items": {"type": "number"}, "description": "[center x, center y, radius x, radius y]"},
                                "polygon": {"type": "array", "items": {"type": "number"}, "description": "[center x, center y, radius, corners]"},
                                "star": {"type": "array", "items": {"type": "number"}, "description": "[center x, center y, outer radius, inner radius, points]"},
                                "arc": {"type": "array", "items": {"type": "number"}, "description": "[center x, center y, radius, from degrees, to degrees]"},
                                "bezier": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "Cubic: start, then control, control, end per segment."},
                                "axes": {"type": "array", "items": {"type": "number"}, "description": "[x tick step, y tick step]: axes through 0 (needs canvas.range)."},
                                "rotate": {"type": "number", "description": "Degrees, clockwise on screen, about `about` (default: the stroke's centre)."},
                                "about": {"type": "array", "items": {"type": "number"}},
                                "repeat": {
                                    "type": "object",
                                    "description": "Copies: copy k is moved by k*offset and turned by k*rotate degrees about `about`.",
                                    "properties": {
                                        "count": {"type": "integer", "minimum": 1},
                                        "offset": {"type": "array", "items": {"type": "number"}},
                                        "rotate": {"type": "number"},
                                        "about": {"type": "array", "items": {"type": "number"}}
                                    },
                                    "required": ["count"],
                                    "additionalProperties": false
                                }
    });
    match v {
        Value::Object(m) => m,
        _ => unreachable!("an object"),
    }
}

/// A draw stroke: a shape, painted solid or not, or a step of a trace or
/// design.
fn stroke_props() -> Value {
    let mut m = shape_props();
    m.extend(
        match json!({
            "fill": {"type": "number", "description": "Paint the closed shape solid instead of its outline, with a round brush this wide (in the stroke's units; set the app's brush to this size). Shapes painted back to front cover each other."},
            "trace": {"type": "string", "description": "A picture traced with trace_image: draw one step of it (with step and fill), fitted into the canvas or element."},
            "design": {"type": "string", "description": "A design from the design tool: draw one step of it (with step, and fill for a solid step), fitted into the canvas or element."},
            "step": {"type": "integer", "minimum": 1, "description": "Which step of the trace or design (1 = first)."}
        }) {
            Value::Object(e) => e,
            _ => unreachable!("an object"),
        },
    );
    Value::Object(m)
}

/// A design layer: a shape or a text, with colours and placement.
fn layer_props() -> Value {
    let mut m = shape_props();
    m.extend(
        match json!({
            "id": {"type": "string", "description": "The layer's name (\"head\", \"title\"); used by change, align, mirror."},
            "text": {"type": "string", "description": "A text layer (lines split at \\n)."},
            "at": {"type": "array", "items": {"type": "number"}, "description": "Text: [x, y] of its top, and of its left, centre or right (align)."},
            "size": {"type": "number", "description": "Text: font size in the design's units."},
            "font": {"type": "string"},
            "bold": {"type": "boolean"},
            "align": {"type": "string", "enum": ["left", "center", "right"]},
            "fill": {"type": "string", "description": "\"#RRGGBB\" or \"none\" (text colour for text)."},
            "stroke": {"type": "string", "description": "Outline colour, \"#RRGGBB\" or \"none\"."},
            "width": {"type": "number", "description": "Outline width."},
            "opacity": {"type": "number", "minimum": 0, "maximum": 1},
            "move": {"type": "array", "items": {"type": "number"}, "description": "Move by [dx, dy]."},
            "to": {"type": "array", "items": {"type": "number"}, "description": "Move so the layer's box starts at [x, y]."},
            "below": {"type": "string", "description": "Place it just below this layer."},
            "above": {"type": "string", "description": "Place it just above this layer."}
        }) {
            Value::Object(e) => e,
            _ => unreachable!("an object"),
        },
    );
    json!({"type": "object", "properties": Value::Object(m), "additionalProperties": false})
}

fn hover_prop(axis: &str) -> Value {
    json!({"type": "number", "description": format!("{axis} in screenshot pixels: point the mouse there first, for apps that send keys to what is under the pointer (Blender, some CAD apps).")})
}

fn read_only(title: &str) -> Value {
    json!({"title": title, "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false})
}

fn acting(title: &str) -> Value {
    json!({"title": title, "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true})
}

/// Tool definitions, ready to hand to an LLM or list over MCP.
pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_apps",
            title: "List apps",
            description: "List running desktop apps with their ids, pids and window counts. Use it to find the exact app to pass to other tools.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: read_only("List apps"),
        },
        ToolDefinition {
            name: "launch_app",
            title: "Launch app",
            description: "Start (or bring up) a desktop app by its name in the system's app menu (\"Google Chrome\"), a bundle id or an executable, and wait until it shows a window. Then call get_app_state. The name must be exact (an error lists similar ones); arguments and command lines are never accepted.",
            input_schema: json!({
                "type": "object",
                "properties": {"app": {"type": "string", "description": "App name as the app menu shows it, bundle id or executable (no arguments)."}},
                "required": ["app"],
                "additionalProperties": false
            }),
            annotations: acting("Launch app"),
        },
        ToolDefinition {
            name: "get_app_state",
            title: "Get app state",
            description: "Get the current state of an app window: its accessibility tree with numbered elements, plus a screenshot when it adds information (first view of a window, a large change, custom-drawn UI; set screenshot=true to always include one). Call this first on every turn before acting on an app. Element indices are only valid until the next get_app_state. After the first call, the tree may come back as a diff against the previous one; pass disable_diff=true to get the full tree. When the app is back on a screen you already saw (\"screen #N (seen before)\"), only what changed since then is sent, its element indices are the ones you saw then, and a new screenshot comes only if its pixels changed. A very large tree has its long lists folded (find_element finds folded items); max_tokens=0 returns it whole.",
            input_schema: schema(
                app_props(),
                json!({
                    "disable_diff": {"type": "boolean", "description": "Return the full tree instead of a diff.", "default": false},
                    "screenshot": {"type": "boolean", "description": "true = always include a screenshot, false = never (default: decided by settings)."},
                    "ocr": {"type": "boolean", "default": false, "description": "Also read the window's text off the screen (for custom-drawn UI); the lines become clickable \"ocr text\" elements. Done automatically when the tree is nearly empty."},
                    "max_tokens": {"type": "integer", "minimum": 0, "description": "Token budget for this tree (default from settings). 0 = the whole tree, with no list folded or cut; use it only when you really need every element at once."}
                }),
                &[],
            ),
            annotations: read_only("Get app state"),
        },
        ToolDefinition {
            name: "click",
            title: "Click",
            description: "Click an element by element_index (preferred: uses the element's accessibility action and works in the background) or at x/y screenshot coordinates. Use button=right for context menus and click_count=2 for double-click.",
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Element to click, from the latest get_app_state."),
                    "x": coord_prop("X"),
                    "y": coord_prop("Y"),
                    "button": {"type": "string", "enum": ["left", "right", "middle"], "default": "left"},
                    "click_count": {"type": "integer", "minimum": 1, "maximum": 3, "default": 1}
                }),
                &[],
            ),
            annotations: acting("Click"),
        },
        ToolDefinition {
            name: "perform_secondary_action",
            title: "Perform secondary action",
            description: "Perform a named accessibility action on an element, other than a plain click: one of the actions listed for that element in get_app_state (e.g. show_menu, increment, decrement, confirm, cancel, raise, expand, collapse, toggle, pick).",
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Target element, from the latest get_app_state."),
                    "action": {"type": "string", "description": "Action name as listed in actions=[...] for the element."}
                }),
                &["element_index", "action"],
            ),
            annotations: acting("Perform secondary action"),
        },
        ToolDefinition {
            name: "set_value",
            title: "Set value",
            description: "Set an element's value directly: replace a text field's contents, move a slider, or set a checkbox/switch (\"true\"/\"false\"). Prefer this over typing when the element is marked editable or settable.",
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Target element, from the latest get_app_state."),
                    "value": {"type": "string", "description": "New value."}
                }),
                &["element_index", "value"],
            ),
            annotations: acting("Set value"),
        },
        ToolDefinition {
            name: "select_text",
            title: "Select text",
            description: "Select text inside a text element: the given substring (its nth occurrence), or all text if `text` is omitted. Follow with type_text to replace it or press_key to act on it.",
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Text element, from the latest get_app_state."),
                    "text": {"type": "string", "description": "Exact text to select. Omit to select everything."},
                    "occurrence": {"type": "integer", "minimum": 1, "default": 1, "description": "Which match to select when the text appears several times."}
                }),
                &["element_index"],
            ),
            annotations: acting("Select text"),
        },
        ToolDefinition {
            name: "scroll",
            title: "Scroll",
            description: "Scroll the content of an element (a scroll area, list, web area…) or the area under x/y screenshot coordinates. amount is in pages (viewport sizes); fractions allowed.",
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Element to scroll (or one inside the area to scroll)."),
                    "x": coord_prop("X"),
                    "y": coord_prop("Y"),
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                    "amount": {"type": "number", "exclusiveMinimum": 0, "default": 1, "description": "Pages to scroll."}
                }),
                &["direction"],
            ),
            annotations: acting("Scroll"),
        },
        ToolDefinition {
            name: "drag",
            title: "Drag",
            description: "Drag from one element or point to another (move items, resize, reorder, select ranges). Each end is either an element index or x/y screenshot coordinates.",
            input_schema: schema(
                app_props(),
                json!({
                    "from_element_index": index_prop("Drag start element."),
                    "from_x": coord_prop("Start X"),
                    "from_y": coord_prop("Start Y"),
                    "to_element_index": index_prop("Drop target element."),
                    "to_x": coord_prop("End X"),
                    "to_y": coord_prop("End Y")
                }),
                &[],
            ),
            annotations: acting("Drag"),
        },
        ToolDefinition {
            name: "draw",
            title: "Draw",
            description: "Draw with the mouse: for each stroke, press the button, move along the stroke and release (a pen or brush in a paint app, a signature field, shapes or function plots on a canvas). A stroke is a shape (rect [x,y,w,h(,radius)], ellipse [cx,cy,rx,ry], polygon [cx,cy,r,n], star [cx,cy,R,r,n], arc [cx,cy,r,from°,to°], bezier [[x,y],...]), points [[x,y],...] (straight lines; closed=true returns to the first point, smooth=true draws a smooth curve through them), a parametric curve (x and y are expressions in t: + - * / ^ %, sin cos tan sqrt abs exp ln log10 min max floor round, pi, e... from t[0] to t[1], default 0 to 1; steps=n draws n straight pieces) or a function plot (only y, in x: {\"y\": \"sin(x)\"}). Any stroke can be turned (rotate, about) and repeated (repeat: count, offset, rotate, about). Coordinates are screenshot pixels like click's x/y; with element_index, fractions of that element's box; with canvas, a document's own units (box + size) or math coordinates with y up (box + range, where axes [xstep, ystep] draws axes and ticks). fill=w paints a closed shape solid with a brush w wide instead of its outline (no bucket needed; later shapes cover earlier ones). trace + step draws one colour step of a picture traced with trace_image. preview=true shows the strokes over a screenshot without drawing. Parts of a curve outside the area are not drawn. Closed outlines come back with where a bucket click fills each (one click per piece when other lines cut it, or where a fill would leak out through a gap). The stop key ends a drawing midway.",
            input_schema: schema(
                app_props(),
                json!({
                    "strokes": {
                        "type": "array",
                        "minItems": 1,
                        "description": "One press-move-release each.",
                        "items": {
                            "type": "object",
                            "properties": stroke_props(),
                            "additionalProperties": false
                        }
                    },
                    "element_index": index_prop("Draw inside this element; coordinates are fractions of its box."),
                    "canvas": canvas_prop(),
                    "preview": {"type": "boolean", "description": "Show the strokes in red over a screenshot, with a grid in the same coordinates; nothing is drawn."},
                    "button": {"type": "string", "enum": ["left", "right", "middle"]},
                    "speed": {"type": "number", "minimum": 50, "description": "Pointer speed in pixels per second (default 800)."}
                }),
                &["strokes"],
            ),
            annotations: acting("Draw"),
        },
        ToolDefinition {
            name: "trace_image",
            title: "Trace a picture",
            description: "Turn a reference picture (an image file, or what a window shows) into a few flat colours and shapes to paint back to front, for copying it in a paint app. Returns the steps (one colour each, in painting order) and a picture of the result. Then, per step: set the app's colour to the step's hex and draw with strokes=[{trace: name, step: n, fill: brush width}] and the canvas. screenshot with compare=name shows where the canvas still differs.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "An image file (PNG or JPEG) the user gave."},
                    "app": {"type": "string", "description": "Or trace what this app's window shows."},
                    "window": {"type": "string", "description": "Window id or title substring."},
                    "box": {"type": "array", "items": {"type": "number"}, "description": "The part of the window: [left, top, right, bottom] in screenshot pixels."},
                    "element_index": index_prop("Or this element of the window."),
                    "colors": {"type": "integer", "minimum": 2, "maximum": 16, "description": "How many flat colours (default 8)."},
                    "detail": {"type": "string", "enum": ["low", "medium", "high"], "description": "How small a shape is kept (default medium; high for faces and small features)."},
                    "name": {"type": "string", "description": "What draw and screenshot call it (default: from the file or app)."}
                },
                "additionalProperties": false
            }),
            annotations: read_only("Trace a picture"),
        },
        ToolDefinition {
            name: "design",
            title: "Design board",
            description: "A design board, like Canva: compose a picture from layers (shapes as draw takes them, and text) and see it rendered before anything is drawn in an app. Each call can add, change, remove, mirror (a symmetric copy: the other eye or ear), align, distribute and reorder layers; it returns the picture, the layers with their boxes and colours, checks (off the page, almost centred, pairs not quite symmetric, hard-to-read text) and the steps to paint it. Then put it into an app: export=\"svg\" or \"png\" (a temporary file to import), the layers' numbers for the app's fields, or draw with strokes=[{design, step, fill}]. Coordinates are the design's units (pixels of the result), y down. Start with name and size; later calls with the same name change it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "size": {"type": "array", "items": {"type": "number"}, "description": "[width, height]: starts the design (or resizes it)."},
                    "background": {"type": "string", "description": "\"#RRGGBB\"."},
                    "margin": {"type": "number", "description": "Space to keep clear at the edges (default 5% of the short side)."},
                    "add": {"type": "array", "items": layer_props(), "description": "New layers, on top (or below/above a layer)."},
                    "change": {"type": "array", "items": layer_props(), "description": "Layers to change, by id: new colours, text, place (move, to) or a new shape."},
                    "remove": {"type": "array", "items": {"type": "string"}},
                    "mirror": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "as": {"type": "string"}, "axis": {"type": "string", "enum": ["x", "y"]}, "line": {"type": "number"}}, "required": ["id", "as"], "additionalProperties": false}, "description": "A mirrored copy: x = left-right about the page's middle (or line)."},
                    "align": {"type": "array", "items": {"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "string"}}, "x": {"type": "string", "enum": ["left", "center", "right"]}, "y": {"type": "string", "enum": ["top", "middle", "bottom"]}, "to": {"type": "string", "description": "page (default), margins, each other, or a layer id."}}, "required": ["ids"], "additionalProperties": false}},
                    "distribute": {"type": "array", "items": {"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "string"}}, "axis": {"type": "string", "enum": ["x", "y"]}}, "required": ["ids"], "additionalProperties": false}, "description": "Equal gaps between 3 or more layers."},
                    "order": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "to": {"type": "string", "enum": ["front", "back", "up", "down"]}}, "required": ["id", "to"], "additionalProperties": false}},
                    "show": {"type": "object", "properties": {"grid": {"type": ["number", "boolean"]}, "ids": {"type": "boolean"}, "guides": {"type": "boolean"}}, "additionalProperties": false, "description": "On the picture: a grid in the design's units, the layers' ids, guides (margins, centre, thirds)."},
                    "export": {"type": "string", "enum": ["png", "svg"], "description": "Write a temporary file to import into an app."}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            annotations: read_only("Design board"),
        },
        ToolDefinition {
            name: "press_key",
            title: "Press key",
            description: "Press a key or shortcut in the app, e.g. \"Return\", \"Escape\", \"Tab\", \"cmd+s\", \"ctrl+shift+t\", \"alt+Left\". \"cmd\" is Cmd on a Mac and Ctrl elsewhere; \"win\"/\"super\" is the Windows/Super key. Several space-separated combos are pressed in order (\"Down Down Return\"). Optionally focus element_index first.",
            input_schema: schema(
                app_props(),
                json!({
                    "key": {"type": "string", "description": "Key combo(s): modifiers (cmd/ctrl/alt/option/shift/meta) joined with + and a key name."},
                    "element_index": index_prop("Element to focus before pressing."),
                    "x": hover_prop("X"),
                    "y": hover_prop("Y")
                }),
                &["key"],
            ),
            annotations: acting("Press key"),
        },
        ToolDefinition {
            name: "type_text",
            title: "Type text",
            description: "Type text into the app's focused element, as keyboard input. Optionally focus element_index first. For replacing a field's whole contents prefer set_value.",
            input_schema: schema(
                app_props(),
                json!({
                    "text": {"type": "string", "description": "Text to type. Newlines press Return."},
                    "element_index": index_prop("Element to focus before typing."),
                    "x": hover_prop("X"),
                    "y": hover_prop("Y")
                }),
                &["text"],
            ),
            annotations: acting("Type text"),
        },
        ToolDefinition {
            name: "find_element",
            title: "Find element",
            description: "Search the app's current accessibility tree for elements matching a role and/or a name/text substring, and return them with their element_index. Cheaper than reading the whole tree. Refreshes state, so the returned indices are current.",
            input_schema: schema(
                app_props(),
                json!({
                    "role": {"type": "string", "description": "Normalized role to match, e.g. button, text field, checkbox, link."},
                    "name": {"type": "string", "description": "Case-insensitive substring of the element's name/label."},
                    "text": {"type": "string", "description": "Case-insensitive substring of the element's name or value."},
                    "editable": {"type": "boolean", "default": false, "description": "Only editable elements."},
                    "max_results": {"type": "integer", "minimum": 1, "default": 20}
                }),
                &[],
            ),
            annotations: read_only("Find element"),
        },
        ToolDefinition {
            name: "wait_for",
            title: "Wait for element",
            description: "Poll the app until an element matching the given role/name/text (and optional state) appears, then return it. Use after actions that take time (loading, dialogs). Fails when the timeout elapses.",
            input_schema: schema(
                app_props(),
                json!({
                    "role": {"type": "string", "description": "Normalized role to match."},
                    "name": {"type": "string", "description": "Case-insensitive substring of the name/label."},
                    "text": {"type": "string", "description": "Case-insensitive substring of name or value."},
                    "state": {"type": "string", "enum": ["present", "visible", "enabled", "focused", "checked"], "default": "present"},
                    "timeout_ms": {"type": "integer", "minimum": 1, "description": "Give up after this long (default from settings)."},
                    "poll_ms": {"type": "integer", "minimum": 1, "description": "Re-check interval (default from settings)."}
                }),
                &[],
            ),
            annotations: read_only("Wait for element"),
        },
        ToolDefinition {
            name: "screenshot",
            title: "Screenshot",
            description: "Capture an image: the screen (mode=auto, the default without app: the whole screen, or only the part that changed since your last full-screen screenshot; mode=full: always all of it), a screen rectangle (mode=region with x/y/width/height), or an app window (mode=window with app). With element_index, zoom into that element of the window (to read small text). With annotate=true on a window, each element's index is drawn over it (set-of-marks). For exact positions and colours: grid=N draws a labelled grid every N units (true: a round step) in the x/y that click and draw use for that window (screen coordinates for full/region); with canvas (as draw takes it) the grid covers just the document, labelled in its units or math range. palette=true lists the main colours, pick=[[x,y],...] gives the exact colour at each point (same coordinates as the grid). compare=name (with canvas) compares the document with a picture traced by trace_image and lists where it differs most.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "mode": {"type": "string", "enum": ["auto", "full", "region", "window"], "description": "What to capture. Defaults to window when app is given, else auto."},
                    "app": {"type": "string", "description": "App for mode=window."},
                    "window": {"type": "string", "description": "Window id or title substring for mode=window."},
                    "x": {"type": "number", "description": "Region left (screen pixels)."},
                    "y": {"type": "number", "description": "Region top (screen pixels)."},
                    "width": {"type": "number", "description": "Region width."},
                    "height": {"type": "number", "description": "Region height."},
                    "annotate": {"type": "boolean", "default": false, "description": "Draw element indices over a window capture."},
                    "element_index": index_prop("Zoom into this element of the window (from get_app_state)."),
                    "grid": {"type": ["number", "boolean"], "description": "Labelled coordinate grid: a line every N units (true = a round step)."},
                    "palette": {"type": "boolean", "description": "List the image's main colours (hex, share)."},
                    "pick": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "[[x, y], ...]: the exact colour at each point."},
                    "canvas": canvas_prop(),
                    "compare": {"type": "string", "description": "A trace_image name: compare the canvas with it."}
                },
                "additionalProperties": false
            }),
            annotations: read_only("Screenshot"),
        },
        ToolDefinition {
            name: "batch",
            title: "Batch actions",
            description: "Run several computer-use tools in order in one call (e.g. fill a form then submit). Each step is {\"tool\": name, \"arguments\": {...}}. Stops at the first failure unless continue_on_error is true. Steps missing `app` inherit the top-level app.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": {"type": "string", "description": "Default app for steps that omit it."},
                    "continue_on_error": {"type": "boolean", "default": false},
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": {"type": "string"},
                                "arguments": {"type": "object"}
                            },
                            "required": ["tool"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["steps"],
                "additionalProperties": false
            }),
            annotations: acting("Batch actions"),
        },
        ToolDefinition {
            name: "window",
            title: "Manage windows",
            description: "Arrange app windows and see the screens. action: displays (the screens and virtual desktops; no app needed), list (the app's windows with position, size and state), focus, move (x, y and optionally width, height), resize (width, height), maximize, minimize, restore, fullscreen, exit_fullscreen, close, tile_left / tile_right / tile_top / tile_bottom (half of a display), center, move_to_display (display), move_to_desktop (desktop). Positions and sizes are screen coordinates (as in get_app_state's window line), not screenshot pixels.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": {"type": "string", "description": "App name, id or pid (not needed for displays)."},
                    "window": {"type": "string", "description": "Window id or title substring (default: the app's current window)."},
                    "action": {"type": "string", "enum": ["displays", "list", "focus", "move", "resize", "maximize", "minimize", "restore", "fullscreen", "exit_fullscreen", "close", "tile_left", "tile_right", "tile_top", "tile_bottom", "center", "move_to_display", "move_to_desktop"]},
                    "x": {"type": "number", "description": "Left edge, screen coordinates."},
                    "y": {"type": "number", "description": "Top edge, screen coordinates."},
                    "width": {"type": "number"},
                    "height": {"type": "number"},
                    "display": {"type": "integer", "minimum": 0, "description": "Display index from action=displays (default: the window's own)."},
                    "desktop": {"type": "integer", "minimum": 0, "description": "Virtual desktop, from 0."}
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            annotations: acting("Manage windows"),
        },
        ToolDefinition {
            name: "get_notifications",
            title: "Read notifications",
            description: "Read the user's recent desktop notifications (app, title, text, how long ago), newest last. Codes and card numbers in them are masked, and notifications from apps the user blocked are left out.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": {"type": "string", "description": "Only this app's notifications (name substring)."},
                    "limit": {"type": "integer", "minimum": 1, "default": 10, "description": "Most notifications to return (the newest)."}
                },
                "additionalProperties": false
            }),
            annotations: read_only("Read notifications"),
        },
        ToolDefinition {
            name: "get_clipboard",
            title: "Get clipboard",
            description: "Read the system clipboard as text.",
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: read_only("Get clipboard"),
        },
        ToolDefinition {
            name: "set_clipboard",
            title: "Set clipboard",
            description: "Write text to the system clipboard (e.g. to paste it into an app with press_key cmd+v / ctrl+v).",
            input_schema: json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "additionalProperties": false
            }),
            annotations: acting("Set clipboard"),
        },
    ]
}

/// One-line descriptions used in `compact` mode.
fn short_description(name: &str) -> Option<&'static str> {
    Some(match name {
        "list_apps" => "List running apps (name, id, pid).",
        "launch_app" => "Start an app by name/id and wait for its window.",
        "get_app_state" => {
            "The app window's numbered accessibility tree (+ a screenshot when useful). Call first each turn; element indices are valid until the next call; later calls return a diff. A screen \"seen before\" keeps the indices you saw then. screenshot=true forces an image; max_tokens=0 returns a huge tree whole, unfolded."
        }
        "click" => {
            "Click element_index (preferred) or x,y in screenshot pixels. button right/middle, click_count 2 = double."
        }
        "perform_secondary_action" => {
            "Run one of an element's listed actions=[...] (not a plain click)."
        }
        "set_value" => "Set a field's text, a slider, or a checkbox (\"true\"/\"false\") directly.",
        "select_text" => "Select the given text (or all text) in a text element.",
        "scroll" => "Scroll an element or the area at x,y; amount is in pages.",
        "drag" => "Drag from an element/point to another element/point.",
        "draw" => {
            "Draw with the mouse held down along strokes: rect [x,y,w,h(,r)], ellipse [cx,cy,rx,ry], polygon [cx,cy,r,n], star [cx,cy,R,r,n], arc [cx,cy,r,a0,a1], bezier, points (closed, smooth), a curve x,y in t over t=[from,to] (steps=n), or a plot {y: \"sin(x)\"}; rotate/about, repeat {count, offset, rotate, about}. Screenshot pixels; element_index fractions; canvas {box:[l,t,r,b], size:[w,h]} document units or {box, range:[x0,x1,y0,y1]} math (y up; axes [dx,dy]). fill=w paints a closed shape solid with a w-wide brush; trace+step draws a step of a trace_image picture. preview=true only shows them. Returns where a bucket click fills each closed outline."
        }
        "press_key" => {
            "Press keys or shortcuts, e.g. \"cmd+s\", \"Down Down Return\". x,y points the mouse there first (Blender sends keys to what is under it)."
        }
        "type_text" => {
            "Type text into the focused element (element_index focuses first; x,y points the mouse there first)."
        }
        "find_element" => "Find elements by role/name/text; returns their indices.",
        "wait_for" => "Wait until an element matching role/name/text (and state) appears.",
        "design" => {
            "Design board (like Canva): build a picture from layers (draw shapes, text) with add/change/remove/mirror/align/distribute/order; returns the picture, layers, checks and paint steps; export svg/png (temporary) or draw {design, step, fill}. Plan every drawing here first."
        }
        "trace_image" => {
            "Turn a reference picture (path, or app [+box]) into flat colour steps to paint back to front; then draw {trace, step, fill} per step, after setting the step's colour."
        }
        "screenshot" => {
            "Image of the screen (auto: only what changed since the last one), a region (x,y,width,height), an app window, or one element (element_index zooms in; annotate=true draws indices). grid=N (or true): labelled grid in the x/y click and draw use, or in canvas units with canvas; palette=true: main colours; pick=[[x,y]]: exact colours; compare=trace name (with canvas): where the canvas differs."
        }
        "batch" => "Run several tools in order: steps=[{tool, arguments}].",
        "window" => {
            "Windows and screens: action displays|list|focus|move|resize|maximize|minimize|restore|fullscreen|exit_fullscreen|close|tile_left|tile_right|tile_top|tile_bottom|center|move_to_display|move_to_desktop; x/y/width/height in screen coordinates."
        }
        "get_notifications" => "Recent desktop notifications (app, title, text); filter by app.",
        "get_clipboard" => "Read the clipboard text.",
        "set_clipboard" => "Write text to the clipboard.",
        _ => return None,
    })
}

/// Remove per-property prose and validation-only keywords from a JSON schema
/// (types, enums, defaults and `required` stay).
fn strip_descriptions(v: &mut Value) {
    const DROP: [&str; 7] = [
        "description",
        "additionalProperties",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "minItems",
        "title",
    ];
    match v {
        Value::Object(map) => {
            map.remove("additionalProperties");
            if let Some(Value::Object(props)) = map.get_mut("properties") {
                for prop in props.values_mut() {
                    if let Value::Object(p) = prop {
                        for k in DROP {
                            p.remove(k);
                        }
                    }
                    strip_descriptions(prop);
                }
            }
            if let Some(items) = map.get_mut("items") {
                strip_descriptions(items);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(strip_descriptions),
        _ => {}
    }
}

/// Size of what a model actually receives for these tools (name,
/// description and input schema), in JSON characters.
pub fn model_visible_len(defs: &[ToolDefinition]) -> usize {
    defs.iter()
        .map(|d| {
            serde_json::to_string(&json!({
                "name": d.name,
                "description": d.description,
                "input_schema": d.input_schema,
            }))
            .map(|s| s.len())
            .unwrap_or(0)
        })
        .sum()
}

/// Tool definitions for a full config: `[tools]` filtering and styling, plus
/// tools hidden because their feature is switched off (clipboard, screenshots).
pub fn definitions_from(config: &crate::config::Config) -> Vec<ToolDefinition> {
    let screenshots = config.screenshot.enabled && !config.text_only;
    definitions_for(&config.tools)
        .into_iter()
        .filter(|d| match d.name {
            "get_clipboard" | "set_clipboard" => config.clipboard,
            "get_notifications" => config.notifications.enabled,
            "screenshot" => screenshots,
            _ => true,
        })
        .collect()
}

/// Tool definitions filtered and styled by the user's `[tools]` settings.
pub fn definitions_for(cfg: &crate::config::ToolsConfig) -> Vec<ToolDefinition> {
    let compact = cfg.descriptions == crate::config::DescriptionStyle::Compact;
    definitions()
        .into_iter()
        .filter(|d| cfg.is_enabled(d.name))
        .map(|mut d| {
            if compact {
                if let Some(short) = short_description(d.name) {
                    d.description = short;
                }
                strip_descriptions(&mut d.input_schema);
            }
            d
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tools_have_object_schemas() {
        let defs = definitions();
        assert_eq!(defs.len(), 22);
        for d in &defs {
            assert_eq!(d.input_schema["type"], "object", "{}", d.name);
            // Every required property is declared.
            if let Some(req) = d.input_schema["required"].as_array() {
                for r in req {
                    let r = r.as_str().unwrap();
                    assert!(
                        d.input_schema["properties"].get(r).is_some(),
                        "{}: {r}",
                        d.name
                    );
                }
            }
            // Every definition parses a call.
            assert!(!matches!(
                ToolCall::parse(d.name, json!({})),
                Err(Error::UnknownTool(_))
            ));
        }
    }

    #[test]
    fn declared_properties_are_accepted() {
        // Each declared property name must round-trip through the parser.
        let full: Value = serde_json::from_str(
            r#"{
            "app": "X", "window": "1", "element_index": 1, "x": 1.0, "y": 2.0,
            "button": "right", "click_count": 2, "value": "v",
            "text": "t", "occurrence": 1, "direction": "down", "amount": 0.5,
            "from_element_index": 1, "from_x": 1, "from_y": 1, "to_element_index": 2,
            "to_x": 2, "to_y": 2, "key": "Return", "disable_diff": true,
            "role": "button", "editable": true, "max_results": 5, "state": "visible",
            "timeout_ms": 1000, "poll_ms": 100, "mode": "full", "width": 10, "height": 10,
            "annotate": true, "continue_on_error": false, "tool": "list_apps", "screenshot": true,
            "action": "move", "display": 0, "desktop": 1, "ocr": true, "limit": 5,
            "steps": [{"tool": "list_apps"}], "speed": 300, "grid": 50, "palette": true,
            "preview": false,
            "pick": [[1, 2]], "canvas": {"box": [0, 0, 10, 10], "size": [100, 100]},
            "strokes": [{"points": [[1, 2], {"x": 3, "y": 4}], "closed": true, "smooth": true},
                        {"x": "t", "y": 5, "t": [0, "2*pi"], "steps": 6},
                        {"rect": [0, 0, 5, 5], "fill": 3},
                        {"design": "d", "step": 1, "fill": 3}],
            "name": "n", "path": "p.png", "colors": 4, "detail": "low", "box": [0, 0, 5, 5],
            "compare": "t", "size": [100, 50], "background": "fff", "margin": 4,
            "add": [{"id": "a", "ellipse": [5, 5, 2, 2], "fill": "123456", "stroke": "none",
                     "width": 1, "opacity": 0.5, "move": [1, 1], "below": "b"},
                    {"text": "hi", "at": [1, 1], "size": 9, "font": "Arial", "bold": true,
                     "align": "center", "to": [0, 0]}],
            "change": [{"id": "a", "rotate": 10}], "remove": ["c"],
            "mirror": [{"id": "a", "as": "a2", "axis": "x", "line": 50}],
            "align": [{"ids": ["a"], "x": "center", "y": "middle", "to": "page"}],
            "distribute": [{"ids": ["a", "b", "c"], "axis": "y"}],
            "order": [{"id": "a", "to": "front"}],
            "show": {"grid": true, "ids": true, "guides": true}, "export": "svg"
            }"#,
        )
        .unwrap();
        for d in definitions() {
            let mut args = serde_json::Map::new();
            for (k, _) in d.input_schema["properties"].as_object().unwrap() {
                args.insert(k.clone(), full[k].clone());
            }
            ToolCall::parse(d.name, Value::Object(args))
                .unwrap_or_else(|e| panic!("{}: {e}", d.name));
        }
    }

    #[test]
    fn compact_definitions_are_smaller_and_filterable() {
        use crate::config::{DescriptionStyle, ToolsConfig};
        let full = model_visible_len(&definitions());
        let compact_cfg = ToolsConfig::default();
        assert_eq!(compact_cfg.descriptions, DescriptionStyle::Compact);
        let compact = definitions_for(&compact_cfg);
        assert_eq!(compact.len(), 22);
        let compact_len = model_visible_len(&compact);
        assert!(
            compact_len * 2 < full,
            "compact {compact_len} vs full {full}"
        );
        // Every tool has a short description, and schemas keep their types.
        for d in &compact {
            assert!(short_description(d.name).is_some(), "{}", d.name);
            assert_eq!(d.input_schema["type"], "object");
        }
        let cfg = ToolsConfig {
            disabled: vec!["drag".into(), "batch".into()],
            ..Default::default()
        };
        let names: Vec<_> = definitions_for(&cfg).iter().map(|d| d.name).collect();
        assert!(!names.contains(&"drag") && !names.contains(&"batch"));
        let cfg = ToolsConfig {
            enabled: vec!["list_apps".into(), "get_app_state".into()],
            ..Default::default()
        };
        assert_eq!(definitions_for(&cfg).len(), 2);
    }

    #[test]
    fn lenient_argument_types() {
        let c = ToolCall::parse(
            "get_app_state",
            json!({"app": "TextEdit", "window": 42, "disableDiff": true}),
        )
        .unwrap();
        assert_eq!(
            c,
            ToolCall::GetAppState(GetAppStateArgs {
                app: "TextEdit".into(),
                window: Some("42".into()),
                disable_diff: true,
                screenshot: None,
                ocr: false,
                max_tokens: None,
            })
        );
        let c = ToolCall::parse(
            "set_value",
            json!({"app": "a", "element_index": "7", "value": true}),
        )
        .unwrap();
        let ToolCall::SetValue(v) = c else { panic!() };
        assert_eq!((v.element_index, v.value.as_str()), (7, "true"));
        assert!(ToolCall::parse("click", json!({"app": "a", "element_index": -1})).is_err());
        assert!(matches!(
            ToolCall::parse("nope", json!({})),
            Err(Error::UnknownTool(_))
        ));
    }
}
