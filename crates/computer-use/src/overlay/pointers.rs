//! The agent's pointers: crystal, paper, jelly, ice, liquid metal and orbit.
//! Each is a picture (3D renders, cut out with their glow, in `assets/`),
//! and each clicks its own way by moving its own picture: the jelly squashes
//! and wobbles back, the ice melts down and drips, the paper's fold opens
//! out, the metal splashes, the crystal flashes with light, the orbit's
//! pearl races round its ring.

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient,
    Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Rect, SpreadMode,
    Stroke, Transform,
};

use super::draw::CursorStyle;

/// The pictures were cut from 512 px renders and shrunk by this much; the
/// numbers below for parts of a picture (a fold, a ring) are on the renders.
const SHRINK: f32 = 256.0 / 435.0;
/// Px at scale 1 per picture px: the tallest picture is about 32 px.
const UNIT: f32 = 32.2 / 256.0;

/// A picture: where its tip is (picture px), and where its top left corner
/// was on the render (render px).
struct Picture {
    image: Pixmap,
    tip: (f32, f32),
    origin: (f32, f32),
    /// The picture halved again and again (`mips[0]` is the picture), each
    /// with its solid part's outline in white and in black: a picture
    /// shrunk eightfold in one step loses its edges and its fine light to
    /// speckle, from a level near its size it stays clean.
    mips: Vec<Level>,
}

/// One size of a picture: the picture, and its solid part in white and in
/// black (for the rim that keeps it clear on any background).
struct Level {
    image: Pixmap,
    white: Pixmap,
    black: Pixmap,
}

impl Picture {
    fn new(image: Pixmap, tip: (f32, f32), origin: (f32, f32)) -> Picture {
        let mut mips = vec![level(image.clone())];
        while let Some(last) = mips.last() {
            if last.image.width() <= 24 || last.image.height() <= 24 {
                break;
            }
            match halve(&last.image) {
                Some(next) => mips.push(level(next)),
                None => break,
            }
        }
        Picture {
            image,
            tip,
            origin,
            mips,
        }
    }
}

/// Half the size, each pixel the average of four (premultiplied).
fn halve(src: &Pixmap) -> Option<Pixmap> {
    let (w, h) = (src.width() / 2, src.height() / 2);
    let mut out = Pixmap::new(w.max(1), h.max(1))?;
    let (from, sw) = (src.data(), src.width() as usize);
    let to = out.data_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..4 {
                let at = |dx: usize, dy: usize| {
                    u32::from(from[((2 * y + dy) * sw + 2 * x + dx) * 4 + c])
                };
                let sum = at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1);
                to[(y * w as usize + x) * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    Some(out)
}

/// A level of `image`: it and its solid part (not the soft glow around
/// it) in white and in black.
fn level(image: Pixmap) -> Level {
    let solid = |v: u8| -> Pixmap {
        let mut out = image.clone();
        for px in out.data_mut().chunks_mut(4) {
            let a = f32::from(px[3]) / 255.0;
            let k = ((a - 0.35) / 0.35).clamp(0.0, 1.0);
            let a = (k * k * (3.0 - 2.0 * k) * 255.0).round() as u8;
            let c = (u16::from(v) * u16::from(a) / 255) as u8;
            px.copy_from_slice(&[c, c, c, a]);
        }
        out
    };
    let (white, black) = (solid(255), solid(0));
    Level {
        image,
        white,
        black,
    }
}

/// A picture's file: its style, bytes, tip and top left corner (as above).
type File = (CursorStyle, &'static [u8], (f32, f32), (f32, f32));

const FILES: [File; 6] = [
    (
        CursorStyle::Crystal,
        include_bytes!("../../assets/cursors/crystal.png"),
        (8.8, 7.7),
        (81.0, 39.0),
    ),
    (
        CursorStyle::Paper,
        include_bytes!("../../assets/cursors/paper.png"),
        (4.7, 4.7),
        (76.0, 47.0),
    ),
    (
        CursorStyle::Jelly,
        include_bytes!("../../assets/cursors/jelly.png"),
        (8.8, 7.1),
        (63.0, 40.0),
    ),
    (
        CursorStyle::Ice,
        include_bytes!("../../assets/cursors/ice.png"),
        (9.4, 6.5),
        (78.0, 22.0),
    ),
    (
        CursorStyle::Metal,
        include_bytes!("../../assets/cursors/metal.png"),
        (8.2, 6.5),
        (73.0, 18.0),
    ),
    (
        CursorStyle::Orbit,
        include_bytes!("../../assets/cursors/orbit.png"),
        (7.7, 5.3),
        (69.0, 22.0),
    ),
];

fn picture(style: CursorStyle) -> Option<&'static Picture> {
    static PICTURES: OnceLock<Vec<(CursorStyle, Picture)>> = OnceLock::new();
    PICTURES
        .get_or_init(|| {
            FILES
                .iter()
                .filter_map(|(style, png, tip, origin)| {
                    let image = Pixmap::decode_png(png).ok()?;
                    Some((*style, Picture::new(image, *tip, *origin)))
                })
                .collect()
        })
        .iter()
        .find(|(s, _)| *s == style)
        .map(|(_, p)| p)
}

/// How a pointer stands at a moment: leaning as it moves (degrees,
/// clockwise: the body swings behind), and breathing while it waits (the
/// phase 0–1 of a slow cycle).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct Pose {
    pub tilt: f32,
    pub idle: Option<f32>,
}

