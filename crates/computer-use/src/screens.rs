//! Screen memory: recognising views the model has already seen.
//!
//! Every view the model is shown is kept as a compact [`View`] — per element
//! only hashes, its index and its rendered line. When the app later shows a
//! view that matches a remembered one (the model went back a page, closed a
//! dialog, re-opened a panel), the engine restores that screen's element
//! indices and reports only what differs from what the model already saw,
//! instead of a fresh tree and screenshot to re-analyse.
//!
//! [`PixelSig`] is the pixel-level counterpart: a small luminance grid of a
//! screenshot, so an unchanged image is not encoded and sent again.

use std::collections::{HashMap, HashSet};

use crate::imaging::CoordMap;
use crate::tree::{Diff, IndexAllocator, Node, hash_str};
use crate::types::Capture;

/// One element of a remembered view.
#[derive(Debug, Clone, Copy)]
struct Entry {
    key: u64,
    shape: u64,
    line: u64,
    index: u32,
    /// Byte range of the rendered line in [`View::text`].
    start: u32,
    len: u32,
}

/// What the model was shown of one screen, compactly.
#[derive(Debug, Default, Clone)]
pub struct View {
    entries: Vec<Entry>,
    text: String,
}

impl View {
    pub fn from_nodes(nodes: &[Node]) -> Self {
        let mut text = String::with_capacity(nodes.iter().map(|n| n.line.len()).sum());
        let entries = nodes
            .iter()
            .map(|n| {
                let start = text.len() as u32;
                text.push_str(&n.line);
                Entry {
                    key: n.key,
                    shape: n.shape,
                    line: hash_str(&n.line),
                    index: n.index,
                    start,
                    len: n.line.len() as u32,
                }
            })
            .collect();
        Self { entries, text }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Approximate heap size, for the memory budget.
    pub fn bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<Entry>() + self.text.capacity()
    }

    fn line(&self, e: &Entry) -> &str {
        &self.text[e.start as usize..(e.start + e.len) as usize]
    }

    /// Jaccard similarity (0–1) between this view's shapes and `shapes`.
    pub fn similarity(&self, shapes: &HashSet<u64>) -> f64 {
        if self.entries.is_empty() && shapes.is_empty() {
            return 1.0;
        }
        let mine: HashSet<u64> = self.entries.iter().map(|e| e.shape).collect();
        let common = mine.intersection(shapes).count();
        let union = mine.len() + shapes.len() - common;
        common as f64 / union.max(1) as f64
    }

    /// For each live node, the remembered entry it corresponds to: same
    /// identity key first, else the same (unambiguous) structural shape.
    fn matches(&self, nodes: &[Node]) -> Vec<Option<usize>> {
        let mut by_key: HashMap<u64, usize> = HashMap::with_capacity(self.entries.len());
        let mut by_shape: HashMap<u64, Option<usize>> = HashMap::with_capacity(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            by_key.insert(e.key, i);
            by_shape
                .entry(e.shape)
                .and_modify(|v| *v = None)
                .or_insert(Some(i));
        }
        let mut used = vec![false; self.entries.len()];
        let mut out = vec![None; nodes.len()];
        for (p, n) in nodes.iter().enumerate() {
            if let Some(&i) = by_key.get(&n.key)
                && !used[i]
            {
                out[p] = Some(i);
                used[i] = true;
            }
        }
        for (p, n) in nodes.iter().enumerate() {
            if out[p].is_none()
                && let Some(Some(i)) = by_shape.get(&n.shape).copied()
                && !used[i]
            {
                out[p] = Some(i);
                used[i] = true;
            }
        }
        out
    }

    /// What changed between this view and the live nodes.
    pub fn diff(&self, nodes: &[Node]) -> Diff {
        let m = self.matches(nodes);
        let mut used = vec![false; self.entries.len()];
        let mut d = Diff::default();
        for (pos, (n, hit)) in nodes.iter().zip(&m).enumerate() {
            match hit {
                None => d.added.push(pos),
                Some(i) => {
                    used[*i] = true;
                    let e = &self.entries[*i];
                    if e.line != hash_str(&n.line) || e.index != n.index {
                        d.changed.push((pos, self.line(e).to_string()));
                    }
                }
            }
        }
        for (i, e) in self.entries.iter().enumerate() {
            if !used[i] {
                d.removed.push((e.index, self.line(e).to_string()));
            }
        }
        d
    }

