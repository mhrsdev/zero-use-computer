//! Pixel art for the overlay, shared by every platform: the agent's cursor
//! (with its state ring, halo and click ripple), the status label, and the
//! border edges. Platforms only place these images on screen.

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PathBuilder, Pixmap,
    Point, RadialGradient, SpreadMode, Stroke, Transform,
};

use super::text::{self, Fonts};

/// Side of the (square) cursor image at scale 1; the hotspot is its centre.
pub const CURSOR_BOX: f32 = 64.0;

/// `#RGB`, `#RRGGBB` or `#RRGGBBAA`.
pub fn parse_color(s: &str) -> Option<Color> {
    let h = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    let (r, g, b, a) = match h.len() {
        3 => {
            let d = |i: usize| {
                u8::from_str_radix(h.get(i..i + 1)?, 16)
                    .ok()
                    .map(|v| v * 17)
            };
            (d(0)?, d(1)?, d(2)?, 255)
        }
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(Color::from_rgba8(r, g, b, a))
}

fn with_alpha(c: Color, a: f32) -> Color {
    let mut c = c;
    c.set_alpha((c.alpha() * a).clamp(0.0, 1.0));
    c
}

/// Relative luminance (0 dark – 1 light).
fn luminance(c: Color) -> f32 {
    0.2126 * c.red() + 0.7152 * c.green() + 0.0722 * c.blue()
}

/// Mix `c` toward white by `t` (0–1).
pub fn lighten(c: Color, t: f32) -> Color {
    let m = |v: f32| v + (1.0 - v) * t;
    Color::from_rgba(m(c.red()), m(c.green()), m(c.blue()), c.alpha()).unwrap_or(c)
}

/// A line that stands out against `c` (for outlines).
fn contrast(c: Color) -> Color {
    if luminance(c) < 0.4 {
        Color::from_rgba8(255, 255, 255, 230)
    } else {
        Color::from_rgba8(0, 0, 0, 110)
    }
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// The agent's pointer, drawn with its hotspot (the arrow tip) at `hotspot`.
pub struct CursorArt {
    pub image: Pixmap,
    pub hotspot: (f32, f32),
}

/// Mix `c` toward black by `t` (0–1).
fn darken(c: Color, t: f32) -> Color {
    let m = |v: f32| v * (1.0 - t);
    Color::from_rgba(m(c.red()), m(c.green()), m(c.blue()), c.alpha()).unwrap_or(c)
}

/// The arrowhead outline (tip at the origin, pointing up-left), in px at
/// scale 1: tip, lower wing, inner notch, side wing.
const ARROW: [(f32, f32); 4] = [(0.0, 0.0), (5.4, 18.6), (8.6, 10.9), (18.0, 7.9)];

fn arrow_path(x: f32, y: f32, s: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for (i, (ax, ay)) in ARROW.iter().enumerate() {
        let (px, py) = (x + ax * s, y + ay * s);
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.close();
    pb.finish()
}

/// The agent's pointer: a rounded arrowhead in its own `body` colour (a
/// soft gradient, so it can't be mistaken for the real mouse), edged in
/// white and outlined in the state colour `ring`, over a soft shadow and a
/// glow in `ring`; beside it, a small name `tag` in the same colours. While
/// `ripple` (0–1) runs, a click ring expands from the tip.
pub fn cursor(
    fonts: &Fonts,
    tag: &str,
    scale: f32,
    body: Color,
    ring: Color,
    ripple: Option<f32>,
    pulse: f32,
) -> CursorArt {
    // About the size of a system pointer, so it is easy to follow.
    let s = sane_scale(scale) * 1.35;
    // Room around the tip for the glow and the ripple.
    let pad = (CURSOR_BOX / 2.0) * s;
    let tag_text = (!tag.trim().is_empty()).then(|| text::layout(fonts, tag.trim(), 10.5 * s));
    let tag_text = tag_text.filter(|t| t.width > 0.0);
    let (tag_x, tag_y) = (12.5 * s, 16.0 * s);
    let (tag_w, tag_h) = tag_text.as_ref().map_or((0.0, 0.0), |t| {
        (t.width + 12.0 * s, (t.height + 3.0 * s).max(16.0 * s))
    });
    let w = pad + pad.max(tag_x + tag_w + 4.0 * s);
    let h = pad + pad.max(tag_y + tag_h + 4.0 * s);
    let mut pm = canvas(w, h);
    let (c, id) = (pad, Transform::identity());

    // A soft glow around the tip in the state colour.
    let glow_r = (16.0 + 4.0 * pulse) * s;
    if let (Some(p), Some(shader)) = (
        PathBuilder::from_circle(c, c, glow_r),
        RadialGradient::new(
            Point::from_xy(c, c),
            Point::from_xy(c, c),
            glow_r,
            vec![
                GradientStop::new(0.0, with_alpha(ring, 0.60)),
                GradientStop::new(0.5, with_alpha(ring, 0.28)),
                GradientStop::new(1.0, with_alpha(ring, 0.0)),
            ],
            SpreadMode::Pad,
            id,
        ),
    ) {
        let glow = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        pm.fill_path(&p, &glow, FillRule::Winding, id, None);
    }

    // Click ripple.
    if let Some(t) = ripple.filter(|t| (0.0..1.0).contains(t)) {
        let r = (5.0 + 22.0 * t) * s;
        if let Some(p) = PathBuilder::from_circle(c, c, r) {
            pm.stroke_path(
                &p,
                &paint(with_alpha(ring, 1.0 - t)),
                &stroke(3.0 * s * (1.0 - 0.5 * t)),
                id,
                None,
            );
        }
        if let Some(p) = PathBuilder::from_circle(c, c, 4.0 * s * (1.0 - t)) {
            pm.fill_path(
                &p,
                &paint(with_alpha(ring, 0.8 * (1.0 - t))),
                FillRule::Winding,
                id,
                None,
            );
        }
    }

    // Soft shadow: the arrow, offset and blurred by layered strokes.
    if let Some(shadow) = arrow_path(c + 1.2 * s, c + 2.2 * s, s) {
        for (width, a) in [(7.0, 0.05), (4.5, 0.08), (2.0, 0.12)] {
            pm.stroke_path(
                &shadow,
                &paint(Color::from_rgba(0.0, 0.0, 0.0, a).unwrap_or(Color::BLACK)),
                &stroke(width * s),
                id,
                None,
            );
        }
        pm.fill_path(
            &shadow,
            &paint(Color::from_rgba(0.0, 0.0, 0.0, 0.18).unwrap_or(Color::BLACK)),
            FillRule::Winding,
            id,
            None,
        );
    }

    // The arrowhead: state-colour outline, white edge, gradient body.
    if let Some(arrow) = arrow_path(c, c, s) {
        pm.stroke_path(&arrow, &paint(ring), &stroke(5.0 * s), id, None);
        pm.stroke_path(
            &arrow,
            &paint(Color::from_rgba8(255, 255, 255, 255)),
            &stroke(2.6 * s),
            id,
            None,
        );
        let fill = LinearGradient::new(
            Point::from_xy(c, c),
            Point::from_xy(c + 12.0 * s, c + 14.0 * s),
            vec![
                GradientStop::new(0.0, lighten(body, 0.28)),
                GradientStop::new(1.0, darken(body, 0.18)),
            ],
            SpreadMode::Pad,
            id,
        )
        .map(|shader| Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        })
        .unwrap_or_else(|| paint(body));
        pm.fill_path(&arrow, &fill, FillRule::Winding, id, None);
        // Round the body's corners to match the outline.
        pm.stroke_path(&arrow, &fill, &stroke(1.2 * s), id, None);
    }

    // The name tag.
    if let Some(t) = tag_text {
        let (x, y) = (c + tag_x, c + tag_y);
        if let Some(pill) = rounded_rect(x, y, tag_w, tag_h, tag_h / 2.0) {
            pm.fill_path(&pill, &paint(body), FillRule::Winding, id, None);
            pm.stroke_path(&pill, &paint(ring), &stroke(1.5 * s), id, None);
        }
        if let Some(p) = &t.path {
            pm.fill_path(
                p,
                &paint(Color::from_rgba8(255, 255, 255, 255)),
                FillRule::Winding,
                Transform::from_translate(
                    x + (tag_w - t.width) / 2.0,
                    y + (tag_h - t.height) / 2.0,
                ),
                None,
            );
        }
    }

    CursorArt {
        image: pm,
        hotspot: (c, c),
    }
}

/// The status label: a pill with a dot in the state colour and `text`.
/// Dark state colours (e.g. black for a sensitive action) get a light pill
/// so they stand out.
pub fn label(fonts: &Fonts, text_str: &str, scale: f32, accent: Color) -> Pixmap {
    let scale = sane_scale(scale);
    let px = 13.5 * scale;
    let t = text::layout(fonts, text_str, px);
    let pad = 12.0 * scale;
    let dot = 8.0 * scale;
    let h = (t.height + 12.0 * scale).max(26.0 * scale).ceil();
    let gap = if t.width > 0.0 { 8.0 * scale } else { 0.0 };
    let w = (pad + dot + gap + t.width + pad).ceil();
    let mut pm = canvas(w, h);
    let w = pm.width() as f32;

    let dark_accent = luminance(accent) < 0.15;
    let (bg, fg) = if dark_accent {
        (
            Color::from_rgba8(248, 248, 250, 240),
            Color::from_rgba8(10, 10, 12, 255),
        )
    } else {
        (
            Color::from_rgba8(22, 22, 28, 232),
            Color::from_rgba8(255, 255, 255, 255),
        )
    };
    let r = h / 2.0;
    let id = Transform::identity();
    if let Some(body) = rounded_rect(
        0.75 * scale,
        0.75 * scale,
        w - 1.5 * scale,
        h - 1.5 * scale,
        r,
    ) {
        pm.fill_path(&body, &paint(bg), FillRule::Winding, id, None);
        pm.stroke_path(&body, &paint(accent), &stroke(1.5 * scale), id, None);
    }
    // Right-to-left text reads from the right: dot on that side.
    let (dot_x, text_x) = if t.rtl {
        (w - pad - dot / 2.0, pad)
    } else {
        (pad + dot / 2.0, pad + dot + gap)
    };
    if let Some(p) = PathBuilder::from_circle(dot_x, h / 2.0, dot / 2.0) {
        pm.fill_path(&p, &paint(accent), FillRule::Winding, id, None);
    }
    if let Some(path) = t.path {
        let top = (h - t.height) / 2.0;
        pm.fill_path(
            &path,
            &paint(fg),
            FillRule::Winding,
            Transform::from_translate(text_x, top),
            None,
        );
    }
    pm
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// A sane drawing scale: finite and within 0.1–8 (a bad setting must never
/// make the helper fail to allocate an image).
pub fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.1, 8.0)
    } else {
        1.0
    }
}

