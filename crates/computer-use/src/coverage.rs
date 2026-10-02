//! Where a window's accessibility tree says nothing.
//!
//! How many elements a window has doesn't tell whether the part a task
//! needs is described: a toolbar full of buttons hides a canvas the tree
//! knows nothing about. This finds the areas no element with text, a value
//! or an action covers (a canvas, a picture, a custom-drawn panel) and that
//! show something (not empty background), so the engine can read their text
//! off the screen and look at their pixels ([ocr] blind_regions).

use crate::roles;
use crate::types::{Capture, RawNode, Rect};

/// Smallest blind area worth reading: this share of the window…
const MIN_SHARE: f64 = 0.05;
/// …and at least this many screen pixels on each side.
const MIN_SIDE: f64 = 48.0;
/// An element this big (share of the window) doesn't cover its area on its
/// own unless it is editable: a container's or a canvas's name says nothing
/// about what is drawn in it.
const BIG_SHARE: f64 = 0.2;
/// Cells of the coverage grid across the window's longer side.
const GRID: f64 = 96.0;

fn has_text(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(|t| !t.trim().is_empty())
}

/// An element that tells the model what is in its area.
fn informative(n: &RawNode) -> bool {
    has_text(&n.name)
        || has_text(&n.value)
        || has_text(&n.description)
        || n.states.editable
        || roles::is_interactive(&n.role)
        || n.actions
            .iter()
            .any(|a| !matches!(a.name.as_str(), "scroll_to_visible" | "show_menu" | "raise"))
}

/// A part of a window no informative element covers.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    /// Its box (screen coordinates), without edges it barely reaches into.
    pub rect: Rect,
    /// The grid cells it is made of: only these are looked at for content
    /// (its box can take in the elements it runs around).
    pub cells: Vec<Rect>,
}

