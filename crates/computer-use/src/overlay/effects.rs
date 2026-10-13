//! What a click and a held button show at the pointer's tip, besides each
//! pointer's own click (`overlay.click_effect_style`, `overlay.hold_effect`):
//! an expanding pulse, a burst of particles or sparkles for a click (two
//! beats for a double click), and a soft glow or a ring for as long as the
//! agent holds the button down (a drag, a drawing), fading when it lets go.
//! Drawn into the pointer's image, so they move with it on every platform.

use std::f32::consts::TAU;

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, Paint, PathBuilder, Pixmap, Point, RadialGradient,
    SpreadMode, Stroke, StrokeDash, Transform,
};

/// What a click shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClickStyle {
    /// Each pointer clicks its own way (the plain arrow ripples): as before.
    Ripple,
    /// A ring that expands and fades.
    Pulse,
    /// Particles flying out.
    Burst,
    /// Sparkles spinning out.
    Sparkle,
    None,
}

impl ClickStyle {
    /// By its name in the settings; anything else is the ripple.
    pub fn named(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "pulse" => Self::Pulse,
            "burst" => Self::Burst,
            "sparkle" => Self::Sparkle,
            "none" => Self::None,
            _ => Self::Ripple,
        }
    }
}

/// What a held button shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldStyle {
    /// A soft ring that stays and pulses.
    Glow,
    /// A ring with a dashed one going round it.
    Ring,
    None,
}

impl HoldStyle {
    pub fn named(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "ring" => Self::Ring,
            "none" => Self::None,
            _ => Self::Glow,
        }
    }
}

/// A click being shown: how, each beat's progress 0–1 (two for a double
/// click), and its colour.
#[derive(Debug, Clone, PartialEq)]
pub struct ClickFx {
    pub style: ClickStyle,
    pub beats: Vec<f32>,
    pub color: Color,
}

/// A held button: how, the phase 0–1 of its pulse, its opacity (it fades
/// in on the press and out after the release), and its colour.
#[derive(Debug, Clone, PartialEq)]
pub struct HoldFx {
    pub style: HoldStyle,
    pub phase: f32,
    pub alpha: f32,
    pub color: Color,
}

/// How far from the tip they reach (px at a unit of 1).
pub const REACH: f32 = 42.0;

/// Rounded to what shows, so the pointer is drawn again only when it would
/// look different.
pub fn key(click: Option<&ClickFx>, hold: Option<&HoldFx>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let q = |v: f32, n: f32| (v * n).round() as i32;
    let c = |c: Color| {
        let c = c.to_color_u8();
        [c.red(), c.green(), c.blue(), c.alpha()]
    };
    click
        .map(|f| {
            (
                f.style,
                f.beats.iter().map(|b| q(*b, 40.0)).collect::<Vec<_>>(),
                c(f.color),
            )
        })
        .hash(&mut h);
    hold.map(|f| (f.style, q(f.phase, 30.0), q(f.alpha, 30.0), c(f.color)))
        .hash(&mut h);
    h.finish()
}

fn with_alpha(c: Color, a: f32) -> Color {
    let mut c = c;
    c.set_alpha((c.alpha() * a).clamp(0.0, 1.0));
    c
}

fn lighten(c: Color, t: f32) -> Color {
    let m = |v: f32| v + (1.0 - v) * t;
    Color::from_rgba(m(c.red()), m(c.green()), m(c.blue()), c.alpha()).unwrap_or(c)
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn line(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        ..Stroke::default()
    }
}

fn ring(pm: &mut Pixmap, (x, y): (f32, f32), r: f32, width: f32, c: Color) {
    if let Some(p) = PathBuilder::from_circle(x, y, r.max(0.5)) {
        pm.stroke_path(
            &p,
            &solid(c),
            &line(width.max(0.3)),
            Transform::identity(),
            None,
        );
    }
}