/// Largest side of any overlay image, in pixels.
const MAX_SIDE: f32 = 16_384.0;

/// A blank image of about `w`×`h` pixels (clamped to a sane size).
fn canvas(w: f32, h: f32) -> Pixmap {
    let side = |v: f32| {
        if v.is_finite() {
            v.ceil().clamp(1.0, MAX_SIDE) as u32
        } else {
            1
        }
    };
    Pixmap::new(side(w), side(h))
        .or_else(|| Pixmap::new(1, 1))
        .expect("1x1 pixmap")
}

/// How one end of a border band meets its neighbours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EdgeEnd {
    /// Nothing special: the glow runs to the end of the band.
    Plain,
    /// The first `n` px lie in a corner shared with the perpendicular band,
    /// which leaves it to this one: glow from the nearer of the two sides.
    Own(f32),
    /// The first `n` px are drawn by the perpendicular band: leave them empty.
    Skip(f32),
    /// The first `n` px reach past an outer corner: glow around the corner.
    Round(f32),
}

impl EdgeEnd {
    fn key(self) -> (u8, i32) {
        match self {
            Self::Plain => (0, 0),
            Self::Own(n) => (1, n.round() as i32),
            Self::Skip(n) => (2, n.round() as i32),
            Self::Round(n) => (3, n.round() as i32),
        }
    }