/// Draw `style` with its tip at `tip` (px), `s` px per unit; `click` runs
/// 0–1 through a click.
pub(super) fn draw(
    pm: &mut Pixmap,
    style: CursorStyle,
    tip: (f32, f32),
    s: f32,
    click: Option<f32>,
    pose: Pose,
) {
    let Some(pic) = picture(style) else {
        return;
    };
    // Waiting, each breathes its own way (about the tip, which stays put).
    let wave = pose.idle.map_or(0.0, |p| (p * TAU).sin());
    let breath = match style {
        CursorStyle::Jelly => Transform::from_scale(1.0 + 0.035 * wave, 1.0 - 0.035 * wave),
        CursorStyle::Paper => Transform::from_rotate(2.5 * wave),
        CursorStyle::Orbit => Transform::from_scale(1.0 + 0.02 * wave, 1.0 + 0.02 * wave),
        _ => Transform::from_scale(1.0 + 0.015 * wave, 1.0 + 0.015 * wave),
    };
    let base = Transform::from_translate(tip.0, tip.1)
        .pre_rotate(pose.tilt)
        .pre_scale(s, s)
        .pre_concat(breath);
    let click = click.filter(|t| (0.0..1.0).contains(t));
    // Picture px, and render px (for the parts of a picture), to the screen.
    let on_pic = |t: Transform| {
        t.pre_scale(UNIT, UNIT)
            .pre_translate(-pic.tip.0, -pic.tip.1)
    };
    let on_render = |t: Transform| {
        on_pic(t)
            .pre_scale(SHRINK, SHRINK)
            .pre_translate(-pic.origin.0, -pic.origin.1)
    };
    match style {
        CursorStyle::Classic => {}
        CursorStyle::Crystal => {
            image(pm, pic, on_pic(base), 1.0, BlendMode::SourceOver, None);
            if let Some(k) = click {
                crystal_flash(pm, pic, on_pic(base), on_render(base), k);
            }
        }
        CursorStyle::Paper => paper(pm, pic, on_pic(base), on_render(base), click),
        CursorStyle::Jelly => {
            // Pressed flat at once, then wobbling back.
            let a = click.map_or(0.0, |k| 0.42 * (-3.0 * k).exp() * (k * 3.4 * PI).sin());
            image(
                pm,
                pic,
                on_pic(squash(base, a)),
                1.0,
                BlendMode::SourceOver,
                None,
            );
        }
        CursorStyle::Ice => ice(pm, pic, on_pic(base), on_render(base), click),
        CursorStyle::Metal => {
            let give = click.map_or(0.0, |k| 0.16 * (-3.0 * k).exp() * (k * 2.6 * PI).sin());
            image(
                pm,
                pic,
                on_pic(squash(base, give)),
                1.0,
                BlendMode::SourceOver,
                None,
            );
            if let Some(k) = click {
                splash(pm, on_render(base), k);
            }
        }
        CursorStyle::Orbit => orbit(pm, pic, on_pic(base), on_render(base), click),
    }
    // A glint runs over glass, ice and chrome now and then while waiting.
    if let (Some(p), None, CursorStyle::Crystal | CursorStyle::Ice | CursorStyle::Metal) =
        (pose.idle, click, style)
        && p < 0.4
    {
        let k = p / 0.4;
        let (w, h) = (pic.image.width() as f32, pic.image.height() as f32);
        let at = -60.0 + (w + h + 120.0) * k;
        let band = poly(&[
            (at + 10.0, -10.0),
            (at + 46.0, -10.0),
            (at - 600.0 + 46.0, 600.0),
            (at - 600.0, 600.0),
        ]);
        if let Some(m) = mask_of(pm, &band, FillRule::Winding, on_pic(base)) {
            let a = 0.5 * (k * PI).sin();
            image(pm, pic, on_pic(base), a, BlendMode::Plus, Some(&m));
        }
    }
}

// ---- helpers ---------------------------------------------------------------

