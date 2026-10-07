//! Natural mouse paths. Some apps (and anti-bot checks) notice a pointer
//! that jumps, or that goes in a perfectly straight line at an even speed.
//! A path here starts quickly and settles slowly, takes a hand's time for
//! its length (Fitts's law) and ends exactly on the target, in one of five
//! ways ([`Style`]): a hand's curve (a bow to one side, a tremor, a long
//! reach sometimes a touch past and back), a sine wave, a circular arc, a
//! spring that overshoots and settles, or a spiral in to the target.
//! [`travel`] gives the points, one per [`STEP`], for the backends to send
//! as pointer motion (`natural_mouse`, `mouse_path`); the overlay's
//! pointer glides along [`Path::at`] too.

use std::f64::consts::{PI, TAU};
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::Duration;

/// Time between two points of a path (a 125 Hz mouse).
pub const STEP: Duration = Duration::from_millis(8);

/// A small random number generator (xorshift64*): good enough for paths,
/// and no dependency.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seeded from the clock and the process's hash keys.
    pub fn new() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        Self::seeded(h.finish())
    }

    pub fn seeded(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [a, b).
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

impl Default for Rng {
    fn default() -> Self {
        Self::new()
    }
}

/// How a path is to be travelled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// Reaching for a point (button up): free to curve and overshoot.
    Reach,
    /// Dragging with a button held: straight on (a drag can draw a line,
    /// in a paint program), only the timing and a faint tremor a hand's.
    Drag,
}

/// The way a path goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    /// A hand's curve: a bow to one side, a tremor, a long reach sometimes
    /// a touch past and back.
    Hand,
    /// A sine wave across the way, one or two swings, calm at both ends.
    Sine,
    /// A circular arc.
    Arc,
    /// A spring: past the target and back, settling (a damped oscillation).
    Spring,
    /// A spiral in to the target, a sixth to a third of a turn.
    Spiral,
}

/// What `mouse_path` and `overlay.cursor_path` may be: "mixed" (one of the
/// styles at random for each move) or a style's name.
pub const PATH_STYLES: [&str; 6] = ["mixed", "hand", "sine", "arc", "spring", "spiral"];

impl Style {
    pub const ALL: [Style; 5] = [
        Style::Hand,
        Style::Sine,
        Style::Arc,
        Style::Spring,
        Style::Spiral,
    ];

    pub fn name(self) -> &'static str {
        PATH_STYLES[self as usize + 1]
    }

    /// The style a setting names; None for "mixed" (or anything else).
    pub fn named(s: &str) -> Option<Style> {
        let s = s.trim();
        Style::ALL
            .into_iter()
            .find(|st| st.name().eq_ignore_ascii_case(s))
    }

    /// The style for one move under `setting`: the one it names, or one at
    /// random ("mixed").
    pub fn pick(setting: &str, rng: &mut Rng) -> Style {
        Style::named(setting).unwrap_or_else(|| Style::ALL[(rng.unit() * 5.0) as usize % 5])
    }
}

/// The real mouse's `mouse_path` setting (0: mixed, else a style + 1).
static MOUSE_STYLE: AtomicU8 = AtomicU8::new(0);

/// How a reach feels: how fast it goes, how often it overshoots and how
/// much it trembles, each as a multiple of a hand's (1 is as made).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feel {
    /// 0.5 to 2: the pace.
    pub speed: f64,
    /// 0 to 1: how often a reach goes a touch past and back. For the real
    /// mouse, the share of long hand reaches that do (set by
    /// `mouse_overshoot`, 0 by default); for a whole path drawn at once (the
    /// overlay's pointer), a share of a hand's own chance (1: as made).
    pub overshoot: f64,
    /// 0 to 2: the tremor.
    pub jitter: f64,
}

impl Default for Feel {
    fn default() -> Self {
        Feel {
            speed: 1.0,
            overshoot: 1.0,
            jitter: 1.0,
        }
    }
}

