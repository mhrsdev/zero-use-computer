//! Drawing with the mouse (`draw`): strokes given as points or as
//! parametric curves `x(t)`, `y(t)`, turned into evenly spaced screen points
//! for a press-move-release gesture.
//!
//! Expressions are parsed by a small recursive-descent parser into a tree
//! and evaluated as plain arithmetic: no names other than `t`, a few
//! constants and a fixed set of math functions, so nothing in them can do
//! anything but compute a number.

use crate::types::{Point, Rect};

/// Longest expression accepted, in bytes.
const MAX_EXPR_LEN: usize = 2000;
/// Deepest nesting accepted (parentheses, operators, calls).
const MAX_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// Expressions

/// A math expression in `t`, parsed once and evaluated many times.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr(Node);

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Num(f64),
    T,
    Neg(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    /// An index into [`FUNCS`] and the arguments.
    Call(usize, Vec<Node>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
}

type MathFn = fn(&[f64]) -> f64;

/// Functions by name: (name, fewest arguments, most arguments, function).
const FUNCS: &[(&str, usize, usize, MathFn)] = &[
    ("sin", 1, 1, |a| a[0].sin()),
    ("cos", 1, 1, |a| a[0].cos()),
    ("tan", 1, 1, |a| a[0].tan()),
    ("asin", 1, 1, |a| a[0].asin()),
    ("acos", 1, 1, |a| a[0].acos()),
    ("atan", 1, 1, |a| a[0].atan()),
    ("atan2", 2, 2, |a| a[0].atan2(a[1])),
    ("sinh", 1, 1, |a| a[0].sinh()),
    ("cosh", 1, 1, |a| a[0].cosh()),
    ("tanh", 1, 1, |a| a[0].tanh()),
    ("sqrt", 1, 1, |a| a[0].sqrt()),
    ("cbrt", 1, 1, |a| a[0].cbrt()),
    ("abs", 1, 1, |a| a[0].abs()),
    ("exp", 1, 1, |a| a[0].exp()),
    ("ln", 1, 1, |a| a[0].ln()),
    ("log", 1, 1, |a| a[0].ln()),
    ("log10", 1, 1, |a| a[0].log10()),
    ("log2", 1, 1, |a| a[0].log2()),
    ("floor", 1, 1, |a| a[0].floor()),
    ("ceil", 1, 1, |a| a[0].ceil()),
    ("round", 1, 1, |a| a[0].round()),
    ("trunc", 1, 1, |a| a[0].trunc()),
    ("sign", 1, 1, |a| {
        if a[0] == 0.0 || a[0].is_nan() {
            a[0]
        } else {
            a[0].signum()
        }
    }),
    ("min", 1, 64, |a| {
        a.iter().copied().fold(f64::INFINITY, f64::min)
    }),
    ("max", 1, 64, |a| {
        a.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }),
    ("pow", 2, 2, |a| a[0].powf(a[1])),
    ("hypot", 2, 2, |a| a[0].hypot(a[1])),
    ("mod", 2, 2, |a| a[0].rem_euclid(a[1])),
    ("clamp", 3, 3, |a| a[0].max(a[1]).min(a[2])),
];

const CONSTANTS: &[(&str, f64)] = &[
    ("pi", std::f64::consts::PI),
    ("tau", std::f64::consts::TAU),
    ("e", std::f64::consts::E),
];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(char),
    Open,
    Close,
    Comma,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // An exponent: 1e3, 2.5E-4 (but not the constant e after a number).
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().collect();
            let n = text
                .parse::<f64>()
                .map_err(|_| format!("`{text}` is not a number"))?;
            out.push(Tok::Num(n));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(
                chars[start..i].iter().collect::<String>().to_lowercase(),
            ));
        } else {
            out.push(match c {
                '*' if chars.get(i + 1) == Some(&'*') => {
                    i += 1;
                    Tok::Op('^')
                }
                '+' | '-' | '*' | '/' | '%' | '^' => Tok::Op(c),
                '(' => Tok::Open,
                ')' => Tok::Close,
                ',' => Tok::Comma,
                other => return Err(format!("unexpected `{other}`")),
            });
            i += 1;
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("the expression is nested too deeply".into());
        }
        Ok(())
    }

    /// sum := term (('+' | '-') term)*
    fn sum(&mut self) -> Result<Node, String> {
        self.enter()?;
        let mut left = self.term()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek() {
            let op = if *c == '+' { Op::Add } else { Op::Sub };
            self.at += 1;
            let right = self.term()?;
            left = Node::Bin(op, Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }

    /// term := unary (('*' | '/' | '%') unary | unary)*, the last being an
    /// implicit product: `2t`, `2pi`, `3(t + 1)`, `t(1 - t)`.
    fn term(&mut self) -> Result<Node, String> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op('*')) => Op::Mul,
                Some(Tok::Op('/')) => Op::Div,
                Some(Tok::Op('%')) => Op::Rem,
                Some(Tok::Name(_) | Tok::Open | Tok::Num(_)) => {
                    let right = self.unary()?;
                    left = Node::Bin(Op::Mul, Box::new(left), Box::new(right));
                    continue;
                }
                _ => break,
            };
            self.at += 1;
            let right = self.unary()?;
            left = Node::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// unary := ('-' | '+') unary | power
    fn unary(&mut self) -> Result<Node, String> {
        self.enter()?;
        let node = match self.peek() {
            Some(Tok::Op('-')) => {
                self.at += 1;
                Node::Neg(Box::new(self.unary()?))
            }
            Some(Tok::Op('+')) => {
                self.at += 1;
                self.unary()?
            }
            _ => self.power()?,
        };
        self.depth -= 1;
        Ok(node)
    }

    /// power := atom ('^' unary)?   (right-associative: 2^3^2 = 2^9)
    fn power(&mut self) -> Result<Node, String> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.at += 1;
            let exp = self.unary()?;
            return Ok(Node::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn atom(&mut self) -> Result<Node, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(Node::Num(n)),
            Some(Tok::Open) => {
                let inner = self.sum()?;
                match self.next() {
                    Some(Tok::Close) => Ok(inner),
                    _ => Err("a `(` is not closed".into()),
                }
            }
            Some(Tok::Name(name)) => self.name(&name),
            Some(Tok::Close) => Err("unexpected `)`".into()),
            Some(Tok::Comma) => Err("unexpected `,`".into()),
            Some(Tok::Op(c)) => Err(format!("unexpected `{c}`")),
            None => Err("the expression ends too early".into()),
        }
    }

    fn name(&mut self, name: &str) -> Result<Node, String> {
        if name == "t" {
            return Ok(Node::T);
        }
        if let Some((_, v)) = CONSTANTS.iter().find(|(n, _)| *n == name) {
            return Ok(Node::Num(*v));
        }
        let Some(i) = FUNCS.iter().position(|(n, ..)| *n == name) else {
            let funcs: Vec<&str> = FUNCS.iter().map(|(n, ..)| *n).collect();
            return Err(format!(
                "unknown name `{name}`: use t, pi, tau, e or a function ({})",
                funcs.join(", ")
            ));
        };
        if self.next() != Some(Tok::Open) {
            return Err(format!("`{name}` needs its arguments in parentheses"));
        }
        let mut args = Vec::new();
        if self.peek() == Some(&Tok::Close) {
            self.at += 1;
        } else {
            loop {
                args.push(self.sum()?);
                match self.next() {
                    Some(Tok::Comma) => continue,
                    Some(Tok::Close) => break,
                    _ => return Err(format!("`{name}(` is not closed")),
                }
            }
        }
        let (_, min, max, _) = FUNCS[i];
        if args.len() < min || args.len() > max {
            let want = if min == max {
                min.to_string()
            } else {
                format!("{min} or more")
            };
            return Err(format!(
                "`{name}` takes {want} argument(s), got {}",
                args.len()
            ));
        }
        Ok(Node::Call(i, args))
    }
}