    /// The same end with its length multiplied by `k` (units → pixels).
    pub fn scale(self, k: f32) -> Self {
        match self {
            Self::Plain => Self::Plain,
            Self::Own(n) => Self::Own(n * k),
            Self::Skip(n) => Self::Skip(n * k),
            Self::Round(n) => Self::Round(n * k),
        }
    }

    /// Shorten by `cut` px taken off this end (clipped to the screen).
    pub fn cut(self, cut: f32) -> Self {
        let c = |n: f32| (n - cut.max(0.0)).max(0.0);
        match self {
            Self::Plain => Self::Plain,
            Self::Own(n) => Self::Own(c(n)),
            Self::Skip(n) => Self::Skip(c(n)),
            Self::Round(n) => Self::Round(c(n)),
        }
    }
}

/// The shape of a border band (per-pixel strength, independent of the
/// colour), so recolouring or fading it only has to tint these values.
pub struct EdgeMask {
    width: u32,
    height: u32,
    /// Strength 0–1 per pixel.
    alpha: Vec<f32>,
    /// Pixels on the contrast line (dark colours only).
    line: Vec<bool>,
}

/// What an [`EdgeMask`] depends on.
pub type EdgeKey = (u32, u32, u8, i32, bool, [(u8, i32); 2]);

impl EdgeMask {
    pub fn key(
        width: u32,
        height: u32,
        strong: u8,
        core: f32,
        dark: bool,
        ends: [EdgeEnd; 2],
    ) -> EdgeKey {
        (
            width,
            height,
            strong,
            (core * 100.0) as i32,
            dark,
            [ends[0].key(), ends[1].key()],
        )
    }
}

