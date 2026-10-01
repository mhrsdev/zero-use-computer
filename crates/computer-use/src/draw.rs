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
    /// Another name for `t` (`x` in a plot of y = f(x)).
    alias: Option<String>,
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
        if name == "t" || self.alias.as_deref() == Some(name) {
            return Ok(Node::T);
        }
        if let Some((_, v)) = CONSTANTS.iter().find(|(n, _)| *n == name) {
            return Ok(Node::Num(*v));
        }
        let Some(i) = FUNCS.iter().position(|(n, ..)| *n == name) else {
            let funcs: Vec<&str> = FUNCS.iter().map(|(n, ..)| *n).collect();
            let vars = match &self.alias {
                Some(a) => format!("t or {a}"),
                None => "t".into(),
            };
            return Err(format!(
                "unknown name `{name}`: use {vars}, pi, tau, e or a function ({})",
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
        Self::parse_in(src, None)
    }

    /// Parse `src`, an expression in `t` or in `alias` (the same thing).
    pub fn parse_in(src: &str, alias: Option<&str>) -> Result<Expr, String> {
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
            alias: alias.map(str::to_lowercase),
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
/// screenshot (pixels), an element's box (fractions, 0 to 1), a document
/// (its own units, y down) or a math range (y up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    origin: Point,
    scale_x: f64,
    scale_y: f64,
    /// Coordinates run from `x0` to `x1` and from `y0` to `y1`.
    pub x0: f64,
    pub x1: f64,
    pub y0: f64,
    pub y1: f64,
}

impl Frame {
    /// Screenshot pixels: an image of `width` x `height` showing `bounds`.
    pub fn pixels(bounds: Rect, width: u32, height: u32) -> Self {
        Self::units(bounds, f64::from(width.max(1)), f64::from(height.max(1)))
    }

    /// Fractions of `bounds` (screen coordinates).
    pub fn fractions(bounds: Rect) -> Self {
        Self::units(bounds, 1.0, 1.0)
    }

    /// A document `width` x `height` units big, shown at `bounds` (screen
    /// coordinates), y down.
    pub fn units(bounds: Rect, width: f64, height: f64) -> Self {
        Self {
            origin: Point::new(bounds.x, bounds.y),
            scale_x: bounds.width / width,
            scale_y: bounds.height / height,
            x0: 0.0,
            x1: width,
            y0: 0.0,
            y1: height,
        }
    }

    /// Math coordinates across `bounds`: x from `x0` (left) to `x1`
    /// (right), y from `y0` (bottom) to `y1` (top).
    pub fn range(bounds: Rect, x0: f64, x1: f64, y0: f64, y1: f64) -> Self {
        let scale_x = bounds.width / (x1 - x0);
        let scale_y = -bounds.height / (y1 - y0);
        Self {
            origin: Point::new(
                bounds.x - x0 * scale_x,
                bounds.y + bounds.height - y0 * scale_y,
            ),
            scale_x,
            scale_y,
            x0,
            x1,
            y0,
            y1,
        }
    }

    /// Whether y grows upwards (a math range).
    pub fn y_up(&self) -> bool {
        self.scale_y < 0.0
    }

    /// Screen units per frame unit on each axis (y negative when y is up).
    pub fn scale(&self) -> (f64, f64) {
        (self.scale_x, self.scale_y)
    }

    /// The area in screen coordinates.
    pub fn screen_rect(&self) -> Rect {
        let (a, b) = (
            self.to_screen(self.x0, self.y0),
            self.to_screen(self.x1, self.y1),
        );
        Rect::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (b.x - a.x).abs(),
            (b.y - a.y).abs(),
        )
    }

    /// "0..800 x 0..600".
    pub fn describe(&self) -> String {
        format!("{}..{} x {}..{}", self.x0, self.x1, self.y0, self.y1)
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        // Allow for rounding at the far edges (1.0000000001).
        let ex = (self.x1 - self.x0).abs() * 1e-9;
        let ey = (self.y1 - self.y0).abs() * 1e-9;
        x.is_finite()
            && y.is_finite()
            && x >= self.x0 - ex
            && x <= self.x1 + ex
            && y >= self.y0 - ey
            && y <= self.y1 + ey
    }

    fn clamp(&self, x: f64, y: f64) -> (f64, f64) {
        (x.clamp(self.x0, self.x1), y.clamp(self.y0, self.y1))
    }

    pub fn to_screen(self, x: f64, y: f64) -> Point {
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

    /// A turn by `degrees` about `about`, in this frame's coordinates.
    /// Positive turns clockwise on screen, whatever way y grows, and a
    /// circle stays a circle when the two axes have different scales.
    pub fn rotation(self, about: (f64, f64), degrees: f64) -> Affine {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let k = self.scale_y / self.scale_x;
        let (a, b, c, d) = (cos, -k * sin, sin / k, cos);
        let (px, py) = about;
        Affine {
            a,
            b,
            c,
            d,
            e: px - a * px - b * py,
            f: py - c * px - d * py,
        }
    }
}

/// An affine map of frame coordinates:
/// `x' = a x + b y + e`, `y' = c x + d y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(dx: f64, dy: f64) -> Affine {
        Affine {
            e: dx,
            f: dy,
            ..Affine::IDENTITY
        }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.b * y + self.e,
            self.c * x + self.d * y + self.f,
        )
    }

    /// `self` first, then `next`.
    pub fn then(self, next: Affine) -> Affine {
        let n = next;
        Affine {
            a: n.a * self.a + n.b * self.c,
            b: n.a * self.b + n.b * self.d,
            c: n.c * self.a + n.d * self.c,
            d: n.c * self.b + n.d * self.d,
            e: n.a * self.e + n.b * self.f + n.e,
            f: n.c * self.e + n.d * self.f + n.f,
        }
    }
}