impl Expr {
    /// Parse `src`, an expression in `t`.
    pub fn parse(src: &str) -> Result<Expr, String> {
        if src.len() > MAX_EXPR_LEN {
            return Err(format!("longer than {MAX_EXPR_LEN} characters"));
        }
        let toks = tokenize(src)?;
        if toks.is_empty() {
            return Err("empty expression".into());
        }
        let mut p = Parser {
            toks,
            at: 0,
            depth: 0,
        };
        let node = p.sum()?;
        if let Some(extra) = p.peek() {
            return Err(match extra {
                Tok::Close => "unexpected `)`".into(),
                Tok::Comma => "unexpected `,`".into(),
                other => format!("unexpected {other:?} after the expression"),
            });
        }
        Ok(Expr(node))
    }

    /// The value at `t` (NaN or infinite where the math is undefined).
    pub fn eval(&self, t: f64) -> f64 {
        eval(&self.0, t)
    }
}

fn eval(n: &Node, t: f64) -> f64 {
    match n {
        Node::Num(v) => *v,
        Node::T => t,
        Node::Neg(a) => -eval(a, t),
        Node::Bin(op, a, b) => {
            let (a, b) = (eval(a, t), eval(b, t));
            match op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                Op::Div => a / b,
                Op::Rem => a % b,
                Op::Pow => a.powf(b),
            }
        }
        Node::Call(i, args) => {
            let vals: Vec<f64> = args.iter().map(|a| eval(a, t)).collect();
            (FUNCS[*i].3)(&vals)
        }
    }
}

