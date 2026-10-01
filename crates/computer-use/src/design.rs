//! The design board: a picture composed of layers (shapes as `draw` takes
//! them, and text) that the model builds, sees rendered and checks before
//! anything is drawn in an app. From there it goes into the app as an
//! imported file (SVG, PNG), as numbers for the app's fields, or painted
//! step by step with `draw`.

use std::collections::HashMap;

use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::draw::{self, Affine, Frame};
use crate::overlay::text::{Fonts, layout};
use crate::tools::{DesignArgs, DesignLayer, DrawStroke, TextAlign};
use crate::types::{Capture, Point, Rect};

pub type Rgb = [u8; 3];

/// Most cells across a page (when the cell size is given).
pub const MAX_CELLS: f64 = 200.0;
/// Most layers on one page (checks compare every pair).
pub const MAX_LAYERS: usize = 500;

/// "#RRGGBB" (or "#RGB"); "none" is no colour.
pub fn parse_colour(s: &str) -> Result<Option<Rgb>, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("none") || t.eq_ignore_ascii_case("transparent") {
        return Ok(None);
    }
    let h = t.strip_prefix('#').unwrap_or(t);
    let hex = |s: &str| u8::from_str_radix(s, 16).ok();
    let rgb = match h.len() {
        6 => (|| Some([hex(&h[0..2])?, hex(&h[2..4])?, hex(&h[4..6])?]))(),
        3 => (|| {
            let d = |i: usize| hex(&h[i..=i]).map(|v| v * 17);
            Some([d(0)?, d(1)?, d(2)?])
        })(),
        _ => None,
    };
    rgb.map(Some)
        .ok_or_else(|| format!("\"{s}\" is not a colour: give \"#RRGGBB\" or \"none\""))
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub text: String,
    /// Top of the first line, and its left, centre or right (`align`).
    pub at: (f64, f64),
    pub size: f64,
    pub font: Option<String>,
    pub bold: bool,
    pub align: TextAlign,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub id: String,
    /// A shape in the design's units, then `transform` (moves, mirrors).
    pub shape: Option<DrawStroke>,
    pub transform: Affine,
    pub text: Option<Text>,
    pub fill: Option<Rgb>,
    pub stroke: Option<Rgb>,
    pub width: f64,
    pub opacity: f64,
}

impl Layer {
    /// What kind of layer it is, in a word.
    pub fn kind(&self) -> &'static str {
        let Some(s) = &self.shape else {
            return "text";
        };
        if s.rect.is_some() {
            "rect"
        } else if s.ellipse.is_some() {
            "ellipse"
        } else if s.polygon.is_some() {
            "polygon"
        } else if s.star.is_some() {
            "star"
        } else if s.arc.is_some() {
            "arc"
        } else if s.bezier.is_some() {
            "bezier"
        } else if s.points.is_some() {
            "points"
        } else {
            "curve"
        }
    }
}

/// Lines of a layer in the design's units: (points, closed).
pub type Lines = Vec<(Vec<(f64, f64)>, bool)>;

/// Fonts by family and weight, loaded once.
#[derive(Default)]
pub struct FontCache {
    loaded: HashMap<(String, bool), Fonts>,
}

impl FontCache {
    pub fn get(&mut self, family: Option<&str>, bold: bool) -> &Fonts {
        let key = (family.unwrap_or("").trim().to_lowercase(), bold);
        self.loaded.entry(key).or_insert_with(|| {
            let file = family.and_then(|f| font_file(f, bold)).unwrap_or_default();
            Fonts::load(&file)
        })
    }
}

/// The font file for a family name (and bold), if one is installed.
fn font_file(family: &str, bold: bool) -> Option<String> {
    let want: String = family
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    if want.is_empty() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let query = if bold {
            format!("{family}:bold")
        } else {
            family.to_string()
        };
        if let Ok(out) = std::process::Command::new("fc-match")
            .args(["-f", "%{file}\n%{family}", &query])
            .output()
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let mut lines = text.lines();
            let (file, found) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
            let found: String = found
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            // fc-match always answers; only take a font of that family.
            if !file.is_empty() && found.contains(&want) {
                return Some(file.to_string());
            }
        }
    }
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(target_os = "windows") {
        dirs.push("C:\\Windows\\Fonts".into());
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(std::path::PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        for d in [
            "/System/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
            "/Library/Fonts",
        ] {
            dirs.push(d.into());
        }
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(std::path::PathBuf::from(home).join("Library/Fonts"));
        }
    } else {
        for d in ["/usr/share/fonts", "/usr/local/share/fonts"] {
            dirs.push(d.into());
        }
    }
    let mut found: Vec<(i32, String)> = Vec::new();
    let mut stack = dirs;
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            seen += 1;
            if seen > 20_000 {
                break;
            }
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
                continue;
            }
            let stem: String = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            if !stem.starts_with(&want) {
                continue;
            }
            let rest = &stem[want.len()..];
            let is_bold = rest.contains("bold") || rest == "bd" || rest == "b";
            let plain = rest.is_empty() || rest == "regular";
            let score = match (bold, is_bold, plain) {
                (true, true, _) => 0,
                (false, _, true) => 0,
                (false, false, _) => 1,
                _ => 2,
            };
            found.push((score, path.to_string_lossy().into_owned()));
        }
    }
    found.sort();
    found.into_iter().next().map(|f| f.1)
}

/// What a step paints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepKind {
    /// Closed shapes painted solid (`draw` with `fill`).
    Solid,
    /// Lines with a brush this wide.
    Outline(f64),
    /// Text, typed with the app's text tool.
    Text,
}

/// One colour of the design, in painting order.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub color: Rgb,
    pub kind: StepKind,
    /// Indexes of the layers it paints.
    pub layers: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Design {
    pub width: f64,
    pub height: f64,
    pub background: Rgb,
    pub margin: f64,
    /// The graph paper's cell size (None: about eight across the page).
    pub cell: Option<f64>,
    pub layers: Vec<Layer>,
}

/// What to draw over the picture besides the design.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extras {
    pub grid: Option<f64>,
    pub ids: bool,
    pub guides: bool,
    /// The named cells (graph paper).
    pub cells: bool,
}

impl Design {
    pub fn new(width: f64, height: f64) -> Self {
        Design {
            width,
            height,
            background: [255, 255, 255],
            margin: (width.min(height) * 0.05).round(),
            cell: None,
            layers: Vec::new(),
        }
    }

    fn index(&self, id: &str) -> Result<usize, String> {
        self.layers
            .iter()
            .position(|l| l.id.eq_ignore_ascii_case(id.trim()))
            .ok_or_else(|| {
                let ids: Vec<&str> = self.layers.iter().map(|l| l.id.as_str()).collect();
                format!("no layer \"{id}\" (layers: {})", ids.join(", "))
            })
    }

    /// The frame shapes are worked out in: the design's units, with room
    /// past the page on every side.
    fn frame(&self) -> Frame {
        let g = 1000.0 / self.width.max(self.height).max(1e-9);
        Frame::window(
            Point::new(0.0, 0.0),
            g,
            -self.width * 2.0,
            self.width * 3.0,
            -self.height * 2.0,
            self.height * 3.0,
        )
    }

    /// A layer's shape as lines in the design's units.
    pub fn lines(&self, layer: &Layer) -> Result<Lines, String> {
        let Some(stroke) = &layer.shape else {
            return Ok(Vec::new());
        };
        let frame = self.frame();
        let shapes = crate::engine::draw_shapes(stroke, &frame)?;
        let named: Vec<(String, draw::Shape)> = shapes
            .into_iter()
            .map(|s| (layer.id.clone(), s.then(layer.transform)))
            .collect();
        let plan = draw::plan_labelled(&named, &frame, 2.0, 200_000)
            .map_err(|e| format!("{}: {e}", layer.id))?;
        Ok(plan
            .strokes
            .iter()
            .map(|s| {
                let pts: Vec<(f64, f64)> = s.iter().map(|p| frame.to_frame(*p)).collect();
                let closed = pts.len() >= 4 && {
                    let (a, b) = (pts[0], pts[pts.len() - 1]);
                    (a.0 - b.0).hypot(a.1 - b.1) * 1000.0 / self.width.max(self.height) <= 1.0
                };
                (pts, closed)
            })
            .collect())
    }

