//! The tool surface: the same ten tools Codex's Computer Use plugin exposes
//! (list_apps, get_app_state, click, perform_secondary_action, set_value,
//! select_text, scroll, drag, press_key, type_text) plus launch_app, draw
//! and helpers.

use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::imaging::EncodedImage;
use crate::types::{MouseButton, ScrollDirection};

/// A tool definition in MCP shape (`name`, `description`, `inputSchema`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: Cow<'static, str>,
    pub title: Cow<'static, str>,
    pub description: Cow<'static, str>,
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
    /// The whole tree, not a diff (`rebase`: everything sent again).
    #[serde(default, alias = "disableDiff", alias = "rebase")]
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
    /// Only this element and what is in it (a look that doesn't change
    /// what later diffs are against).
    #[serde(default, deserialize_with = "de_opt_index")]
    pub within: Option<u32>,
    /// Only the parts of the window that have to do with this (the other
    /// parts folded, each to one line): words matched, or the decision
    /// model's judgment when one is set up.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub about: Option<String>,
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
    /// Move x/y to the nearest "corner", "edge", "center" (of the small
    /// shape there) or colour ("#RRGGBB") first.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub snap: Option<String>,
    /// How far to look for it, in screenshot pixels (default 10).
    #[serde(default)]
    pub snap_radius: Option<f64>,
    /// The element to click by its name (and role), when no index or
    /// point is given: it must match one element.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub role: Option<String>,
    /// What should follow, checked after the action ([`Expect`]).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub expect: Option<String>,
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
    #[serde(default, deserialize_with = "de_opt_string")]
    pub expect: Option<String>,
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
    #[serde(default, deserialize_with = "de_opt_string")]
    pub expect: Option<String>,
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
    /// Snap the x/y ends, as for click.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub snap: Option<String>,
    #[serde(default)]
    pub snap_radius: Option<f64>,
}

/// Find exact places in a window: areas of a colour, look-alikes of a
/// part of it, or the nearest corner, edge or centre to a point.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocateArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    /// Where to look: [left, top, right, bottom] in screenshot pixels
    /// (default: the whole window).
    #[serde(default, rename = "box")]
    pub area: Option<[f64; 4]>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub color: Option<String>,
    /// Largest difference per channel still counted as the colour.
    #[serde(default)]
    pub tolerance: Option<f64>,
    /// [left, top, right, bottom] of something to find again elsewhere.
    #[serde(default)]
    pub like: Option<[f64; 4]>,
    #[serde(default)]
    pub near: Option<DrawPoint>,
    /// What to find near the point: "corner", "edge" or "center".
    #[serde(default, deserialize_with = "de_opt_string")]
    pub feature: Option<String>,
    #[serde(default)]
    pub radius: Option<f64>,
    /// Send the window with the places found numbered (default: settings).
    #[serde(default)]
    pub picture: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DrawArgs {
    pub app: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    /// Objects, or lines ("rect 10 10 50 30", [`stroke_line`]).
    #[serde(deserialize_with = "de_strokes")]
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
    /// The named cells' size in the canvas's units (default: sized to the
    /// drawing).
    #[serde(default)]
    pub cell_size: Option<f64>,
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
    #[serde(default, deserialize_with = "de_opt_string")]
    pub expect: Option<String>,
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
    #[serde(default, deserialize_with = "de_opt_string")]
    pub expect: Option<String>,
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
    /// Skip this many matches (the next page of results).
    #[serde(default)]
    pub offset: Option<usize>,
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
    /// A yes/no question about the window, asked of the decision model each
    /// time: wait until the answer is yes.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub until: Option<String>,
}

/// `decide`: typed questions for the decision model.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DecideArgs {
    /// One question (yes/no, or with options or a scale).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub question: Option<String>,
    /// The options of a choice: ["a", "b"] or {"a": "what it means"}.
    #[serde(default)]
    pub options: Option<Value>,
    /// The levels of a score, low to high.
    #[serde(default)]
    pub scale: Option<Value>,
    /// Several questions at once: {"name": {"type", "question", ...}}.
    #[serde(default)]
    pub questions: Option<Value>,
    /// What to judge: text, or any JSON.
    #[serde(default)]
    pub state: Option<Value>,
    /// Judge each of these separately (in parallel).
    #[serde(default)]
    pub items: Option<Vec<Value>>,
    /// Judge this app's window (its elements, as text).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub app: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub window: Option<String>,
    /// Which element of the app's window this describes: its index.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub pick: Option<String>,
    /// With pick: also read the element's text or value (whole), so the
    /// window needn't be read to get it.
    #[serde(default)]
    pub read: bool,
    /// "status", "open" (the settings page), "test", "remove", or the
    /// settings: {provider, base_url, model, api_key}.
    #[serde(default)]
    pub setup: Option<Value>,
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
    /// The named cells over the canvas (graph paper).
    #[serde(default)]
    pub cells: bool,
    /// Just this cell of the canvas, magnified ("C4").
    #[serde(default, deserialize_with = "de_opt_string")]
    pub cell: Option<String>,
    /// The cells' size in the canvas's units (default: about 8 across).
    #[serde(default)]
    pub cell_size: Option<f64>,
    /// Magnify around this point (screenshot pixels of a window), to aim.
    #[serde(default)]
    pub zoom: Option<DrawPoint>,
    /// How far around it, in screen pixels (default 12).
    #[serde(default)]
    pub radius: Option<f64>,
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
    /// The cells' size in the design's units (0: about eight across).
    #[serde(default)]
    pub cell_size: Option<f64>,
    /// Layers as objects, or as lines ("sun ellipse 80 20 12 12 fill
    /// #ffcc00"; [`layer_line`]).
    #[serde(default, deserialize_with = "de_layers")]
    pub add: Option<Vec<DesignLayer>>,
    #[serde(default, deserialize_with = "de_layers")]
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
    /// The named cells (on unless false).
    #[serde(default)]
    pub cells: Option<bool>,
    /// Show just this cell, magnified ("C4").
    #[serde(default, deserialize_with = "de_opt_string")]
    pub cell: Option<String>,
    /// List the steps to paint it ([tools] design_steps = "asked").
    #[serde(default)]
    pub steps: bool,
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

/// A 3D model planned as solids before it is built in an app.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SceneArgs {
    pub name: String,
    /// Objects as objects, or as lines ("seat box 0.5 0.5 0.05 at 0 0
    /// 0.45 color #884422"; [`object_line`]).
    #[serde(default, deserialize_with = "de_objects")]
    pub add: Option<Vec<SceneObject>>,
    #[serde(default, deserialize_with = "de_objects")]
    pub change: Option<Vec<SceneObject>>,
    #[serde(default)]
    pub remove: Option<Vec<String>>,
    #[serde(default)]
    pub mirror: Option<Vec<SceneMirror>>,
    #[serde(default)]
    pub repeat: Option<Vec<SceneRepeat>>,
    /// The ground at z 0 (default on): what floats or sinks is reported.
    #[serde(default)]
    pub ground: Option<bool>,
    #[serde(default)]
    pub view: Option<SceneView>,
    /// The perspective camera: [turn, tilt] in degrees (turn 0 looks from
    /// the front, 90 from the right; tilt 0 is level, 90 from above).
    #[serde(default)]
    pub look: Option<[f64; 2]>,
    /// The objects' ids on the picture (default on).
    #[serde(default)]
    pub ids: Option<bool>,
    #[serde(default)]
    pub export: Option<SceneExport>,
}

/// A solid: what it is, its size, where its centre is, how it is turned.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SceneObject {
    #[serde(default, deserialize_with = "de_opt_string")]
    pub id: Option<String>,
    #[serde(default)]
    pub shape: Option<SceneShape>,
    /// box [x, y, z]; cylinder and cone [diameter, height]; sphere
    /// [diameter]; torus [outer diameter, thickness]; plane [x, y]. Three
    /// numbers give each axis its own size.
    #[serde(default)]
    pub size: Option<Vec<f64>>,
    /// Its centre [x, y, z].
    #[serde(default)]
    pub at: Option<[f64; 3]>,
    /// Degrees about x, then y, then z (as Blender's XYZ rotation).
    #[serde(default)]
    pub rotate: Option<[f64; 3]>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub color: Option<String>,
    /// Set its height so it rests on this object's top, or on "ground".
    #[serde(default, deserialize_with = "de_opt_string")]
    pub on: Option<String>,
    /// Move by [dx, dy, dz].
    #[serde(default, rename = "move")]
    pub shift: Option<[f64; 3]>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SceneShape {
    #[serde(alias = "cube")]
    Box,
    Cylinder,
    #[serde(alias = "ball")]
    Sphere,
    Cone,
    #[serde(alias = "ring")]
    Torus,
    Plane,
}

/// A mirrored copy of an object (the other leg, the other wing).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SceneMirror {
    pub id: String,
    #[serde(rename = "as")]
    pub copy: String,
    /// The axis that flips: "x" (left-right, default), "y" or "z".
    #[serde(default, deserialize_with = "de_opt_string")]
    pub axis: Option<String>,
    /// Where the mirror is on that axis (default 0).
    #[serde(default)]
    pub at: Option<f64>,
}