impl Feel {
    /// From the settings (`mouse_speed`, `mouse_overshoot`, `mouse_jitter`).
    pub fn from_config(c: &crate::config::Config) -> Feel {
        Feel {
            speed: c.mouse_speed.clamp(0.5, 2.0),
            overshoot: f64::from(c.mouse_overshoot.min(100)) / 100.0,
            jitter: f64::from(c.mouse_jitter.min(200)) / 100.0,
        }
    }
}

/// The real mouse's feel, in hundredths (speed, overshoot, jitter).
static MOUSE_FEEL: [AtomicU32; 3] = [AtomicU32::new(100), AtomicU32::new(0), AtomicU32::new(100)];

fn mouse_feel() -> Feel {
    let get = |i: usize| f64::from(MOUSE_FEEL[i].load(Ordering::Relaxed)) / 100.0;
    Feel {
        speed: get(0),
        overshoot: get(1),
        jitter: get(2),
    }
}

/// Take `mouse_path` and how it feels from the settings, for [`travel`].
pub fn configure(config: &crate::config::Config) {
    let v = Style::named(&config.mouse_path).map_or(0, |s| s as u8 + 1);
    MOUSE_STYLE.store(v, Ordering::Relaxed);
    let f = Feel::from_config(config);
    for (slot, v) in MOUSE_FEEL.iter().zip([f.speed, f.overshoot, f.jitter]) {
        slot.store((v * 100.0).round() as u32, Ordering::Relaxed);
    }
}

fn mouse_setting() -> &'static str {
    PATH_STYLES[usize::from(MOUSE_STYLE.load(Ordering::Relaxed)).min(5)]
}

/// How long a move of `d` px takes, about (ms): longer reaches take longer,
/// but not in proportion (Fitts's law).
fn duration_ms(d: f64, kind: Kind, rng: &mut Rng) -> f64 {
    let ms = (60.0 + 50.0 * (1.0 + d / 12.0).log2()) * rng.range(0.85, 1.15);
    match kind {
        Kind::Reach => ms.clamp(100.0, 600.0),
        Kind::Drag => (ms * 1.25).clamp(150.0, 800.0),
    }
}

/// Quick to start, slower to settle: minimum jerk on a skewed clock.
fn ease(t: f64) -> f64 {
    let u = t.clamp(0.0, 1.0).powf(0.9);
    u * u * u * (10.0 - 15.0 * u + 6.0 * u * u)
}

/// One way from `from` to `to`, as a function of time ([`Path::at`]).
#[derive(Debug, Clone)]
pub struct Path {
    from: (f64, f64),
    to: (f64, f64),
    style: Style,
    kind: Kind,
    /// Across the way, unit length (left of the direction of travel).
    n: (f64, f64),
    d: f64,
    /// The style's own numbers (sizes, counts, sides, phases).
    k: [f64; 8],
}

impl Path {
    pub fn new(from: (f64, f64), to: (f64, f64), style: Style, kind: Kind, rng: &mut Rng) -> Path {
        Path::feeling(from, to, style, kind, rng, Feel::default())
    }