/// Draw `pic` through `t` (picture px to the canvas), from the level
/// nearest its size on screen. Drawn plainly (not a light pass), it gets a
/// rim first: a thin light edge and a soft dark one below it, so it stays
/// clear on a light, a grey or a dark background.
fn image(
    pm: &mut Pixmap,
    pic: &Picture,
    t: Transform,
    opacity: f32,
    blend_mode: BlendMode,
    mask: Option<&Mask>,
) {
    // Canvas px per picture px, and the smallest level still at least as big.
    let scale = (t.sx * t.sy - t.kx * t.ky).abs().sqrt();
    let mut k = 0;
    while k + 1 < pic.mips.len() && scale * (1u32 << (k + 1)) as f32 <= 1.0 {
        k += 1;
    }
    let lv = &pic.mips[k];
    let t = t.pre_scale((1u32 << k) as f32, (1u32 << k) as f32);
    let draw = |pm: &mut Pixmap, img: &Pixmap, t: Transform, opacity: f32, mode: BlendMode| {
        pm.draw_pixmap(
            0,
            0,
            img.as_ref(),
            &PixmapPaint {
                opacity,
                blend_mode: mode,
                quality: FilterQuality::Bicubic,
            },
            t,
            mask,
        );
    };
    if blend_mode == BlendMode::SourceOver && opacity > 0.5 {
        // A soft shadow below and right, then a light edge all round.
        let a = opacity;
        draw(
            pm,
            &lv.black,
            t.post_translate(0.8, 1.6),
            0.35 * a,
            BlendMode::SourceOver,
        );
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            draw(
                pm,
                &lv.white,
                t.post_translate(dx, dy),
                0.5 * a,
                BlendMode::SourceOver,
            );
        }
    }
    draw(pm, &lv.image, t, opacity, blend_mode);
}

/// A mask over the canvas letting through `path` (as `t` places it).
fn mask_of(pm: &Pixmap, path: &Option<Path>, rule: FillRule, t: Transform) -> Option<Mask> {
    let mut m = Mask::new(pm.width(), pm.height())?;
    m.fill_path(path.as_ref()?, rule, true, t);
    Some(m)
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn radial(c: (f32, f32), r: f32, s: &[(f32, Color)]) -> Paint<'static> {
    match RadialGradient::new(
        Point::from_xy(c.0, c.1),
        Point::from_xy(c.0, c.1),
        r.max(0.01),
        s.iter().map(|(p, c)| GradientStop::new(*p, *c)).collect(),
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        Some(shader) => Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        },
        None => solid(s.first().map_or(Color::WHITE, |s| s.1)),
    }
}

fn linear(a: (f32, f32), b: (f32, f32), s: &[(f32, Color)]) -> Paint<'static> {
    match LinearGradient::new(
        Point::from_xy(a.0, a.1),
        Point::from_xy(b.0, b.1),
        s.iter().map(|(p, c)| GradientStop::new(*p, *c)).collect(),
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        Some(shader) => Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        },
        None => solid(s.first().map_or(Color::WHITE, |s| s.1)),
    }
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// An open line through `pts`.
fn polyline(pts: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for (i, p) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(p.0, p.1);
        } else {
            pb.line_to(p.0, p.1);
        }
    }
    pb.finish()
}

fn poly(pts: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for (i, p) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(p.0, p.1);
        } else {
            pb.line_to(p.0, p.1);
        }
    }
    pb.close();
    pb.finish()
}

fn circle(x: f32, y: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(x, y, r.max(0.01))
}

fn fill(pm: &mut Pixmap, path: &Option<Path>, paint: &Paint, t: Transform) {
    if let Some(p) = path {
        pm.fill_path(p, paint, FillRule::Winding, t, None);
    }
}

/// Squash along the pointer's axis (the diagonal from the tip), keeping the
/// tip where it is: `a` > 0 flattens it, < 0 stretches it.
fn squash(t: Transform, a: f32) -> Transform {
    t.pre_rotate(45.0)
        .pre_scale(1.0 - a, 1.0 + a * 0.8)
        .pre_rotate(-45.0)
}

/// A colour around the hue circle (0–1), soft, for rainbow light.
fn hue(h: f32, a: f32) -> Color {
    let h = h.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let soft = |v: f32| 0.4 + 0.6 * v;
    Color::from_rgba(soft(r), soft(g), soft(b), a).unwrap_or(Color::WHITE)
}

/// A shiny bead: white at its top left, `body` round it, `edge` at its rim.
fn bead(pm: &mut Pixmap, x: f32, y: f32, r: f32, body: Color, edge: Color, t: Transform) {
    fill(
        pm,
        &circle(x, y, r),
        &radial(
            (x - r * 0.35, y - r * 0.35),
            r * 1.3,
            &[(0.0, Color::WHITE), (0.45, body), (1.0, edge)],
        ),
        t,
    );
}

// ---- the clicks ----------------------------------------------------------------

