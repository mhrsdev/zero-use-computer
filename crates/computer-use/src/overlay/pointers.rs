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
                    Some((
                        *style,
                        Picture {
                            image,
                            tip: *tip,
                            origin: *origin,
                        },
                    ))
                })
                .collect()
        })
        .iter()
        .find(|(s, _)| *s == style)
        .map(|(_, p)| p)
}

/// Draw `style` with its tip at `tip` (px), `s` px per unit; `click` runs
/// 0–1 through a click.
pub(super) fn draw(
    pm: &mut Pixmap,
    style: CursorStyle,
    tip: (f32, f32),
    s: f32,
    click: Option<f32>,
) {
    let Some(pic) = picture(style) else {
        return;
    };
    let base = Transform::from_scale(s, s).post_translate(tip.0, tip.1);
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
}

// ---- helpers ---------------------------------------------------------------

fn image(
    pm: &mut Pixmap,
    pic: &Picture,
    t: Transform,
    opacity: f32,
    blend_mode: BlendMode,
    mask: Option<&Mask>,
) {
    pm.draw_pixmap(
        0,
        0,
        pic.image.as_ref(),
        &PixmapPaint {
            opacity,
            blend_mode,
            quality: FilterQuality::Bicubic,
        },
        t,
        mask,
    );
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
    let melted = Picture {
        image: melt(&pic.image, m),
        tip: pic.tip,
        origin: pic.origin,
    };
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