// ---------------------------------------------------------------------------
// Strokes

/// The area coordinates refer to and its place on the screen: the latest
/// screenshot (pixels) or an element's box (fractions, 0 to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    origin: Point,
    scale_x: f64,
    scale_y: f64,
    /// Coordinates run from 0 to `width` and 0 to `height`.
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// Screenshot pixels: an image of `width` x `height` showing `bounds`.
    pub fn pixels(bounds: Rect, width: u32, height: u32) -> Self {
        let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
        Self {
            origin: Point::new(bounds.x, bounds.y),
            scale_x: bounds.width / w,
            scale_y: bounds.height / h,
            width: w,
            height: h,
        }
    }

    /// Fractions of `bounds` (screen coordinates).
    pub fn fractions(bounds: Rect) -> Self {
        Self::units(bounds, 1.0, 1.0)
    }

    /// A document `width` x `height` units big, shown at `bounds` (screen
    /// coordinates).
    pub fn units(bounds: Rect, width: f64, height: f64) -> Self {
        Self {
            origin: Point::new(bounds.x, bounds.y),
            scale_x: bounds.width / width,
            scale_y: bounds.height / height,
            width,
            height,
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        // Allow for rounding at the far edges (1.0000000001).
        let (ex, ey) = (self.width * 1e-9, self.height * 1e-9);
        x.is_finite()
            && y.is_finite()
            && x >= -ex
            && y >= -ey
            && x <= self.width + ex
            && y <= self.height + ey
    }

    fn to_screen(self, x: f64, y: f64) -> Point {
        Point::new(
            self.origin.x + x * self.scale_x,
            self.origin.y + y * self.scale_y,
        )
    }

    /// Back from the screen to this frame's coordinates.
    pub fn to_frame(self, p: Point) -> (f64, f64) {
        (
            (p.x - self.origin.x) / self.scale_x,
            (p.y - self.origin.y) / self.scale_y,
        )
    }
}

/// One stroke as asked for.
#[derive(Debug, Clone)]
pub enum Shape {
    /// Straight lines through `points` (or a smooth curve through them);
    /// `closed` goes back to the first.
    Points {
        points: Vec<(f64, f64)>,
        closed: bool,
        smooth: bool,
    },
    /// `(x(t), y(t))` for `t` from `t0` to `t1`; `steps` = that many
    /// straight pieces instead of a smooth curve.
    Curve {
        x: Expr,
        y: Expr,
        t0: f64,
        t1: f64,
        steps: Option<u32>,
    },
}

/// The gesture to perform: screen points, one list per press of the button.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub strokes: Vec<Vec<Point>>,
    /// Samples of curves that fell outside the frame (not drawn).
    pub skipped: usize,
    /// Distance the pointer travels with the button down, in screen units.
    pub length: f64,
}

impl Plan {
    pub fn points(&self) -> usize {
        self.strokes.iter().map(Vec::len).sum()
    }
}

/// Samples per curve before refining.
const CURVE_SAMPLES: usize = 512;
/// How many times a curve's piece may be halved to make it short enough.
const MAX_HALVINGS: u32 = 16;