/// Crystal: light floods through the glass and breaks out in colours.
fn crystal_flash(pm: &mut Pixmap, pic: &Picture, t: Transform, render: Transform, k: f32) {
    let flash = (k * PI).sin();
    image(pm, pic, t, 0.7 * flash, BlendMode::Plus, None);
    for i in 0..9 {
        let ang = -0.35 + i as f32 * (TAU / 9.0);
        let (r0, r1) = (60.0 + 80.0 * k, 140.0 + 260.0 * k);
        let w = 0.1;
        let c = (215.0, 215.0);
        let p = |r: f32, d: f32| (c.0 + r * (ang + d).cos(), c.1 + r * (ang + d).sin());
        let ray = poly(&[p(r0, -w), p(r1, -w * 0.4), p(r1, w * 0.4), p(r0, w)]);
        fill(
            pm,
            &ray,
            &solid(hue(i as f32 / 9.0, 0.7 * (1.0 - k))),
            render,
        );
    }
}

/// Paper: the wing and the curled flap open out from the fold, away from
/// the long left blade, and fold back.
fn paper(pm: &mut Pixmap, pic: &Picture, t: Transform, render: Transform, click: Option<f32>) {
    let Some(k) = click else {
        image(pm, pic, t, 1.0, BlendMode::SourceOver, None);
        return;
    };
    let u = (k * PI).sin().powf(0.8);
    // The fold, on the render: from the tip down the blade's inner edge.
    let tip = (85.0, 55.0);
    let blade = poly(&[
        (-60.0, -60.0),
        (85.0, -60.0),
        tip,
        (234.0, 300.0),
        (214.0, 560.0),
        (-60.0, 560.0),
    ]);
    let wing = poly(&[
        (85.0, -60.0),
        (600.0, -60.0),
        (600.0, 560.0),
        (214.0, 560.0),
        (234.0, 300.0),
        tip,
    ]);
    // The wing turns about the tip, out and a little up.
    let turn = |t: Transform| {
        t.pre_translate(tip.0, tip.1)
            .pre_rotate(-16.0 * u)
            .pre_scale(1.0 + 0.06 * u, 1.0 + 0.06 * u)
            .pre_translate(-tip.0, -tip.1)
    };
    let wing_t = turn(render);
    if let Some(m) = mask_of(pm, &blade, FillRule::Winding, render) {
        image(pm, pic, t, 1.0, BlendMode::SourceOver, Some(&m));
    }
    // The gap the wing leaves behind it, the paper's warm inside.
    let warm = |a: f32| {
        radial(
            (200.0, 230.0),
            150.0,
            &[
                (0.0, rgba(255, 200, 120, 0.95 * a)),
                (1.0, rgba(230, 120, 40, 0.6 * a)),
            ],
        )
    };
    let edge = (234.0, 300.0);
    let (sin, cos) = (-16.0 * u).to_radians().sin_cos();
    let grow = 1.0 + 0.06 * u;
    let (dx, dy) = ((edge.0 - tip.0) * grow, (edge.1 - tip.1) * grow);
    let moved = (tip.0 + dx * cos - dy * sin, tip.1 + dx * sin + dy * cos);
    if let Some(p) = poly(&[tip, edge, moved]) {
        pm.fill_path(&p, &warm(1.0), FillRule::Winding, render, None);
    }
    // The inside of the fold shows, warm, as it opens.
    let open = poly(&[tip, (234.0, 300.0), (226.0, 352.0), (300.0, 312.0)]);
    if let Some(p) = &open {
        pm.fill_path(p, &warm(u), FillRule::Winding, wing_t, None);
    }
    if let Some(m) = mask_of(pm, &wing, FillRule::Winding, wing_t) {
        // The picture, turned the same way about the same point.
        let pic_tip = (
            (tip.0 - pic.origin.0) * SHRINK,
            (tip.1 - pic.origin.1) * SHRINK,
        );
        let turned = t
            .pre_translate(pic_tip.0, pic_tip.1)
            .pre_rotate(-16.0 * u)
            .pre_scale(1.0 + 0.06 * u, 1.0 + 0.06 * u)
            .pre_translate(-pic_tip.0, -pic_tip.1);
        image(pm, pic, turned, 1.0, BlendMode::SourceOver, Some(&m));
    }
}

/// Ice: it slumps and runs down in drips (each column of the picture
/// stretched its own way), drops fall from its points, then it freezes back.
fn ice(pm: &mut Pixmap, pic: &Picture, t: Transform, render: Transform, click: Option<f32>) {
    let Some(k) = click else {
        image(pm, pic, t, 1.0, BlendMode::SourceOver, None);
        return;
    };
    let m = (k * PI).sin();
    let melted = Picture::new(melt(&pic.image, m), pic.tip, pic.origin);
    image(pm, &melted, t, 1.0, BlendMode::SourceOver, None);
    // Wet: a cold sheen while it runs.
    image(pm, pic, t, 0.25 * m, BlendMode::Plus, None);
    // Drops falling from its points, and a little pool below.
    for (i, (x, y)) in [(172.0, 385.0), (326.0, 395.0), (420.0, 248.0)]
        .into_iter()
        .enumerate()
    {
        let start = i as f32 * 0.12;
        let d = ((k - start) / (1.0 - start)).clamp(0.0, 1.0);
        if d <= 0.0 {
            continue;
        }
        let y = y + 40.0 * m;
        let fall = 20.0 + 190.0 * d * d;
        let r = 14.0 * (1.0 - 0.4 * d);
        let a = 0.9 * (1.0 - d);
        bead(
            pm,
            x,
            y + fall,
            r,
            rgba(120, 215, 255, a),
            rgba(30, 140, 220, a),
            render,
        );
    }
    if let Some(r) = Rect::from_xywh(110.0, 480.0, 170.0 * m + 1.0, 26.0 * m + 1.0) {
        fill(
            pm,
            &PathBuilder::from_oval(r),
            &solid(rgba(140, 215, 250, 0.4 * m)),
            render,
        );
    }
}

