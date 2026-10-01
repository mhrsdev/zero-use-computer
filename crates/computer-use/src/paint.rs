//! Painting helpers that look at pixels: what a bucket fill would do inside
//! a drawn outline, and a reference picture turned into a few flat colours
//! and shapes to paint back to front (and compared with the result).

use std::collections::VecDeque;

use crate::draw::crossings;
use crate::types::{Capture, Point, Rect};

/// Colours this close (largest channel difference) count as one, like a
/// bucket fill's tolerance.
const SAME: i32 = 24;

fn same(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter()
        .zip(&b)
        .all(|(p, q)| (i32::from(*p) - i32::from(*q)).abs() <= SAME)
}

/// The colour of capture pixel (x, y), on white where it is see-through.
fn rgb_at(cap: &Capture, x: usize, y: usize) -> [u8; 3] {
    let o = (y * cap.width as usize + x) * 4;
    let a = f32::from(cap.rgba[o + 3]) / 255.0;
    let c = |v: u8| (f32::from(v) * a + 255.0 * (1.0 - a)).round() as u8;
    [c(cap.rgba[o]), c(cap.rgba[o + 1]), c(cap.rgba[o + 2])]
}

fn valid(cap: &Capture) -> bool {
    cap.width > 0
        && cap.height > 0
        && cap.rgba.len() >= cap.width as usize * cap.height as usize * 4
}

// ---------------------------------------------------------------------------
// Bucket fills

/// What a bucket fill does inside one closed outline.
#[derive(Debug, Clone, PartialEq)]
pub struct FillCheck {
    /// Where to click, one per piece the other lines cut the shape into,
    /// biggest first (capture pixels).
    pub clicks: Vec<Point>,
    /// Near where a fill would run out of the shape through a gap in its
    /// outline (capture pixels), if it would.
    pub leak: Option<Point>,
}

/// Flood `cap` like a bucket fill inside the closed `outline` (capture
/// pixels), from `seed`, minus the closed `holes` drawn within it. `None`
/// when the shape isn't wholly in the picture.
pub fn fill_check(
    cap: &Capture,
    outline: &[Point],
    holes: &[&[Point]],
    seed: Point,
) -> Option<FillCheck> {
    if !valid(cap) || outline.len() < 3 {
        return None;
    }
    let (w, h) = (i64::from(cap.width), i64::from(cap.height));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in outline {
        (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
    }
    // The shape's box and a margin: a fill that reaches its edge has left
    // the shape.
    const MARGIN: i64 = 4;
    let (bx0, by0) = (x0.floor() as i64 - MARGIN, y0.floor() as i64 - MARGIN);
    let (bx1, by1) = (x1.ceil() as i64 + MARGIN, y1.ceil() as i64 + MARGIN);
    if bx0 < 0 || by0 < 0 || bx1 >= w || by1 >= h {
        return None;
    }
    let (bw, bh) = ((bx1 - bx0 + 1) as usize, (by1 - by0 + 1) as usize);
    let (si, sj) = (seed.x.floor() as i64 - bx0, seed.y.floor() as i64 - by0);
    if si < 0 || sj < 0 || si as usize >= bw || sj as usize >= bh {
        return None;
    }
    let seed_k = sj as usize * bw + si as usize;
    // Which pixel centres are inside a closed outline.
    let mask = |poly: &[Point]| -> Vec<bool> {
        let mut m = vec![false; bw * bh];
        for j in 0..bh {
            let y = (by0 + j as i64) as f64 + 0.5;
            let mut xs = crossings(poly, y, false);
            xs.sort_by(f64::total_cmp);
            for pair in xs.as_chunks::<2>().0 {
                let i0 = ((pair[0] - 0.5).ceil() as i64 - bx0).max(0);
                let i1 = ((pair[1] - 0.5).floor() as i64 - bx0).min(bw as i64 - 1);
                for i in i0..=i1 {
                    m[j * bw + i as usize] = true;
                }
            }
        }
        m
    };
    let shape = mask(outline);
    let mut inner = shape.clone();
    for hole in holes {
        for (k, in_hole) in mask(hole).into_iter().enumerate() {
            if in_hole {
                inner[k] = false;
            }
        }
    }
    // Inside and 2 px clear of the edge: not the drawn line itself.
    let core: Vec<bool> = (0..bw * bh)
        .map(|k| {
            let (i, j) = ((k % bw) as i64, (k / bw) as i64);
            [(0, 0), (2, 0), (-2, 0), (0, 2), (0, -2)]
                .iter()
                .all(|(dx, dy)| {
                    let (a, b) = (i + dx, j + dy);
                    a >= 0
                        && b >= 0
                        && (a as usize) < bw
                        && (b as usize) < bh
                        && inner[b as usize * bw + a as usize]
                })
        })
        .collect();
    let colour = |k: usize| rgb_at(cap, bx0 as usize + k % bw, by0 as usize + k / bw);
    // What a bucket fills here is the colour most of the inside has (the
    // seed may sit on another line): start from the pixel of that colour
    // nearest the seed.
    let mut counts: std::collections::HashMap<[u8; 3], usize> = std::collections::HashMap::new();
    for k in (0..bw * bh).filter(|&k| core[k]) {
        *counts.entry(colour(k)).or_default() += 1;
    }
    let c0 = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(c, _)| c)?;
    let kept = core[seed_k] && same(colour(seed_k), c0);
    let seed_k = if kept {
        seed_k
    } else {
        let (si, sj) = (seed_k % bw, seed_k / bw);
        (0..bw * bh)
            .filter(|&k| core[k] && same(colour(k), c0))
            .min_by_key(|&k| {
                let (i, j) = (k % bw, k / bw);
                i.abs_diff(si).pow(2) + j.abs_diff(sj).pow(2)
            })?
    };
    let seed = if kept {
        seed
    } else {
        Point::new(
            (bx0 + (seed_k % bw) as i64) as f64 + 0.5,
            (by0 + (seed_k / bw) as i64) as f64 + 0.5,
        )
    };
    let at = |k: usize| {
        Point::new(
            (bx0 + (k % bw) as i64) as f64 + 0.5,
            (by0 + (k / bw) as i64) as f64 + 0.5,
        )
    };

    let mut piece = vec![0u32; bw * bh];
    let fill = Flood {
        bw,
        bh,
        shape: &shape,
        colour: &colour,
        c0,
    };
    let (first, escaped) = fill.run(&mut piece, seed_k, 1);
    if let Some(k) = escaped {
        return Some(FillCheck {
            clicks: vec![seed],
            leak: Some(at(k)),
        });
    }
    let core_area = |pixels: &[usize]| pixels.iter().filter(|&&k| core[k]).count();
    // (area, click)
    let mut pieces: Vec<(usize, Point)> = vec![(core_area(&first), seed)];
    let mut leak = None;
    let total = (0..bw * bh)
        .filter(|&k| core[k] && same(colour(k), c0))
        .count();
    let floor = (total * 2 / 100).max(20);
    let mut id = 1;
    for k in 0..bw * bh {
        if !core[k] || !same(colour(k), c0) {
            continue;
        }
        // Already part of a piece.
        if piece[k] != 0 {
            continue;
        }
        id += 1;
        let (pixels, escaped) = fill.run(&mut piece, k, id);
        if let Some(e) = escaped {
            leak.get_or_insert(at(e));
            continue;
        }
        let area = core_area(&pixels);
        if area >= floor {
            pieces.push((area, click_in(&pixels, bw).map_or(at(k), at)));
        }
    }
    pieces.sort_by_key(|p| std::cmp::Reverse(p.0));
    pieces.truncate(8);
    Some(FillCheck {
        clicks: pieces.into_iter().map(|p| p.1).collect(),
        leak,
    })
}