    /// [`Path::new`], with the tremor and the overshoot as `feel` says.
    pub fn feeling(
        from: (f64, f64),
        to: (f64, f64),
        style: Style,
        kind: Kind,
        rng: &mut Rng,
        feel: Feel,
    ) -> Path {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let d = dx.hypot(dy);
        let n = if d > 1e-9 {
            (-dy / d, dx / d)
        } else {
            (0.0, 0.0)
        };
        let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
        let mut k = [0.0; 8];
        // A faint tremor for every style: its size and two frequencies and phases.
        k[4] = match kind {
            Kind::Reach => rng.range(0.3, 0.9),
            Kind::Drag => rng.range(0.15, 0.35),
        };
        k[4] *= feel.jitter;
        k[5] = rng.range(9.0, 15.0);
        k[6] = rng.range(0.0, TAU);
        k[7] = rng.range(19.0, 29.0);
        match style {
            Style::Hand => {
                // The bow, where its two pulls are, and an overshoot.
                let bow = (d * rng.range(0.04, 0.16)).min(120.0) * side;
                k[0] = bow * rng.range(0.6, 1.2);
                k[1] = bow * rng.range(0.6, 1.2);
                k[2] = if d > 250.0 && rng.chance(0.45 * feel.overshoot) {
                    (d * rng.range(0.015, 0.035)).min(16.0)
                } else {
                    0.0
                };
                k[3] = rng.range(-0.4, 0.4);
            }
            Style::Sine => {
                // Size, half-waves (2: an S; 3: one more swing), phase side.
                k[0] = (d * rng.range(0.06, 0.12)).min(45.0) * side;
                k[1] = if d > 300.0 && rng.chance(0.5) {
                    3.0
                } else {
                    2.0
                };
            }
            Style::Arc => {
                // How far the middle stands off the chord.
                k[0] = d * rng.range(0.15, 0.3) * side;
            }
            Style::Spring => {
                // Swings and damping (past the target by 6-10%), and a
                // little sideways wobble.
                k[1] = rng.range(2.2, 2.8) * PI;
                k[0] = k[1] / PI * rng.range(2.3, 2.8);
                k[2] = (d * rng.range(0.03, 0.06)).min(24.0) * side;
            }
            Style::Spiral => {
                // Turns (0.15 to 0.3 of one), one way or the other: more
                // would swing far out round the target.
                k[0] = rng.range(0.15, 0.3) * TAU * side;
            }
        }
        Path {
            from,
            to,
            style,
            kind,
            n,
            d,
            k,
        }
    }

    pub fn style(&self) -> Style {
        self.style
    }

    /// Where the path is at `t` (0 at the start, 1 at the end) of its time:
    /// `from` at 0, exactly `to` at 1.
    pub fn at(&self, t: f64) -> (f64, f64) {
        let t = t.clamp(0.0, 1.0);
        if t >= 1.0 || self.d < 2.0 {
            return if t >= 1.0 { self.to } else { self.from };
        }
        let (from, to, n, d, k) = (self.from, self.to, self.n, self.d, &self.k);
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let along = |s: f64, off: f64| (from.0 + dx * s + n.0 * off, from.1 + dy * s + n.1 * off);
        let s = ease(t);
        // The tremor, across the way: none at either end.
        let shake = k[4]
            * 4.0
            * s
            * (1.0 - s)
            * ((t * k[5] + k[6]).sin() * 0.7 + (t * k[7] + k[6]).sin() * 0.3);
        if self.kind == Kind::Drag {
            return along(s, shake);
        }
        let p = match self.style {
            Style::Hand => {
                // To a point a touch past (if it overshoots), then back.
                let past = k[2];
                let (ux, uy) = (dx / d, dy / d);
                let aim = (
                    to.0 + ux * past - uy * past * k[3],
                    to.1 + uy * past + ux * past * k[3],
                );
                let main = if past > 0.0 { 0.86 } else { 1.0 };
                if t <= main {
                    let s = ease(t / main);
                    let (ax, ay) = (aim.0 - from.0, aim.1 - from.1);
                    let p1 = (
                        from.0 + ax * 0.3 + n.0 * k[0],
                        from.1 + ay * 0.3 + n.1 * k[0],
                    );
                    let p2 = (
                        from.0 + ax * 0.7 + n.0 * k[1],
                        from.1 + ay * 0.7 + n.1 * k[1],
                    );
                    let (x, y) = bezier(from, p1, p2, aim, s);
                    (x + n.0 * shake, y + n.1 * shake)
                } else {
                    let s = ease((t - main) / (1.0 - main));
                    (aim.0 + (to.0 - aim.0) * s, aim.1 + (to.1 - aim.1) * s)
                }
            }
            Style::Sine => {
                // Calm at both ends, swinging in between.
                let off = k[0] * (PI * s).sin() * (PI * k[1] * s / 2.0).sin();
                along(s, off + shake)
            }
            Style::Arc => {
                // A circle through both ends, its middle k[0] off the chord.
                let h = k[0];
                let r = (d * d / 4.0 + h * h) / (2.0 * h.abs());
                let mid = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
                let sign = h.signum();
                // The centre, on the far side of the chord from the bulge.
                let c = (
                    mid.0 - n.0 * sign * (r - h.abs()),
                    mid.1 - n.1 * sign * (r - h.abs()),
                );
                let a0 = (from.1 - c.1).atan2(from.0 - c.0);
                let a1 = (to.1 - c.1).atan2(to.0 - c.0);
                // The short way round (the bulge is under half the chord,
                // so the arc is the smaller one, on the bulge's side).
                let sweep = (a1 - a0 + PI).rem_euclid(TAU) - PI;
                let a = a0 + sweep * s;
                (
                    c.0 + r * a.cos() + n.0 * shake,
                    c.1 + r * a.sin() + n.1 * shake,
                )
            }
            Style::Spring => {
                // An underdamped spring's step response, scaled to end at 1.
                let (z, w) = (k[0], k[1]);
                let step = |t: f64| 1.0 - (-z * t).exp() * ((w * t).cos() + z / w * (w * t).sin());
                let s = step(t) / step(1.0);
                let wobble = k[2] * (-z * t).exp() * (w * 1.5 * t).sin();
                along(s, wobble + shake)
            }
            Style::Spiral => {
                // Round the target: the radius closing in, the angle turning.
                let a0 = (from.1 - to.1).atan2(from.0 - to.0);
                let a = a0 + k[0] * s;
                let r = d * (1.0 - s);
                (
                    to.0 + r * a.cos() + n.0 * shake,
                    to.1 + r * a.sin() + n.1 * shake,
                )
            }
        };
        if p.0.is_finite() && p.1.is_finite() {
            p
        } else {
            along(s, 0.0)
        }
    }
}

