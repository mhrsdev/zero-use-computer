//! The agent's pointers, drawn by hand: crystal, paper, jelly, ice, liquid
//! metal and orbit, each with its own way of clicking. Shapes are in units
//! of about a pixel at scale 1, the tip at (0, 0) pointing up and left; a
//! pointer is about 28 units tall.

use std::f32::consts::{PI, TAU};

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder,
    Pixmap, Point, RadialGradient, Shader, SpreadMode, Stroke, Transform,
};

use super::draw::CursorStyle;

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
    match style {
        CursorStyle::Classic => {}
        CursorStyle::Crystal => crystal(pm, base, click),
        CursorStyle::Paper => paper(pm, base, click),
        CursorStyle::Jelly => jelly(pm, base, click),
        CursorStyle::Ice => ice(pm, base, click),
        CursorStyle::Metal => metal(pm, base, click),
        CursorStyle::Orbit => orbit(pm, base, click),
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

fn with_shader(shader: Option<Shader<'static>>, fallback: Color) -> Paint<'static> {
    match shader {
        Some(shader) => Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        },
        None => solid(fallback),
    }
}

fn linear(a: (f32, f32), b: (f32, f32), stops: &[(f32, Color)]) -> Paint<'static> {
    let fallback = stops.first().map_or(Color::WHITE, |s| s.1);
    with_shader(
        LinearGradient::new(
            Point::from_xy(a.0, a.1),
            Point::from_xy(b.0, b.1),
            stops
                .iter()
                .map(|(p, c)| GradientStop::new(*p, *c))
                .collect(),
            SpreadMode::Pad,
            Transform::identity(),
        ),
        fallback,
    )
}