/// The picture run down from its top: each column stretched by its own
/// amount, varying slowly across so the lower edge hangs in drips.
fn melt(src: &Pixmap, m: f32) -> Pixmap {
    let (w, h) = (src.width(), src.height());
    let out_h = (h as f32 * (1.0 + 0.2 * m)).ceil() as u32;
    let Some(mut out) = Pixmap::new(w, out_h) else {
        return src.clone();
    };
    let from = src.data();
    let to = out.data_mut();
    for x in 0..w {
        let fx = x as f32;
        let wave = (fx * 0.06).sin() * 0.6 + (fx * 0.17 + 1.3).sin() * 0.4;
        let stretch = 1.0 + m * (0.07 + 0.11 * (wave * 0.5 + 0.5).powf(2.0));
        for y in 0..out_h {
            let sy = y as f32 / stretch;
            let (y0, f) = (sy.floor() as u32, sy.fract());
            if y0 >= h {
                break;
            }
            let y1 = (y0 + 1).min(h - 1);
            let (a, b) = (((y0 * w + x) * 4) as usize, ((y1 * w + x) * 4) as usize);
            let c = ((y * w + x) * 4) as usize;
            for ch in 0..4 {
                let v = from[a + ch] as f32 * (1.0 - f) + from[b + ch] as f32 * f;
                to[c + ch] = v.round() as u8;
            }
        }
    }
    out
}

/// Liquid metal: drops of chrome thrown out from the tip, falling as they go.
fn splash(pm: &mut Pixmap, render: Transform, k: f32) {
    for i in 0..8 {
        let ang = PI * (0.55 + 0.26 * i as f32) + 0.12 * (i % 2) as f32;
        let r = 40.0 + 230.0 * k * (0.75 + 0.1 * (i % 3) as f32);
        let (x, y) = (88.0 + r * ang.cos(), 30.0 + r * ang.sin() + 120.0 * k * k);
        let size = 20.0 * (1.0 - k) + 5.0;
        let a = 1.0 - k * 0.6;
        let tint = if i % 3 == 0 {
            rgba(240, 175, 100, a)
        } else {
            rgba(160, 162, 172, a)
        };
        bead(pm, x, y, size, tint, rgba(45, 42, 44, a), render);
    }
}

/// Orbit: the pearl leaves its place and races once round the ring, a
/// trail behind it, and the ring lights up.
fn orbit(pm: &mut Pixmap, pic: &Picture, t: Transform, render: Transform, click: Option<f32>) {
    let Some(k) = click else {
        image(pm, pic, t, 1.0, BlendMode::SourceOver, None);
        return;
    };
    // The pearl's own place, left out of the picture while it goes round.
    let pearl = (383.0, 240.0);
    let mut pb = PathBuilder::new();
    if let Some(r) = Rect::from_xywh(-100.0, -100.0, 800.0, 800.0) {
        pb.push_rect(r);
    }
    pb.push_circle(pearl.0, pearl.1, 40.0);
    let without = pb.finish();
    let glow = (k * PI).sin();
    let mask = mask_of(pm, &without, FillRule::EvenOdd, render);
    image(pm, pic, t, 1.0, BlendMode::SourceOver, mask.as_ref());
    // The ring and the dart light up as it goes.
    image(pm, pic, t, 0.25 * glow, BlendMode::Plus, mask.as_ref());
    // The ring on the render: an ellipse round the dart, tilted along it.
    let ring = render.pre_translate(255.0, 245.0).pre_rotate(-25.7);
    let (rx, ry) = (186.0, 78.0);
    let at = |a: f32| (rx * a.cos(), ry * a.sin());
    // Where the pearl sits on the ring, as an angle round it.
    let start = PI / 4.0;
    let lap = k * k * (3.0 - 2.0 * k) * TAU;
    let a = start + lap;
    let trail: Vec<(f32, f32)> = (0..=24).map(|j| at(a - j as f32 * 0.05)).collect();
    if let Some(p) = {
        let mut pb = PathBuilder::new();
        for (i, q) in trail.iter().enumerate() {
            if i == 0 {
                pb.move_to(q.0, q.1);
            } else {
                pb.line_to(q.0, q.1);
            }
        }
        pb.finish()
    } {
        pm.stroke_path(
            &p,
            &solid(rgba(255, 210, 240, 0.55 * glow)),
            &stroke(18.0),
            ring,
            None,
        );
    }
    let p = at(a);
    fill(
        pm,
        &circle(p.0, p.1, 60.0),
        &radial(
            p,
            60.0,
            &[
                (0.0, rgba(230, 200, 255, 0.5)),
                (1.0, rgba(230, 200, 255, 0.0)),
            ],
        ),
        ring,
    );
    bead(
        pm,
        p.0,
        p.1,
        34.0,
        rgba(185, 180, 225, 1.0),
        rgba(45, 38, 80, 1.0),
        ring,
    );
}

