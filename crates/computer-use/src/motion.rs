//! Natural mouse paths. Some apps (and anti-bot checks) notice a pointer
//! that jumps, or that goes in a perfectly straight line at an even speed.
//! A hand moves along a gentle curve instead: it speeds up and slows down
//! again, trembles a little, and on a long reach may go a touch past the
//! target and come back. [`travel`] gives such a path, one point per
//! [`STEP`], for the backends to send as pointer motion (`natural_mouse`).

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
    /// Reaching for a point (button up): a free curve, maybe overshooting.
    Reach,
    /// Dragging with a button held: steadier, a flatter curve, no overshoot.
    Drag,
}

/// The points from `from` to `to` (excluding `from`, ending exactly at
/// `to`), one per [`STEP`].
pub fn travel(from: (f64, f64), to: (f64, f64), kind: Kind) -> Vec<(f64, f64)> {
    travel_with(from, to, kind, &mut Rng::new())
}

pub fn travel_with(from: (f64, f64), to: (f64, f64), kind: Kind, rng: &mut Rng) -> Vec<(f64, f64)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let d = dx.hypot(dy);
    if !d.is_finite() || d < 2.0 {
        return vec![to];
    }
    // Longer reaches take longer, but not in proportion (Fitts's law).
    let ms = (80.0 + 85.0 * (1.0 + d / 12.0).log2()) * rng.range(0.85, 1.15);
    let ms = match kind {
        Kind::Reach => ms.clamp(120.0, 800.0),
        Kind::Drag => (ms * 1.25).clamp(180.0, 1000.0),
    };
    // A long reach sometimes goes a little past and comes back.
    let overshoot = kind == Kind::Reach && d > 250.0 && rng.chance(0.45);
    let aim = if overshoot {
        let past = (d * rng.range(0.015, 0.035)).min(16.0);
        let side = rng.range(-0.4, 0.4) * past;
        let (ux, uy) = (dx / d, dy / d);
        (to.0 + ux * past - uy * side, to.1 + uy * past + ux * side)
    } else {
        to
    };
    let correction = if overshoot {
        rng.range(70.0, 120.0)
    } else {
        0.0
    };
    let main_ms = ms - correction * 0.5;

    let mut out = curve(from, aim, main_ms, kind, rng);
    if overshoot {
        out.extend(curve(aim, to, correction, Kind::Drag, rng));
    }
    if let Some(last) = out.last_mut() {
        *last = to;
    }
    out
}

/// One eased, bowed stroke from `a` to `b` taking about `ms`.
fn curve(a: (f64, f64), b: (f64, f64), ms: f64, kind: Kind, rng: &mut Rng) -> Vec<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let d = dx.hypot(dy).max(1e-9);
    let (nx, ny) = (-dy / d, dx / d);
    // The bow: to one side, a few per cent of the distance.
    let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
    let bow = match kind {
        Kind::Reach => (d * rng.range(0.04, 0.16)).min(120.0),
        Kind::Drag => (d * rng.range(0.01, 0.05)).min(40.0),
    } * side;
    let (a1, a2) = (rng.range(0.2, 0.4), rng.range(0.6, 0.8));
    let (o1, o2) = (bow * rng.range(0.6, 1.2), bow * rng.range(0.6, 1.2));
    let p1 = (a.0 + dx * a1 + nx * o1, a.1 + dy * a1 + ny * o1);
    let p2 = (a.0 + dx * a2 + nx * o2, a.1 + dy * a2 + ny * o2);
    // A little tremor across the path, gone by the end.
    let shake = match kind {
        Kind::Reach => rng.range(0.3, 0.9),
        Kind::Drag => rng.range(0.2, 0.5),
    };
    let (f1, f2) = (rng.range(9.0, 15.0), rng.range(19.0, 29.0));
    let (ph1, ph2) = (rng.range(0.0, 6.3), rng.range(0.0, 6.3));

    let steps = ((ms / STEP.as_secs_f64() / 1000.0).round() as usize).max(2);
    (1..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            // Quick to start, slower to settle: minimum jerk on a skewed clock.
            let u = t.powf(0.9);
            let s = u * u * u * (10.0 - 15.0 * u + 6.0 * u * u);
            let (x, y) = bezier(a, p1, p2, b, s);
            let wobble =
                shake * (1.0 - s) * ((t * f1 + ph1).sin() * 0.7 + (t * f2 + ph2).sin() * 0.3);
            (x + nx * wobble, y + ny * wobble)
        })
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

    #[test]
    fn a_path_ends_on_the_target_and_takes_human_time() {
        for seed in 1..200 {
            let mut rng = Rng::seeded(seed);
            let (from, to) = ((100.0, 700.0), (900.0, 150.0));
            let p = travel_with(from, to, Kind::Reach, &mut rng);
            assert_eq!(*p.last().unwrap(), to);
            let ms = p.len() as u128 * STEP.as_millis();
            assert!((300..=900).contains(&ms), "{ms} ms");
            // Never wildly off the way.
            let bound = dist(from, to) * 0.25 + 30.0;
            for q in &p {
                assert!(dist(*q, from) + dist(*q, to) < dist(from, to) + 2.0 * bound);
            }
        }
    }

    #[test]
    fn a_path_is_curved_not_straight() {
        let (from, to) = ((0.0, 0.0), (600.0, 0.0));
        let mut bowed = 0;
        for seed in 1..100 {
            let p = travel_with(from, to, Kind::Reach, &mut Rng::seeded(seed));
            let off = p.iter().map(|q| q.1.abs()).fold(0.0, f64::max);
            if off > 8.0 {
                bowed += 1;
            }
        }
        assert!(bowed > 90, "only {bowed} of 99 paths curve");
    }

    #[test]
    fn speed_rises_then_falls() {
        let p = travel_with((0.0, 0.0), (800.0, 0.0), Kind::Drag, &mut Rng::seeded(7));
        let speeds: Vec<f64> = p.windows(2).map(|w| dist(w[0], w[1])).collect();
        let n = speeds.len();
        let (start, mid, end) = (speeds[0], speeds[n / 2], speeds[n - 1]);
        assert!(mid > start * 3.0 && mid > end * 3.0, "{start} {mid} {end}");
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
}
