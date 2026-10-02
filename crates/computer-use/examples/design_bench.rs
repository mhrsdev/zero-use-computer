//! Benchmark the design board: what each call costs the model (text and
//! picture tokens) and the server (time), over a session the way an agent
//! builds a design: start it, add layers, fix a colour, mirror, align,
//! space out, retitle, look, zoom into a cell, export; and a page of many
//! layers. No desktop needed.
//!
//! ```text
//! cargo run --release -p computer-use --example design_bench [-- --json]
//! ```
//!
//! Token figures are estimates: text at about 4 characters a token, a
//! picture at width × height / 750.

use std::time::Instant;

use computer_use::config::{Config, ConfigStore};
use computer_use::engine::Engine;
use computer_use::mock::MockBackend;
use serde_json::{Value, json};

struct Row {
    step: &'static str,
    ms: f64,
    text_tok: usize,
    image: Option<(u32, u32)>,
    error: bool,
}

impl Row {
    fn image_tok(&self) -> usize {
        self.image
            .map_or(0, |(w, h)| (w as usize * h as usize).div_ceil(750))
    }
}

/// One run: the badge's rows, the pattern's rows.
type Run = (Vec<Row>, Vec<Row>);

fn engine() -> Engine<MockBackend> {
    Engine::new(
        MockBackend::new(),
        ConfigStore::in_memory(Config::default()),
    )
}

fn call(e: &mut Engine<MockBackend>, step: &'static str, args: Value) -> Row {
    let start = Instant::now();
    let out = e.call_tool("design", args);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    if out.is_error {
        eprintln!("{step}: error: {}", out.text);
    }
    Row {
        step,
        ms,
        text_tok: computer_use::text::estimate_tokens(&out.text),
        image: out.image.as_ref().map(|i| (i.width, i.height)),
        error: out.is_error,
    }
}

/// A badge: the steps an agent takes to build and fix one.
fn session(e: &mut Engine<MockBackend>) -> Vec<Row> {
    vec![
        call(
            e,
            "start + 8 layers",
            json!({"name": "badge", "size": [800, 600], "background": "#F4F1EA", "add": [
                {"id": "disc", "ellipse": [400, 300, 220, 220], "fill": "#1D3557"},
                {"id": "ring", "ellipse": [400, 300, 240, 240], "stroke": "#E63946", "width": 12},
                {"id": "ear-l", "polygon": [250, 150, 60, 3], "fill": "#1D3557"},
                {"id": "eye-l", "ellipse": [330, 260, 28, 34], "fill": "#F1FAEE"},
                {"id": "eye-r", "ellipse": [472, 262, 28, 34], "fill": "#F1FAEE"},
                {"id": "mouth", "arc": [400, 330, 80, 20, 160], "stroke": "#F1FAEE", "width": 10},
                {"id": "star", "star": [400, 470, 40, 18, 5], "fill": "#FFB703"},
                {"id": "title", "text": "OWL CLUB", "at": [400, 40], "size": 56, "bold": true, "align": "center", "fill": "#1D3557"}
            ]}),
        ),
        call(
            e,
            "recolour one layer",
            json!({"name": "badge", "change": [{"id": "star", "fill": "#E63946"}]}),
        ),
        call(
            e,
            "mirror an ear",
            json!({"name": "badge", "mirror": [{"id": "ear-l", "as": "ear-r"}]}),
        ),
        call(
            e,
            "fix the eyes' height",
            json!({"name": "badge", "change": [{"id": "eye-r", "move": [0, -2]}]}),
        ),
        call(
            e,
            "add 3 dots",
            json!({"name": "badge", "add": [
                {"id": "d1", "ellipse": [300, 540, 8, 8], "fill": "#1D3557"},
                {"id": "d2", "ellipse": [390, 540, 8, 8], "fill": "#1D3557"},
                {"id": "d3", "ellipse": [500, 540, 8, 8], "fill": "#1D3557"}
            ]}),
        ),
        call(
            e,
            "space the dots",
            json!({"name": "badge", "distribute": [{"ids": ["d1", "d2", "d3"]}]}),
        ),
        call(
            e,
            "retitle",
            json!({"name": "badge", "change": [{"id": "title", "text": "OWL CLUB 2026"}]}),
        ),
        call(e, "look again", json!({"name": "badge"})),
        call(
            e,
            "zoom into a cell",
            json!({"name": "badge", "show": {"cell": "D3"}}),
        ),
        call(e, "export svg", json!({"name": "badge", "export": "svg"})),
    ]
}