    /// A text layer's lines: (text, left, top, width, height).
    fn text_lines(t: &Text, fonts: &mut FontCache) -> Vec<(String, f64, f64, f64, f64)> {
        let f = fonts.get(t.font.as_deref(), t.bold);
        let mut out = Vec::new();
        let mut top = t.at.1;
        for line in t.text.split('\n') {
            let p = layout(f, line, t.size as f32);
            let (w, h) = (f64::from(p.width), f64::from(p.height).max(t.size * 1.1));
            let left = match t.align {
                TextAlign::Left => t.at.0,
                TextAlign::Center => t.at.0 - w / 2.0,
                TextAlign::Right => t.at.0 - w,
            };
            out.push((line.to_string(), left, top, w, h));
            top += h * 1.15;
        }
        out
    }

    /// The box a layer covers.
    pub fn bbox(&self, layer: &Layer, fonts: &mut FontCache) -> Option<Rect> {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        if let Some(t) = &layer.text {
            for (_, l, top, w, h) in Self::text_lines(t, fonts) {
                (x0, y0, x1, y1) = (x0.min(l), y0.min(top), x1.max(l + w), y1.max(top + h));
            }
        }
        for (pts, _) in self.lines(layer).ok()? {
            for (x, y) in pts {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        (x0 <= x1).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    fn move_layer(&mut self, i: usize, dx: f64, dy: f64) {
        let l = &mut self.layers[i];
        l.transform = l.transform.then(Affine::translate(dx, dy));
        if let Some(t) = &mut l.text {
            t.at = (t.at.0 + dx, t.at.1 + dy);
        }
    }

    /// Apply everything `args` asks for, in order: size and colours,
    /// removals, new layers, changes, mirrors, alignment, spacing, order.
    pub fn apply(&mut self, args: &DesignArgs, fonts: &mut FontCache) -> Result<(), String> {
        if let Some([w, h]) = args.size {
            if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
                return Err("size is [width, height], both positive".into());
            }
            self.width = w;
            self.height = h;
            if args.margin.is_none() {
                self.margin = (w.min(h) * 0.05).round();
            }
        }
        if let Some(b) = &args.background {
            self.background = parse_colour(b)?.unwrap_or([255, 255, 255]);
        }
        if let Some(m) = args.margin {
            if !(m.is_finite() && m >= 0.0) {
                return Err("margin must be 0 or more".into());
            }
            self.margin = m;
        }
        if let Some(c) = args.cell_size {
            if !(c.is_finite() && c >= 0.0) {
                return Err("cell_size must be 0 (cells sized for the page) or more".into());
            }
            self.cell = (c > 0.0).then_some(c);
        }
        if let Some(c) = self.cell {
            let most = (self.width / c).ceil().max((self.height / c).ceil());
            if most > MAX_CELLS {
                return Err(format!(
                    "cells of {c} make {most} across the page; at most {MAX_CELLS} (give a bigger cell_size)"
                ));
            }
        }
        for id in args.remove.iter().flatten() {
            let i = self.index(id)?;
            self.layers.remove(i);
        }
        for spec in args.add.iter().flatten() {
            if self.layers.len() >= MAX_LAYERS {
                return Err(format!(
                    "a page holds at most {MAX_LAYERS} layers: remove some, or start another design"
                ));
            }
            let id = match &spec.id {
                Some(id) => {
                    let id = id.trim().to_string();
                    if self.index(&id).is_ok() {
                        return Err(format!(
                            "there is already a layer \"{id}\": change it, or give another id"
                        ));
                    }
                    id
                }
                None => {
                    let mut n = self.layers.len() + 1;
                    while self.index(&format!("layer-{n}")).is_ok() {
                        n += 1;
                    }
                    format!("layer-{n}")
                }
            };
            let mut layer = Layer {
                id: id.clone(),
                shape: None,
                transform: Affine::IDENTITY,
                text: None,
                fill: None,
                stroke: None,
                width: 0.0,
                opacity: 1.0,
            };
            set(&mut layer, spec, true).map_err(|e| format!("{id}: {e}"))?;
            let at = match (&spec.below, &spec.above) {
                (Some(b), _) => self.index(b)?,
                (None, Some(a)) => self.index(a)? + 1,
                (None, None) => self.layers.len(),
            };
            self.layers.insert(at, layer);
            let i = self.index(&id)?;
            self.place(i, spec, fonts)?;
        }
        for spec in args.change.iter().flatten() {
            let id = spec
                .id
                .as_deref()
                .ok_or("each change needs the id of the layer to change")?;
            let i = self.index(id)?;
            set(&mut self.layers[i], spec, false).map_err(|e| format!("{id}: {e}"))?;
            self.place(i, spec, fonts)?;
            if spec.below.is_some() || spec.above.is_some() {
                let layer = self.layers.remove(i);
                let at = match (&spec.below, &spec.above) {
                    (Some(b), _) => self.index(b)?,
                    (None, Some(a)) => self.index(a)? + 1,
                    (None, None) => i,
                };
                self.layers.insert(at, layer);
            }
        }
        for m in args.mirror.iter().flatten() {
            let i = self.index(&m.id)?;
            if self.index(&m.copy).is_ok() {
                return Err(format!("there is already a layer \"{}\"", m.copy));
            }
            let vertical = match m.axis.as_deref().map(str::trim) {
                None | Some("x") => true,
                Some("y") => false,
                Some(other) => {
                    return Err(format!("mirror axis is \"x\" or \"y\", not \"{other}\""));
                }
            };
            let line = m.line.unwrap_or(if vertical {
                self.width / 2.0
            } else {
                self.height / 2.0
            });
            let mut copy = self.layers[i].clone();
            copy.id = m.copy.trim().to_string();
            let flip = if vertical {
                Affine {
                    a: -1.0,
                    e: 2.0 * line,
                    ..Affine::IDENTITY
                }
            } else {
                Affine {
                    d: -1.0,
                    f: 2.0 * line,
                    ..Affine::IDENTITY
                }
            };
            copy.transform = copy.transform.then(flip);
            if let Some(t) = &mut copy.text {
                // Text stays readable: its place is mirrored, not its letters.
                if vertical {
                    t.at.0 = 2.0 * line - t.at.0;
                    t.align = match t.align {
                        TextAlign::Left => TextAlign::Right,
                        TextAlign::Right => TextAlign::Left,
                        TextAlign::Center => TextAlign::Center,
                    };
                } else if let Some(b) = self.bbox(&self.layers[i], fonts) {
                    t.at.1 = 2.0 * line - (b.y + b.height);
                }
            }
            self.layers.insert(i + 1, copy);
        }
        for a in args.align.iter().flatten() {
            self.align(a, fonts)?;
        }
        for d in args.distribute.iter().flatten() {
            self.distribute(&d.ids, d.axis.as_deref(), fonts)?;
        }
        for o in args.order.iter().flatten() {
            let i = self.index(&o.id)?;
            let layer = self.layers.remove(i);
            let at = match o.to.trim() {
                "front" => self.layers.len(),
                "back" => 0,
                "up" => (i + 1).min(self.layers.len()),
                "down" => i.saturating_sub(1),
                other => {
                    self.layers.insert(i, layer);
                    return Err(format!(
                        "order is \"front\", \"back\", \"up\" or \"down\", not \"{other}\""
                    ));
                }
            };
            self.layers.insert(at, layer);
        }
        // Every shape must work out.
        for l in &self.layers {
            self.lines(l)?;
        }
        Ok(())
    }

    /// `move` and `to` of an added or changed layer.
    fn place(&mut self, i: usize, spec: &DesignLayer, fonts: &mut FontCache) -> Result<(), String> {
        if let Some([dx, dy]) = spec.shift {
            self.move_layer(i, dx, dy);
        }
        if let Some([x, y]) = spec.to {
            let b = self
                .bbox(&self.layers[i], fonts)
                .ok_or_else(|| format!("{} has nothing to move", self.layers[i].id))?;
            self.move_layer(i, x - b.x, y - b.y);
        }
        Ok(())
    }

    fn align(
        &mut self,
        a: &crate::tools::DesignAlign,
        fonts: &mut FontCache,
    ) -> Result<(), String> {
        let idx: Vec<usize> = a
            .ids
            .iter()
            .map(|id| self.index(id))
            .collect::<Result<_, _>>()?;
        let boxes: Vec<Rect> = idx
            .iter()
            .map(|&i| self.bbox(&self.layers[i], fonts).ok_or("an empty layer"))
            .collect::<Result<_, _>>()?;
        let to = a.to.as_deref().map(str::trim).unwrap_or("page");
        let target = match to {
            "page" => Rect::new(0.0, 0.0, self.width, self.height),
            "margins" => Rect::new(
                self.margin,
                self.margin,
                self.width - 2.0 * self.margin,
                self.height - 2.0 * self.margin,
            ),
            "each other" | "selection" => union(&boxes),
            id => {
                let j = self.index(id)?;
                self.bbox(&self.layers[j], fonts).ok_or("an empty layer")?
            }
        };
        for (&i, b) in idx.iter().zip(&boxes) {
            let dx = match a.x.as_deref().map(str::trim) {
                None => 0.0,
                Some("left") => target.x - b.x,
                Some("center") => (target.x + target.width / 2.0) - (b.x + b.width / 2.0),
                Some("right") => (target.x + target.width) - (b.x + b.width),
                Some(o) => return Err(format!("align x is left, center or right, not \"{o}\"")),
            };
            let dy = match a.y.as_deref().map(str::trim) {
                None => 0.0,
                Some("top") => target.y - b.y,
                Some("middle") => (target.y + target.height / 2.0) - (b.y + b.height / 2.0),
                Some("bottom") => (target.y + target.height) - (b.y + b.height),
                Some(o) => return Err(format!("align y is top, middle or bottom, not \"{o}\"")),
            };
            self.move_layer(i, dx, dy);
        }
        Ok(())
    }

    fn distribute(
        &mut self,
        ids: &[String],
        axis: Option<&str>,
        fonts: &mut FontCache,
    ) -> Result<(), String> {
        if ids.len() < 3 {
            return Err("distribute needs 3 or more layers".into());
        }
        let across = match axis.map(str::trim) {
            None | Some("x") => true,
            Some("y") => false,
            Some(o) => return Err(format!("distribute axis is \"x\" or \"y\", not \"{o}\"")),
        };
        let mut items: Vec<(usize, Rect)> = Vec::new();
        for id in ids {
            let i = self.index(id)?;
            let b = self.bbox(&self.layers[i], fonts).ok_or("an empty layer")?;
            items.push((i, b));
        }
        let start = |b: &Rect| if across { b.x } else { b.y };
        let size = |b: &Rect| if across { b.width } else { b.height };
        items.sort_by(|p, q| start(&p.1).total_cmp(&start(&q.1)));
        let first = start(&items[0].1);
        let last = items
            .last()
            .map(|(_, b)| start(b) + size(b))
            .unwrap_or(first);
        let total: f64 = items.iter().map(|(_, b)| size(b)).sum();
        let gap = (last - first - total) / (items.len() - 1) as f64;
        let mut pos = first;
        for (i, b) in items {
            let d = pos - start(&b);
            if across {
                self.move_layer(i, d, 0.0);
            } else {
                self.move_layer(i, 0.0, d);
            }
            pos += size(&b) + gap;
        }
        Ok(())
    }

    /// The layers, back to front, one line each.
    pub fn listing(&self, fonts: &mut FontCache) -> String {
        let mut out = Vec::new();
        for l in &self.layers {
            let b = self.bbox(l, fonts);
            let mut s = format!("{} {}", l.id, l.kind());
            if let Some(b) = b {
                s.push_str(&format!(
                    " x {:.0} y {:.0} w {:.0} h {:.0}",
                    b.x, b.y, b.width, b.height
                ));
            }
            if let Some(t) = &l.text {
                let short: String = t.text.chars().take(24).collect();
                s.push_str(&format!(" \"{short}\" size {:.0}", t.size));
            }
            if let Some(c) = l.fill {
                s.push_str(&format!(" fill {}", hex(c)));
            }
            if let Some(c) = l.stroke {
                s.push_str(&format!(" line {} {:.0}", hex(c), l.width));
            }
            if l.opacity < 1.0 {
                s.push_str(&format!(" opacity {:.2}", l.opacity));
            }
            out.push(s);
        }
        out.join("; ")
    }

    /// Paint the design `scale` pixels per unit.
    fn paint(&self, scale: f64, fonts: &mut FontCache) -> Result<Pixmap, String> {
        self.paint_view(Rect::new(0.0, 0.0, self.width, self.height), scale, fonts)
    }

    /// Paint the part `view` of the design (its units), `scale` pixels per
    /// unit.
    fn paint_view(&self, view: Rect, scale: f64, fonts: &mut FontCache) -> Result<Pixmap, String> {
        let (pw, ph) = (
            (view.width * scale).round().max(1.0) as u32,
            (view.height * scale).round().max(1.0) as u32,
        );
        let mut pix = Pixmap::new(pw, ph).ok_or("the design is too big to show")?;
        let [r, g, b] = self.background;
        pix.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        for l in &self.layers {
            let alpha = (l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
            let paint = |c: Rgb| {
                let mut p = Paint::default();
                p.set_color_rgba8(c[0], c[1], c[2], alpha);
                p.anti_alias = true;
                p
            };
            for (pts, closed) in self.lines(l)? {
                let mut pb = PathBuilder::new();
                for (k, (x, y)) in pts.iter().enumerate() {
                    let (x, y) = (((x - view.x) * scale) as f32, ((y - view.y) * scale) as f32);
                    if k == 0 {
                        pb.move_to(x, y);
                    } else {
                        pb.line_to(x, y);
                    }
                }
                if closed {
                    pb.close();
                }
                let Some(path) = pb.finish() else { continue };
                if let (Some(c), true) = (l.fill, closed) {
                    pix.fill_path(
                        &path,
                        &paint(c),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                if let Some(c) = l.stroke {
                    let stroke = Stroke {
                        width: (l.width.max(0.5) * scale) as f32,
                        line_cap: LineCap::Round,
                        line_join: LineJoin::Round,
                        ..Stroke::default()
                    };
                    pix.stroke_path(&path, &paint(c), &stroke, Transform::identity(), None);
                }
            }
            if let Some(t) = &l.text {
                let c = l.fill.unwrap_or([0, 0, 0]);
                let lines = Self::text_lines(t, fonts);
                let f = fonts.get(t.font.as_deref(), t.bold);
                for (line, left, top, _, _) in lines {
                    let p = layout(f, &line, (t.size * scale) as f32);
                    if let Some(path) = p.path {
                        pix.fill_path(
                            &path,
                            &paint(c),
                            FillRule::Winding,
                            Transform::from_translate(
                                ((left - view.x) * scale) as f32,
                                ((top - view.y) * scale) as f32,
                            ),
                            None,
                        );
                    }
                }
            }
        }
        Ok(pix)
    }

    /// The design as a picture at most `max_side` pixels across, with
    /// `extras` drawn over it.
    pub fn render(
        &self,
        max_side: u32,
        extras: Extras,
        fonts: &mut FontCache,
    ) -> Result<Capture, String> {
        let scale = f64::from(max_side.max(16)) / self.width.max(self.height).max(1e-9);
        let mut pix = self.paint(scale, fonts)?;
        if extras.guides {
            let mut p = Paint::default();
            p.set_color_rgba8(230, 0, 150, 140);
            p.anti_alias = true;
            let thin = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            let (w, h, m) = (self.width * scale, self.height * scale, self.margin * scale);
            let mut lines: Vec<((f64, f64), (f64, f64))> = vec![
                ((w / 2.0, 0.0), (w / 2.0, h)),
                ((0.0, h / 2.0), (w, h / 2.0)),
            ];
            if m > 0.0 {
                lines.extend([
                    ((m, m), (w - m, m)),
                    ((w - m, m), (w - m, h - m)),
                    ((w - m, h - m), (m, h - m)),
                    ((m, h - m), (m, m)),
                ]);
            }
            for k in [1.0, 2.0] {
                lines.push(((w * k / 3.0, 0.0), (w * k / 3.0, h)));
                lines.push(((0.0, h * k / 3.0), (w, h * k / 3.0)));
            }
            for ((x0, y0), (x1, y1)) in lines {
                let mut pb = PathBuilder::new();
                pb.move_to(x0 as f32, y0 as f32);
                pb.line_to(x1 as f32, y1 as f32);
                if let Some(path) = pb.finish() {
                    pix.stroke_path(&path, &p, &thin, Transform::identity(), None);
                }
            }
        }
        if extras.ids {
            // Where each label goes (boxes need the fonts too).
            let mut placed: Vec<(String, f64, f64)> = Vec::new();
            for l in &self.layers {
                if let Some(b) = self.bbox(l, fonts) {
                    placed.push((l.id.clone(), b.x * scale, b.y * scale));
                }
            }
            let label_fonts = fonts.get(None, true);
            let size = (14.0f64).max(f64::from(max_side) / 60.0) as f32;
            let mut halo = Paint::default();
            halo.set_color_rgba8(255, 255, 255, 230);
            halo.anti_alias = true;
            let mut ink = Paint::default();
            ink.set_color_rgba8(20, 20, 160, 255);
            ink.anti_alias = true;
            for (id, x, y) in placed {
                let p = layout(label_fonts, &id, size);
                if let Some(path) = p.path {
                    let at =
                        Transform::from_translate(x.max(0.0) as f32 + 2.0, y.max(0.0) as f32 + 2.0);
                    let halo_stroke = Stroke {
                        width: 3.0,
                        line_join: LineJoin::Round,
                        ..Stroke::default()
                    };
                    pix.stroke_path(&path, &halo, &halo_stroke, at, None);
                    pix.fill_path(&path, &ink, FillRule::Winding, at, None);
                }
            }
        }
        let mut cap = Capture {
            width: pix.width(),
            height: pix.height(),
            rgba: pix.data().to_vec(),
            bounds: Rect::new(0.0, 0.0, f64::from(pix.width()), f64::from(pix.height())),
        };
        let axis = crate::imaging::Axis {
            offset: 0.0,
            scale: 1.0 / scale,
        };
        if extras.cells {
            self.cells().draw(&mut cap, axis, axis, 1.0, None);
        }
        if let Some(step) = extras.grid {
            crate::imaging::draw_grid(&mut cap, axis, axis, step, 1.0, None);
        }
        Ok(cap)
    }

    /// The design's cells: `cell` units square, or about eight across the
    /// page.
    pub fn cells(&self) -> crate::cells::Cells {
        match self.cell {
            Some(c) => crate::cells::Cells::with_step(0.0, self.width, 0.0, self.height, false, c),
            None => crate::cells::Cells::new(0.0, self.width, 0.0, self.height, false, None),
        }
    }

    /// One cell magnified, with a fine grid in the design's units, and
    /// which layers reach into it.
    pub fn render_cell(
        &self,
        name: &str,
        fonts: &mut FontCache,
    ) -> Result<(Capture, String), String> {
        let cells = self.cells();
        let (col, row) = cells.parse(name)?;
        let sp = cells.span(col, row);
        let pad = cells.step * 0.15;
        let view = Rect::new(
            (sp.x0 - pad).max(0.0),
            (sp.y0 - pad).max(0.0),
            ((sp.x1 + pad).min(self.width) - (sp.x0 - pad).max(0.0)).max(1e-6),
            ((sp.y1 + pad).min(self.height) - (sp.y0 - pad).max(0.0)).max(1e-6),
        );
        let scale = 512.0 / view.width.max(view.height);
        let pix = self.paint_view(view, scale, fonts)?;
        let mut cap = Capture {
            width: pix.width(),
            height: pix.height(),
            rgba: pix.data().to_vec(),
            bounds: Rect::new(0.0, 0.0, f64::from(pix.width()), f64::from(pix.height())),
        };
        let fine = crate::imaging::nice_step(cells.step, 10.0);
        let (ax, ay) = (
            crate::imaging::Axis {
                offset: view.x,
                scale: 1.0 / scale,
            },
            crate::imaging::Axis {
                offset: view.y,
                scale: 1.0 / scale,
            },
        );
        let used = crate::imaging::draw_grid(&mut cap, ax, ay, fine, 1.0, None);
        // The cell's own edges, in the cells' blue.
        let (x0, x1) = (ax.pixel(sp.x0), ax.pixel(sp.x1));
        let (y0, y1) = (ay.pixel(sp.y0), ay.pixel(sp.y1));
        for t in 0..2 {
            for x in x0 as i64..=x1 as i64 {
                crate::imaging::blend(&mut cap, x, y0 as i64 + t, [40, 100, 210], 0.9);
                crate::imaging::blend(&mut cap, x, y1 as i64 - t, [40, 100, 210], 0.9);
            }
            for y in y0 as i64..=y1 as i64 {
                crate::imaging::blend(&mut cap, x0 as i64 + t, y, [40, 100, 210], 0.9);
                crate::imaging::blend(&mut cap, x1 as i64 - t, y, [40, 100, 210], 0.9);
            }
        }
        let inside: Vec<String> = self
            .layers
            .iter()
            .filter(|l| {
                self.bbox(l, fonts).is_some_and(|b| {
                    b.x < sp.x1 && sp.x0 < b.x + b.width && b.y < sp.y1 && sp.y0 < b.y + b.height
                })
            })
            .map(|l| l.id.clone())
            .collect();
        let n = |v: f64| crate::imaging::grid_label(v, fine);
        let text = format!(
            "Cell {}: x {} to {}, y {} to {} (shown {:.1} times bigger, a grid line every {}). Layers in it, back to front: {}.",
            crate::cells::Cells::name(col, row),
            n(sp.x0),
            n(sp.x1),
            n(sp.y0),
            n(sp.y1),
            scale,
            n(used),
            if inside.is_empty() {
                "none".to_string()
            } else {
                inside.join(", ")
            }
        );
        Ok((cap, text))
    }

    /// The design as a PNG file's bytes, at its own size (at most 4096
    /// pixels across).
    pub fn png(&self, fonts: &mut FontCache) -> Result<Vec<u8>, String> {
        let side = self.width.max(self.height);
        let scale = if side > 4096.0 { 4096.0 / side } else { 1.0 };
        self.paint(scale, fonts)?
            .encode_png()
            .map(crate::imaging::signed_png)
            .map_err(|e| format!("could not write the PNG: {e}"))
    }

    /// The design as SVG: rectangles and ellipses as such, other shapes as
    /// paths, text as text (so vector apps can edit it).
    pub fn svg(&self, fonts: &mut FontCache) -> Result<String, String> {
        let n = |v: f64| {
            let r = (v * 100.0).round() / 100.0;
            if r == r.trunc() {
                format!("{}", r as i64)
            } else {
                format!("{r}")
            }
        };
        let esc = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        };
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n<metadata>computer-use design board, by mhrsdev (github.com/mhrsdev/zero-use-computer)</metadata>\n<rect id=\"background\" width=\"{w}\" height=\"{h}\" fill=\"{}\"/>\n",
            hex(self.background),
            w = n(self.width),
            h = n(self.height)
        );
        for l in &self.layers {
            let mut style = String::new();
            match l.fill {
                Some(c) => style.push_str(&format!(" fill=\"{}\"", hex(c))),
                None => style.push_str(" fill=\"none\""),
            }
            if let Some(c) = l.stroke {
                style.push_str(&format!(
                    " stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
                    hex(c),
                    n(l.width)
                ));
            }
            if l.opacity < 1.0 {
                style.push_str(&format!(" opacity=\"{}\"", n(l.opacity)));
            }
            let id = esc(&l.id);
            if let Some(t) = &l.text {
                for (k, (line, left, top, w, h)) in
                    Self::text_lines(t, fonts).into_iter().enumerate()
                {
                    let rtl = unicode_bidi::BidiInfo::new(&line, None)
                        .paragraphs
                        .first()
                        .is_some_and(|p| p.level.is_rtl());
                    let x = match t.align {
                        TextAlign::Left => left,
                        TextAlign::Center => left + w / 2.0,
                        TextAlign::Right => left + w,
                    };
                    let anchor = match (t.align, rtl) {
                        (TextAlign::Center, _) => "middle",
                        (TextAlign::Left, false) | (TextAlign::Right, true) => "start",
                        _ => "end",
                    };
                    let suffix = if k == 0 {
                        String::new()
                    } else {
                        format!("-{}", k + 1)
                    };
                    out.push_str(&format!(
                        "<text id=\"{id}{suffix}\" x=\"{}\" y=\"{}\" font-size=\"{}\"{}{} text-anchor=\"{anchor}\" fill=\"{}\"{}>{}</text>\n",
                        n(x),
                        n(top + h * 0.8),
                        n(t.size),
                        t.font
                            .as_deref()
                            .map(|f| format!(" font-family=\"{}\"", esc(f)))
                            .unwrap_or_default(),
                        if t.bold { " font-weight=\"bold\"" } else { "" },
                        hex(l.fill.unwrap_or([0, 0, 0])),
                        if rtl { " direction=\"rtl\"" } else { "" },
                        esc(&line)
                    ));
                }
                continue;
            }
            let Some(s) = &l.shape else { continue };
            let plain = s.rotate.is_none() && s.repeat.is_none();
            // Moves and mirrors keep a rectangle or an ellipse square to the
            // page: write it as one (editable in vector apps).
            let tr = l.transform;
            let square = tr.b == 0.0 && tr.c == 0.0 && tr.a.abs() == 1.0 && tr.d.abs() == 1.0;
            if plain && square {
                if let Some(r) = &s.rect {
                    let rx = r.get(4).copied().unwrap_or(0.0);
                    let (ax, ay) = tr.apply(r[0], r[1]);
                    let (bx, by) = tr.apply(r[0] + r[2], r[1] + r[3]);
                    out.push_str(&format!(
                        "<rect id=\"{id}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}{style}/>\n",
                        n(ax.min(bx)),
                        n(ay.min(by)),
                        n(r[2]),
                        n(r[3]),
                        if rx > 0.0 { format!(" rx=\"{}\"", n(rx)) } else { String::new() }
                    ));
                    continue;
                }
                if let Some([cx, cy, rx, ry]) = s.ellipse {
                    let (cx, cy) = tr.apply(cx, cy);
                    out.push_str(&format!(
                        "<ellipse id=\"{id}\" cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"{style}/>\n",
                        n(cx),
                        n(cy),
                        n(rx),
                        n(ry)
                    ));
                    continue;
                }
            }
            let mut d = String::new();
            for (pts, closed) in self.lines(l)? {
                let pts = thin(&pts, 0.15);
                for (k, (x, y)) in pts.iter().enumerate() {
                    d.push_str(&format!(
                        "{}{} {} ",
                        if k == 0 { "M" } else { "L" },
                        n(*x),
                        n(*y)
                    ));
                }
                if closed {
                    d.push_str("Z ");
                }
            }
            out.push_str(&format!(
                "<path id=\"{id}\" d=\"{}\"{style}/>\n",
                d.trim_end()
            ));
        }
        out.push_str("</svg>\n");
        Ok(out)
    }

    /// The colours to paint in an app, back to front (merged where the
    /// same colour and kind follow each other).
    pub fn steps(&self) -> Vec<Step> {
        let mut steps: Vec<Step> = Vec::new();
        for (i, l) in self.layers.iter().enumerate() {
            let closed = self
                .lines(l)
                .map(|ls| ls.iter().any(|(_, c)| *c))
                .unwrap_or(false);
            let mut items: Vec<(Rgb, StepKind)> = Vec::new();
            if l.text.is_some() {
                items.push((l.fill.unwrap_or([0, 0, 0]), StepKind::Text));
            } else {
                if let (Some(c), true) = (l.fill, closed) {
                    items.push((c, StepKind::Solid));
                }
                if let Some(c) = l.stroke {
                    items.push((c, StepKind::Outline(l.width.max(0.5))));
                }
            }
            for (color, kind) in items {
                match steps.last_mut() {
                    Some(s) if s.color == color && s.kind == kind && kind != StepKind::Text => {
                        s.layers.push(i)
                    }
                    _ => steps.push(Step {
                        color,
                        kind,
                        layers: vec![i],
                    }),
                }
            }
        }
        steps
    }

    /// What a step paints: lines in the design's units, closed ones
    /// painted solid for a solid step.
    pub fn step_lines(&self, step: &Step) -> Result<Lines, String> {
        let mut out = Vec::new();
        for &i in &step.layers {
            for (pts, closed) in self.lines(&self.layers[i])? {
                match step.kind {
                    StepKind::Solid if closed => out.push((pts, true)),
                    StepKind::Outline(_) => out.push((pts, closed)),
                    _ => {}
                }
            }
        }
        Ok(out)
    }

    /// What looks wrong: things off the page or in the margins, near
    /// misses of the centre, pairs that are almost mirror images, text
    /// that is hard to read or runs into other text, too many colours.
    pub fn checks(&self, fonts: &mut FontCache) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let (w, h, m) = (self.width, self.height, self.margin);
        let boxes: Vec<Option<Rect>> = self.layers.iter().map(|l| self.bbox(l, fonts)).collect();
        let tol = (w.max(h) / 400.0).max(0.5);
        for (l, b) in self.layers.iter().zip(&boxes) {
            let Some(b) = b else { continue };
            // A background or a full-width band is meant to reach the edges.
            let spans = b.width >= w * 0.9 || b.height >= h * 0.9;
            let over = [
                ("left", -b.x),
                ("top", -b.y),
                ("right", b.x + b.width - w),
                ("bottom", b.y + b.height - h),
            ];
            let past: Vec<String> = over
                .iter()
                .filter(|(_, d)| *d > tol)
                .map(|(side, d)| format!("{d:.0} past the {side} edge"))
                .collect();
            if !past.is_empty() && !spans {
                out.push(format!("{} goes {}", l.id, past.join(" and ")));
            } else if m > 0.0 && !spans && past.is_empty() {
                let inside = [
                    ("left", m - b.x),
                    ("top", m - b.y),
                    ("right", b.x + b.width - (w - m)),
                    ("bottom", b.y + b.height - (h - m)),
                ];
                let into: Vec<String> = inside
                    .iter()
                    .filter(|(_, d)| *d > tol)
                    .map(|(side, d)| format!("{d:.0} into the {side} margin"))
                    .collect();
                if !into.is_empty() {
                    out.push(format!("{} reaches {}", l.id, into.join(" and ")));
                }
            }
            let off = b.x + b.width / 2.0 - w / 2.0;
            if off.abs() > tol && off.abs() <= w * 0.02 && !spans {
                out.push(format!(
                    "{} is {:.0} {} of the page's centre: centre it (align x center) if it should be",
                    l.id,
                    off.abs(),
                    if off > 0.0 { "right" } else { "left" }
                ));
            }
        }
        // Pairs that look meant to mirror each other (two eyes, two ears).
        for i in 0..self.layers.len() {
            for j in i + 1..self.layers.len() {
                let (Some(a), Some(b)) = (boxes[i], boxes[j]) else {
                    continue;
                };
                let (li, lj) = (&self.layers[i], &self.layers[j]);
                let similar = (a.width - b.width).abs() <= a.width.max(b.width) * 0.12
                    && (a.height - b.height).abs() <= a.height.max(b.height) * 0.12
                    && li.kind() == lj.kind()
                    && li.fill == lj.fill;
                let (ac, bc) = (
                    (a.x + a.width / 2.0, a.y + a.height / 2.0),
                    (b.x + b.width / 2.0, b.y + b.height / 2.0),
                );
                let side_by_side = (ac.1 - bc.1).abs() < a.height.max(b.height) * 0.5
                    && (ac.0 - bc.0).abs() > a.width.max(b.width) * 0.5;
                if !(similar && side_by_side) {
                    continue;
                }
                let mid = (ac.0 + bc.0) / 2.0 - w / 2.0;
                if mid.abs() > tol && mid.abs() <= w * 0.05 {
                    out.push(format!(
                        "{} and {} sit {:.0} {} of the centre line (mirror one from the other to fix it)",
                        li.id,
                        lj.id,
                        mid.abs(),
                        if mid > 0.0 { "right" } else { "left" }
                    ));
                }
                let dy = bc.1 - ac.1;
                if dy.abs() > tol {
                    out.push(format!(
                        "{} is {:.0} {} than {}",
                        lj.id,
                        dy.abs(),
                        if dy > 0.0 { "lower" } else { "higher" },
                        li.id
                    ));
                }
            }
        }
        // Text: contrast with what is behind it, and text on text.
        for (i, l) in self.layers.iter().enumerate() {
            let (Some(_), Some(b)) = (&l.text, boxes[i]) else {
                continue;
            };
            let fg = l.fill.unwrap_or([0, 0, 0]);
            let centre = (b.x + b.width / 2.0, b.y + b.height / 2.0);
            let behind = self.layers[..i]
                .iter()
                .rev()
                .find_map(|o| {
                    let c = o.fill?;
                    self.lines(o)
                        .ok()?
                        .iter()
                        .any(|(pts, closed)| *closed && contains(pts, centre))
                        .then_some(c)
                })
                .unwrap_or(self.background);
            let ratio = contrast(fg, behind);
            if ratio < 3.0 {
                out.push(format!(
                    "{} ({}) on {} is hard to read (contrast {ratio:.1}:1; aim for 4.5)",
                    l.id,
                    hex(fg),
                    hex(behind)
                ));
            }
            for (j, o) in self.layers.iter().enumerate().skip(i + 1) {
                if let (Some(_), Some(c)) = (&o.text, boxes[j])
                    && b.x < c.x + c.width
                    && c.x < b.x + b.width
                    && b.y < c.y + c.height
                    && c.y < b.y + b.height
                {
                    out.push(format!("{} and {} overlap", l.id, o.id));
                }
            }
        }
        let mut colours: Vec<Rgb> = Vec::new();
        for l in &self.layers {
            for c in [l.fill, l.stroke].into_iter().flatten() {
                if !colours.contains(&c) {
                    colours.push(c);
                }
            }
        }
        if colours.len() > 8 {
            out.push(format!(
                "{} colours: a design reads better with a few (one main, one second, one accent)",
                colours.len()
            ));
        }
        out.truncate(10);
        out
    }
}

/// Set a layer's fields from a spec (all of them when `new`).
fn set(layer: &mut Layer, spec: &DesignLayer, new: bool) -> Result<(), String> {
    let shape = shape_of(spec);
    let has_text = spec.text.is_some();
    if shape.is_some() && has_text {
        return Err("a layer is a shape or a text, not both".into());
    }
    if new && shape.is_none() && !has_text {
        return Err(
            "give a shape (rect, ellipse, polygon, star, arc, bezier, points, or x/y in t) or a text"
                .into(),
        );
    }
    if let Some(s) = shape {
        if layer.text.is_some() && !new {
            return Err(
                "this is a text layer: change its text, or remove it and add a shape".into(),
            );
        }
        layer.shape = Some(s);
        layer.transform = Affine::IDENTITY;
    } else if let Some(s) = &mut layer.shape {
        // Turning or repeating the shape that is there.
        if spec.rotate.is_some() {
            s.rotate = spec.rotate;
        }
        if spec.about.is_some() {
            s.about = spec.about;
        }
        if spec.repeat.is_some() {
            s.repeat = spec.repeat.clone();
        }
    }
    if let Some(text) = &spec.text {
        if layer.shape.is_some() && !new {
            return Err("this is a shape layer: remove it and add a text".into());
        }
        let old = layer.text.clone();
        let at = spec
            .at
            .map(|p| p.xy())
            .or(old.as_ref().map(|t| t.at))
            .ok_or("a text needs at: [x, y], where it starts")?;
        layer.text = Some(Text {
            text: text.clone(),
            at,
            size: old.as_ref().map_or(32.0, |t| t.size),
            font: old.as_ref().and_then(|t| t.font.clone()),
            bold: old.as_ref().is_some_and(|t| t.bold),
            align: old.as_ref().map_or(TextAlign::Left, |t| t.align),
        });
    }
    if let Some(t) = &mut layer.text {
        if let Some(p) = spec.at {
            t.at = p.xy();
        }
        if let Some(s) = spec.size {
            if !(s.is_finite() && s > 0.0) {
                return Err("size must be positive".into());
            }
            t.size = s;
        }
        if let Some(f) = &spec.font {
            t.font = Some(f.clone());
        }
        if let Some(b) = spec.bold {
            t.bold = b;
        }
        if let Some(a) = spec.align {
            t.align = a;
        }
    }
    if let Some(c) = &spec.fill {
        layer.fill = parse_colour(c)?;
    }
    if let Some(c) = &spec.stroke {
        layer.stroke = parse_colour(c)?;
    }
    if let Some(w) = spec.width {
        if !(w.is_finite() && w >= 0.0) {
            return Err("width must be 0 or more".into());
        }
        layer.width = w;
    }
    if let Some(o) = spec.opacity {
        if !(0.0..=1.0).contains(&o) {
            return Err("opacity is 0 to 1".into());
        }
        layer.opacity = o;
    }
    if new {
        // Sensible defaults: shapes filled black, open lines drawn 2 wide,
        // text black.
        let open = layer.shape.as_ref().is_some_and(|s| {
            s.arc.is_some()
                || s.x.is_some()
                || s.y.is_some()
                || ((s.points.is_some() || s.bezier.is_some()) && !s.closed)
        });
        if spec.fill.is_none() && spec.stroke.is_none() {
            if open {
                layer.stroke = Some([0, 0, 0]);
            } else {
                layer.fill = Some([0, 0, 0]);
            }
        }
        if layer.stroke.is_some() && spec.width.is_none() {
            layer.width = 2.0;
        }
    }
    Ok(())
}

/// The `draw` stroke a layer's shape fields make, if it has one.
fn shape_of(l: &DesignLayer) -> Option<DrawStroke> {
    let any = l.points.is_some()
        || l.x.is_some()
        || l.y.is_some()
        || l.rect.is_some()
        || l.ellipse.is_some()
        || l.polygon.is_some()
        || l.star.is_some()
        || l.arc.is_some()
        || l.bezier.is_some();
    any.then(|| DrawStroke {
        points: l.points.clone(),
        closed: l.closed.unwrap_or(false),
        smooth: l.smooth.unwrap_or(false),
        x: l.x.clone(),
        y: l.y.clone(),
        t: l.t.clone(),
        steps: l.steps,
        rect: l.rect.clone(),
        ellipse: l.ellipse,
        polygon: l.polygon,
        star: l.star,
        arc: l.arc,
        bezier: l.bezier.clone(),
        rotate: l.rotate,
        about: l.about,
        repeat: l.repeat.clone(),
        ..DrawStroke::default()
    })
}

fn union(boxes: &[Rect]) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for b in boxes {
        (x0, y0, x1, y1) = (
            x0.min(b.x),
            y0.min(b.y),
            x1.max(b.x + b.width),
            y1.max(b.y + b.height),
        );
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn contains(pts: &[(f64, f64)], p: (f64, f64)) -> bool {
    let mut inside = false;
    for (a, b) in pts.iter().zip(pts.iter().cycle().skip(1)) {
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) / (b.1 - a.1) * (b.0 - a.0) {
            inside = !inside;
        }
    }
    inside
}

/// WCAG contrast ratio of two colours (1 to 21).
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let lum = |c: Rgb| {
        let ch = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
    };
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Fewer points (Douglas–Peucker): the line stays within `tol` of the
/// original.
fn thin(pts: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    let off = |p: (f64, f64)| {
        if len < 1e-9 {
            (p.0 - a.0).hypot(p.1 - a.1)
        } else {
            ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len
        }
    };
    let (i, d) = (1..pts.len() - 1)
        .map(|i| (i, off(pts[i])))
        .fold((0, 0.0), |m, v| if v.1 > m.1 { v } else { m });
    if d <= tol {
        return vec![a, b];
    }
    let mut left = thin(&pts[..=i], tol);
    left.pop();
    left.extend(thin(&pts[i..], tol));
    left
}

/// Temporary export files: in the system's temp folder, deleted when this
/// is dropped (the server stops), and old or too many ones from any run
/// cleared before each new one.
pub struct TempFiles {
    dir: std::path::PathBuf,
    mine: Vec<std::path::PathBuf>,
    max_bytes: u64,
}

impl Default for TempFiles {
    fn default() -> Self {
        TempFiles {
            dir: std::env::temp_dir().join("computer-use-exports"),
            mine: Vec::new(),
            max_bytes: Self::MAX_BYTES,
        }
    }
}

impl TempFiles {
    /// Files older than this are deleted whoever made them.
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
    /// And the folder is kept under this size, oldest first.
    const MAX_BYTES: u64 = 200 * 1024 * 1024;

    #[cfg(test)]
    pub fn in_dir(dir: std::path::PathBuf, max_bytes: u64) -> Self {
        TempFiles {
            dir,
            mine: Vec::new(),
            max_bytes,
        }
    }

    /// Write `bytes` as a new file named after `stem` with `ext`. Never
    /// replaces a file.
    pub fn write(
        &mut self,
        stem: &str,
        ext: &str,
        bytes: &[u8],
    ) -> Result<std::path::PathBuf, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            format!(
                "can't make the temporary folder {}: {e}",
                self.dir.display()
            )
        })?;
        self.prune(bytes.len() as u64);
        let safe: String = stem
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        for n in 1..10_000 {
            let path = self.dir.join(format!("{safe}-{n}.{ext}"));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    f.write_all(bytes)
                        .map_err(|e| format!("can't write {}: {e}", path.display()))?;
                    self.mine.push(path.clone());
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("can't write {}: {e}", path.display())),
            }
        }
        Err("too many exported files; try again later".into())
    }

    /// Delete files past their age, then the oldest until `incoming` more
    /// bytes fit.
    fn prune(&mut self, incoming: u64) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let now = std::time::SystemTime::now();
        let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = Vec::new();
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let when = meta.modified().unwrap_or(now);
            if now.duration_since(when).unwrap_or_default() > Self::MAX_AGE {
                let _ = std::fs::remove_file(e.path());
                continue;
            }
            files.push((when, meta.len(), e.path()));
        }
        files.sort();
        let mut total: u64 = files.iter().map(|f| f.1).sum::<u64>() + incoming;
        for (_, len, path) in files {
            if total <= self.max_bytes {
                break;
            }
            let _ = std::fs::remove_file(&path);
            total = total.saturating_sub(len);
        }
    }
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        for p in &self.mine {
            let _ = std::fs::remove_file(p);
        }
        // The folder too, when nothing else is in it.
        let _ = std::fs::remove_dir(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{DesignAlign, DesignDistribute, DesignMirror, DesignOrder};

    fn layer(json: serde_json::Value) -> DesignLayer {
        serde_json::from_value(json).unwrap()
    }

    fn design(add: Vec<serde_json::Value>) -> (Design, FontCache) {
        let mut fonts = FontCache::default();
        let mut d = Design::new(800.0, 600.0);
        let args = DesignArgs {
            name: "t".into(),
            add: Some(add.into_iter().map(layer).collect()),
            ..DesignArgs::default()
        };
        d.apply(&args, &mut fonts).unwrap();
        (d, fonts)
    }

    fn bbox(d: &Design, fonts: &mut FontCache, id: &str) -> Rect {
        let i = d.index(id).unwrap();
        d.bbox(&d.layers[i], fonts).unwrap()
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1.0
    }

    #[test]
    fn colours_read_as_hex_or_none() {
        assert_eq!(parse_colour("#ABC"), Ok(Some([170, 187, 204])));
        assert_eq!(parse_colour("a1b2c3"), Ok(Some([161, 178, 195])));
        assert_eq!(parse_colour("none"), Ok(None));
        assert!(parse_colour("red").unwrap_err().contains("#RRGGBB"));
        assert_eq!(hex([1, 2, 255]), "#0102FF");
    }

    #[test]
    fn layers_are_added_moved_and_ordered() {
        let (mut d, mut fonts) = design(vec![
            serde_json::json!({"id": "bg", "rect": [0, 0, 800, 600], "fill": "#EEEEEE"}),
            serde_json::json!({"id": "dot", "ellipse": [100, 100, 20, 20], "fill": "#FF0000"}),
            serde_json::json!({"id": "under", "rect": [10, 10, 5, 5], "below": "dot"}),
        ]);
        let ids: Vec<&str> = d.layers.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["bg", "under", "dot"]);
        // Defaults: a closed shape is filled black.
        assert_eq!(d.layers[1].fill, Some([0, 0, 0]));
        let args = DesignArgs {
            name: "t".into(),
            change: Some(vec![
                layer(serde_json::json!({"id": "dot", "move": [10, 0]})),
                layer(serde_json::json!({"id": "under", "to": [300, 200]})),
            ]),
            order: Some(vec![DesignOrder {
                id: "dot".into(),
                to: "back".into(),
            }]),
            ..DesignArgs::default()
        };
        d.apply(&args, &mut fonts).unwrap();
        let b = bbox(&d, &mut fonts, "dot");
        assert!(near(b.x, 90.0) && near(b.y, 80.0), "{b:?}");
        let u = bbox(&d, &mut fonts, "under");
        assert!(near(u.x, 300.0) && near(u.y, 200.0), "{u:?}");
        assert_eq!(d.layers[0].id, "dot");
        // An id in use, a missing id, and an unknown colour are refused.
        for (args, want) in [
            (
                DesignArgs {
                    add: Some(vec![layer(
                        serde_json::json!({"id": "dot", "rect": [0, 0, 1, 1]}),
                    )]),
                    ..DesignArgs::default()
                },
                "already a layer",
            ),
            (
                DesignArgs {
                    remove: Some(vec!["nope".into()]),
                    ..DesignArgs::default()
                },
                "no layer \"nope\"",
            ),
            (
                DesignArgs {
                    add: Some(vec![layer(
                        serde_json::json!({"rect": [0, 0, 1, 1], "fill": "pink"}),
                    )]),
                    ..DesignArgs::default()
                },
                "not a colour",
            ),
        ] {
            let e = d.clone().apply(&args, &mut fonts).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn mirrors_align_and_spacing() {
        let (mut d, mut fonts) = design(vec![
            serde_json::json!({"id": "eye-l", "ellipse": [300, 250, 40, 30], "fill": "#333333"}),
            serde_json::json!({"id": "a", "rect": [10, 400, 50, 20]}),
            serde_json::json!({"id": "b", "rect": [100, 430, 80, 20]}),
            serde_json::json!({"id": "c", "rect": [500, 410, 60, 20]}),
        ]);
        let args = DesignArgs {
            name: "t".into(),
            mirror: Some(vec![DesignMirror {
                id: "eye-l".into(),
                copy: "eye-r".into(),
                axis: None,
                line: None,
            }]),
            align: Some(vec![DesignAlign {
                ids: vec!["a".into(), "b".into(), "c".into()],
                x: None,
                y: Some("top".into()),
                to: Some("each other".into()),
            }]),
            distribute: Some(vec![DesignDistribute {
                ids: vec!["a".into(), "b".into(), "c".into()],
                axis: None,
            }]),
            ..DesignArgs::default()
        };
        d.apply(&args, &mut fonts).unwrap();
        // The copy is the mirror image about the page's middle (x 400).
        let r = bbox(&d, &mut fonts, "eye-r");
        assert!(
            near(r.x + r.width / 2.0, 500.0) && near(r.y + r.height / 2.0, 250.0),
            "{r:?}"
        );
        let (a, b, c) = (
            bbox(&d, &mut fonts, "a"),
            bbox(&d, &mut fonts, "b"),
            bbox(&d, &mut fonts, "c"),
        );
        assert!(near(a.y, 400.0) && near(b.y, 400.0) && near(c.y, 400.0));
        // Equal gaps; the first and last stay put.
        let (g1, g2) = (b.x - (a.x + a.width), c.x - (b.x + b.width));
        assert!(
            near(a.x, 10.0) && near(c.x, 500.0) && near(g1, g2),
            "{g1} {g2}"
        );
        // A symmetric pair raises no complaint; nudge one and it does.
        assert!(!d.checks(&mut fonts).iter().any(|c| c.contains("eye")));
        let args = DesignArgs {
            name: "t".into(),
            change: Some(vec![layer(
                serde_json::json!({"id": "eye-r", "move": [6, 5]}),
            )]),
            ..DesignArgs::default()
        };
        d.apply(&args, &mut fonts).unwrap();
        let checks = d.checks(&mut fonts).join("; ");
        assert!(
            checks.contains("eye-l and eye-r sit 3 right of the centre line"),
            "{checks}"
        );
        assert!(checks.contains("eye-r is 5 lower than eye-l"), "{checks}");
    }

    #[test]
    fn checks_find_what_looks_off() {
        let (d, mut fonts) = design(vec![
            serde_json::json!({"id": "bg", "rect": [0, 0, 800, 600], "fill": "#FFFFFF"}),
            serde_json::json!({"id": "logo", "ellipse": [395, 300, 50, 50], "fill": "#0055AA"}),
            serde_json::json!({"id": "off", "rect": [760, 10, 100, 30], "fill": "#0055AA"}),
            serde_json::json!({"id": "edge", "rect": [12, 200, 50, 30], "fill": "#0055AA"}),
        ]);
        let checks = d.checks(&mut fonts).join("; ");
        assert!(
            checks.contains("logo is 5 left of the page's centre"),
            "{checks}"
        );
        assert!(
            checks.contains("off goes 60 past the right edge"),
            "{checks}"
        );
        assert!(
            checks.contains("edge reaches 18 into the left margin"),
            "{checks}"
        );
        // The background spans the page: not flagged.
        assert!(!checks.contains("bg "), "{checks}");
        assert!(contrast([0, 0, 0], [255, 255, 255]) > 20.0);
        assert!(contrast([200, 200, 200], [255, 255, 255]) < 2.0);
    }

    #[test]
    fn text_is_measured_rendered_and_checked() {
        let mut fonts = FontCache::default();
        if fonts.get(None, false).is_empty() {
            return; // no system fonts here
        }
        let (d, mut fonts) = design(vec![
            serde_json::json!({"id": "title", "text": "Grand opening", "at": [400, 50], "size": 60, "align": "center", "fill": "#DDDDDD"}),
            serde_json::json!({"id": "sub", "text": "May 3", "at": [400, 80], "size": 40, "align": "center"}),
        ]);
        let t = bbox(&d, &mut fonts, "title");
        assert!(
            near(t.x + t.width / 2.0, 400.0) && t.width > 200.0 && near(t.y, 50.0),
            "{t:?}"
        );
        let checks = d.checks(&mut fonts).join("; ");
        assert!(
            checks.contains("title (#DDDDDD) on #FFFFFF is hard to read"),
            "{checks}"
        );
        assert!(checks.contains("title and sub overlap"), "{checks}");
        let pic = d.render(400, Extras::default(), &mut fonts).unwrap();
        assert_eq!((pic.width, pic.height), (400, 300));
        // Some ink where the subtitle is (black on white).
        let dark = pic.rgba.chunks(4).filter(|p| p[0] < 100).count();
        assert!(dark > 50, "{dark}");
    }

    #[test]
    fn steps_follow_the_layers_and_merge_colours() {
        let (d, _) = design(vec![
            serde_json::json!({"id": "a", "rect": [0, 0, 100, 100], "fill": "#FF0000"}),
            serde_json::json!({"id": "b", "ellipse": [300, 300, 50, 50], "fill": "#FF0000"}),
            serde_json::json!({"id": "c", "rect": [10, 10, 20, 20], "fill": "#00FF00", "stroke": "#000000", "width": 3}),
            serde_json::json!({"id": "line", "points": [[0, 500], [800, 500]], "stroke": "#000000", "width": 3}),
            serde_json::json!({"id": "t", "text": "Hi", "at": [10, 10]}),
        ]);
        let steps = d.steps();
        let kinds: Vec<(String, StepKind, usize)> = steps
            .iter()
            .map(|s| (hex(s.color), s.kind, s.layers.len()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("#FF0000".to_string(), StepKind::Solid, 2),
                ("#00FF00".to_string(), StepKind::Solid, 1),
                ("#000000".to_string(), StepKind::Outline(3.0), 2),
                ("#000000".to_string(), StepKind::Text, 1),
            ]
        );
        // A solid step paints closed shapes; a line step all its lines.
        let solid = d.step_lines(&steps[0]).unwrap();
        assert_eq!(solid.len(), 2);
        assert!(solid.iter().all(|(_, closed)| *closed));
        let lines = d.step_lines(&steps[2]).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().any(|(_, closed)| !*closed));
    }

    #[test]
    fn svg_keeps_shapes_editable() {
        let (mut d, mut fonts) = design(vec![
            serde_json::json!({"id": "card", "rect": [100, 100, 300, 200, 16], "fill": "#FFFFFF", "stroke": "#222222", "width": 2}),
            serde_json::json!({"id": "eye", "ellipse": [200, 200, 30, 20], "fill": "#333333"}),
            serde_json::json!({"id": "star", "star": [600, 300, 80, 35, 5], "fill": "#FFCC00"}),
            serde_json::json!({"id": "t", "text": "A & B", "at": [400, 500], "size": 30, "align": "center", "font": "Georgia"}),
        ]);
        let args = DesignArgs {
            name: "t".into(),
            mirror: Some(vec![DesignMirror {
                id: "eye".into(),
                copy: "eye-2".into(),
                axis: None,
                line: None,
            }]),
            ..DesignArgs::default()
        };
        d.apply(&args, &mut fonts).unwrap();
        let svg = d.svg(&mut fonts).unwrap();
        assert!(
            svg.starts_with(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"800\" height=\"600\""
            ),
            "{svg}"
        );
        assert!(svg.contains("<rect id=\"card\" x=\"100\" y=\"100\" width=\"300\" height=\"200\" rx=\"16\" fill=\"#FFFFFF\" stroke=\"#222222\""), "{svg}");
        assert!(
            svg.contains("<ellipse id=\"eye-2\" cx=\"600\" cy=\"200\" rx=\"30\" ry=\"20\""),
            "{svg}"
        );
        // The star is a path of its ten corners (and back to the start).
        let star = svg.lines().find(|l| l.contains("id=\"star\"")).unwrap();
        assert_eq!(star.matches(" L").count(), 10, "{star}");
        assert!(
            svg.contains("font-family=\"Georgia\"") && svg.contains(">A &amp; B</text>"),
            "{svg}"
        );
        let png = d.png(&mut fonts).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn temporary_files_never_overwrite_and_go_away() {
        let dir = std::env::temp_dir().join(format!("cu-design-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut files = TempFiles::in_dir(dir.clone(), 2500);
        let a = files.write("my design", "png", &[1u8; 1000]).unwrap();
        let b = files.write("my design", "png", &[2u8; 1000]).unwrap();
        assert_ne!(a, b);
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("my-design-")
        );
        // A third would pass the size cap: the oldest goes.
        let c = files.write("x", "svg", &[3u8; 1000]).unwrap();
        assert_eq!([&a, &b, &c].iter().filter(|p| p.exists()).count(), 2);
        assert!(!a.exists());
        assert!(c.exists());
        drop(files);
        assert!(!dir.exists(), "the folder is gone with its files");
    }
}
