//! 3D models planned as solids before anything is built in an app: boxes,
//! cylinders, spheres, cones, tori and planes with exact sizes, centres and
//! turns, in metres (or any one unit), Z up with the ground at z 0. A scene
//! is seen from the front, the right, above and in perspective on one
//! picture, to one scale with a grid; it is checked for parts that float,
//! sink or run into each other; and it is written out as the numbers to
//! build it in a 3D app, or as an OBJ file to import.
//!
//! Written by mhrsdev — https://github.com/mhrsdev/zero-use-computer

use std::f64::consts::TAU;

use crate::design::{Rgb, hex, parse_colour};
use crate::imaging;
use crate::tools::{SceneArgs, SceneObject, SceneShape, SceneView};
use crate::types::{Capture, Rect};

type V3 = [f64; 3];
type M3 = [[f64; 3]; 3];

/// At most this many objects in a scene.
const MAX_OBJECTS: usize = 300;
/// Segments around a round solid.
const ROUND: usize = 32;
/// Rendered this many times bigger, then averaged, for smooth edges.
const SS: usize = 2;

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn times(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { times(a, 1.0 / l) } else { a }
}

fn mul(m: M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// The transpose times `v`: back from the scene's axes to an object's own.
fn mul_t(m: M3, v: V3) -> V3 {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2],
    ]
}

fn matmul(a: M3, b: M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// Turns in degrees about x, then y, then z (Blender's XYZ order):
/// Rz · Ry · Rx.
fn rotation(t: V3) -> M3 {
    let (sa, ca) = t[0].to_radians().sin_cos();
    let (sb, cb) = t[1].to_radians().sin_cos();
    let (sc, cc) = t[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, ca, -sa], [0.0, sa, ca]];
    let ry = [[cb, 0.0, sb], [0.0, 1.0, 0.0], [-sb, 0.0, cb]];
    let rz = [[cc, -sc, 0.0], [sc, cc, 0.0], [0.0, 0.0, 1.0]];
    matmul(rz, matmul(ry, rx))
}

/// Back from a rotation to XYZ turns in degrees.
fn euler(m: M3) -> V3 {
    let sy = (-m[2][0]).clamp(-1.0, 1.0);
    let b = sy.asin();
    let (a, c) = if b.cos() > 1e-9 {
        (m[2][1].atan2(m[2][2]), m[1][0].atan2(m[0][0]))
    } else {
        ((-m[1][2]).atan2(m[1][1]), 0.0)
    };
    [a, b, c].map(|r| tidy_angle(r.to_degrees()))
}

/// An angle in (-180, 180], without rounding noise.
fn tidy_angle(d: f64) -> f64 {
    let mut d = (d * 1e6).round() / 1e6;
    while d > 180.0 {
        d -= 360.0;
    }
    while d <= -180.0 {
        d += 360.0;
    }
    if d == 0.0 { 0.0 } else { d }
}

/// A number as short as it can be: "0.45", "2", "-1.25".
pub fn num(v: f64) -> String {
    let s = format!("{:.4}", (v * 1e4).round() / 1e4);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn triple(v: V3) -> String {
    format!("{}, {}, {}", num(v[0]), num(v[1]), num(v[2]))
}

pub fn shape_name(s: SceneShape) -> &'static str {
    match s {
        SceneShape::Box => "box",
        SceneShape::Cylinder => "cylinder",
        SceneShape::Sphere => "sphere",
        SceneShape::Cone => "cone",
        SceneShape::Torus => "torus",
        SceneShape::Plane => "plane",
    }
}

/// The mesh Blender adds for it (Add > Mesh).
pub fn blender_mesh(s: SceneShape) -> &'static str {
    match s {
        SceneShape::Box => "Cube",
        SceneShape::Cylinder => "Cylinder",
        SceneShape::Sphere => "UV Sphere",
        SceneShape::Cone => "Cone",
        SceneShape::Torus => "Torus",
        SceneShape::Plane => "Plane",
    }
}

/// A size as given ([diameter, height] and so on) on all three axes.
fn full_size(shape: SceneShape, v: &[f64]) -> Result<V3, String> {
    if v.iter().any(|x| !x.is_finite() || *x < 0.0) {
        return Err("sizes must be numbers of 0 or more".into());
    }
    let s = match (shape, v) {
        (_, [a, b, c]) => [*a, *b, *c],
        (SceneShape::Box | SceneShape::Sphere, [a]) => [*a, *a, *a],
        (SceneShape::Plane, [a]) => [*a, *a, 0.0],
        (SceneShape::Plane, [a, b]) => [*a, *b, 0.0],
        (SceneShape::Cylinder | SceneShape::Cone | SceneShape::Torus, [d, h]) => [*d, *d, *h],
        _ => {
            return Err(format!(
                "a {}'s size is {}",
                shape_name(shape),
                match shape {
                    SceneShape::Box => "[x, y, z] (or one number for a cube)",
                    SceneShape::Cylinder | SceneShape::Cone =>
                        "[diameter, height] or [x, y, height]",
                    SceneShape::Sphere => "[diameter] or [x, y, z]",
                    SceneShape::Torus => "[outer diameter, thickness]",
                    SceneShape::Plane => "[x, y]",
                }
            ));
        }
    };
    checked_size(shape, s)
}

fn checked_size(shape: SceneShape, mut s: V3) -> Result<V3, String> {
    if shape == SceneShape::Plane {
        s[2] = 0.0;
    }
    let needed = if shape == SceneShape::Plane { 2 } else { 3 };
    if s[..needed].iter().any(|v| *v <= 0.0) {
        return Err(format!(
            "a {} needs a size above 0 on every axis",
            shape_name(shape)
        ));
    }
    if shape == SceneShape::Torus && s[2] >= s[0].min(s[1]) {
        return Err("a torus's thickness must be less than its outer diameter".into());
    }
    Ok(s)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub id: String,
    pub shape: SceneShape,
    /// Its extent along its own x, y and z before turning (a torus: its
    /// outer diameters and the ring's thickness; a plane: z 0).
    pub size: V3,
    /// Its centre.
    pub at: V3,
    /// Degrees about x, then y, then z.
    pub rotate: V3,
    pub color: Rgb,
}

impl Object {
    fn half(&self) -> V3 {
        times(self.size, 0.5)
    }

    fn rot(&self) -> M3 {
        rotation(self.rotate)
    }

    /// How far it reaches from its centre along `d` (its own axes).
    fn reach(&self, d: V3) -> f64 {
        let h = self.half();
        let round = |a: f64, b: f64| a.hypot(b);
        match self.shape {
            SceneShape::Box => h[0] * d[0].abs() + h[1] * d[1].abs() + h[2] * d[2].abs(),
            SceneShape::Plane => h[0] * d[0].abs() + h[1] * d[1].abs(),
            SceneShape::Sphere => round(round(h[0] * d[0], h[1] * d[1]), h[2] * d[2]),
            SceneShape::Cylinder => h[2] * d[2].abs() + round(h[0] * d[0], h[1] * d[1]),
            SceneShape::Cone => (h[2] * d[2]).max(-h[2] * d[2] + round(h[0] * d[0], h[1] * d[1])),
            SceneShape::Torus => {
                let r = h[2];
                round((h[0] - r) * d[0], (h[1] - r) * d[1]) + r * dot(d, d).sqrt()
            }
        }
    }

    /// Its lowest and highest x, y and z in the scene.
    pub fn bounds(&self) -> (V3, V3) {
        let r = self.rot();
        let (mut lo, mut hi) = ([0.0; 3], [0.0; 3]);
        for i in 0..3 {
            let mut axis = [0.0; 3];
            axis[i] = 1.0;
            let own = mul_t(r, axis);
            hi[i] = self.at[i] + self.reach(own);
            lo[i] = self.at[i] - self.reach(times(own, -1.0));
        }
        (lo, hi)
    }