/// Flooding pixels of one colour inside a shape's box.
struct Flood<'a> {
    bw: usize,
    bh: usize,
    /// Which pixels are inside the outline.
    shape: &'a [bool],
    colour: &'a dyn Fn(usize) -> [u8; 3],
    c0: [u8; 3],
}

impl Flood<'_> {
    /// Mark the piece reached from `start` with `id`. Returns its pixels,
    /// and where it left the shape if it reached the edge of the box.
    fn run(&self, piece: &mut [u32], start: usize, id: u32) -> (Vec<usize>, Option<usize>) {
        let (bw, bh) = (self.bw, self.bh);
        let mut pixels = Vec::new();
        let mut out_at = None;
        let mut escaped = false;
        let mut queue = VecDeque::from([start]);
        piece[start] = id;
        while let Some(k) = queue.pop_front() {
            pixels.push(k);
            let (i, j) = (k % bw, k / bw);
            if !self.shape[k] && out_at.is_none() {
                out_at = Some(k);
            }
            if i == 0 || j == 0 || i + 1 == bw || j + 1 == bh {
                escaped = true;
            }
            let neighbours = [
                (i > 0).then(|| k - 1),
                (i + 1 < bw).then(|| k + 1),
                (j > 0).then(|| k - bw),
                (j + 1 < bh).then(|| k + bw),
            ];
            for n in neighbours.into_iter().flatten() {
                if piece[n] == 0 && same((self.colour)(n), self.c0) {
                    piece[n] = id;
                    queue.push_back(n);
                }
            }
        }
        (pixels, escaped.then(|| out_at.unwrap_or(start)))
    }
}

/// A pixel well inside a piece: the middle of its longest row.
fn click_in(pixels: &[usize], bw: usize) -> Option<usize> {
    let mut sorted = pixels.to_vec();
    sorted.sort_unstable();
    let mut best: Option<(usize, usize)> = None; // (length, middle)
    let mut start = 0;
    for i in 1..=sorted.len() {
        let breaks = i == sorted.len()
            || sorted[i] != sorted[i - 1] + 1
            || sorted[i] / bw != sorted[i - 1] / bw;
        if breaks {
            let len = i - start;
            if best.is_none_or(|(l, _)| len > l) {
                best = Some((len, sorted[start + len / 2]));
            }
            start = i;
        }
    }
    best.map(|b| b.1)
}

// ---------------------------------------------------------------------------
// Colour