/// Copies of an object in a row, or around a vertical axis.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SceneRepeat {
    pub id: String,
    /// How many there are in all, the original first.
    pub count: u32,
    /// Each copy this much further [dx, dy, dz].
    #[serde(default)]
    pub offset: Option<[f64; 3]>,
    /// Or turned around a vertical axis through [x, y].
    #[serde(default)]
    pub around: Option<[f64; 2]>,
    /// The whole turn the copies spread over (default 360).
    #[serde(default)]
    pub angle: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SceneView {
    /// Front, right, top and perspective on one picture.
    #[default]
    All,
    Front,
    Right,
    Top,
    Perspective,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SceneExport {
    Obj,
    Png,
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

/// What a word after a field's name is.
#[derive(Clone, Copy, PartialEq)]
enum Field {
    /// Words, joined.
    Text,
    /// One number.
    Num,
    /// Numbers, as a list.
    Nums,
    /// Present (true), or followed by true/false.
    Flag,
    /// `line #RRGGBB [width]`: the outline's colour and width.
    Outline,
}

const LAYER_FIELDS: &[(&str, Field)] = &[
    ("id", Field::Text),
    ("rect", Field::Nums),
    ("ellipse", Field::Nums),
    ("polygon", Field::Nums),
    ("star", Field::Nums),
    ("arc", Field::Nums),
    ("rotate", Field::Num),
    ("about", Field::Nums),
    ("text", Field::Text),
    ("at", Field::Nums),
    ("size", Field::Num),
    ("font", Field::Text),
    ("bold", Field::Flag),
    ("align", Field::Text),
    ("fill", Field::Text),
    ("stroke", Field::Text),
    ("line", Field::Outline),
    ("width", Field::Num),
    ("opacity", Field::Num),
    ("move", Field::Nums),
    ("to", Field::Nums),
    ("below", Field::Text),
    ("above", Field::Text),
    ("closed", Field::Flag),
    ("smooth", Field::Flag),
];

const OBJECT_FIELDS: &[(&str, Field)] = &[
    ("id", Field::Text),
    ("shape", Field::Text),
    ("size", Field::Nums),
    ("at", Field::Nums),
    ("rotate", Field::Nums),
    ("color", Field::Text),
    ("on", Field::Text),
    ("move", Field::Nums),
];

const STROKE_FIELDS: &[(&str, Field)] = &[
    ("rect", Field::Nums),
    ("ellipse", Field::Nums),
    ("polygon", Field::Nums),
    ("star", Field::Nums),
    ("arc", Field::Nums),
    ("axes", Field::Nums),
    ("rotate", Field::Num),
    ("about", Field::Nums),
    ("fill", Field::Num),
    ("closed", Field::Flag),
    ("smooth", Field::Flag),
    ("steps", Field::Num),
    ("trace", Field::Text),
    ("design", Field::Text),
    ("step", Field::Num),
];

const SOLIDS: &[&str] = &[
    "box", "cube", "cylinder", "sphere", "cone", "torus", "plane",
];

/// A design layer written as a line: its id first, then fields by name,
/// as the layer listing shows them: `sun ellipse 80 20 12 12 fill #ffcc00`,
/// `title text "Hello" at 50 10 size 8 align center`, `box rect 10 10 50
/// 30 4 fill none line #000000 2`.
pub fn layer_line(line: &str) -> std::result::Result<Value, String> {
    fields_line(line, LAYER_FIELDS, &[], true)
}

/// A draw stroke written as a line of fields by name: `rect 10 10 50 30`,
/// `ellipse 100 100 40 40 fill 6`, `design logo step 2 fill 8`.
pub fn stroke_line(line: &str) -> std::result::Result<Value, String> {
    fields_line(line, STROKE_FIELDS, &[], false)
}

/// A scene object written as a line: its id, its shape and size, then
/// fields by name: `seat box 0.5 0.5 0.05 at 0 0 0.45 color #884422`,
/// `leg cylinder 0.04 0.45 at 0.2 0.2 0.225`, `seat at 0 0 0.5`.
pub fn object_line(line: &str) -> std::result::Result<Value, String> {
    fields_line(line, OBJECT_FIELDS, SOLIDS, true)
}

/// A line of fields by name (after an id, when `id`) as a JSON object.
fn fields_line(
    line: &str,
    fields: &[(&str, Field)],
    shapes: &[&str],
    id: bool,
) -> std::result::Result<Value, String> {
    let bad = |why: String| {
        let names: Vec<&str> = fields.iter().map(|f| f.0).collect();
        format!(
            "`{line}`: {why} ({}{})",
            if id { "an id first, then " } else { "fields: " },
            names.join(", ")
        )
    };
    let words = words_of(line).map_err(|e| bad(e.to_string()))?;
    let field = |w: &(String, bool)| {
        (!w.1)
            .then(|| fields.iter().find(|f| f.0 == w.0.to_lowercase()))
            .flatten()
            .copied()
    };
    let shape = |w: &(String, bool)| !w.1 && shapes.contains(&w.0.to_lowercase().as_str());
    let starts = |w: &(String, bool)| field(w).is_some() || shape(w);
    // Whole numbers as integers: they also fill integer fields (step).
    let number = |w: &(String, bool)| {
        w.0.parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .map(|n| {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    json!(n as i64)
                } else {
                    json!(n)
                }
            })
            .ok_or_else(|| bad(format!("`{}` is not a number", w.0)))
    };
    let mut m = serde_json::Map::new();
    let mut i = 0;
    if let Some(first) = words.first()
        && id
        && !starts(first)
    {
        m.insert("id".into(), json!(first.0));
        i = 1;
    }
    while i < words.len() {
        let at = &words[i];
        i += 1;
        let from = i;
        while i < words.len() && !starts(&words[i]) {
            i += 1;
        }
        let values = &words[from..i];
        if shape(at) {
            m.insert("shape".into(), json!(at.0.to_lowercase()));
            if !values.is_empty() {
                let nums: Vec<Value> = values.iter().map(number).collect::<Result<_, _>>()?;
                m.insert("size".into(), Value::Array(nums));
            }
            continue;
        }
        let Some((name, kind)) = field(at) else {
            return Err(bad(format!("`{}` is not a field", at.0)));
        };
        let value = match kind {
            Field::Text if values.is_empty() => return Err(bad(format!("{name} needs a value"))),
            Field::Text => json!(
                values
                    .iter()
                    .map(|w| w.0.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            Field::Num => match values {
                [one] => number(one)?,
                _ => return Err(bad(format!("{name} takes one number"))),
            },
            Field::Nums if values.is_empty() => return Err(bad(format!("{name} needs numbers"))),
            Field::Nums => Value::Array(values.iter().map(number).collect::<Result<_, _>>()?),
            Field::Flag => match values {
                [] => json!(true),
                [w] if w.0 == "true" || w.0 == "false" => json!(w.0 == "true"),
                _ => return Err(bad(format!("{name} takes true or false"))),
            },
            Field::Outline => match values {
                [colour] => json!(colour.0),
                [colour, width] => {
                    m.insert("width".into(), number(width)?);
                    json!(colour.0)
                }
                _ => return Err(bad("line takes a colour and a width".into())),
            },
        };
        let key = if kind == Field::Outline {
            "stroke"
        } else {
            name
        };
        m.insert(key.into(), value);
    }
    Ok(Value::Object(m))
}

fn de_layers<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Vec<DesignLayer>>, D::Error> {
    de_lines(d, layer_line)
}

fn de_strokes<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<DrawStroke>, D::Error> {
    de_lines(d, stroke_line)?.ok_or_else(|| serde::de::Error::custom("strokes are needed"))
}

fn de_objects<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Vec<SceneObject>>, D::Error> {
    de_lines(d, object_line)
}

/// A list whose items are objects, or lines `parse` turns into them.
fn de_lines<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(
    d: D,
    parse: fn(&str) -> std::result::Result<Value, String>,
) -> std::result::Result<Option<Vec<T>>, D::Error> {
    let Some(items) = Option::<Vec<Value>>::deserialize(d)? else {
        return Ok(None);
    };
    items
        .into_iter()
        .map(|v| {
            let v = match v {
                Value::String(line) => parse(&line).map_err(serde::de::Error::custom)?,
                other => other,
            };
            serde_json::from_value(v).map_err(serde::de::Error::custom)
        })
        .collect::<std::result::Result<Vec<T>, _>>()
        .map(Some)
}

/// One step of a batch: `{tool, arguments}`, or a short line such as
/// `click 12`, `set 4 "Ada"`, `key cmd+s` ([`parse_step`]).
#[derive(Debug, Clone, PartialEq)]
pub struct BatchStep {
    pub tool: String,
    pub arguments: Value,
}

impl<'de> Deserialize<'de> for BatchStep {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Line(String),
            Full {
                tool: String,
                #[serde(default)]
                arguments: Value,
            },
        }
        match Repr::deserialize(d)? {
            Repr::Full { tool, arguments } => Ok(BatchStep { tool, arguments }),
            Repr::Line(line) => parse_step(&line).map_err(serde::de::Error::custom),
        }
    }
}

/// A batch step written as a short line:
///
/// `click 12` · `click "Save"` (by name) · `double 12` · `right 12` ·
/// `set 4 "Ada"` · `type "hello\n"` · `type 4 "hello"` · `key cmd+s` ·
/// `scroll 7 down 2` · `select 4 "word"` · `action 9 show_menu` ·
/// `wait "Saved"` · `find "Total"` · `look`; any action line can end with
/// `expect dialog` (or `expect "text"`).
pub fn parse_step(line: &str) -> std::result::Result<BatchStep, String> {
    let bad = |why: &str| {
        format!(
            "step `{line}`: {why}; write click N, click \"name\", double N, right N, set N \"value\", type [N] \"text\", key K, scroll [N] up|down|left|right [pages], select N [\"text\"], action N name, wait \"text\", find \"text\" or look (each may end with expect …), or {{tool, arguments}}"
        )
    };
    let mut words = words_of(line).map_err(|e| bad(&e))?;
    // `… expect dialog`: the rest is what should follow.
    let mut expect = None;
    if let Some(at) = words
        .iter()
        .skip(1)
        .position(|(w, quoted)| !quoted && w == "expect")
    {
        let rest: Vec<String> = words.drain(at + 1..).skip(1).map(|(w, _)| w).collect();
        if rest.is_empty() {
            return Err(bad("expect needs what to expect"));
        }
        expect = Some(rest.join(" "));
    }
    let Some((verb, _)) = words.first().cloned() else {
        return Err(bad("empty"));
    };
    let args: Vec<(String, bool)> = words[1..].to_vec();
    let index = |i: usize| -> Option<u32> {
        args.get(i)
            .filter(|(_, quoted)| !quoted)
            .and_then(|(w, _)| w.parse().ok())
    };
    let joined = |from: usize| -> String {
        args[from.min(args.len())..]
            .iter()
            .map(|(w, _)| w.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut m = serde_json::Map::new();
    let tool = match verb.to_lowercase().as_str() {
        "click" | "double" | "right" => {
            match (index(0), args.first()) {
                (Some(i), _) => m.insert("element_index".into(), json!(i)),
                (None, Some(_)) => m.insert("name".into(), json!(joined(0))),
                (None, None) => return Err(bad("click what")),
            };
            match verb.to_lowercase().as_str() {
                "double" => m.insert("click_count".into(), json!(2)),
                "right" => m.insert("button".into(), json!("right")),
                _ => None,
            };
            "click"
        }
        "set" => {
            let i = index(0).ok_or_else(|| bad("set needs an element number"))?;
            if args.len() < 2 {
                return Err(bad("set needs a value"));
            }
            m.insert("element_index".into(), json!(i));
            m.insert("value".into(), json!(joined(1)));
            "set_value"
        }
        "type" => {
            let from = match index(0) {
                Some(i) if args.len() > 1 => {
                    m.insert("element_index".into(), json!(i));
                    1
                }
                _ => 0,
            };
            if args.len() <= from {
                return Err(bad("type needs the text"));
            }
            m.insert("text".into(), json!(joined(from)));
            "type_text"
        }
        "key" | "press" => {
            if args.is_empty() {
                return Err(bad("key needs a key"));
            }
            m.insert("key".into(), json!(joined(0)));
            "press_key"
        }
        "scroll" => {
            let from = match index(0) {
                Some(i) => {
                    m.insert("element_index".into(), json!(i));
                    1
                }
                None => 0,
            };
            let dir = args
                .get(from)
                .map(|(w, _)| w.to_lowercase())
                .filter(|d| ["up", "down", "left", "right"].contains(&d.as_str()))
                .ok_or_else(|| bad("scroll needs up, down, left or right"))?;
            m.insert("direction".into(), json!(dir));
            if let Some((n, _)) = args.get(from + 1) {
                let pages: f64 = n.parse().map_err(|_| bad("pages must be a number"))?;
                m.insert("amount".into(), json!(pages));
            }
            "scroll"
        }
        "select" => {
            let i = index(0).ok_or_else(|| bad("select needs an element number"))?;
            m.insert("element_index".into(), json!(i));
            if args.len() > 1 {
                m.insert("text".into(), json!(joined(1)));
            }
            "select_text"
        }
        "action" => {
            let i = index(0).ok_or_else(|| bad("action needs an element number"))?;
            if args.len() < 2 {
                return Err(bad("action needs the action's name"));
            }
            m.insert("element_index".into(), json!(i));
            m.insert("action".into(), json!(joined(1)));
            "perform_secondary_action"
        }
        "wait" | "find" => {
            if args.is_empty() {
                return Err(bad("wait and find need the text"));
            }
            m.insert("text".into(), json!(joined(0)));
            if verb.eq_ignore_ascii_case("wait") {
                "wait_for"
            } else {
                "find_element"
            }
        }
        "look" => {
            if !args.is_empty() {
                return Err(bad("look takes nothing"));
            }
            "get_app_state"
        }
        _ => return Err(bad("unknown step")),
    };
    if let Some(e) = expect {
        if !matches!(
            tool,
            "click" | "set_value" | "type_text" | "press_key" | "perform_secondary_action"
        ) {
            return Err(bad("only an action can expect something"));
        }
        m.insert("expect".into(), json!(e));
    }
    Ok(BatchStep {
        tool: tool.into(),
        arguments: Value::Object(m),
    })
}

/// The words of a step line, and whether each was quoted. A quoted word
/// keeps its spaces; `\n`, `\t`, `\"` and `\\` inside quotes are
/// what they say.
fn words_of(line: &str) -> std::result::Result<Vec<(String, bool)>, String> {
    let mut out = Vec::new();
    let mut chars = line.trim().chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut w = String::new();
            loop {
                match chars.next() {
                    None => return Err("a quote isn't closed".into()),
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some('n') => w.push('\n'),
                        Some('t') => w.push('\t'),
                        Some(other) => w.push(other),
                        None => return Err("a quote isn't closed".into()),
                    },
                    Some(other) => w.push(other),
                }
            }
            out.push((w, true));
        } else {
            let mut w = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                w.push(c);
                chars.next();
            }
            out.push((w, false));
        }
    }
    Ok(out)
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
    /// Go on when a step brings up another window it didn't `expect`
    /// (default: stop there, since later steps were meant for the window
    /// before).
    #[serde(default)]
    pub through_windows: bool,
}