/// A page of many layers (a pattern: columns of dots, stars and tiles):
/// the server's time, and a long listing.
fn heavy(e: &mut Engine<MockBackend>) -> Vec<Row> {
    let mut add = Vec::new();
    for i in 0..15 {
        for j in 0..10 {
            let (x, y) = (40.0 + f64::from(i) * 50.0, 40.0 + f64::from(j) * 52.0);
            // Columns of look-alikes (dots, stars, tiles), as a pattern is.
            add.push(match i % 3 {
                0 => json!({"ellipse": [x, y, 16, 16], "fill": "#264653"}),
                1 => json!({"star": [x, y, 18, 8, 5], "fill": "#E9C46A"}),
                _ => json!({"rect": [x - 15.0, y - 15.0, 30, 30, 6], "fill": "#E76F51"}),
            });
        }
    }
    vec![
        call(
            e,
            "150 layers",
            json!({"name": "pattern", "size": [800, 600], "add": add}),
        ),
        call(
            e,
            "recolour one of 150",
            json!({"name": "pattern", "change": [{"id": "layer-75", "fill": "#000000"}]}),
        ),
        call(e, "look at 150", json!({"name": "pattern"})),
    ]
}

fn print(title: &str, rows: &[Row]) -> (usize, usize, f64) {
    println!("\n## {title}\n");
    println!("| step | ms | text tok | picture | picture tok | total tok |");
    println!("|---|---:|---:|---|---:|---:|");
    let (mut text, mut img, mut ms) = (0, 0, 0.0);
    for r in rows {
        let pic = r.image.map_or("—".to_string(), |(w, h)| format!("{w}×{h}"));
        println!(
            "| {}{} | {:.1} | {} | {} | {} | {} |",
            r.step,
            if r.error { " (error)" } else { "" },
            r.ms,
            r.text_tok,
            pic,
            r.image_tok(),
            r.text_tok + r.image_tok()
        );
        text += r.text_tok;
        img += r.image_tok();
        ms += r.ms;
    }
    println!(
        "| **total** | **{ms:.1}** | **{text}** | | **{img}** | **{}** |",
        text + img
    );
    (text, img, ms)
}

fn main() {
    let json_out = std::env::args().any(|a| a == "--json");
    // Warm the fonts and the code paths once, as a running server is.
    {
        let mut e = engine();
        let _ = session(&mut e);
    }
    let mut runs: Vec<Run> = Vec::new();
    for _ in 0..5 {
        let mut e = engine();
        let s = session(&mut e);
        let h = heavy(&mut e);
        runs.push((s, h));
    }
    // The fastest of the runs for each step (the least noise); tokens are
    // the same every run.
    let best = |pick: fn(&Run) -> &Vec<Row>| -> Vec<Row> {
        let first = pick(&runs[0]);
        (0..first.len())
            .map(|k| {
                let ms = runs.iter().map(|r| pick(r)[k].ms).fold(f64::MAX, f64::min);
                let r = &first[k];
                Row {
                    step: r.step,
                    ms,
                    text_tok: r.text_tok,
                    image: r.image,
                    error: r.error,
                }
            })
            .collect()
    };
    let s = best(|r| &r.0);
    let h = best(|r| &r.1);
    let (st, si, sm) = print("A badge, built and fixed (10 calls)", &s);
    let (ht, hi, hm) = print("A page of 150 layers", &h);
    if json_out {
        println!(
            "{}",
            json!({
                "session": {"text": st, "image": si, "ms": sm},
                "heavy": {"text": ht, "image": hi, "ms": hm},
            })
        );
    }
}