fn disc(pm: &mut Pixmap, (x, y): (f32, f32), r: f32, c: Color) {
    if let Some(p) = PathBuilder::from_circle(x, y, r.max(0.3)) {
        pm.fill_path(
            &p,
            &solid(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// A soft disc: `stops` (position 0–1, opacity) of `c` from the centre out.
fn soft(pm: &mut Pixmap, (x, y): (f32, f32), r: f32, c: Color, stops: &[(f32, f32)]) {
    let stops = stops
        .iter()
        .map(|(at, a)| GradientStop::new(*at, with_alpha(c, *a)))
        .collect();
    if let (Some(p), Some(shader)) = (
        PathBuilder::from_circle(x, y, r.max(0.5)),
        RadialGradient::new(
            Point::from_xy(x, y),
            Point::from_xy(x, y),
            r.max(0.5),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        ),
    ) {
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// A four-pointed star.
fn star(pm: &mut Pixmap, (x, y): (f32, f32), r: f32, turn: f32, c: Color) {
    let k = r * 0.3;
    let mut pb = PathBuilder::new();
    for i in 0..8 {
        let a = turn + i as f32 * TAU / 8.0;
        let d = if i % 2 == 0 { r } else { k };
        let (px, py) = (x + d * a.cos(), y + d * a.sin());
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(
            &p,
            &solid(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// The same number from `i` every time, 0–1.
fn noise(i: usize) -> f32 {
    ((i as f32 * 12.9898).sin() * 43_758.547).fract().abs()
}

/// A click at `at` (px), `s` px per unit.
pub fn draw_click(pm: &mut Pixmap, at: (f32, f32), s: f32, fx: &ClickFx) {
    for &t in &fx.beats {
        if !(0.0..1.0).contains(&t) {
            continue;
        }
        let out = 1.0 - (1.0 - t).powi(3);
        let fade = 1.0 - t;
        match fx.style {
            ClickStyle::Pulse => {
                if t < 0.3 {
                    soft(
                        pm,
                        at,
                        12.0 * s,
                        fx.color,
                        &[(0.0, 0.45 * (1.0 - t / 0.3)), (1.0, 0.0)],
                    );
                }
                ring(
                    pm,
                    at,
                    (4.0 + 30.0 * out) * s,
                    (3.4 * fade + 0.8) * s,
                    with_alpha(fx.color, fade),
                );
                let t2 = t - 0.2;
                if t2 > 0.0 {
                    let out2 = 1.0 - (1.0 - t2 / 0.8).powi(3);
                    let fade2 = 1.0 - t2 / 0.8;
                    ring(
                        pm,
                        at,
                        (3.0 + 20.0 * out2) * s,
                        1.6 * s,
                        with_alpha(lighten(fx.color, 0.4), 0.7 * fade2),
                    );
                }
            }
            ClickStyle::Burst => {
                const N: usize = 12;
                for i in 0..N {
                    let a = i as f32 * TAU / N as f32 + 0.35 * (i as f32 * 2.1).sin();
                    let speed = 0.7 + 0.6 * noise(i);
                    let d = (5.0 + 30.0 * speed * out) * s;
                    let (ux, uy) = (a.cos(), a.sin());
                    let p = (at.0 + ux * d, at.1 + uy * d);
                    let c = if i % 3 == 0 {
                        lighten(fx.color, 0.55)
                    } else {
                        fx.color
                    };
                    // A streak behind each particle, then the particle.
                    let back = d * 0.55;
                    let mut pb = PathBuilder::new();
                    pb.move_to(at.0 + ux * back, at.1 + uy * back);
                    pb.line_to(p.0, p.1);
                    if let Some(path) = pb.finish() {
                        pm.stroke_path(
                            &path,
                            &solid(with_alpha(c, 0.45 * fade)),
                            &line((1.6 * fade + 0.4) * s),
                            Transform::identity(),
                            None,
                        );
                    }
                    disc(pm, p, (2.8 * fade + 0.6) * s, with_alpha(c, fade.powf(1.2)));
                }
            }
            ClickStyle::Sparkle => {
                if t < 0.45 {
                    let k = t / 0.45;
                    soft(
                        pm,
                        at,
                        11.0 * s * (1.0 - 0.4 * k),
                        lighten(fx.color, 0.7),
                        &[(0.0, 0.9 * (1.0 - k)), (1.0, 0.0)],
                    );
                }
                const N: usize = 7;
                for i in 0..N {
                    let a = i as f32 * TAU / N as f32 + t * 1.3;
                    let d = (7.0 + 22.0 * out * (0.8 + 0.4 * noise(i + 3))) * s;
                    // Up at once, then smaller as they fade.
                    let size =
                        (2.5 + 4.0 * noise(i + 7)) * s * (t * 5.0).min(1.0) * (1.0 - 0.5 * t * t);
                    let c = if i % 2 == 0 {
                        lighten(fx.color, 0.65)
                    } else {
                        fx.color
                    };
                    star(
                        pm,
                        (at.0 + a.cos() * d, at.1 + a.sin() * d),
                        size,
                        a + t * 2.0,
                        with_alpha(c, fade),
                    );
                }
            }
            ClickStyle::Ripple | ClickStyle::None => {}
        }
    }
}

/// A held button at `at` (px), `s` px per unit: drawn under the pointer.
pub fn draw_hold(pm: &mut Pixmap, at: (f32, f32), s: f32, fx: &HoldFx) {
    let a = fx.alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let beat = 0.5 + 0.5 * (fx.phase * TAU).sin();
    match fx.style {
        HoldStyle::Glow => {
            let r = (17.0 + 3.0 * beat) * s;
            soft(
                pm,
                at,
                r,
                fx.color,
                &[
                    (0.0, 0.10 * a),
                    (0.5, (0.30 + 0.12 * beat) * a),
                    (0.74, (0.55 + 0.15 * beat) * a),
                    (1.0, 0.0),
                ],
            );
            ring(
                pm,
                at,
                r * 0.74,
                1.4 * s,
                with_alpha(lighten(fx.color, 0.35), 0.7 * a),
            );
        }
        HoldStyle::Ring => {
            ring(
                pm,
                at,
                12.0 * s,
                (2.2 + 0.6 * beat) * s,
                with_alpha(fx.color, 0.9 * a),
            );
            // A dashed ring going round it.
            let r = 17.0 * s;
            if let Some(p) = PathBuilder::from_circle(at.0, at.1, r) {
                let mut st = line(1.8 * s);
                let dash = TAU * r / 12.0;
                st.dash = StrokeDash::new(vec![dash * 0.55, dash * 0.45], fx.phase * dash * 2.0);
                pm.stroke_path(
                    &p,
                    &solid(with_alpha(lighten(fx.color, 0.3), 0.8 * a)),
                    &st,
                    Transform::identity(),
                    None,
                );
            }
        }
        HoldStyle::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_effect_draws_around_the_tip_and_none_draws_nothing() {
        let c = Color::from_rgba8(30, 136, 229, 255);
        let painted = |f: &dyn Fn(&mut Pixmap)| {
            let mut pm = Pixmap::new(120, 120).unwrap();
            f(&mut pm);
            pm.pixels().iter().filter(|p| p.alpha() > 0).count()
        };
        for style in [ClickStyle::Pulse, ClickStyle::Burst, ClickStyle::Sparkle] {
            for t in [0.1, 0.4, 0.8] {
                let fx = ClickFx {
                    style,
                    beats: vec![t],
                    color: c,
                };
                let n = painted(&|pm| draw_click(pm, (60.0, 60.0), 1.0, &fx));
                assert!(n > 20, "{style:?} at {t}: {n}");
            }
        }
        for style in [ClickStyle::Ripple, ClickStyle::None] {
            let fx = ClickFx {
                style,
                beats: vec![0.3],
                color: c,
            };
            assert_eq!(painted(&|pm| draw_click(pm, (60.0, 60.0), 1.0, &fx)), 0);
        }
        for style in [HoldStyle::Glow, HoldStyle::Ring] {
            let fx = HoldFx {
                style,
                phase: 0.3,
                alpha: 1.0,
                color: c,
            };
            assert!(painted(&|pm| draw_hold(pm, (60.0, 60.0), 1.0, &fx)) > 100);
            let gone = HoldFx { alpha: 0.0, ..fx };
            assert_eq!(painted(&|pm| draw_hold(pm, (60.0, 60.0), 1.0, &gone)), 0);
        }
        assert_eq!(ClickStyle::named(" Burst "), ClickStyle::Burst);
        assert_eq!(ClickStyle::named("anything"), ClickStyle::Ripple);
        assert_eq!(HoldStyle::named("RING"), HoldStyle::Ring);
    }
}
