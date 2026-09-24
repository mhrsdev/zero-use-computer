//! Live end-to-end test of the Linux backend + engine against a real GTK app.
//!
//! It only runs when `COMPUTER_USE_LIVE=1` (the harness in
//! `tests/run_live_linux.sh` sets up Xvfb, D-Bus and AT-SPI and starts the
//! GTK fixture first). Everything else is a no-op so `cargo test` stays green
//! in environments without a display.

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use computer_use::Backend;
use computer_use::config::{ApprovalMode, Config, ConfigStore};
use computer_use::engine::{AllowApprover, Engine};
use computer_use::linux::LinuxBackend;
use computer_use::tools::ToolCall;

fn live() -> bool {
    std::env::var("COMPUTER_USE_LIVE").as_deref() == Ok("1")
}

fn engine() -> Engine<LinuxBackend> {
    let backend = LinuxBackend::new().expect("connect to AT-SPI/X11");
    let mut cfg = Config::default();
    cfg.approvals.mode = ApprovalMode::AllowAll;
    Engine::new(backend, ConfigStore::in_memory(cfg))
}

/// Find our GTK fixture's app id by scanning windows for the "CU Test" title.
fn wait_for_app(e: &mut Engine<LinuxBackend>) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(apps) = e.backend_mut().list_apps() {
            for app in apps {
                if let Ok(windows) = e.backend_mut().list_windows(&app) {
                    if windows.iter().any(|w| w.title.contains("CU Test")) {
                        return app.id;
                    }
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "GTK fixture never appeared over AT-SPI"
        );
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn state_text(e: &mut Engine<LinuxBackend>, app: &str) -> String {
    let out = e
        .call(
            ToolCall::GetAppState(computer_use::tools::GetAppStateArgs {
                app: app.into(),
                window: Some("CU Test".into()),
                disable_diff: true,
            }),
            &mut AllowApprover,
        )
        .expect("get_app_state");
    assert!(!out.is_error, "get_app_state errored: {}", out.text);
    out.text
}

fn index_of(tree: &str, needle: &str) -> u32 {
    for line in tree.lines() {
        if line.contains(needle) {
            let trimmed = line.trim_start();
            if let Some(tok) = trimmed.split_whitespace().next() {
                if let Ok(n) = tok.parse::<u32>() {
                    return n;
                }
            }
        }
    }
    panic!("no element matching `{needle}` in tree:\n{tree}");
}

#[test]
fn atspi_tree_actions_and_screenshot() {
    if !live() {
        eprintln!("skipping live test (set COMPUTER_USE_LIVE=1 to run)");
        return;
    }
    let mut e = engine();
    let app = wait_for_app(&mut e);
    eprintln!("found fixture app id = {app}");

    // 1. The tree exposes the widgets and a screenshot.
    let out = e
        .call(
            ToolCall::GetAppState(computer_use::tools::GetAppStateArgs {
                app: app.clone(),
                window: Some("CU Test".into()),
                disable_diff: true,
            }),
            &mut AllowApprover,
        )
        .unwrap();
    assert!(out.image.is_some(), "expected a screenshot");
    let img = out.image.unwrap();
    assert!(img.width > 100 && img.height > 50, "screenshot too small");
    let tree = out.text;
    eprintln!("--- initial tree ---\n{tree}");
    assert!(tree.contains("button \"Click Me\""), "no button:\n{tree}");
    assert!(tree.contains("Enable feature"), "no checkbox:\n{tree}");

    // 2. Press the button via its accessibility action; the status label updates.
    let button = index_of(&tree, "Click Me");
    let out = e
        .call(
            ToolCall::Click(computer_use::tools::ClickArgs {
                app: app.clone(),
                element_index: Some(button),
                ..Default::default()
            }),
            &mut AllowApprover,
        )
        .unwrap();
    assert!(!out.is_error, "click failed: {}", out.text);
    std::thread::sleep(Duration::from_millis(150));
    let tree = state_text(&mut e, &app);
    assert!(tree.contains("clicked"), "status did not update:\n{tree}");

    // 3. set_value into the entry; the status label echoes it.
    let entry = index_of(&tree, "text field");
    let out = e
        .call(
            ToolCall::SetValue(computer_use::tools::SetValueArgs {
                app: app.clone(),
                element_index: entry,
                value: "hello atspi".into(),
                ..Default::default()
            }),
            &mut AllowApprover,
        )
        .unwrap();
    assert!(!out.is_error, "set_value failed: {}", out.text);
    std::thread::sleep(Duration::from_millis(150));
    let tree = state_text(&mut e, &app);
    assert!(
        tree.contains("hello atspi"),
        "entry value not reflected:\n{tree}"
    );

    // 4. Toggle the checkbox with perform_secondary_action / set_value.
    let check = index_of(&tree, "Enable feature");
    let out = e
        .call(
            ToolCall::SetValue(computer_use::tools::SetValueArgs {
                app: app.clone(),
                element_index: check,
                value: "true".into(),
                ..Default::default()
            }),
            &mut AllowApprover,
        )
        .unwrap();
    assert!(!out.is_error, "checkbox set failed: {}", out.text);
    std::thread::sleep(Duration::from_millis(150));
    let tree = state_text(&mut e, &app);
    assert!(
        tree.contains("feature: on"),
        "checkbox not toggled:\n{tree}"
    );

    // 5. Keyboard shortcut path (synthetic key via XTest): focus entry, select
    //    all.
    let tree = state_text(&mut e, &app);
    let entry = index_of(&tree, "text field");
    let out = e
        .call(
            ToolCall::PressKey(computer_use::tools::PressKeyArgs {
                app: app.clone(),
                key: "ctrl+a".into(),
                element_index: Some(entry),
                ..Default::default()
            }),
            &mut AllowApprover,
        )
        .unwrap();
    assert!(!out.is_error, "press_key failed: {}", out.text);

    eprintln!("live test passed");
}

#[test]
fn new_tools_over_real_backend() {
    if !live() {
        return;
    }
    let mut e = engine();
    let app = wait_for_app(&mut e);

    // find_element: locate the button by role.
    let out = e.call_tool(
        "find_element",
        serde_json::json!({"app": app, "role": "button"}),
        &mut AllowApprover,
    );
    assert!(!out.is_error, "find_element: {}", out.text);
    assert!(out.text.contains("Click Me"), "find_element:\n{}", out.text);

    // wait_for: the entry is already present.
    let out = e.call_tool(
        "wait_for",
        serde_json::json!({"app": app, "role": "text field", "timeout_ms": 3000}),
        &mut AllowApprover,
    );
    assert!(!out.is_error, "wait_for: {}", out.text);

    // screenshot: full screen and a region (X11 GetImage path).
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "full"}),
        &mut AllowApprover,
    );
    assert!(out.image.is_some(), "full screenshot: {}", out.text);
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "region", "x": 0, "y": 0, "width": 200, "height": 120}),
        &mut AllowApprover,
    );
    assert!(out.image.is_some(), "region screenshot: {}", out.text);
    let img = out.image.unwrap();
    assert!(img.width > 0 && img.height > 0);

    // annotated window screenshot (set-of-marks overlay).
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "window", "app": app, "window": "CU Test", "annotate": true}),
        &mut AllowApprover,
    );
    assert!(out.image.is_some(), "annotated screenshot: {}", out.text);

    // clipboard round-trip via xclip (installed by the harness).
    let out = e.call_tool(
        "set_clipboard",
        serde_json::json!({"text": "computer-use clip"}),
        &mut AllowApprover,
    );
    assert!(!out.is_error, "set_clipboard: {}", out.text);
    let out = e.call_tool("get_clipboard", serde_json::json!({}), &mut AllowApprover);
    assert!(
        out.text.contains("computer-use clip"),
        "clipboard round-trip:\n{}",
        out.text
    );

    eprintln!("new-tools live test passed");
}