/// Dark colours (e.g. black for a sensitive action) get a light line.
pub fn is_dark(color: Color) -> bool {
    luminance(color) < 0.15
}

/// The shape of one border edge: a wide, soft glow strongest on side
/// `strong` (0 top, 1 right, 2 bottom, 3 left) and fading out smoothly (a
/// gaussian falloff) across the band. `core` px along the edge stay at full
/// strength (0 = none). `ends` say how each end (start = left/top) meets the
/// neighbouring bands, so the four bands join in continuously lit corners.
/// A dark colour gets a light line so it shows on dark screens too.
pub fn edge_mask(
    width: u32,
    height: u32,
    dark: bool,
    strong: u8,
    core: f32,
    ends: [EdgeEnd; 2],
) -> EdgeMask {
    let (w, h) = (
        width.clamp(1, MAX_SIDE as u32),
        height.clamp(1, MAX_SIDE as u32),
    );
    let (fw, fh) = (w as f32, h as f32);
    let horizontal = strong == 0 || strong == 2;
    let depth = if horizontal { fh } else { fw };
    let length = if horizontal { fw } else { fh };
    let core = if core.is_finite() { core.max(0.0) } else { 0.0 };
    let core = if dark { core.max(2.0) } else { core };
    const PEAK: f32 = 0.92;
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let n = (w * h) as usize;
    let (mut alpha, mut line) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let (x, y) = ((i % w as usize) as f32 + 0.5, (i / w as usize) as f32 + 0.5);
        // Distance from the strong side, and position along the edge.
        let (mut d, along) = match strong {
            0 => (y, x),
            1 => (fw - x, y),
            2 => (fh - y, x),
            _ => (x, y),
        };
        let mut skip = false;
        for (end, pos) in [(ends[0], along), (ends[1], length - along)] {
            match end {
                EdgeEnd::Own(l) if pos < l => d = d.min(pos),
                EdgeEnd::Skip(l) if pos < l => skip = true,
                EdgeEnd::Round(l) if pos < l => d = d.hypot(l - pos),
                _ => {}
            }
        }
        if skip {
            alpha.push(0.0);
            line.push(false);
            continue;
        }
        let body = if d <= core {
            1.0
        } else {
            let u = ((d - core) / (depth - core).max(1.0)).clamp(0.0, 1.0);
            // Gaussian-like falloff, forced to zero at the far side.
            ((-(u * 2.8).powi(2)).exp() * (1.0 - smooth(u).powi(3))).clamp(0.0, 1.0)
        };
        let mut a = PEAK * body;
        let on_line = dark && (d - core).abs() <= (core * 0.5).max(1.0);
        if on_line {
            a = a.max(0.85);
        }
        alpha.push(a);
        line.push(on_line);
    }
    EdgeMask {
        width: w,
        height: h,
        alpha,
        line,
    }
}