/// How far from its target a reach really travels (px). An app sees the
/// pointer only over its own window, so the way across the rest of the
/// screen shows it nothing; it only takes the user's pointer from them
/// for longer. A reach goes at once to a point this far off and travels
/// the last stretch.
const LAST_STRETCH: (f64, f64) = (70.0, 140.0);

/// The points the real pointer goes through from `from` to `to`
/// (excluding `from`, ending exactly at `to`), one per [`STEP`], in the
/// style `mouse_path` asks for. A reach from farther than its last stretch
/// starts with a jump to it ([`travel_near`]).
pub fn travel(from: (f64, f64), to: (f64, f64), kind: Kind) -> Vec<(f64, f64)> {
    let mut rng = Rng::new();
    let style = Style::pick(mouse_setting(), &mut rng);
    travel_near_feel(from, to, kind, style, &mut rng, mouse_feel())
}

/// [`travel_with`], but a reach from far away goes at once to a point a
/// short stretch from `to` (back the way it came, a little to one side),
/// the first point, and travels only from there. A drag travels the whole
/// way: that is the drag.
pub fn travel_near(
    from: (f64, f64),
    to: (f64, f64),
    kind: Kind,
    style: Style,
    rng: &mut Rng,
) -> Vec<(f64, f64)> {
    // As a hand would, and as before there was a setting: the last stretch
    // doesn't overshoot.
    travel_near_feel(
        from,
        to,
        kind,
        style,
        rng,
        Feel {
            overshoot: 0.0,
            ..Feel::default()
        },
    )
}