/// Run a script, or keep, show, list and delete saved ones.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScriptArgs {
    /// The script to run (or, with `save`, to keep).
    #[serde(default, deserialize_with = "de_opt_string")]
    pub code: Option<String>,
    /// Run this saved script.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub run: Option<String>,
    /// `args` in the script.
    #[serde(default)]
    pub args: Option<Value>,
    /// `data` in the script: anything the model hands it.
    #[serde(default)]
    pub data: Option<Value>,
    /// Keep `code` under this name: a script to run again, and a tool.
    #[serde(default, deserialize_with = "de_opt_string")]
    pub save: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub description: Option<String>,
    /// The saved script's arguments, as JSON-schema properties.
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub list: bool,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub show: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub delete: Option<String>,
    /// Every function scripts have.
    #[serde(default)]
    pub help: bool,
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
    Scene(SceneArgs),
    Locate(LocateArgs),
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
    Script(ScriptArgs),
    Decide(DecideArgs),
}

/// The names of the built-in tools (saved scripts may not take them).
pub const BUILTIN: [&str; 26] = [
    "list_apps",
    "launch_app",
    "get_app_state",
    "click",
    "perform_secondary_action",
    "set_value",
    "select_text",
    "scroll",
    "drag",
    "draw",
    "trace_image",
    "design",
    "scene",
    "locate",
    "press_key",
    "type_text",
    "find_element",
    "wait_for",
    "screenshot",
    "batch",
    "get_clipboard",
    "set_clipboard",
    "window",
    "get_notifications",
    "script",
    "decide",
];

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
            "scene" => ToolCall::Scene(parse_args(name, args)?),
            "locate" => ToolCall::Locate(parse_args(name, args)?),
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
            "script" => ToolCall::Script(parse_args(name, args)?),
            "decide" => ToolCall::Decide(parse_args(name, args)?),
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
            ToolCall::Scene(_) => "scene",
            ToolCall::Locate(_) => "locate",
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
            ToolCall::Script(_) => "script",
            ToolCall::Decide(_) => "decide",
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
    json!({"type": ["object", "string"], "properties": Value::Object(m), "additionalProperties": false, "description": "Or a line: id, then fields by name (\"sun ellipse 80 20 12 12 fill #ffcc00\", \"t text \\\"Hi\\\" at 50 10 size 8\", \"sun fill #ff0000\" to change)."})
}

/// A solid in a 3D scene.
fn scene_object_props() -> Value {
    json!({
        "type": ["object", "string"],
        "description": "Or a line: id, shape and size, then fields by name (\"seat box 0.5 0.5 0.05 at 0 0 0.45 color #884422\", \"seat at 0 0 0.5\" to change).",
        "properties": {
            "id": {"type": "string", "description": "Its name (\"seat\", \"leg-fl\"); used by change, mirror, repeat, on."},
            "shape": {"type": "string", "enum": ["box", "cylinder", "sphere", "cone", "torus", "plane"]},
            "size": {"type": "array", "items": {"type": "number"}, "description": "box [x, y, z]; cylinder, cone [diameter, height]; sphere [diameter]; torus [outer diameter, thickness]; plane [x, y]; or [x, y, z] for any."},
            "at": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "Its centre [x, y, z]."},
            "rotate": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "Degrees about x, then y, then z (Blender's XYZ). A cylinder or cone stands along z until turned."},
            "color": {"type": "string", "description": "\"#RRGGBB\"."},
            "on": {"type": "string", "description": "Set its height so it rests on this object's top, or on \"ground\"."},
            "move": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "Move by [dx, dy, dz]."}
        },
        "additionalProperties": false
    })
}

fn expect_prop() -> Value {
    json!({"type": "string", "description": "What should follow, checked after the action: \"dialog\" (another window comes up), \"change\", \"value\" (the element's value changes), \"gone\" (the element or its window goes), or a text that should then be on screen. The answer says confirmed, not seen, or uncertain."})
}