/// sRGB to CIE L*a*b* (D65).
fn lab(c: [u8; 3]) -> [f32; 3] {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f32| {
        if t > 0.008_856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// How different two colours look (CIE76 ΔE: about 2 is just visible, 10
/// clearly different).
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    f64::from(dist2(lab(a), lab(b)).sqrt())
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

// ---------------------------------------------------------------------------
// Tracing a reference

/// A reference picture as flat colours: regions to paint back to front.
#[derive(Debug, Clone)]
pub struct Trace {
    /// The source picture's size (its proportions are kept).
    pub width: u32,
    pub height: u32,
    /// What to paint, in order: one colour per step.
    pub steps: Vec<TraceStep>,
    /// The flat picture at the working size, for comparing.
    pub target: Capture,
}

#[derive(Debug, Clone)]
pub struct TraceStep {
    pub color: [u8; 3],
    /// Closed outlines in 0–1 of the picture's width and height.
    pub regions: Vec<Vec<(f64, f64)>>,
    /// Share of the picture inside these outlines.
    pub share: f64,
}

/// How fine a trace is: the working size's longest side, and the smallest
/// region kept (a share of the picture).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Detail {
    pub side: u32,
    pub min_share: f64,
}

impl Detail {
    pub const LOW: Detail = Detail {
        side: 96,
        min_share: 0.015,
    };
    pub const MEDIUM: Detail = Detail {
        side: 160,
        min_share: 0.005,
    };
    pub const HIGH: Detail = Detail {
        side: 256,
        min_share: 0.002,
    };
}

/// At most this many regions; smaller ones are merged into neighbours.
const MAX_REGIONS: usize = 60;

/// Turn `src` into at most `colors` flat colours and shapes to paint back
/// to front.
pub fn trace(src: &Capture, colors: usize, detail: Detail) -> Trace {
    let (sw, sh) = (src.width.max(1), src.height.max(1));
    let (ww, wh) = crate::imaging::fit(sw, sh, detail.side.max(16));
    let (ww, wh) = (ww.max(1) as usize, wh.max(1) as usize);
    let n = ww * wh;
    // Box-average down to the working size.
    let mut px: Vec<[u8; 3]> = Vec::with_capacity(n);
    for oy in 0..wh {
        let ya = oy * sh as usize / wh;
        let yb = ((oy + 1) * sh as usize).div_ceil(wh).max(ya + 1);
        for ox in 0..ww {
            let xa = ox * sw as usize / ww;
            let xb = ((ox + 1) * sw as usize).div_ceil(ww).max(xa + 1);
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            if valid(src) {
                for y in ya..yb.min(sh as usize) {
                    for x in xa..xb.min(sw as usize) {
                        let c = rgb_at(src, x, y);
                        for (s, v) in sum.iter_mut().zip(c) {
                            *s += u64::from(v);
                        }
                        count += 1;
                    }
                }
            }
            let count = count.max(1);
            px.push(sum.map(|s| (s / count) as u8));
        }
    }
    // Flatten texture (fur, grain) but keep edges.
    for _ in 0..2 {
        px = kuwahara(&px, ww, wh);
    }
    let labs: Vec<[f32; 3]> = px.iter().map(|&c| lab(c)).collect();
    let k = colors.clamp(2, 16);
    let mut labels = cluster(&labs, k);
    // Smooth away specks.
    for _ in 0..2 {
        labels = majority(&labels, ww, wh, k);
    }
    // Merge regions smaller than the floor (raised while there are too
    // many) into their most common neighbour.
    let mut floor = ((detail.min_share * n as f64) as usize).max(4);
    let mut comps = components(&labels, ww, wh);
    for _ in 0..12 {
        let small: Vec<usize> = (0..comps.count)
            .filter(|&c| comps.area[c] < floor)
            .collect();
        if small.is_empty() {
            if comps.count <= MAX_REGIONS {
                break;
            }
            floor = floor * 3 / 2 + 1;
            continue;
        }
        // Each label's colour, to merge a speck into the neighbour that
        // looks most like it.
        let mut sums = vec![([0.0f32; 3], 0.0f32); 16];
        for (p, &l) in labels.iter().enumerate() {
            let e = &mut sums[usize::from(l)];
            for (acc, v) in e.0.iter_mut().zip(labs[p]) {
                *acc += v;
            }
            e.1 += 1.0;
        }
        let looks: Vec<[f32; 3]> = sums
            .iter()
            .map(|(s, n)| s.map(|v| v / n.max(1.0)))
            .collect();
        let mut pixels_of: Vec<Vec<usize>> = vec![Vec::new(); comps.count];
        for (p, &id) in comps.id.iter().enumerate() {
            pixels_of[id].push(p);
        }
        for c in small {
            let own = usize::from(labels[comps.first[c]]);
            // Border length shared with each neighbouring label.
            let mut border = [0usize; 16];
            for &p in &pixels_of[c] {
                let (x, y) = (p % ww, p / ww);
                let around = [
                    (x > 0).then(|| p - 1),
                    (x + 1 < ww).then(|| p + 1),
                    (y > 0).then(|| p - ww),
                    (y + 1 < wh).then(|| p + ww),
                ];
                for q in around.into_iter().flatten() {
                    if comps.id[q] != c {
                        border[usize::from(labels[q])] += 1;
                    }
                }
            }
            let best = (0..16)
                .filter(|&l| border[l] > 0 && l != own)
                .min_by(|&a, &b| {
                    dist2(looks[own], looks[a])
                        .total_cmp(&dist2(looks[own], looks[b]))
                        .then(border[b].cmp(&border[a]))
                });
            if let Some(best) = best {
                for &p in &pixels_of[c] {
                    labels[p] = best as u8;
                }
            }
        }
        comps = components(&labels, ww, wh);
    }
    // The colour of each label: the mean of its pixels.
    let mut sums = vec![[0u64; 4]; k];
    for (p, &l) in labels.iter().enumerate() {
        let s = &mut sums[usize::from(l)];
        for c in 0..3 {
            s[c] += u64::from(px[p][c]);
        }
        s[3] += 1;
    }
    let colour_of: Vec<[u8; 3]> = sums
        .iter()
        .map(|s| {
            let n = s[3].max(1);
            [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
        })
        .collect();

    // Each region with its holes filled: its outline, the area inside it,
    // and the regions inside its holes (painted after it).
    struct Region {
        label: u8,
        outline: Vec<(f64, f64)>,
        outer: usize,
        inside: Vec<usize>,
    }
    let mut regions: Vec<Region> = Vec::with_capacity(comps.count);
    for c in 0..comps.count {
        let (filled, bx, by, lw, lh) = filled_mask(&comps, c, ww, wh);
        let outer = filled.iter().filter(|&&f| f).count();
        let mut inside: Vec<usize> = Vec::new();
        for j in 0..lh {
            for i in 0..lw {
                let (x, y) = (bx + i, by + j);
                if filled[j * lw + i] && comps.id[y * ww + x] != c {
                    let other = comps.id[y * ww + x];
                    if !inside.contains(&other) {
                        inside.push(other);
                    }
                }
            }
        }
        let outline = simplify_closed(&crack_outline(&filled, lw, lh, bx, by), 0.75)
            .into_iter()
            .map(|(x, y)| (x / ww as f64, y / wh as f64))
            .collect();
        regions.push(Region {
            label: labels[comps.first[c]],
            outline,
            outer,
            inside,
        });
    }
    // What must be painted first: every region whose holes hold this one.
    let mut before: Vec<Vec<usize>> = vec![Vec::new(); regions.len()];
    for (r, region) in regions.iter().enumerate() {
        for &c in &region.inside {
            before[c].push(r);
        }
    }
    // Steps: of the regions ready to paint, all those of the colour that
    // covers most, then again.
    let mut done = vec![false; regions.len()];
    let mut steps: Vec<TraceStep> = Vec::new();
    while done.iter().any(|d| !d) {
        let ready: Vec<usize> = (0..regions.len())
            .filter(|&r| !done[r] && before[r].iter().all(|&b| done[b]))
            .collect();
        let mut by_colour = [0usize; 16];
        for &r in &ready {
            by_colour[usize::from(regions[r].label)] += regions[r].outer;
        }
        let Some((label, _)) = by_colour.iter().enumerate().max_by_key(|v| *v.1) else {
            break;
        };
        let mut chosen: Vec<usize> = ready
            .into_iter()
            .filter(|&r| usize::from(regions[r].label) == label)
            .collect();
        if chosen.is_empty() {
            break; // cannot happen: a ready region always exists
        }
        chosen.sort_by_key(|&r| std::cmp::Reverse(regions[r].outer));
        let outer: usize = chosen.iter().map(|&r| regions[r].outer).sum();
        for &r in &chosen {
            done[r] = true;
        }
        steps.push(TraceStep {
            color: colour_of[label],
            regions: chosen
                .iter()
                .map(|&r| regions[r].outline.clone())
                .filter(|o| o.len() >= 3)
                .collect(),
            share: outer as f64 / n as f64,
        });
    }
    steps.retain(|s| !s.regions.is_empty());
    let mut t = Trace {
        width: sw,
        height: sh,
        steps,
        target: Capture {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
        },
    };
    t.target = render(&t, ww as u32, wh as u32);
    t
}

/// The trace painted at `w` x `h`: what drawing every step gives.
pub fn render(t: &Trace, w: u32, h: u32) -> Capture {
    let (wu, hu) = (w.max(1) as usize, h.max(1) as usize);
    let mut rgba = vec![255u8; wu * hu * 4];
    for step in &t.steps {
        for region in &step.regions {
            let poly: Vec<Point> = region
                .iter()
                .map(|&(x, y)| Point::new(x * wu as f64, y * hu as f64))
                .collect();
            for j in 0..hu {
                let mut xs = crossings(&poly, j as f64 + 0.5, false);
                xs.sort_by(f64::total_cmp);
                for pair in xs.as_chunks::<2>().0 {
                    let i0 = (pair[0] - 0.5).ceil().max(0.0) as usize;
                    let i1 = ((pair[1] - 0.5).floor().min(wu as f64 - 1.0)).max(-1.0);
                    if i1 < 0.0 {
                        continue;
                    }
                    for i in i0..=(i1 as usize) {
                        let o = (j * wu + i) * 4;
                        rgba[o..o + 3].copy_from_slice(&step.color);
                    }
                }
            }
        }
    }
    Capture {
        width: wu as u32,
        height: hu as u32,
        rgba,
        bounds: Rect::new(0.0, 0.0, wu as f64, hu as f64),
    }
}

/// Kuwahara filter (radius 2): each pixel takes the mean of the most even
/// of the four 3 x 3 squares at its corners. Flattens texture, keeps
/// edges sharp.
fn kuwahara(px: &[[u8; 3]], w: usize, h: usize) -> Vec<[u8; 3]> {
    let at = |x: i64, y: i64| {
        px[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize]
    };
    let mut out = Vec::with_capacity(px.len());
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut best = (f32::MAX, [0u8; 3]);
            for (qx, qy) in [(-2, -2), (0, -2), (-2, 0), (0, 0)] {
                let mut sum = [0.0f32; 3];
                let mut sq = 0.0f32;
                for dy in 0..3 {
                    for dx in 0..3 {
                        let c = at(x + qx + dx, y + qy + dy);
                        for k in 0..3 {
                            let v = f32::from(c[k]);
                            sum[k] += v;
                            sq += v * v;
                        }
                    }
                }
                let mean = sum.map(|v| v / 9.0);
                let var = sq / 9.0 - mean.iter().map(|m| m * m).sum::<f32>();
                if var < best.0 {
                    best = (var, mean.map(|v| v.round() as u8));
                }
            }
            out.push(best.1);
        }
    }
    out
}