/// One stroke as asked for, with the transform (turn, offset) to apply
/// to its coordinates.
#[derive(Debug, Clone)]
pub enum Shape {
    /// Straight lines through `points` (or a smooth curve through them);
    /// `closed` goes back to the first.
    Points {
        points: Vec<(f64, f64)>,
        closed: bool,
        smooth: bool,
        transform: Affine,
    },
    /// `(x(t), y(t))` for `t` from `t0` to `t1`; `steps` = that many
    /// straight pieces instead of a smooth curve.
    Curve {
        x: Expr,
        y: Expr,
        t0: f64,
        t1: f64,
        steps: Option<u32>,
        transform: Affine,
    },
}

impl Shape {
    pub fn points(points: Vec<(f64, f64)>, closed: bool) -> Shape {
        Shape::Points {
            points,
            closed,
            smooth: false,
            transform: Affine::IDENTITY,
        }
    }

    /// The middle of its bounding box, before its transform.
    pub fn center(&self) -> Option<(f64, f64)> {
        let pts: Vec<(f64, f64)> = match self {
            Shape::Points { points, .. } => points.clone(),
            Shape::Curve { x, y, t0, t1, .. } => (0..=128)
                .map(|i| {
                    let t = t0 + (t1 - t0) * f64::from(i) / 128.0;
                    (x.eval(t), y.eval(t))
                })
                .filter(|(a, b)| a.is_finite() && b.is_finite())
                .collect(),
        };
        let first = *pts.first()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
        for (x, y) in pts {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0))
    }

    /// The same shape, moved by `t` after its own transform.
    pub fn then(&self, t: Affine) -> Shape {
        let mut s = self.clone();
        match &mut s {
            Shape::Points { transform, .. } | Shape::Curve { transform, .. } => {
                *transform = transform.then(t);
            }
        }
        s
    }
}