    /// Whether a point (its own axes, from its centre) is in it, grown by
    /// `g` on every side (shrunk when `g` is below 0).
    fn inside_local(&self, q: V3, g: f64) -> bool {
        let h = self.half();
        let ellipse = |x: f64, a: f64, y: f64, b: f64| {
            a > 0.0 && b > 0.0 && (x / a).powi(2) + (y / b).powi(2) <= 1.0
        };
        match self.shape {
            SceneShape::Box => (0..3).all(|i| q[i].abs() <= h[i] + g),
            SceneShape::Plane => {
                q[0].abs() <= h[0] + g && q[1].abs() <= h[1] + g && q[2].abs() <= g
            }
            SceneShape::Sphere => {
                let r = [h[0] + g, h[1] + g, h[2] + g];
                r.iter().all(|v| *v > 0.0)
                    && (0..3).map(|i| (q[i] / r[i]).powi(2)).sum::<f64>() <= 1.0
            }
            SceneShape::Cylinder => {
                q[2].abs() <= h[2] + g && ellipse(q[0], h[0] + g, q[1], h[1] + g)
            }
            SceneShape::Cone => {
                if q[2].abs() > h[2] + g {
                    return false;
                }
                let f = ((h[2] - q[2]) / (2.0 * h[2])).clamp(0.0, 1.0);
                ellipse(q[0], h[0] * f + g, q[1], h[1] * f + g)
            }
            SceneShape::Torus => {
                let r = h[2];
                let (ra, rb) = (h[0] - r, h[1] - r);
                let y = q[1] * ra / rb;
                (q[0].hypot(y) - ra).hypot(q[2]) <= r + g
            }
        }
    }

    /// Its surface as triangles in its own axes, each facing outwards.
    fn mesh(&self) -> Vec<[V3; 3]> {
        let h = self.half();
        let n = ROUND;
        let ring = |k: usize| (k as f64 * TAU / n as f64).sin_cos();
        let mut t: Vec<[V3; 3]> = Vec::new();
        let mut quad = |a: V3, b: V3, c: V3, d: V3| {
            t.push([a, b, c]);
            t.push([a, c, d]);
        };
        match self.shape {
            SceneShape::Box => {
                let c = |x: f64, y: f64, z: f64| [x * h[0], y * h[1], z * h[2]];
                for s in [-1.0, 1.0] {
                    quad(
                        c(s, -1.0, -1.0),
                        c(s, 1.0, -1.0),
                        c(s, 1.0, 1.0),
                        c(s, -1.0, 1.0),
                    );
                    quad(
                        c(-1.0, s, -1.0),
                        c(1.0, s, -1.0),
                        c(1.0, s, 1.0),
                        c(-1.0, s, 1.0),
                    );
                    quad(
                        c(-1.0, -1.0, s),
                        c(1.0, -1.0, s),
                        c(1.0, 1.0, s),
                        c(-1.0, 1.0, s),
                    );
                }
            }
            SceneShape::Plane => quad(
                [-h[0], -h[1], 0.0],
                [h[0], -h[1], 0.0],
                [h[0], h[1], 0.0],
                [-h[0], h[1], 0.0],
            ),
            SceneShape::Cylinder | SceneShape::Cone => {
                let top = if self.shape == SceneShape::Cone {
                    0.0
                } else {
                    1.0
                };
                for k in 0..n {
                    let ((s0, c0), (s1, c1)) = (ring(k), ring(k + 1));
                    let b0 = [h[0] * c0, h[1] * s0, -h[2]];
                    let b1 = [h[0] * c1, h[1] * s1, -h[2]];
                    let t0 = [h[0] * c0 * top, h[1] * s0 * top, h[2]];
                    let t1 = [h[0] * c1 * top, h[1] * s1 * top, h[2]];
                    quad(b0, b1, t1, t0);
                    quad([0.0, 0.0, -h[2]], b0, b1, b1);
                    if top > 0.0 {
                        quad([0.0, 0.0, h[2]], t0, t1, t1);
                    }
                }
            }
            SceneShape::Sphere | SceneShape::Torus => {
                let m = n / 2;
                let (r, ra, rb) = (h[2], h[0] - h[2], h[1] - h[2]);
                let p = |k: usize, j: usize| -> V3 {
                    let (sa, ca) = ring(k);
                    if self.shape == SceneShape::Sphere {
                        let (sp, cp) =
                            (std::f64::consts::PI * (j as f64 / m as f64 - 0.5)).sin_cos();
                        [h[0] * cp * ca, h[1] * cp * sa, h[2] * sp]
                    } else {
                        let (sb, cb) = (TAU * j as f64 / m as f64).sin_cos();
                        [(ra + r * cb) * ca, (rb + r * cb) * sa, r * sb]
                    }
                };
                for k in 0..n {
                    for j in 0..m {
                        quad(p(k, j), p(k + 1, j), p(k + 1, j + 1), p(k, j + 1));
                    }
                }
            }
        }
        t.into_iter()
            .filter(|tri| dot(normal(tri), normal(tri)) > 1e-24)
            .map(|tri| self.outward(tri))
            .collect()
    }

    fn outward(&self, tri: [V3; 3]) -> [V3; 3] {
        let c = times(add(add(tri[0], tri[1]), tri[2]), 1.0 / 3.0);
        let core = match self.shape {
            SceneShape::Plane => return tri,
            SceneShape::Torus => {
                let h = self.half();
                let (ra, rb) = (h[0] - h[2], h[1] - h[2]);
                let a = (c[1] / rb).atan2(c[0] / ra);
                [ra * a.cos(), rb * a.sin(), 0.0]
            }
            _ => [0.0; 3],
        };
        if dot(normal(&tri), sub(c, core)) < 0.0 {
            [tri[0], tri[2], tri[1]]
        } else {
            tri
        }
    }

    /// Its surface in the scene.
    fn triangles(&self) -> Vec<[V3; 3]> {
        let r = self.rot();
        self.mesh()
            .into_iter()
            .map(|t| t.map(|p| add(mul(r, p), self.at)))
            .collect()
    }
}

fn normal(t: &[V3; 3]) -> V3 {
    cross(sub(t[1], t[0]), sub(t[2], t[0]))
}

/// An object placed for checking: its turn, extent and some points on its
/// surface.
struct Placed<'a> {
    o: &'a Object,
    r: M3,
    lo: V3,
    hi: V3,
    /// Made only for parts that come near another.
    points: std::cell::OnceCell<Vec<V3>>,
}

impl Placed<'_> {
    fn inside(&self, p: V3, g: f64) -> bool {
        self.o.inside_local(mul_t(self.r, sub(p, self.o.at)), g)
    }

    /// Points on its surface: each triangle's corners, the middles of two
    /// sides and of the triangle.
    fn points(&self) -> &[V3] {
        self.points.get_or_init(|| {
            let mut points = Vec::new();
            for t in self.o.triangles() {
                for (a, b) in [
                    (0.0, 0.0),
                    (1.0, 0.0),
                    (0.0, 1.0),
                    (0.5, 0.5),
                    (0.5, 0.0),
                    (0.0, 0.5),
                    (1.0 / 3.0, 1.0 / 3.0),
                ] {
                    points.push(add(
                        t[0],
                        add(times(sub(t[1], t[0]), a), times(sub(t[2], t[0]), b)),
                    ));
                }
            }
            points
        })
    }
}

/// Whether two boxes (lowest and highest corners) meet, grown by `g`.
fn boxes_meet(a: (V3, V3), b: (V3, V3), g: f64) -> bool {
    (0..3).all(|i| a.0[i] - g <= b.1[i] && b.0[i] - g <= a.1[i])
}

/// Points spread through the box from `lo` to `hi`, `n` per axis.
fn grid_points(lo: V3, hi: V3, n: usize) -> Vec<V3> {
    let step = [0, 1, 2].map(|i| (hi[i] - lo[i]) / (n - 1) as f64);
    let mut v = Vec::with_capacity(n * n * n);
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                v.push([
                    lo[0] + step[0] * i as f64,
                    lo[1] + step[1] * j as f64,
                    lo[2] + step[2] * k as f64,
                ]);
            }
        }
    }
    v
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub objects: Vec<Object>,
    /// The ground at z 0 is there: what floats or sinks is reported.
    pub ground: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            objects: Vec::new(),
            ground: true,
        }
    }
}