/// The name tag's pill in the pointer's own material (glass, satin paper,
/// jelly, frost, chrome, dark crystal); returns the colour for its text.
pub(super) fn tag(
    pm: &mut Pixmap,
    style: CursorStyle,
    pill: &Path,
    (x, y, w, h): (f32, f32, f32, f32),
    s: f32,
) -> Color {
    let id = Transform::identity();
    let down = |stops: &[(f32, Color)]| linear((x, y), (x, y + h), stops);
    let across = |stops: &[(f32, Color)]| linear((x, y), (x + w, y + h), stops);
    let (body, rim, text) = match style {
        CursorStyle::Paper => (
            down(&[
                (0.0, rgba(255, 240, 214, 1.0)),
                (0.55, rgba(240, 196, 140, 1.0)),
                (1.0, rgba(214, 150, 88, 1.0)),
            ]),
            across(&[
                (0.0, rgba(196, 128, 66, 1.0)),
                (1.0, rgba(140, 82, 36, 1.0)),
            ]),
            rgba(92, 50, 18, 1.0),
        ),
        CursorStyle::Jelly => (
            across(&[
                (0.0, rgba(120, 160, 255, 0.96)),
                (0.6, rgba(150, 110, 245, 0.96)),
                (1.0, rgba(205, 120, 240, 0.96)),
            ]),
            solid(rgba(236, 228, 255, 0.9)),
            rgba(255, 255, 255, 1.0),
        ),
        CursorStyle::Ice => (
            down(&[
                (0.0, rgba(240, 252, 255, 0.95)),
                (1.0, rgba(150, 218, 244, 0.95)),
            ]),
            across(&[
                (0.0, rgba(255, 255, 255, 1.0)),
                (1.0, rgba(60, 200, 235, 1.0)),
            ]),
            rgba(14, 84, 124, 1.0),
        ),
        CursorStyle::Metal => (
            down(&[
                (0.0, rgba(250, 250, 252, 1.0)),
                (0.48, rgba(196, 198, 206, 1.0)),
                (0.52, rgba(170, 172, 182, 1.0)),
                (1.0, rgba(232, 232, 238, 1.0)),
            ]),
            across(&[
                (0.0, rgba(236, 180, 104, 1.0)),
                (1.0, rgba(172, 108, 48, 1.0)),
            ]),
            rgba(28, 28, 34, 1.0),
        ),
        CursorStyle::Orbit => (
            down(&[(0.0, rgba(46, 34, 78, 1.0)), (1.0, rgba(16, 12, 32, 1.0))]),
            across(&[
                (0.0, rgba(170, 120, 255, 1.0)),
                (0.5, rgba(255, 176, 222, 1.0)),
                (1.0, rgba(120, 146, 255, 1.0)),
            ]),
            rgba(232, 218, 255, 1.0),
        ),
        // Crystal (and anything else): clear glass with a rainbow edge.
        _ => (
            down(&[
                (0.0, rgba(252, 253, 255, 0.94)),
                (1.0, rgba(222, 230, 246, 0.94)),
            ]),
            across(&[
                (0.0, rgba(255, 120, 200, 1.0)),
                (0.25, rgba(255, 210, 110, 1.0)),
                (0.5, rgba(120, 230, 170, 1.0)),
                (0.75, rgba(100, 200, 255, 1.0)),
                (1.0, rgba(170, 120, 255, 1.0)),
            ]),
            rgba(38, 46, 70, 1.0),
        ),
    };
    pm.fill_path(pill, &body, FillRule::Winding, id, None);
    // A soft shine across the top half, as on the pointer itself.
    let shine = if style == CursorStyle::Orbit {
        0.18
    } else {
        0.5
    };
    let gloss = down(&[
        (0.0, rgba(255, 255, 255, shine)),
        (0.5, rgba(255, 255, 255, 0.0)),
    ]);
    pm.fill_path(pill, &gloss, FillRule::Winding, id, None);
    pm.stroke_path(pill, &rim, &stroke(1.5 * s), id, None);
    text
}

// ---- motion: trails, drag lines, scroll marks ----------------------------------