/// Paint `mask` in `color` at `opacity` (0–1).
pub fn tint(mask: &EdgeMask, color: Color, opacity: f32) -> Pixmap {
    let mut pm = canvas(mask.width as f32, mask.height as f32);
    if pm.width() != mask.width || pm.height() != mask.height {
        return pm;
    }
    let opacity = opacity.clamp(0.0, 1.0);
    let main = (color.red(), color.green(), color.blue());
    let c = contrast(color);
    let light = (c.red(), c.green(), c.blue());
    for ((px, &a), &on_line) in pm.pixels_mut().iter_mut().zip(&mask.alpha).zip(&mask.line) {
        let a = a * opacity;
        if a <= 0.0 {
            continue;
        }
        let (r, g, b) = if on_line { light } else { main };
        let a8 = (a * 255.0).round() as u8;
        let m = |v: f32| ((v * a * 255.0).round() as u8).min(a8);
        if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(m(r), m(g), m(b), a8) {
            *px = c;
        }
    }
    pm
}

/// One border edge in `color` (see [`edge_mask`]).
pub fn edge(
    width: u32,
    height: u32,
    color: Color,
    strong: u8,
    core: f32,
    ends: [EdgeEnd; 2],
) -> Pixmap {
    tint(
        &edge_mask(width, height, is_dark(color), strong, core, ends),
        color,
        1.0,
    )
}

/// Fit `text` into `max_w` pixels at `px`, cutting it with "…" if needed.
fn fit_text(fonts: &Fonts, text_str: &str, px: f32, max_w: f32) -> text::TextPath {
    let t = text::layout(fonts, text_str, px);
    if t.width <= max_w {
        return t;
    }
    let chars: Vec<char> = text_str.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let s: String = chars[..n].iter().collect::<String>() + "…";
        let t = text::layout(fonts, &s, px);
        if t.width <= max_w {
            return t;
        }
    }
    text::layout(fonts, "…", px)
}

