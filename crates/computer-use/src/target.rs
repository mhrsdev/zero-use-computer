//! Pixel targeting: exact points on screen for a click or a drag. Snap a
//! rough point to the nearest corner, edge, shape centre or colour; find
//! every area of a colour, or every place that looks like a given one; and
//! magnify around a point to aim.
//!
//! Everything here works in capture pixels; the engine converts to and
//! from the x/y actions take.

use std::collections::VecDeque;

use crate::types::{Capture, Rect};

/// Colours this close (largest channel difference) count as one.
pub const SAME: i32 = 24;

fn rgb(cap: &Capture, x: usize, y: usize) -> [u8; 3] {
    let o = (y * cap.width as usize + x) * 4;
    [cap.rgba[o], cap.rgba[o + 1], cap.rgba[o + 2]]
}

fn close(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    a.iter()
        .zip(&b)
        .all(|(p, q)| (i32::from(*p) - i32::from(*q)).abs() <= tol)
}

fn gray(c: [u8; 3]) -> f64 {
    0.299 * f64::from(c[0]) + 0.587 * f64::from(c[1]) + 0.114 * f64::from(c[2])
}

fn valid(cap: &Capture) -> bool {
    cap.width > 0
        && cap.height > 0
        && cap.rgba.len() >= cap.width as usize * cap.height as usize * 4
}

/// What to snap a point to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Feature {
    /// The centre of the small shape the point is on.
    Center,
    /// The nearest place where the colour changes sharply.
    Edge,
    /// The nearest corner.
    Corner,
    /// The middle of the nearest area of this colour.
    Colour([u8; 3]),
}

impl Feature {
    pub fn parse(s: &str) -> Result<Feature, String> {
        match s.trim().to_lowercase().as_str() {
            "center" | "centre" => Ok(Feature::Center),
            "edge" => Ok(Feature::Edge),
            "corner" => Ok(Feature::Corner),
            other => match crate::design::parse_colour(other) {
                Ok(Some(c)) => Ok(Feature::Colour(c)),
                _ => Err(format!(
                    "snap to \"corner\", \"edge\", \"center\" or a colour \"#RRGGBB\", not \"{s}\""
                )),
            },
        }
    }

    /// "corner", "shape centre", "#FF0000 area".
    pub fn name(&self) -> String {
        match self {
            Feature::Center => "shape centre".into(),
            Feature::Edge => "edge".into(),
            Feature::Corner => "corner".into(),
            Feature::Colour(c) => format!("{} area", crate::design::hex(*c)),
        }
    }
}

/// A square window of the capture around (cx, cy): x0, y0, x1, y1
/// (inclusive), clipped.
fn window(cap: &Capture, cx: f64, cy: f64, r: f64) -> (usize, usize, usize, usize) {
    let (w, h) = (cap.width as f64, cap.height as f64);
    let x0 = (cx - r).floor().clamp(0.0, w - 1.0) as usize;
    let y0 = (cy - r).floor().clamp(0.0, h - 1.0) as usize;
    let x1 = (cx + r).ceil().clamp(0.0, w - 1.0) as usize;
    let y1 = (cy + r).ceil().clamp(0.0, h - 1.0) as usize;
    (x0, y0, x1, y1)
}