    /// Give live nodes the indices they had in this view.
    pub fn restore_indices(&self, alloc: &mut IndexAllocator, nodes: &[Node]) {
        for (n, hit) in nodes.iter().zip(self.matches(nodes)) {
            if let Some(i) = hit {
                alloc.pin(n.key, self.entries[i].index);
            }
        }
    }
}

/// A screen the model has seen.
#[derive(Debug, Clone)]
pub struct Screen {
    /// Number shown to the model ("screen #3").
    pub id: u32,
    pub pid: u32,
    pub window: u64,
    /// Window size when last seen (a different size invalidates the old
    /// screenshot's coordinates).
    pub size: Option<(f64, f64)>,
    /// Window origin when last seen.
    pub origin: Option<(f64, f64)>,
    pub view: View,
    /// Coordinate map of the last screenshot the model received of it.
    pub coord: Option<CoordMap>,
    /// Pixel signature of that screenshot.
    pub pixels: Option<PixelSig>,
    /// A screenshot was attempted for this screen.
    pub shot: bool,
    /// The number of that screenshot ("screenshot #7"): what x/y and a
    /// later changed part refer to.
    pub shot_id: Option<u32>,
    /// LRU clock.
    used: u64,
}

impl Screen {
    pub fn new(id: u32, pid: u32, window: u64, view: View) -> Self {
        Self {
            id,
            pid,
            window,
            size: None,
            origin: None,
            view,
            coord: None,
            pixels: None,
            shot: false,
            shot_id: None,
            used: 0,
        }
    }

    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.view.bytes()
            + self.pixels.as_ref().map_or(0, |p| p.cells.capacity())
    }
}

/// Screens the model has seen and moved away from, least recently used
/// first out.
#[derive(Debug, Default)]
pub struct ScreenMemory {
    screens: Vec<Screen>,
    tick: u64,
    next_id: u32,
}

impl ScreenMemory {
    /// A number for a screen seen for the first time.
    pub fn new_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    pub fn len(&self) -> usize {
        self.screens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.screens.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.screens.iter().map(Screen::bytes).sum()
    }

    /// Store a screen (replacing an older copy), then evict least recently
    /// used screens beyond `max_screens` / `max_bytes`.
    pub fn remember(&mut self, mut screen: Screen, max_screens: usize, max_bytes: usize) {
        self.tick += 1;
        screen.used = self.tick;
        self.screens.retain(|s| s.id != screen.id);
        if max_screens == 0 {
            return;
        }
        self.screens.push(screen);
        while self.screens.len() > max_screens
            || (self.screens.len() > 1 && self.bytes() > max_bytes)
        {
            let oldest = self
                .screens
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.used)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.screens.swap_remove(oldest);
        }
    }

    pub fn get(&self, id: u32) -> Option<&Screen> {
        self.screens.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Screen> {
        self.screens.iter_mut().find(|s| s.id == id)
    }

    /// Remove and return a screen (it becomes the current one again).
    pub fn take(&mut self, id: u32) -> Option<Screen> {
        let pos = self.screens.iter().position(|s| s.id == id)?;
        Some(self.screens.swap_remove(pos))
    }

    /// The remembered screen of `pid` most similar to `shapes`, if at least
    /// `threshold` similar. Ties prefer the same window.
    pub fn best_match(
        &self,
        pid: u32,
        window: u64,
        shapes: &HashSet<u64>,
        threshold: f64,
    ) -> Option<(u32, f64)> {
        let mut best: Option<(u32, f64, bool)> = None;
        for s in self.screens.iter().filter(|s| s.pid == pid) {
            let sim = s.view.similarity(shapes);
            if sim < threshold {
                continue;
            }
            let same = s.window == window;
            let better = match best {
                None => true,
                Some((_, b, bsame)) => {
                    sim > b + 1e-9 || ((sim - b).abs() <= 1e-9 && same && !bsame)
                }
            };
            if better {
                best = Some((s.id, sim, same));
            }
        }
        best.map(|(id, sim, _)| (id, sim))
    }

    /// Forget screens of apps that are no longer running.
    pub fn retain_pids(&mut self, live: &HashSet<u32>) {
        self.screens.retain(|s| live.contains(&s.pid));
    }

    pub fn clear(&mut self) {
        self.screens.clear();
    }
}

