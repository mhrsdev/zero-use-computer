//! The agent's pointers, drawn by hand: crystal, paper, jelly, ice, liquid
//! metal and orbit, each with its own way of clicking. Each is drawn on a
//! 512 × 512 sheet (its tip near the top left, pointing up and left) and
//! shrunk to about 30 px tall at scale 1, its tip on the hotspot.

use std::f32::consts::{PI, TAU};

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder,
    Pixmap, Point, RadialGradient, Shader, SpreadMode, Stroke, Transform,
};

use super::draw::CursorStyle;

/// Sheet px per px on the screen at scale 1.
const SHEET: f32 = 13.5;

/// Draw `style` with its tip at `tip` (px), `s` px per unit; `click` runs
/// 0–1 through a click.
pub(super) fn draw(
    pm: &mut Pixmap,
    style: CursorStyle,
    tip: (f32, f32),
    s: f32,
    click: Option<f32>,
) {
    let base = Transform::from_scale(s, s).post_translate(tip.0, tip.1);
    let click = click.filter(|t| (0.0..1.0).contains(t));
    // The sheet, its tip at the origin.
    let sheet = |t: Transform, at: (f32, f32)| {
        t.pre_scale(1.0 / SHEET, 1.0 / SHEET)
            .pre_translate(-at.0, -at.1)
    };
    match style {
        CursorStyle::Classic => {}
        CursorStyle::Crystal => crystal(pm, sheet(base, CRYSTAL_TIP), click),
        CursorStyle::Paper => paper(pm, sheet(base, PAPER_TIP), click),
        CursorStyle::Jelly => {
            // Pressed flat at once, then wobbling back.
            let a = click.map_or(0.0, |k| 0.42 * (-3.0 * k).exp() * (k * 3.4 * PI).sin());
            jelly(pm, sheet(squash(base, a), JELLY_TIP))
        }
        CursorStyle::Ice => ice(pm, sheet(base, ICE_TIP), click),
        CursorStyle::Metal => {
            let give = click.map_or(0.0, |k| 0.14 * (-3.0 * k).exp() * (k * 2.6 * PI).sin());
            metal(
                pm,
                sheet(squash(base, give), METAL_TIP),
                sheet(base, METAL_TIP),
                click,
            )
        }
        CursorStyle::Orbit => orbit(pm, sheet(base, ORBIT_TIP), click),
    }
}

// ---- helpers ---------------------------------------------------------------

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba8(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn shaded(shader: Option<Shader<'static>>, fallback: Color) -> Paint<'static> {
    match shader {
        Some(shader) => Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        },
        None => solid(fallback),
    }
}

fn stops(s: &[(f32, Color)]) -> Vec<GradientStop> {
    s.iter().map(|(p, c)| GradientStop::new(*p, *c)).collect()
}

fn linear(a: (f32, f32), b: (f32, f32), s: &[(f32, Color)]) -> Paint<'static> {
    let fallback = s.first().map_or(Color::WHITE, |s| s.1);
    shaded(
        LinearGradient::new(
            Point::from_xy(a.0, a.1),
            Point::from_xy(b.0, b.1),
            stops(s),
            SpreadMode::Pad,
            Transform::identity(),
        ),
        fallback,
    )
}

fn radial(c: (f32, f32), r: f32, s: &[(f32, Color)]) -> Paint<'static> {
    let fallback = s.first().map_or(Color::WHITE, |s| s.1);
    shaded(
        RadialGradient::new(
            Point::from_xy(c.0, c.1),
            Point::from_xy(c.0, c.1),
            r.max(0.01),
            stops(s),
            SpreadMode::Pad,
            Transform::identity(),
        ),
        fallback,
    )
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