impl Scene {
    fn find(&self, id: &str) -> Result<usize, String> {
        let id = id.trim();
        self.objects.iter().position(|o| o.id == id).ok_or_else(|| {
            let ids: Vec<&str> = self.objects.iter().map(|o| o.id.as_str()).collect();
            format!(
                "no object \"{id}\" (there are: {})",
                if ids.is_empty() {
                    "none".into()
                } else {
                    ids.join(", ")
                }
            )
        })
    }

    fn free(&self, id: &str) -> Result<String, String> {
        let id = id.trim();
        if id.is_empty() {
            return Err("an object's id can't be empty".into());
        }
        if self.objects.iter().any(|o| o.id == id) {
            return Err(format!("there is already an object \"{id}\""));
        }
        Ok(id.to_string())
    }

    /// Apply removals, additions, changes, mirrors and repeats, in that
    /// order. On an error the scene may be half changed: apply to a copy.
    pub fn apply(&mut self, a: &SceneArgs) -> Result<(), String> {
        if let Some(g) = a.ground {
            self.ground = g;
        }
        for id in a.remove.iter().flatten() {
            let i = self.find(id)?;
            self.objects.remove(i);
        }
        for spec in a.add.iter().flatten() {
            let shape = spec
                .shape
                .ok_or("add needs shape: box, cylinder, sphere, cone, torus or plane")?;
            let id = match &spec.id {
                Some(id) => self.free(id)?,
                None => (1..)
                    .map(|n| format!("{}-{n}", shape_name(shape)))
                    .find(|id| self.objects.iter().all(|o| o.id != *id))
                    .unwrap_or_default(),
            };
            let size = spec
                .size
                .as_deref()
                .ok_or_else(|| format!("add {id}: give its size"))?;
            let mut o = Object {
                id,
                shape,
                size: full_size(shape, size)?,
                at: [0.0; 3],
                rotate: [0.0; 3],
                color: [170, 170, 170],
            };
            // Without a place, it stands on the ground at the middle.
            let mut spec = spec.clone();
            if spec.at.is_none() && spec.on.is_none() && self.ground {
                spec.on = Some("ground".into());
            }
            self.set(&mut o, &spec)?;
            self.objects.push(o);
            if self.objects.len() > MAX_OBJECTS {
                return Err(format!("a scene holds at most {MAX_OBJECTS} objects"));
            }
        }
        for spec in a.change.iter().flatten() {
            let id = spec.id.as_deref().ok_or("change needs each object's id")?;
            let i = self.find(id)?;
            let mut o = self.objects[i].clone();
            if let Some(shape) = spec.shape {
                o.shape = shape;
                if spec.size.is_none() {
                    o.size = checked_size(shape, o.size)
                        .map_err(|e| format!("{}: {e}; give its size too", o.id))?;
                }
            }
            if let Some(size) = &spec.size {
                o.size = full_size(o.shape, size).map_err(|e| format!("{}: {e}", o.id))?;
            }
            self.set(&mut o, spec)?;
            self.objects[i] = o;
        }
        for m in a.mirror.iter().flatten() {
            let i = self.find(&m.id)?;
            let copy = self.free(&m.copy)?;
            let axis = match m.axis.as_deref().map(str::trim) {
                None | Some("x" | "X") => 0,
                Some("y" | "Y") => 1,
                Some("z" | "Z") => 2,
                Some(other) => {
                    return Err(format!(
                        "mirror axis \"{other}\": give \"x\", \"y\" or \"z\""
                    ));
                }
            };
            let at = m.at.unwrap_or(0.0);
            if !at.is_finite() {
                return Err("mirror at must be a number".into());
            }
            let mut o = self.objects[i].clone();
            o.id = copy;
            o.at[axis] = 2.0 * at - o.at[axis];
            let mut flip = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            flip[axis][axis] = -1.0;
            let mut r = matmul(flip, matmul(o.rot(), flip));
            // A cone flipped top to bottom points the other way.
            if axis == 2 && o.shape == SceneShape::Cone {
                r = matmul(r, rotation([180.0, 0.0, 0.0]));
            }
            o.rotate = euler(r);
            self.objects.push(o);
        }
        for rep in a.repeat.iter().flatten() {
            let i = self.find(&rep.id)?;
            if !(2..=100).contains(&rep.count) {
                return Err("repeat count is how many in all, 2 to 100".into());
            }
            let base = self.objects[i].clone();
            for k in 1..rep.count {
                let mut o = base.clone();
                o.id = self.free(&format!("{}-{}", base.id, k + 1))?;
                let kf = f64::from(k);
                match (rep.offset, rep.around) {
                    (Some(d), None) if d.iter().all(|v| v.is_finite()) => {
                        o.at = add(o.at, times(d, kf));
                    }
                    (None, Some([cx, cy])) if cx.is_finite() && cy.is_finite() => {
                        let total = rep.angle.unwrap_or(360.0);
                        if !total.is_finite() {
                            return Err("repeat angle must be a number".into());
                        }
                        let step = if (total.abs() - 360.0).abs() < 1e-9 {
                            total / f64::from(rep.count)
                        } else {
                            total / f64::from(rep.count - 1)
                        };
                        let (s, c) = (step * kf).to_radians().sin_cos();
                        let (dx, dy) = (o.at[0] - cx, o.at[1] - cy);
                        o.at[0] = cx + dx * c - dy * s;
                        o.at[1] = cy + dx * s + dy * c;
                        o.rotate[2] = tidy_angle(o.rotate[2] + step * kf);
                    }
                    _ => {
                        return Err(
                            "repeat needs offset [dx, dy, dz] or around [x, y] (one of them)"
                                .into(),
                        );
                    }
                }
                self.objects.push(o);
            }
            if self.objects.len() > MAX_OBJECTS {
                return Err(format!("a scene holds at most {MAX_OBJECTS} objects"));
            }
        }
        Ok(())
    }

    /// The parts of a spec beyond its shape and size.
    fn set(&self, o: &mut Object, spec: &SceneObject) -> Result<(), String> {
        let finite = |v: &[f64]| v.iter().all(|x| x.is_finite());
        if let Some(at) = spec.at {
            if !finite(&at) {
                return Err(format!("{}: at must be numbers", o.id));
            }
            o.at = at;
        }
        if let Some(r) = spec.rotate {
            if !finite(&r) {
                return Err(format!("{}: rotate must be numbers", o.id));
            }
            o.rotate = r.map(tidy_angle);
        }
        if let Some(d) = spec.shift {
            if !finite(&d) {
                return Err(format!("{}: move must be numbers", o.id));
            }
            o.at = add(o.at, d);
        }
        if let Some(c) = &spec.color {
            o.color =
                parse_colour(c)?.ok_or_else(|| format!("{}: give a colour, not none", o.id))?;
        }
        if let Some(on) = &spec.on {
            let top = if on.trim().eq_ignore_ascii_case("ground") {
                0.0
            } else {
                let i = self.find(on)?;
                if self.objects[i].id == o.id {
                    return Err(format!("{} can't rest on itself", o.id));
                }
                self.objects[i].bounds().1[2]
            };
            o.at[2] += top - o.bounds().0[2];
        }
        Ok(())
    }

    /// The lowest and highest corner of everything.
    pub fn bounds(&self) -> Option<(V3, V3)> {
        let mut it = self.objects.iter().map(Object::bounds);
        let first = it.next()?;
        Some(it.fold(first, |(lo, hi), (l, h)| {
            (
                [0, 1, 2].map(|i| lo[i].min(l[i])),
                [0, 1, 2].map(|i| hi[i].max(h[i])),
            )
        }))
    }

    /// "seat box 0.5 x 0.5 x 0.05 at (0, 0, 0.45), z 0.425 to 0.475,
    /// #8B5A2B; …"
    pub fn listing(&self) -> String {
        let parts: Vec<String> = self
            .objects
            .iter()
            .map(|o| {
                let (lo, hi) = o.bounds();
                let size = match o.shape {
                    SceneShape::Plane => format!("{} x {}", num(o.size[0]), num(o.size[1])),
                    _ => format!(
                        "{} x {} x {}",
                        num(o.size[0]),
                        num(o.size[1]),
                        num(o.size[2])
                    ),
                };
                let turned = if o.rotate.iter().any(|v| *v != 0.0) {
                    format!(" rotated ({})", triple(o.rotate))
                } else {
                    String::new()
                };
                format!(
                    "{} {} {size} at ({}){turned}, z {} to {}, {}",
                    o.id,
                    shape_name(o.shape),
                    triple(o.at),
                    num(lo[2]),
                    num(hi[2]),
                    hex(o.color)
                )
            })
            .collect();
        parts.join("; ")
    }