fn dist(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// Turn `shapes` into strokes of screen points no more than `max_step`
/// apart, at most `limit` points in all.
pub fn plan(shapes: &[Shape], frame: &Frame, max_step: f64, limit: usize) -> Result<Plan, String> {
    let mut out = Builder {
        plan: Plan::default(),
        limit,
        max_step,
    };
    for (i, shape) in shapes.iter().enumerate() {
        let n = i + 1;
        match shape {
            Shape::Points {
                points,
                closed,
                smooth,
            } => {
                if points.is_empty() {
                    return Err(format!("stroke {n} has no points"));
                }
                for (k, &(x, y)) in points.iter().enumerate() {
                    if !frame.contains(x, y) {
                        return Err(format!(
                            "stroke {n}, point {} ({x}, {y}) is outside the drawing area (0..{} x 0..{})",
                            k + 1,
                            frame.width,
                            frame.height
                        ));
                    }
                }
                let mut verts = points.clone();
                if *smooth && verts.len() >= 3 {
                    verts = smooth_through(&verts, *closed, frame, max_step);
                } else if *closed && verts.len() >= 2 {
                    verts.push(verts[0]);
                }
                let screen: Vec<Point> = verts
                    .iter()
                    .map(|&(x, y)| {
                        // Only a smooth curve can bulge past the area: keep it in.
                        frame.to_screen(x.clamp(0.0, frame.width), y.clamp(0.0, frame.height))
                    })
                    .collect();
                out.add(screen, true)?;
            }
            Shape::Curve {
                x,
                y,
                t0,
                t1,
                steps,
            } => {
                if !(t0.is_finite() && t1.is_finite()) {
                    return Err(format!("stroke {n}: t must run between finite numbers"));
                }
                let before = out.plan.strokes.len();
                curve(x, y, *t0, *t1, *steps, frame, &mut out)?;
                if out.plan.strokes.len() == before {
                    return Err(format!(
                        "stroke {n}: the curve is never inside the drawing area (0..{} x 0..{}); check its numbers",
                        frame.width, frame.height
                    ));
                }
            }
        }
    }
    Ok(out.plan)
}

struct Builder {
    plan: Plan,
    limit: usize,
    max_step: f64,
}

impl Builder {
    /// Add a stroke through `verts`, filling in points so none are more than
    /// `max_step` apart. A single point is a dot when `keep_dot`.
    fn add(&mut self, verts: Vec<Point>, keep_dot: bool) -> Result<(), String> {
        let Some(&first) = verts.first() else {
            return Ok(());
        };
        let mut stroke = vec![first];
        for &p in &verts[1..] {
            let last = *stroke.last().expect("non-empty");
            let d = dist(last, p);
            if d < 0.25 {
                continue;
            }
            let pieces = (d / self.max_step).ceil().max(1.0) as usize;
            if self.plan.points() + stroke.len() + pieces > self.limit {
                return Err(self.too_many());
            }
            for k in 1..=pieces {
                let f = k as f64 / pieces as f64;
                stroke.push(Point::new(
                    last.x + (p.x - last.x) * f,
                    last.y + (p.y - last.y) * f,
                ));
            }
            self.plan.length += d;
        }
        if stroke.len() == 1 && !keep_dot {
            return Ok(());
        }
        if self.plan.points() + stroke.len() > self.limit {
            return Err(self.too_many());
        }
        self.plan.strokes.push(stroke);
        Ok(())
    }

    fn too_many(&self) -> String {
        format!(
            "the drawing needs more than {} pointer positions; draw it in parts",
            self.limit
        )
    }
}

/// Sample a curve: evenly in t, then halving each piece that is still
/// longer than `max_step` on screen. Where the curve leaves the frame or
/// is undefined (or jumps: a piece that stays long however small), the
/// button is lifted.
fn curve(
    x: &Expr,
    y: &Expr,
    t0: f64,
    t1: f64,
    steps: Option<u32>,
    frame: &Frame,
    out: &mut Builder,
) -> Result<(), String> {
    let at = |t: f64, skipped: &mut usize| -> Option<Point> {
        let (bx, by) = (x.eval(t), y.eval(t));
        if !(bx.is_finite() && by.is_finite()) {
            return None;
        }
        if !frame.contains(bx, by) {
            *skipped += 1;
            return None;
        }
        Some(frame.to_screen(bx, by))
    };
    let mut skipped = 0;
    let mut cur: Vec<Point> = Vec::new();
    if let Some(n) = steps {
        // A polygon: n straight pieces.
        let n = n.max(1) as usize;
        for i in 0..=n {
            let t = t0 + (t1 - t0) * i as f64 / n as f64;
            match at(t, &mut skipped) {
                Some(p) => cur.push(p),
                None => out.add(std::mem::take(&mut cur), false)?,
            }
        }
        out.add(cur, false)?;
        out.plan.skipped += skipped;
        return Ok(());
    }

    let mut prev: Option<(f64, Point)> = None;
    for i in 0..=CURVE_SAMPLES {
        let t = t0 + (t1 - t0) * i as f64 / CURVE_SAMPLES as f64;
        match at(t, &mut skipped) {
            None => {
                out.add(std::mem::take(&mut cur), false)?;
                prev = None;
            }
            Some(p) => {
                match prev {
                    None => cur.push(p),
                    Some((tp, pp)) => {
                        refine(&at, tp, pp, t, p, 0, &mut cur, out, &mut skipped)?;
                    }
                }
                prev = Some((t, p));
            }
        }
        if out.plan.points() + cur.len() > out.limit {
            return Err(out.too_many());
        }
    }
    out.add(cur, false)?;
    out.plan.skipped += skipped;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn refine(
    at: &dyn Fn(f64, &mut usize) -> Option<Point>,
    ta: f64,
    pa: Point,
    tb: f64,
    pb: Point,
    halvings: u32,
    cur: &mut Vec<Point>,
    out: &mut Builder,
    skipped: &mut usize,
) -> Result<(), String> {
    if dist(pa, pb) <= out.max_step {
        cur.push(pb);
        return Ok(());
    }
    if halvings >= MAX_HALVINGS || cur.len() > out.limit {
        // Still long over a tiny step of t: a jump, not a line to draw.
        out.add(std::mem::take(cur), false)?;
        cur.push(pb);
        return Ok(());
    }
    let tm = (ta + tb) / 2.0;
    match at(tm, skipped) {
        Some(pm) => {
            refine(at, ta, pa, tm, pm, halvings + 1, cur, out, skipped)?;
            refine(at, tm, pm, tb, pb, halvings + 1, cur, out, skipped)
        }
        None => {
            // It leaves the area (or is undefined) in between: lift.
            out.add(std::mem::take(cur), false)?;
            cur.push(pb);
            Ok(())
        }
    }
}

/// A smooth curve through `pts` (Catmull-Rom), as points about `max_step`
/// apart on screen.
fn smooth_through(
    pts: &[(f64, f64)],
    closed: bool,
    frame: &Frame,
    max_step: f64,
) -> Vec<(f64, f64)> {
    let n = pts.len();
    let get = |i: isize| -> (f64, f64) {
        if closed {
            pts[i.rem_euclid(n as isize) as usize]
        } else {
            pts[i.clamp(0, n as isize - 1) as usize]
        }
    };
    let segments = if closed { n } else { n - 1 };
    let mut out = vec![pts[0]];
    for s in 0..segments as isize {
        let (p0, p1, p2, p3) = (get(s - 1), get(s), get(s + 1), get(s + 2));
        let a = frame.to_screen(p1.0, p1.1);
        let b = frame.to_screen(p2.0, p2.1);
        let samples = ((dist(a, b) / max_step).ceil() as usize).clamp(1, 400);
        for k in 1..=samples {
            let u = k as f64 / samples as f64;
            let (u2, u3) = (u * u, u * u * u);
            let c = |a: f64, b: f64, c: f64, d: f64| {
                0.5 * ((2.0 * b)
                    + (-a + c) * u
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * u2
                    + (-a + 3.0 * b - 3.0 * c + d) * u3)
            };
            out.push((c(p0.0, p1.0, p2.0, p3.0), c(p0.1, p1.1, p2.1, p3.1)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn ev(src: &str, t: f64) -> f64 {
        Expr::parse(src).unwrap().eval(t)
    }

    #[test]
    fn expressions_follow_the_usual_rules() {
        assert_eq!(ev("1 + 2 * 3", 0.0), 7.0);
        assert_eq!(ev("(1 + 2) * 3", 0.0), 9.0);
        assert_eq!(ev("2 ^ 3 ^ 2", 0.0), 512.0);
        assert_eq!(ev("2 ** 3", 0.0), 8.0);
        assert_eq!(ev("-t^2", 3.0), -9.0);
        assert_eq!(ev("2^-1", 0.0), 0.5);
        assert_eq!(ev("10 - 4 - 3", 0.0), 3.0);
        assert_eq!(ev("7 % 4", 0.0), 3.0);
        assert_eq!(ev("1.5e2 + .5", 0.0), 150.5);
        assert!((ev("2pi", 0.0) - 2.0 * PI).abs() < 1e-12);
        assert_eq!(ev("3(t + 1)", 1.0), 6.0);
        assert_eq!(ev("t(1 - t)", 0.5), 0.25);
        assert!((ev("e", 0.0) - std::f64::consts::E).abs() < 1e-12);
        assert!((ev("sin(pi / 2) + cos(0)", 0.0) - 2.0).abs() < 1e-12);
        assert_eq!(ev("max(1, t, 3)", 5.0), 5.0);
        assert_eq!(ev("min(4, 2)", 0.0), 2.0);
        assert_eq!(ev("atan2(1, 1) * 4", 0.0), PI);
        assert_eq!(ev("sign(-3) + sign(0)", 0.0), -1.0);
        assert_eq!(ev("mod(-1, 3)", 0.0), 2.0);
        assert_eq!(ev("clamp(t, 0, 1)", 7.0), 1.0);
        assert_eq!(ev("SQRT(T)", 16.0), 4.0);
        assert!(ev("ln(t)", -1.0).is_nan());
        assert!(ev("1 / t", 0.0).is_infinite());
    }

    #[test]
    fn bad_expressions_are_explained() {
        let err = |s: &str| Expr::parse(s).unwrap_err();
        assert!(err("").contains("empty"));
        assert!(err("x + 1").contains("unknown name `x`"));
        assert!(err("sin t").contains("parentheses"));
        assert!(err("(1 + 2").contains("not closed"));
        assert!(err("1 + 2)").contains("`)`"));
        assert!(err("atan2(1)").contains("takes 2"));
        assert!(err("1 + ").contains("ends too early"));
        assert!(err("2 $ 3").contains("unexpected `$`"));
        assert!(err(&"(".repeat(200)).contains("nested too deeply"));
        assert!(err(&"1+".repeat(1500)).contains("longer than"));
    }

    fn screen() -> Frame {
        // An 800x600 screenshot of a window at (100, 50), at half scale.
        Frame::pixels(Rect::new(100.0, 50.0, 1600.0, 1200.0), 800, 600)
    }

    fn circle(steps: Option<u32>) -> Shape {
        Shape::Curve {
            x: Expr::parse("400 + 100*cos(t)").unwrap(),
            y: Expr::parse("300 + 100*sin(t)").unwrap(),
            t0: 0.0,
            t1: 2.0 * PI,
            steps,
        }
    }

    #[test]
    fn a_circle_is_one_closed_evenly_spaced_stroke() {
        let f = screen();
        let p = plan(&[circle(None)], &f, 3.0, 50_000).unwrap();
        assert_eq!(p.strokes.len(), 1);
        let s = &p.strokes[0];
        // Centre (400, 300) in screenshot pixels is (900, 650) on screen,
        // radius 100 px is 200 screen units.
        for p in s {
            let r = dist(*p, Point::new(900.0, 650.0));
            assert!((r - 200.0).abs() < 0.5, "{p:?} r={r}");
        }
        for w in s.windows(2) {
            assert!(dist(w[0], w[1]) <= 3.0 + 1e-9);
        }
        assert!(dist(s[0], *s.last().unwrap()) < 1e-6, "it closes");
        assert!((p.length - 2.0 * PI * 200.0).abs() < 2.0, "{}", p.length);
        assert_eq!(p.skipped, 0);
    }

    #[test]
    fn steps_make_a_polygon() {
        let f = screen();
        let p = plan(&[circle(Some(5))], &f, 1000.0, 50_000).unwrap();
        // Five pieces: six vertices, the last back on the first.
        assert_eq!(p.strokes[0].len(), 6);
        let star = Shape::Curve {
            x: Expr::parse("400 + 100*cos(t*4*pi/5)").unwrap(),
            y: Expr::parse("300 + 100*sin(t*4*pi/5)").unwrap(),
            t0: 0.0,
            t1: 5.0,
            steps: Some(5),
        };
        assert_eq!(
            plan(&[star], &f, 1000.0, 50_000).unwrap().strokes[0].len(),
            6
        );
    }

    #[test]
    fn curves_lift_where_they_leave_the_area_or_jump() {
        let f = screen();
        // tan goes off the top and bottom: one stroke per branch.
        let tan = Shape::Curve {
            x: Expr::parse("400 + 100*t").unwrap(),
            y: Expr::parse("300 - 50*tan(t)").unwrap(),
            t0: -3.0,
            t1: 3.0,
            steps: None,
        };
        let p = plan(&[tan], &f, 3.0, 50_000).unwrap();
        assert_eq!(p.strokes.len(), 3, "{:?}", p.strokes.len());
        assert!(p.skipped > 0);
        // A step function: no vertical line at the jump.
        let steps = Shape::Curve {
            x: Expr::parse("100 + 100*t").unwrap(),
            y: Expr::parse("300 - 50*floor(t)").unwrap(),
            t0: 0.0,
            t1: 2.5,
            steps: None,
        };
        let p = plan(&[steps], &f, 3.0, 50_000).unwrap();
        assert_eq!(p.strokes.len(), 3);
        // Wholly outside: an error, not a silent nothing.
        let away = Shape::Curve {
            x: Expr::parse("5000 + t").unwrap(),
            y: Expr::parse("t").unwrap(),
            t0: 0.0,
            t1: 1.0,
            steps: None,
        };
        assert!(
            plan(&[away], &f, 3.0, 50_000)
                .unwrap_err()
                .contains("never inside")
        );
    }

    #[test]
    fn points_draw_straight_lines_and_must_be_inside() {
        let f = screen();
        let square = Shape::Points {
            points: vec![
                (100.0, 100.0),
                (200.0, 100.0),
                (200.0, 200.0),
                (100.0, 200.0),
            ],
            closed: true,
            smooth: false,
        };
        let p = plan(&[square], &f, 4.0, 50_000).unwrap();
        let s = &p.strokes[0];
        assert_eq!(s[0], Point::new(300.0, 250.0));
        assert_eq!(*s.last().unwrap(), s[0]);
        assert!((p.length - 800.0).abs() < 1e-6);
        for w in s.windows(2) {
            assert!(dist(w[0], w[1]) <= 4.0 + 1e-9);
        }
        // A single point is a dot.
        let dot = Shape::Points {
            points: vec![(10.0, 10.0)],
            closed: false,
            smooth: false,
        };
        assert_eq!(plan(&[dot], &f, 4.0, 50_000).unwrap().strokes[0].len(), 1);
        let outside = Shape::Points {
            points: vec![(10.0, 10.0), (900.0, 10.0)],
            closed: false,
            smooth: false,
        };
        let err = plan(&[outside], &f, 4.0, 50_000).unwrap_err();
        assert!(err.contains("point 2 (900, 10) is outside"), "{err}");
    }

    #[test]
    fn smooth_strokes_pass_through_their_points() {
        let f = screen();
        let pts = vec![
            (100.0, 300.0),
            (200.0, 200.0),
            (300.0, 300.0),
            (400.0, 200.0),
        ];
        let wave = Shape::Points {
            points: pts.clone(),
            closed: false,
            smooth: true,
        };
        let p = plan(&[wave], &f, 3.0, 50_000).unwrap();
        let s = &p.strokes[0];
        for (x, y) in pts {
            let target = f.to_screen(x, y);
            assert!(
                s.iter().any(|q| dist(*q, target) < 1e-6),
                "misses ({x}, {y})"
            );
        }
    }

    #[test]
    fn element_fractions_and_limits() {
        let f = Frame::fractions(Rect::new(200.0, 100.0, 400.0, 200.0));
        let diag = Shape::Points {
            points: vec![(0.0, 0.0), (1.0, 1.0)],
            closed: false,
            smooth: false,
        };
        let p = plan(&[diag], &f, 5.0, 50_000).unwrap();
        assert_eq!(p.strokes[0][0], Point::new(200.0, 100.0));
        assert_eq!(*p.strokes[0].last().unwrap(), Point::new(600.0, 300.0));
        assert_eq!(f.to_frame(Point::new(400.0, 200.0)), (0.5, 0.5));
        let err = plan(&[circle(None)], &screen(), 3.0, 50).unwrap_err();
        assert!(err.contains("more than 50"), "{err}");
    }
}