/// The point of `feature` nearest `at` within `radius` (capture pixels).
pub fn snap(cap: &Capture, at: (f64, f64), radius: f64, feature: Feature) -> Option<(f64, f64)> {
    if !valid(cap) || radius.is_nan() || radius <= 0.0 {
        return None;
    }
    let (ax, ay) = (at.0.floor(), at.1.floor());
    if ax < 0.0 || ay < 0.0 || ax >= f64::from(cap.width) || ay >= f64::from(cap.height) {
        return None;
    }
    let (x0, y0, x1, y1) = window(cap, ax, ay, radius + 2.0);
    let nearest = |cands: &mut dyn Iterator<Item = (usize, usize)>| {
        cands
            .filter(|&(x, y)| (x as f64 - ax).hypot(y as f64 - ay) <= radius)
            .min_by(|a, b| {
                let da = (a.0 as f64 - ax).hypot(a.1 as f64 - ay);
                let db = (b.0 as f64 - ax).hypot(b.1 as f64 - ay);
                da.total_cmp(&db)
            })
    };
    match feature {
        Feature::Center => {
            let c = rgb(cap, ax as usize, ay as usize);
            region_centre(cap, (ax as usize, ay as usize), c, radius * 3.0)
        }
        Feature::Colour(want) => {
            let mut cands = (y0..=y1)
                .flat_map(|y| (x0..=x1).map(move |x| (x, y)))
                .filter(|&(x, y)| close(rgb(cap, x, y), want, SAME));
            let (x, y) = nearest(&mut cands)?;
            region_centre(cap, (x, y), want, radius * 3.0)
                .or(Some((x as f64 + 0.5, y as f64 + 0.5)))
        }
        Feature::Edge => {
            let g = gradients(cap, x0, y0, x1, y1);
            let top = g.iter().map(|v| v.2).fold(0.0, f64::max);
            if top < 30.0 {
                return None;
            }
            let mut cands = g.iter().filter(|v| v.2 >= top * 0.35).map(|v| (v.0, v.1));
            let (x, y) = nearest(&mut cands)?;
            Some((x as f64 + 0.5, y as f64 + 0.5))
        }
        Feature::Corner => {
            let r = harris(cap, x0, y0, x1, y1);
            let top = r.iter().map(|v| v.2).fold(0.0, f64::max);
            if top <= 1.0 {
                return None;
            }
            // Responses by position, for the local-maximum test.
            let bw = x1 - x0 + 1;
            let at = |x: usize, y: usize| {
                if x < x0 || y < y0 || x > x1 || y > y1 {
                    0.0
                } else {
                    r[(y - y0) * bw + (x - x0)].2
                }
            };
            let mut cands = r
                .iter()
                .filter(|v| {
                    v.2 >= top * 0.1
                        && (-1i64..=1).all(|dy| {
                            (-1i64..=1).all(|dx| {
                                let (nx, ny) = (v.0 as i64 + dx, v.1 as i64 + dy);
                                nx < 0 || ny < 0 || at(nx as usize, ny as usize) <= v.2
                            })
                        })
                })
                .map(|v| (v.0, v.1));
            let (x, y) = nearest(&mut cands)?;
            // A corner pixel's own corner nearest the shape: its centre is
            // close enough for a click.
            Some((x as f64 + 0.5, y as f64 + 0.5))
        }
    }
}