/// A coarse luminance grid of a screenshot: equal signatures mean the model
/// already has this picture.
#[derive(Debug, Clone, PartialEq)]
pub struct PixelSig {
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
    cells: Vec<u8>,
}

impl PixelSig {
    /// Average luminance over a grid `grid` cells across the longer side.
    pub fn of(cap: &Capture, grid: u32) -> Self {
        let (w, h) = (cap.width.max(1), cap.height.max(1));
        let grid = grid.clamp(4, 256);
        let (cols, rows) = if w >= h {
            (grid.min(w), ((grid * h).div_ceil(w)).clamp(1, h))
        } else {
            (((grid * w).div_ceil(h)).clamp(1, w), grid.min(h))
        };
        let col_of: Vec<u32> = (0..w).map(|x| x * cols / w).collect();
        let mut sums = vec![0u64; (cols * rows) as usize];
        let mut counts = vec![0u32; (cols * rows) as usize];
        let stride = (cap.width * 4) as usize;
        // Tolerate a short buffer (it will fail to encode anyway).
        let full_rows = cap
            .rgba
            .len()
            .checked_div(stride)
            .unwrap_or(0)
            .min(cap.height as usize) as u32;
        for y in 0..full_rows {
            let row = (y * rows / h) * cols;
            let line = &cap.rgba[y as usize * stride..(y as usize + 1) * stride];
            for (x, px) in line.as_chunks::<4>().0.iter().enumerate() {
                // Rec. 601 luma, integer.
                let l = (299 * u32::from(px[0]) + 587 * u32::from(px[1]) + 114 * u32::from(px[2]))
                    / 1000;
                let c = (row + col_of[x]) as usize;
                sums[c] += u64::from(l);
                counts[c] += 1;
            }
        }
        let cells = sums
            .iter()
            .zip(&counts)
            .map(|(s, c)| (s / u64::from((*c).max(1))) as u8)
            .collect();
        Self {
            width: cap.width,
            height: cap.height,
            cols,
            rows,
            cells,
        }
    }