fn snap_prop() -> Value {
    json!({"type": "string", "description": "Move the x/y point to the nearest \"corner\", \"edge\", \"center\" (of the small shape there) or colour \"#RRGGBB\" first, to hit it exactly."})
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

/// Tool definitions, ready to hand to an LLM or list over MCP. Built once.
pub fn definitions() -> Vec<ToolDefinition> {
    static DEFS: std::sync::OnceLock<Vec<ToolDefinition>> = std::sync::OnceLock::new();
    DEFS.get_or_init(build_definitions).clone()
}

fn build_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_apps".into(),
            title: "List apps".into(),
            description: "List running desktop apps with their ids, pids and window counts. Use it to find the exact app to pass to other tools.".into(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: read_only("List apps"),
        },
        ToolDefinition {
            name: "launch_app".into(),
            title: "Launch app".into(),
            description: "Start (or bring up) a desktop app by its name in the system's app menu (\"Google Chrome\"), a bundle id or an executable, and wait until it shows a window, then return its first state (as get_app_state would); or open a web address (https://…) in the default browser. The name must be exact (an error lists similar ones); arguments and command lines are never accepted.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"app": {"type": "string", "description": "App name as the app menu shows it, bundle id or executable (no arguments), or an https:// address to open in the browser."}},
                "required": ["app"],
                "additionalProperties": false
            }),
            annotations: acting("Launch app"),
        },
        ToolDefinition {
            name: "get_app_state".into(),
            title: "Get app state".into(),
            description: "Get the current state of an app window: its accessibility tree with numbered elements, plus a screenshot when it adds information (first view of a window, a large change, custom-drawn UI; set screenshot=true to always include one). Call this first, before acting on an app; each action then reports the state after it, so call it again only when you need more. Element indices are only valid until the next get_app_state. After the first call, the tree may come back as a diff against the previous one; pass disable_diff=true to get the full tree. When the app is back on a screen you already saw (\"screen #N (seen before)\"), only what changed since then is sent, its element indices are the ones you saw then, and a new screenshot comes only if its pixels changed. A very large tree has its long lists folded (find_element finds folded items); max_tokens=0 returns it whole.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "disable_diff": {"type": "boolean", "description": "Return the full tree instead of a diff.", "default": false},
                    "screenshot": {"type": "boolean", "description": "true = always include a screenshot, false = never (default: decided by settings)."},
                    "ocr": {"type": "boolean", "default": false, "description": "Also read the window's text off the screen (for custom-drawn UI); the lines become clickable \"ocr text\" elements. Done automatically when the tree is nearly empty."},
                    "max_tokens": {"type": "integer", "minimum": 0, "description": "Token budget for this tree (default from settings). 0 = the whole tree, with no list folded or cut; use it only when you really need every element at once."},
                    "within": index_prop("Only this element and what is in it (a table, a panel, a dialog's part)."),
                    "about": {"type": "string", "description": "Only the parts of the window that have to do with this (\"shipping address\"); the others are folded to a line each."},
                }),
                &[],
            ),
            annotations: read_only("Get app state"),
        },
        ToolDefinition {
            name: "click".into(),
            title: "Click".into(),
            description: "Click an element by element_index (preferred: uses the element's accessibility action and works in the background), by its name (and role) when that names one element, or at x/y screenshot coordinates. Use button=right for context menus and click_count=2 for double-click.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Element to click, from the latest get_app_state."),
                    "x": coord_prop("X"),
                    "y": coord_prop("Y"),
                    "button": {"type": "string", "enum": ["left", "right", "middle"], "default": "left"},
                    "click_count": {"type": "integer", "minimum": 1, "maximum": 3, "default": 1},
                    "snap": snap_prop(),
                    "snap_radius": {"type": "number", "description": "How far to look for the snap, in screenshot pixels (default 10)."},
                    "name": {"type": "string", "description": "Instead of element_index: the name of the element to click (one element must have it)."},
                    "role": {"type": "string", "description": "With name: its role (\"button\", \"menu item\")."},
                    "expect": expect_prop()
                }),
                &[],
            ),
            annotations: acting("Click"),
        },
        ToolDefinition {
            name: "perform_secondary_action".into(),
            title: "Perform secondary action".into(),
            description: "Perform a named accessibility action on an element, other than a plain click: one of the actions listed for that element in get_app_state (e.g. show_menu, increment, decrement, confirm, cancel, raise, expand, collapse, toggle, pick).".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Target element, from the latest get_app_state."),
                    "action": {"type": "string", "description": "Action name as listed in actions=[...] for the element."},
                    "expect": expect_prop()
                }),
                &["element_index", "action"],
            ),
            annotations: acting("Perform secondary action"),
        },
        ToolDefinition {
            name: "set_value".into(),
            title: "Set value".into(),
            description: "Set an element's value directly: replace a text field's contents, move a slider, or set a checkbox/switch (\"true\"/\"false\"). Prefer this over typing when the element is marked editable or settable.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "element_index": index_prop("Target element, from the latest get_app_state."),
                    "value": {"type": "string", "description": "New value."},
                    "expect": expect_prop()
                }),
                &["element_index", "value"],
            ),
            annotations: acting("Set value"),
        },
        ToolDefinition {
            name: "select_text".into(),
            title: "Select text".into(),
            description: "Select text inside a text element: the given substring (its nth occurrence), or all text if `text` is omitted. Follow with type_text to replace it or press_key to act on it.".into(),
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
            name: "scroll".into(),
            title: "Scroll".into(),
            description: "Scroll the content of an element (a scroll area, list, web area…) or the area under x/y screenshot coordinates. amount is in pages (viewport sizes); fractions allowed.".into(),
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
            name: "drag".into(),
            title: "Drag".into(),
            description: "Drag from one element or point to another (move items, resize, reorder, select ranges). Each end is either an element index or x/y screenshot coordinates.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "from_element_index": index_prop("Drag start element."),
                    "from_x": coord_prop("Start X"),
                    "from_y": coord_prop("Start Y"),
                    "to_element_index": index_prop("Drop target element."),
                    "to_x": coord_prop("End X"),
                    "to_y": coord_prop("End Y"),
                    "snap": snap_prop(),
                    "snap_radius": {"type": "number", "description": "How far to look for the snap, in screenshot pixels (default 10)."}
                }),
                &[],
            ),
            annotations: acting("Drag"),
        },
        ToolDefinition {
            name: "draw".into(),
            title: "Draw".into(),
            description: "Draw with the mouse: for each stroke, press the button, move along the stroke and release (a pen or brush in a paint app, a signature field, shapes or function plots on a canvas). A stroke is a shape (rect [x,y,w,h(,radius)], ellipse [cx,cy,rx,ry], polygon [cx,cy,r,n], star [cx,cy,R,r,n], arc [cx,cy,r,from°,to°], bezier [[x,y],...]), points [[x,y],...] (straight lines; closed=true returns to the first point, smooth=true draws a smooth curve through them), a parametric curve (x and y are expressions in t: + - * / ^ %, sin cos tan sqrt abs exp ln log10 min max floor round, pi, e... from t[0] to t[1], default 0 to 1; steps=n draws n straight pieces) or a function plot (only y, in x: {\"y\": \"sin(x)\"}). Any stroke can be turned (rotate, about) and repeated (repeat: count, offset, rotate, about). Coordinates are screenshot pixels like click's x/y; with element_index, fractions of that element's box; with canvas, a document's own units (box + size) or math coordinates with y up (box + range, where axes [xstep, ystep] draws axes and ticks). fill=w paints a closed shape solid with a brush w wide instead of its outline (no bucket needed; later shapes cover earlier ones). trace + step draws one colour step of a picture traced with trace_image, design + step one of a design board picture. preview=true shows the strokes over a screenshot without drawing, on named cells (A1 top-left, sized to what is drawn), and the result says which cells the drawing covers. Parts of a curve outside the area are not drawn. Closed outlines come back with where a bucket click fills each (one click per piece when other lines cut it, or where a fill would leak out through a gap). The stop key ends a drawing midway.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "strokes": {
                        "type": "array",
                        "minItems": 1,
                        "description": "One press-move-release each.",
                        "items": {
                            "type": ["object", "string"],
                            "properties": stroke_props(),
                            "additionalProperties": false,
                            "description": "Or a line of fields by name: \"rect 10 10 50 30\", \"ellipse 100 100 40 40 fill 6\", \"design logo step 2 fill 8\"."
                        }
                    },
                    "element_index": index_prop("Draw inside this element; coordinates are fractions of its box."),
                    "canvas": canvas_prop(),
                    "preview": {"type": "boolean", "description": "Show the strokes in red over a screenshot, with a grid in the same coordinates; nothing is drawn."},
                    "cell_size": {"type": "number", "description": "The named cells' size in the canvas's units (default: sized to the drawing), to name the same cells as a design or a script's page."},
                    "button": {"type": "string", "enum": ["left", "right", "middle"]},
                    "speed": {"type": "number", "minimum": 50, "description": "Pointer speed in pixels per second (default 800)."}
                }),
                &["strokes"],
            ),
            annotations: acting("Draw"),
        },
        ToolDefinition {
            name: "trace_image".into(),
            title: "Trace a picture".into(),
            description: "Turn a reference picture (an image file, or what a window shows) into a few flat colours and shapes to paint back to front, for copying it in a paint app. Returns the steps (one colour each, in painting order) and a picture of the result. Then, per step: set the app's colour to the step's hex and draw with strokes=[{trace: name, step: n, fill: brush width}] and the canvas. screenshot with compare=name shows where the canvas still differs.".into(),
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
            name: "locate".into(),
            title: "Locate exactly".into(),
            description: "Find exact places in a window, in the x/y click takes: color=\"#RRGGBB\" (every area of that colour, with its centre and box), like=[l,t,r,b] (every other place that looks like that part of the window: the same icon, button or marker), or near=[x,y] with feature corner/edge/center (the exact point next to a rough one). box limits where to look. Use it before clicking small or custom-drawn targets.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "box": {"type": "array", "items": {"type": "number"}, "description": "[left, top, right, bottom] in screenshot pixels: where to look (default: the whole window)."},
                    "color": {"type": "string", "description": "\"#RRGGBB\": find the areas of this colour."},
                    "tolerance": {"type": "number", "description": "Largest difference per colour channel still counted (default 16)."},
                    "like": {"type": "array", "items": {"type": "number"}, "description": "[left, top, right, bottom] of something to find again."},
                    "near": {"type": "array", "items": {"type": "number"}, "description": "[x, y]: a rough point."},
                    "feature": {"type": "string", "enum": ["corner", "edge", "center"], "description": "What to find near the point."},
                    "radius": {"type": "number", "description": "How far from near to look, in screenshot pixels (default 12)."},
                    "picture": {"type": "boolean", "description": "Send the window with the places found numbered (default: settings, usually yes)."}
                }),
                &[],
            ),
            annotations: read_only("Locate exactly"),
        },
        ToolDefinition {
            name: "design".into(),
            title: "Design board".into(),
            description: "A design board, like Canva: compose a picture from layers (shapes as draw takes them, and text) and see it rendered before anything is drawn in an app. Each call can add, change, remove, mirror (a symmetric copy: the other eye or ear), align, distribute and reorder layers; it returns the picture, the layers with their boxes and colours, checks (off the page, almost centred, pairs not quite symmetric, hard-to-read text) and the steps to paint it. Then put it into an app: export=\"svg\" or \"png\" (a temporary file to import), the layers' numbers for the app's fields, or draw with strokes=[{design, step, fill}]. Coordinates are the design's units (pixels of the result), y down. Start with name and size; later calls with the same name change it.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "size": {"type": "array", "items": {"type": "number"}, "description": "[width, height]: starts the design (or resizes it)."},
                    "background": {"type": "string", "description": "\"#RRGGBB\"."},
                    "margin": {"type": "number", "description": "Space to keep clear at the edges (default 5% of the short side)."},
                    "cell_size": {"type": "number", "minimum": 0, "description": "The named cells' size in the design's units, e.g. 100 for an 8 x 8 board on 800 x 800 (default 0: about eight across)."},
                    "add": {"type": "array", "items": layer_props(), "description": "New layers, on top (or below/above a layer)."},
                    "change": {"type": "array", "items": layer_props(), "description": "Layers to change, by id: new colours, text, place (move, to) or a new shape."},
                    "remove": {"type": "array", "items": {"type": "string"}},
                    "mirror": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "as": {"type": "string"}, "axis": {"type": "string", "enum": ["x", "y"]}, "line": {"type": "number"}}, "required": ["id", "as"], "additionalProperties": false}, "description": "A mirrored copy: x = left-right about the page's middle (or line)."},
                    "align": {"type": "array", "items": {"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "string"}}, "x": {"type": "string", "enum": ["left", "center", "right"]}, "y": {"type": "string", "enum": ["top", "middle", "bottom"]}, "to": {"type": "string", "description": "page (default), margins, each other, or a layer id."}}, "required": ["ids"], "additionalProperties": false}},
                    "distribute": {"type": "array", "items": {"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "string"}}, "axis": {"type": "string", "enum": ["x", "y"]}}, "required": ["ids"], "additionalProperties": false}, "description": "Equal gaps between 3 or more layers."},
                    "order": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "to": {"type": "string", "enum": ["front", "back", "up", "down"]}}, "required": ["id", "to"], "additionalProperties": false}},
                    "show": {"type": "object", "properties": {"grid": {"type": ["number", "boolean"]}, "ids": {"type": "boolean"}, "guides": {"type": "boolean"}, "cells": {"type": "boolean"}, "cell": {"type": "string"}, "steps": {"type": "boolean"}}, "additionalProperties": false, "description": "On the picture: the named cells (A1 top-left, on unless cells=false), a grid in the design's units, the layers' ids, guides (margins, centre, thirds); cell=\"C4\" shows just that cell, magnified, with the layers in it. steps=true lists the steps to paint it."},
                    "export": {"type": "string", "enum": ["png", "svg"], "description": "Write a temporary file to import into an app."}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            annotations: read_only("Design board"),
        },
        ToolDefinition {
            name: "scene".into(),
            title: "3D scene".into(),
            description: "Plan a 3D model as solids before building it in a 3D app: boxes, cylinders, spheres, cones, tori and planes with exact sizes, centres and rotations, in metres (or any one unit), Z up, the ground at z 0. Each call can add, change, remove, mirror (the other leg or wing) and repeat (in a row, or around an axis) objects; it returns one picture with the front (x right, z up), right (y right, z up) and top (x right, y up) views to one scale with a grid, and a perspective view with shadows straight down; the objects with their extents; checks (parts that float, sink below the ground or run into each other, with how far); and how to build it. Then build it: Location = at, Rotation = rotate, Dimensions = size in Blender's sidebar (the same numbers in other apps), or export=\"obj\" (a temporary file to import). Start with name and add; later calls with the same name change it.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "add": {"type": "array", "items": scene_object_props(), "description": "New objects (shape and size needed). Without at, an object stands on the ground at the middle."},
                    "change": {"type": "array", "items": scene_object_props(), "description": "Objects to change, by id."},
                    "remove": {"type": "array", "items": {"type": "string"}},
                    "mirror": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "as": {"type": "string"}, "axis": {"type": "string", "enum": ["x", "y", "z"]}, "at": {"type": "number"}}, "required": ["id", "as"], "additionalProperties": false}, "description": "A mirrored copy: axis x flips left-right about x = at (default 0)."},
                    "repeat": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "count": {"type": "integer", "minimum": 2, "maximum": 100}, "offset": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3}, "around": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "angle": {"type": "number"}}, "required": ["id", "count"], "additionalProperties": false}, "description": "Copies named id-2, id-3...: count in all, each offset [dx, dy, dz] further, or turned around a vertical axis through around [x, y] (angle in all, default 360)."},
                    "ground": {"type": "boolean", "description": "The ground at z 0 (default true): report what floats or sinks."},
                    "view": {"type": "string", "enum": ["all", "front", "right", "top", "perspective"], "description": "One view, bigger (default all four)."},
                    "look": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "Perspective camera [turn, tilt] in degrees: turn 0 from the front, 90 from the right; tilt 90 from above (default [35, 25])."},
                    "ids": {"type": "boolean", "description": "Objects' ids on the picture (default true)."},
                    "export": {"type": "string", "enum": ["obj", "png"], "description": "Write a temporary file: obj (with its colours, to import into a 3D app) or png (the picture)."}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            annotations: read_only("3D scene"),
        },
        ToolDefinition {
            name: "press_key".into(),
            title: "Press key".into(),
            description: "Press a key or shortcut in the app, e.g. \"Return\", \"Escape\", \"Tab\", \"cmd+s\", \"ctrl+shift+t\", \"alt+Left\". \"cmd\" is Cmd on a Mac and Ctrl elsewhere; \"win\"/\"super\" is the Windows/Super key. Several space-separated combos are pressed in order (\"Down Down Return\"). Optionally focus element_index first.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "key": {"type": "string", "description": "Key combo(s): modifiers (cmd/ctrl/alt/option/shift/meta) joined with + and a key name."},
                    "element_index": index_prop("Element to focus before pressing."),
                    "x": hover_prop("X"),
                    "y": hover_prop("Y"),
                    "expect": expect_prop()
                }),
                &["key"],
            ),
            annotations: acting("Press key"),
        },
        ToolDefinition {
            name: "type_text".into(),
            title: "Type text".into(),
            description: "Type text into the app's focused element, as keyboard input. Optionally focus element_index first. For replacing a field's whole contents prefer set_value.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "text": {"type": "string", "description": "Text to type. Newlines press Return."},
                    "element_index": index_prop("Element to focus before typing."),
                    "x": hover_prop("X"),
                    "y": hover_prop("Y"),
                    "expect": expect_prop()
                }),
                &["text"],
            ),
            annotations: acting("Type text"),
        },
        ToolDefinition {
            name: "find_element".into(),
            title: "Find element".into(),
            description: "Search the app's current accessibility tree for elements matching a role and/or a name/text substring, and return them with their element_index. Cheaper than reading the whole tree. Refreshes state, so the returned indices are current.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "role": {"type": "string", "description": "Normalized role to match, e.g. button, text field, checkbox, link."},
                    "name": {"type": "string", "description": "Case-insensitive substring of the element's name/label."},
                    "text": {"type": "string", "description": "Case-insensitive substring of the element's name or value."},
                    "editable": {"type": "boolean", "default": false, "description": "Only editable elements."},
                    "max_results": {"type": "integer", "minimum": 1, "default": 20},
                    "offset": {"type": "integer", "minimum": 0, "default": 0, "description": "Skip this many matches: the next page."}
                }),
                &[],
            ),
            annotations: read_only("Find element"),
        },
        ToolDefinition {
            name: "wait_for".into(),
            title: "Wait for element".into(),
            description: "Poll the app until an element matching the given role/name/text (and optional state) appears, then return it; or, with until, until the decision model answers yes to a question about the window (\"Have the search results loaded?\"). Use after actions that take time (loading, dialogs). Fails when the timeout elapses.".into(),
            input_schema: schema(
                app_props(),
                json!({
                    "role": {"type": "string", "description": "Normalized role to match."},
                    "name": {"type": "string", "description": "Case-insensitive substring of the name/label."},
                    "text": {"type": "string", "description": "Case-insensitive substring of name or value."},
                    "state": {"type": "string", "enum": ["present", "visible", "enabled", "focused", "checked"], "default": "present"},
                    "timeout_ms": {"type": "integer", "minimum": 1, "description": "Give up after this long (default from settings)."},
                    "poll_ms": {"type": "integer", "minimum": 1, "description": "Re-check interval (default from settings)."},
                    "until": {"type": "string", "description": "A yes/no question about the window, asked of the decision model each time (see decide): wait until the answer is yes."}
                }),
                &[],
            ),
            annotations: read_only("Wait for element"),
        },
        ToolDefinition {
            name: "screenshot".into(),
            title: "Screenshot".into(),
            description: "Capture an image: the screen (mode=auto, the default without app: the whole screen, or only the part that changed since your last full-screen screenshot; mode=full: always all of it), a screen rectangle (x/y/width/height in screen coordinates; mode=region), or an app window (with app: nothing if it looks as in your last picture of it, else only the part that changed when that is small, else all of it; mode=window: always all of it). With element_index, zoom into that element of the window (to read small text). With annotate=true on a window, each element's index is drawn over it (set-of-marks). For exact positions and colours: grid=N draws a labelled grid every N units (true: a round step) in the x/y that click and draw use for that window (screen coordinates for full/region); with canvas (as draw takes it) the grid covers just the document, labelled in its units or math range. palette=true lists the main colours, pick=[[x,y],...] gives the exact colour at each point (same coordinates as the grid). compare=name (with canvas) compares the document with a picture traced by trace_image and lists where it differs most. zoom=[x,y] magnifies around a point to aim a click exactly.".into(),
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
                    "compare": {"type": "string", "description": "A trace_image name: compare the canvas with it."},
                    "zoom": {"type": "array", "items": {"type": "number"}, "description": "[x, y] (window shots): a magnified view around this point, each screen pixel a square, with a crosshair on it and a grid in the x/y click takes. To aim exactly."},
                    "radius": {"type": "number", "description": "How far around the zoom point, in screen pixels (default 12)."},
                    "cells": {"type": "boolean", "description": "With canvas: named cells over the document (graph paper: columns A, B…, rows 1, 2…, about 8 across)."},
                    "cell": {"type": "string", "description": "With canvas: just this cell (\"C4\"), magnified, with a fine grid and its colours."},
                    "cell_size": {"type": "number", "description": "With cells or cell: the cells' size in the canvas's units (default: about 8 across)."}
                },
                "additionalProperties": false
            }),
            annotations: read_only("Screenshot"),
        },
        ToolDefinition {
            name: "batch".into(),
            title: "Batch actions".into(),
            description: "Run several computer-use tools in order in one call (e.g. fill a form then submit), and get one report of what changed. Each step is a line: click 12, click \"Save\" (by name), double 12, right 12, set 4 \"Ada\", type \"text\" (type 4 \"text\" focuses 4 first), key cmd+s, scroll 7 down 2, select 4 \"word\", action 9 show_menu, wait \"Saved\", find \"Total\", look; an action line may end with expect dialog (or another expect). Or {\"tool\": name, \"arguments\": {...}}. Stops at the first failure unless continue_on_error is true, and when a step brings up another window it didn't expect (through_windows=true goes on). Steps missing `app` inherit the top-level app.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": {"type": "string", "description": "Default app for steps that omit it."},
                    "continue_on_error": {"type": "boolean", "default": false},
                    "through_windows": {"type": "boolean", "default": false, "description": "Go on when a step brings up another window it didn't expect."},
                    "steps": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": ["string", "object"],
                            "description": "A step line (\"click 12\", \"set 4 \\\"Ada\\\"\", \"key Return\") or {tool, arguments}."
                        }
                    }
                },
                "required": ["steps"],
                "additionalProperties": false
            }),
            annotations: acting("Batch actions"),
        },
        ToolDefinition {
            name: "window".into(),
            title: "Manage windows".into(),
            description: "Arrange app windows and see the screens. action: displays (the screens and virtual desktops; no app needed), list (the app's windows with position, size and state), focus, move (x, y and optionally width, height), resize (width, height), maximize, minimize, restore, fullscreen, exit_fullscreen, close, tile_left / tile_right / tile_top / tile_bottom (half of a display), center, move_to_display (display), move_to_desktop (desktop). Positions and sizes are screen coordinates (as in get_app_state's window line), not screenshot pixels.".into(),
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
            name: "get_notifications".into(),
            title: "Read notifications".into(),
            description: "Read the user's recent desktop notifications (app, title, text, how long ago), newest last. Codes and card numbers in them are masked, and notifications from apps the user blocked are left out.".into(),
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
            name: "script".into(),
            title: "Run a script".into(),
            description: "Write and run a script: a small program the server runs (Rhai, like JavaScript: let, if/else, for x in list or range(a, b), while, fn name(a) {...}, closures |x| ..., arrays [..], maps #{key: value}, `text ${x}`), for what the other tools can't do in one call: loops and conditions over tool calls, maths, data from files or the web, pictures on graph paper. In a script: tool(name, #{...}) runs any tool and returns its text (a failure stops the script; try_tool(name, #{...}) returns #{ok, text, image} instead); set_app(name) fills in app; elements(app, #{role, name, text}) gives the matching elements as maps (index, role, name, value, states, x, y, w, h); colors(app, [[x, y], ...]) exact colours; page(name, width, height, #{cell: size}) is a graph-paper page (a design-board design, cells A1 top-left) with p.rect, circle, ellipse, line, path, polygon, star, arc, curve, text, layer, fill_cell(\"C4\", colour), text_in(\"C4\", text), p.cell(\"C4\") and p.at(x, y), p.show() (the last page touched is shown anyway), p.steps(), p.export(\"png\"); cells(w, h, size) gives the same cells for any canvas; read_text/read_json/read_csv and write_text/write_json (relative paths: the scripts' own folder), fetch/fetch_json/download (http), parse_json, to_json, regex_find, numbers(text), remember/recall (kept between runs), random, now, sleep; print() writes to the result; data and args hold what you pass. help=true lists every function. save=name (with description and params) keeps the script: run=name runs it, and it becomes a tool of its own; list, show and delete manage saved scripts. A script's tool calls are ordinary calls: the stop key and the pause while the user works apply.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "The script (Rhai). It runs now, or with save is kept."},
                    "run": {"type": "string", "description": "Run this saved script."},
                    "args": {"type": "object", "description": "Arguments: `args` in the script."},
                    "data": {"description": "Any JSON the script needs: `data` in it (a table, points, text...)."},
                    "save": {"type": "string", "description": "Keep code under this name (lowercase letters, digits, _ and -): run it later by name, and as a tool of its own."},
                    "description": {"type": "string", "description": "With save: what it does (the tool's description)."},
                    "params": {"type": "object", "description": "With save: its arguments as JSON-schema properties, e.g. {\"size\": {\"type\": \"number\"}}."},
                    "list": {"type": "boolean", "description": "List the saved scripts."},
                    "show": {"type": "string", "description": "A saved script's code."},
                    "delete": {"type": "string", "description": "Delete a saved script."},
                    "help": {"type": "boolean", "description": "Every function scripts have, with examples."}
                },
                "additionalProperties": false
            }),
            annotations: acting("Run a script"),
        },
        ToolDefinition {
            name: "decide".into(),
            title: "Decide fast".into(),
            description: "Typed answers from a decision model (TypeSafe's Jev, or a fast OpenAI-compatible model the user added with Ctrl+Alt+J), in well under a second and without reading the thing yourself: a yes/no question (the probability of yes), a choice among options, or a score on a scale, about a state: text or JSON you pass, each of many items (judged in parallel: reviews, results, rows), or an app's window (app: its elements as text). pick=\"what you look for\" (with app) returns the element_index that fits. Use it to classify, score, filter or check many things, and to find the element meant by a description. Several questions at once: questions={\"name\": {\"type\": \"yes_no\"|\"choice\"|\"score\", \"question\", \"options\" or \"scale\"}}. setup: \"status\", \"open\" (opens the settings page in the user's browser), \"test\", \"remove\", or {provider: \"jev\"|\"openai\", base_url, model, api_key} when the user gives these in the chat."
                .replace("Ctrl+Alt+J", crate::decision::SETTINGS_KEY).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "A yes/no question, or the question for options or scale."},
                    "options": {"type": ["array", "object"], "description": "A choice: [\"a\", \"b\"], or {\"a\": \"what a means\"}."},
                    "scale": {"type": "array", "items": {"type": "string"}, "description": "A score: the levels, low to high."},
                    "questions": {"type": "object", "description": "Several questions: {\"name\": {\"type\": \"yes_no\"|\"choice\"|\"score\", \"question\": \"...\", \"options\": ..., \"scale\": [...]}}."},
                    "state": {"description": "What to judge: text or any JSON."},
                    "items": {"type": "array", "description": "Judge each of these on its own (text or JSON each)."},
                    "app": {"type": "string", "description": "Judge this app's window (or pick in it)."},
                    "window": {"type": "string", "description": "Window id or title substring."},
                    "pick": {"type": "string", "description": "With app: describe an element (\"the button that adds it to the cart\"); returns its element_index."},
                    "read": {"type": "boolean", "description": "With pick: also return the element's whole text or value (\"the order total\")."},
                    "setup": {"type": ["string", "object"], "description": "\"status\", \"open\", \"test\", \"remove\", or {provider, base_url, model, api_key} (only when the user asks for it)."}
                },
                "additionalProperties": false
            }),
            annotations: read_only("Decide fast"),
        },
        ToolDefinition {
            name: "get_clipboard".into(),
            title: "Get clipboard".into(),
            description: "Read the system clipboard as text.".into(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            annotations: read_only("Get clipboard"),
        },
        ToolDefinition {
            name: "set_clipboard".into(),
            title: "Set clipboard".into(),
            description: "Write text to the system clipboard (e.g. to paste it into an app with press_key cmd+v / ctrl+v).".into(),
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
        "launch_app" => {
            "Start an app by name/id; returns its first state. Or open an https:// address in the browser."
        }
        "get_app_state" => {
            "The app window's numbered accessibility tree (+ a screenshot when useful). Call first; actions then report the state after them, so call again only for more. Element indices are valid until the next call; later calls return a diff. A screen \"seen before\" keeps the indices you saw then. screenshot=true forces an image; max_tokens=0 returns a huge tree whole, unfolded; within=index: just that element's part; about=\"words\": just the parts about that; rebase=true: all of it again."
        }
        "click" => {
            "Click element_index (preferred), name (+role) of one element, or x,y in screenshot pixels. button right/middle, click_count 2 = double; snap corner/edge/center/#hex moves x,y onto it. expect = dialog, change, value, gone or a text to see after: checked (confirmed, not seen, uncertain)."
        }
        "perform_secondary_action" => {
            "Run one of an element's listed actions=[...] (not a plain click). expect as in click."
        }
        "set_value" => {
            "Set a field's text, a slider, or a checkbox (\"true\"/\"false\") directly. expect as in click."
        }
        "select_text" => "Select the given text (or all text) in a text element.",
        "scroll" => "Scroll an element or the area at x,y; amount is in pages.",
        "drag" => {
            "Drag from an element/point to another element/point; snap moves the ends onto a corner/edge/center/#hex."
        }
        "draw" => {
            "Draw with the mouse held down along strokes: rect [x,y,w,h(,r)], ellipse [cx,cy,rx,ry], polygon [cx,cy,r,n], star [cx,cy,R,r,n], arc [cx,cy,r,a0,a1], bezier, points (closed, smooth), a curve x,y in t over t=[from,to] (steps=n), or a plot {y: \"sin(x)\"}; rotate/about, repeat {count, offset, rotate, about}; a stroke may be a line (\"ellipse 100 100 40 40 fill 6\"). Screenshot pixels; element_index fractions; canvas {box:[l,t,r,b], size:[w,h]} document units or {box, range:[x0,x1,y0,y1]} math (y up; axes [dx,dy]). fill=w paints a closed shape solid with a w-wide brush; trace or design + step draws one colour step of a trace_image picture or a design. preview=true only shows them (over named cells, A1 top-left). Returns the cells it covers and where a bucket click fills each closed outline."
        }
        "press_key" => {
            "Press keys or shortcuts, e.g. \"cmd+s\", \"Down Down Return\". x,y points the mouse there first (Blender sends keys to what is under it). expect as in click."
        }
        "type_text" => {
            "Type text into the focused element (element_index focuses first; x,y points the mouse there first). expect as in click."
        }
        "find_element" => {
            "Find elements by role/name/text; returns their indices (offset: the next page)."
        }
        "wait_for" => {
            "Wait until an element matching role/name/text (and state) appears, or until the decision model answers yes to until=\"question about the window\"."
        }
        "locate" => {
            "Exact places in a window (x/y click takes): color=#hex areas, like=[l,t,r,b] look-alikes, or near=[x,y] + feature corner/edge/center."
        }
        "design" => {
            "Design board (like Canva): build a picture from layers (draw shapes, text) with add/change/remove/mirror/align/distribute/order; a layer may be a line (\"sun ellipse 80 20 12 12 fill #ffcc00\"; change: \"sun fill #ff0000\"). Returns the picture, layers, checks and paint steps, then only what changed (a call that changes nothing shows all); export svg/png (temporary) or draw {design, step, fill}. Plan every drawing here first."
        }
        "scene" => {
            "3D scene: plan a model as solids (box, cylinder, sphere, cone, torus, plane; size, at = centre, rotate; metres, Z up) with add/change/remove/mirror/repeat; an object may be a line (\"seat box 0.5 0.5 0.05 at 0 0 0.45 color #884422\"). Returns front/right/top views to scale + perspective, extents, checks (floating, sinking, overlaps) and build numbers, then only what changed (a call that changes nothing shows all); export obj (temporary). Plan every 3D model here first."
        }
        "trace_image" => {
            "Turn a reference picture (path, or app [+box]) into flat colour steps to paint back to front; then draw {trace, step, fill} per step, after setting the step's colour."
        }
        "screenshot" => {
            "Image of the screen (auto: only what changed since the last one), a region (x,y,width,height), an app window (only what changed since your last picture of it; mode=window: all), or one element (element_index zooms in; annotate=true draws indices). grid=N (or true): labelled grid in the x/y click and draw use, or in canvas units with canvas; palette=true: main colours; pick=[[x,y]]: exact colours; compare=trace name (with canvas): where the canvas differs; cells=true (with canvas): named cells over the document, cell=\"C4\": that cell magnified; zoom=[x,y]: magnified view to aim."
        }
        "batch" => {
            "Run steps in order, one report at the end. A step is a line (click 12 · click \"Save\" · double 12 · right 12 · set 4 \"Ada\" · type [4] \"text\" · key cmd+s · scroll [7] down [2] · select 4 \"word\" · action 9 name · wait \"text\" · find \"text\" · look; actions may end with expect …) or {tool, arguments}. Stops on an error, or on a window a step didn't expect (through_windows=true goes on)."
        }
        "window" => {
            "Windows and screens: action displays|list|focus|move|resize|maximize|minimize|restore|fullscreen|exit_fullscreen|close|tile_left|tile_right|tile_top|tile_bottom|center|move_to_display|move_to_desktop; x/y/width/height in screen coordinates."
        }
        "get_notifications" => "Recent desktop notifications (app, title, text); filter by app.",
        "script" => {
            "Run a script (Rhai, like JavaScript: let, if, for x in range(a, b), fn, |x| closures, [arrays], #{maps}) for loops over tools, maths, file or web data and graph-paper pictures. tool(name, #{args}) → text (try_tool() → #{ok, text, image}); set_app; elements(app, #{role, name, text}) → maps; colors(app, [[x,y]]); page(name, w, h, #{cell}) → p.rect/circle/line/path/polygon/text/fill_cell(\"C4\", colour)/text_in/cell(\"C4\")/at(x,y)/show/steps/export; cells(w, h, size); read_text/read_json/read_csv/write_text, fetch/fetch_json/download, remember/recall, regex_find, numbers, random, sleep, print; data/args = what you pass. help=true: every function. save=name (+description, params) keeps it as a tool of its own; run=name, list, show, delete."
        }
        "decide" => {
            "Fast typed answers from the decision model (Jev, or a model the user added with the settings key): question (yes/no → probability), + options (choice) or scale (score, low→high); questions={name: {type, question, options|scale}} for several. About state (text/JSON), each of items (in parallel), or app's window; pick=\"description\" + app → element_index (read=true: and its whole text). setup=status|open (settings page)|test|remove|{provider, base_url, model, api_key}."
        }
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
                // A list's items are described by the tool's short text.
                if let Value::Object(i) = items {
                    i.remove("description");
                }
                strip_descriptions(items);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(strip_descriptions),
        _ => {}
    }
}