/// The centre of the area of colour `c` around `start`, if it fits in a
/// box `reach` across each way (a large area has no useful centre).
fn region_centre(
    cap: &Capture,
    start: (usize, usize),
    c: [u8; 3],
    reach: f64,
) -> Option<(f64, f64)> {
    let (x0, y0, x1, y1) = window(cap, start.0 as f64, start.1 as f64, reach);
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut seen = vec![false; bw * bh];
    let idx = |x: usize, y: usize| (y - y0) * bw + (x - x0);
    let mut queue = VecDeque::from([start]);
    seen[idx(start.0, start.1)] = true;
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    while let Some((x, y)) = queue.pop_front() {
        // Touching the window's edge (not the picture's): too big.
        let edge_of_window = (x == x0 && x0 > 0)
            || (y == y0 && y0 > 0)
            || (x == x1 && x1 + 1 < cap.width as usize)
            || (y == y1 && y1 + 1 < cap.height as usize);
        if edge_of_window {
            return None;
        }
        sx += x as f64 + 0.5;
        sy += y as f64 + 0.5;
        n += 1.0;
        let around = [
            (x > x0).then(|| (x - 1, y)),
            (x < x1).then(|| (x + 1, y)),
            (y > y0).then(|| (x, y - 1)),
            (y < y1).then(|| (x, y + 1)),
        ];
        for (nx, ny) in around.into_iter().flatten() {
            if !seen[idx(nx, ny)] && close(rgb(cap, nx, ny), c, SAME) {
                seen[idx(nx, ny)] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    (n > 0.0).then(|| (sx / n, sy / n))
}

/// Sobel gradient magnitude over a window: (x, y, magnitude).
fn gradients(
    cap: &Capture,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> Vec<(usize, usize, f64)> {
    let (w, h) = (cap.width as usize, cap.height as usize);
    let g = |x: i64, y: i64| {
        gray(rgb(
            cap,
            x.clamp(0, w as i64 - 1) as usize,
            y.clamp(0, h as i64 - 1) as usize,
        ))
    };
    let mut out = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (x, y) = (x as i64, y as i64);
            let gx = g(x + 1, y - 1) + 2.0 * g(x + 1, y) + g(x + 1, y + 1)
                - g(x - 1, y - 1)
                - 2.0 * g(x - 1, y)
                - g(x - 1, y + 1);
            let gy = g(x - 1, y + 1) + 2.0 * g(x, y + 1) + g(x + 1, y + 1)
                - g(x - 1, y - 1)
                - 2.0 * g(x, y - 1)
                - g(x + 1, y - 1);
            out.push((x as usize, y as usize, gx.hypot(gy) / 4.0));
        }
    }
    out
}

/// Harris corner response over a window: (x, y, response).
fn harris(cap: &Capture, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<(usize, usize, f64)> {
    let (w, h) = (cap.width as usize, cap.height as usize);
    let g = |x: i64, y: i64| {
        gray(rgb(
            cap,
            x.clamp(0, w as i64 - 1) as usize,
            y.clamp(0, h as i64 - 1) as usize,
        ))
    };
    let d = |x: i64, y: i64| {
        (
            (g(x + 1, y) - g(x - 1, y)) / 2.0,
            (g(x, y + 1) - g(x, y - 1)) / 2.0,
        )
    };
    let mut out = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (mut a, mut b, mut c) = (0.0, 0.0, 0.0);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (ix, iy) = d(x as i64 + dx, y as i64 + dy);
                    a += ix * ix;
                    b += ix * iy;
                    c += iy * iy;
                }
            }
            let r = (a * c - b * b) - 0.04 * (a + c) * (a + c);
            out.push((x, y, r.max(0.0)));
        }
    }
    out
}

/// An area of one colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Blob {
    pub center: (f64, f64),
    pub bbox: Rect,
    pub pixels: usize,
}

/// The areas of `colour` (within `tol`) in `area` of the capture, biggest
/// first.
pub fn colour_blobs(cap: &Capture, area: Rect, colour: [u8; 3], tol: i32) -> Vec<Blob> {
    if !valid(cap) {
        return Vec::new();
    }
    let x0 = area.x.max(0.0).floor() as usize;
    let y0 = area.y.max(0.0).floor() as usize;
    let x1 = ((area.x + area.width).ceil() as usize).min(cap.width as usize);
    let y1 = ((area.y + area.height).ceil() as usize).min(cap.height as usize);
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let bw = x1 - x0;
    let mut seen = vec![false; bw * (y1 - y0)];
    let mut out = Vec::new();
    for sy in y0..y1 {
        for sx in x0..x1 {
            let k = (sy - y0) * bw + (sx - x0);
            if seen[k] || !close(rgb(cap, sx, sy), colour, tol) {
                continue;
            }
            seen[k] = true;
            let mut queue = VecDeque::from([(sx, sy)]);
            let (mut cx, mut cy, mut n) = (0.0, 0.0, 0usize);
            let (mut bx0, mut by0, mut bx1, mut by1) = (sx, sy, sx, sy);
            while let Some((x, y)) = queue.pop_front() {
                cx += x as f64 + 0.5;
                cy += y as f64 + 0.5;
                n += 1;
                (bx0, by0, bx1, by1) = (bx0.min(x), by0.min(y), bx1.max(x), by1.max(y));
                let around = [
                    (x > x0).then(|| (x - 1, y)),
                    (x + 1 < x1).then(|| (x + 1, y)),
                    (y > y0).then(|| (x, y - 1)),
                    (y + 1 < y1).then(|| (x, y + 1)),
                ];
                for (nx, ny) in around.into_iter().flatten() {
                    let k = (ny - y0) * bw + (nx - x0);
                    if !seen[k] && close(rgb(cap, nx, ny), colour, tol) {
                        seen[k] = true;
                        queue.push_back((nx, ny));
                    }
                }
            }
            out.push(Blob {
                center: (cx / n as f64, cy / n as f64),
                bbox: Rect::new(
                    bx0 as f64,
                    by0 as f64,
                    (bx1 - bx0 + 1) as f64,
                    (by1 - by0 + 1) as f64,
                ),
                pixels: n,
            });
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.pixels));
    out
}

