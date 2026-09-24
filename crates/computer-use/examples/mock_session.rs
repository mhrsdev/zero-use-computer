//! A runnable, platform-independent walk-through of the tool contract, using
//! the in-memory mock backend. Run with:
//!
//! ```text
//! cargo run -p computer-use --example mock_session
//! ```
//!
//! It shows the loop a host agent runs: get_app_state first, then act on
//! elements by their index.

use computer_use::config::{ApprovalMode, Config, ConfigStore};
use computer_use::engine::{AllowApprover, Engine};
use computer_use::mock::MockBackend;
use computer_use::tools::ToolCall;
use serde_json::json;

fn main() {
    // A backend with a fake TextEdit-like app.
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));

    let mut cfg = Config::default();
    cfg.approvals.mode = ApprovalMode::AllowAll; // no prompting in this demo
    let mut engine = Engine::new(backend, ConfigStore::in_memory(cfg));

    // 1. Discover apps.
    run(&mut engine, "list_apps", json!({}));

    // 2. Inspect the app: tree + (mock) screenshot.
    run(&mut engine, "get_app_state", json!({"app": "TextEdit"}));

    // 3. Set the document's text, then read the state back as a diff.
    //    (Element index 4 is the text area in the mock tree above.)
    run(
        &mut engine,
        "set_value",
        json!({"app": "TextEdit", "element_index": 4, "value": "Hello, computer use!"}),
    );
    run(&mut engine, "get_app_state", json!({"app": "TextEdit"}));

    // 4. Press a shortcut.
    run(
        &mut engine,
        "press_key",
        json!({"app": "TextEdit", "key": "cmd+s"}),
    );
}

fn run(engine: &mut Engine<MockBackend>, tool: &str, args: serde_json::Value) {
    println!("\n$ {tool} {args}");
    let out = match ToolCall::parse(tool, args).map(|c| engine.call(c, &mut AllowApprover)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            println!("  error: {e}");
            return;
        }
        Err(e) => {
            println!("  bad call: {e}");
            return;
        }
    };
    for line in out.text.lines() {
        println!("  {line}");
    }
    if let Some(img) = out.image {
        println!("  [screenshot {}x{} {}]", img.width, img.height, img.mime);
    }
}
