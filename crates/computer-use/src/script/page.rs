//! The graph-paper page of scripts, and cells for any canvas.
//!
//! A page is a design on the design board: what a script puts on it are
//! layers, so the same picture can be seen, checked, exported or drawn
//! into an app step by step like any design. Its named cells (A1 top-left)
//! are the design's cells, of the size the script asked for.

use serde_json::{Map, Value, json};

use crate::cells::{Cells, Span};

/// Keys a layer's style may have (the design tool's, plus `radius` for a
/// rectangle's corners).
pub const STYLE_KEYS: &[&str] = &[
    "id", "fill", "stroke", "width", "opacity", "rotate", "about", "repeat", "closed", "smooth",
    "steps", "font", "size", "bold", "align", "below", "above", "move", "to", "radius",
];

/// What a script did to one page since it was last sent to the board.
#[derive(Debug, Clone, Default)]
pub struct PageState {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub cell: Option<f64>,
    /// Size, background, cell size, margin: sent with the next update.
    pub start: Map<String, Value>,
    pub add: Vec<Value>,
    pub change: Vec<Value>,
    pub remove: Vec<String>,
    /// Ids of every layer on it, to name new ones and to clear it.
    pub ids: Vec<String>,
    pub next: usize,
    /// Changed since the board last drew it.
    pub dirty: bool,
    /// The board's picture of it, as of the last update.
    pub image: Option<u64>,
    pub summary: String,
}

impl PageState {
    pub fn cells(&self) -> Cells {
        match self.cell {
            Some(c) => Cells::with_step(0.0, self.width, 0.0, self.height, false, c),
            None => Cells::new(0.0, self.width, 0.0, self.height, false, None),
        }
    }

    /// A free id like "rect-3".
    pub fn new_id(&mut self, kind: &str) -> String {
        loop {
            self.next += 1;
            let id = format!("{kind}-{}", self.next);
            if !self.ids.iter().any(|i| i.eq_ignore_ascii_case(&id)) {
                return id;
            }
        }
    }

    /// Add a layer: `shape` (the layer's shape keys) with `style`.
    pub fn add(
        &mut self,
        kind: &str,
        mut shape: Map<String, Value>,
        style: Map<String, Value>,
    ) -> Result<String, String> {
        for (k, v) in style {
            if !STYLE_KEYS.contains(&k.as_str()) {
                return Err(format!(
                    "\"{k}\" is not a style key (keys: {})",
                    STYLE_KEYS.join(", ")
                ));
            }
            if k == "radius" {
                match shape.get_mut("rect") {
                    Some(Value::Array(r)) if r.len() == 4 => r.push(v),
                    _ => return Err("radius is for rect".into()),
                }
                continue;
            }
            shape.insert(k, v);
        }
        let id = match shape.get("id") {
            Some(Value::String(id)) if !id.trim().is_empty() => {
                let id = id.trim().to_string();
                if self.ids.iter().any(|i| i.eq_ignore_ascii_case(&id)) {
                    return Err(format!("there is already a layer \"{id}\" on the page"));
                }
                id
            }
            Some(Value::String(_)) | None => self.new_id(kind),
            Some(other) => return Err(format!("id must be text, not {other}")),
        };
        shape.insert("id".into(), Value::String(id.clone()));
        self.ids.push(id.clone());
        self.add.push(Value::Object(shape));
        self.dirty = true;
        Ok(id)
    }

    pub fn change(&mut self, id: &str, mut spec: Map<String, Value>) -> Result<(), String> {
        let id = self.known(id)?;
        spec.insert("id".into(), Value::String(id));
        // A layer added since the last update is changed in the same one.
        self.change.push(Value::Object(spec));
        self.dirty = true;
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let id = self.known(id)?;
        self.ids.retain(|i| *i != id);
        let before = self.add.len();
        self.add
            .retain(|l| l.get("id").and_then(Value::as_str) != Some(id.as_str()));
        if self.add.len() == before {
            self.remove.push(id);
        }
        self.change.retain(|l| {
            l.get("id")
                .and_then(Value::as_str)
                .is_some_and(|i| self.ids.iter().any(|k| k == i))
        });
        self.dirty = true;
        Ok(())
    }

    pub fn clear(&mut self) {
        for id in std::mem::take(&mut self.ids) {
            if !self
                .add
                .iter()
                .any(|l| l.get("id").and_then(Value::as_str) == Some(id.as_str()))
            {
                self.remove.push(id);
            }
        }
        self.add.clear();
        self.change.clear();
        self.dirty = true;
    }