/// k colour clusters (in Lab): seeded with the most distinct of the
/// common colours, then a few rounds of k-means.
fn cluster(labs: &[[f32; 3]], k: usize) -> Vec<u8> {
    // Coarse colour bins: (count, sum).
    let mut bins: std::collections::HashMap<[i32; 3], (usize, [f32; 3])> =
        std::collections::HashMap::new();
    for p in labs {
        let key = p.map(|v| (v / 6.0).floor() as i32);
        let e = bins.entry(key).or_insert((0, [0.0; 3]));
        e.0 += 1;
        for (acc, v) in e.1.iter_mut().zip(p) {
            *acc += v;
        }
    }
    // Common enough to be a region, not a speck or a blended edge.
    let floor = (labs.len() / 300).max(2);
    let mut common: Vec<(usize, [f32; 3])> = bins
        .values()
        .map(|&(n, sum)| (n, sum.map(|v| v / n as f32)))
        .filter(|&(n, _)| n >= floor)
        .collect();
    if common.is_empty() {
        common = bins
            .values()
            .map(|&(n, sum)| (n, sum.map(|v| v / n as f32)))
            .collect();
    }
    // Deterministic order whatever the map's.
    common.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1[0].total_cmp(&b.1[0]))
            .then(a.1[1].total_cmp(&b.1[1]))
            .then(a.1[2].total_cmp(&b.1[2]))
    });
    let mut centres: Vec<[f32; 3]> = common.first().map(|c| vec![c.1]).unwrap_or_default();
    while centres.len() < k {
        let far = common
            .iter()
            .map(|c| {
                let d = centres
                    .iter()
                    .map(|s| dist2(c.1, *s))
                    .fold(f32::MAX, f32::min);
                (d, c.1)
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        match far {
            Some((d, c)) if d > 4.0 => centres.push(c),
            _ => break,
        }
    }
    let mean = |idx: &mut dyn Iterator<Item = usize>| -> Option<[f32; 3]> {
        let mut s = [0.0f32; 3];
        let mut n = 0.0f32;
        for i in idx {
            for c in 0..3 {
                s[c] += labs[i][c];
            }
            n += 1.0;
        }
        (n > 0.0).then(|| s.map(|v| v / n))
    };
    let mut labels = vec![0u8; labs.len()];
    for _ in 0..12 {
        let mut moved = false;
        for (i, p) in labs.iter().enumerate() {
            let best = (0..centres.len())
                .min_by(|&a, &b| dist2(*p, centres[a]).total_cmp(&dist2(*p, centres[b])))
                .unwrap_or(0) as u8;
            if labels[i] != best {
                labels[i] = best;
                moved = true;
            }
        }
        for (c, centre) in centres.iter_mut().enumerate() {
            if let Some(m) = mean(&mut (0..labs.len()).filter(|&i| usize::from(labels[i]) == c)) {
                *centre = m;
            }
        }
        if !moved {
            break;
        }
    }
    labels
}

/// Each pixel takes the label most common around it (3 x 3).
fn majority(labels: &[u8], w: usize, h: usize, k: usize) -> Vec<u8> {
    let mut out = labels.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut votes = [0u8; 16];
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (a, b) = (x as i64 + dx, y as i64 + dy);
                    if a >= 0 && b >= 0 && (a as usize) < w && (b as usize) < h {
                        votes[usize::from(labels[b as usize * w + a as usize])] += 1;
                    }
                }
            }
            let own = labels[y * w + x];
            let top = votes[..k].iter().copied().max().unwrap_or(0);
            if votes[usize::from(own)] < top {
                out[y * w + x] = votes.iter().position(|&v| v == top).unwrap_or(0) as u8;
            }
        }
    }
    out
}