/// [`travel_near`], with the pace, overshoot and tremor of `feel`.
pub fn travel_near_feel(
    from: (f64, f64),
    to: (f64, f64),
    kind: Kind,
    style: Style,
    rng: &mut Rng,
    feel: Feel,
) -> Vec<(f64, f64)> {
    let d = (to.0 - from.0).hypot(to.1 - from.1);
    let reach = rng.range(LAST_STRETCH.0, LAST_STRETCH.1);
    if kind == Kind::Reach && d.is_finite() && d > reach * 1.3 {
        let turn = rng.range(-0.45, 0.45);
        let (ux, uy) = ((from.0 - to.0) / d, (from.1 - to.1) / d);
        let (sin, cos) = turn.sin_cos();
        let start = (
            to.0 + (ux * cos - uy * sin) * reach,
            to.1 + (ux * sin + uy * cos) * reach,
        );
        // The overshoot belongs to the whole reach (a hand going far comes
        // in a touch past), not to the short last stretch: decided here,
        // and only when asked for (no draw at all otherwise).
        let past = (style == Style::Hand
            && feel.overshoot > 0.0
            && d > 250.0
            && rng.chance(feel.overshoot.min(1.0)))
        .then(|| {
            (
                (d * rng.range(0.015, 0.035)).min(16.0),
                rng.range(-0.4, 0.4),
            )
        });
        let mut out = vec![start];
        out.extend(travel_inner(start, to, kind, style, rng, feel, past));
        return out;
    }
    travel_feel(from, to, kind, style, rng, feel)
}

pub fn travel_with(
    from: (f64, f64),
    to: (f64, f64),
    kind: Kind,
    style: Style,
    rng: &mut Rng,
) -> Vec<(f64, f64)> {
    travel_feel(from, to, kind, style, rng, Feel::default())
}

/// [`travel_with`], with the pace, overshoot and tremor of `feel`.
pub fn travel_feel(
    from: (f64, f64),
    to: (f64, f64),
    kind: Kind,
    style: Style,
    rng: &mut Rng,
    feel: Feel,
) -> Vec<(f64, f64)> {
    travel_inner(from, to, kind, style, rng, feel, None)
}

/// [`travel_feel`], with an overshoot decided by the caller (how far past,
/// and to which side) for a hand's path.
fn travel_inner(
    from: (f64, f64),
    to: (f64, f64),
    kind: Kind,
    style: Style,
    rng: &mut Rng,
    feel: Feel,
    past: Option<(f64, f64)>,
) -> Vec<(f64, f64)> {
    let d = (to.0 - from.0).hypot(to.1 - from.1);
    if !d.is_finite() || d < 2.0 {
        return vec![to];
    }
    let ms = duration_ms(d, kind, rng);
    // A spring settles and a spiral turns: a little longer.
    let ms = match style {
        Style::Spring | Style::Spiral if kind == Kind::Reach => ms * 1.3,
        _ => ms,
    };
    // Quicker or slower than a hand (a drag keeps to its own pace).
    let ms = if kind == Kind::Reach {
        ms / feel.speed.clamp(0.5, 2.0)
    } else {
        ms
    };
    let mut path = Path::feeling(from, to, style, kind, rng, feel);
    if let (Some((far, side)), Style::Hand, Kind::Reach) = (past, style, kind) {
        path.k[2] = far;
        path.k[3] = side;
    }
    let steps = ((ms / STEP.as_secs_f64() / 1000.0).round() as usize).max(2);
    (1..=steps)
        .map(|i| path.at(i as f64 / steps as f64))
        .collect()
}