/// A property whose schema repeats an earlier one's word for word (a
/// design's `change` items are its `add` items) is sent as just its type:
/// the model has the keys already, and they are the biggest part of the
/// tool list.
fn share_repeats(schema: &mut Value) {
    let Some(Value::Object(props)) = schema.get_mut("properties") else {
        return;
    };
    let mut seen: Vec<(String, String)> = Vec::new();
    for (name, prop) in props.iter_mut() {
        let text = prop.to_string();
        if text.len() < 200 {
            continue;
        }
        if let Some((first, _)) = seen.iter().find(|(_, t)| *t == text) {
            let kind = prop.get("type").cloned().unwrap_or(json!("object"));
            let mut short = json!({"type": kind, "description": format!("Same keys as {first}.")});
            if kind == "array" {
                short["items"] = json!({"type": "object"});
            }
            *prop = short;
        } else {
            seen.push((name.clone(), text));
        }
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
    let lean = config.tools.descriptions == crate::config::DescriptionStyle::Lean;
    let decisions = !config.decision.provider.trim().is_empty();
    definitions_for(&config.tools)
        .into_iter()
        .filter(|d| match &*d.name {
            "get_clipboard" | "set_clipboard" => config.clipboard,
            "get_notifications" => config.notifications.enabled,
            "screenshot" => screenshots,
            // Lean: the decision model's tool once there is one.
            "decide" => !lean || decisions,
            _ => true,
        })
        .collect()
}

/// The tools that come first with the tool manager on ([tools] manager);
/// the others are found by category with `find_tools`.
pub const BASE_TOOLS: &[&str] = &[
    "list_apps",
    "launch_app",
    "get_app_state",
    "click",
    "perform_secondary_action",
    "set_value",
    "select_text",
    "scroll",
    "drag",
    "press_key",
    "type_text",
    "find_element",
    "wait_for",
    "batch",
    "screenshot",
];

/// The other tools, by category: (name, what they are for, tools). Saved
/// scripts belong to "scripts".
pub const CATEGORIES: &[(&str, &str, &[&str])] = &[
    (
        "design",
        "drawing, the design board, 3D scenes, copying a picture, exact places to aim at",
        &["draw", "design", "scene", "trace_image", "locate"],
    ),
    ("windows", "arranging windows and screens", &["window"]),
    (
        "scripts",
        "small programs for loops, maths and data, and saved scripts",
        &["script"],
    ),
    (
        "clipboard",
        "reading and writing the clipboard",
        &["get_clipboard", "set_clipboard"],
    ),
    (
        "notifications",
        "recent desktop notifications",
        &["get_notifications"],
    ),
    (
        "decisions",
        "a fast decision model's typed answers",
        &["decide"],
    ),
];

/// The tool manager's own tools.
pub const MANAGER_TOOLS: [&str; 2] = ["find_tools", "use_tool"];

/// The category of a tool that isn't a base tool (a saved script is a
/// script).
pub fn category_of(name: &str) -> &'static str {
    CATEGORIES
        .iter()
        .find(|(_, _, tools)| tools.contains(&name))
        .map_or("scripts", |(c, _, _)| c)
}