/// Connected regions (4-neighbour) of equal labels.
struct Components {
    /// Region of each pixel.
    id: Vec<usize>,
    count: usize,
    area: Vec<usize>,
    /// A pixel of each region.
    first: Vec<usize>,
    /// Each region's box: x0, y0, x1, y1 (inclusive).
    bbox: Vec<[usize; 4]>,
}

fn components(labels: &[u8], w: usize, h: usize) -> Components {
    let mut c = Components {
        id: vec![usize::MAX; labels.len()],
        count: 0,
        area: Vec::new(),
        first: Vec::new(),
        bbox: Vec::new(),
    };
    for start in 0..labels.len() {
        if c.id[start] != usize::MAX {
            continue;
        }
        let id = c.count;
        c.count += 1;
        let mut area = 0;
        let mut b = [usize::MAX, usize::MAX, 0, 0];
        let mut queue = VecDeque::from([start]);
        c.id[start] = id;
        while let Some(p) = queue.pop_front() {
            area += 1;
            let (x, y) = (p % w, p / w);
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            let mut visit = |q: usize| {
                if c.id[q] == usize::MAX && labels[q] == labels[start] {
                    c.id[q] = id;
                    queue.push_back(q);
                }
            };
            if x > 0 {
                visit(p - 1);
            }
            if x + 1 < w {
                visit(p + 1);
            }
            if y > 0 {
                visit(p - w);
            }
            if y + 1 < h {
                visit(p + w);
            }
        }
        c.area.push(area);
        c.first.push(start);
        c.bbox.push(b);
    }
    c
}