fn bezier(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), t: f64) -> (f64, f64) {
    let m = 1.0 - t;
    let (a, b, c, d) = (m * m * m, 3.0 * m * m * t, 3.0 * m * t * t, t * t * t);
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

/// Where a pointer the system won't show us (macOS events posted to an
/// app, Wayland) comes from the first time: a hand's reach away.
pub fn somewhere_near(to: (f64, f64), rng: &mut Rng) -> (f64, f64) {
    let angle = rng.range(0.0, std::f64::consts::TAU);
    let r = rng.range(140.0, 280.0);
    (to.0 + r * angle.cos(), to.1 + r * angle.sin())
}

/// The pause between wheel clicks when `n` of them are sent: a finger
/// rolls a wheel a notch every few tens of milliseconds, a long scroll
/// faster.
pub fn wheel_pause(n: u32, rng: &mut Rng) -> Duration {
    let base = (600.0 / f64::from(n.max(1))).clamp(8.0, 45.0);
    Duration::from_secs_f64(base * rng.range(0.75, 1.25) / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
        (a.0 - b.0).hypot(a.1 - b.1)
    }

    /// How far the farthest point is off the straight line.
    fn off_line(p: &[(f64, f64)], from: (f64, f64), to: (f64, f64)) -> f64 {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let l = dx.hypot(dy);
        p.iter()
            .map(|q| ((q.0 - from.0) * dy - (q.1 - from.1) * dx).abs() / l)
            .fold(0.0, f64::max)
    }

    #[test]
    fn every_style_ends_on_the_target_in_a_hands_time_and_stays_near() {
        let (from, to) = ((100.0, 700.0), (900.0, 150.0));
        for style in Style::ALL {
            for seed in 1..60 {
                let mut rng = Rng::seeded(seed);
                // As the real pointer goes: a jump near, then the last stretch.
                let p = travel_near(from, to, Kind::Reach, style, &mut rng);
                assert_eq!(*p.last().unwrap(), to, "{style:?}");
                let ms = p.len() as u128 * STEP.as_millis();
                assert!((100..=600).contains(&ms), "{style:?}: {ms} ms");
                for q in &p {
                    assert!(
                        dist(*q, to) <= dist(from, to) * 1.15 + 10.0,
                        "{style:?} {q:?}"
                    );
                }
                // No jumps after the first.
                for w in p[1..].windows(2) {
                    assert!(dist(w[0], w[1]) < 40.0, "{style:?}: {w:?}");
                }
            }
        }
    }

    #[test]
    fn each_style_has_its_own_shape() {
        let (from, to) = ((0.0, 0.0), (600.0, 0.0));
        // All but the spring (which goes along, past and back) leave the line.
        for style in Style::ALL.into_iter().filter(|s| *s != Style::Spring) {
            let p = travel_with(from, to, Kind::Reach, style, &mut Rng::seeded(11));
            let off = off_line(&p, from, to);
            assert!(off > 8.0, "{style:?} is straight ({off})");
        }
        // The sine crosses the line between its swings.
        let p = travel_with(from, to, Kind::Reach, Style::Sine, &mut Rng::seeded(3));
        let (above, below) = (p.iter().any(|q| q.1 > 5.0), p.iter().any(|q| q.1 < -5.0));
        assert!(above && below, "{p:?}");
        // The spring goes past the target, then comes back.
        let p = travel_with(from, to, Kind::Reach, Style::Spring, &mut Rng::seeded(5));
        assert!(p.iter().any(|q| q.0 > 610.0));
        // The spiral comes round: at some point it heads back the other way.
        let p = travel_with(from, to, Kind::Reach, Style::Spiral, &mut Rng::seeded(9));
        assert!(off_line(&p, from, to) > 60.0);
    }

    #[test]
    fn speed_rises_then_falls() {
        let p = travel_with(
            (0.0, 0.0),
            (800.0, 0.0),
            Kind::Reach,
            Style::Hand,
            &mut Rng::seeded(7),
        );
        let speeds: Vec<f64> = p.windows(2).map(|w| dist(w[0], w[1])).collect();
        let n = speeds.len();
        let (start, mid, end) = (speeds[0], speeds[n / 2], speeds[n - 1]);
        assert!(mid > start * 3.0 && mid > end * 3.0, "{start} {mid} {end}");
    }

    #[test]
    fn a_drag_goes_straight_whatever_the_style() {
        let (from, to) = ((0.0, 0.0), (500.0, 200.0));
        for style in Style::ALL {
            let p = travel_with(from, to, Kind::Drag, style, &mut Rng::seeded(2));
            assert!(off_line(&p, from, to) < 1.0, "{style:?}");
            assert_eq!(*p.last().unwrap(), to);
        }
    }

    #[test]
    fn mixed_picks_every_style_and_a_name_picks_one() {
        let mut rng = Rng::seeded(1);
        let seen: std::collections::HashSet<Style> =
            (0..200).map(|_| Style::pick("mixed", &mut rng)).collect();
        assert_eq!(seen.len(), 5);
        assert_eq!(Style::pick("Spiral", &mut rng), Style::Spiral);
        for s in Style::ALL {
            assert_eq!(Style::named(s.name()), Some(s));
        }
    }

    #[test]
    fn a_far_reach_jumps_near_and_travels_only_the_last_stretch() {
        let (from, to) = ((50.0, 900.0), (1500.0, 120.0));
        for style in Style::ALL {
            for seed in 1..40 {
                let p = travel_near(from, to, Kind::Reach, style, &mut Rng::seeded(seed));
                // The first point is the jump: a short stretch from the target.
                let near = dist(p[0], to);
                assert!((60.0..=150.0).contains(&near), "{style:?}: {near}");
                assert_eq!(*p.last().unwrap(), to);
                // The user's pointer is taken for a moment, not a journey.
                let ms = p.len() as u128 * STEP.as_millis();
                assert!(ms <= 450, "{style:?}: {ms} ms");
                // Everything after the jump stays near the target.
                for q in &p[1..] {
                    assert!(dist(*q, to) <= near * 1.4 + 10.0, "{style:?} {q:?}");
                }
            }
        }
        // A drag goes the whole way; a short reach has no jump.
        let d = travel_near(from, to, Kind::Drag, Style::Hand, &mut Rng::seeded(1));
        assert!(dist(d[0], from) < 30.0);
        let short = travel_near(
            (100.0, 100.0),
            (160.0, 100.0),
            Kind::Reach,
            Style::Hand,
            &mut Rng::seeded(1),
        );
        assert!(dist(short[0], (100.0, 100.0)) < 20.0);
    }

    #[test]
    fn tiny_moves_just_go_there() {
        let p = travel((10.0, 10.0), (11.0, 10.0), Kind::Reach);
        assert_eq!(p, vec![(11.0, 10.0)]);
    }

    #[test]
    fn two_paths_differ() {
        let a = travel((0.0, 0.0), (500.0, 300.0), Kind::Reach);
        let b = travel((0.0, 0.0), (500.0, 300.0), Kind::Reach);
        assert_ne!(a, b);
    }

    #[test]
    fn the_feel_changes_pace_overshoot_and_tremor_and_nothing_else() {
        let (from, to) = ((20.0, 400.0), (900.0, 120.0));
        let steps = |feel: Feel| {
            let mut rng = Rng::new();
            travel_feel(from, to, Kind::Reach, Style::Hand, &mut rng, feel).len()
        };
        // A pace of 2 takes about half as long as 1, and 0.5 about twice.
        let avg = |feel: Feel| (0..40).map(|_| steps(feel)).sum::<usize>() as f64 / 40.0;
        let normal = avg(Feel::default());
        let fast = avg(Feel {
            speed: 2.0,
            ..Feel::default()
        });
        let slow = avg(Feel {
            speed: 0.5,
            ..Feel::default()
        });
        assert!((fast / normal - 0.5).abs() < 0.08, "{fast} of {normal}");
        assert!((slow / normal - 2.0).abs() < 0.3, "{slow} of {normal}");
        // Every path still ends exactly on its target.
        for feel in [
            Feel {
                speed: 2.0,
                overshoot: 0.0,
                jitter: 0.0,
            },
            Feel {
                speed: 0.5,
                overshoot: 1.0,
                jitter: 2.0,
            },
        ] {
            for style in Style::ALL {
                let mut rng = Rng::new();
                let p = travel_feel(from, to, Kind::Reach, style, &mut rng, feel);
                assert_eq!(*p.last().unwrap(), to, "{style:?}");
            }
        }
        // No overshoot: a long hand reach never goes past the target; with
        // all of it, some do.
        let past = |feel: Feel| {
            (0..200)
                .filter(|_| {
                    let mut rng = Rng::new();
                    let p = travel_feel(from, to, Kind::Reach, Style::Hand, &mut rng, feel);
                    let (ux, uy) = ((to.0 - from.0), (to.1 - from.1));
                    let l = ux.hypot(uy);
                    p.iter()
                        .any(|q| ((q.0 - to.0) * ux + (q.1 - to.1) * uy) / l > 3.0)
                })
                .count()
        };
        assert_eq!(
            past(Feel {
                overshoot: 0.0,
                ..Feel::default()
            }),
            0
        );
        let some = past(Feel::default());
        assert!((50..=130).contains(&some), "{some} of 200 overshoot");
        let half = past(Feel {
            overshoot: 0.5,
            ..Feel::default()
        });
        assert!(half < some && half > 5, "{half} against {some}");
        // No tremor: the straight-line drag keeps to its line exactly.
        let mut rng = Rng::new();
        let steady = travel_feel(
            from,
            to,
            Kind::Drag,
            Style::Hand,
            &mut rng,
            Feel {
                jitter: 0.0,
                ..Feel::default()
            },
        );
        assert!(
            off_line(&steady, from, to) < 1e-6,
            "{}",
            off_line(&steady, from, to)
        );
        let mut rng = Rng::new();
        let shaky = travel_feel(
            from,
            to,
            Kind::Drag,
            Style::Hand,
            &mut rng,
            Feel {
                jitter: 2.0,
                ..Feel::default()
            },
        );
        assert!(off_line(&shaky, from, to) > 0.1);
    }

    #[test]
    fn the_real_mouse_overshoots_only_when_asked_and_then_on_its_last_stretch() {
        let (from, to) = ((20.0, 400.0), (900.0, 120.0));
        let past = |feel: Feel| {
            (0..300)
                .filter(|_| {
                    let mut rng = Rng::new();
                    let p = travel_near_feel(from, to, Kind::Reach, Style::Hand, &mut rng, feel);
                    // A reach from far jumps to its last stretch first.
                    assert!(dist(p[0], to) <= LAST_STRETCH.1 + 1.0, "{:?}", p[0]);
                    assert_eq!(*p.last().unwrap(), to);
                    let (ux, uy) = (to.0 - from.0, to.1 - from.1);
                    let l = ux.hypot(uy);
                    p.iter()
                        .any(|q| ((q.0 - to.0) * ux + (q.1 - to.1) * uy) / l > 3.0)
                })
                .count()
        };
        assert_eq!(
            past(Feel {
                overshoot: 0.0,
                ..Feel::default()
            }),
            0
        );
        let all = past(Feel {
            overshoot: 1.0,
            ..Feel::default()
        });
        assert!(all > 250, "{all} of 300");
        let some = past(Feel {
            overshoot: 0.3,
            ..Feel::default()
        });
        assert!((40..=140).contains(&some), "{some} of 300");
        // Short reaches (no jump) and other styles never do it this way.
        let mut rng = Rng::new();
        let near = travel_near_feel(
            (800.0, 150.0),
            to,
            Kind::Reach,
            Style::Hand,
            &mut rng,
            Feel {
                overshoot: 1.0,
                ..Feel::default()
            },
        );
        assert_eq!(*near.last().unwrap(), to);
    }

    #[test]
    fn the_feel_comes_from_the_settings() {
        let mut c = crate::config::Config::default();
        // By default the real mouse doesn't overshoot (as before the
        // setting): only its pace and tremor are a hand's.
        assert_eq!(
            Feel::from_config(&c),
            Feel {
                overshoot: 0.0,
                ..Feel::default()
            }
        );
        c.mouse_speed = 1.5;
        c.mouse_overshoot = 40;
        c.mouse_jitter = 150;
        assert_eq!(
            Feel::from_config(&c),
            Feel {
                speed: 1.5,
                overshoot: 0.4,
                jitter: 1.5
            }
        );
        c.mouse_speed = 9.0;
        assert_eq!(Feel::from_config(&c).speed, 2.0);
    }
}
