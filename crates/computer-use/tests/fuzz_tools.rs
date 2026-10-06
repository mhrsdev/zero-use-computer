//! Every tool, called with arguments built from its own schema: missing,
//! wrong types, empty, huge, negative, NaN-like and non-Latin values. No
//! call may panic (the engine turns a panic into an error result, so a
//! panic hook records them here) or take long.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use computer_use::config::{Config, ConfigStore};
use computer_use::engine::Engine;
use computer_use::mock::MockBackend;
use serde_json::{Value, json};

static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn values_for(schema: &Value) -> Vec<Value> {
    let ty = schema.get("type").and_then(Value::as_str).unwrap_or("");
    let mut v = vec![
        Value::Null,
        json!(""),
        json!("سلام دنیا ✓ 日本"),
        json!(-1),
        json!(0),
        json!(1e308),
        json!(-1e308),
        json!(u64::MAX),
        json!(i64::MIN),
        json!([]),
        json!({}),
        json!(true),
    ];
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        v.extend(e.iter().cloned());
    }
    match ty {
        "array" => {
            v.push(json!([0, 0, 0, 0]));
            v.push(json!([1e9, -1e9, 1e9, -1e9]));
            v.push(json!([[0, 0], [1e12, 1e12]]));
            v.push(json!(["", "x"]));
            v.push(json!([{}]));
            v.push(Value::Array(vec![json!(1); 5000]));
        }
        "string" => {
            v.push(json!("a".repeat(100_000)));
            v.push(json!("\u{0}\n\r\t"));
            v.push(json!("ctrl+alt+"));
        }
        "integer" | "number" => {
            v.push(json!(2_147_483_648u64));
            v.push(json!(0.0000001));
        }
        _ => {}
    }
    v
}

#[test]
fn no_tool_panics_on_odd_arguments() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        PANICS.lock().unwrap().push(format!("{info}"));
        prev(info);
    }));
    let mut cfg = Config::default();
    cfg.tools.manager = computer_use::config::ToolManager::Off;
    // The tools behind a setting run their real path, not "turned off".
    cfg.notifications.enabled = true;
    cfg.clipboard = true;
    // Every tool with its full schema: the settings' list leaves some out
    // (decide, get_notifications) and lean schemas drop arguments (window).
    let defs = computer_use::tools::definitions();
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    let mut found = Vec::new();
    let mut slow = Vec::new();
    for d in &defs {
        let props = d.input_schema["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut calls: Vec<Value> = vec![json!({}), json!({"app": "TextEdit"})];
        for (k, s) in &props {
            for val in values_for(s) {
                calls.push(json!({"app": "TextEdit", k: val.clone()}));
                calls.push(json!({k: val}));
            }
        }
        for mut args in calls {
            // A wait waits by design: briefly here.
            if d.name == "wait_for" && args.get("timeout_ms").is_none() {
                args["timeout_ms"] = json!(20);
            }
            PANICS.lock().unwrap().clear();
            let start = Instant::now();
            let out = e.call_tool(&d.name, args.clone());
            let took = start.elapsed();
            let p = PANICS.lock().unwrap().clone();
            if !p.is_empty() {
                let shown: String = args.to_string().chars().take(200).collect();
                found.push(format!("{} {} -> {}", d.name, shown, p[0]));
            }
            if took > Duration::from_secs(5) {
                let shown: String = args.to_string().chars().take(120).collect();
                slow.push(format!("{} {} took {took:?}", d.name, shown));
            }
            let _ = out;
        }
    }
    found.sort();
    found.dedup();
    assert!(
        found.is_empty() && slow.is_empty(),
        "panics:\n{}\nslow:\n{}",
        found.join("\n"),
        slow.join("\n")
    );
}