fn open_line(pts: &[(f32, f32)]) -> Option<Path> {
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

/// A closed polygon with each corner rounded by its own radius.
fn rounded(pts: &[(f32, f32)], radii: &[f32]) -> Option<Path> {
    let n = pts.len();
    let dist = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let toward = |a: (f32, f32), b: (f32, f32), d: f32| {
        let l = dist(a, b).max(1e-3);
        (a.0 + (b.0 - a.0) * d / l, a.1 + (b.1 - a.1) * d / l)
    };
    let mut pb = PathBuilder::new();
    for i in 0..n {
        let (p0, p1, p2) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let r = radii.get(i).copied().unwrap_or(0.0);
        let r = r.min(dist(p0, p1) / 2.0).min(dist(p1, p2) / 2.0);
        let (a, b) = (toward(p1, p0, r), toward(p1, p2, r));
        if i == 0 {
            pb.move_to(a.0, a.1);
        } else {
            pb.line_to(a.0, a.1);
        }
        pb.quad_to(p1.0, p1.1, b.0, b.1);
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

fn line(pm: &mut Pixmap, path: &Option<Path>, paint: &Paint, width: f32, t: Transform) {
    if let Some(p) = path {
        pm.stroke_path(p, paint, &stroke(width), t, None);
    }
}

/// A soft drop shadow (sheet px): layered strokes, offset down and right.
fn shadow(pm: &mut Pixmap, path: &Option<Path>, t: Transform, strength: f32) {
    let t = t.pre_translate(10.0, 22.0);
    for (w, a) in [(70.0, 0.035), (45.0, 0.05), (24.0, 0.07)] {
        line(pm, path, &solid(rgba(0, 0, 0, a * strength)), w, t);
    }
    fill(pm, path, &solid(rgba(0, 0, 0, 0.14 * strength)), t);
}

/// A glow in `c` around `path` (sheet px): layered wide strokes.
fn glow(pm: &mut Pixmap, path: &Option<Path>, c: (u8, u8, u8), strength: f32, t: Transform) {
    for (w, a) in [(46.0, 0.06), (28.0, 0.10), (14.0, 0.18)] {
        line(pm, path, &solid(rgba(c.0, c.1, c.2, a * strength)), w, t);
    }
}

/// Squash along the pointer's axis (the diagonal from the tip), keeping the
/// tip where it is: `a` > 0 flattens it, < 0 stretches it.
fn squash(t: Transform, a: f32) -> Transform {
    t.pre_rotate(45.0)
        .pre_scale(1.0 - a, 1.0 + a * 0.8)
        .pre_rotate(-45.0)
}

fn lerp(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
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

/// A highlight spot: white fading out.
fn spot(pm: &mut Pixmap, x: f32, y: f32, r: f32, a: f32, t: Transform) {
    fill(
        pm,
        &circle(x, y, r),
        &radial(
            (x, y),
            r,
            &[
                (0.0, rgba(255, 255, 255, a)),
                (1.0, rgba(255, 255, 255, 0.0)),
            ],
        ),
        t,
    );
}

// ---- crystal -----------------------------------------------------------------

const CRYSTAL_TIP: (f32, f32) = (95.0, 52.0);

/// Cut glass: a bevelled rim splitting light into orange and blue around a
/// dark glassy face. Clicking, light breaks out of it in a burst of colours.
fn crystal(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let outer = [
        (95.0, 52.0),
        (418.0, 282.0),
        (300.0, 290.0),
        (362.0, 398.0),
        (336.0, 432.0),
        (230.0, 330.0),
        (165.0, 412.0),
    ];
    let inner = [
        (132.0, 108.0),
        (352.0, 270.0),
        (282.0, 276.0),
        (338.0, 402.0),
        (226.0, 300.0),
        (176.0, 370.0),
    ];
    let outline = poly(&outer);
    glow(pm, &outline, (200, 220, 255), 0.6, t);
    shadow(pm, &outline, t, 0.5);
    fill(
        pm,
        &outline,
        &linear(
            (95.0, 52.0),
            (330.0, 420.0),
            &[(0.0, rgba(70, 80, 95, 0.95)), (1.0, rgba(20, 24, 32, 0.97))],
        ),
        t,
    );
    // The bevel: each strip splits the light its own way.
    let (w, o, b) = (
        rgba(255, 255, 255, 0.95),
        rgba(255, 165, 70, 0.95),
        rgba(100, 165, 255, 0.95),
    );
    let lights = [
        [w, o, b],
        [o, w, b],
        [b, w, o],
        [w, o, w],
        [o, b, w],
        [b, o, w],
        [w, b, o],
    ];
    let pairs = [(0, 0), (1, 1), (2, 2), (3, 3), (4, 3), (5, 4), (6, 5)];
    for i in 0..outer.len() {
        let (o1, i1) = pairs[i];
        let (o2, i2) = pairs[(i + 1) % outer.len()];
        let strip = poly(&[outer[o1], outer[o2], inner[i2], inner[i1]]);
        let [c0, c1, c2] = lights[i];
        fill(
            pm,
            &strip,
            &linear(outer[o1], outer[o2], &[(0.0, c0), (0.5, c1), (1.0, c2)]),
            t,
        );
        line(
            pm,
            &open_line(&[outer[o1], inner[i1]]),
            &solid(rgba(255, 255, 255, 0.7)),
            2.5,
            t,
        );
    }
    // The face: dark glass, light catching its upper corner.
    let face = poly(&inner);
    fill(
        pm,
        &face,
        &linear(
            (132.0, 108.0),
            (300.0, 340.0),
            &[
                (0.0, rgba(185, 195, 210, 0.95)),
                (0.35, rgba(70, 78, 92, 0.97)),
                (1.0, rgba(14, 16, 22, 0.98)),
            ],
        ),
        t,
    );
    let sheen = poly(&[(140.0, 122.0), (310.0, 258.0), (185.0, 250.0)]);
    fill(
        pm,
        &sheen,
        &linear(
            (140.0, 122.0),
            (240.0, 250.0),
            &[
                (0.0, rgba(255, 255, 255, 0.35)),
                (1.0, rgba(255, 255, 255, 0.0)),
            ],
        ),
        t,
    );
    line(pm, &face, &solid(rgba(255, 255, 255, 0.55)), 2.5, t);
    line(pm, &outline, &solid(rgba(255, 255, 255, 0.95)), 3.5, t);
    for (x, y, r) in [
        (105.0, 65.0, 26.0),
        (405.0, 282.0, 24.0),
        (340.0, 425.0, 20.0),
        (210.0, 330.0, 16.0),
    ] {
        spot(pm, x, y, r, 0.95, t);
    }
    if let Some(k) = click {
        // A flash through the glass, and rays of colour breaking out.
        let flash = (k * PI).sin();
        fill(pm, &outline, &solid(rgba(255, 255, 255, 0.5 * flash)), t);
        for i in 0..9 {
            let ang = -0.35 + i as f32 * (TAU / 9.0);
            let (r0, r1) = (60.0 + 80.0 * k, 140.0 + 260.0 * k);
            let w = 0.1;
            let c = (200.0, 200.0);
            let p = |r: f32, d: f32| (c.0 + r * (ang + d).cos(), c.1 + r * (ang + d).sin());
            let ray = poly(&[p(r0, -w), p(r1, -w * 0.4), p(r1, w * 0.4), p(r0, w)]);
            fill(pm, &ray, &solid(hue(i as f32 / 9.0, 0.7 * (1.0 - k))), t);
        }
    }
}

// ---- paper -------------------------------------------------------------------

const PAPER_TIP: (f32, f32) = (85.0, 55.0);

/// A folded sheet of satin metal-paper: a broad upper wing, a long left
/// blade, the inside of the fold glowing orange, and a curled flap below.
/// Clicking, the fold opens out flat and folds back.
fn paper(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    // How open the fold is: 0 as drawn, 1 flat.
    let u = click.map_or(0.0, |k| (k * PI).sin().powf(0.8));
    let tip = (85.0, 55.0);
    let wing_tip = lerp((403.0, 278.0), (430.0, 250.0), u);
    let fold_in = lerp((210.0, 235.0), (250.0, 215.0), u);
    let crease_low = (234.0, 300.0);
    // The curled flap swings out to lie flat beside the blade.
    let flap_end = lerp((368.0, 458.0), (450.0, 380.0), u);
    let flap_root_a = lerp((300.0, 312.0), (330.0, 300.0), u);

    let blade = [tip, fold_in, crease_low, (172.0, 412.0)];
    let wing = [
        tip,
        wing_tip,
        lerp((300.0, 262.0), (330.0, 250.0), u),
        fold_in,
    ];
    let inside = [
        fold_in,
        lerp((300.0, 262.0), (330.0, 250.0), u),
        wing_tip,
        (330.0, 296.0),
        crease_low,
    ];
    let flap = [crease_low, flap_root_a, flap_end, (226.0, 352.0)];

    let all = poly(&[
        tip,
        wing_tip,
        (330.0, 296.0),
        flap_root_a,
        flap_end,
        (172.0, 412.0),
    ]);
    shadow(pm, &all, t, 0.9);
    glow(pm, &all, (255, 170, 80), 0.35, t);
    let satin = |a: (f32, f32), b: (f32, f32), dim: f32| {
        let d = |v: u8| (f32::from(v) * dim) as u8;
        linear(
            a,
            b,
            &[
                (0.0, rgba(d(250), d(246), d(242), 1.0)),
                (0.4, rgba(d(205), d(198), d(196), 1.0)),
                (0.75, rgba(d(150), d(140), d(138), 1.0)),
                (1.0, rgba(d(220), d(205), d(190), 1.0)),
            ],
        )
    };
    // The inside of the fold, lit warm where the light gets in; it closes
    // up as the fold opens.
    fill(
        pm,
        &poly(&inside),
        &linear(
            fold_in,
            (380.0, 300.0),
            &[
                (0.0, rgba(255, 225, 160, 1.0)),
                (0.25, rgba(250, 165, 60, 1.0)),
                (0.7, rgba(190, 105, 40, 1.0)),
                (1.0, rgba(120, 70, 35, 1.0)),
            ],
        ),
        t,
    );
    fill(pm, &poly(&wing), &satin(tip, (330.0, 260.0), 1.0), t);
    fill(
        pm,
        &poly(&blade),
        &satin((120.0, 80.0), (210.0, 400.0), 0.85),
        t,
    );
    // The flap: curled under as drawn, flat once open.
    fill(
        pm,
        &poly(&flap),
        &linear(
            (226.0, 352.0),
            flap_end,
            &[
                (0.0, rgba(140, 130, 128, 1.0)),
                (0.4, rgba(235, 230, 226, 1.0)),
                (1.0, rgba(180, 170, 165, 1.0)),
            ],
        ),
        t,
    );
    // The glow where the fold opens.
    spot(
        pm,
        fold_in.0 - 12.0,
        fold_in.1 - 4.0,
        60.0 * (1.0 - 0.6 * u),
        0.85,
        t,
    );
    fill(
        pm,
        &circle(fold_in.0, fold_in.1 + 10.0, 75.0),
        &radial(
            (fold_in.0, fold_in.1 + 10.0),
            75.0,
            &[
                (0.0, rgba(255, 170, 60, 0.6 * (1.0 - u))),
                (1.0, rgba(255, 140, 40, 0.0)),
            ],
        ),
        t,
    );
    // Warm rim along the left edge, crisp edges elsewhere.
    line(
        pm,
        &open_line(&[tip, (172.0, 412.0)]),
        &solid(rgba(255, 175, 90, 0.9)),
        6.0,
        t,
    );
    line(
        pm,
        &open_line(&[tip, wing_tip]),
        &solid(rgba(255, 252, 248, 0.95)),
        4.0,
        t,
    );
    line(pm, &poly(&flap), &solid(rgba(255, 190, 120, 0.55)), 3.0, t);
    line(
        pm,
        &open_line(&[tip, fold_in, crease_low]),
        &solid(rgba(90, 70, 60, 0.35)),
        3.0,
        t,
    );
}

// ---- jelly -------------------------------------------------------------------

const JELLY_TIP: (f32, f32) = (78.0, 55.0);

/// A soft, glossy jelly in cyan to violet with a pink sheen, its tail a
/// round lobe. Clicking, it squashes flat and wobbles back (done by the
/// caller's transform).
fn jelly(pm: &mut Pixmap, t: Transform) {
    let mut pb = PathBuilder::new();
    pb.move_to(78.0, 55.0);
    pb.line_to(372.0, 238.0);
    pb.cubic_to(410.0, 258.0, 412.0, 290.0, 380.0, 294.0);
    pb.cubic_to(350.0, 298.0, 318.0, 296.0, 304.0, 304.0);
    pb.cubic_to(296.0, 322.0, 326.0, 358.0, 344.0, 382.0);
    pb.cubic_to(366.0, 410.0, 364.0, 450.0, 330.0, 452.0);
    pb.cubic_to(300.0, 454.0, 286.0, 430.0, 272.0, 404.0);
    pb.cubic_to(258.0, 378.0, 246.0, 356.0, 230.0, 350.0);
    pb.cubic_to(218.0, 346.0, 214.0, 368.0, 196.0, 364.0);
    pb.cubic_to(178.0, 360.0, 176.0, 340.0, 172.0, 322.0);
    pb.line_to(74.0, 66.0);
    pb.quad_to(70.0, 52.0, 78.0, 55.0);
    pb.close();
    let blob = pb.finish();
    glow(pm, &blob, (120, 210, 255), 1.2, t);
    shadow(pm, &blob, t, 0.6);
    fill(
        pm,
        &blob,
        &linear(
            (78.0, 55.0),
            (360.0, 430.0),
            &[
                (0.0, rgba(230, 248, 255, 0.98)),
                (0.28, rgba(150, 205, 255, 0.97)),
                (0.55, rgba(120, 160, 245, 0.97)),
                (0.8, rgba(175, 160, 240, 0.97)),
                (1.0, rgba(235, 200, 245, 0.98)),
            ],
        ),
        t,
    );
    // Light inside it.
    fill(
        pm,
        &blob,
        &radial(
            (215.0, 220.0),
            175.0,
            &[
                (0.0, rgba(255, 255, 255, 0.38)),
                (1.0, rgba(255, 255, 255, 0.0)),
            ],
        ),
        t,
    );
    // Pink sheen on the wing tip, light in the lobe.
    fill(
        pm,
        &circle(378.0, 272.0, 62.0),
        &radial(
            (378.0, 272.0),
            62.0,
            &[
                (0.0, rgba(255, 180, 235, 0.65)),
                (1.0, rgba(255, 180, 235, 0.0)),
            ],
        ),
        t,
    );
    fill(
        pm,
        &circle(338.0, 420.0, 52.0),
        &radial(
            (338.0, 420.0),
            52.0,
            &[
                (0.0, rgba(255, 255, 255, 0.6)),
                (1.0, rgba(200, 225, 255, 0.0)),
            ],
        ),
        t,
    );
    // A bright rim, brightest along the top and left edges.
    line(pm, &blob, &solid(rgba(215, 240, 255, 0.85)), 6.0, t);
    line(
        pm,
        &open_line(&[(92.0, 66.0), (380.0, 255.0)]),
        &solid(rgba(255, 255, 255, 0.75)),
        7.0,
        t,
    );
    let gloss = rounded(
        &[(98.0, 82.0), (152.0, 112.0), (195.0, 318.0), (176.0, 322.0)],
        &[8.0, 22.0, 14.0, 14.0],
    );
    fill(
        pm,
        &gloss,
        &linear(
            (98.0, 82.0),
            (190.0, 320.0),
            &[
                (0.0, rgba(255, 255, 255, 0.95)),
                (1.0, rgba(255, 255, 255, 0.05)),
            ],
        ),
        t,
    );
    spot(pm, 112.0, 92.0, 24.0, 1.0, t);
    spot(pm, 352.0, 432.0, 16.0, 0.9, t);
}

// ---- ice ---------------------------------------------------------------------

const ICE_TIP: (f32, f32) = (95.0, 35.0);

/// Frosted ice in panes: a long blade, a broad wing, and a glowing cyan
/// crystal between them. Clicking, it melts and drips, then freezes again.
fn ice(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let m = click.map_or(0.0, |k| (k * PI).sin());
    let sag = 26.0 * m;
    let blade = [
        (95.0, 35.0),
        (196.0, 182.0),
        (210.0, 290.0),
        (172.0, 385.0 + sag),
    ];
    let wing = [
        (112.0, 44.0),
        (422.0, 247.0 + sag * 0.5),
        (300.0, 252.0 + sag * 0.3),
        (198.0, 168.0),
    ];
    let gem = [
        (218.0, 196.0),
        (302.0, 262.0),
        (326.0, 395.0 + sag),
        (232.0, 300.0),
    ];
    let soft = 4.0 + 30.0 * m;
    let blade_p = rounded(&blade, &[4.0, soft, soft, soft]);
    let wing_p = rounded(&wing, &[4.0, soft, soft, soft]);
    let gem_p = rounded(&gem, &[soft, soft, soft, soft]);
    for p in [&blade_p, &wing_p] {
        glow(pm, p, (60, 200, 255), 0.8, t);
        shadow(pm, p, t, 0.5);
    }
    glow(pm, &gem_p, (40, 220, 255), 1.0, t);
    let clear = 1.0 - 0.35 * m;
    // Frosted panes, lit from the top left; wet and clearer as it melts.
    fill(
        pm,
        &blade_p,
        &linear(
            (95.0, 35.0),
            (190.0, 385.0),
            &[
                (0.0, rgba(225, 245, 255, 0.95 * clear)),
                (0.5, rgba(150, 205, 235, 0.88 * clear)),
                (1.0, rgba(110, 180, 225, 0.9 * clear)),
            ],
        ),
        t,
    );
    fill(
        pm,
        &wing_p,
        &linear(
            (112.0, 44.0),
            (420.0, 250.0),
            &[
                (0.0, rgba(240, 250, 255, 0.95 * clear)),
                (0.55, rgba(200, 220, 240, 0.9 * clear)),
                (1.0, rgba(170, 195, 225, 0.92 * clear)),
            ],
        ),
        t,
    );
    // The shaded crease between them.
    let crease = poly(&[(105.0, 42.0), (198.0, 168.0), (196.0, 182.0), (100.0, 50.0)]);
    fill(pm, &crease, &solid(rgba(30, 50, 70, 0.6)), t);
    // The crystal: deep blue inside a bright cyan rim.
    fill(
        pm,
        &gem_p,
        &linear(
            (218.0, 196.0),
            (326.0, 395.0),
            &[
                (0.0, rgba(120, 245, 255, 1.0)),
                (0.5, rgba(0, 150, 220, 1.0)),
                (1.0, rgba(60, 220, 255, 1.0)),
            ],
        ),
        t,
    );
    let core = rounded(
        &[
            (235.0, 225.0),
            (285.0, 270.0),
            (300.0, 360.0 + sag * 0.8),
            (248.0, 296.0),
        ],
        &[soft; 4],
    );
    fill(pm, &core, &solid(rgba(10, 90, 150, 0.75)), t);
    line(pm, &gem_p, &solid(rgba(150, 250, 255, 0.95)), 5.0, t);
    // Icy edges.
    line(pm, &blade_p, &solid(rgba(140, 235, 255, 0.95)), 5.0, t);
    line(pm, &wing_p, &solid(rgba(235, 250, 255, 0.95)), 4.0, t);
    line(
        pm,
        &open_line(&[(112.0, 44.0), (422.0, 247.0 + sag * 0.5)]),
        &solid(rgba(255, 255, 255, 1.0)),
        4.0,
        t,
    );
    if let Some(k) = click {
        // Drops falling from the points, and a small pool below.
        for (i, (x, y)) in [
            (172.0, 385.0 + sag),
            (326.0, 395.0 + sag),
            (422.0, 247.0 + sag * 0.5),
        ]
        .into_iter()
        .enumerate()
        {
            let start = i as f32 * 0.12;
            let d = ((k - start) / (1.0 - start)).clamp(0.0, 1.0);
            if d <= 0.0 {
                continue;
            }
            let fall = 20.0 + 190.0 * d * d;
            let r = 15.0 * (1.0 - 0.4 * d);
            let drop = rounded(
                &[
                    (x, y + fall - r * 2.4),
                    (x + r, y + fall),
                    (x, y + fall + r),
                    (x - r, y + fall),
                ],
                &[2.0, r, r, r],
            );
            fill(pm, &drop, &solid(rgba(150, 225, 255, 0.9 * (1.0 - d))), t);
            spot(
                pm,
                x - r * 0.3,
                y + fall - r * 0.2,
                r * 0.5,
                0.8 * (1.0 - d),
                t,
            );
        }
        if let Some(r) = tiny_skia::Rect::from_xywh(110.0, 470.0, 160.0 * m + 1.0, 26.0 * m + 1.0) {
            fill(
                pm,
                &PathBuilder::from_oval(r),
                &solid(rgba(150, 220, 250, 0.4 * m)),
                t,
            );
        }
    }
}

// ---- liquid metal ------------------------------------------------------------

const METAL_TIP: (f32, f32) = (88.0, 30.0);

/// Liquid chrome: a sharp tip, a curl at the wing, and a tail that hangs in
/// a drop, mirror-bright above and warm gold below. Clicking, it gives a
/// little and drops of metal splash out from the tip.
fn metal(pm: &mut Pixmap, t: Transform, still: Transform, click: Option<f32>) {
    let curve = |pts: &[(f32, f32)]| {
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].0, pts[0].1);
        for c in pts[1..].chunks(3) {
            pb.cubic_to(c[0].0, c[0].1, c[1].0, c[1].1, c[2].0, c[2].1);
        }
        pb.close();
        pb.finish()
    };
    let body = curve(&[
        (88.0, 30.0),
        (200.0, 95.0),
        (290.0, 160.0),
        (330.0, 205.0),
        (362.0, 242.0),
        (372.0, 270.0),
        (348.0, 274.0),
        (330.0, 277.0),
        (338.0, 302.0),
        (372.0, 322.0),
        (418.0, 348.0),
        (422.0, 402.0),
        (386.0, 412.0),
        (346.0, 422.0),
        (318.0, 398.0),
        (304.0, 372.0),
        (284.0, 332.0),
        (258.0, 252.0),
        (212.0, 262.0),
        (186.0, 268.0),
        (175.0, 300.0),
        (160.0, 328.0),
        (132.0, 240.0),
        (112.0, 132.0),
        (88.0, 30.0),
    ]);
    shadow(pm, &body, t, 1.0);
    glow(pm, &body, (255, 175, 95), 0.4, t);
    // Dark chrome underneath everything.
    fill(
        pm,
        &body,
        &linear(
            (88.0, 30.0),
            (380.0, 410.0),
            &[
                (0.0, rgba(190, 195, 205, 1.0)),
                (0.18, rgba(45, 47, 55, 1.0)),
                (0.5, rgba(22, 23, 29, 1.0)),
                (0.8, rgba(55, 48, 46, 1.0)),
                (1.0, rgba(28, 28, 34, 1.0)),
            ],
        ),
        t,
    );
    // The sky in it: a broad mirror-bright sweep along the top.
    let sky = curve(&[
        (102.0, 50.0),
        (200.0, 106.0),
        (284.0, 166.0),
        (322.0, 210.0),
        (345.0, 238.0),
        (350.0, 262.0),
        (330.0, 262.0),
        (298.0, 250.0),
        (250.0, 216.0),
        (198.0, 206.0),
        (160.0, 196.0),
        (130.0, 140.0),
        (102.0, 50.0),
    ]);
    fill(
        pm,
        &sky,
        &linear(
            (110.0, 60.0),
            (300.0, 250.0),
            &[
                (0.0, rgba(255, 255, 255, 0.98)),
                (0.5, rgba(225, 230, 240, 0.97)),
                (1.0, rgba(150, 158, 175, 0.95)),
            ],
        ),
        t,
    );
    // A softer grey on the left face.
    let side = curve(&[
        (104.0, 82.0),
        (118.0, 150.0),
        (135.0, 230.0),
        (158.0, 300.0),
        (170.0, 268.0),
        (184.0, 242.0),
        (198.0, 230.0),
        (158.0, 204.0),
        (130.0, 150.0),
        (104.0, 82.0),
    ]);
    fill(
        pm,
        &side,
        &linear(
            (104.0, 82.0),
            (170.0, 290.0),
            &[
                (0.0, rgba(150, 155, 168, 0.9)),
                (1.0, rgba(60, 62, 72, 0.9)),
            ],
        ),
        t,
    );
    // Gold light under the neck and round the drop.
    let gold = curve(&[
        (214.0, 266.0),
        (256.0, 258.0),
        (282.0, 330.0),
        (304.0, 372.0),
        (318.0, 398.0),
        (346.0, 416.0),
        (382.0, 410.0),
        (352.0, 398.0),
        (332.0, 372.0),
        (318.0, 342.0),
        (296.0, 300.0),
        (268.0, 266.0),
        (214.0, 266.0),
    ]);
    fill(
        pm,
        &gold,
        &linear(
            (214.0, 266.0),
            (380.0, 410.0),
            &[
                (0.0, rgba(255, 205, 140, 0.95)),
                (0.5, rgba(240, 160, 80, 0.95)),
                (1.0, rgba(255, 200, 130, 0.95)),
            ],
        ),
        t,
    );
    // The drop: dark, a bright window of light, gold at its foot.
    fill(
        pm,
        &circle(372.0, 368.0, 36.0),
        &radial(
            (372.0, 368.0),
            36.0,
            &[(0.0, rgba(40, 40, 48, 0.9)), (1.0, rgba(40, 40, 48, 0.0))],
        ),
        t,
    );
    spot(pm, 370.0, 352.0, 20.0, 1.0, t);
    spot(pm, 352.0, 250.0, 14.0, 0.95, t);
    // Warm rim light down the left edge.
    let mut rim = PathBuilder::new();
    rim.move_to(92.0, 45.0);
    rim.cubic_to(115.0, 140.0, 135.0, 240.0, 160.0, 322.0);
    line(pm, &rim.finish(), &solid(rgba(255, 180, 100, 0.95)), 6.0, t);
    line(pm, &body, &solid(rgba(255, 255, 255, 0.3)), 2.5, t);
    if let Some(k) = click {
        // Drops thrown out from the tip, falling as they go.
        for i in 0..8 {
            let ang = PI * (0.55 + 0.26 * i as f32) + 0.12 * (i % 2) as f32;
            let r = 40.0 + 230.0 * k * (0.75 + 0.1 * (i % 3) as f32);
            let (x, y) = (88.0 + r * ang.cos(), 30.0 + r * ang.sin() + 120.0 * k * k);
            let size = 20.0 * (1.0 - k) + 5.0;
            fill(
                pm,
                &circle(x, y, size),
                &radial(
                    (x - size * 0.35, y - size * 0.35),
                    size * 1.3,
                    &[
                        (0.0, rgba(255, 255, 255, 1.0 - k * 0.6)),
                        (0.5, rgba(160, 162, 170, 1.0 - k * 0.6)),
                        (1.0, rgba(50, 46, 46, 1.0 - k * 0.6)),
                    ],
                ),
                still,
            );
        }
    }
}

// ---- orbit -------------------------------------------------------------------

const ORBIT_TIP: (f32, f32) = (82.0, 32.0);

/// A dark crystal dart, lavender at the edges, circled by a glowing ring
/// with a pearl on it. Clicking, the pearl races once round the ring.
fn orbit(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let dart = [
        (82.0, 32.0),
        (358.0, 215.0),
        (258.0, 250.0),
        (348.0, 418.0),
        (302.0, 402.0),
        (212.0, 282.0),
        (158.0, 358.0),
    ];
    // The ring: an ellipse around the dart, tilted along it; seen as an
    // open arc, thin at its ends.
    let ring_t = t.pre_translate(255.0, 245.0).pre_rotate(-25.7);
    let (rx, ry) = (186.0, 78.0);
    let at = |a: f32| (rx * a.cos(), ry * a.sin());
    let k = click.unwrap_or(0.0);
    if click.is_some() {
        // The whole ring, faintly, while the pearl goes round.
        let pts: Vec<(f32, f32)> = (0..=64).map(|i| at(i as f32 / 64.0 * TAU)).collect();
        line(
            pm,
            &open_line(&pts),
            &solid(rgba(220, 190, 255, 0.35 * (k * PI).sin())),
            4.0,
            ring_t,
        );
    }
    let outline = poly(&dart);
    glow(pm, &outline, (190, 150, 255), 0.6, t);
    shadow(pm, &outline, t, 0.9);
    // Dark glass, faceted: a lit lavender left face, a near-black middle.
    fill(
        pm,
        &outline,
        &linear(
            (82.0, 32.0),
            (330.0, 400.0),
            &[
                (0.0, rgba(200, 180, 255, 1.0)),
                (0.3, rgba(45, 40, 85, 1.0)),
                (0.6, rgba(18, 16, 34, 1.0)),
                (1.0, rgba(120, 85, 190, 1.0)),
            ],
        ),
        t,
    );
    let left_face = poly(&[(82.0, 32.0), (180.0, 220.0), (212.0, 282.0), (158.0, 358.0)]);
    fill(
        pm,
        &left_face,
        &linear(
            (82.0, 32.0),
            (190.0, 340.0),
            &[
                (0.0, rgba(225, 210, 255, 0.95)),
                (0.6, rgba(150, 130, 210, 0.9)),
                (1.0, rgba(90, 70, 150, 0.9)),
            ],
        ),
        t,
    );
    let tail_face = poly(&[
        (258.0, 250.0),
        (348.0, 418.0),
        (302.0, 402.0),
        (232.0, 262.0),
    ]);
    fill(
        pm,
        &tail_face,
        &linear(
            (240.0, 250.0),
            (330.0, 410.0),
            &[
                (0.0, rgba(170, 140, 230, 0.8)),
                (1.0, rgba(70, 50, 120, 0.9)),
            ],
        ),
        t,
    );
    line(
        pm,
        &open_line(&[(82.0, 32.0), (180.0, 220.0), (212.0, 282.0)]),
        &solid(rgba(230, 215, 255, 0.6)),
        3.0,
        t,
    );
    line(pm, &outline, &solid(rgba(215, 190, 255, 0.95)), 5.0, t);
    line(
        pm,
        &open_line(&[(82.0, 32.0), (358.0, 215.0)]),
        &solid(rgba(255, 245, 255, 0.95)),
        4.0,
        t,
    );
    // The ring, and the pearl on it.
    let (from, to) = (-1.22, PI);
    let steps = 60;
    for i in 0..steps {
        let (u0, u1) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
        let (a0, a1) = (from + (to - from) * u0, from + (to - from) * u1);
        let taper = ((u0 + u1) * 0.5 * PI).sin().powf(0.7);
        let seg = open_line(&[at(a0), at(a1)]);
        let c = if u0 < 0.5 {
            rgba(255, 215, 235, 0.95 * taper)
        } else {
            rgba(255, 195, 170, 0.95 * taper)
        };
        line(
            pm,
            &seg,
            &solid(rgba(230, 150, 220, 0.18 * taper)),
            26.0 * taper,
            ring_t,
        );
        line(pm, &seg, &solid(c), 1.5 + 8.0 * taper, ring_t);
    }
    let lap = click.map_or(0.0, |k| k * k * (3.0 - 2.0 * k) * TAU);
    let a = PI * 0.25 + lap;
    if click.is_some() {
        // A trail behind the pearl as it goes round.
        for j in 1..9 {
            let p = at(a - j as f32 * 0.14);
            fill(
                pm,
                &circle(p.0, p.1, 24.0 - j as f32 * 2.2),
                &solid(rgba(
                    235,
                    200,
                    255,
                    0.35 * (1.0 - j as f32 / 9.0) * (k * PI).sin(),
                )),
                ring_t,
            );
        }
    }
    let p = at(a);
    fill(
        pm,
        &circle(p.0, p.1, 34.0),
        &radial(
            (p.0 - 10.0, p.1 - 12.0),
            44.0,
            &[
                (0.0, rgba(255, 255, 255, 1.0)),
                (0.35, rgba(190, 185, 230, 1.0)),
                (0.75, rgba(90, 80, 140, 1.0)),
                (1.0, rgba(40, 32, 70, 1.0)),
            ],
        ),
        ring_t,
    );
}