    /// What looks wrong: parts that float, sink or run into each other.
    pub fn checks(&self) -> Vec<String> {
        let Some((lo, hi)) = self.bounds() else {
            return Vec::new();
        };
        let extent = (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max);
        // Closer than this counts as touching.
        let tol = (extent * 1e-3).max(1e-9);
        let placed: Vec<Placed> = self
            .objects
            .iter()
            .map(|o| {
                let (lo, hi) = o.bounds();
                Placed {
                    o,
                    r: o.rot(),
                    lo,
                    hi,
                    points: std::cell::OnceCell::new(),
                }
            })
            .collect();
        let n = placed.len();
        let mut out = Vec::new();

        // Which parts touch, and which run into each other (how deep).
        let mut touch = vec![Vec::new(); n];
        let mut overlaps: Vec<(usize, usize, f64)> = Vec::new();
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (&placed[i], &placed[j]);
                if !boxes_meet((a.lo, a.hi), (b.lo, b.hi), tol * 2.0) {
                    continue;
                }
                let lo = [0, 1, 2].map(|k| a.lo[k].max(b.lo[k]) - tol * 2.0);
                let hi = [0, 1, 2].map(|k| a.hi[k].min(b.hi[k]) + tol * 2.0);
                let grid = grid_points(lo, hi, 13);
                let both = |p: &V3, g: f64| a.inside(*p, g) && b.inside(*p, g);
                let touching = grid.iter().any(|p| both(p, tol * 2.0))
                    || a.points().iter().any(|p| b.inside(*p, tol * 2.0))
                    || b.points().iter().any(|p| a.inside(*p, tol * 2.0));
                if !touching {
                    continue;
                }
                touch[i].push(j);
                touch[j].push(i);
                // How far one goes into the other: the thinnest side of
                // where both are, its ends found exactly from the points
                // furthest out.
                let deep: Vec<V3> = grid.into_iter().filter(|p| both(p, -tol)).collect();
                if deep.is_empty() {
                    continue;
                }
                let depth = (0..3)
                    .map(|k| {
                        let end = |dir: f64| {
                            let far = deep
                                .iter()
                                .max_by(|p, q| (dir * p[k]).total_cmp(&(dir * q[k])))
                                .copied()
                                .unwrap_or(deep[0]);
                            let (mut inn, mut out) = (far, far);
                            out[k] = if dir > 0.0 { hi[k] } else { lo[k] };
                            for _ in 0..40 {
                                let mut m = inn;
                                m[k] = (inn[k] + out[k]) / 2.0;
                                if both(&m, 0.0) {
                                    inn = m;
                                } else {
                                    out = m;
                                }
                            }
                            inn[k]
                        };
                        end(1.0) - end(-1.0)
                    })
                    .fold(f64::MAX, f64::min);
                if depth > tol * 2.0 {
                    overlaps.push((i, j, depth));
                }
            }
        }

        if self.ground {
            for p in &placed {
                if p.lo[2] < -tol {
                    out.push(format!(
                        "{} sinks {} below the ground (its bottom is at z {}): move it up {} unless it is meant to",
                        p.o.id,
                        num(-p.lo[2]),
                        num(p.lo[2]),
                        num(-p.lo[2])
                    ));
                }
            }
        }

        // Groups of parts joined together; a group off the ground floats.
        let mut group = vec![usize::MAX; n];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for s in 0..n {
            if group[s] != usize::MAX {
                continue;
            }
            let g = groups.len();
            let mut stack = vec![s];
            group[s] = g;
            let mut members = Vec::new();
            while let Some(i) = stack.pop() {
                members.push(i);
                for &j in &touch[i] {
                    if group[j] == usize::MAX {
                        group[j] = g;
                        stack.push(j);
                    }
                }
            }
            members.sort_unstable();
            groups.push(members);
        }
        let names = |m: &[usize]| {
            let ids: Vec<&str> = m.iter().take(6).map(|&i| placed[i].o.id.as_str()).collect();
            let more = if m.len() > 6 {
                format!(" and {} more", m.len() - 6)
            } else {
                String::new()
            };
            format!("{}{more}", ids.join(", "))
        };
        if self.ground {
            for m in &groups {
                if m.iter().any(|&i| placed[i].lo[2] <= tol) {
                    continue;
                }
                // The lowest part, and what is under it.
                let &low = m
                    .iter()
                    .min_by(|&&a, &&b| placed[a].lo[2].total_cmp(&placed[b].lo[2]))
                    .unwrap_or(&m[0]);
                let p = &placed[low];
                let under = placed
                    .iter()
                    .enumerate()
                    .filter(|(k, q)| {
                        group[*k] != group[low]
                            && q.hi[2] <= p.lo[2] + tol
                            && (0..2).all(|a| q.lo[a] < p.hi[a] && p.lo[a] < q.hi[a])
                    })
                    .max_by(|a, b| a.1.hi[2].total_cmp(&b.1.hi[2]));
                let (what, top) = match under {
                    Some((_, q)) => (q.o.id.as_str(), q.hi[2]),
                    None => ("the ground", 0.0),
                };
                let gap = p.lo[2] - top;
                out.push(format!(
                    "{} {} in the air: nothing holds {}; the bottom of {} is {} above {what} (move {} down {}, or join it to something)",
                    names(m),
                    if m.len() == 1 { "floats" } else { "float together" },
                    if m.len() == 1 { "it" } else { "them" },
                    p.o.id,
                    num(gap),
                    if m.len() == 1 { "it" } else { "them" },
                    num(gap)
                ));
            }
        } else if groups.len() > 1 {
            let list: Vec<String> = groups.iter().map(|m| format!("[{}]", names(m))).collect();
            out.push(format!(
                "{} separate groups that don't touch: {}",
                groups.len(),
                list.join(" ")
            ));
        }

        if !overlaps.is_empty() {
            overlaps.sort_by(|a, b| b.2.total_cmp(&a.2));
            let list: Vec<String> = overlaps
                .iter()
                .take(8)
                .map(|(i, j, d)| {
                    format!(
                        "{} and {} ({} deep)",
                        placed[*i].o.id,
                        placed[*j].o.id,
                        num(*d)
                    )
                })
                .collect();
            let more = if overlaps.len() > 8 {
                format!(" and {} more", overlaps.len() - 8)
            } else {
                String::new()
            };
            out.push(format!(
                "parts run into each other (fine where meant, like a leg set into a top): {}{more}",
                list.join(", ")
            ));
        }
        out
    }

    /// The box the views show: everything, the ground at z 0, a margin.
    fn view_box(&self) -> (V3, V3) {
        let (mut lo, mut hi) = self
            .bounds()
            .unwrap_or(([-0.5, -0.5, 0.0], [0.5, 0.5, 1.0]));
        if self.ground {
            lo[2] = lo[2].min(0.0);
            hi[2] = hi[2].max(0.0);
        }
        let ext = (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max).max(1e-6);
        for i in 0..3 {
            // Flat sides get some depth; all get a margin.
            let pad = ext * 0.06 + ((ext * 0.05 - (hi[i] - lo[i])) / 2.0).max(0.0);
            lo[i] -= pad;
            hi[i] += pad;
        }
        (lo, hi)
    }

    /// The grid spacing of the views: about eight lines across.
    pub fn step(&self) -> f64 {
        let (lo, hi) = self.view_box();
        let ext = (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max);
        imaging::nice_step(ext, 8.0)
    }

    /// The views on one picture.
    pub fn render(&self, view: SceneView, look: [f64; 2], ids: bool) -> Capture {
        let views: Vec<Look> = match view {
            SceneView::All => vec![Look::Front, Look::Right, Look::Top, Look::Persp],
            SceneView::Front => vec![Look::Front],
            SceneView::Right => vec![Look::Right],
            SceneView::Top => vec![Look::Top],
            SceneView::Perspective => vec![Look::Persp],
        };
        let side = if views.len() == 1 { 720 } else { 360 };
        let cols = if views.len() == 1 { 1 } else { 2 };
        let (w, h) = (side * cols, side * views.len().div_ceil(cols));
        let mut cap = Capture {
            width: w as u32,
            height: h as u32,
            rgba: vec![255; w * h * 4],
            bounds: Rect::new(0.0, 0.0, w as f64, h as f64),
        };
        let (lo, hi) = self.view_box();
        let step = self.step();
        let tris: Vec<(usize, Vec<[V3; 3]>)> = self
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| (i, o.triangles()))
            .collect();
        // Front, right and top to one scale.
        let flat: Vec<Look> = views
            .iter()
            .copied()
            .filter(|v| *v != Look::Persp)
            .collect();
        let pad = Pad::flat(lo, hi, step);
        let scale = flat
            .iter()
            .map(|v| {
                let (u0, u1, v0, v1) = v.extent(lo, hi);
                ((side as f64 - pad.left - pad.right) / (u1 - u0))
                    .min((side as f64 - pad.top - pad.bottom) / (v1 - v0))
            })
            .fold(f64::MAX, f64::min);
        for (k, v) in views.iter().enumerate() {
            let (px, py) = ((k % cols) * side, (k / cols) * side);
            let panel = if *v == Look::Persp {
                Panel::perspective(lo, hi, side, look, self.ground)
            } else {
                Panel::flat(*v, lo, hi, side, scale, pad)
            };
            let pic = panel.paint(self, &tris, step, lo, hi);
            for y in 0..side {
                for x in 0..side {
                    let o = ((py + y) * w + px + x) * 4;
                    cap.rgba[o..o + 3].copy_from_slice(&pic[y * side + x]);
                }
            }
            panel.labels(&mut cap, self, (px, py), step, ids, lo, hi);
        }
        // Lines between the views.
        if views.len() > 1 {
            for y in 0..h {
                for x in [side - 1, side] {
                    let o = (y * w + x) * 4;
                    cap.rgba[o..o + 3].copy_from_slice(&[150, 150, 150]);
                }
            }
            for x in 0..w {
                for y in [side - 1, side] {
                    let o = (y * w + x) * 4;
                    cap.rgba[o..o + 3].copy_from_slice(&[150, 150, 150]);
                }
            }
        }
        cap
    }

    /// The scene as an OBJ file (Y up, as OBJ files are) and its colours as
    /// the MTL file it names.
    pub fn obj(&self, name: &str, mtl_file: &str) -> (String, String) {
        let mut obj = format!(
            "# Scene \"{name}\": metres, Z up in the scene (OBJ's Y up here).\n# Planned with computer-use (mhrsdev/zero-use-computer on GitHub).\nmtllib {mtl_file}\n"
        );
        let mut mtl = String::from("# computer-use scene colours (by mhrsdev)\n\n");
        let mut colours: Vec<Rgb> = Vec::new();
        let mut next = 1usize;
        for o in &self.objects {
            let material = format!("c{}", hex(o.color).trim_start_matches('#'));
            if !colours.contains(&o.color) {
                colours.push(o.color);
                let c = o.color.map(|v| f64::from(v) / 255.0);
                mtl.push_str(&format!(
                    "newmtl {material}\nKd {:.4} {:.4} {:.4}\nKa 0 0 0\nKs 0.1 0.1 0.1\nNs 20\nd 1\nillum 2\n\n",
                    c[0], c[1], c[2]
                ));
            }
            let id: String =
                o.id.chars()
                    .map(|c| if c.is_whitespace() { '_' } else { c })
                    .collect();
            obj.push_str(&format!("o {id}\nusemtl {material}\n"));
            let tris = o.triangles();
            for t in &tris {
                for p in t {
                    obj.push_str(&format!("v {:.6} {:.6} {:.6}\n", p[0], p[2], -p[1]));
                }
            }
            for _ in &tris {
                obj.push_str(&format!("f {} {} {}\n", next, next + 1, next + 2));
                next += 3;
            }
        }
        (obj, mtl)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    /// From the front (-y): x right, z up.
    Front,
    /// From the right (+x): y right, z up.
    Right,
    /// From above: x right, y up.
    Top,
    Persp,
}