/// A pointer's own colours for what it draws around it: its main colour and
/// a light one (`ring` for the classic arrow).
pub(super) fn accent(style: CursorStyle, ring: Color) -> (Color, Color) {
    match style {
        CursorStyle::Classic => (ring, Color::WHITE),
        CursorStyle::Crystal => (rgba(170, 140, 255, 1.0), rgba(255, 255, 255, 1.0)),
        CursorStyle::Paper => (rgba(222, 142, 64, 1.0), rgba(255, 228, 176, 1.0)),
        CursorStyle::Jelly => (rgba(146, 108, 246, 1.0), rgba(206, 188, 255, 1.0)),
        CursorStyle::Ice => (rgba(64, 196, 238, 1.0), rgba(232, 250, 255, 1.0)),
        CursorStyle::Metal => (rgba(150, 152, 162, 1.0), rgba(236, 182, 106, 1.0)),
        CursorStyle::Orbit => (rgba(168, 108, 255, 1.0), rgba(255, 176, 222, 1.0)),
    }
}

/// A four-pointed sparkle.
fn sparkle(pm: &mut Pixmap, (x, y): (f32, f32), r: f32, c: Color) {
    let k = r * 0.28;
    let star = poly(&[
        (x, y - r),
        (x + k, y - k),
        (x + r, y),
        (x + k, y + k),
        (x, y + r),
        (x - k, y + k),
        (x - r, y),
        (x - k, y - k),
    ]);
    fill(pm, &star, &solid(c), Transform::identity());
}

/// What a moving pointer leaves behind, at `points` (px, newest first):
/// sparkles of colour (crystal), gold flecks (paper), a gooey tail (jelly),
/// frost (ice), chrome drops (metal), a glowing streak (orbit).
pub(super) fn trail(pm: &mut Pixmap, style: CursorStyle, points: &[(f32, f32)], s: f32) {
    let n = points.len().max(1) as f32;
    let id = Transform::identity();
    if style == CursorStyle::Orbit && points.len() >= 2 {
        for (i, w) in points.windows(2).enumerate() {
            let a = 1.0 - i as f32 / n;
            let line = polyline(&[w[0], w[1]]);
            if let Some(p) = &line {
                let glow = stroke(7.0 * s * a);
                pm.stroke_path(p, &solid(rgba(168, 108, 255, 0.22 * a)), &glow, id, None);
                let core = stroke(2.2 * s * a);
                pm.stroke_path(p, &solid(rgba(255, 176, 222, 0.85 * a)), &core, id, None);
            }
        }
        return;
    }
    for (i, &(x, y)) in points.iter().enumerate() {
        let a = 1.0 - i as f32 / n;
        // Off the line a little, the same way each time for each point.
        let side = if i % 2 == 0 { 1.0 } else { -1.0 } * 2.5 * s * ((i * 7 % 5) as f32 / 4.0);
        let (x, y) = (x + side, y - side * 0.6);
        match style {
            CursorStyle::Crystal => {
                if i % 2 == 0 {
                    sparkle(pm, (x, y), 4.2 * s * a, hue(i as f32 * 0.13, 0.95 * a));
                }
            }
            CursorStyle::Paper => {
                let r = 2.6 * s * a;
                let fleck = poly(&[
                    (x, y - r),
                    (x + r * 0.7, y),
                    (x, y + r * 0.5),
                    (x - r * 0.7, y),
                ]);
                let c = if i % 3 == 0 {
                    rgba(255, 226, 170, 0.9 * a)
                } else {
                    rgba(222, 142, 64, 0.85 * a)
                };
                fill(pm, &fleck, &solid(c), id);
            }
            CursorStyle::Jelly => {
                let r = (6.5 * a + 1.0) * s;
                fill(
                    pm,
                    &circle(x, y, r),
                    &radial(
                        (x - r * 0.3, y - r * 0.3),
                        r * 1.2,
                        &[
                            (0.0, rgba(236, 228, 255, 0.55 * a)),
                            (0.5, rgba(150, 120, 250, 0.4 * a)),
                            (1.0, rgba(110, 150, 255, 0.0)),
                        ],
                    ),
                    id,
                );
            }
            CursorStyle::Ice => {
                if i % 2 == 0 {
                    sparkle(pm, (x, y), 3.4 * s * a, rgba(255, 255, 255, 0.95 * a));
                } else {
                    fill(
                        pm,
                        &circle(x, y, 1.6 * s * a),
                        &solid(rgba(120, 220, 250, 0.8 * a)),
                        id,
                    );
                }
            }
            CursorStyle::Metal => {
                if i % 2 == 1 {
                    let r = 2.8 * s * a + 0.5;
                    bead(pm, x, y, r, rgba(176, 178, 188, a), rgba(70, 70, 80, a), id);
                }
            }
            CursorStyle::Classic | CursorStyle::Orbit => {}
        }
    }
}