/// Areas of `window` that no informative element covers, biggest first.
///
/// Two kinds: an element with nothing informative in or under it that is
/// big enough (a drawing area, a canvas, a picture, an empty panel: what
/// it shows only the pixels have), and, for toolkits that don't expose
/// such an element at all, what is left of the window that no element
/// covers. Hidden elements and roots (the window, a menu bar) cover
/// nothing; nor does an element as big as a fifth of the window unless it
/// is editable (its children cover their parts).
pub fn uncovered(raw: &[RawNode], window: Rect) -> Vec<Area> {
    if window.width < MIN_SIDE || window.height < MIN_SIDE {
        return Vec::new();
    }
    let area = window.width * window.height;
    let cell = (window.width.max(window.height) / GRID).max(4.0);
    let cols = (window.width / cell).ceil() as usize;
    let rows = (window.height / cell).ceil() as usize;
    // The grid cells a screen rectangle touches (or lies on, `inner`).
    let span = |b: Rect, inner: bool| {
        let (lo, hi) = if inner {
            (f64::ceil as fn(f64) -> f64, f64::floor as fn(f64) -> f64)
        } else {
            (f64::floor as fn(f64) -> f64, f64::ceil as fn(f64) -> f64)
        };
        let x0 = lo((b.x - window.x) / cell).max(0.0) as usize;
        let y0 = lo((b.y - window.y) / cell).max(0.0) as usize;
        let x1 = (hi((b.x + b.width - window.x) / cell).max(0.0) as usize).min(cols);
        let y1 = (hi((b.y + b.height - window.y) / cell).max(0.0) as usize).min(rows);
        (x0, y0, x1.max(x0), y1.max(y0))
    };
    let visible = |n: &RawNode| {
        n.parent.is_some() && !n.states.hidden && n.bounds.is_some_and(|b| !b.is_empty())
    };
    let mut covered = vec![false; cols * rows];
    for n in raw.iter().filter(|n| visible(n) && informative(n)) {
        let b = n.bounds.expect("visible");
        if b.width * b.height >= BIG_SHARE * area && !n.states.editable {
            continue;
        }
        let (x0, y0, x1, y1) = span(b, false);
        for y in y0..y1 {
            for x in x0..x1 {
                covered[y * cols + x] = true;
            }
        }
    }
    let to_rect = |x: usize, y: usize| {
        Rect::new(
            window.x + x as f64 * cell,
            window.y + y as f64 * cell,
            cell,
            cell,
        )
    };
    let big_enough = |count: usize, r: Rect| {
        count as f64 * cell * cell >= MIN_SHARE * area
            && r.width >= MIN_SIDE
            && r.height >= MIN_SIDE
    };
    let mut out: Vec<(usize, Area)> = Vec::new();
    // Cells already in an area.
    let mut seen = covered.clone();

    // Elements with nothing informative in or under them, deepest first (a
    // canvas inside an otherwise empty scroll area is the canvas).
    let mut inside = vec![false; raw.len()];
    for (i, n) in raw.iter().enumerate().rev() {
        if let Some(p) = n.parent.filter(|p| *p < i) {
            inside[p] |= inside[i] || informative(n);
        }
    }
    for (i, n) in raw.iter().enumerate().rev() {
        if !visible(n) || inside[i] || informative(n) {
            continue;
        }
        let Some(b) = n.bounds.and_then(|b| clip(b, window)) else {
            continue;
        };
        let (x0, y0, x1, y1) = span(b, true);
        let mut members = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                if !seen[y * cols + x] {
                    members.push((x, y));
                }
            }
        }
        if !big_enough(members.len(), b) {
            continue;
        }
        for &(x, y) in &members {
            seen[y * cols + x] = true;
        }
        let cells = members.iter().map(|&(x, y)| to_rect(x, y)).collect();
        out.push((members.len(), Area { rect: b, cells }));
    }

    // What is left: connected areas of uncovered cells.
    let mut stack = Vec::new();
    for start in 0..cols * rows {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut members = Vec::new();
        while let Some(i) = stack.pop() {
            let (x, y) = (i % cols, i / cols);
            members.push((x, y));
            let mut push = |j: usize| {
                if !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < cols {
                push(i + 1);
            }
            if y > 0 {
                push(i - cols);
            }
            if y + 1 < rows {
                push(i + cols);
            }
        }
        let (cx0, cy0, cx1, cy1) = tight_box(&members);
        let r = clip(
            Rect::new(
                window.x + cx0 as f64 * cell,
                window.y + cy0 as f64 * cell,
                (cx1 - cx0) as f64 * cell,
                (cy1 - cy0) as f64 * cell,
            ),
            window,
        );
        if let Some(r) = r
            && big_enough(members.len(), r)
        {
            let cells = members.iter().map(|&(x, y)| to_rect(x, y)).collect();
            out.push((members.len(), Area { rect: r, cells }));
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.into_iter().map(|(_, a)| a).collect()
}

/// `r` cut to `bounds`, if anything is left.
fn clip(r: Rect, bounds: Rect) -> Option<Rect> {
    let x0 = r.x.max(bounds.x);
    let y0 = r.y.max(bounds.y);
    let x1 = (r.x + r.width).min(bounds.x + bounds.width);
    let y1 = (r.y + r.height).min(bounds.y + bounds.height);
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// The box of an area's cells, without edge rows and columns it barely
/// reaches into (the gap at the end of a toolbar joins the canvas below
/// it, but isn't part of it).
fn tight_box(cells: &[(usize, usize)]) -> (usize, usize, usize, usize) {
    let mut b = (usize::MAX, usize::MAX, 0, 0);
    for &(x, y) in cells {
        b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1));
    }
    let (ox, oy, gw) = (b.0, b.1, b.2 - b.0);
    let mut grid = vec![false; gw * (b.3 - b.1)];
    for &(x, y) in cells {
        grid[(y - oy) * gw + (x - ox)] = true;
    }
    let at = |x: usize, y: usize| grid[(y - oy) * gw + (x - ox)];
    loop {
        let (x0, y0, x1, y1) = b;
        let in_row = |y: usize| (x0..x1).filter(|&x| at(x, y)).count();
        let in_col = |x: usize| (y0..y1).filter(|&y| at(x, y)).count();
        let (w, h) = (x1 - x0, y1 - y0);
        if h > 1 && in_row(y0) * 2 < w {
            b.1 += 1;
        } else if h > 1 && in_row(y1 - 1) * 2 < w {
            b.3 -= 1;
        } else if w > 1 && in_col(x0) * 2 < h {
            b.0 += 1;
        } else if w > 1 && in_col(x1 - 1) * 2 < h {
            b.2 -= 1;
        } else {
            return b;
        }
    }
}

/// The pixel rectangle of a capture showing the screen area `r` (None when
/// it is outside the capture).
pub fn pixels_of(cap: &Capture, r: Rect) -> Option<(u32, u32, u32, u32)> {
    let sx = f64::from(cap.width) / cap.bounds.width.max(1e-9);
    let sy = f64::from(cap.height) / cap.bounds.height.max(1e-9);
    let x0 = ((r.x - cap.bounds.x) * sx).floor().max(0.0);
    let y0 = ((r.y - cap.bounds.y) * sy).floor().max(0.0);
    let x1 = ((r.x + r.width - cap.bounds.x) * sx)
        .ceil()
        .min(f64::from(cap.width));
    let y1 = ((r.y + r.height - cap.bounds.y) * sy)
        .ceil()
        .min(f64::from(cap.height));
    (x1 > x0 + 1.0 && y1 > y0 + 1.0).then_some((
        x0 as u32,
        y0 as u32,
        (x1 - x0) as u32,
        (y1 - y0) as u32,
    ))
}

/// Whether an area shows something in a capture: edges (text, lines,
/// shapes) in enough of its cells, not empty background or a gentle
/// gradient.
pub fn shows_something(cap: &Capture, area: &Area) -> bool {
    const EDGE: u8 = 40;
    let lum = |x: u32, y: u32| -> u8 {
        let i = (y as usize * cap.width as usize + x as usize) * 4;
        let (r, g, b) = (
            u32::from(cap.rgba[i]),
            u32::from(cap.rgba[i + 1]),
            u32::from(cap.rgba[i + 2]),
        );
        ((r * 299 + g * 587 + b * 114) / 1000) as u8
    };
    let (mut cells, mut busy) = (0usize, 0usize);
    for cell in &area.cells {
        let Some((x0, y0, w, h)) = pixels_of(cap, *cell) else {
            continue;
        };
        let (mut lo, mut hi) = (u8::MAX, 0u8);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let l = lum(x, y);
                lo = lo.min(l);
                hi = hi.max(l);
            }
        }
        cells += 1;
        if hi.saturating_sub(lo) > EDGE {
            busy += 1;
        }
    }
    busy >= 8 && busy * 100 >= cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ActionDesc, NodeStates};

    fn node(parent: Option<usize>, role: &str, name: &str, b: Rect) -> RawNode {
        RawNode {
            handle: 0,
            parent,
            role: role.into(),
            name: (!name.is_empty()).then(|| name.to_string()),
            bounds: Some(b),
            ..RawNode::default()
        }
    }

    #[test]
    fn a_canvas_under_a_full_toolbar_is_blind() {
        let win = Rect::new(0.0, 0.0, 1000.0, 600.0);
        let mut raw = vec![node(None, "window", "Board", win)];
        // 24 buttons along the top: plenty of elements.
        for i in 0..24 {
            raw.push(node(
                Some(0),
                "button",
                &format!("Tool {i}"),
                Rect::new(f64::from(i) * 40.0, 0.0, 38.0, 30.0),
            ));
        }
        // The canvas: a big, unnamed drawing area.
        raw.push(node(
            Some(0),
            "drawing area",
            "",
            Rect::new(0.0, 40.0, 1000.0, 560.0),
        ));
        let blind = uncovered(&raw, win);
        assert_eq!(blind.len(), 1, "{blind:?}");
        let b = blind[0].rect;
        assert!(b.y <= 45.0 && b.y >= 30.0 && b.height > 500.0, "{b:?}");
        // A named canvas is just as blind: its name doesn't say what's drawn.
        raw.last_mut().unwrap().name = Some("Canvas".into());
        assert_eq!(uncovered(&raw, win).len(), 1);
    }

    #[test]
    fn a_painted_notice_is_its_own_area_however_much_space_is_around_it() {
        let win = Rect::new(0.0, 0.0, 560.0, 260.0);
        let raw = vec![
            node(None, "window", "Orders", win),
            node(Some(0), "group", "", Rect::new(12.0, 12.0, 536.0, 236.0)),
            node(
                Some(1),
                "drawing area",
                "",
                Rect::new(12.0, 12.0, 536.0, 90.0),
            ),
            node(
                Some(1),
                "text field",
                "Order number",
                Rect::new(110.0, 110.0, 300.0, 30.0),
            ),
            node(
                Some(1),
                "button",
                "Submit",
                Rect::new(110.0, 150.0, 80.0, 30.0),
            ),
        ];
        let areas = uncovered(&raw, win);
        let notice = &areas
            .iter()
            .find(|a| a.rect == Rect::new(12.0, 12.0, 536.0, 90.0))
            .expect("the drawing area");
        // Its own cells, not the margins around it.
        assert!(
            notice
                .cells
                .iter()
                .all(|c| c.y >= 12.0 && c.y + c.height <= 102.0 + 1e-9)
        );
    }

    #[test]
    fn a_described_window_is_not_blind() {
        let win = Rect::new(100.0, 100.0, 400.0, 300.0);
        let mut raw = vec![node(None, "window", "Form", win)];
        for row in 0..6 {
            for col in 0..2 {
                raw.push(node(
                    Some(0),
                    if col == 0 { "text" } else { "text field" },
                    "Field",
                    Rect::new(
                        100.0 + f64::from(col) * 200.0,
                        100.0 + f64::from(row) * 50.0,
                        200.0,
                        50.0,
                    ),
                ));
            }
        }
        assert!(uncovered(&raw, win).is_empty());
        // A big editable text area covers itself.
        let raw = vec![
            node(None, "window", "Editor", win),
            RawNode {
                states: NodeStates {
                    editable: true,
                    ..NodeStates::default()
                },
                actions: vec![ActionDesc::new("press", "activate")],
                ..node(Some(0), "text area", "", win)
            },
        ];
        assert!(uncovered(&raw, win).is_empty());
    }

    #[test]
    fn empty_background_shows_nothing_and_text_does() {
        let mut cap = Capture {
            width: 200,
            height: 100,
            rgba: vec![240; 200 * 100 * 4],
            bounds: Rect::new(0.0, 0.0, 200.0, 100.0),
        };
        // Cells of 10 px, as the grid cuts a window.
        let grid = |y0: u32, y1: u32| -> Vec<Rect> {
            (y0..y1)
                .flat_map(|y| {
                    (0..20).map(move |x| {
                        Rect::new(f64::from(x) * 10.0, f64::from(y) * 10.0, 10.0, 10.0)
                    })
                })
                .collect()
        };
        let area = Area {
            rect: Rect::new(0.0, 0.0, 200.0, 100.0),
            cells: grid(0, 10),
        };
        assert!(!shows_something(&cap, &area));
        // A few dark strokes, as text would draw.
        for y in 40..50 {
            for x in (20..180).step_by(6) {
                let i = (y * 200 + x) * 4;
                cap.rgba[i..i + 3].copy_from_slice(&[20, 20, 20]);
            }
        }
        assert!(shows_something(&cap, &area));
        // Only the area's own cells count: the strokes are outside this one.
        let corner = Area {
            rect: Rect::new(0.0, 0.0, 200.0, 100.0),
            cells: grid(6, 10),
        };
        assert!(!shows_something(&cap, &corner));
        assert_eq!(
            pixels_of(&cap, Rect::new(50.0, 20.0, 100.0, 60.0)),
            Some((50, 20, 100, 60))
        );
        assert_eq!(pixels_of(&cap, Rect::new(300.0, 20.0, 10.0, 10.0)), None);
    }

    #[test]
    fn the_margins_around_a_form_are_one_area_of_empty_background() {
        // Fields in the middle of a big window: the space around them is
        // uncovered, but its cells are background.
        let win = Rect::new(0.0, 0.0, 720.0, 360.0);
        let mut raw = vec![node(None, "window", "Settings", win)];
        for row in 0..4 {
            raw.push(node(
                Some(0),
                "text field",
                "Field",
                Rect::new(186.0, 12.0 + f64::from(row) * 42.0, 371.0, 34.0),
            ));
        }
        let areas = uncovered(&raw, win);
        assert!(!areas.is_empty());
        let mut cap = Capture {
            width: 720,
            height: 360,
            rgba: vec![240; 720 * 360 * 4],
            bounds: win,
        };
        // The fields themselves draw edges (inside their own bounds).
        for row in 0..4 {
            for y in 14 + row * 42..44 + row * 42 {
                for x in (188..555).step_by(5) {
                    let i = (y * 720 + x) * 4;
                    cap.rgba[i..i + 3].copy_from_slice(&[30, 30, 30]);
                }
            }
        }
        assert!(
            areas.iter().all(|a| !shows_something(&cap, a)),
            "{} area(s)",
            areas.len()
        );
    }
}