/// A place that looks like the template.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub bbox: Rect,
    /// Normalised cross-correlation: 1 is the same picture.
    pub score: f64,
}

/// Every place in `area` that looks like the `template` part of the
/// capture (score at least `min`), best first. Errors when the template
/// has no detail to match.
pub fn look_alikes(
    cap: &Capture,
    template: Rect,
    area: Rect,
    min: f64,
) -> Result<Vec<Match>, String> {
    if !valid(cap) {
        return Ok(Vec::new());
    }
    let (cw, ch) = (cap.width as usize, cap.height as usize);
    let clip = |r: Rect| {
        let x0 = r.x.max(0.0).floor() as usize;
        let y0 = r.y.max(0.0).floor() as usize;
        let x1 = ((r.x + r.width).ceil().max(0.0) as usize).min(cw);
        let y1 = ((r.y + r.height).ceil().max(0.0) as usize).min(ch);
        (x0, y0, x1.max(x0), y1.max(y0))
    };
    let (tx0, ty0, tx1, ty1) = clip(template);
    let (tw, th) = (tx1 - tx0, ty1 - ty0);
    if tw < 3 || th < 3 {
        return Err("the box to look for must be at least 3 x 3 pixels".into());
    }
    // Work on a smaller copy so the template is about 16 pixels across.
    let f = (tw.max(th) as f64 / 16.0).ceil().max(1.0) as usize;
    let small = |x0: usize, y0: usize, w: usize, h: usize| -> (Vec<f64>, usize, usize) {
        let (sw, sh) = (w / f, h / f);
        let mut out = vec![0.0; sw * sh];
        for y in 0..sh {
            for x in 0..sw {
                let mut s = 0.0;
                for dy in 0..f {
                    for dx in 0..f {
                        s += gray(rgb(cap, x0 + x * f + dx, y0 + y * f + dy));
                    }
                }
                out[y * sw + x] = s / (f * f) as f64;
            }
        }
        (out, sw, sh)
    };
    let (t, sw, sh) = small(tx0, ty0, tw, th);
    let n = (sw * sh) as f64;
    let tm = t.iter().sum::<f64>() / n;
    let tz: Vec<f64> = t.iter().map(|v| v - tm).collect();
    let tn = tz.iter().map(|v| v * v).sum::<f64>().sqrt();
    if tn < 1e-6 || tn / n.sqrt() < 2.0 {
        return Err("the box to look for is one flat colour: give a box round something with detail, or look for its colour".into());
    }
    let (ax0, ay0, ax1, ay1) = clip(area);
    let (img, iw, ih) = small(ax0, ay0, ax1 - ax0, ay1 - ay0);
    if iw < sw || ih < sh {
        return Ok(Vec::new());
    }
    let mut hits: Vec<(f64, usize, usize)> = Vec::new();
    for y in 0..=ih - sh {
        for x in 0..=iw - sw {
            let mut sum = 0.0;
            let mut sq = 0.0;
            for j in 0..sh {
                let row = &img[(y + j) * iw + x..(y + j) * iw + x + sw];
                for v in row {
                    sum += v;
                    sq += v * v;
                }
            }
            let m = sum / n;
            let var = (sq - sum * m).max(0.0);
            if var < 1e-6 {
                continue;
            }
            let mut dot = 0.0;
            for j in 0..sh {
                let row = &img[(y + j) * iw + x..(y + j) * iw + x + sw];
                for (v, t) in row.iter().zip(&tz[j * sw..(j + 1) * sw]) {
                    dot += (v - m) * t;
                }
            }
            let score = dot / (var.sqrt() * tn);
            if score >= min {
                hits.push((score, x, y));
            }
        }
    }
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    // One hit per place: drop those overlapping a better one by over half.
    let mut out: Vec<Match> = Vec::new();
    for (score, x, y) in hits {
        let bbox = Rect::new(
            (ax0 + x * f) as f64,
            (ay0 + y * f) as f64,
            tw as f64,
            th as f64,
        );
        let overlaps = out.iter().any(|m| {
            (m.bbox.x - bbox.x).abs() < tw as f64 / 2.0
                && (m.bbox.y - bbox.y).abs() < th as f64 / 2.0
        });
        if !overlaps {
            out.push(Match { bbox, score });
            if out.len() >= 20 {
                break;
            }
        }
    }
    Ok(out)
}