/// `find_tools` (and `use_tool` when tools are run through it).
pub fn manager_definitions(dispatch: bool) -> Vec<ToolDefinition> {
    let categories: Vec<String> = CATEGORIES
        .iter()
        .map(|(c, about, _)| format!("{c} ({about})"))
        .collect();
    let then = if dispatch {
        "run one with use_tool(name, arguments)"
    } else {
        "it is added to your tools"
    };
    let mut defs = vec![ToolDefinition {
        name: "find_tools".into(),
        title: "Find tools".into(),
        description: format!(
            "More tools, by category or by what they do: {}. Returns each tool with its arguments; {then}.",
            categories.join(", ")
        )
        .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": CATEGORIES.iter().map(|c| c.0).collect::<Vec<_>>()},
                "query": {"type": "string", "description": "Words for what the tool should do."}
            },
            "additionalProperties": false
        }),
        annotations: read_only("Find tools"),
    }];
    if dispatch {
        defs.push(ToolDefinition {
            name: "use_tool".into(),
            title: "Use a tool".into(),
            description: "Run a tool find_tools showed: name and its arguments.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "arguments": {"type": "object"}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            annotations: acting("Use a tool"),
        });
    }
    defs
}

/// Lean schemas: nested objects (a design's layers, a drawing's strokes)
/// as the list of their keys (`*` marks required ones), no `window`, no
/// defaults.
fn lighten(schema: &mut Value) {
    fn keys(v: &Value) -> Option<String> {
        let props = v.get("properties")?.as_object()?;
        let required: Vec<&str> = v
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        Some(
            props
                .keys()
                .map(|k| {
                    if required.contains(&k.as_str()) {
                        format!("{k}*")
                    } else {
                        k.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
    let Some(Value::Object(props)) = schema.get_mut("properties") else {
        return;
    };
    props.remove("window");
    for prop in props.values_mut() {
        let Value::Object(p) = prop else { continue };
        p.remove("default");
        if let Some(k) = keys(&Value::Object(p.clone())) {
            let kind = p.get("type").cloned().unwrap_or(json!("object"));
            *prop = json!({"type": kind, "description": format!("keys: {k}")});
            continue;
        }
        if let Some(items) = p.get_mut("items")
            && let Some(k) = keys(items)
        {
            let kind = items.get("type").cloned().unwrap_or(json!("object"));
            let line = if kind.is_array() { " (or a line)" } else { "" };
            *items = json!({"type": kind, "description": format!("keys: {k}{line}")});
        }
    }
}

/// Tool definitions filtered and styled by the user's `[tools]` settings.
pub fn definitions_for(cfg: &crate::config::ToolsConfig) -> Vec<ToolDefinition> {
    use crate::config::DescriptionStyle;
    let compact = cfg.descriptions != DescriptionStyle::Full;
    let lean = cfg.descriptions == DescriptionStyle::Lean;
    definitions()
        .into_iter()
        .filter(|d| cfg.is_enabled(&d.name))
        .map(|mut d| {
            if compact {
                if let Some(short) = short_description(&d.name) {
                    d.description = short.into();
                }
                strip_descriptions(&mut d.input_schema);
                share_repeats(&mut d.input_schema);
            }
            if lean {
                lighten(&mut d.input_schema);
            }
            // The last app named stands in for a missing one.
            if cfg.default_app
                && let Some(Value::Array(req)) = d.input_schema.get_mut("required")
            {
                req.retain(|r| r != "app");
            }
            d
        })
        .collect()
}

/// Tools whose schema requires `app`: the ones [tools] default_app fills
/// in.
pub fn needs_app(name: &str) -> bool {
    static NAMES: std::sync::OnceLock<Vec<Cow<'static, str>>> = std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| {
            definitions()
                .into_iter()
                .filter(|d| {
                    d.input_schema
                        .get("required")
                        .and_then(Value::as_array)
                        .is_some_and(|r| r.iter().any(|v| v == "app"))
                })
                .map(|d| d.name)
                .collect()
        })
        .iter()
        .any(|n| n == name)
}

/// Tools that take an `app` argument (a script's `set_app` fills it in).
pub fn takes_app() -> &'static [Cow<'static, str>] {
    static NAMES: std::sync::OnceLock<Vec<Cow<'static, str>>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        definitions()
            .into_iter()
            .filter(|d| d.input_schema["properties"].get("app").is_some())
            .map(|d| d.name)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tools_have_object_schemas() {
        let defs = definitions();
        assert_eq!(defs.len(), 26);
        let mut names: Vec<&str> = defs.iter().map(|d| &*d.name).collect();
        let mut builtin = BUILTIN.to_vec();
        names.sort_unstable();
        builtin.sort_unstable();
        assert_eq!(names, builtin, "BUILTIN lists every tool");
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
                ToolCall::parse(&d.name, json!({})),
                Err(Error::UnknownTool(_))
            ));
        }
    }

    #[test]
    fn layers_and_objects_can_be_lines() {
        assert_eq!(
            layer_line("sun ellipse 80 20 12 12 fill #ffcc00").unwrap(),
            json!({"id": "sun", "ellipse": [80, 20, 12, 12], "fill": "#ffcc00"})
        );
        assert_eq!(
            layer_line("title text \"Hello there\" at 50 10 size 8 align center bold").unwrap(),
            json!({"id": "title", "text": "Hello there", "at": [50, 10], "size": 8,
                   "align": "center", "bold": true})
        );
        assert_eq!(
            layer_line("box rect 10 10 50 30 4 fill none line #000000 2").unwrap(),
            json!({"id": "box", "rect": [10, 10, 50, 30, 4], "fill": "none",
                   "stroke": "#000000", "width": 2})
        );
        // An id that is a field's name is quoted.
        assert_eq!(
            layer_line("\"text\" fill #000000").unwrap(),
            json!({"id": "text", "fill": "#000000"})
        );
        assert_eq!(
            object_line("seat box 0.5 0.5 0.05 at 0 0 0.45 color #884422").unwrap(),
            json!({"id": "seat", "shape": "box", "size": [0.5, 0.5, 0.05],
                   "at": [0, 0, 0.45], "color": "#884422"})
        );
        assert_eq!(
            object_line("leg-2 at 1 2 3 rotate 0 90 0").unwrap(),
            json!({"id": "leg-2", "at": [1, 2, 3], "rotate": [0, 90, 0]})
        );
        for bad in [
            "a size",
            "a size 1 2",
            "a rect x",
            "a bold maybe",
            "a line",
            "a wobble 1",
        ] {
            assert!(layer_line(bad).is_err(), "{bad}");
        }
        let args: DesignArgs = serde_json::from_value(json!({
            "name": "d", "add": ["a rect 0 0 1 1", {"id": "b", "ellipse": [1, 1, 1, 1]}]
        }))
        .unwrap();
        let add = args.add.unwrap();
        assert_eq!(add[0].rect, Some(vec![0.0, 0.0, 1.0, 1.0]));
        assert_eq!(add[1].id.as_deref(), Some("b"));
        let args: SceneArgs =
            serde_json::from_value(json!({"name": "s", "change": ["seat at 0 0 1"]})).unwrap();
        assert_eq!(args.change.unwrap()[0].at, Some([0.0, 0.0, 1.0]));
        assert_eq!(
            stroke_line("design logo step 2 fill 8").unwrap(),
            json!({"design": "logo", "step": 2, "fill": 8})
        );
        assert!(stroke_line("logo rect 1 2 3 4").is_err());
        let args: DrawArgs = serde_json::from_value(json!({
            "app": "X", "strokes": ["ellipse 100 100 40 40 fill 6", {"rect": [1, 2, 3, 4]}]
        }))
        .unwrap();
        assert_eq!(args.strokes[0].ellipse, Some([100.0, 100.0, 40.0, 40.0]));
        assert_eq!(args.strokes[0].fill, Some(6.0));
        assert_eq!(args.strokes[1].rect, Some(vec![1.0, 2.0, 3.0, 4.0]));
    }

    #[test]
    fn batch_steps_can_be_short_lines() {
        let step = |line: &str| {
            let s = parse_step(line).unwrap_or_else(|e| panic!("{e}"));
            (s.tool, s.arguments)
        };
        assert_eq!(
            step("click 12"),
            ("click".into(), json!({"element_index": 12}))
        );
        assert_eq!(
            step("click \"Save As\""),
            ("click".into(), json!({"name": "Save As"}))
        );
        assert_eq!(
            step("click Save As"),
            ("click".into(), json!({"name": "Save As"}))
        );
        assert_eq!(
            step("double 3"),
            (
                "click".into(),
                json!({"element_index": 3, "click_count": 2})
            )
        );
        assert_eq!(
            step("right 4"),
            (
                "click".into(),
                json!({"element_index": 4, "button": "right"})
            )
        );
        assert_eq!(
            step("set 4 \"Ada Lovelace\""),
            (
                "set_value".into(),
                json!({"element_index": 4, "value": "Ada Lovelace"})
            )
        );
        assert_eq!(
            step("set 4 12"),
            (
                "set_value".into(),
                json!({"element_index": 4, "value": "12"})
            )
        );
        assert_eq!(
            step("type \"hi\\n\""),
            ("type_text".into(), json!({"text": "hi\n"}))
        );
        assert_eq!(
            step("type 4 \"x y\""),
            (
                "type_text".into(),
                json!({"element_index": 4, "text": "x y"})
            )
        );
        // A number alone is the text.
        assert_eq!(step("type 42"), ("type_text".into(), json!({"text": "42"})));
        assert_eq!(
            step("key cmd+s"),
            ("press_key".into(), json!({"key": "cmd+s"}))
        );
        assert_eq!(
            step("key Down Down Return"),
            ("press_key".into(), json!({"key": "Down Down Return"}))
        );
        assert_eq!(
            step("scroll 7 down 2"),
            (
                "scroll".into(),
                json!({"element_index": 7, "direction": "down", "amount": 2.0})
            )
        );
        assert_eq!(
            step("scroll up"),
            ("scroll".into(), json!({"direction": "up"}))
        );
        assert_eq!(
            step("select 4 \"word\""),
            (
                "select_text".into(),
                json!({"element_index": 4, "text": "word"})
            )
        );
        assert_eq!(
            step("select 4"),
            ("select_text".into(), json!({"element_index": 4}))
        );
        assert_eq!(
            step("action 9 show_menu"),
            (
                "perform_secondary_action".into(),
                json!({"element_index": 9, "action": "show_menu"})
            )
        );
        assert_eq!(
            step("wait \"Saved\""),
            ("wait_for".into(), json!({"text": "Saved"}))
        );
        assert_eq!(
            step("find Total"),
            ("find_element".into(), json!({"text": "Total"}))
        );
        assert_eq!(step("look"), ("get_app_state".into(), json!({})));
        assert_eq!(
            step("click 5 expect dialog"),
            (
                "click".into(),
                json!({"element_index": 5, "expect": "dialog"})
            )
        );
        assert_eq!(
            step("set 3 \"expect\" expect \"Saved\""),
            (
                "set_value".into(),
                json!({"element_index": 3, "value": "expect", "expect": "Saved"})
            )
        );
        for bad in [
            "",
            "jump 3",
            "set x \"v\"",
            "set 3",
            "type",
            "look 3",
            "wait \"x\" expect y",
            "type \"open",
            "click 3 expect",
            "scroll 3 sideways",
        ] {
            assert!(parse_step(bad).is_err(), "{bad}");
        }
        // Lines and objects mix.
        let args: BatchArgs = serde_json::from_value(json!({
            "app": "X",
            "steps": ["click 1", {"tool": "get_app_state"}]
        }))
        .unwrap();
        assert_eq!(args.steps[0].tool, "click");
        assert_eq!(args.steps[1].tool, "get_app_state");
        assert!(serde_json::from_value::<BatchArgs>(json!({"steps": ["nope"]})).is_err());
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
            "steps": [{"tool": "list_apps"}, "click 3"], "speed": 300,
            "through_windows": true, "expect": "dialog", "grid": 50, "palette": true,
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
            "show": {"grid": true, "ids": true, "guides": true}, "export": "svg",
            "snap": "corner", "snap_radius": 8, "zoom": [5, 5], "radius": 10,
            "color": "00ff00", "tolerance": 20, "like": [0, 0, 5, 5], "near": [3, 3],
            "feature": "edge", "cells": true, "cell": "B2", "cell_size": 50
            }"#,
        )
        .unwrap();
        // The scene's add, mirror and export differ from the design's.
        let scene = json!({
            "name": "s", "ground": true, "view": "front", "look": [30, 20], "ids": false,
            "export": "obj", "remove": ["c"],
            "add": [{"id": "a", "shape": "cylinder", "size": [1, 2], "at": [0, 0, 1],
                     "rotate": [0, 90, 0], "color": "ff0000", "on": "ground", "move": [1, 0, 0]}],
            "change": [{"id": "a", "shape": "cube", "size": [1]}],
            "mirror": [{"id": "a", "as": "b", "axis": "y", "at": 1}],
            "repeat": [{"id": "a", "count": 3, "offset": [1, 0, 0]},
                       {"id": "b", "count": 4, "around": [0, 0], "angle": 180}]
        });
        // The script's show is a name, not the design's picture options.
        let script = json!({
            "code": "1 + 1", "run": "x", "args": {"a": 1}, "data": [1, 2], "save": "x",
            "description": "d", "params": {"a": {"type": "number"}}, "list": true,
            "show": "x", "delete": "x", "help": true
        });
        let decide = json!({
            "question": "q", "options": ["a", "b"], "scale": ["lo", "hi"],
            "questions": {"n": {"question": "q"}}, "state": {"any": "json"},
            "items": ["x", {"y": 1}], "app": "X", "window": "1", "pick": "the button", "read": true,
            "setup": "status"
        });
        for d in definitions() {
            let sample = match &*d.name {
                "scene" => &scene,
                "script" => &script,
                "decide" => &decide,
                _ => &full,
            };
            let mut args = serde_json::Map::new();
            for (k, _) in d.input_schema["properties"].as_object().unwrap() {
                args.insert(k.clone(), sample[k].clone());
            }
            ToolCall::parse(&d.name, Value::Object(args))
                .unwrap_or_else(|e| panic!("{}: {e}", d.name));
        }
    }

    #[test]
    fn lean_definitions_are_smaller_still_and_lose_no_tool() {
        use crate::config::{Config, DescriptionStyle, ToolPreset};
        let mut cfg = Config::default();
        let compact = definitions_from(&cfg);
        cfg.tools.descriptions = DescriptionStyle::Lean;
        let lean = definitions_from(&cfg);
        assert!(
            model_visible_len(&lean) * 10 < model_visible_len(&compact) * 8,
            "lean {} vs compact {}",
            model_visible_len(&lean),
            model_visible_len(&compact)
        );
        // decide waits for a decision model; every other tool is there.
        assert_eq!(lean.len() + 1, compact.len());
        assert!(!lean.iter().any(|d| d.name == "decide"));
        cfg.decision.provider = "jev".into();
        assert!(definitions_from(&cfg).iter().any(|d| d.name == "decide"));
        // No window; a design's layers as the list of their keys.
        let design = lean.iter().find(|d| d.name == "design").unwrap();
        let text = design.input_schema.to_string();
        assert!(
            !lean
                .iter()
                .any(|d| d.input_schema["properties"].get("window").is_some())
        );
        assert!(text.contains("keys: ") && text.contains("rect"), "{text}");
        assert!(!text.contains("\"default\""), "{text}");
        // A small set for smaller models.
        let mut small = Config::default();
        small.tools.preset = ToolPreset::Small;
        let names: Vec<String> = definitions_from(&small)
            .iter()
            .map(|d| d.name.to_string())
            .collect();
        assert_eq!(names.len(), crate::config::SMALL_TOOLS.len(), "{names:?}");
        // The app need not be named.
        let mut d = Config::default();
        d.tools.default_app = true;
        let click = definitions_from(&d)
            .into_iter()
            .find(|t| t.name == "click")
            .unwrap();
        assert!(
            !click.input_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "app")
        );
        assert!(needs_app("click") && !needs_app("screenshot") && !needs_app("list_apps"));
    }

    #[test]
    fn compact_definitions_are_smaller_and_filterable() {
        use crate::config::{DescriptionStyle, ToolsConfig};
        let full = model_visible_len(&definitions());
        let compact_cfg = ToolsConfig::default();
        assert_eq!(compact_cfg.descriptions, DescriptionStyle::Compact);
        let compact = definitions_for(&compact_cfg);
        assert_eq!(compact.len(), 26);
        let compact_len = model_visible_len(&compact);
        assert!(
            compact_len * 2 < full,
            "compact {compact_len} vs full {full}"
        );
        // Every tool has a short description, and schemas keep their types.
        for d in &compact {
            assert!(short_description(&d.name).is_some(), "{}", d.name);
            assert_eq!(d.input_schema["type"], "object");
        }
        let cfg = ToolsConfig {
            disabled: vec!["drag".into(), "batch".into()],
            ..Default::default()
        };
        let names: Vec<String> = definitions_for(&cfg)
            .iter()
            .map(|d| d.name.to_string())
            .collect();
        assert!(!names.iter().any(|n| n == "drag" || n == "batch"));
        let cfg = ToolsConfig {
            enabled: vec!["list_apps".into(), "get_app_state".into()],
            ..Default::default()
        };
        assert_eq!(definitions_for(&cfg).len(), 2);
    }

    #[test]
    fn compact_schemas_send_a_repeated_schema_once() {
        use crate::config::ToolsConfig;
        let compact = definitions_for(&ToolsConfig::default());
        for name in ["design", "scene"] {
            let d = compact.iter().find(|d| d.name == name).unwrap();
            let props = &d.input_schema["properties"];
            assert!(props["add"]["items"]["properties"].is_object(), "{name}");
            assert_eq!(
                props["change"]["items"],
                json!({"type": "object"}),
                "{name}"
            );
            assert_eq!(props["change"]["description"], "Same keys as add.");
        }
        // Full descriptions keep every schema as it is.
        let full = definitions();
        let design = full.iter().find(|d| d.name == "design").unwrap();
        assert!(design.input_schema["properties"]["change"]["items"]["properties"].is_object());
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
                within: None,
                about: None,
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