/// Region `c` with its holes filled, over its box: (mask, box x, box y,
/// box width, box height).
fn filled_mask(
    comps: &Components,
    c: usize,
    w: usize,
    _h: usize,
) -> (Vec<bool>, usize, usize, usize, usize) {
    let [x0, y0, x1, y1] = comps.bbox[c];
    let (lw, lh) = (x1 - x0 + 1, y1 - y0 + 1);
    let own = |i: usize, j: usize| comps.id[(y0 + j) * w + x0 + i] == c;
    // Reach everything not in the region from the box's edge: what is
    // left is the region and its holes.
    let mut reached = vec![false; lw * lh];
    let mut queue = VecDeque::new();
    for j in 0..lh {
        for i in 0..lw {
            if (i == 0 || j == 0 || i + 1 == lw || j + 1 == lh) && !own(i, j) {
                reached[j * lw + i] = true;
                queue.push_back(j * lw + i);
            }
        }
    }
    while let Some(p) = queue.pop_front() {
        let (i, j) = (p % lw, p / lw);
        let mut visit = |q: usize| {
            if !reached[q] && !own(q % lw, q / lw) {
                reached[q] = true;
                queue.push_back(q);
            }
        };
        if i > 0 {
            visit(p - 1);
        }
        if i + 1 < lw {
            visit(p + 1);
        }
        if j > 0 {
            visit(p - lw);
        }
        if j + 1 < lh {
            visit(p + lw);
        }
    }
    (reached.iter().map(|r| !r).collect(), x0, y0, lw, lh)
}

/// The outer edge of a mask with no holes, along pixel edges, clockwise
/// on screen: the corner points, offset by (`ox`, `oy`).
fn crack_outline(mask: &[bool], w: usize, h: usize, ox: usize, oy: usize) -> Vec<(f64, f64)> {
    let inside = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && mask[y as usize * w + x as usize]
    };
    let Some(start) = mask.iter().position(|&m| m) else {
        return Vec::new();
    };
    let (sx, sy) = ((start % w) as i64, (start / w) as i64);
    // Directions: right, down, left, up (y down); the region on the right.
    const DIRS: [(i64, i64); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    // The pixels ahead of a corner, to the left and right of the way.
    let ahead = |vx: i64, vy: i64, d: usize| -> ((i64, i64), (i64, i64)) {
        match d {
            0 => ((vx, vy - 1), (vx, vy)),
            1 => ((vx, vy), (vx - 1, vy)),
            2 => ((vx - 1, vy), (vx - 1, vy - 1)),
            _ => ((vx - 1, vy - 1), (vx, vy - 1)),
        }
    };
    let (mut vx, mut vy, mut d) = (sx, sy, 0usize);
    let mut corners = vec![(vx, vy)];
    let limit = 4 * (w + 1) * (h + 1) + 8;
    for _ in 0..limit {
        let (left, right) = ahead(vx, vy, d);
        let nd = if !inside(right.0, right.1) {
            (d + 1) % 4
        } else if inside(left.0, left.1) {
            (d + 3) % 4
        } else {
            d
        };
        if nd != d {
            if corners.last() != Some(&(vx, vy)) {
                corners.push((vx, vy));
            }
            d = nd;
            // A turn on the spot: look again before moving.
            let (left, right) = ahead(vx, vy, d);
            if !inside(right.0, right.1) || inside(left.0, left.1) {
                continue;
            }
        }
        vx += DIRS[d].0;
        vy += DIRS[d].1;
        if (vx, vy) == (sx, sy) && d == 3 {
            break;
        }
    }
    corners
        .into_iter()
        .map(|(x, y)| ((x + ox as i64) as f64, (y + oy as i64) as f64))
        .collect()
}

/// Douglas–Peucker on a closed outline.
fn simplify_closed(pts: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    if pts.len() <= 4 {
        return pts.to_vec();
    }
    // Split at the point farthest from the first.
    let far = (1..pts.len())
        .max_by(|&a, &b| {
            let da = (pts[a].0 - pts[0].0).hypot(pts[a].1 - pts[0].1);
            let db = (pts[b].0 - pts[0].0).hypot(pts[b].1 - pts[0].1);
            da.total_cmp(&db)
        })
        .unwrap_or(pts.len() / 2);
    let mut a = simplify_open(&pts[..=far], tol);
    let mut tail: Vec<(f64, f64)> = pts[far..].to_vec();
    tail.push(pts[0]);
    let b = simplify_open(&tail, tol);
    a.pop();
    a.extend(b);
    a.pop(); // the first point again
    a
}

fn simplify_open(pts: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    let off = |p: (f64, f64)| {
        if len < 1e-9 {
            (p.0 - a.0).hypot(p.1 - a.1)
        } else {
            ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / len
        }
    };
    let (i, d) = (1..pts.len() - 1)
        .map(|i| (i, off(pts[i])))
        .fold((0, 0.0), |m, v| if v.1 > m.1 { v } else { m });
    if d <= tol {
        return vec![a, b];
    }
    let mut left = simplify_open(&pts[..=i], tol);
    let right = simplify_open(&pts[i..], tol);
    left.pop();
    left.extend(right);
    left
}

