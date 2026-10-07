//! Pixel art for the overlay, shared by every platform: the agent's cursor
//! (with its state ring, halo and click ripple), the status label, and the
//! border edges. Platforms only place these images on screen.

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PathBuilder, Pixmap,
    PixmapPaint, Point, RadialGradient, Rect as SkRect, SpreadMode, Stroke, Transform,
};

use super::pointers;
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

/// How the agent's pointer looks: the classic arrow, or one of the
/// pointers drawn in `pointers` (each with its own click).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorStyle {
    Classic,
    Crystal,
    Paper,
    Jelly,
    Ice,
    Metal,
    Orbit,
}

/// What `overlay.cursor_style` takes: "random" (a pointer picked for each
/// agent) or a style's name.
pub const CURSOR_STYLES: [&str; 8] = [
    "random", "classic", "crystal", "paper", "jelly", "ice", "metal", "orbit",
];

impl CursorStyle {
    /// The pointers "random" picks from.
    pub const POINTERS: [CursorStyle; 6] = [
        CursorStyle::Crystal,
        CursorStyle::Paper,
        CursorStyle::Jelly,
        CursorStyle::Ice,
        CursorStyle::Metal,
        CursorStyle::Orbit,
    ];

    /// A style by its name; None for "random" (or an unknown name).
    pub fn named(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "classic" => Self::Classic,
            "crystal" => Self::Crystal,
            "paper" => Self::Paper,
            "jelly" => Self::Jelly,
            "ice" => Self::Ice,
            "metal" => Self::Metal,
            "orbit" => Self::Orbit,
            _ => return None,
        })
    }

    /// One of the pointers, at random, preferring those not in `taken`
    /// (other agents').
    pub fn random(taken: &[CursorStyle]) -> Self {
        use std::hash::{BuildHasher, Hasher};
        let free: Vec<CursorStyle> = Self::POINTERS
            .into_iter()
            .filter(|p| !taken.contains(p))
            .collect();
        let pool = if free.is_empty() {
            Self::POINTERS.to_vec()
        } else {
            free
        };
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        pool[(h.finish() % pool.len() as u64) as usize]
    }
}

/// What goes on around the pointer: its lean and trail as it moves, its
/// breathing while it waits, the keys it presses, the text it types and
/// the way it scrolls.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CursorFx {
    /// Degrees, clockwise (the body swings behind a move).
    pub tilt: f32,
    /// Where the tip was a moment ago, relative to where it is (px),
    /// newest first.
    pub trail: Vec<(f32, f32)>,
    /// The phase 0–1 of its breathing while it waits.
    pub idle: Option<f32>,
    /// How calm that breathing is: 0 as the pointer was made, 1 none at all.
    pub calm: f32,
    /// Keys pressed together (one cap each: "Ctrl", "S"), and 0–1 through
    /// showing them.
    pub keys: Option<(Vec<String>, f32)>,
    /// The end of the text typed so far, and its opacity.
    pub typed: Option<(String, f32)>,
    /// Scrolling across and down (-1, 0 or 1 each), and 0–1 through it.
    pub scroll: Option<((i8, i8), f32)>,
}