/// The capture around (cx, cy), `radius` pixels each way, blown up so
/// each pixel is a square `zoom` pixels across, with lines between the
/// pixels and a crosshair on the point. Returns the picture and the
/// capture pixel its top-left square shows.
pub fn loupe(
    cap: &Capture,
    cx: f64,
    cy: f64,
    radius: usize,
    zoom: usize,
) -> Option<(Capture, (usize, usize))> {
    if !valid(cap) || zoom == 0 {
        return None;
    }
    let (x0, y0, x1, y1) = window(cap, cx.floor(), cy.floor(), radius as f64);
    let (w, h) = ((x1 - x0 + 1) * zoom, (y1 - y0 + 1) * zoom);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x0 + x / zoom, y0 + y / zoom);
            let mut c = rgb(cap, sx, sy);
            // Pixel borders: a little darker or lighter, so colours stay
            // readable.
            if zoom >= 6 && (x % zoom == 0 || y % zoom == 0) {
                c = c.map(|v| if v > 128 { v - 40 } else { v + 40 });
            }
            let o = (y * w + x) * 4;
            rgba[o..o + 3].copy_from_slice(&c);
            rgba[o + 3] = 255;
        }
    }
    // Crosshair through the point, with a gap so the pixel shows.
    let (px, py) = (
        ((cx - x0 as f64) * zoom as f64).round() as i64,
        ((cy - y0 as f64) * zoom as f64).round() as i64,
    );
    let gap = zoom as i64;
    let mut put = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
            let o = (y as usize * w + x as usize) * 4;
            rgba[o..o + 3].copy_from_slice(&[255, 0, 60]);
        }
    };
    for d in 0..(w.max(h) as i64) {
        for t in [-1, 0] {
            if (d - px).abs() > gap {
                put(d, py + t);
            }
            if (d - py).abs() > gap {
                put(px + t, d);
            }
        }
    }
    Some((
        Capture {
            width: w as u32,
            height: h as u32,
            rgba,
            bounds: Rect::new(0.0, 0.0, w as f64, h as f64),
        },
        (x0, y0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32) -> Capture {
        Capture {
            width: w,
            height: h,
            rgba: [255u8, 255, 255, 255].repeat((w * h) as usize),
            bounds: Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        }
    }

    fn fill(cap: &mut Capture, x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                let o = (y * cap.width as usize + x) * 4;
                cap.rgba[o..o + 3].copy_from_slice(&c);
            }
        }
    }

    #[test]
    fn points_snap_to_corners_edges_centres_and_colours() {
        let mut cap = canvas(120, 80);
        // A dark square 40..80 x 20..60, and a small red dot.
        fill(&mut cap, 40, 20, 80, 60, [30, 30, 30]);
        fill(&mut cap, 100, 10, 106, 16, [220, 20, 20]);
        // A point a little off the square's top-left corner.
        let c = snap(&cap, (44.0, 23.0), 10.0, Feature::Corner).unwrap();
        assert!(
            (c.0 - 40.0).abs() <= 1.5 && (c.1 - 20.0).abs() <= 1.5,
            "{c:?}"
        );
        // Near the left edge, from outside it.
        let e = snap(&cap, (34.0, 40.0), 10.0, Feature::Edge).unwrap();
        assert!(
            (e.0 - 39.5).abs() <= 1.5 && (e.1 - 40.0).abs() <= 1.0,
            "{e:?}"
        );
        // The dot's centre, from a point on it.
        let m = snap(&cap, (101.0, 11.0), 10.0, Feature::Center).unwrap();
        assert!(
            (m.0 - 103.0).abs() < 0.1 && (m.1 - 13.0).abs() < 0.1,
            "{m:?}"
        );
        // The red area, from a point near it.
        let r = snap(&cap, (96.0, 20.0), 12.0, Feature::Colour([220, 20, 20])).unwrap();
        assert!(
            (r.0 - 103.0).abs() < 0.1 && (r.1 - 13.0).abs() < 0.1,
            "{r:?}"
        );
        // The big white background has no useful centre; plain white has
        // no edge or corner.
        assert_eq!(snap(&cap, (10.0, 70.0), 8.0, Feature::Center), None);
        assert_eq!(snap(&cap, (10.0, 70.0), 8.0, Feature::Edge), None);
        assert_eq!(snap(&cap, (10.0, 70.0), 8.0, Feature::Corner), None);
        assert_eq!(Feature::parse("CORNER"), Ok(Feature::Corner));
        assert_eq!(Feature::parse("#00ff00"), Ok(Feature::Colour([0, 255, 0])));
        assert!(Feature::parse("middle").is_err());
    }

    #[test]
    fn colour_areas_are_found_biggest_first() {
        let mut cap = canvas(100, 60);
        fill(&mut cap, 10, 10, 20, 20, [0, 120, 255]);
        fill(&mut cap, 50, 30, 80, 50, [5, 125, 250]);
        let blobs = colour_blobs(&cap, Rect::new(0.0, 0.0, 100.0, 60.0), [0, 120, 255], 12);
        assert_eq!(blobs.len(), 2);
        assert_eq!(blobs[0].pixels, 600);
        assert_eq!(blobs[0].center, (65.0, 40.0));
        assert_eq!(blobs[1].bbox, Rect::new(10.0, 10.0, 10.0, 10.0));
        // Only inside the area asked.
        let left = colour_blobs(&cap, Rect::new(0.0, 0.0, 40.0, 60.0), [0, 120, 255], 12);
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn look_alikes_find_the_same_icon_elsewhere() {
        let mut cap = canvas(160, 80);
        // Three copies of a small icon (a square with a dot), one other
        // shape.
        for (x, y) in [(10, 10), (70, 30), (120, 50)] {
            fill(&mut cap, x, y, x + 12, y + 12, [40, 90, 200]);
            fill(&mut cap, x + 4, y + 4, x + 8, y + 8, [250, 250, 250]);
        }
        fill(&mut cap, 40, 50, 52, 62, [200, 40, 40]);
        let found = look_alikes(
            &cap,
            Rect::new(8.0, 8.0, 16.0, 16.0),
            Rect::new(0.0, 0.0, 160.0, 80.0),
            0.9,
        )
        .unwrap();
        let mut at: Vec<(f64, f64)> = found.iter().map(|m| (m.bbox.x, m.bbox.y)).collect();
        at.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(
            at,
            vec![(8.0, 8.0), (68.0, 28.0), (118.0, 48.0)],
            "{found:?}"
        );
        assert!(found.iter().all(|m| m.score > 0.95));
        // A flat box can't be looked for.
        let flat = look_alikes(
            &cap,
            Rect::new(140.0, 0.0, 10.0, 10.0),
            Rect::new(0.0, 0.0, 160.0, 80.0),
            0.9,
        );
        assert!(flat.unwrap_err().contains("flat colour"));
    }

    #[test]
    fn the_loupe_magnifies_with_a_crosshair() {
        let mut cap = canvas(50, 50);
        fill(&mut cap, 25, 25, 26, 26, [0, 0, 0]);
        let (pic, origin) = loupe(&cap, 25.5, 25.5, 5, 10).unwrap();
        assert_eq!((pic.width, pic.height), (110, 110));
        assert_eq!(origin, (20, 20));
        // The black pixel is the square at (5, 5): its middle stays black.
        let at = |x: usize, y: usize| {
            let o = (y * 110 + x) * 4;
            [pic.rgba[o], pic.rgba[o + 1], pic.rgba[o + 2]]
        };
        assert_eq!(at(54, 54), [0, 0, 0]);
        // The crosshair runs through the point, away from it.
        assert_eq!(at(5, 55), [255, 0, 60]);
        assert_eq!(at(55, 100), [255, 0, 60]);
    }
}