/// The line a drag draws along `path` (px), in the pointer's own material,
/// at `alpha`.
pub(super) fn drag_line(
    pm: &mut Pixmap,
    style: CursorStyle,
    ring: Color,
    path: &[(f32, f32)],
    s: f32,
    alpha: f32,
) {
    let id = Transform::identity();
    let (Some(&from), Some(&to), Some(line)) = (path.first(), path.last(), polyline(path)) else {
        return;
    };
    let (main, light) = accent(style, ring);
    let fade = |c: Color, a: f32| {
        let mut c = c;
        c.set_alpha((c.alpha() * a * alpha).clamp(0.0, 1.0));
        c
    };
    // A soft glow under every line.
    pm.stroke_path(&line, &solid(fade(main, 0.22)), &stroke(10.0 * s), id, None);
    match style {
        CursorStyle::Crystal => {
            let stops: Vec<(f32, Color)> = (0..=6)
                .map(|i| (i as f32 / 6.0, fade(hue(i as f32 / 6.0, 1.0), 1.0)))
                .collect();
            pm.stroke_path(&line, &linear(from, to, &stops), &stroke(3.2 * s), id, None);
        }
        CursorStyle::Paper => {
            let mut dashed = stroke(2.6 * s);
            dashed.dash = tiny_skia::StrokeDash::new(vec![7.0 * s, 4.5 * s], 0.0);
            pm.stroke_path(&line, &solid(fade(main, 1.0)), &dashed, id, None);
        }
        CursorStyle::Jelly => {
            pm.stroke_path(&line, &solid(fade(main, 0.55)), &stroke(7.0 * s), id, None);
            pm.stroke_path(&line, &solid(fade(light, 0.8)), &stroke(2.0 * s), id, None);
        }
        CursorStyle::Metal => {
            pm.stroke_path(
                &line,
                &solid(fade(rgba(96, 98, 108, 1.0), 1.0)),
                &stroke(4.4 * s),
                id,
                None,
            );
            pm.stroke_path(
                &line,
                &solid(fade(rgba(236, 236, 242, 1.0), 1.0)),
                &stroke(1.6 * s),
                id,
                None,
            );
        }
        CursorStyle::Ice => {
            pm.stroke_path(&line, &solid(fade(main, 0.9)), &stroke(3.6 * s), id, None);
            pm.stroke_path(&line, &solid(fade(light, 1.0)), &stroke(1.4 * s), id, None);
            // Frost along the way, every so far.
            let mut run = 0.0;
            for w in path.windows(2) {
                run += (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
                if run >= 22.0 * s {
                    run = 0.0;
                    sparkle(pm, w[1], 3.0 * s, fade(light, 0.9));
                }
            }
        }
        CursorStyle::Orbit => {
            pm.stroke_path(&line, &solid(fade(main, 0.9)), &stroke(3.4 * s), id, None);
            pm.stroke_path(&line, &solid(fade(light, 1.0)), &stroke(1.3 * s), id, None);
        }
        CursorStyle::Classic => {
            pm.stroke_path(&line, &solid(fade(main, 1.0)), &stroke(3.0 * s), id, None);
        }
    }
    // Where it was picked up.
    fill(
        pm,
        &circle(from.0, from.1, 4.5 * s),
        &solid(fade(light, 0.95)),
        id,
    );
    if let Some(dot) = circle(from.0, from.1, 4.5 * s) {
        pm.stroke_path(&dot, &solid(fade(main, 1.0)), &stroke(1.6 * s), id, None);
    }
}

/// Arrows the way the pointer scrolls (`dir`: -1, 0 or 1 across and down),
/// three in a row from `at` (px), each lit in turn as `k` runs 0–1.
pub(super) fn chevrons(
    pm: &mut Pixmap,
    style: CursorStyle,
    ring: Color,
    at: (f32, f32),
    dir: (f32, f32),
    s: f32,
    k: f32,
) {
    let (main, light) = accent(style, ring);
    let id = Transform::identity();
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux);
    for i in 0..3 {
        // Lit one after another down the row, then all fading together.
        let on = ((k * 3.2 - i as f32 * 0.45) * 2.0).clamp(0.0, 1.0);
        let a = on * (1.0 - ((k - 0.7) / 0.3).clamp(0.0, 1.0));
        if a <= 0.0 {
            continue;
        }
        let d = i as f32 * 7.0 * s;
        let (cx, cy) = (at.0 + ux * d, at.1 + uy * d);
        let (w, h) = (5.0 * s, 3.4 * s);
        let Some(v) = polyline(&[
            (cx - ux * h + px * w, cy - uy * h + py * w),
            (cx, cy),
            (cx - ux * h - px * w, cy - uy * h - py * w),
        ]) else {
            continue;
        };
        let mut c = light;
        c.set_alpha(0.9 * a);
        pm.stroke_path(&v, &solid(c), &stroke(4.2 * s), id, None);
        let mut c = main;
        c.set_alpha(a);
        pm.stroke_path(&v, &solid(c), &stroke(2.2 * s), id, None);
    }
}