/// Break `text` into at most `max_lines` lines of at most `max_w` pixels
/// at `px`, between words where possible; only what doesn't fit even then
/// is cut with "…" (at the end of the last line).
fn wrap_text(
    fonts: &Fonts,
    text_str: &str,
    px: f32,
    max_w: f32,
    max_lines: usize,
) -> Vec<text::TextPath> {
    let fits = |s: &str| text::layout(fonts, s, px).width <= max_w;
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text_str.split_whitespace() {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if fits(&candidate) {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        // A word wider than a line: break it between characters.
        let mut rest: Vec<char> = word.chars().collect();
        while !rest.is_empty() {
            let mut n = rest.len();
            while n > 1 && !fits(&rest[..n].iter().collect::<String>()) {
                n -= 1;
            }
            let piece: String = rest.drain(..n).collect();
            if rest.is_empty() {
                cur = piece;
            } else {
                lines.push(piece);
            }
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    let max_lines = max_lines.max(1);
    let cut = lines.len() > max_lines;
    lines.truncate(max_lines);
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == last && cut {
                fit_text(fonts, &format!("{l} …"), px, max_w)
            } else {
                fit_text(fonts, l, px, max_w)
            }
        })
        .collect()
}

/// A confirmation panel (for platforms without a native dialog): title,
/// message (wrapped over several lines, so what is asked is shown in full)
/// and two buttons. Returns the image and the button rectangles (allow,
/// deny) as (x, y, w, h) in image pixels.
#[allow(clippy::type_complexity)]
pub fn panel(
    fonts: &Fonts,
    title: &str,
    message: &str,
    allow: &str,
    deny: &str,
    accent: Color,
    scale: f32,
) -> (Pixmap, [(f32, f32, f32, f32); 2]) {
    let s = sane_scale(scale);
    let (w, pad) = (460.0 * s, 20.0 * s);
    let title_t = fit_text(fonts, title, 16.0 * s, w - 2.0 * pad);
    let msg_lines = wrap_text(fonts, message, 14.0 * s, w - 2.0 * pad, 6);
    let line_gap = 3.0 * s;
    let msg_h = msg_lines.iter().map(|t| t.height + line_gap).sum::<f32>() - line_gap;
    let (bw, bh) = (120.0 * s, 34.0 * s);
    let h = pad + title_t.height + 10.0 * s + msg_h + 18.0 * s + bh + pad;
    let mut pm = canvas(w, h);
    let id = Transform::identity();
    if let Some(bg) = rounded_rect(s, s, w - 2.0 * s, h - 2.0 * s, 14.0 * s) {
        pm.fill_path(
            &bg,
            &paint(Color::from_rgba8(24, 24, 30, 250)),
            FillRule::Winding,
            id,
            None,
        );
        pm.stroke_path(&bg, &paint(accent), &stroke(2.5 * s), id, None);
    }
    let white = paint(Color::from_rgba8(255, 255, 255, 255));
    // Right-to-left lines are aligned right.
    let x_for = |t: &text::TextPath| if t.rtl { w - pad - t.width } else { pad };
    if let Some(p) = &title_t.path {
        pm.fill_path(
            p,
            &white,
            FillRule::Winding,
            Transform::from_translate(x_for(&title_t), pad),
            None,
        );
    }
    let mut my = pad + title_t.height + 10.0 * s;
    for t in &msg_lines {
        if let Some(p) = &t.path {
            pm.fill_path(
                p,
                &paint(Color::from_rgba8(225, 225, 232, 255)),
                FillRule::Winding,
                Transform::from_translate(x_for(t), my),
                None,
            );
        }
        my += t.height + line_gap;
    }
    let by = h - pad - bh;
    let allow_r = (w - pad - bw, by, bw, bh);
    let deny_r = (w - pad - 2.0 * bw - 12.0 * s, by, bw, bh);
    for ((x, y, bw, bh), fill, label_str) in [
        (allow_r, Color::from_rgba8(46, 125, 50, 255), allow),
        (deny_r, Color::from_rgba8(70, 70, 80, 255), deny),
    ] {
        if let Some(b) = rounded_rect(x, y, bw, bh, 8.0 * s) {
            pm.fill_path(&b, &paint(fill), FillRule::Winding, id, None);
        }
        let t = fit_text(fonts, label_str, 14.0 * s, bw - 12.0 * s);
        if let Some(p) = &t.path {
            pm.fill_path(
                p,
                &white,
                FillRule::Winding,
                Transform::from_translate(x + (bw - t.width) / 2.0, y + (bh - t.height) / 2.0),
                None,
            );
        }
    }
    (pm, [allow_r, deny_r])
}

/// Premultiplied RGBA → straight BGRA, for surfaces without alpha (the
/// shape mask decides which pixels show).
pub fn to_bgra_opaque(pm: &Pixmap) -> (Vec<u8>, Vec<bool>) {
    let mut out = Vec::with_capacity(pm.data().len());
    let mut mask = Vec::with_capacity(pm.data().len() / 4);
    for px in pm.pixels() {
        let a = px.alpha();
        let c = px.demultiply();
        out.extend_from_slice(&[c.blue(), c.green(), c.red(), 255]);
        mask.push(a >= 110);
    }
    (out, mask)
}

/// Premultiplied RGBA → premultiplied BGRA (Windows layered windows, X11
/// ARGB visuals).
pub fn to_bgra_premultiplied(pm: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pm.data().len());
    for px in pm.pixels() {
        out.extend_from_slice(&[px.blue(), px.green(), px.red(), px.alpha()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse() {
        assert_eq!(
            parse_color("#ff0000").unwrap().to_color_u8().red(),
            255,
            "rrggbb"
        );
        assert_eq!(parse_color("#0f0").unwrap().to_color_u8().green(), 255);
        assert_eq!(parse_color("#00000080").unwrap().to_color_u8().alpha(), 128);
        assert!(parse_color("red").is_none());
        assert!(parse_color("#12").is_none());
    }

    #[test]
    fn sprites_render() {
        let body = parse_color("#8E24AA").unwrap();
        let ring = parse_color("#1E88E5").unwrap();
        let fonts = Fonts::load("");
        let art = cursor(&Fonts::default(), "", 1.0, body, ring, Some(0.3), 0.0);
        let c = &art.image;
        let (hx, hy) = art.hotspot;
        assert!((hx - 43.2).abs() < 0.01 && hx == hy, "{:?}", art.hotspot);
        // Just inside the tip is painted; a far corner is transparent.
        let at = |x: u32, y: u32| c.pixel(x, y).unwrap().alpha();
        assert!(at(47, 51) > 200);
        assert_eq!(at(1, 1), 0);
        // A name tag widens the image to the right, the tip stays put.
        let tagged = cursor(&fonts, "Zero", 1.0, body, ring, None, 0.0);
        if !fonts.is_empty() {
            assert!(tagged.image.width() > c.width());
        }
        assert_eq!(tagged.hotspot, art.hotspot);
        let l = label(&fonts, "Zero is using the computer", 1.0, ring);
        assert!(l.height() >= 26);
        let plain = [EdgeEnd::Plain; 2];
        let e = edge(100, 30, ring, 0, 3.0, plain);
        // Strongest at the top, faded out at the bottom.
        assert!(e.pixel(50, 0).unwrap().alpha() > 200);
        assert!(e.pixel(50, 29).unwrap().alpha() < 20);
        let dark = edge(100, 30, parse_color("#000").unwrap(), 0, 3.0, plain);
        assert!(
            dark.pixel(50, 2).unwrap().red() > 120,
            "light line on black"
        );
    }

    /// Composite `top` over `bottom` (premultiplied alpha) at one pixel.
    fn over(a: u8, b: u8) -> f32 {
        let (a, b) = (f32::from(a) / 255.0, f32::from(b) / 255.0);
        a + b * (1.0 - a)
    }

    #[test]
    fn screen_corners_stay_lit() {
        // The four bands of a 400x300 screen glow, 40 px deep.
        let c = parse_color("#1E88E5").unwrap();
        let (w, h, band) = (400, 300, 40);
        let own = [EdgeEnd::Own(band as f32); 2];
        let skip = [EdgeEnd::Skip(band as f32); 2];
        let top = edge(w, band, c, 0, 0.0, own);
        let left = edge(band, h, c, 3, 0.0, skip);
        let at = |x: u32, y: u32| {
            over(
                top.pixel(x, y).map_or(0, |p| p.alpha()),
                if x < band {
                    left.pixel(x, y).unwrap().alpha()
                } else {
                    0
                },
            )
        };
        let mid = at(w / 2, 2);
        for (x, y) in [(0, 0), (2, 2), (5, 5), (2, 20), (20, 2)] {
            let corner = at(x, y);
            assert!(corner >= mid * 0.8, "({x},{y}): {corner} vs {mid}");
        }
        // Along the left side too.
        let side = left.pixel(2, h / 2).unwrap().alpha();
        assert!(f32::from(side) / 255.0 >= mid * 0.8);
    }

    #[test]
    fn window_corners_glow_around() {
        // A band above a window reaching 30 px past its corners: past the
        // corner the glow bends around it instead of stopping.
        let c = parse_color("#1E88E5").unwrap();
        let e = edge(200, 30, c, 2, 0.0, [EdgeEnd::Round(30.0); 2]);
        let a = |x: u32, y: u32| u32::from(e.pixel(x, y).unwrap().alpha());
        // Right at the window corner, as bright as mid-edge.
        assert!(
            a(30, 29) >= a(100, 29) * 8 / 10,
            "{} {}",
            a(30, 29),
            a(100, 29)
        );
        // Fading with distance from the corner.
        assert!(a(20, 29) > a(5, 29));
        assert!(a(5, 5) < 30);
    }

    #[test]
    fn silly_scales_do_not_panic() {
        let c = parse_color("#1E88E5").unwrap();
        let fonts = Fonts::default();
        for s in [f32::INFINITY, f32::NAN, 1e6, -3.0, 0.0] {
            let art = cursor(&fonts, "Zero", s, c, c, None, 0.0);
            assert!(art.image.width() > 0);
            assert!(label(&fonts, "x", s, c).width() > 0);
            assert!(panel(&fonts, "t", "m", "a", "d", c, s).0.width() > 0);
        }
        assert!(edge(u32::MAX, 5, c, 0, f32::NAN, [EdgeEnd::Plain; 2]).width() > 0);
    }

    #[test]
    fn panel_message_wraps_instead_of_cutting() {
        let fonts = Fonts::load("");
        if fonts.is_empty() {
            return;
        }
        let long = "Waiting for your approval: press button \"Send\" in Some Very Long Application Name Mail Client";
        let lines = wrap_text(&fonts, long, 14.0, 420.0, 6);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|t| t.width <= 420.0));
        let (tall, _) = panel(&fonts, "t", long, "Allow", "Deny", Color::WHITE, 1.0);
        let (short, _) = panel(&fonts, "t", "ok", "Allow", "Deny", Color::WHITE, 1.0);
        assert!(tall.height() > short.height());
    }

    /// `OVERLAY_PREVIEW_DIR=/tmp/x cargo test -p computer-use preview -- --ignored`
    /// writes every sprite as PNG for a visual check.
    #[test]
    #[ignore]
    fn preview() {
        let Some(dir) = std::env::var_os("OVERLAY_PREVIEW_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fonts = Fonts::load("");
        let body = parse_color("#9C27B0").unwrap();
        let states = [
            ("thinking", "#D4A017", "Zero is thinking…"),
            ("working", "#1E88E5", "Zero is using the computer"),
            (
                "approval",
                "#FFE600",
                "Waiting for your approval: press \"Send\"",
            ),
            ("danger", "#000000", "Sensitive action: press \"Delete\""),
            ("error", "#E53935", "Zero hit an error"),
            ("done", "#2E7D32", "Zero is done"),
            ("rtl", "#1E88E5", "\u{05E9}\u{05DC}\u{05D5}\u{05DD} 123"),
        ];
        for (name, color, text_str) in states {
            let c = parse_color(color).unwrap();
            cursor(&fonts, "Zero", 3.0, body, c, None, 0.0)
                .image
                .save_png(dir.join(format!("cursor-{name}.png")))
                .unwrap();
            cursor(&fonts, "Zero", 3.0, body, c, Some(0.35), 0.0)
                .image
                .save_png(dir.join(format!("cursor-{name}-click.png")))
                .unwrap();
            label(&fonts, text_str, 2.0, c)
                .save_png(dir.join(format!("label-{name}.png")))
                .unwrap();
            edge(600, 40, c, 2, 3.0, [EdgeEnd::Round(40.0); 2])
                .save_png(dir.join(format!("edge-{name}.png")))
                .unwrap();
        }
        panel(
            &fonts,
            "Zero is using the computer",
            "Waiting for your approval: press \"Send\"",
            "Allow",
            "Deny",
            parse_color("#FFE600").unwrap(),
            2.0,
        )
        .0
        .save_png(dir.join("panel.png"))
        .unwrap();
    }
}