impl Look {
    /// (across, up, depth: smaller is nearer) of a scene point.
    fn flat(self, p: V3) -> (f64, f64, f64) {
        match self {
            Look::Front => (p[0], p[2], p[1]),
            Look::Right => (p[1], p[2], -p[0]),
            Look::Top | Look::Persp => (p[0], p[1], -p[2]),
        }
    }

    fn extent(self, lo: V3, hi: V3) -> (f64, f64, f64, f64) {
        match self {
            Look::Front => (lo[0], hi[0], lo[2], hi[2]),
            Look::Right => (lo[1], hi[1], lo[2], hi[2]),
            Look::Top | Look::Persp => (lo[0], hi[0], lo[1], hi[1]),
        }
    }

    fn title(self) -> &'static str {
        match self {
            Look::Front => "FRONT",
            Look::Right => "RIGHT",
            Look::Top => "TOP",
            Look::Persp => "PERSPECTIVE",
        }
    }
}

/// Room around a flat view for its numbers and title.
#[derive(Debug, Clone, Copy)]
struct Pad {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

impl Pad {
    fn flat(lo: V3, hi: V3, step: f64) -> Pad {
        // Wide enough for the longest number up the side.
        let widest = [lo[1], hi[1], lo[2], hi[2]]
            .iter()
            .map(|v| imaging::grid_label((v / step).round() * step, step).len())
            .max()
            .unwrap_or(1)
            .max(2);
        Pad {
            left: imaging::tag_size(&"0".repeat(widest), 2).0 as f64 + 6.0,
            right: 10.0,
            top: 24.0,
            bottom: 22.0,
        }
    }
}

/// One view: how scene points land on its pixels (at the final size).
struct Panel {
    look: Look,
    side: usize,
    /// Flat views: pixels per unit and the point at the middle.
    scale: f64,
    mid: (f64, f64),
    centre: (f64, f64),
    /// Perspective: the eye and its axes.
    eye: V3,
    right: V3,
    up: V3,
    fwd: V3,
    ground: bool,
}

impl Panel {
    fn flat(look: Look, lo: V3, hi: V3, side: usize, scale: f64, pad: Pad) -> Panel {
        let (u0, u1, v0, v1) = look.extent(lo, hi);
        Panel {
            look,
            side,
            scale,
            mid: ((u0 + u1) / 2.0, (v0 + v1) / 2.0),
            centre: (
                pad.left + (side as f64 - pad.left - pad.right) / 2.0,
                pad.top + (side as f64 - pad.top - pad.bottom) / 2.0,
            ),
            eye: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fwd: [0.0, 1.0, 0.0],
            ground: false,
        }
    }