    fn known(&self, id: &str) -> Result<String, String> {
        self.ids
            .iter()
            .find(|i| i.eq_ignore_ascii_case(id.trim()))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "no layer \"{id}\" on the page (layers: {})",
                    if self.ids.is_empty() {
                        "none".to_string()
                    } else {
                        self.ids.join(", ")
                    }
                )
            })
    }

    /// The design tool's arguments for what changed; the change is then
    /// counted as sent.
    pub fn update(&mut self) -> Value {
        let mut args = std::mem::take(&mut self.start);
        args.insert("name".into(), json!(self.name));
        if !self.remove.is_empty() {
            args.insert("remove".into(), json!(std::mem::take(&mut self.remove)));
        }
        if !self.add.is_empty() {
            args.insert("add".into(), Value::Array(std::mem::take(&mut self.add)));
        }
        if !self.change.is_empty() {
            args.insert(
                "change".into(),
                Value::Array(std::mem::take(&mut self.change)),
            );
        }
        args.insert("show".into(), json!({"cells": true}));
        self.dirty = false;
        Value::Object(args)
    }
}

/// A cell as a script sees it.
pub fn cell_map(c: &Cells, col: usize, row: usize) -> Value {
    let s = c.span(col, row);
    json!({
        "name": Cells::name(col, row),
        "col": col,
        "row": row,
        "x": s.x0,
        "y": if c.y_up { s.y1 } else { s.y0 },
        "w": s.x1 - s.x0,
        "h": s.y1 - s.y0,
        "cx": (s.x0 + s.x1) / 2.0,
        "cy": (s.y0 + s.y1) / 2.0,
    })
}

/// The cell at a point, or "" outside the cells.
pub fn cell_at(c: &Cells, x: f64, y: f64) -> String {
    // NaN is no point (and would be read as the first cell).
    if !(x.is_finite() && y.is_finite()) {
        return String::new();
    }
    let (col, row) = c.at(x, y);
    if col < 0 || row < 0 || col as usize >= c.cols || row as usize >= c.rows {
        String::new()
    } else {
        Cells::name(col as usize, row as usize)
    }
}

/// Cells over an area: `size` [w, h] (y down) or `range` [x0, x1, y0, y1]
/// (y up), `cell` units each or about `across` (default 8) across.
pub fn cells_from(opts: &Map<String, Value>) -> Result<Cells, String> {
    let nums = |k: &str| -> Option<Vec<f64>> {
        opts.get(k)?.as_array()?.iter().map(Value::as_f64).collect()
    };
    let (x0, x1, y0, y1, y_up) = match (nums("size"), nums("range")) {
        (Some(s), None) if s.len() == 2 => (0.0, s[0], 0.0, s[1], false),
        (None, Some(r)) if r.len() == 4 => (r[0], r[1], r[2], r[3], true),
        _ => return Err("cells takes size: [width, height] (y down) or range: [x min, x max, y min, y max] (y up)".into()),
    };
    if ![x0, x1, y0, y1].iter().all(|v| v.is_finite()) || x0 == x1 || y0 == y1 {
        return Err("the area must have a width and a height".into());
    }
    for k in opts.keys() {
        if !["size", "range", "cell", "across"].contains(&k.as_str()) {
            return Err(format!(
                "cells takes size or range, and cell or across, not {k}"
            ));
        }
    }
    let cells = match (
        opts.get("cell").and_then(Value::as_f64),
        opts.get("across").and_then(Value::as_f64),
    ) {
        (Some(c), _) if c > 0.0 => Cells::with_step(x0, x1, y0, y1, y_up, c),
        (None, Some(n)) if n >= 1.0 => {
            let span = (x1 - x0).abs().max((y1 - y0).abs());
            Cells::with_step(x0, x1, y0, y1, y_up, span / n.round())
        }
        (None, None) => Cells::new(x0, x1, y0, y1, y_up, None),
        _ => return Err("cell is a size above 0, across a count of 1 or more".into()),
    };
    if cells.cols as f64 > crate::design::MAX_CELLS || cells.rows as f64 > crate::design::MAX_CELLS
    {
        return Err(format!(
            "that makes {} x {} cells; at most {} across",
            cells.cols,
            cells.rows,
            crate::design::MAX_CELLS
        ));
    }
    Ok(cells)
}

/// A cell's box as a `rect` [x, y, w, h] (y down pages).
pub fn rect_of(s: Span) -> [f64; 4] {
    [s.x0, s.y0, s.x1 - s.x0, s.y1 - s.y0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_that_is_not_a_number_is_in_no_cell() {
        let c = Cells::new(0.0, 100.0, 0.0, 100.0, false, None);
        assert_eq!(cell_at(&c, 1.0, 1.0), "A1");
        assert_eq!(cell_at(&c, f64::NAN, 1.0), "");
        assert_eq!(cell_at(&c, 1.0, f64::NAN), "");
        assert_eq!(cell_at(&c, f64::INFINITY, 1.0), "");
    }
}
