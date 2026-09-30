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
    let mut pm = Pixmap::new(w.ceil() as u32, h.ceil() as u32).expect("non-zero cursor size");
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
    let px = 13.5 * scale;
    let t = text::layout(fonts, text_str, px);
    let pad = 12.0 * scale;
    let dot = 8.0 * scale;
    let h = (t.height + 12.0 * scale).max(26.0 * scale).ceil();
    let gap = if t.width > 0.0 { 8.0 * scale } else { 0.0 };
    let w = (pad + dot + gap + t.width + pad).ceil();
    let mut pm = Pixmap::new(w.max(1.0) as u32, h.max(1.0) as u32).expect("label size");

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

/// One border edge: a wide, soft glow in `color`, strongest on side `strong`
/// (0 top, 1 right, 2 bottom, 3 left) and fading out smoothly (a gaussian
/// falloff) across the band. `core` px along the edge stay at full strength
/// (0 = none). Along the edge the glow also fades towards both ends, so the
/// four bands meet in rounded, seamless corners. A dark colour (e.g. black
/// for a sensitive action) gets a light line so it shows on dark screens too.
pub fn edge(width: u32, height: u32, color: Color, strong: u8, core: f32) -> Pixmap {
    let (w, h) = (width.max(1), height.max(1));
    let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("edge size");
    let (fw, fh) = (w as f32, h as f32);
    let horizontal = strong == 0 || strong == 2;
    let depth = if horizontal { fh } else { fw };
    let length = if horizontal { fw } else { fh };
    let dark = luminance(color) < 0.15;
    let core = if dark { core.max(2.0) } else { core };
    let (r, g, b) = (color.red(), color.green(), color.blue());
    const PEAK: f32 = 0.92;
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let width_px = w as usize;
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let (x, y) = ((i % width_px) as f32 + 0.5, (i / width_px) as f32 + 0.5);
        // Distance from the strong side, and position along the edge.
        let (d, along) = match strong {
            0 => (y, x),
            1 => (fw - x, y),
            2 => (fh - y, x),
            _ => (x, y),
        };
        let body = if d <= core {
            1.0
        } else {
            let u = ((d - core) / (depth - core).max(1.0)).clamp(0.0, 1.0);
            // Gaussian-like falloff, forced to zero at the far side.
            ((-(u * 2.8).powi(2)).exp() * (1.0 - smooth(u).powi(3))).clamp(0.0, 1.0)
        };
        let end = (along.min(length - along) / depth.min(length / 2.0)).clamp(0.0, 1.0);
        let mut a = PEAK * body * smooth(end.max(0.0));
        let (mut cr, mut cg, mut cb) = (r, g, b);
        if dark && (d - core).abs() <= (core * 0.5).max(1.0) {
            let c = contrast(color);
            (cr, cg, cb) = (c.red(), c.green(), c.blue());
            a = a.max(0.85 * smooth(end));
        }
        let a8 = (a * 255.0).round() as u8;
        let m = |v: f32| ((v * a * 255.0).round() as u8).min(a8);
        if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(m(cr), m(cg), m(cb), a8) {
            *px = c;
        }
    }
    pm
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

/// A confirmation panel (for platforms without a native dialog): title,
/// message and two buttons. Returns the image and the button rectangles
/// (allow, deny) as (x, y, w, h) in image pixels.
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
    let s = scale;
    let (w, pad) = (460.0 * s, 20.0 * s);
    let title_t = fit_text(fonts, title, 16.0 * s, w - 2.0 * pad);
    let msg_t = fit_text(fonts, message, 14.0 * s, w - 2.0 * pad);
    let (bw, bh) = (120.0 * s, 34.0 * s);
    let h = pad + title_t.height + 10.0 * s + msg_t.height + 18.0 * s + bh + pad;
    let mut pm = Pixmap::new(w.ceil() as u32, h.ceil() as u32).expect("panel size");
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
    let my = pad + title_t.height + 10.0 * s;
    if let Some(p) = &msg_t.path {
        pm.fill_path(
            p,
            &paint(Color::from_rgba8(225, 225, 232, 255)),
            FillRule::Winding,
            Transform::from_translate(x_for(&msg_t), my),
            None,
        );
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
        let e = edge(100, 30, ring, 0, 3.0);
        // Strongest at the top, faded out at the bottom.
        assert!(e.pixel(50, 0).unwrap().alpha() > 200);
        assert!(e.pixel(50, 29).unwrap().alpha() < 20);
        // Faded towards the ends, so neighbouring bands blend at the corners.
        assert!(e.pixel(0, 0).unwrap().alpha() < e.pixel(50, 0).unwrap().alpha() / 2);
        let dark = edge(100, 30, parse_color("#000").unwrap(), 0, 3.0);
        assert!(
            dark.pixel(50, 2).unwrap().red() > 120,
            "light line on black"
        );
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
            ("fa", "#1E88E5", "زیرو در حال استفاده از رایانه است"),
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
            edge(600, 40, c, 2, 3.0)
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
        panel(
            &fonts,
            "زیرو در حال استفاده از رایانه است",
            "منتظر تأیید شما: فشردن دکمهٔ «ارسال»",
            "اجازه",
            "رد",
            parse_color("#FFE600").unwrap(),
            2.0,
        )
        .0
        .save_png(dir.join("panel-fa.png"))
        .unwrap();
    }
}