    fn perspective(lo: V3, hi: V3, side: usize, look: [f64; 2], ground: bool) -> Panel {
        let centre = times(add(lo, hi), 0.5);
        let radius = (dot(sub(hi, lo), sub(hi, lo)).sqrt() / 2.0).max(1e-6);
        let (turn, tilt) = (
            look[0].to_radians(),
            look[1].clamp(-89.0, 89.0).to_radians(),
        );
        let toward = [
            turn.sin() * tilt.cos(),
            -turn.cos() * tilt.cos(),
            tilt.sin(),
        ];
        let eye = add(centre, times(toward, radius * 4.5));
        let fwd = times(toward, -1.0);
        let right = unit(cross(fwd, [0.0, 0.0, 1.0]));
        let up = cross(right, fwd);
        let mut p = Panel {
            look: Look::Persp,
            side,
            scale: 1.0,
            mid: (0.0, 0.0),
            centre: (side as f64 / 2.0, 12.0 + side as f64 / 2.0),
            eye,
            right,
            up,
            fwd,
            ground,
        };
        // Fit the corners of everything.
        let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for i in 0..8 {
            let c = [
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let (u, v, _) = p.camera(c);
            (u0, u1, v0, v1) = (u0.min(u), u1.max(u), v0.min(v), v1.max(v));
        }
        let room = side as f64 - 40.0;
        p.scale = (room / (u1 - u0).max(1e-9)).min((room - 12.0) / (v1 - v0).max(1e-9));
        p.mid = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
        p
    }

    /// Across and up on the eye's picture plane, and the depth.
    fn camera(&self, p: V3) -> (f64, f64, f64) {
        let q = sub(p, self.eye);
        let z = dot(q, self.fwd).max(1e-9);
        (dot(q, self.right) / z, dot(q, self.up) / z, z)
    }

    /// A scene point on the panel (final pixels) and a depth key (smaller
    /// is nearer; straight across the panel for both kinds of view).
    fn project(&self, p: V3) -> (f64, f64, f64) {
        let (u, v, d) = if self.look == Look::Persp {
            let (u, v, z) = self.camera(p);
            (u, v, -1.0 / z)
        } else {
            self.look.flat(p)
        };
        (
            self.centre.0 + (u - self.mid.0) * self.scale,
            self.centre.1 - (v - self.mid.1) * self.scale,
            d,
        )
    }

    /// Towards the eye from a point.
    fn toward_eye(&self, p: V3) -> V3 {
        match self.look {
            Look::Front => [0.0, -1.0, 0.0],
            Look::Right => [1.0, 0.0, 0.0],
            Look::Top => [0.0, 0.0, 1.0],
            Look::Persp => unit(sub(self.eye, p)),
        }
    }

    fn screen_axes(&self) -> (V3, V3) {
        match self.look {
            Look::Front => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Look::Right => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            Look::Top => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Look::Persp => (self.right, self.up),
        }
    }

    fn paint(
        &self,
        scene: &Scene,
        tris: &[(usize, Vec<[V3; 3]>)],
        step: f64,
        lo: V3,
        hi: V3,
    ) -> Vec<[u8; 3]> {
        let mut r = Raster::new(self.side * SS, [0.973, 0.973, 0.965]);
        let k = SS as f64;
        let at = |p: V3| {
            let (x, y, d) = self.project(p);
            (x * k, y * k, d)
        };
        let grid = [0.86, 0.86, 0.86];
        let axis = [0.70, 0.70, 0.70];
        if self.look == Look::Persp {
            // The ground's grid, then the shadows straight down.
            if self.ground {
                let first = |a: f64| (a / step).floor() as i64 - 1;
                let last = |a: f64| (a / step).ceil() as i64 + 1;
                let (x0, x1) = (first(lo[0]) as f64 * step, last(hi[0]) as f64 * step);
                let (y0, y1) = (first(lo[1]) as f64 * step, last(hi[1]) as f64 * step);
                for i in first(lo[0])..=last(hi[0]) {
                    let x = i as f64 * step;
                    let c = if i == 0 { axis } else { grid };
                    r.line(at([x, y0, 0.0]), at([x, y1, 0.0]), c, k);
                }
                for j in first(lo[1])..=last(hi[1]) {
                    let y = j as f64 * step;
                    let c = if j == 0 { axis } else { grid };
                    r.line(at([x0, y, 0.0]), at([x1, y, 0.0]), c, k);
                }
                for (_, ts) in tris {
                    for t in ts {
                        let s = t.map(|p| at([p[0], p[1], 0.0]));
                        r.shadow(s, [0.80, 0.80, 0.79]);
                    }
                }
            }
        } else {
            let (u0, u1, v0, v1) = self.look.extent(lo, hi);
            let px = |u: f64, v: f64| {
                let p = (
                    self.centre.0 + (u - self.mid.0) * self.scale,
                    self.centre.1 - (v - self.mid.1) * self.scale,
                );
                (p.0 * k, p.1 * k, 0.0)
            };
            let span = |a: f64, b: f64| ((a / step).floor() as i64)..=((b / step).ceil() as i64);
            let (wide0, wide1) = (u0 - (u1 - u0), u1 + (u1 - u0));
            let (tall0, tall1) = (v0 - (v1 - v0), v1 + (v1 - v0));
            for i in span(wide0, wide1) {
                let u = i as f64 * step;
                r.line(
                    px(u, tall0),
                    px(u, tall1),
                    if i == 0 { axis } else { grid },
                    k,
                );
            }
            for j in span(tall0, tall1) {
                let v = j as f64 * step;
                r.line(
                    px(wide0, v),
                    px(wide1, v),
                    if j == 0 { axis } else { grid },
                    k,
                );
            }
            if scene.ground && self.look != Look::Top {
                r.line(px(wide0, 0.0), px(wide1, 0.0), [0.55, 0.45, 0.35], 2.0 * k);
            }
        }
        let (sr, su) = self.screen_axes();
        for (i, ts) in tris {
            let o = &scene.objects[*i];
            let base = o.color.map(|v| f32::from(v) / 255.0);
            for t in ts {
                let c = times(add(add(t[0], t[1]), t[2]), 1.0 / 3.0);
                let eye = self.toward_eye(c);
                let mut n = unit(normal(t));
                if dot(n, eye) < 0.0 {
                    n = times(n, -1.0);
                }
                let light = unit(add(add(eye, times(su, 0.7)), times(sr, -0.4)));
                let b = (0.42 + 0.58 * dot(n, light).max(0.0)) as f32;
                r.tri(
                    t.map(at),
                    base.map(|v| v * b),
                    *i as i32,
                    n.map(|v| v as f32),
                );
            }
        }
        r.edges();
        r.shrink(SS)
    }

    /// Numbers along the sides, the title, and the objects' ids.
    #[allow(clippy::too_many_arguments)]
    fn labels(
        &self,
        cap: &mut Capture,
        scene: &Scene,
        origin: (usize, usize),
        step: f64,
        ids: bool,
        lo: V3,
        hi: V3,
    ) {
        let (ox, oy) = (origin.0 as i64, origin.1 as i64);
        let s = self.side as f64;
        let mut taken: Vec<(i64, i64, i64, i64)> = Vec::new();
        let put = |cap: &mut Capture,
                   text: &str,
                   x: i64,
                   y: i64,
                   fg: [u8; 3],
                   bg: [u8; 3],
                   taken: &mut Vec<(i64, i64, i64, i64)>| {
            let (w, h) = imaging::tag_size(text, 2);
            let r = (x, y, x + w, y + h);
            if x < 0 || y < 0 || x + w > self.side as i64 || y + h > self.side as i64 {
                return;
            }
            if taken
                .iter()
                .any(|t| r.0 < t.2 && t.0 < r.2 && r.1 < t.3 && t.1 < r.3)
            {
                return;
            }
            taken.push(r);
            imaging::draw_tag(cap, text, ox + x, oy + y, 2, fg, bg);
        };
        put(
            cap,
            self.look.title(),
            4,
            4,
            [255, 255, 255],
            [70, 70, 70],
            &mut taken,
        );
        if self.look != Look::Persp {
            let (u0, u1, v0, v1) = self.look.extent(lo, hi);
            let gap = step * self.scale;
            let every = |w: i64| ((w as f64 + 8.0) / gap.max(1e-9)).ceil().max(1.0) as i64;
            let at_u = |u: f64| self.centre.0 + (u - self.mid.0) * self.scale;
            let at_v = |v: f64| self.centre.1 - (v - self.mid.1) * self.scale;
            let widest = imaging::tag_size("-0000", 2).0;
            let n = every(widest);
            let first = (u0 / step).ceil() as i64;
            for i in first..=(u1 / step).floor() as i64 {
                if i.rem_euclid(n) != 0 {
                    continue;
                }
                let text = imaging::grid_label(i as f64 * step, step);
                let w = imaging::tag_size(&text, 2).0;
                let x = at_u(i as f64 * step).round() as i64 - w / 2;
                put(
                    cap,
                    &text,
                    x,
                    s as i64 - 18,
                    [255, 255, 0],
                    [0, 0, 0],
                    &mut taken,
                );
            }
            let m = every(14);
            for j in (v0 / step).ceil() as i64..=(v1 / step).floor() as i64 {
                if j.rem_euclid(m) != 0 {
                    continue;
                }
                let text = imaging::grid_label(j as f64 * step, step);
                let y = at_v(j as f64 * step).round() as i64 - 7;
                put(cap, &text, 2, y, [255, 255, 0], [0, 0, 0], &mut taken);
            }
        }
        if ids && scene.objects.len() <= 40 {
            // The nearest first: where names would overlap, what is in
            // front keeps its name.
            let mut order: Vec<(f64, &Object)> = scene
                .objects
                .iter()
                .map(|o| (self.project(o.at).2, o))
                .collect();
            order.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, o) in order {
                let (x, y, _) = self.project(o.at);
                let text: String = o.id.chars().take(14).collect();
                let (w, h) = imaging::tag_size(&text, 2);
                put(
                    cap,
                    &text,
                    x.round() as i64 - w / 2,
                    y.round() as i64 - h / 2,
                    [255, 255, 255],
                    [40, 40, 40],
                    &mut taken,
                );
            }
        }
    }
}

/// A small z-buffered painter, kept compact (a big view has millions of
/// pixels): colours as bytes, normals as signed bytes.
struct Raster {
    side: usize,
    rgb: Vec<[u8; 3]>,
    depth: Vec<f32>,
    id: Vec<i32>,
    normal: Vec<[i8; 3]>,
}

fn bytes(c: [f32; 3]) -> [u8; 3] {
    c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
}

impl Raster {
    fn new(side: usize, bg: [f32; 3]) -> Raster {
        Raster {
            side,
            rgb: vec![bytes(bg); side * side],
            depth: vec![f32::MAX; side * side],
            id: vec![-1; side * side],
            normal: vec![[0; 3]; side * side],
        }
    }

