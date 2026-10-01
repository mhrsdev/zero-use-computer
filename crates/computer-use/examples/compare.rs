//! One agent session run twice on the real platform backend: once the way
//! Codex's computer use behaves (a screenshot attached to every
//! get_app_state, no screen memory, no picture dedupe, whole-window
//! pictures, no change report after an action), once with this server's
//! defaults. Same app, same steps: look, go to another page, look, come
//! back, look. Reports what the model would receive (estimated tokens) and
//! how long the calls took.
//!
//! ```text
//! APPS=gtk3-widget-factory scripts/desktop-session.sh \
//!   cargo run --release -p computer-use --example compare -- gtk3-widget-factory "Page 2|Page 1" 5
//! ```
//!
//! Token estimates as in bench.rs: text ≈ 4 characters a token, an image
//! width × height / 750.

use std::time::{Duration, Instant};

use computer_use::config::{AttachMode, Config, ConfigStore, ShotScope};
use computer_use::engine::Engine;
use serde_json::{Value, json};

type Eng = Engine<Box<dyn computer_use::Backend>>;

#[derive(Default)]
struct Totals {
    calls: usize,
    chars: usize,
    images: usize,
    image_tokens: u64,
    time: Duration,
    looks: usize,
    look_time: Duration,
}

impl Totals {
    fn tokens(&self) -> u64 {
        self.chars.div_ceil(4) as u64 + self.image_tokens
    }
}

fn call(engine: &mut Eng, t: &mut Totals, tool: &str, args: Value) -> String {
    let t0 = Instant::now();
    let out = engine.call_tool(tool, args);
    let took = t0.elapsed();
    if out.is_error {
        eprintln!("  ! {tool}: {}", out.text);
    }
    t.calls += 1;
    t.time += took;
    t.chars += out.text.chars().count();
    if tool == "get_app_state" {
        t.looks += 1;
        t.look_time += took;
    }
    if let Some(img) = &out.image {
        t.images += 1;
        t.image_tokens += (u64::from(img.width) * u64::from(img.height)).div_ceil(750);
    }
    out.text
}

/// Find the element named `name` and click it, as a model would.
fn press(engine: &mut Eng, t: &mut Totals, app: &str, name: &str) {
    let found = call(engine, t, "find_element", json!({"app": app, "name": name}));
    let index: u32 = found
        .lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no element named {name}: {found}"));
    call(
        engine,
        t,
        "click",
        json!({"app": app, "element_index": index}),
    );
}

fn session(app: &str, forward: &str, back: &str, cycles: usize, codex: bool) -> Totals {
    let mut cfg = Config::default();
    cfg.audit.enabled = false;
    if codex {
        cfg.screenshot.attach = AttachMode::Always;
        cfg.screenshot.scope = ShotScope::Full;
        cfg.cache.enabled = false;
        cfg.cache.dedupe_screenshots = false;
        cfg.tree.report_changes = false;
    }
    let backend = computer_use::platform_backend().expect("backend");
    let mut engine = Engine::new(backend, ConfigStore::in_memory(cfg));
    let mut t = Totals::default();
    let look = json!({"app": app});
    call(&mut engine, &mut t, "get_app_state", look.clone());
    for _ in 0..cycles {
        press(&mut engine, &mut t, app, forward);
        call(&mut engine, &mut t, "get_app_state", look.clone());
        press(&mut engine, &mut t, app, back);
        call(&mut engine, &mut t, "get_app_state", look.clone());
    }
    t
}

fn main() {
    let mut args = std::env::args().skip(1);
    let app = args
        .next()
        .expect("usage: compare <app> \"<forward>|<back>\" [cycles]");
    let nav = args.next().unwrap_or_else(|| "Page 2|Page 1".into());
    let (forward, back) = nav.split_once('|').expect("navigation as \"forward|back\"");
    let cycles: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    println!("app {app}, {cycles} round trips ({forward} and back)\n");
    let codex = session(&app, forward, back, cycles, true);
    let ours = session(&app, forward, back, cycles, false);
    println!(
        "{:<22} {:>6} {:>9} {:>7} {:>10} {:>10} {:>9} {:>11}",
        "", "calls", "text tok", "images", "image tok", "total tok", "time ms", "look ms avg"
    );
    for (name, t) in [("Codex-style", &codex), ("computer-use defaults", &ours)] {
        println!(
            "{:<22} {:>6} {:>9} {:>7} {:>10} {:>10} {:>9.0} {:>11.1}",
            name,
            t.calls,
            t.chars.div_ceil(4),
            t.images,
            t.image_tokens,
            t.tokens(),
            t.time.as_secs_f64() * 1000.0,
            t.look_time.as_secs_f64() * 1000.0 / t.looks.max(1) as f64
        );
    }
    let ratio = codex.tokens() as f64 / ours.tokens().max(1) as f64;
    let saved = 100.0 * (1.0 - ours.tokens() as f64 / codex.tokens().max(1) as f64);
    let faster = codex.time.as_secs_f64() / ours.time.as_secs_f64().max(1e-9);
    println!("\ntokens: {ratio:.1}x fewer ({saved:.0}% saved); time: {faster:.2}x as fast");
}