/// A rectangle's outline from the corner (x, y), with corners rounded by
/// `radius` (0 = sharp).
pub fn rect(x: f64, y: f64, w: f64, h: f64, radius: f64) -> Vec<(f64, f64)> {
    let r = radius.max(0.0).min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    }
    let mut out = Vec::new();
    let corners = [
        (x + w - r, y + r, -90.0),
        (x + w - r, y + h - r, 0.0),
        (x + r, y + h - r, 90.0),
        (x + r, y + r, 180.0),
    ];
    for (cx, cy, start) in corners {
        for k in 0..=12 {
            let a = (start + 90.0 * f64::from(k) / 12.0_f64).to_radians();
            out.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    out
}

/// The angle that points up on screen, in frame coordinates.
fn up(y_up: bool) -> f64 {
    if y_up {
        std::f64::consts::FRAC_PI_2
    } else {
        -std::f64::consts::FRAC_PI_2
    }
}

/// A regular polygon with `n` corners, the first pointing up.
pub fn polygon(cx: f64, cy: f64, r: f64, n: u32, y_up: bool) -> Vec<(f64, f64)> {
    (0..n)
        .map(|k| {
            let a = up(y_up) + std::f64::consts::TAU * f64::from(k) / f64::from(n);
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// A star with `n` points (outer radius `outer`, inner `inner`), the first
/// pointing up.
pub fn star(cx: f64, cy: f64, outer: f64, inner: f64, n: u32, y_up: bool) -> Vec<(f64, f64)> {
    (0..2 * n)
        .map(|k| {
            let a = up(y_up) + std::f64::consts::PI * f64::from(k) / f64::from(n);
            let r = if k % 2 == 0 { outer } else { inner };
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Points along cubic Bézier segments: `pts` is the start, then two
/// control points and an end point per segment (3n + 1 in all).
pub fn bezier(pts: &[(f64, f64)]) -> Result<Vec<(f64, f64)>, String> {
    if pts.len() < 4 || !(pts.len() - 1).is_multiple_of(3) {
        return Err(
            "bezier needs a start point, then two control points and an end point per segment (4, 7, 10… points)"
                .into(),
        );
    }
    let mut out = vec![pts[0]];
    for seg in pts[1..].chunks(3) {
        let p0 = *out.last().expect("non-empty");
        let (c1, c2, p1) = (seg[0], seg[1], seg[2]);
        for k in 1..=48 {
            let t = f64::from(k) / 48.0;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            out.push((
                w[0] * p0.0 + w[1] * c1.0 + w[2] * c2.0 + w[3] * p1.0,
                w[0] * p0.1 + w[1] * c1.1 + w[2] * c2.1 + w[3] * p1.1,
            ));
        }
    }
    Ok(out)
}

/// Axes through 0 (or the nearest edge) with ticks every `tick_x` and
/// `tick_y` (0 = no ticks) about `tick_len` screen units long.
pub fn axes(frame: &Frame, tick_x: f64, tick_y: f64, tick_len: f64) -> Result<Vec<Shape>, String> {
    let (sx, sy) = frame.scale();
    let ya = 0f64.clamp(frame.y0, frame.y1);
    let xa = 0f64.clamp(frame.x0, frame.x1);
    let mut out = vec![
        Shape::points(vec![(frame.x0, ya), (frame.x1, ya)], false),
        Shape::points(vec![(xa, frame.y0), (xa, frame.y1)], false),
    ];
    let ticks = |lo: f64, hi: f64, step: f64| -> Result<Vec<f64>, String> {
        if step <= 0.0 {
            return Ok(Vec::new());
        }
        if !step.is_finite() || (hi - lo) / step > 200.0 {
            return Err("too many ticks: use a larger tick step".into());
        }
        let mut v = Vec::new();
        let mut k = (lo / step).ceil();
        while k * step <= hi + step * 1e-9 {
            v.push(k * step);
            k += 1.0;
        }
        Ok(v)
    };
    let half_y = tick_len / 2.0 / sy.abs();
    for v in ticks(frame.x0, frame.x1, tick_x)? {
        let (a, b) = ((ya - half_y).max(frame.y0), (ya + half_y).min(frame.y1));
        out.push(Shape::points(vec![(v, a), (v, b)], false));
    }
    let half_x = tick_len / 2.0 / sx.abs();
    for v in ticks(frame.y0, frame.y1, tick_y)? {
        let (a, b) = ((xa - half_x).max(frame.x0), (xa + half_x).min(frame.x1));
        out.push(Shape::points(vec![(a, v), (b, v)], false));
    }
    Ok(out)
}

/// The gesture to perform: screen points, one list per press of the button.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub strokes: Vec<Vec<Point>>,
    /// For each stroke, the index of the shape it came from.
    pub shape: Vec<usize>,
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

/// Where a click fills each closed stroke (one that ends where it starts)
/// with a bucket or magic wand, for at most `limit` of them: `(stroke
/// index, point)`. The point is inside the stroke, clear of its edges, and
/// outside the closed strokes drawn within it, so a ring's point lies
/// between its two circles. Open strokes have none.
pub fn fill_points(strokes: &[Vec<Point>], limit: usize) -> Vec<(usize, Point)> {
    fill_targets(strokes, limit, |_| true)
        .into_iter()
        .map(|t| (t.stroke, t.point))
        .collect()
}

/// A closed stroke to fill: where to click, and the closed strokes drawn
/// within it (a fill stops at them).
#[derive(Debug, Clone, PartialEq)]
pub struct FillTarget {
    pub stroke: usize,
    pub point: Point,
    pub holes: Vec<usize>,
}

/// [`fill_points`] for the strokes `wanted` says, with their holes.
pub fn fill_targets(
    strokes: &[Vec<Point>],
    limit: usize,
    wanted: impl Fn(usize) -> bool,
) -> Vec<FillTarget> {
    let closed: Vec<(usize, &[Point], Bounds, f64)> = strokes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.len() >= 4 && dist(s[0], s[s.len() - 1]) <= 1.0)
        .map(|(i, s)| (i, s.as_slice(), Bounds::of(s), polygon_area(s).abs()))
        .filter(|&(.., area)| area >= 4.0) // a real inside
        .collect();
    let mut out = Vec::new();
    for &(i, s, b, area) in &closed {
        if out.len() >= limit {
            break;
        }
        if !wanted(i) {
            continue;
        }
        // The strokes within this one: a fill stops at them. (Two of the
        // same box: the smaller one is within.)
        let inner: Vec<usize> = closed
            .iter()
            .filter(|&&(j, _, c, other)| {
                j != i && b.holds(&c) && (!c.holds(&b) || other < area * 0.98)
            })
            .map(|&(j, ..)| j)
            .collect();
        let holes: Vec<(&[Point], Bounds)> = closed
            .iter()
            .filter(|c| inner.contains(&c.0))
            .map(|&(_, t, c, _)| (t, c))
            .collect();
        if let Some(point) = fill_point((s, b), &holes) {
            out.push(FillTarget {
                stroke: i,
                point,
                holes: inner,
            });
        }
    }
    out
}

/// A point inside `poly` and outside every hole, as far from the edges as
/// a few tries find: the centre when it is clear enough, else the middle of
/// the best span along one of 16 lines across.
fn fill_point(poly: (&[Point], Bounds), holes: &[(&[Point], Bounds)]) -> Option<Point> {
    let all = || std::iter::once(poly).chain(holes.iter().copied());
    // Only shapes whose box reaches the line can cross it.
    let across = |y: f64| {
        all()
            .filter(move |(_, b)| b.y0 <= y && y <= b.y1)
            .flat_map(move |(s, _)| crossings(s, y, false))
    };
    let down = |x: f64| {
        all()
            .filter(move |(_, b)| b.x0 <= x && x <= b.x1)
            .flat_map(move |(s, _)| crossings(s, x, true))
    };
    let within = |(s, b): (&[Point], Bounds), p: Point| {
        b.x0 <= p.x && p.x <= b.x1 && b.y0 <= p.y && p.y <= b.y1 && inside(s, p)
    };
    let free = |p: Point| within(poly, p) && !holes.iter().any(|&h| within(h, p));
    // Distance to the nearest edge straight left, right, up or down.
    let clearance = |p: Point| -> f64 {
        across(p.y)
            .map(|x| (x - p.x).abs())
            .chain(down(p.x).map(|y| (y - p.y).abs()))
            .fold(f64::MAX, f64::min)
    };
    // (point, clearance, score): equal clearances go to the line nearer
    // the middle.
    let b = poly.1;
    let mid = (b.y0 + b.y1) / 2.0;
    let mut best: Option<(Point, f64, f64)> = None;
    for k in 0..16 {
        let y = b.y0 + (b.y1 - b.y0) * (k as f64 + 0.5) / 16.0;
        let mut xs: Vec<f64> = across(y).collect();
        xs.sort_by(f64::total_cmp);
        for w in xs.windows(2) {
            let p = Point::new((w[0] + w[1]) / 2.0, y);
            if w[1] - w[0] > 1.0 && free(p) {
                let c = clearance(p);
                let score = c - 1e-3 * (y - mid).abs();
                if best.is_none_or(|(.., top)| score > top) {
                    best = Some((p, c, score));
                }
            }
        }
    }
    match (centroid(poly.0).filter(|&c| free(c)), best) {
        (Some(c), Some((_, top, _))) if clearance(c) >= top / 2.0 => Some(c),
        (Some(c), None) => Some(c),
        (_, best) => best.map(|(p, ..)| p),
    }
}

/// Strokes that paint the inside of the closed `outline` with a round
/// brush `width` across (screen units): a pass round the inside of the
/// edge, then rows across, joined into one stroke wherever the join stays
/// inside. The paint goes up to `bleed` past the edge (0: exactly to it),
/// so shapes painted side by side leave no gap between them.
pub fn brush_fill(outline: &[Point], width: f64, bleed: f64) -> Vec<Vec<Point>> {
    let mut out = Vec::new();
    if outline.len() < 3 || !(width.is_finite() && width > 0.0) {
        return out;
    }
    let r = width / 2.0;
    let inset = (r - bleed.max(0.0)).max(0.0);
    let b = Bounds::of(outline);
    // The edge pass.
    if inset < 0.5 {
        out.push(outline.to_vec());
    } else {
        out.extend(inner_edge(outline, inset));
    }
    // Rows: no more than 0.6 of a brush apart, so they overlap.
    let gap = width * 0.6;
    let edge = inset.max(gap / 2.0).min(r);
    let (top, bottom) = (b.y0 + edge, b.y1 - edge);
    let rows: Vec<f64> = if bottom <= top {
        vec![(b.y0 + b.y1) / 2.0]
    } else {
        let n = ((bottom - top) / gap).ceil().max(1.0) as usize;
        (0..=n)
            .map(|k| top + (bottom - top) * k as f64 / n as f64)
            .collect()
    };
    // Open chains: (stroke, the row's span as cut, ends going right).
    let mut open: Vec<(Vec<Point>, (f64, f64), bool)> = Vec::new();
    let edges: Vec<(Point, Point)> = outline
        .iter()
        .zip(outline.iter().cycle().skip(1))
        .map(|(p, q)| (*p, *q))
        .collect();
    for y in rows {
        let mut xs = crossings(outline, y, false);
        xs.sort_by(f64::total_cmp);
        // The edges near this row: the only ones the brush can touch.
        let near: Vec<(Point, Point)> = edges
            .iter()
            .filter(|(p, q)| p.y.min(q.y) - inset <= y && y <= p.y.max(q.y) + inset)
            .copied()
            .collect();
        let clear = |x: f64| {
            let c = Point::new(x, y);
            near.iter()
                .map(|&(p, q)| segment_distance(c, p, q))
                .fold(f64::MAX, f64::min)
                >= inset - 0.25
        };
        // Where the brush runs on this row, with the stretch of the row
        // each run stands for (to join it to the row above).
        let mut runs: Vec<((f64, f64), (f64, f64))> = Vec::new();
        for pair in xs.chunks_exact(2) {
            let (left, right) = (pair[0], pair[1]);
            if right <= left {
                continue;
            }
            let (a, z) = (left + inset, right - inset);
            let m = (left + right) / 2.0;
            if inset <= 0.0 {
                runs.push(((a.min(m), z.max(m)), (left, right)));
                continue;
            }
            // Narrower than the brush: exact shapes only where the brush
            // fits; shapes that may bleed get a line down the middle.
            let lenient = bleed > 0.0;
            if a > z {
                if lenient || clear(m) {
                    runs.push(((m, m), (left, right)));
                }
                continue;
            }
            // Split where the brush would touch an edge (a notch above or
            // below the row, a slanting edge at the ends).
            let n = ((z - a) / (inset / 3.0).max(0.5)).ceil().max(1.0) as usize;
            let mut from: Option<f64> = None;
            let mut last = a;
            for k in 0..=n {
                let x = a + (z - a) * k as f64 / n as f64;
                if clear(x) {
                    from.get_or_insert(x);
                    last = x;
                } else if let Some(f) = from.take() {
                    runs.push(((f, last), (f - inset, last + inset)));
                }
            }
            if let Some(f) = from {
                runs.push(((f, last), (f - inset, last + inset)));
            }
            if lenient && !runs.iter().any(|r| r.1.0 < right && left < r.1.1) {
                runs.push(((m, m), (left, right)));
            }
        }
        let overlaps = |p: (f64, f64), q: (f64, f64)| p.0 < q.1 && q.0 < p.1;
        let mut next: Vec<(Vec<Point>, (f64, f64), bool)> = Vec::new();
        let mut taken = vec![false; open.len()];
        for &((a, z), span) in &runs {
            // Join the chain above when it is the only one touching this
            // run and touches no other, and the join stays clear.
            let above: Vec<usize> = (0..open.len())
                .filter(|&i| !taken[i] && overlaps(open[i].1, span))
                .collect();
            let joined = match above.as_slice() {
                [i] if runs.iter().filter(|r| overlaps(open[*i].1, r.1)).count() == 1 => {
                    let i = *i;
                    let right = !open[i].2;
                    let (from, to) = if right { (a, z) } else { (z, a) };
                    let last = *open[i].0.last().expect("chains are never empty");
                    let start = Point::new(from, y);
                    let mid = Point::new((last.x + start.x) / 2.0, (last.y + start.y) / 2.0);
                    if stays_inside(outline, last, start)
                        && edge_distance(outline, mid) >= inset - 0.25
                    {
                        taken[i] = true;
                        let mut stroke = std::mem::take(&mut open[i].0);
                        stroke.push(start);
                        stroke.push(Point::new(to, y));
                        next.push((stroke, span, right));
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if !joined {
                next.push((vec![Point::new(a, y), Point::new(z, y)], span, true));
            }
        }
        for (i, chain) in open.into_iter().enumerate() {
            if !taken[i] {
                out.push(chain.0);
            }
        }
        open = next;
    }
    out.extend(open.into_iter().map(|c| c.0));
    out.retain(|s| !s.is_empty());
    out
}

/// The closed `outline` moved `d` inwards: where a round brush `2d` wide
/// must run to paint up to the edge and no further. Pieces that would
/// leave the shape are dropped.
fn inner_edge(outline: &[Point], d: f64) -> Vec<Vec<Point>> {
    // Distinct points, without the closing repeat.
    let mut pts: Vec<Point> = Vec::with_capacity(outline.len());
    for &p in outline {
        if pts.last().is_none_or(|q: &Point| dist(*q, p) > 1e-6) {
            pts.push(p);
        }
    }
    if pts.len() > 1 && dist(pts[0], pts[pts.len() - 1]) <= 1e-6 {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    // With y down, a positive area means the inside is to the left of
    // each edge: (-dy, dx).
    let sign = if polygon_area(&pts) > 0.0 { 1.0 } else { -1.0 };
    let inward = |p: Point, q: Point| -> Option<(f64, f64)> {
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        let len = dx.hypot(dy);
        (len > 1e-9).then(|| (-dy / len * sign, dx / len * sign))
    };
    let mut runs: Vec<Vec<Point>> = vec![Vec::new()];
    let mut all = true;
    for i in 0..n {
        let (prev, p, next) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let (Some(a), Some(c)) = (inward(prev, p), inward(p, next)) else {
            continue;
        };
        let (mx, my) = (a.0 + c.0, a.1 + c.1);
        let len = mx.hypot(my);
        let q = if len < 1e-9 {
            None
        } else {
            let (mx, my) = (mx / len, my / len);
            // Further out at corners, but never more than about 3 d.
            let cos = (mx * a.0 + my * a.1).max(0.35);
            let k = d / cos;
            Some(Point::new(p.x + mx * k, p.y + my * k))
        };
        // Inside, and at least about `d` from every edge (a point near a
        // corner, moved off one edge, can be too near the other).
        match q.filter(|&q| inside(outline, q) && edge_distance(outline, q) >= d * 0.9) {
            Some(q) => runs.last_mut().expect("never empty").push(q),
            None => {
                all = false;
                runs.push(Vec::new());
            }
        }
    }
    if all
        && let Some(run) = runs.first_mut()
        && let Some(&first) = run.first()
    {
        run.push(first);
    }
    // A run that ends where the first begins is one piece.
    if !all && runs.len() > 1 {
        let first = runs.remove(0);
        if let Some(last) = runs.last_mut() {
            last.extend(first);
        }
    }
    runs.retain(|r| r.len() >= 2);
    runs
}

/// About how wide a closed stroke is at its narrowest kind of place: 4 x
/// area / perimeter (a circle's diameter; a long strip comes out wider
/// than it is).
pub fn shape_width(stroke: &[Point]) -> f64 {
    let perimeter: f64 = stroke.windows(2).map(|w| dist(w[0], w[1])).sum();
    if perimeter <= 0.0 {
        return 0.0;
    }
    4.0 * polygon_area(stroke).abs() / perimeter
}

/// How far `p` is from the nearest edge of the closed `poly`.
fn edge_distance(poly: &[Point], p: Point) -> f64 {
    poly.iter()
        .zip(poly.iter().cycle().skip(1))
        .map(|(&a, &b)| segment_distance(p, a, b))
        .fold(f64::MAX, f64::min)
}

/// How far `p` is from the segment `a`-`b`.
fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-12 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

/// Whether the segment `a`-`b` stays inside the closed `poly` (crosses
/// none of its edges and its middle is inside).
fn stays_inside(poly: &[Point], a: Point, b: Point) -> bool {
    let cross =
        |o: Point, p: Point, q: Point| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    let hits = poly
        .iter()
        .zip(poly.iter().cycle().skip(1))
        .any(|(&p, &q)| {
            let (d1, d2) = (cross(p, q, a), cross(p, q, b));
            let (d3, d4) = (cross(a, b, p), cross(a, b, q));
            d1 * d2 < 0.0 && d3 * d4 < 0.0
        });
    !hits && inside(poly, Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0))
}

/// Where the edges of the closed `poly` cross the line y = `at` (x = `at`
/// when `vertical`): the x (or y) of each crossing.
pub(crate) fn crossings(poly: &[Point], at: f64, vertical: bool) -> Vec<f64> {
    let flip = |p: &Point| if vertical { (p.y, p.x) } else { (p.x, p.y) };
    poly.iter()
        .zip(poly.iter().cycle().skip(1))
        .filter_map(|(p, q)| {
            let ((px, py), (qx, qy)) = (flip(p), flip(q));
            ((py <= at) != (qy <= at)).then(|| px + (at - py) / (qy - py) * (qx - px))
        })
        .collect()
}

/// Whether `p` is inside the closed `poly` (even-odd).
fn inside(poly: &[Point], p: Point) -> bool {
    crossings(poly, p.y, false)
        .into_iter()
        .filter(|&x| x > p.x)
        .count()
        % 2
        == 1
}

/// Twice the signed area of the closed `poly`, with its centre of mass.
fn polygon_moments(poly: &[Point]) -> (f64, f64, f64) {
    poly.iter()
        .zip(poly.iter().cycle().skip(1))
        .fold((0.0, 0.0, 0.0), |(a, cx, cy), (p, q)| {
            let cross = p.x * q.y - q.x * p.y;
            (
                a + cross,
                cx + (p.x + q.x) * cross,
                cy + (p.y + q.y) * cross,
            )
        })
}

fn polygon_area(poly: &[Point]) -> f64 {
    polygon_moments(poly).0 / 2.0
}

fn centroid(poly: &[Point]) -> Option<Point> {
    let (a, cx, cy) = polygon_moments(poly);
    (a.abs() > 1e-9).then(|| Point::new(cx / (3.0 * a), cy / (3.0 * a)))
}

/// The box around some points.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Bounds {
    fn of(points: &[Point]) -> Bounds {
        points.iter().fold(
            Bounds {
                x0: f64::MAX,
                y0: f64::MAX,
                x1: f64::MIN,
                y1: f64::MIN,
            },
            |b, p| Bounds {
                x0: b.x0.min(p.x),
                y0: b.y0.min(p.y),
                x1: b.x1.max(p.x),
                y1: b.y1.max(p.y),
            },
        )
    }

    /// Whether `other` fits in this box (give or take a pixel).
    fn holds(&self, other: &Bounds) -> bool {
        other.x0 >= self.x0 - 1.0
            && other.y0 >= self.y0 - 1.0
            && other.x1 <= self.x1 + 1.0
            && other.y1 <= self.y1 + 1.0
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
    let labelled: Vec<(String, Shape)> = shapes
        .iter()
        .enumerate()
        .map(|(i, s)| (format!("stroke {}", i + 1), s.clone()))
        .collect();
    plan_labelled(&labelled, frame, max_step, limit)
}

/// [`plan`], with the name each shape goes by in messages ("stroke 2,
/// copy 3").
pub fn plan_labelled(
    shapes: &[(String, Shape)],
    frame: &Frame,
    max_step: f64,
    limit: usize,
) -> Result<Plan, String> {
    let mut out = Builder {
        plan: Plan::default(),
        limit,
        max_step,
    };
    for (index, (n, shape)) in shapes.iter().enumerate() {
        let before = out.plan.strokes.len();
        match shape {
            Shape::Points {
                points,
                closed,
                smooth,
                transform,
            } => {
                if points.is_empty() {
                    return Err(format!("{n} has no points"));
                }
                let mut verts: Vec<(f64, f64)> =
                    points.iter().map(|&(x, y)| transform.apply(x, y)).collect();
                for (k, &(x, y)) in verts.iter().enumerate() {
                    if !frame.contains(x, y) {
                        return Err(format!(
                            "{n}, point {} ({}, {}) is outside the drawing area ({})",
                            k + 1,
                            round4(x),
                            round4(y),
                            frame.describe()
                        ));
                    }
                }
                if *smooth && verts.len() >= 3 {
                    verts = smooth_through(&verts, *closed, frame, max_step);
                } else if *closed && verts.len() >= 2 {
                    verts.push(verts[0]);
                }
                let screen: Vec<Point> = verts
                    .iter()
                    .map(|&(x, y)| {
                        // Only a smooth curve can bulge past the area: keep it in.
                        let (x, y) = frame.clamp(x, y);
                        frame.to_screen(x, y)
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
                transform,
            } => {
                if !(t0.is_finite() && t1.is_finite()) {
                    return Err(format!("{n}: t must run between finite numbers"));
                }
                curve(x, y, *t0, *t1, *steps, transform, frame, &mut out)?;
                if out.plan.strokes.len() == before {
                    return Err(format!(
                        "{n}: the curve is never inside the drawing area ({}); check its numbers",
                        frame.describe()
                    ));
                }
            }
        }
        let made = out.plan.strokes.len();
        out.plan.shape.resize(made, index);
    }
    Ok(out.plan)
}

/// `plan` with the strokes of some shapes painted solid instead: `fills`
/// holds, per shape, the brush width in screen units and how far its paint
/// may go past the edge (see [`brush_fill`]), or `None` to keep the outline.
pub fn fill_plan(
    plan: Plan,
    fills: &[Option<(f64, f64)>],
    names: &[String],
    max_step: f64,
    limit: usize,
) -> Result<Plan, String> {
    if fills.iter().all(Option::is_none) {
        return Ok(plan);
    }
    let mut out = Builder {
        plan: Plan {
            skipped: plan.skipped,
            ..Plan::default()
        },
        limit,
        max_step,
    };
    for (stroke, &shape) in plan.strokes.into_iter().zip(&plan.shape) {
        let before = out.plan.strokes.len();
        match fills.get(shape).copied().flatten() {
            Some((width, bleed)) => {
                let closed = stroke.len() >= 4 && dist(stroke[0], stroke[stroke.len() - 1]) <= 1.0;
                if !closed {
                    let name = names.get(shape).map_or("a stroke", String::as_str);
                    return Err(format!(
                        "{name}: fill needs a closed shape that is all inside the drawing area"
                    ));
                }
                // Straight rows need fewer pointer positions.
                out.max_step = max_step.max(width / 2.0);
                for s in brush_fill(&stroke, width, bleed) {
                    out.add(s, true)?;
                }
                out.max_step = max_step;
            }
            None => out.add(stroke, true)?,
        }
        let made = out.plan.strokes.len();
        out.plan.shape.resize(made.max(before), shape);
    }
    Ok(out.plan)
}

/// A coordinate for a message: at most four decimals.
fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
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
#[allow(clippy::too_many_arguments)]
fn curve(
    x: &Expr,
    y: &Expr,
    t0: f64,
    t1: f64,
    steps: Option<u32>,
    transform: &Affine,
    frame: &Frame,
    out: &mut Builder,
) -> Result<(), String> {
    let at = |t: f64, skipped: &mut usize| -> Option<Point> {
        let (bx, by) = transform.apply(x.eval(t), y.eval(t));
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
            transform: Affine::IDENTITY,
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
            transform: Affine::IDENTITY,
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
            transform: Affine::IDENTITY,
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
            transform: Affine::IDENTITY,
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
            transform: Affine::IDENTITY,
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
            transform: Affine::IDENTITY,
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
            transform: Affine::IDENTITY,
        };
        assert_eq!(plan(&[dot], &f, 4.0, 50_000).unwrap().strokes[0].len(), 1);
        let outside = Shape::Points {
            points: vec![(10.0, 10.0), (900.0, 10.0)],
            closed: false,
            smooth: false,
            transform: Affine::IDENTITY,
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
            transform: Affine::IDENTITY,
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

    fn near(a: Point, b: Point) -> bool {
        dist(a, b) < 1e-6
    }

    #[test]
    fn math_ranges_put_y_up() {
        // x -2..2, y -1..1 across (100, 100)-(500, 300).
        let f = Frame::range(Rect::new(100.0, 100.0, 400.0, 200.0), -2.0, 2.0, -1.0, 1.0);
        assert!(f.y_up());
        assert!(near(f.to_screen(0.0, 0.0), Point::new(300.0, 200.0)));
        assert!(near(f.to_screen(2.0, 1.0), Point::new(500.0, 100.0)));
        assert!(near(f.to_screen(-2.0, -1.0), Point::new(100.0, 300.0)));
        assert_eq!(f.to_frame(Point::new(500.0, 100.0)), (2.0, 1.0));
        assert_eq!(f.describe(), "-2..2 x -1..1");
        assert_eq!(f.screen_rect(), Rect::new(100.0, 100.0, 400.0, 200.0));
        // A plot of y = f(x) parsed with x as the variable.
        let sin = Expr::parse_in("sin(x)", Some("x")).unwrap();
        assert!((sin.eval(PI / 2.0) - 1.0).abs() < 1e-12);
        assert!(
            Expr::parse("sin(x)")
                .unwrap_err()
                .contains("unknown name `x`")
        );
    }

    #[test]
    fn turns_are_clockwise_on_screen_and_keep_circles_round() {
        // y up, and 1 unit is 100 px across but 50 px down.
        let f = Frame::range(Rect::new(0.0, 0.0, 400.0, 200.0), -2.0, 2.0, -2.0, 2.0);
        let r = f.rotation((0.0, 0.0), 90.0);
        let (x, y) = r.apply(1.0, 0.0);
        // Right of the centre, turned clockwise: straight down on screen,
        // as far away as before.
        let (c, p) = (f.to_screen(0.0, 0.0), f.to_screen(x, y));
        assert!((p.x - c.x).abs() < 1e-9 && p.y > c.y, "{p:?}");
        assert!((dist(c, p) - 100.0).abs() < 1e-9);
        // Composition: a move then a turn.
        let t = Affine::translate(1.0, 0.0).then(f.rotation((0.0, 0.0), 180.0));
        let (x, y) = t.apply(0.0, 0.0);
        assert!((x + 1.0).abs() < 1e-9 && y.abs() < 1e-9);
    }

    #[test]
    fn shape_helpers() {
        // A square (4 corners) with its first corner up, on screen.
        let sq = polygon(0.0, 0.0, 10.0, 4, false);
        assert_eq!(sq.len(), 4);
        assert!((sq[0].0).abs() < 1e-9 && (sq[0].1 + 10.0).abs() < 1e-9);
        let up_sq = polygon(0.0, 0.0, 10.0, 4, true);
        assert!((up_sq[0].1 - 10.0).abs() < 1e-9);
        let st = star(0.0, 0.0, 10.0, 4.0, 5, false);
        assert_eq!(st.len(), 10);
        assert!((st[1].0.hypot(st[1].1) - 4.0).abs() < 1e-9);
        let rr = rect(0.0, 0.0, 100.0, 50.0, 10.0);
        assert!(
            rr.iter()
                .all(|&(x, y)| (0.0..=100.0).contains(&x) && (0.0..=50.0).contains(&y))
        );
        assert_eq!(rect(0.0, 0.0, 10.0, 10.0, 0.0).len(), 4);
        let b = bezier(&[(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)]).unwrap();
        assert_eq!(b[0], (0.0, 0.0));
        assert!((b.last().unwrap().0 - 10.0).abs() < 1e-9);
        assert!(
            bezier(&[(0.0, 0.0), (1.0, 1.0)])
                .unwrap_err()
                .contains("4, 7, 10")
        );
        // Axes: two lines, then a tick at each step.
        let f = Frame::range(Rect::new(0.0, 0.0, 600.0, 300.0), -3.0, 3.0, -1.5, 1.5);
        let ax = axes(&f, 1.0, 0.5, 10.0).unwrap();
        assert_eq!(ax.len(), 2 + 7 + 7);
        assert!(
            axes(&f, 0.0001, 0.0, 10.0)
                .unwrap_err()
                .contains("too many ticks")
        );
    }

    #[test]
    fn closed_strokes_have_a_point_inside() {
        let p = |x, y| Point::new(x, y);
        let square = [
            p(0.0, 0.0),
            p(100.0, 0.0),
            p(100.0, 100.0),
            p(0.0, 100.0),
            p(0.0, 0.0),
        ];
        let one = |s: &[Point]| fill_points(&[s.to_vec()], 20).first().map(|&(_, q)| q);
        assert_eq!(one(&square), Some(p(50.0, 50.0)));
        // A C shape: its centre is in the gap, so the point comes from a
        // scan across it, inside the shape and clear of its edges.
        let c = [
            p(0.0, 0.0),
            p(100.0, 0.0),
            p(100.0, 20.0),
            p(20.0, 20.0),
            p(20.0, 80.0),
            p(100.0, 80.0),
            p(100.0, 100.0),
            p(0.0, 100.0),
            p(0.0, 0.0),
        ];
        let q = one(&c).unwrap();
        assert!(q.x > 5.0 && q.x < 15.0 && q.y > 25.0 && q.y < 75.0, "{q:?}");
        // An open line has no inside.
        assert_eq!(
            one(&[p(0.0, 0.0), p(10.0, 0.0), p(20.0, 5.0), p(30.0, 0.0)]),
            None
        );
    }

    /// Paint `strokes` with a square brush `w` wide on a grid; which
    /// pixel centres got paint.
    fn painted(strokes: &[Vec<Point>], w: f64, size: usize) -> Vec<bool> {
        let mut grid = vec![false; size * size];
        let r = w / 2.0;
        for s in strokes {
            let mut pts = s.clone();
            if pts.len() == 1 {
                pts.push(pts[0]);
            }
            for seg in pts.windows(2) {
                let n = (dist(seg[0], seg[1]) / 0.5).ceil().max(1.0) as usize;
                for k in 0..=n {
                    let f = k as f64 / n as f64;
                    let (cx, cy) = (
                        seg[0].x + (seg[1].x - seg[0].x) * f,
                        seg[0].y + (seg[1].y - seg[0].y) * f,
                    );
                    let (x0, x1) = ((cx - r).ceil().max(0.0) as usize, (cx + r).floor() as usize);
                    let (y0, y1) = ((cy - r).ceil().max(0.0) as usize, (cy + r).floor() as usize);
                    for y in y0..=y1.min(size - 1) {
                        for x in x0..=x1.min(size - 1) {
                            // Round brush.
                            if (x as f64 - cx).hypot(y as f64 - cy) <= r {
                                grid[y * size + x] = true;
                            }
                        }
                    }
                }
            }
        }
        grid
    }

    #[test]
    fn brush_fills_cover_the_shape_and_stay_inside() {
        let p = |x, y| Point::new(x, y);
        let circle: Vec<Point> = (0..=240)
            .map(|k| {
                let t = k as f64 / 240.0 * std::f64::consts::TAU;
                p(100.0 + 70.0 * t.cos(), 100.0 + 70.0 * t.sin())
            })
            .collect();
        let star: Vec<Point> = star(100.0, 100.0, 80.0, 35.0, 5, false)
            .into_iter()
            .chain(std::iter::once((100.0, 20.0)))
            .map(|(x, y)| p(x, y))
            .collect();
        for (name, shape) in [("circle", circle), ("star", star)] {
            for w in [6.0, 14.0] {
                let strokes = brush_fill(&shape, w, 0.0);
                let grid = painted(&strokes, w, 200);
                let (mut inside_px, mut covered, mut outside) = (0, 0, 0);
                for y in 0..200 {
                    for x in 0..200 {
                        let c = p(x as f64, y as f64);
                        let ins = inside(&shape, c);
                        // Clear of the edge by more than a pixel: must be
                        // painted. Outside by more than a pixel: must not.
                        let near = edge_distance(&shape, c) <= 1.5;
                        if ins && !near {
                            inside_px += 1;
                            covered += usize::from(grid[y * 200 + x]);
                        }
                        if !ins && !near && grid[y * 200 + x] {
                            outside += 1;
                        }
                    }
                }
                let share = covered as f64 / inside_px as f64;
                // Sharp star tips are too narrow for a round brush.
                let want = if name == "star" { 0.97 } else { 0.995 };
                assert!(share >= want, "{name} w {w}: {share}");
                assert_eq!(outside, 0, "{name} w {w}");
                // Few strokes: the rows are joined.
                assert!(strokes.len() <= 12, "{name} w {w}: {}", strokes.len());
            }
        }
        // With bleed the paint goes that far past the edge, no further.
        let sq: Vec<Point> = [
            (50.0, 50.0),
            (150.0, 50.0),
            (150.0, 150.0),
            (50.0, 150.0),
            (50.0, 50.0),
        ]
        .into_iter()
        .map(|(x, y)| p(x, y))
        .collect();
        let grid = painted(&brush_fill(&sq, 10.0, 3.0), 10.0, 200);
        assert!(grid[100 * 200 + 47] && !grid[100 * 200 + 45]);
        assert!(grid[48 * 200 + 100] && !grid[46 * 200 + 100]);
        assert!(shape_width(&sq) > 99.0 && shape_width(&sq) < 101.0);
    }

    #[test]
    fn fill_points_skip_the_shapes_drawn_inside() {
        // A badge: two circles (a ring), a star in the middle, an open line
        // and a dot outside the ring.
        let f = Frame::units(Rect::new(0.0, 0.0, 1000.0, 1000.0), 1000.0, 1000.0);
        let circle = |r: f64| Shape::Curve {
            x: Expr::parse(&format!("500+{r}*cos(t)")).unwrap(),
            y: Expr::parse(&format!("500+{r}*sin(t)")).unwrap(),
            t0: 0.0,
            t1: std::f64::consts::TAU,
            steps: None,
            transform: Affine::IDENTITY,
        };
        let shapes = vec![
            circle(400.0),
            circle(340.0),
            Shape::points(vec![(10.0, 990.0), (200.0, 990.0)], false),
            Shape::points(star(500.0, 500.0, 260.0, 110.0, 5, false), true),
            Shape::points(rect(490.0, 40.0, 20.0, 20.0, 0.0), true),
        ];
        let p = plan(&shapes, &f, 3.0, 50_000).unwrap();
        assert_eq!(p.shape, vec![0, 1, 2, 3, 4]);
        let fills = fill_points(&p.strokes, 20);
        let at = |i: usize| fills.iter().find(|&&(k, _)| k == i).map(|&(_, q)| q);
        let r = |q: Point| (q.x - 500.0).hypot(q.y - 500.0);
        // The ring's point is between its circles.
        let ring = at(0).unwrap();
        assert!(r(ring) > 345.0 && r(ring) < 395.0, "{ring:?}");
        // The inner circle's is inside it but not in the star.
        let disc = at(1).unwrap();
        assert!(r(disc) < 335.0 && r(disc) > 110.0, "{disc:?}");
        // The star's is its centre; the square's its middle.
        let st = at(3).unwrap();
        assert!(r(st) < 0.01, "{st:?}");
        let sq = at(4).unwrap();
        assert!(
            (sq.x - 500.0).abs() < 0.01 && (sq.y - 50.0).abs() < 0.01,
            "{sq:?}"
        );
        // The line has none.
        assert_eq!(at(2), None);
        assert_eq!(fills.len(), 4);
        assert_eq!(fill_points(&p.strokes, 2).len(), 2);

        // A circle full of dots: its point is clear of all of them.
        let mut many = vec![circle(400.0)];
        for gx in 0..20 {
            for gy in 0..20 {
                let (x, y) = (230.0 + gx as f64 * 28.0, 230.0 + gy as f64 * 28.0);
                many.push(Shape::points(rect(x, y, 10.0, 10.0, 0.0), true));
            }
        }
        let p = plan(&many, &f, 3.0, 50_000).unwrap();
        let q = fill_points(&p.strokes, 1)[0].1;
        assert!(r(q) < 395.0, "{q:?}");
        for s in &p.strokes[1..] {
            assert!(!inside(s, q), "{q:?} is in a dot");
        }
    }

    #[test]
    fn element_fractions_and_limits() {
        let f = Frame::fractions(Rect::new(200.0, 100.0, 400.0, 200.0));
        let diag = Shape::Points {
            points: vec![(0.0, 0.0), (1.0, 1.0)],
            closed: false,
            smooth: false,
            transform: Affine::IDENTITY,
        };
        let p = plan(&[diag], &f, 5.0, 50_000).unwrap();
        assert_eq!(p.strokes[0][0], Point::new(200.0, 100.0));
        assert_eq!(*p.strokes[0].last().unwrap(), Point::new(600.0, 300.0));
        assert_eq!(f.to_frame(Point::new(400.0, 200.0)), (0.5, 0.5));
        let err = plan(&[circle(None)], &screen(), 3.0, 50).unwrap_err();
        assert!(err.contains("more than 50"), "{err}");
    }
}