fn radial(c: (f32, f32), r: f32, stops: &[(f32, Color)]) -> Paint<'static> {
    let fallback = stops.first().map_or(Color::WHITE, |s| s.1);
    with_shader(
        RadialGradient::new(
            Point::from_xy(c.0, c.1),
            Point::from_xy(c.0, c.1),
            r.max(0.01),
            stops
                .iter()
                .map(|(p, c)| GradientStop::new(*p, *c))
                .collect(),
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

/// A soft drop shadow of `path`: layered strokes, offset down and right.
fn shadow(pm: &mut Pixmap, path: &Option<Path>, t: Transform, strength: f32) {
    let t = t.pre_translate(0.9, 1.8);
    for (w, a) in [(6.0, 0.04), (4.0, 0.06), (2.2, 0.08)] {
        line(pm, path, &solid(rgba(0, 0, 0, a * strength)), w, t);
    }
    fill(pm, path, &solid(rgba(0, 0, 0, 0.14 * strength)), t);
}

/// Squash along the pointer's axis (the diagonal from the tip), keeping the
/// tip where it is: `a` > 0 flattens it, < 0 stretches it.
fn squash(t: Transform, a: f32) -> Transform {
    t.pre_rotate(45.0)
        .pre_scale(1.0 - a, 1.0 + a * 0.8)
        .pre_rotate(-45.0)
}

/// A colour around the hue circle (0–1), bright, for rainbow light.
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
    let soft = |v: f32| 0.35 + 0.65 * v;
    Color::from_rgba(soft(r), soft(g), soft(b), a).unwrap_or(Color::WHITE)
}

/// The dart outline (with a tail), symmetric about the diagonal.
const DART: [(f32, f32); 7] = [
    (0.0, 0.0),
    (24.0, 10.0),
    (14.5, 13.6),
    (22.5, 20.4),
    (20.4, 22.5),
    (13.6, 14.5),
    (10.0, 24.0),
];

// ---- crystal -----------------------------------------------------------------

/// Cut glass: clear facets over a deep blue core, rainbow light along the
/// edges. Clicking, light breaks out of it in a burst of colours.
fn crystal(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let outline = poly(&DART);
    shadow(pm, &outline, t, 0.8);
    // The core: dark, glassy.
    fill(
        pm,
        &outline,
        &linear(
            (0.0, 0.0),
            (20.0, 20.0),
            &[
                (0.0, rgba(225, 240, 255, 0.95)),
                (0.35, rgba(120, 150, 200, 0.85)),
                (0.7, rgba(35, 45, 75, 0.92)),
                (1.0, rgba(90, 120, 170, 0.9)),
            ],
        ),
        t,
    );
    // Facets, each lit from its own angle.
    let core = (9.0, 9.0);
    let facet_light = [0.55, 0.12, 0.35, 0.05, 0.4, 0.18, 0.6];
    for i in 0..DART.len() {
        let (a, b) = (DART[i], DART[(i + 1) % DART.len()]);
        let f = poly(&[a, b, core]);
        let l = facet_light[i];
        fill(
            pm,
            &f,
            &linear(
                a,
                b,
                &[
                    (0.0, rgba(255, 255, 255, l)),
                    (0.5, rgba(170, 200, 255, l * 0.4)),
                    (1.0, rgba(255, 255, 255, l * 0.7)),
                ],
            ),
            t,
        );
        line(
            pm,
            &poly(&[core, a]),
            &solid(rgba(255, 255, 255, 0.35)),
            0.35,
            t,
        );
    }
    // Rainbow light along the edges, white at the very edge.
    let rim = linear(
        (0.0, 0.0),
        (24.0, 24.0),
        &[
            (0.0, hue(0.55, 0.95)),
            (0.25, hue(0.15, 0.9)),
            (0.5, hue(0.85, 0.9)),
            (0.75, hue(0.35, 0.9)),
            (1.0, hue(0.6, 0.9)),
        ],
    );
    line(pm, &outline, &rim, 1.5, t);
    line(pm, &outline, &solid(rgba(255, 255, 255, 0.9)), 0.45, t);
    // Glints.
    for (x, y, r) in [(3.0, 2.5, 1.3), (17.0, 8.5, 0.9), (8.5, 17.5, 0.8)] {
        fill(
            pm,
            &circle(x, y, r * 1.6),
            &radial(
                (x, y),
                r * 1.6,
                &[
                    (0.0, rgba(255, 255, 255, 0.95)),
                    (1.0, rgba(255, 255, 255, 0.0)),
                ],
            ),
            t,
        );
    }
    if let Some(k) = click {
        // A flash through the glass, and rays of colour breaking out.
        let flash = (k * PI).sin();
        fill(pm, &outline, &solid(rgba(255, 255, 255, 0.55 * flash)), t);
        for i in 0..9 {
            let ang = -0.35 + i as f32 * (TAU / 9.0);
            let (r0, r1) = (4.0 + 6.0 * k, 10.0 + 22.0 * k);
            let w = 0.12;
            let p = |r: f32, d: f32| (8.0 + r * (ang + d).cos(), 8.0 + r * (ang + d).sin());
            let ray = poly(&[p(r0, -w), p(r1, -w * 0.4), p(r1, w * 0.4), p(r0, w)]);
            fill(pm, &ray, &solid(hue(i as f32 / 9.0, 0.75 * (1.0 - k))), t);
        }
    }
}

// ---- paper -------------------------------------------------------------------

/// A folded sheet, satin silver-gold, its inside lit warm orange. Clicking,
/// the folded wing opens out flat and folds back.
fn paper(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let tip = (0.0, 0.0);
    let crease = (14.0, 14.5);
    let left = [tip, (9.5, 28.0), crease];
    // The other wing turns about the crease: `open` 1 lies flat beside the
    // left one, 0 stands edge-on, below 0 shows its inside.
    let rest = -0.5;
    let open = match click {
        Some(k) => rest + (1.0 - rest) * (k * PI).sin(),
        None => rest,
    };
    let flat = (27.0, 9.0);
    // Where the wing's outer corner is: across the crease by `open`.
    let d = (crease.0 / 20.15, crease.1 / 20.15);
    let along = flat.0 * d.0 + flat.1 * d.1;
    let foot = (d.0 * along, d.1 * along);
    let corner = (
        foot.0 + (flat.0 - foot.0) * open,
        foot.1 + (flat.1 - foot.1) * open,
    );
    let wing = [tip, corner, crease];

    let left_p = poly(&left);
    let wing_p = poly(&wing);
    shadow(pm, &left_p, t, 0.9);
    if open > 0.0 {
        shadow(pm, &wing_p, t, 0.6);
    }
    let satin = |a: (f32, f32), b: (f32, f32)| {
        linear(
            a,
            b,
            &[
                (0.0, rgba(250, 244, 236, 1.0)),
                (0.45, rgba(214, 196, 178, 1.0)),
                (0.8, rgba(170, 148, 128, 1.0)),
                (1.0, rgba(232, 214, 196, 1.0)),
            ],
        )
    };
    // The inside of the fold, warm where the light gets in.
    let inside = linear(
        tip,
        corner,
        &[
            (0.0, rgba(255, 214, 140, 1.0)),
            (0.35, rgba(240, 150, 50, 1.0)),
            (1.0, rgba(140, 80, 30, 1.0)),
        ],
    );
    fill(pm, &left_p, &satin((0.0, 0.0), (12.0, 26.0)), t);
    if open < 0.0 {
        fill(pm, &wing_p, &inside, t);
        // The glow where the fold opens.
        fill(
            pm,
            &circle(crease.0 * 0.7, crease.1 * 0.7, 6.0),
            &radial(
                (crease.0 * 0.7, crease.1 * 0.7),
                6.0,
                &[
                    (0.0, rgba(255, 190, 90, 0.85)),
                    (1.0, rgba(255, 150, 40, 0.0)),
                ],
            ),
            t,
        );
    } else {
        fill(pm, &wing_p, &satin((0.0, 0.0), (26.0, 10.0)), t);
    }
    // Edges and the crease.
    line(pm, &left_p, &solid(rgba(120, 95, 70, 0.55)), 0.5, t);
    line(pm, &wing_p, &solid(rgba(120, 95, 70, 0.55)), 0.5, t);
    line(
        pm,
        &poly(&[tip, crease]),
        &solid(rgba(255, 250, 240, 0.95)),
        0.6,
        t,
    );
}

// ---- jelly -------------------------------------------------------------------

/// A soft jelly in blue to violet, glossy. Clicking, it squashes flat and
/// wobbles back.
fn jelly(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let a = match click {
        // Pressed flat at once, then wobbling back.
        Some(k) => 0.55 * (-3.0 * k).exp() * (k * 3.4 * PI).sin(),
        None => 0.0,
    };
    let t = squash(t, a);
    let blob = rounded(
        &[
            (0.0, 0.0),
            (24.0, 10.5),
            (15.5, 14.0),
            (21.5, 22.5),
            (17.5, 25.5),
            (12.5, 16.5),
            (8.5, 25.0),
        ],
        &[1.2, 4.0, 3.0, 4.5, 4.5, 3.0, 4.0],
    );
    shadow(pm, &blob, t, 0.9);
    fill(
        pm,
        &blob,
        &linear(
            (0.0, 0.0),
            (20.0, 24.0),
            &[
                (0.0, rgba(205, 215, 255, 0.97)),
                (0.35, rgba(150, 130, 245, 0.95)),
                (0.7, rgba(150, 90, 225, 0.95)),
                (1.0, rgba(215, 130, 225, 0.97)),
            ],
        ),
        t,
    );
    // Light through it: brighter inside, a rim of pink at the far edge.
    fill(
        pm,
        &blob,
        &radial(
            (9.0, 9.0),
            14.0,
            &[
                (0.0, rgba(255, 255, 255, 0.35)),
                (1.0, rgba(255, 255, 255, 0.0)),
            ],
        ),
        t,
    );
    line(pm, &blob, &solid(rgba(255, 220, 250, 0.6)), 0.9, t);
    // Gloss.
    let gloss = rounded(&[(2.0, 3.5), (9.0, 5.5), (4.0, 12.0)], &[1.5, 2.5, 2.0]);
    fill(
        pm,
        &gloss,
        &linear(
            (2.0, 3.5),
            (6.0, 10.0),
            &[
                (0.0, rgba(255, 255, 255, 0.9)),
                (1.0, rgba(255, 255, 255, 0.0)),
            ],
        ),
        t,
    );
    fill(
        pm,
        &circle(19.0, 21.5, 1.2),
        &solid(rgba(255, 255, 255, 0.7)),
        t,
    );
}

// ---- ice ---------------------------------------------------------------------

/// Frosted ice with a glowing blue crystal inside. Clicking, it melts and
/// drips, then freezes again.
fn ice(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    // How melted: up and back down.
    let m = click.map_or(0.0, |k| (k * PI).sin());
    let sag = 3.0 * m;
    let pts = [
        (0.0, 0.0),
        (26.0, 9.0 + sag * 0.6),
        (13.0, 13.0 + sag * 0.5),
        (9.0 + sag * 0.3, 27.0 + sag),
    ];
    let radius = 0.4 + 2.5 * m;
    let body = rounded(&pts, &[0.6, radius, radius, radius]);
    shadow(pm, &body, t, 0.8);
    let cool = 1.0 - 0.45 * m;
    // Two planes of frost, lit differently, split by the ridge.
    let ridge = (13.0, 13.0 + sag * 0.5);
    let upper = rounded(&[pts[0], pts[1], ridge], &[0.6, radius, radius]);
    let lower = rounded(&[pts[0], ridge, pts[3]], &[0.6, radius, radius]);
    fill(
        pm,
        &upper,
        &linear(
            (0.0, 0.0),
            (26.0, 9.0),
            &[
                (0.0, rgba(245, 252, 255, 0.95 * cool + 0.3 * m)),
                (1.0, rgba(170, 205, 230, 0.9 * cool + 0.3 * m)),
            ],
        ),
        t,
    );
    fill(
        pm,
        &lower,
        &linear(
            (0.0, 0.0),
            (9.0, 27.0),
            &[
                (0.0, rgba(200, 228, 245, 0.95 * cool + 0.3 * m)),
                (1.0, rgba(120, 175, 215, 0.92 * cool + 0.3 * m)),
            ],
        ),
        t,
    );
    // The crystal inside, glowing.
    let gem = poly(&[
        (9.5, 15.0),
        (12.5, 14.5),
        (11.5, 23.0 + sag * 0.6),
        (9.0, 22.0 + sag * 0.6),
    ]);
    line(pm, &gem, &solid(rgba(60, 230, 255, 0.35 * cool)), 2.5, t);
    fill(
        pm,
        &gem,
        &linear(
            (9.0, 15.0),
            (12.0, 23.0),
            &[
                (0.0, rgba(140, 250, 255, 0.95)),
                (1.0, rgba(0, 170, 230, 0.95)),
            ],
        ),
        t,
    );
    // Edges: icy light, wet when melting.
    line(pm, &body, &solid(rgba(120, 230, 255, 0.85)), 0.9, t);
    line(
        pm,
        &poly(&[pts[0], ridge]),
        &solid(rgba(255, 255, 255, 0.85 * cool)),
        0.5,
        t,
    );
    if let Some(k) = click {
        // Drops falling from the corners, and a little pool at the bottom.
        for (i, (x, y)) in [
            (pts[3].0, pts[3].1),
            (pts[1].0, pts[1].1),
            (18.0, 19.0 + sag),
        ]
        .into_iter()
        .enumerate()
        {
            let start = i as f32 * 0.12;
            let d = ((k - start) / (1.0 - start)).clamp(0.0, 1.0);
            if d <= 0.0 {
                continue;
            }
            let fall = 2.0 + 16.0 * d * d;
            let r = 1.3 * (1.0 - 0.4 * d);
            let drop = rounded(
                &[
                    (x, y + fall - r * 2.2),
                    (x + r, y + fall),
                    (x, y + fall + r),
                    (x - r, y + fall),
                ],
                &[0.2, r, r, r],
            );
            fill(pm, &drop, &solid(rgba(150, 220, 255, 0.85 * (1.0 - d))), t);
        }
        let pool = PathBuilder::from_oval(
            tiny_skia::Rect::from_xywh(
                pts[3].0 - 6.0 * m,
                pts[3].1 + 7.0,
                12.0 * m + 0.1,
                2.2 * m + 0.1,
            )
            .unwrap_or(tiny_skia::Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap()),
        );
        fill(pm, &pool, &solid(rgba(150, 215, 245, 0.45 * m)), t);
    }
}

// ---- liquid metal ------------------------------------------------------------

/// Liquid chrome, its tail a hanging drop with a warm reflection. Clicking,
/// it gives a little and drops of metal splash out from the tip.
fn metal(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let give = click.map_or(0.0, |k| {
        0.16 * (-3.0 * k).exp() * (k * 2.5 * PI).cos() * (k * 12.0).min(1.0)
    });
    let t = squash(t, give);
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.quad_to(12.0, 3.0, 22.0, 10.5);
    pb.quad_to(16.0, 11.5, 13.5, 14.0);
    pb.cubic_to(16.0, 17.0, 23.5, 19.5, 22.0, 24.5);
    pb.cubic_to(20.5, 28.5, 15.0, 27.5, 14.5, 23.5);
    pb.cubic_to(14.0, 20.0, 13.0, 17.5, 11.0, 16.0);
    pb.quad_to(9.0, 18.5, 7.0, 23.0);
    pb.quad_to(3.0, 12.0, 0.0, 0.0);
    pb.close();
    let body = pb.finish();
    shadow(pm, &body, t, 1.0);
    // Chrome: bands of dark and light across it.
    fill(
        pm,
        &body,
        &linear(
            (0.0, 0.0),
            (16.0, 22.0),
            &[
                (0.0, rgba(250, 250, 252, 1.0)),
                (0.2, rgba(60, 62, 70, 1.0)),
                (0.38, rgba(235, 238, 245, 1.0)),
                (0.55, rgba(70, 72, 82, 1.0)),
                (0.72, rgba(225, 228, 235, 1.0)),
                (0.85, rgba(235, 190, 130, 1.0)),
                (1.0, rgba(160, 110, 70, 1.0)),
            ],
        ),
        t,
    );
    // A bright streak along the top edge, and the drop's highlight.
    let mut hl = PathBuilder::new();
    hl.move_to(2.0, 1.5);
    hl.quad_to(11.0, 4.0, 18.5, 9.5);
    line(pm, &hl.finish(), &solid(rgba(255, 255, 255, 0.85)), 0.9, t);
    fill(
        pm,
        &circle(18.2, 22.6, 2.2),
        &radial(
            (18.2, 22.6),
            2.2,
            &[
                (0.0, rgba(255, 255, 255, 0.95)),
                (1.0, rgba(255, 255, 255, 0.0)),
            ],
        ),
        t,
    );
    line(pm, &body, &solid(rgba(30, 30, 36, 0.55)), 0.35, t);
    if let Some(k) = click {
        // Drops thrown out from the tip, falling as they go.
        for i in 0..7 {
            let ang = PI * (0.62 + 0.27 * i as f32) + 0.1 * (i % 2) as f32;
            let r = 3.0 + 15.0 * k * (0.75 + 0.1 * (i % 3) as f32);
            let (x, y) = (r * ang.cos(), r * ang.sin() + 9.0 * k * k);
            let size = 1.5 * (1.0 - k) + 0.3;
            fill(
                pm,
                &circle(x, y, size),
                &radial(
                    (x - size * 0.35, y - size * 0.35),
                    size * 1.3,
                    &[
                        (0.0, rgba(255, 255, 255, 1.0 - k * 0.6)),
                        (0.5, rgba(150, 152, 160, 1.0 - k * 0.6)),
                        (1.0, rgba(40, 40, 46, 1.0 - k * 0.6)),
                    ],
                ),
                t,
            );
        }
    }
}

// ---- orbit -------------------------------------------------------------------

/// A dark crystal dart circled by a ring with a pearl on it. Clicking, the
/// pearl races once round the ring.
fn orbit(pm: &mut Pixmap, t: Transform, click: Option<f32>) {
    let outline = poly(&DART);
    // The ring: an ellipse around the dart, tilted along it.
    let ring_t = t.pre_translate(12.5, 13.0).pre_rotate(-30.0);
    let (rx, ry) = (17.0, 6.5);
    let ring_point = |a: f32| (rx * a.cos(), ry * a.sin());
    let ring = |pm: &mut Pixmap, from: f32, to: f32, alpha: f32| {
        let mut pb = PathBuilder::new();
        let steps = 40;
        for i in 0..=steps {
            let a = from + (to - from) * i as f32 / steps as f32;
            let p = ring_point(a);
            if i == 0 {
                pb.move_to(p.0, p.1);
            } else {
                pb.line_to(p.0, p.1);
            }
        }
        let glow = linear(
            (-rx, 0.0),
            (rx, 0.0),
            &[
                (0.0, rgba(255, 190, 170, alpha)),
                (0.5, rgba(200, 150, 255, alpha)),
                (1.0, rgba(255, 220, 240, alpha)),
            ],
        );
        line(pm, &pb.finish(), &glow, 0.9, ring_t);
    };
    // The back of the ring, behind the dart.
    ring(pm, PI, TAU, 0.55);
    shadow(pm, &outline, t, 0.9);
    // Dark glass, violet at the edges.
    fill(
        pm,
        &outline,
        &linear(
            (0.0, 0.0),
            (22.0, 22.0),
            &[
                (0.0, rgba(190, 170, 255, 1.0)),
                (0.3, rgba(40, 35, 75, 1.0)),
                (0.65, rgba(15, 14, 30, 1.0)),
                (1.0, rgba(120, 80, 190, 1.0)),
            ],
        ),
        t,
    );
    let core = (9.0, 9.0);
    for (i, l) in [0.22, 0.05, 0.15, 0.02, 0.18, 0.06, 0.3]
        .into_iter()
        .enumerate()
    {
        let (a, b) = (DART[i], DART[(i + 1) % DART.len()]);
        fill(pm, &poly(&[a, b, core]), &solid(rgba(200, 180, 255, l)), t);
    }
    line(pm, &outline, &solid(rgba(200, 170, 255, 0.9)), 0.7, t);
    // The front of the ring, and the pearl on it.
    let lap = click.map_or(0.0, |k| {
        let e = k * k * (3.0 - 2.0 * k);
        e * TAU
    });
    ring(
        pm,
        0.0,
        PI,
        0.9 + 0.1 * click.map_or(0.0, |k| (k * PI).sin()),
    );
    let at = 0.35 + lap;
    if let Some(k) = click {
        // A trail behind the pearl as it goes round.
        for j in 1..8 {
            let a = at - j as f32 * 0.16;
            let p = ring_point(a);
            fill(
                pm,
                &circle(p.0, p.1, 1.6 - j as f32 * 0.15),
                &solid(rgba(
                    230,
                    200,
                    255,
                    0.35 * (1.0 - j as f32 / 8.0) * (k * PI).sin(),
                )),
                ring_t,
            );
        }
    }
    let p = ring_point(at);
    fill(
        pm,
        &circle(p.0, p.1, 2.2),
        &radial(
            (p.0 - 0.7, p.1 - 0.7),
            2.8,
            &[
                (0.0, rgba(255, 255, 255, 1.0)),
                (0.45, rgba(170, 160, 210, 1.0)),
                (1.0, rgba(40, 35, 70, 1.0)),
            ],
        ),
        ring_t,
    );
}