    /// The part of the picture that differs from `other` (same size), as a
    /// pixel rectangle of the capture: (x, y, width, height). `None` when
    /// nothing differs or the pictures can't be compared.
    pub fn changed_area(&self, other: &PixelSig, tolerance: u8) -> Option<(u32, u32, u32, u32)> {
        if (self.width, self.height, self.cols, self.rows)
            != (other.width, other.height, other.cols, other.rows)
        {
            return None;
        }
        let (mut c0, mut r0, mut c1, mut r1) = (u32::MAX, u32::MAX, 0, 0);
        for (i, (a, b)) in self.cells.iter().zip(&other.cells).enumerate() {
            if a.abs_diff(*b) > tolerance {
                let (c, r) = (i as u32 % self.cols, i as u32 / self.cols);
                (c0, r0, c1, r1) = (c0.min(c), r0.min(r), c1.max(c), r1.max(r));
            }
        }
        if c0 == u32::MAX {
            return None;
        }
        let x0 = c0 * self.width / self.cols;
        let y0 = r0 * self.height / self.rows;
        let x1 = ((c1 + 1) * self.width).div_ceil(self.cols).min(self.width);
        let y1 = ((r1 + 1) * self.height)
            .div_ceil(self.rows)
            .min(self.height);
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    /// Same picture, allowing each cell's average to drift by `tolerance`.
    pub fn same_as(&self, other: &PixelSig, tolerance: u8) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.cols == other.cols
            && self.rows == other.rows
            && self
                .cells
                .iter()
                .zip(&other.cells)
                .all(|(a, b)| a.abs_diff(*b) <= tolerance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TreeConfig;
    use crate::tree::prune;
    use crate::types::{NodeStates, RawNode, Rect};

    fn raw(items: &[(&str, &str, Option<&str>)]) -> Vec<RawNode> {
        let mut v = vec![RawNode {
            role: "window".into(),
            name: Some("W".into()),
            bounds: Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
            states: NodeStates {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        }];
        for (role, name, key) in items {
            v.push(RawNode {
                parent: Some(0),
                role: role.to_string(),
                name: Some(name.to_string()),
                key: key.map(String::from),
                bounds: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
                states: NodeStates {
                    enabled: true,
                    ..Default::default()
                },
                ..Default::default()
            });
        }
        v
    }

    fn nodes(items: &[(&str, &str, Option<&str>)], alloc: &mut IndexAllocator) -> Vec<Node> {
        let mut n = prune(&raw(items), None, &TreeConfig::default()).nodes;
        alloc.assign_stable(&mut n);
        n
    }

    #[test]
    fn view_diff_and_similarity() {
        let mut alloc = IndexAllocator::default();
        let a = nodes(
            &[("button", "OK", None), ("button", "Cancel", None)],
            &mut alloc,
        );
        let view = View::from_nodes(&a);
        assert!(view.diff(&a).is_empty());
        let shapes: HashSet<u64> = a.iter().map(|n| n.shape).collect();
        assert!((view.similarity(&shapes) - 1.0).abs() < 1e-9);

        let b = nodes(
            &[("button", "OK", None), ("button", "Apply", None)],
            &mut alloc,
        );
        let d = view.diff(&b);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].1, "button \"Cancel\"");
    }

    #[test]
    fn restores_indices_by_shape_when_keys_change() {
        // Backend identities differ (a dialog opened again) but the layout is
        // the same: the old indices come back.
        let mut alloc = IndexAllocator::default();
        let a = nodes(
            &[
                ("button", "OK", Some("/a/1")),
                ("button", "Cancel", Some("/a/2")),
            ],
            &mut alloc,
        );
        let view = View::from_nodes(&a);
        let mut alloc2 = IndexAllocator::default();
        alloc2.pin(0, 50); // numbers already handed out elsewhere
        let mut b = prune(
            &raw(&[
                ("button", "OK", Some("/b/7")),
                ("button", "Cancel", Some("/b/8")),
            ]),
            None,
            &TreeConfig::default(),
        )
        .nodes;
        view.restore_indices(&mut alloc2, &b);
        alloc2.assign_stable(&mut b);
        let idx = |ns: &[Node], name: &str| {
            ns.iter()
                .find(|n| n.name.as_deref() == Some(name))
                .unwrap()
                .index
        };
        assert_eq!(idx(&a, "OK"), idx(&b, "OK"));
        assert_eq!(idx(&a, "Cancel"), idx(&b, "Cancel"));
        assert!(view.diff(&b).is_empty());
    }

    #[test]
    fn memory_matches_and_evicts() {
        let mut alloc = IndexAllocator::default();
        let a = nodes(
            &[("button", "One", None), ("button", "Two", None)],
            &mut alloc,
        );
        let b = nodes(&[("tab", "X", None), ("tab", "Y", None)], &mut alloc);
        let mut mem = ScreenMemory::default();
        let (ia, ib) = (mem.new_id(), mem.new_id());
        mem.remember(Screen::new(ia, 1, 10, View::from_nodes(&a)), 8, usize::MAX);
        mem.remember(Screen::new(ib, 1, 10, View::from_nodes(&b)), 8, usize::MAX);
        let shapes: HashSet<u64> = a.iter().map(|n| n.shape).collect();
        assert_eq!(mem.best_match(1, 10, &shapes, 0.8).map(|m| m.0), Some(ia));
        assert_eq!(mem.best_match(2, 10, &shapes, 0.8), None, "other app");
        // Capacity 1: the least recently used goes.
        let ic = mem.new_id();
        mem.remember(Screen::new(ic, 1, 10, View::from_nodes(&a)), 1, usize::MAX);
        assert_eq!(mem.len(), 1);
        assert!(mem.get(ic).is_some());
        assert!(mem.take(ic).is_some());
        assert!(mem.is_empty());
    }

    #[test]
    fn pixel_signature_detects_changes() {
        let mut cap = Capture {
            width: 200,
            height: 100,
            rgba: vec![255; 200 * 100 * 4],
            bounds: Rect::new(0.0, 0.0, 200.0, 100.0),
        };
        let a = PixelSig::of(&cap, 64);
        assert!(a.same_as(&PixelSig::of(&cap, 64), 0));
        // Darken a 3x3 patch (a small glyph change).
        for y in 40..43 {
            for x in 100..103 {
                let i = (y * 200 + x) * 4;
                cap.rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let b = PixelSig::of(&cap, 64);
        assert!(!a.same_as(&b, 2));

        // Degenerate captures don't panic.
        let empty = Capture {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
        };
        let _ = PixelSig::of(&empty, 64);
        let short = Capture {
            width: 10,
            height: 10,
            rgba: vec![0; 40],
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
        };
        let _ = PixelSig::of(&short, 64);
    }
}