impl CursorFx {
    /// Rounded to what shows, so a redraw happens only when it would look
    /// different.
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let q = |v: f32, n: f32| (v * n).round() as i32;
        q(self.tilt, 2.0).hash(&mut h);
        for (x, y) in &self.trail {
            (q(*x, 1.0), q(*y, 1.0)).hash(&mut h);
        }
        self.idle.map(|p| q(p, 32.0)).hash(&mut h);
        q(self.calm, 20.0).hash(&mut h);
        self.keys
            .as_ref()
            .map(|(k, t)| (k, q(*t, 30.0)))
            .hash(&mut h);
        self.typed
            .as_ref()
            .map(|(t, a)| (t, q(*a, 20.0)))
            .hash(&mut h);
        self.scroll.map(|(d, t)| (d, q(t, 30.0))).hash(&mut h);
        h.finish()
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
/// With another `style`, that pointer takes the arrow's place (the state
/// colour glows behind it, and the click is its own).
///
/// `fx` adds what goes on around it (see [`CursorFx`]).
#[allow(clippy::too_many_arguments)]
pub fn cursor(
    fonts: &Fonts,
    tag: &str,
    scale: f32,
    body: Color,
    ring: Color,
    ripple: Option<f32>,
    style: CursorStyle,
    fx: &CursorFx,
) -> CursorArt {
    let pose = pointers::Pose {
        tilt: fx.tilt,
        idle: fx.idle,
        calm: fx.calm,
    };
    let art = if style == CursorStyle::Classic {
        classic_cursor(fonts, tag, scale, body, ring, ripple)
    } else {
        styled_cursor(fonts, tag, scale, body, ring, ripple, style, pose)
    };
    around(art, fonts, scale, (body, ring), style, fx)
}

/// The plain arrow (see [`cursor`]).
fn classic_cursor(
    fonts: &Fonts,
    tag: &str,
    scale: f32,
    body: Color,
    ring: Color,
    ripple: Option<f32>,
) -> CursorArt {
    // About the size of a system pointer, so it is easy to follow.
    let s = scale * 1.35;
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
    let glow_r = 16.0 * s;
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

    if let Some(t) = &tag_text {
        name_tag(
            &mut pm,
            t,
            (c + tag_x, c + tag_y),
            (tag_w, tag_h),
            s,
            (body, ring),
            CursorStyle::Classic,
        );
    }

    CursorArt {
        image: pm,
        hotspot: (c, c),
    }
}

/// The name tag beside the pointer: a pill in `body`, edged in `ring`, or
/// in a drawn pointer's own material.
fn name_tag(
    pm: &mut Pixmap,
    t: &text::TextPath,
    (x, y): (f32, f32),
    (tag_w, tag_h): (f32, f32),
    s: f32,
    (body, ring): (Color, Color),
    style: CursorStyle,
) {
    let ink = themed_box(
        pm,
        style,
        (x, y, tag_w, tag_h),
        tag_h / 2.0,
        s,
        (body, ring),
    );
    if let Some(p) = &t.path {
        pm.fill_path(
            p,
            &paint(ink),
            FillRule::Winding,
            Transform::from_translate(x + (tag_w - t.width) / 2.0, y + (tag_h - t.height) / 2.0),
            None,
        );
    }
}

/// A drawn pointer: the state colour glows behind it, it clicks in its own
/// way, and the name tag sits by its lower right.
#[allow(clippy::too_many_arguments)]
fn styled_cursor(
    fonts: &Fonts,
    tag: &str,
    scale: f32,
    body: Color,
    ring: Color,
    click: Option<f32>,
    style: CursorStyle,
    pose: pointers::Pose,
) -> CursorArt {
    let s = scale * 1.35;
    let pad = (CURSOR_BOX / 2.0) * s;
    let tag_text = (!tag.trim().is_empty()).then(|| text::layout(fonts, tag.trim(), 10.5 * s));
    let tag_text = tag_text.filter(|t| t.width > 0.0);
    // Under the pointer: beside it, it would sit on a tail or a ring.
    let (tag_x, tag_y) = (STYLED_TAG.0 * s, STYLED_TAG.1 * s);
    let (tag_w, tag_h) = tag_text.as_ref().map_or((0.0, 0.0), |t| {
        (t.width + 12.0 * s, (t.height + 3.0 * s).max(16.0 * s))
    });
    // Room below and right for what a click throws out (drops, rays).
    let w = pad + (58.0 * s).max(tag_x + tag_w + 4.0 * s);
    let h = pad + (60.0 * s).max(tag_y + tag_h + 4.0 * s);
    let mut pm = canvas(w, h);
    let (c, id) = (pad, Transform::identity());

    // The state colour, glowing behind the body of the pointer.
    let (gx, gy, glow_r) = (c + 13.0 * s, c + 15.0 * s, 26.0 * s);
    if let (Some(p), Some(shader)) = (
        PathBuilder::from_circle(gx, gy, glow_r),
        RadialGradient::new(
            Point::from_xy(gx, gy),
            Point::from_xy(gx, gy),
            glow_r,
            vec![
                GradientStop::new(0.0, with_alpha(ring, 0.55)),
                GradientStop::new(0.55, with_alpha(ring, 0.22)),
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

    // Bigger than the plain arrow: there is more to see in a picture.
    pointers::draw(&mut pm, style, (c, c), s * 1.35, click, pose);

    if let Some(t) = &tag_text {
        name_tag(
            &mut pm,
            t,
            (c + tag_x, c + tag_y),
            (tag_w, tag_h),
            s,
            (body, ring),
            style,
        );
    }

    CursorArt {
        image: pm,
        hotspot: (c, c),
    }
}

/// A rounded box in the pointer's own material (the classic arrow's: in
/// `body`, edged in `ring`); returns the colour for text on it.
fn themed_box(
    pm: &mut Pixmap,
    style: CursorStyle,
    (x, y, w, h): (f32, f32, f32, f32),
    radius: f32,
    s: f32,
    (body, ring): (Color, Color),
) -> Color {
    let id = Transform::identity();
    let Some(shape) = rounded_rect(x, y, w, h, radius) else {
        return Color::WHITE;
    };
    if style == CursorStyle::Classic {
        pm.fill_path(&shape, &paint(body), FillRule::Winding, id, None);
        pm.stroke_path(&shape, &paint(ring), &stroke(1.5 * s), id, None);
        Color::WHITE
    } else {
        pointers::tag(pm, style, &shape, (x, y, w, h), s)
    }
}

/// Text at (x, y) (its box's top left) in `ink`.
fn ink_text(pm: &mut Pixmap, t: &text::TextPath, (x, y): (f32, f32), ink: Color) {
    if let Some(p) = &t.path {
        pm.fill_path(
            p,
            &paint(ink),
            FillRule::Winding,
            Transform::from_translate(x, y),
            None,
        );
    }
}

/// Keycaps for `keys` in the pointer's material, pressed down as `k` runs
/// 0–1 (they pop up, go down together, come up, fade).
fn keycaps(
    fonts: &Fonts,
    keys: &[String],
    k: f32,
    s: f32,
    style: CursorStyle,
    colors: (Color, Color),
) -> Option<Pixmap> {
    let caps: Vec<text::TextPath> = keys
        .iter()
        .map(|key| text::layout(fonts, key, 9.5 * s))
        .collect();
    let h = 17.0 * s;
    let gap = 3.0 * s;
    let widths: Vec<f32> = caps.iter().map(|t| (t.width + 9.0 * s).max(h)).collect();
    let total = widths.iter().sum::<f32>() + gap * (caps.len().saturating_sub(1)) as f32;
    let mut pm = Pixmap::new(
        (total + 4.0 * s).ceil().max(1.0) as u32,
        (h + 6.0 * s).ceil() as u32,
    )?;
    // Down between 15% and 45% of the way, gently.
    let press = ((k - 0.15) / 0.3).clamp(0.0, 1.0);
    let down = (press * std::f32::consts::PI).sin() * 2.2 * s;
    let mut x = 2.0 * s;
    for (t, w) in caps.iter().zip(&widths) {
        // The cap's side, shown below it, less of it as it goes down.
        if let Some(side) = rounded_rect(x, 3.0 * s, *w, h, 4.0 * s) {
            pm.fill_path(
                &side,
                &paint(Color::from_rgba8(0, 0, 0, 70)),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let top = 1.0 * s + down * 0.8;
        let ink = themed_box(&mut pm, style, (x, top, *w, h), 4.0 * s, s, colors);
        ink_text(
            &mut pm,
            t,
            (x + (w - t.width) / 2.0, top + (h - t.height) / 2.0),
            ink,
        );
        x += w + gap;
    }
    Some(pm)
}

/// The end of what was typed, in a bubble of the pointer's material, with
/// a caret.
fn typed_bubble(
    fonts: &Fonts,
    typed: &str,
    s: f32,
    style: CursorStyle,
    colors: (Color, Color),
) -> Option<Pixmap> {
    let t = text::layout(fonts, typed, 10.0 * s);
    let (pad, caret) = (6.0 * s, 3.0 * s);
    let (w, h) = (t.width + 2.0 * pad + caret, t.height + 5.0 * s);
    let mut pm = Pixmap::new((w + 4.0 * s).ceil() as u32, (h + 4.0 * s).ceil() as u32)?;
    let ink = themed_box(&mut pm, style, (2.0 * s, 2.0 * s, w, h), 6.0 * s, s, colors);
    let (tx, ty) = (2.0 * s + pad, 2.0 * s + (h - t.height) / 2.0);
    ink_text(&mut pm, &t, (tx, ty), ink);
    // The caret, just after the last letter.
    let cx = tx + t.width + 1.2 * s;
    if let Some(bar) = SkRect::from_xywh(cx, ty + 1.0 * s, 1.4 * s, t.height - 2.0 * s) {
        let bar = PathBuilder::from_rect(bar);
        pm.fill_path(
            &bar,
            &paint(ink),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    Some(pm)
}

/// The pointer `art` with what goes on around it (`fx`) drawn in: its
/// trail behind it, keycaps above it, typed text below its tag and arrows
/// the way it scrolls beside it. The image grows to hold them.
fn around(
    art: CursorArt,
    fonts: &Fonts,
    scale: f32,
    colors: (Color, Color),
    style: CursorStyle,
    fx: &CursorFx,
) -> CursorArt {
    let s = scale * 1.35;
    let caps = fx
        .keys
        .as_ref()
        .filter(|(k, _)| !k.is_empty())
        .and_then(|(k, t)| Some((keycaps(fonts, k, *t, s, style, colors)?, fade_in_out(*t))));
    let bubble = fx
        .typed
        .as_ref()
        .filter(|(t, a)| !t.is_empty() && *a > 0.0)
        .and_then(|(t, a)| Some((typed_bubble(fonts, t, s, style, colors)?, *a)));
    let trail = style != CursorStyle::Classic && fx.trail.len() >= 2;
    if caps.is_none() && bubble.is_none() && fx.scroll.is_none() && !trail {
        return art;
    }
    // Everything relative to the tip.
    let (hx, hy) = art.hotspot;
    let (bw, bh) = (art.image.width() as f32, art.image.height() as f32);
    let mut lo = (-hx, -hy);
    let mut hi = (bw - hx, bh - hy);
    let mut grow = |x: f32, y: f32| {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    };
    // The trail follows the body, a little below and right of the tip.
    let body = if style == CursorStyle::Classic {
        (10.0 * s, 11.0 * s)
    } else {
        (13.0 * s, 15.0 * s)
    };
    let trail_at: Vec<(f32, f32)> = fx
        .trail
        .iter()
        .map(|(x, y)| (x + body.0, y + body.1))
        .collect();
    let m = 8.0 * s;
    if trail {
        for (x, y) in &trail_at {
            grow(x - m, y - m);
            grow(x + m, y + m);
        }
    }
    let caps_at = (4.0 * s, -28.0 * s);
    if let Some((pm, _)) = &caps {
        grow(caps_at.0, caps_at.1);
        grow(
            caps_at.0 + pm.width() as f32,
            caps_at.1 + pm.height() as f32,
        );
    }
    // Under the name tag.
    let bubble_at = if style == CursorStyle::Classic {
        (20.0 * s, 36.0 * s)
    } else {
        (STYLED_TAG.0 * s, (STYLED_TAG.1 + 21.0) * s)
    };
    if let Some((pm, _)) = &bubble {
        grow(
            bubble_at.0 + pm.width() as f32,
            bubble_at.1 + pm.height() as f32,
        );
    }
    let scroll = fx.scroll.map(|((dx, dy), k)| {
        let dir = (f32::from(dx.signum()), f32::from(dy.signum()));
        let start = if dir.1 != 0.0 {
            (-15.0 * s, if dir.1 > 0.0 { 6.0 } else { 34.0 } * s)
        } else {
            (if dir.0 > 0.0 { 2.0 } else { 30.0 } * s, -12.0 * s)
        };
        (start, dir, k)
    });
    if let Some((start, dir, _)) = scroll {
        grow(start.0 - m, start.1 - m);
        let end = (start.0 + dir.0 * 14.0 * s, start.1 + dir.1 * 14.0 * s);
        grow(end.0 + m, end.1 + m);
    }
    let pad = 2.0 * s;
    let (ox, oy) = (-lo.0 + pad, -lo.1 + pad);
    let mut pm = canvas(hi.0 - lo.0 + 2.0 * pad, hi.1 - lo.1 + 2.0 * pad);
    let place = |(x, y): (f32, f32)| (x + ox, y + oy);
    if trail {
        let pts: Vec<(f32, f32)> = trail_at.iter().map(|p| place(*p)).collect();
        pointers::trail(&mut pm, style, &pts, s);
    }
    let blit = |pm: &mut Pixmap, img: &Pixmap, (x, y): (f32, f32), opacity: f32| {
        pm.draw_pixmap(
            0,
            0,
            img.as_ref(),
            &PixmapPaint {
                opacity: opacity.clamp(0.0, 1.0),
                ..PixmapPaint::default()
            },
            Transform::from_translate(x.round(), y.round()),
            None,
        );
    };
    blit(&mut pm, &art.image, (ox - hx, oy - hy), 1.0);
    if let Some((start, dir, k)) = scroll {
        pointers::chevrons(&mut pm, style, colors.1, place(start), dir, s, k);
    }
    if let Some((img, a)) = &caps {
        blit(&mut pm, img, place(caps_at), *a);
    }
    if let Some((img, a)) = &bubble {
        blit(&mut pm, img, place(bubble_at), *a);
    }
    CursorArt {
        image: pm,
        hotspot: (ox, oy),
    }
}

/// Opacity through a short show 0–1: in quickly, out at the end.
fn fade_in_out(k: f32) -> f32 {
    (k / 0.08).clamp(0.0, 1.0) * (1.0 - ((k - 0.75) / 0.25).clamp(0.0, 1.0))
}

/// The line a drag draws along `path` (screen px), in its own image: the
/// image and where its top left goes (px).
pub fn drag_line(
    path: &[(f32, f32)],
    scale: f32,
    style: CursorStyle,
    ring: Color,
    alpha: f32,
) -> (Pixmap, (f32, f32)) {
    if path.is_empty() {
        return (canvas(1.0, 1.0), (0.0, 0.0));
    }
    let s = scale * 1.35;
    let m = 14.0 * s;
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for p in path {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    }
    let (x0, y0) = (lo.0 - m, lo.1 - m);
    let mut pm = canvas(hi.0 + m - x0, hi.1 + m - y0);
    let pts: Vec<(f32, f32)> = path.iter().map(|p| (p.0 - x0, p.1 - y0)).collect();
    pointers::drag_line(&mut pm, style, ring, &pts, s, alpha);
    (pm, (x0, y0))
}

/// Where a drawn pointer's name tag goes (px at scale 1 from the tip, times
/// the pointer's 1.35): just under the pointer, clear of its tail.
const STYLED_TAG: (f32, f32) = (14.0, 44.0);

/// The status label: a pill with a dot in the state colour and `text`.
/// Dark state colours get a light pill
/// so they stand out.
pub fn label(fonts: &Fonts, text_str: &str, scale: f32, accent: Color) -> Pixmap {
    let px = 13.5 * scale;
    let t = text::layout(fonts, text_str, px);
    let pad = 12.0 * scale;
    let dot = 8.0 * scale;
    let h = (t.height + 12.0 * scale).max(26.0 * scale).ceil();
    let gap = if t.width > 0.0 { 8.0 * scale } else { 0.0 };
    let w = (pad + dot + gap + t.width + pad).ceil();
    let mut pm = canvas(w, h);

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

/// A drawing scale that can't make an image absurdly large (or empty):
/// finite and between 0.1 and 8, else 1.
pub fn sane_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.1, 8.0)
    } else {
        1.0
    }
}

/// Largest side of any overlay image, in pixels.
const MAX_SIDE: f32 = 16_384.0;

/// A blank image of about `w`×`h` pixels, clamped to 1..=16384 per side.
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

/// One border edge: a glow in `color` that is strongest (with a bright core
/// line `core` px thick) on side `strong` — 0 top, 1 right, 2 bottom, 3 left
/// — and fades out across the band. A dark colour gets a light core so it
/// shows on dark screens too.
pub fn edge(width: u32, height: u32, color: Color, strong: u8, core: f32) -> Pixmap {
    let mut pm = canvas(width as f32, height as f32);
    let (w, h) = (pm.width(), pm.height());
    let (fw, fh) = (w as f32, h as f32);
    let (from, to, depth) = match strong {
        0 => (Point::from_xy(0.0, 0.0), Point::from_xy(0.0, fh), fh),
        1 => (Point::from_xy(fw, 0.0), Point::from_xy(0.0, 0.0), fw),
        2 => (Point::from_xy(0.0, fh), Point::from_xy(0.0, 0.0), fh),
        _ => (Point::from_xy(0.0, 0.0), Point::from_xy(fw, 0.0), fw),
    };
    let core_t = (core / depth.max(1.0)).clamp(0.0, 0.9);
    let stops = vec![
        GradientStop::new(0.0, with_alpha(color, 1.0)),
        GradientStop::new(core_t, with_alpha(color, 0.92)),
        GradientStop::new((core_t + 0.12).min(0.95), with_alpha(color, 0.45)),
        GradientStop::new((core_t + 0.45).min(0.97), with_alpha(color, 0.14)),
        GradientStop::new(1.0, with_alpha(color, 0.0)),
    ];
    if let Some(shader) =
        LinearGradient::new(from, to, stops, SpreadMode::Pad, Transform::identity())
    {
        let p = Paint {
            shader,
            ..Paint::default()
        };
        if let Some(r) = SkRect::from_xywh(0.0, 0.0, fw, fh) {
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    } else {
        pm.fill(color);
    }
    // A thin contrasting line along the core keeps dark colours visible.
    if luminance(color) < 0.15 {
        let t = (core * 0.5).max(1.0);
        let r = match strong {
            0 => SkRect::from_xywh(0.0, core, fw, t),
            1 => SkRect::from_xywh(fw - core - t, 0.0, t, fh),
            2 => SkRect::from_xywh(0.0, fh - core - t, fw, t),
            _ => SkRect::from_xywh(core, 0.0, t, fh),
        };
        if let Some(r) = r {
            pm.fill_rect(r, &paint(contrast(color)), Transform::identity(), None);
        }
    }
    pm
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
    fn absurd_sizes_and_scales_are_clamped() {
        assert_eq!(sane_scale(f32::INFINITY), 1.0);
        assert_eq!(sane_scale(f32::NAN), 1.0);
        assert_eq!(sane_scale(1e9), 8.0);
        assert_eq!(sane_scale(0.0), 0.1);
        let c = canvas(f32::INFINITY, 1e12);
        assert_eq!((c.width(), c.height()), (1, 16_384));
        let blue = parse_color("#1E88E5").unwrap();
        let e = edge(0, u32::MAX, blue, 0, 3.0);
        assert_eq!((e.width(), e.height()), (1, 16_384));
    }

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
        let none = CursorFx::default();
        let art = cursor(
            &Fonts::default(),
            "",
            1.0,
            body,
            ring,
            Some(0.3),
            CursorStyle::Classic,
            &CursorFx::default(),
        );
        let c = &art.image;
        let (hx, hy) = art.hotspot;
        assert!((hx - 43.2).abs() < 0.01 && hx == hy, "{:?}", art.hotspot);
        // Just inside the tip is painted; a far corner is transparent.
        let at = |x: u32, y: u32| c.pixel(x, y).unwrap().alpha();
        assert!(at(47, 51) > 200);
        assert_eq!(at(1, 1), 0);
        // A name tag widens the image to the right, the tip stays put.
        let tagged = cursor(
            &fonts,
            "Zero",
            1.0,
            body,
            ring,
            None,
            CursorStyle::Classic,
            &none,
        );
        if !fonts.is_empty() {
            assert!(tagged.image.width() > c.width());
        }
        assert_eq!(tagged.hotspot, art.hotspot);
        let l = label(&fonts, "Zero is using the computer", 1.0, ring);
        assert!(l.height() >= 26);
        let e = edge(100, 30, ring, 0, 3.0);
        // Strongest at the top, faded out at the bottom.
        assert!(e.pixel(5, 0).unwrap().alpha() > 240);
        assert!(e.pixel(5, 29).unwrap().alpha() < 20);
        let dark = edge(100, 30, parse_color("#000").unwrap(), 0, 3.0);
        assert!(dark.pixel(5, 3).unwrap().red() > 120, "light core on black");
    }

    #[test]
    fn every_pointer_is_drawn_with_its_tip_on_the_hotspot() {
        let body = parse_color("#8E24AA").unwrap();
        let ring = parse_color("#1E88E5").unwrap();
        let fonts = Fonts::load("");
        let none = CursorFx::default();
        for style in CursorStyle::POINTERS {
            // Through a click too: each frame keeps the tip in place.
            for click in [None, Some(0.2), Some(0.5), Some(0.9)] {
                let art = cursor(&fonts, "Zero", 1.0, body, ring, click, style, &none);
                let (hx, hy) = (art.hotspot.0 as u32, art.hotspot.1 as u32);
                let a = art.image.pixel(hx + 2, hy + 3).unwrap().alpha();
                assert!(a > 150, "{style:?} {click:?}: {a} at the tip");
            }
            let art = cursor(&fonts, "Zero", 1.0, body, ring, None, style, &none);
            assert_eq!(
                art.hotspot,
                cursor(&fonts, "", 1.0, body, ring, None, style, &none).hotspot
            );
        }
    }

    #[test]
    fn what_goes_on_around_the_pointer_widens_it_and_keeps_the_tip() {
        let body = parse_color("#8E24AA").unwrap();
        let ring = parse_color("#1E88E5").unwrap();
        let fonts = Fonts::load("");
        let busy = CursorFx {
            tilt: 10.0,
            trail: vec![(-20.0, 4.0), (-40.0, 8.0), (-60.0, 12.0)],
            idle: None,
            calm: 0.0,
            keys: Some((vec!["Ctrl".into(), "S".into()], 0.3)),
            typed: Some(("hello".into(), 1.0)),
            scroll: Some(((0, 1), 0.5)),
        };
        for style in std::iter::once(CursorStyle::Classic).chain(CursorStyle::POINTERS) {
            let plain = cursor(
                &fonts,
                "Zero",
                1.0,
                body,
                ring,
                None,
                style,
                &CursorFx::default(),
            );
            let art = cursor(&fonts, "Zero", 1.0, body, ring, None, style, &busy);
            assert!(art.image.width() > plain.image.width(), "{style:?}");
            assert!(art.image.height() > plain.image.height(), "{style:?}");
            let (hx, hy) = (art.hotspot.0 as u32, art.hotspot.1 as u32);
            let a = art.image.pixel(hx + 2, hy + 3).unwrap().alpha();
            assert!(a > 150, "{style:?}: {a} at the tip");
            // Breathing never moves the tip either.
            for p in [0.0, 0.25, 0.5, 0.75] {
                let idle = CursorFx {
                    idle: Some(p),
                    ..CursorFx::default()
                };
                let art = cursor(&fonts, "Zero", 1.0, body, ring, None, style, &idle);
                let (hx, hy) = (art.hotspot.0 as u32, art.hotspot.1 as u32);
                assert!(art.image.pixel(hx + 2, hy + 3).unwrap().alpha() > 150);
            }
        }
        assert_ne!(busy.key(), CursorFx::default().key());
        let (line, at) = drag_line(
            &[(100.0, 100.0), (180.0, 60.0), (300.0, 140.0)],
            1.0,
            CursorStyle::Ice,
            ring,
            1.0,
        );
        assert!(at.0 < 100.0 && at.1 < 60.0);
        assert!(line.width() > 200 && line.height() > 80);
    }

    #[test]
    fn styles_are_named_and_random_ones_go_round() {
        assert_eq!(CursorStyle::named(" Crystal "), Some(CursorStyle::Crystal));
        assert_eq!(CursorStyle::named("classic"), Some(CursorStyle::Classic));
        assert_eq!(CursorStyle::named("random"), None);
        for name in CURSOR_STYLES.iter().skip(1) {
            assert!(CursorStyle::named(name).is_some(), "{name}");
        }
        // Each new agent gets a pointer none of the others has.
        let mut taken = Vec::new();
        for _ in 0..CursorStyle::POINTERS.len() {
            let p = CursorStyle::random(&taken);
            assert!(!taken.contains(&p) && p != CursorStyle::Classic);
            taken.push(p);
        }
        // Past six, any pointer.
        assert!(CursorStyle::POINTERS.contains(&CursorStyle::random(&taken)));
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
        let none = CursorFx::default();
        let body = parse_color("#9C27B0").unwrap();
        let states = [
            ("thinking", "#D4A017", "Zero is thinking…"),
            ("working", "#1E88E5", "Zero is using the computer"),
            ("error", "#E53935", "Zero hit an error"),
            ("done", "#2E7D32", "Zero is done"),
            ("rtl", "#1E88E5", "\u{05E9}\u{05DC}\u{05D5}\u{05DD} 123"),
        ];
        for (name, color, text_str) in states {
            let c = parse_color(color).unwrap();
            cursor(
                &fonts,
                "Zero",
                3.0,
                body,
                c,
                None,
                CursorStyle::Classic,
                &none,
            )
            .image
            .save_png(dir.join(format!("cursor-{name}.png")))
            .unwrap();
            for style in CursorStyle::POINTERS {
                cursor(&fonts, "Zero", 3.0, body, c, None, style, &none)
                    .image
                    .save_png(dir.join(format!("cursor-{name}-{style:?}.png")))
                    .unwrap();
            }
            cursor(
                &fonts,
                "Zero",
                3.0,
                body,
                c,
                Some(0.35),
                CursorStyle::Classic,
                &none,
            )
            .image
            .save_png(dir.join(format!("cursor-{name}-click.png")))
            .unwrap();
            label(&fonts, text_str, 2.0, c)
                .save_png(dir.join(format!("label-{name}.png")))
                .unwrap();
            edge(600, 40, c, 2, 3.0)
                .save_png(dir.join(format!("edge-{name}.png")))
                .unwrap();
        }
    }
}