    /// Pixels a triangle covers, with its corners' weights there.
    fn cover(&self, p: [(f64, f64, f64); 3], mut f: impl FnMut(usize, [f64; 3])) {
        let edge = |a: (f64, f64, f64), b: (f64, f64, f64), x: f64, y: f64| {
            (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
        };
        let area = edge(p[0], p[1], p[2].0, p[2].1);
        if area.abs() < 1e-12 || p.iter().any(|q| !q.0.is_finite() || !q.1.is_finite()) {
            return;
        }
        let side = self.side as f64;
        let x0 = p
            .iter()
            .map(|q| q.0)
            .fold(f64::MAX, f64::min)
            .floor()
            .max(0.0);
        let x1 = p
            .iter()
            .map(|q| q.0)
            .fold(f64::MIN, f64::max)
            .ceil()
            .min(side - 1.0);
        let y0 = p
            .iter()
            .map(|q| q.1)
            .fold(f64::MAX, f64::min)
            .floor()
            .max(0.0);
        let y1 = p
            .iter()
            .map(|q| q.1)
            .fold(f64::MIN, f64::max)
            .ceil()
            .min(side - 1.0);
        if x1 < x0 || y1 < y0 {
            return;
        }
        for y in y0 as usize..=y1 as usize {
            for x in x0 as usize..=x1 as usize {
                let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
                let w = [
                    edge(p[1], p[2], cx, cy) / area,
                    edge(p[2], p[0], cx, cy) / area,
                    edge(p[0], p[1], cx, cy) / area,
                ];
                if w.iter().all(|v| *v >= -1e-9) {
                    f(y * self.side + x, w);
                }
            }
        }
    }

    fn tri(&mut self, p: [(f64, f64, f64); 3], rgb: [f32; 3], id: i32, n: [f32; 3]) {
        let (rgb, n) = (bytes(rgb), n.map(|v| (v * 127.0).round() as i8));
        let mut hits = Vec::new();
        self.cover(p, |i, w| {
            hits.push((i, (w[0] * p[0].2 + w[1] * p[1].2 + w[2] * p[2].2) as f32));
        });
        for (i, d) in hits {
            if d < self.depth[i] {
                self.depth[i] = d;
                self.rgb[i] = rgb;
                self.id[i] = id;
                self.normal[i] = n;
            }
        }
    }

    fn shadow(&mut self, p: [(f64, f64, f64); 3], rgb: [f32; 3]) {
        let mut hits = Vec::new();
        self.cover(p, |i, _| hits.push(i));
        for i in hits {
            self.rgb[i] = bytes(rgb);
        }
    }

    fn line(&mut self, a: (f64, f64, f64), b: (f64, f64, f64), rgb: [f32; 3], width: f64) {
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        if !len.is_finite() {
            return;
        }
        let n = len.ceil().max(1.0) as usize;
        let r = (width / 2.0).max(0.5);
        let side = self.side as i64;
        let c = bytes(rgb);
        for s in 0..=n {
            let t = s as f64 / n as f64;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            for yy in (y - r).round() as i64..(y + r).round() as i64 {
                for xx in (x - r).round() as i64..(x + r).round() as i64 {
                    if (0..side).contains(&xx) && (0..side).contains(&yy) {
                        self.rgb[(yy * side + xx) as usize] = c;
                    }
                }
            }
        }
    }

    /// Dark outlines where one object ends, lighter ones at its creases.
    fn edges(&mut self) {
        let s = self.side;
        let mut mark = vec![0u8; s * s];
        for y in 0..s {
            for x in 0..s {
                let i = y * s + x;
                for j in [(x + 1 < s).then(|| i + 1), (y + 1 < s).then(|| i + s)]
                    .into_iter()
                    .flatten()
                {
                    let (a, b) = (self.id[i], self.id[j]);
                    if a != b {
                        mark[i] = 2;
                        mark[j] = 2;
                    } else if a >= 0 {
                        let (n, m) = (self.normal[i], self.normal[j]);
                        let dot: i32 = (0..3).map(|k| i32::from(n[k]) * i32::from(m[k])).sum();
                        // cos below 0.8: a crease.
                        if dot < 12_903 {
                            mark[i] = mark[i].max(1);
                            mark[j] = mark[j].max(1);
                        }
                    }
                }
            }
        }
        for (c, m) in self.rgb.iter_mut().zip(&mark) {
            match m {
                2 => *c = [41, 41, 41],
                1 => *c = c.map(|v| (f32::from(v) * 0.55) as u8),
                _ => {}
            }
        }
    }

    /// Averaged down `k` times.
    fn shrink(&self, k: usize) -> Vec<[u8; 3]> {
        let out = self.side / k;
        let mut v = vec![[0u8; 3]; out * out];
        for y in 0..out {
            for x in 0..out {
                let mut sum = [0u32; 3];
                for dy in 0..k {
                    for dx in 0..k {
                        let c = self.rgb[(y * k + dy) * self.side + x * k + dx];
                        for (s, c) in sum.iter_mut().zip(c) {
                            *s += u32::from(c);
                        }
                    }
                }
                let n = (k * k) as u32;
                v[y * out + x] = sum.map(|s| ((s + n / 2) / n) as u8);
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{SceneMirror, SceneRepeat};

    fn obj(id: &str, shape: SceneShape, size: &[f64], at: V3) -> SceneObject {
        SceneObject {
            id: Some(id.into()),
            shape: Some(shape),
            size: Some(size.to_vec()),
            at: Some(at),
            ..Default::default()
        }
    }

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6)
    }

    #[test]
    fn bounds_follow_shape_and_turn() {
        let mut o = Object {
            id: "b".into(),
            shape: SceneShape::Box,
            size: [2.0, 1.0, 0.5],
            at: [0.0, 0.0, 1.0],
            rotate: [0.0, 0.0, 90.0],
            color: [0; 3],
        };
        let (lo, hi) = o.bounds();
        assert!(
            close(lo, [-0.5, -1.0, 0.75]) && close(hi, [0.5, 1.0, 1.25]),
            "{lo:?} {hi:?}"
        );
        // A cylinder lying down along y.
        o.shape = SceneShape::Cylinder;
        o.size = [1.0, 1.0, 4.0];
        o.rotate = [90.0, 0.0, 0.0];
        let (lo, hi) = o.bounds();
        assert!(
            close(lo, [-0.5, -2.0, 0.5]) && close(hi, [0.5, 2.0, 1.5]),
            "{lo:?} {hi:?}"
        );
        // A cone's tip is at the top.
        o.shape = SceneShape::Cone;
        o.rotate = [0.0; 3];
        assert!(o.inside_local([0.0, 0.0, 1.9], 0.0));
        assert!(!o.inside_local([0.4, 0.0, 1.9], 0.0));
        assert!(o.inside_local([0.4, 0.0, -1.9], 0.0));
        let r = rotation([30.0, -50.0, 120.0]);
        assert!(close(euler(r), [30.0, -50.0, 120.0]), "{:?}", euler(r));
    }

    #[test]
    fn meshes_close_and_face_out() {
        for shape in [
            SceneShape::Box,
            SceneShape::Cylinder,
            SceneShape::Sphere,
            SceneShape::Cone,
            SceneShape::Torus,
        ] {
            let o = Object {
                id: "o".into(),
                shape,
                size: [2.0, 2.0, 0.5],
                at: [0.0; 3],
                rotate: [0.0; 3],
                color: [0; 3],
            };
            // The divergence theorem: outward faces give a positive volume.
            let v: f64 = o
                .mesh()
                .iter()
                .map(|t| dot(t[0], cross(t[1], t[2])) / 6.0)
                .sum();
            let want = match shape {
                SceneShape::Box => 2.0,
                SceneShape::Cylinder => std::f64::consts::PI * 0.5,
                SceneShape::Sphere => 4.0 / 3.0 * std::f64::consts::PI * 0.25,
                SceneShape::Cone => std::f64::consts::PI * 0.5 / 3.0,
                _ => 2.0 * std::f64::consts::PI.powi(2) * 0.75 * 0.0625,
            };
            assert!((v - want).abs() < want * 0.04, "{shape:?} {v} {want}");
        }
    }

    #[test]
    fn a_table_is_built_mirrored_checked_and_written() {
        let mut s = Scene::default();
        let args = SceneArgs {
            name: "table".into(),
            add: Some(vec![
                obj("leg", SceneShape::Box, &[0.05, 0.05, 0.7], [0.5, 0.3, 0.35]),
                SceneObject {
                    on: Some("leg".into()),
                    color: Some("#8B5A2B".into()),
                    ..obj("top", SceneShape::Box, &[1.2, 0.8, 0.04], [0.0, 0.0, 9.0])
                },
                obj("cup", SceneShape::Cylinder, &[0.08, 0.1], [0.2, 0.0, 0.85]),
                obj("ball", SceneShape::Sphere, &[0.2], [-0.3, 0.0, 0.0]),
            ]),
            mirror: Some(vec![SceneMirror {
                id: "leg".into(),
                copy: "leg-b".into(),
                axis: None,
                at: None,
            }]),
            repeat: Some(vec![SceneRepeat {
                id: "leg".into(),
                count: 2,
                offset: Some([0.0, -0.6, 0.0]),
                ..Default::default()
            }]),
            ..Default::default()
        };
        s.apply(&args).unwrap();
        let top = &s.objects[s.find("top").unwrap()];
        assert!(close(top.at, [0.0, 0.0, 0.72]), "{:?}", top.at);
        assert!(close(
            s.objects[s.find("leg-b").unwrap()].at,
            [-0.5, 0.3, 0.35]
        ));
        assert!(close(
            s.objects[s.find("leg-2").unwrap()].at,
            [0.5, -0.3, 0.35]
        ));
        let checks = s.checks();
        let all = checks.join(" | ");
        // The cup floats 0.06 above the top; the ball sinks 0.1.
        assert!(
            all.contains(
                "cup floats in the air: nothing holds it; the bottom of cup is 0.06 above top"
            ),
            "{all}"
        );
        assert!(all.contains("ball sinks 0.1 below the ground"), "{all}");
        assert!(!all.contains("leg"), "{all}");

        // Set it down, and push a leg up into the top.
        let fix = SceneArgs {
            name: "table".into(),
            change: Some(vec![
                SceneObject {
                    id: Some("cup".into()),
                    on: Some("top".into()),
                    ..Default::default()
                },
                SceneObject {
                    id: Some("ball".into()),
                    on: Some("ground".into()),
                    ..Default::default()
                },
                SceneObject {
                    id: Some("leg-2".into()),
                    shift: Some([0.0, 0.0, 0.02]),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        s.apply(&fix).unwrap();
        let all = s.checks().join(" | ");
        assert!(all.contains("top and leg-2 (0.02 deep)"), "{all}");
        assert!(!all.contains("floats") && !all.contains("sinks"), "{all}");
        assert!(
            s.listing()
                .contains("top box 1.2 x 0.8 x 0.04 at (0, 0, 0.72), z 0.7 to 0.74, #8B5A2B"),
            "{}",
            s.listing()
        );

        let pic = s.render(SceneView::All, [35.0, 25.0], true);
        assert_eq!((pic.width, pic.height), (720, 720));
        // The top's brown shows in the front view.
        let brown = pic
            .rgba
            .chunks(4)
            .filter(|p| i32::from(p[0]) > i32::from(p[2]) + 40 && p[0] > 90)
            .count();
        assert!(brown > 2000, "{brown}");
        let (o, m) = s.obj("table", "table-1.mtl");
        assert!(
            o.contains("mtllib table-1.mtl\no leg\nusemtl cAAAAAA"),
            "{o}"
        );
        assert!(m.contains("newmtl c8B5A2B\nKd 0.5451 0.3529 0.1686"), "{m}");
        assert_eq!(o.matches("\nf ").count() * 3, o.matches("\nv ").count());
    }

    #[test]
    fn copies_go_around_and_mirrors_turn() {
        let mut s = Scene::default();
        let args = SceneArgs {
            name: "w".into(),
            add: Some(vec![
                SceneObject {
                    rotate: Some([0.0, 30.0, 10.0]),
                    ..obj("blade", SceneShape::Box, &[1.0, 0.1, 0.02], [1.0, 0.0, 1.0])
                },
                obj("tip", SceneShape::Cone, &[0.2, 0.4], [0.0, 0.0, 0.5]),
            ]),
            mirror: Some(vec![
                SceneMirror {
                    id: "blade".into(),
                    copy: "blade-m".into(),
                    axis: Some("y".into()),
                    at: Some(0.5),
                },
                SceneMirror {
                    id: "tip".into(),
                    copy: "tip-down".into(),
                    axis: Some("z".into()),
                    at: Some(0.0),
                },
            ]),
            repeat: Some(vec![SceneRepeat {
                id: "blade".into(),
                count: 4,
                around: Some([0.0, 0.0]),
                ..Default::default()
            }]),
            ground: Some(false),
            ..Default::default()
        };
        s.apply(&args).unwrap();
        let b3 = &s.objects[s.find("blade-3").unwrap()];
        assert!(
            close(b3.at, [-1.0, 0.0, 1.0]) && (b3.rotate[2] - 190.0 + 360.0).abs() < 1e-6,
            "{b3:?}"
        );
        // The mirrored blade's corners are the original's, flipped in y.
        let (a, m) = (&s.objects[0], &s.objects[s.find("blade-m").unwrap()]);
        let (alo, ahi) = a.bounds();
        let (mlo, mhi) = m.bounds();
        assert!((mlo[1] - (1.0 - ahi[1])).abs() < 1e-9 && (mhi[1] - (1.0 - alo[1])).abs() < 1e-9);
        // The mirrored cone points down.
        let down = &s.objects[s.find("tip-down").unwrap()];
        let r = down.rot();
        let tip = add(mul(r, [0.0, 0.0, 0.2]), down.at);
        assert!(close(tip, [0.0, 0.0, -0.7]), "{tip:?}");
        // No ground: separate pieces are named.
        assert!(
            s.checks().join(" ").contains("separate groups"),
            "{:?}",
            s.checks()
        );
        assert!(
            s.apply(&SceneArgs {
                name: "w".into(),
                repeat: Some(vec![SceneRepeat {
                    id: "tip".into(),
                    count: 3,
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .unwrap_err()
            .contains("offset")
        );
    }
}