/// The largest rectangle of the picture's proportions (`w` x `h`) that
/// fits in `r`, centred.
pub fn fit_in(r: Rect, w: u32, h: u32) -> Rect {
    let aspect = f64::from(w.max(1)) / f64::from(h.max(1));
    if r.width / r.height.max(1e-9) > aspect {
        let width = r.height * aspect;
        Rect::new(r.x + (r.width - width) / 2.0, r.y, width, r.height)
    } else {
        let height = r.width / aspect;
        Rect::new(r.x, r.y + (r.height - height) / 2.0, r.width, height)
    }
}

// ---------------------------------------------------------------------------
// Comparing

/// One cell of a comparison grid: the colour the trace has there, the
/// colour found, and how different they look (ΔE).
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub col: usize,
    pub row: usize,
    pub want: [u8; 3],
    pub got: [u8; 3],
    pub delta: f64,
}

/// Compare the trace's flat picture with `area` of `cap` (capture
/// pixels), cell by cell on an `n` x `n` grid.
pub fn compare(t: &Trace, cap: &Capture, area: Rect, n: usize) -> Vec<Cell> {
    let n = n.max(1);
    let mean = |img: &Capture, r: Rect| -> Option<[u8; 3]> {
        let x0 = r.x.max(0.0).floor() as usize;
        let y0 = r.y.max(0.0).floor() as usize;
        let x1 = ((r.x + r.width).ceil() as usize).min(img.width as usize);
        let y1 = ((r.y + r.height).ceil() as usize).min(img.height as usize);
        let mut s = [0u64; 3];
        let mut count = 0u64;
        for y in y0..y1 {
            for x in x0..x1 {
                for (acc, v) in s.iter_mut().zip(rgb_at(img, x, y)) {
                    *acc += u64::from(v);
                }
                count += 1;
            }
        }
        (count > 0).then(|| s.map(|v| (v / count) as u8))
    };
    if !valid(cap) || !valid(&t.target) {
        return Vec::new();
    }
    let (tw, th) = (f64::from(t.target.width), f64::from(t.target.height));
    let mut cells = Vec::with_capacity(n * n);
    for row in 0..n {
        for col in 0..n {
            let f = |k: usize| k as f64 / n as f64;
            let want = mean(
                &t.target,
                Rect::new(f(col) * tw, f(row) * th, tw / n as f64, th / n as f64),
            );
            let got = mean(
                cap,
                Rect::new(
                    area.x + f(col) * area.width,
                    area.y + f(row) * area.height,
                    area.width / n as f64,
                    area.height / n as f64,
                ),
            );
            if let (Some(want), Some(got)) = (want, got) {
                cells.push(Cell {
                    col,
                    row,
                    want,
                    got,
                    delta: delta_e(want, got),
                });
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32, c: [u8; 3]) -> Capture {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        Capture {
            width: w,
            height: h,
            rgba,
            bounds: Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        }
    }

    fn line(cap: &mut Capture, pts: &[Point]) {
        let p: Vec<(f64, f64)> = pts.iter().map(|p| (p.x, p.y)).collect();
        crate::imaging::draw_path(cap, &p, [0, 0, 0], 2);
    }

    fn circle(cx: f64, cy: f64, r: f64) -> Vec<Point> {
        (0..=180)
            .map(|k| {
                let t = k as f64 / 180.0 * std::f64::consts::TAU;
                Point::new(cx + r * t.cos(), cy + r * t.sin())
            })
            .collect()
    }

    #[test]
    fn a_closed_shape_fills_from_one_click() {
        let mut cap = canvas(200, 200, [255, 255, 255]);
        let c = circle(100.0, 100.0, 60.0);
        line(&mut cap, &c);
        let f = fill_check(&cap, &c, &[], Point::new(100.0, 100.0)).unwrap();
        assert_eq!(f.clicks, vec![Point::new(100.0, 100.0)]);
        assert_eq!(f.leak, None);
    }

    #[test]
    fn crossing_lines_cut_a_shape_into_pieces() {
        let mut cap = canvas(200, 200, [255, 255, 255]);
        let c = circle(100.0, 100.0, 60.0);
        line(&mut cap, &c);
        // A line right across it: two halves, each needs a click.
        line(
            &mut cap,
            &[Point::new(20.0, 100.0), Point::new(180.0, 100.0)],
        );
        let f = fill_check(&cap, &c, &[], Point::new(100.0, 70.0)).unwrap();
        assert_eq!(f.clicks.len(), 2, "{f:?}");
        assert!(f.clicks.iter().any(|p| p.y < 99.0) && f.clicks.iter().any(|p| p.y > 101.0));
        assert_eq!(f.leak, None);
    }

    #[test]
    fn a_gap_in_the_outline_is_found() {
        let mut cap = canvas(200, 200, [255, 255, 255]);
        let c = circle(100.0, 100.0, 60.0);
        // Leave the last tenth of the circle out.
        line(&mut cap, &c[..160]);
        let f = fill_check(&cap, &c, &[], Point::new(100.0, 100.0)).unwrap();
        let leak = f.leak.expect("the gap");
        // The gap is on the right, a little above the start of the circle.
        assert!(leak.x > 140.0 && leak.y < 100.0, "{leak:?}");
    }

    #[test]
    fn holes_and_earlier_paint_are_not_pieces() {
        let mut cap = canvas(200, 200, [255, 255, 255]);
        let outer = circle(100.0, 100.0, 70.0);
        let inner = circle(100.0, 100.0, 30.0);
        line(&mut cap, &outer);
        line(&mut cap, &inner);
        // A blob of earlier paint in the ring: not a piece to click.
        for y in 40..50 {
            for x in 95..105 {
                let o = (y * 200 + x) * 4;
                cap.rgba[o..o + 3].copy_from_slice(&[200, 30, 30]);
            }
        }
        let f = fill_check(&cap, &outer, &[&inner], Point::new(100.0, 150.0)).unwrap();
        assert_eq!(f.clicks.len(), 1, "{f:?}");
        assert_eq!(f.leak, None);
    }

    #[test]
    fn shapes_at_the_edge_are_not_judged() {
        let cap = canvas(100, 100, [255, 255, 255]);
        let c = circle(50.0, 50.0, 49.0);
        assert_eq!(fill_check(&cap, &c, &[], Point::new(50.0, 50.0)), None);
    }

    /// A picture: a sky, a sun and a ground, with a dark ring round the sun.
    fn scene() -> Capture {
        let mut cap = canvas(300, 200, [120, 180, 240]);
        for y in 0..200usize {
            for x in 0..300usize {
                let o = (y * 300 + x) * 4;
                let d = ((x as f64 - 200.0).powi(2) + (y as f64 - 60.0).powi(2)).sqrt();
                let c = if y >= 140 {
                    [60, 140, 50]
                } else if d < 25.0 {
                    [250, 220, 40]
                } else if d < 32.0 {
                    [90, 60, 20]
                } else {
                    [120, 180, 240]
                };
                cap.rgba[o..o + 3].copy_from_slice(&c);
            }
        }
        cap
    }

    #[test]
    fn a_picture_becomes_flat_colours_painted_back_to_front() {
        let t = trace(&scene(), 4, Detail::MEDIUM);
        assert_eq!((t.width, t.height), (300, 200));
        let colours: Vec<String> = t.steps.iter().map(|s| hex(s.color)).collect();
        // The sky first (it holds the sun's ring), the ring before the sun
        // inside it; the ground whenever.
        let pos = |want: [u8; 3]| {
            t.steps
                .iter()
                .position(|s| delta_e(s.color, want) < 8.0)
                .unwrap_or_else(|| panic!("{want:?} not in {colours:?}"))
        };
        let (sky, ring, sun, ground) = (
            pos([120, 180, 240]),
            pos([90, 60, 20]),
            pos([250, 220, 40]),
            pos([60, 140, 50]),
        );
        assert_eq!(sky, 0, "{colours:?}");
        assert!(ring < sun, "{colours:?}");
        assert!(ground > 0);
        // The sky's outline is the whole picture; the sun is round about
        // its middle.
        let sun_region = &t.steps[sun].regions[0];
        let (mut x0, mut x1) = (1.0f64, 0.0f64);
        for &(x, _) in sun_region {
            (x0, x1) = (x0.min(x), x1.max(x));
        }
        assert!(
            (x0 * 300.0 - 175.0).abs() < 6.0 && (x1 * 300.0 - 225.0).abs() < 6.0,
            "{x0} {x1}"
        );
        // Painting the steps gives back the picture.
        let flat = render(&t, 300, 200);
        let cells = compare(&t, &scene(), Rect::new(0.0, 0.0, 300.0, 200.0), 6);
        assert_eq!(cells.len(), 36);
        assert!(cells.iter().all(|c| c.delta < 12.0), "{cells:?}");
        assert!(delta_e(rgb_at(&flat, 200, 60), [250, 220, 40]) < 8.0);
        assert!(delta_e(rgb_at(&flat, 10, 190), [60, 140, 50]) < 8.0);
    }

    #[test]
    fn comparing_finds_where_the_canvas_differs() {
        let t = trace(&scene(), 4, Detail::LOW);
        // A blank canvas: the cells differ everywhere, most where the
        // ground should be dark green.
        let blank = canvas(300, 200, [255, 255, 255]);
        let cells = compare(&t, &blank, Rect::new(0.0, 0.0, 300.0, 200.0), 4);
        let worst = cells
            .iter()
            .max_by(|a, b| a.delta.total_cmp(&b.delta))
            .unwrap();
        assert_eq!(worst.row, 3, "{worst:?}");
        assert_eq!(hex(worst.got), "#FFFFFF");
    }

    #[test]
    fn outlines_follow_pixel_edges_and_simplify() {
        // A 3 x 2 block in a 5 x 4 mask.
        let mut mask = vec![false; 20];
        for y in 1..3 {
            for x in 1..4 {
                mask[y * 5 + x] = true;
            }
        }
        let o = crack_outline(&mask, 5, 4, 10, 20);
        assert_eq!(
            o,
            vec![(11.0, 21.0), (14.0, 21.0), (14.0, 23.0), (11.0, 23.0)]
        );
        // An L shape keeps its six corners.
        let mut l = vec![false; 16];
        for (x, y) in [(0, 0), (0, 1), (0, 2), (1, 2), (2, 2)] {
            l[y * 4 + x] = true;
        }
        assert_eq!(crack_outline(&l, 4, 4, 0, 0).len(), 6);
        // A staircase simplifies to its diagonal.
        let stairs: Vec<(f64, f64)> = (0..10)
            .flat_map(|k| [(k as f64, k as f64), (k as f64 + 1.0, k as f64)])
            .chain([(10.0, 10.0), (0.0, 10.0)])
            .collect();
        assert!(simplify_closed(&stairs, 0.75).len() <= 4);
    }
}
