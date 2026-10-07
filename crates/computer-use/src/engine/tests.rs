use super::drawing::PathPosition;
use super::observe::reads_as_start_of;
use super::*;
use crate::config::Config;
use crate::mock::{Event, MockBackend};
use crate::tree::{expand, index_of};

/// An engine whose `design` lists its paint steps with every answer.
fn steps_engine() -> Engine<MockBackend> {
    let mut e = engine();
    let mut cfg = e.store().config.clone();
    cfg.tools.design_steps = crate::config::DesignSteps::Always;
    e.set_config(ConfigStore::in_memory(cfg));
    e
}

fn engine() -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let cfg = Config::default();
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg));
    // Deterministic, instant time.
    e = e.with_time(Instant::now, |_| {});
    e
}

#[test]
fn list_apps_and_state() {
    let mut e = engine();
    let out = e.call(ToolCall::ListApps).unwrap();
    assert!(out.text.contains("TextEdit"));

    let out = e
        .call(ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        }))
        .unwrap();
    assert!(out.image.is_some());
    assert!(out.text.contains("button \"Bold\""));
    assert!(out.text.contains("text area \"Document\""));
    // Bold has only press (hidden); Style keeps show_menu as a listed action.
    assert!(out.text.contains("actions=[show_menu]"));
    assert!(!out.text.contains("actions=[press"));
}

#[test]
fn click_uses_accessibility_then_falls_back_to_coords() {
    let mut e = engine();
    e.call(ToolCall::GetAppState(GetAppStateArgs {
        app: "TextEdit".into(),
        ..Default::default()
    }))
    .unwrap();
    // Find Bold's index.
    let bold = e
        .state(4242)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.name.as_deref() == Some("Bold"))
        .unwrap()
        .index;
    e.call(ToolCall::Click(ClickArgs {
        app: "TextEdit".into(),
        element_index: Some(bold),
        ..Default::default()
    }))
    .unwrap();
    assert!(matches!(
        e.backend().events.last().unwrap(),
        Event::Action(3, a) if a == "AXPress"
    ));

    // Right click -> coordinate path.
    e.call(ToolCall::Click(ClickArgs {
        app: "TextEdit".into(),
        element_index: Some(bold),
        button: MouseButton::Right,
        ..Default::default()
    }))
    .unwrap();
    assert!(matches!(
        e.backend().events.last().unwrap(),
        Event::Click(4242, _, MouseButton::Right, 1)
    ));
}

#[test]
fn coordinate_click_maps_through_screenshot() {
    let mut e = engine();
    e.call(ToolCall::GetAppState(GetAppStateArgs {
        app: "TextEdit".into(),
        ..Default::default()
    }))
    .unwrap();
    e.call(ToolCall::Click(ClickArgs {
        app: "TextEdit".into(),
        x: Some(400.0),
        y: Some(300.0),
        ..Default::default()
    }))
    .unwrap();
    // Window is 800x600; screenshot fits to 800x600 (<=1280) so 1:1.
    assert!(matches!(
        e.backend().events.last().unwrap(),
        Event::Click(4242, p, _, _) if (p.x-400.0).abs()<1.0 && (p.y-300.0).abs()<1.0
    ));
}

#[test]
fn coordinates_follow_a_window_that_moved() {
    let mut e = engine();
    let mut cfg = e.store().config.clone();
    cfg.cache.snapshot_ttl_ms = 0;
    cfg.timing.app_cache_ms = 0;
    e.set_config(ConfigStore::in_memory(cfg));
    let look = || {
        ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        })
    };
    e.call(look()).unwrap();
    // The user drags the window 300 px right: the tree reads the same.
    e.backend_mut().app_mut(4242).unwrap().windows[0].bounds.x += 300.0;
    // No new screenshot comes with this look: the old one must follow.
    let out = e
        .call(ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            screenshot: Some(false),
            ..Default::default()
        }))
        .unwrap();
    assert!(out.image.is_none());
    e.call(ToolCall::Click(ClickArgs {
        app: "TextEdit".into(),
        x: Some(400.0),
        y: Some(300.0),
        ..Default::default()
    }))
    .unwrap();
    assert!(
        matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, p, _, _) if (p.x - 700.0).abs() < 1.0 && (p.y - 300.0).abs() < 1.0
        ),
        "{:?}",
        e.backend().events.last()
    );
}

#[test]
fn looking_at_another_window_leaves_the_screenshot_coordinates_alone() {
    let mut e = engine();
    {
        let app = e.backend_mut().app_mut(4242).unwrap();
        let mut other = app.windows[0].clone();
        other.id = 2;
        other.title = "Other".into();
        other.root = 99;
        other.focused = false;
        other.bounds.x += 300.0;
        other.bounds.y += 200.0;
        app.elements
            .push(MockElement::new(99, "window", "Other", other.bounds));
        app.windows.push(other);
    }
    let look = |window: &str, shot: bool| {
        ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            window: Some(window.into()),
            screenshot: Some(shot),
            ..Default::default()
        })
    };
    let out = e.call(look("Untitled", true)).unwrap();
    assert!(out.image.is_some(), "{}", out.text);
    e.call(look("Other", false)).unwrap();
    // x/y still mean the picture of "Untitled".
    e.call(ToolCall::Click(ClickArgs {
        app: "TextEdit".into(),
        x: Some(400.0),
        y: Some(300.0),
        ..Default::default()
    }))
    .unwrap();
    assert!(
        matches!(
            e.backend().events.last().unwrap(),
            Event::Click(4242, p, _, _) if (p.x - 400.0).abs() < 1.0 && (p.y - 300.0).abs() < 1.0
        ),
        "{:?}",
        e.backend().events.last()
    );
}

#[test]
fn a_call_whose_answer_was_dropped_counts_as_unseen() {
    let mut backend = MockBackend::new();
    backend.add_app(page_a(7));
    backend.on_press.insert(20, page_b(7));
    let mut cfg = Config::default();
    cfg.tree.report_changes_max_lines = 4;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    // The client cancels the click; its answer never reaches the model.
    let shown = e.shown();
    press_named(&mut e, 7, "Next");
    e.not_delivered(shown);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(!out.text.contains("the rest:"), "{}", out.text);
    assert!(out.text.contains("button \"Back\""), "{}", out.text);
    assert!(
        out.image.is_some(),
        "the model has no picture of it: {}",
        out.text
    );
}

#[test]
fn set_value_and_type_and_select() {
    let mut e = engine();
    e.call(ToolCall::GetAppState(GetAppStateArgs {
        app: "TextEdit".into(),
        ..Default::default()
    }))
    .unwrap();
    let doc = e
        .state(4242)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.role == "text area")
        .unwrap()
        .index;
    e.call(ToolCall::SetValue(SetValueArgs {
        app: "TextEdit".into(),
        element_index: doc,
        value: "World".into(),
        ..Default::default()
    }))
    .unwrap();
    assert!(matches!(
        e.backend().events.last().unwrap(),
        Event::SetValue(5, v) if v == "World"
    ));
    e.call(ToolCall::SelectText(SelectTextArgs {
        app: "TextEdit".into(),
        element_index: doc,
        text: Some("or".into()),
        occurrence: 1,
        ..Default::default()
    }))
    .unwrap();
    assert!(matches!(
        e.backend().events.last().unwrap(),
        Event::SelectText(5, Some(t), 1) if t == "or"
    ));
}

#[test]
fn press_key_sequences() {
    let mut e = engine();
    e.call(ToolCall::GetAppState(GetAppStateArgs {
        app: "TextEdit".into(),
        ..Default::default()
    }))
    .unwrap();
    e.call(ToolCall::PressKey(PressKeyArgs {
        app: "TextEdit".into(),
        key: "cmd+a Delete".into(),
        ..Default::default()
    }))
    .unwrap();
    let keys: Vec<_> = e
        .backend()
        .events
        .iter()
        .filter_map(|ev| match ev {
            Event::Key(_, k) => Some(k.clone()),
            _ => None,
        })
        .collect();
    // "cmd" is the shortcut key: Cmd on a Mac, Ctrl elsewhere.
    let select_all = if cfg!(target_os = "macos") {
        "meta+a"
    } else {
        "ctrl+a"
    };
    assert_eq!(keys, vec![select_all, "Delete"]);
}

/// An engine with TextEdit (pid 4242) behind another app that is in front.
fn behind_engine() -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    let mut app = MockBackend::text_editor(4242);
    app.info.frontmost = false;
    backend.add_app(app);
    let mut front = MockBackend::text_editor(77);
    front.info.name = "Terminal".into();
    front.info.id = "com.apple.Terminal".into();
    backend.add_app(front);
    Engine::new(backend, ConfigStore::in_memory(Config::default())).with_time(Instant::now, |_| {})
}

fn keys_sent(e: &Engine<MockBackend>) -> Vec<(u32, String)> {
    e.backend()
        .events
        .iter()
        .filter_map(|ev| match ev {
            Event::Key(pid, k) => Some((*pid, k.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn keys_go_to_the_app_only_once_it_is_in_front() {
    let mut e = behind_engine();
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "Return"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        e.backend()
            .window_ops
            .iter()
            .any(|(_, op)| *op == WindowOp::Focus),
        "brought to the front first"
    );
    assert_eq!(keys_sent(&e), vec![(4242, "Return".to_string())]);
}

#[test]
fn no_input_when_the_app_cannot_come_to_the_front() {
    let mut e = behind_engine();
    e.backend_mut().refuse_focus = true;
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "text": "rm -rf ~\n"}),
    );
    assert!(out.is_error, "{}", out.text);
    assert!(
        out.text.contains("Terminal, which is in front"),
        "{}",
        out.text
    );
    assert!(keys_sent(&e).is_empty(), "nothing typed anywhere");
    assert!(
        !e.backend()
            .events
            .iter()
            .any(|ev| matches!(ev, Event::Type(..))),
        "nothing typed anywhere"
    );
}

#[test]
fn unknown_index_is_a_tool_error() {
    let mut e = engine();
    e.call(ToolCall::GetAppState(GetAppStateArgs {
        app: "TextEdit".into(),
        ..Default::default()
    }))
    .unwrap();
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": 999}),
    );
    assert!(out.is_error);
    assert!(out.text.contains("Element indices are only valid"));
}

#[test]
fn launch_makes_app_available() {
    let mut backend = MockBackend::new();
    backend.add_launchable("notes", {
        let mut a = MockBackend::text_editor(55);
        a.info.name = "Notes".into();
        a.info.id = "com.apple.Notes".into();
        a.info.exe = Some("/System/Applications/Notes.app".into());
        a
    });
    let cfg = Config::default();
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    let out = e
        .call(ToolCall::LaunchApp(LaunchAppArgs {
            app: "Notes".into(),
        }))
        .unwrap();
    assert!(out.text.contains("Launched Notes"));
    // Its first state comes with it ([tools] launch_look).
    assert!(out.text.contains("text area \"Document\""), "{}", out.text);
    assert!(out.image.is_some());
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Notes"}));
    assert!(!out.is_error);
    assert!(!out.text.contains("Document"), "already sent: {}", out.text);
}

#[test]
fn launch_by_catalog_name_finds_the_program_that_ran() {
    let mut backend = MockBackend::new();
    // "Visual Studio Code" is the menu name; the app runs as "Code".
    backend.add_launchable("visual studio code", {
        let mut a = MockBackend::text_editor(56);
        a.info.name = "Code".into();
        a.info.id = "code".into();
        a.info.exe = Some("/usr/share/code/code".into());
        a
    });
    let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
        .with_time(Instant::now, |_| {});
    let out = e.call_tool(
        "launch_app",
        serde_json::json!({"app": "Visual Studio Code"}),
    );
    assert!(out.text.contains("Launched Code (id: code"), "{}", out.text);
}

#[test]
fn launch_app_never_takes_a_command_line() {
    let mut e = engine();
    // The whole string is the program; it is never split into arguments.
    let out = e.call_tool(
        "launch_app",
        serde_json::json!({"app": "xterm -e sh -c id"}),
    );
    assert!(out.is_error);
    assert!(
        e.backend()
            .events
            .iter()
            .any(|ev| matches!(ev, Event::Launch(q) if q == "xterm -e sh -c id"))
    );
    // Nothing that could read as an option.
    for app in [" --args x", "notes\n--args"] {
        let out = e.call_tool("launch_app", serde_json::json!({ "app": app }));
        assert!(
            out.is_error && out.text.contains("no options"),
            "{app:?}: {}",
            out.text
        );
    }
}

#[test]
fn screenshot_regions_off_the_screen_are_refused_not_fatal() {
    let mut e = engine();
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "region", "x": 1.0e9, "y": 5, "width": 1.0e12, "height": 10}),
    );
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("not on any display"), "{}", out.text);
    // Partly on a display: the visible part is captured.
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "region", "x": -50, "y": -50, "width": 100, "height": 100}),
    );
    assert!(!out.is_error, "{}", out.text);
}

#[test]
fn resolve_by_pid_and_ambiguity() {
    let a = AppInfo {
        name: "Safari".into(),
        id: "com.apple.Safari".into(),
        pid: 1,
        exe: None,
        frontmost: false,
        hidden: false,
    };
    let b = AppInfo {
        name: "Safari Technology Preview".into(),
        id: "com.apple.SafariTechnologyPreview".into(),
        pid: 2,
        exe: None,
        frontmost: true,
        hidden: false,
    };
    let apps = vec![a.clone(), b.clone()];
    assert_eq!(resolve_app_in(&apps, "1").unwrap().pid, 1);
    assert_eq!(resolve_app_in(&apps, "com.apple.Safari").unwrap().pid, 1);
    // Exact name beats the frontmost tiebreak.
    assert_eq!(resolve_app_in(&apps, "Safari").unwrap().pid, 1);
    // Part of two different apps' names: never a guess, even for the
    // frontmost one.
    assert!(matches!(
        resolve_app_in(&apps, "afari"),
        Err(Error::AmbiguousApp { .. })
    ));
    assert!(resolve_app_in(&apps, "Firefox").is_err());
    // Two processes of one app: the frontmost.
    let note = |pid, frontmost| AppInfo {
        name: "Notepad".into(),
        id: format!("notepad-{pid}"),
        pid,
        exe: None,
        frontmost,
        hidden: false,
    };
    let two = vec![note(7, false), note(8, true)];
    assert_eq!(resolve_app_in(&two, "notep").unwrap().pid, 8);
    assert_eq!(resolve_app_in(&two, "Notepad").unwrap().pid, 8);
}

#[test]
fn find_element_filters() {
    let mut e = engine();
    let out = e
        .call_tool(
            "find_element",
            serde_json::json!({"app": "TextEdit", "role": "button"}),
        )
        .text;
    assert!(out.contains("Bold"), "{out}");
    assert!(!out.contains("text area"), "{out}");

    let out = e.call_tool(
        "find_element",
        serde_json::json!({"app": "TextEdit", "editable": true}),
    );
    assert!(out.text.contains("text area"), "{}", out.text);
}

#[test]
fn wait_for_finds_immediately() {
    let mut e = engine();
    let out = e.call_tool(
        "wait_for",
        serde_json::json!({"app": "TextEdit", "name": "Bold", "timeout_ms": 500}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("Found after waiting"));

    let out = e.call_tool(
        "wait_for",
        serde_json::json!({"app": "TextEdit", "name": "Nonexistent", "timeout_ms": 60, "poll_ms": 20})
    );
    assert!(out.is_error);
    assert!(out.text.contains("timed out"));
}

#[test]
fn clipboard_round_trips() {
    let mut e = engine();
    let out = e.call_tool("set_clipboard", serde_json::json!({"text": "hello clip"}));
    assert!(!out.is_error);
    let out = e.call_tool("get_clipboard", serde_json::json!({}));
    assert!(out.text.contains("hello clip"), "{}", out.text);
}

#[test]
fn screenshot_full_and_region() {
    let mut e = engine();
    let out = e.call_tool("screenshot", serde_json::json!({"mode": "full"}));
    assert!(out.image.is_some());
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "region", "x": 0, "y": 0, "width": 100, "height": 50}),
    );
    assert!(out.image.is_some());
    // Region without full coords is an error.
    let out = e.call_tool("screenshot", serde_json::json!({"mode": "region", "x": 0}));
    assert!(out.is_error);
}

#[test]
fn screenshot_window_annotated() {
    let mut e = engine();
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "window", "app": "TextEdit", "annotate": true}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some());
}

#[test]
fn batch_runs_steps_and_stops_on_error() {
    let mut e = engine();
    let out = e.call_tool(
        "batch",
        serde_json::json!({
            "app": "TextEdit",
            "steps": [
                {"tool": "get_app_state", "arguments": {}},
                {"tool": "set_value", "arguments": {"element_index": 4, "value": "hi"}}
            ]
        }),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("1. get_app_state"));
    assert!(out.text.contains("2. set_value"));

    // A failing step stops the batch.
    let out = e.call_tool(
        "batch",
        serde_json::json!({
            "app": "TextEdit",
            "steps": [
                {"tool": "click", "arguments": {"element_index": 999}},
                {"tool": "get_app_state", "arguments": {}}
            ]
        }),
    );
    assert!(out.is_error);
    assert!(
        !out.text.contains("2. get_app_state"),
        "should stop early: {}",
        out.text
    );
}

/// A step that panics ends the batch with an error, and leaves nothing of
/// it behind: before, the steps' quiet reports (and the element they aimed
/// at) outlived it, so later calls inside a call reported no changes.
#[test]
fn a_batch_step_that_panics_leaves_no_state_behind() {
    let mut e = engine();
    e.backend_mut().panic_on_key = Some("ctrl+p".into());
    let out = e.call_tool(
        "batch",
        serde_json::json!({
            "app": "TextEdit",
            "steps": [
                {"tool": "get_app_state", "arguments": {}},
                {"tool": "click", "arguments": {"element_index": 4}},
                {"tool": "press_key", "arguments": {"key": "ctrl+p"}},
                {"tool": "press_key", "arguments": {"key": "a"}}
            ]
        }),
    );
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("failed unexpectedly"), "{}", out.text);
    assert_eq!(e.ctx.depth, 0);
    assert_eq!(e.ctx.quiet_depth, None);
    assert_eq!(e.ctx.target, None);
    assert!(e.ctx.last_expect.is_none() && !e.ctx.in_script);
    // The engine goes on as before.
    e.backend_mut().panic_on_key = None;
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "a"}),
    );
    assert!(!out.is_error, "{}", out.text);
}

#[test]
fn change_report_appended_after_action() {
    let mut e = engine();
    e.call_tool("get_app_state", serde_json::json!({"app": "TextEdit"}));
    let doc = e
        .state(4242)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.role == "text area")
        .unwrap()
        .index;
    let out = e.call_tool(
        "set_value",
        serde_json::json!({"app": "TextEdit", "element_index": doc, "value": "changed!"}),
    );
    assert!(out.text.contains("State after the action"), "{}", out.text);
    assert!(out.text.contains("changed!"), "{}", out.text);
}
// -- screen memory & caches ------------------------------------------

use crate::mock::{MockApp, MockElement, MockWindow};

fn button(h: u64, name: &str, parent: u64, x: f64) -> MockElement {
    MockElement::new(h, "button", name, Rect::new(x, 8.0, 50.0, 24.0))
        .child_of(parent)
        .with_actions(&["AXPress"])
}

/// Page A: the editor plus a "Next" button.
fn page_a(pid: u32) -> MockApp {
    let mut a = MockBackend::text_editor(pid);
    a.elements.push(button(20, "Next", 2, 200.0));
    a
}

/// Page B: same toolbar, a list instead of the document, and "Back".
fn page_b(pid: u32) -> MockApp {
    let mut a = MockBackend::text_editor(pid);
    a.elements.retain(|e| e.handle != 5);
    a.elements.push(button(30, "Back", 2, 200.0));
    a.elements.push(
        MockElement::new(31, "list", "Results", Rect::new(0.0, 40.0, 800.0, 500.0)).child_of(1),
    );
    for i in 0..4u64 {
        a.elements.push(
            MockElement::new(
                32 + i,
                "list item",
                &format!("Result {i}"),
                Rect::new(0.0, 40.0 + 30.0 * i as f64, 800.0, 30.0),
            )
            .child_of(31)
            .with_actions(&["AXPress"]),
        );
    }
    a
}

fn nav_engine(report: bool) -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    backend.add_app(page_a(7));
    backend.on_press.insert(20, page_b(7));
    backend.on_press.insert(30, page_a(7));
    let mut cfg = Config::default();
    cfg.tree.report_changes = report;
    Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
}

fn index_named(e: &Engine<MockBackend>, pid: u32, name: &str) -> u32 {
    e.state(pid)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no {name}"))
        .index
}

fn state_of(e: &mut Engine<MockBackend>, extra: serde_json::Value) -> ToolOutput {
    let mut args = serde_json::json!({"app": "TextEdit"});
    if let (Some(a), Some(x)) = (args.as_object_mut(), extra.as_object()) {
        a.extend(x.clone());
    }
    let out = e.call_tool("get_app_state", args);
    assert!(!out.is_error, "{}", out.text);
    out
}

fn press(e: &mut Engine<MockBackend>, index: u32) -> ToolOutput {
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": index}),
    );
    assert!(!out.is_error, "{}", out.text);
    out
}

fn press_named(e: &mut Engine<MockBackend>, pid: u32, name: &str) -> ToolOutput {
    let i = index_named(e, pid, name);
    press(e, i)
}

#[test]
fn lines_an_action_already_showed_are_not_sent_again() {
    let mut backend = MockBackend::new();
    backend.add_app(page_a(7));
    backend.on_press.insert(20, page_b(7));
    let mut cfg = Config::default();
    cfg.tree.report_changes_max_lines = 4;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    let out = press_named(&mut e, 7, "Next");
    assert!(
        out.text.contains("more lines; call get_app_state"),
        "{}",
        out.text
    );
    let shown: Vec<String> = out
        .text
        .lines()
        .skip_while(|l| !l.starts_with("State after the action"))
        .skip(1)
        .take(4)
        .map(String::from)
        .collect();
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("the rest:"), "{}", out.text);
    for line in &shown {
        assert!(
            !out.text.contains(line.as_str()),
            "{line} again:\n{}",
            out.text
        );
    }
    assert!(out.text.contains("Result 3"), "{}", out.text);

    // Asked for in full, or changed meanwhile: all of it.
    let mut e2 = nav_engine(true);
    e2.store.config.tree.report_changes_max_lines = 4;
    state_of(&mut e2, serde_json::json!({}));
    press_named(&mut e2, 7, "Next");
    let out = state_of(&mut e2, serde_json::json!({"disable_diff": true}));
    assert!(!out.text.contains("the rest:"), "{}", out.text);
}

#[test]
fn a_settings_file_caught_half_saved_is_not_taken() {
    let dir = std::env::temp_dir().join(format!("cu-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[tools]\ndisabled = [\"drag\"]\n").unwrap();
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut e = Engine::new(backend, ConfigStore::load(Some(&path)).unwrap())
        .with_time(Instant::now, |_| {});
    e.reload_if_changed();
    assert!(!e.store.config.tools.is_enabled("drag"));
    let touch = |secs: u64| {
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(std::time::SystemTime::now() + Duration::from_secs(secs))
            .unwrap();
    };
    // An editor empties the file before writing it.
    std::fs::write(&path, "").unwrap();
    touch(5);
    e.reload_if_changed();
    assert!(!e.store.config.tools.is_enabled("drag"), "taken half-saved");
    // Still empty the next time: the user emptied it.
    e.reload_if_changed();
    assert!(e.store.config.tools.is_enabled("drag"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_returning_screen_that_looks_different_gets_a_new_picture() {
    let mut e = nav_engine(false);
    state_of(&mut e, serde_json::json!({}));
    let next = index_named(&e, 7, "Next");
    press(&mut e, next);
    state_of(&mut e, serde_json::json!({}));
    let back = index_named(&e, 7, "Back");
    press(&mut e, back);
    // Same elements as before, different pixels (say, a new image).
    e.backend_mut().fill = 40;
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("screen #1 (seen before)"), "{}", out.text);
    assert!(out.image.is_some(), "{}", out.text);
}

#[test]
fn returning_to_a_seen_screen_skips_tree_and_screenshot() {
    let mut e = nav_engine(false);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("screen #1 (new)"), "{}", out.text);
    assert!(out.image.is_some());
    let (doc, next) = (index_named(&e, 7, "Document"), index_named(&e, 7, "Next"));

    press(&mut e, next);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("screen #2 (new)"), "{}", out.text);
    assert!(out.text.contains("Result 0"), "{}", out.text);
    assert!(out.image.is_some(), "a new screen gets a picture");
    let back = index_named(&e, 7, "Back");
    assert!(back > next, "numbers are never reused: {back} vs {next}");

    press(&mut e, back);
    let captures = e.backend().captures;
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("screen #1 (seen before)"), "{}", out.text);
    assert!(
        out.text
            .contains("Identical to when you last saw screen #1"),
        "{}",
        out.text
    );
    assert!(
        !out.text.contains("text area"),
        "no tree re-sent: {}",
        out.text
    );
    // The pixels are checked, not assumed: the same picture isn't sent.
    assert!(out.image.is_none(), "no screenshot re-sent");
    assert_eq!(e.backend().captures, captures + 1, "checked once");
    assert!(
        out.text.contains("unchanged since you last saw it"),
        "{}",
        out.text
    );
    // The indices the model analysed are back.
    assert_eq!(index_named(&e, 7, "Document"), doc);
    assert_eq!(index_named(&e, 7, "Next"), next);
    // An index from page B is refused rather than hitting something else.
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": back}),
    );
    assert!(out.is_error, "{}", out.text);

    // Coordinates from page A's screenshot still map (1:1 here).
    e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "x": 100, "y": 100}),
    );
    assert!(matches!(
        e.backend().events.last().unwrap(),
        Event::Click(7, p, _, _) if (p.x - 100.0).abs() < 1.0 && (p.y - 100.0).abs() < 1.0
    ));
}

#[test]
fn returning_screen_reports_only_what_changed_since() {
    let mut e = nav_engine(false);
    state_of(&mut e, serde_json::json!({}));
    let next = index_named(&e, 7, "Next");
    let doc = index_named(&e, 7, "Document");
    press(&mut e, next);
    state_of(&mut e, serde_json::json!({}));
    // Going back finds the document edited meanwhile.
    let mut edited = page_a(7);
    edited
        .elements
        .iter_mut()
        .find(|el| el.handle == 5)
        .unwrap()
        .value = Some("Edited".into());
    e.backend_mut().on_press.insert(30, edited);
    press_named(&mut e, 7, "Back");
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(
        out.text.contains("Changes since you last saw screen #1"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains(&format!("~ {doc} text area")),
        "{}",
        out.text
    );
    assert!(!out.text.contains("button \"Bold\""), "{}", out.text);
}

#[test]
fn change_report_announces_new_and_returning_screens() {
    let mut e = nav_engine(true);
    state_of(&mut e, serde_json::json!({}));
    let out = press_named(&mut e, 7, "Next");
    assert!(out.text.contains("now on screen #2 (new)"), "{}", out.text);
    assert!(out.text.contains("Result 3"), "{}", out.text);
    let out = press_named(&mut e, 7, "Back");
    assert!(
        out.text.contains("back on screen #1 (seen before)"),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("Identical to when you last saw screen #1")
    );
    // The report was the model's view of it: nothing new to say now.
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("nothing changed"), "{}", out.text);
    assert!(out.image.is_none());
}

fn pointer_events(e: &Engine<MockBackend>) -> Vec<Event> {
    e.backend()
        .events
        .iter()
        .filter(|ev| {
            matches!(
                ev,
                Event::PointerDown(..) | Event::PointerMove(..) | Event::PointerUp(..)
            )
        })
        .cloned()
        .collect()
}

#[test]
fn draw_follows_a_parametric_curve() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [
            {"x": "400 + 100*cos(t)", "y": "300 + 100*sin(t)", "t": [0, "2*pi"]}
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("Drew 1 stroke ("), "{}", out.text);
    assert!(
        out.text.contains("x 300 to 500, y 200 to 400"),
        "{}",
        out.text
    );
    let events = pointer_events(&e);
    let Event::PointerDown(4242, start, MouseButton::Left) = events[0] else {
        panic!("{:?}", events[0]);
    };
    assert!((start.x - 500.0).abs() < 1e-6 && (start.y - 300.0).abs() < 1e-6);
    let Event::PointerUp(_, end, _) = events.last().unwrap() else {
        panic!("ends with the button up");
    };
    assert!((end.x - start.x).abs() < 1e-6 && (end.y - start.y).abs() < 1e-6);
    let moves: Vec<Point> = events
        .iter()
        .filter_map(|ev| match ev {
            Event::PointerMove(_, p) => Some(*p),
            _ => None,
        })
        .collect();
    assert!(moves.len() > 150, "{}", moves.len());
    for p in &moves {
        let r = (p.x - 400.0).hypot(p.y - 300.0);
        assert!((r - 100.0).abs() < 0.5, "{p:?}");
    }
}

#[test]
fn draw_in_an_element_uses_fractions_of_its_box() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let doc = index_named(&e, 4242, "Document");
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "element_index": doc, "button": "right",
            "strokes": [{"points": [[0, 0], {"x": 1, "y": 1}]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    let events = pointer_events(&e);
    // The text area is (0, 40, 800, 560) on screen.
    assert_eq!(
        events[0],
        Event::PointerDown(4242, Point::new(0.0, 40.0), MouseButton::Right)
    );
    assert_eq!(
        *events.last().unwrap(),
        Event::PointerUp(4242, Point::new(800.0, 600.0), MouseButton::Right)
    );
    assert!(out.text.contains("x 0.00 to 1.00"), "{}", out.text);
}

#[test]
fn the_stop_key_ends_a_drawing_with_the_button_up() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    // The first pause in the drawing is when the user presses stop.
    let stop = e.stop_handle();
    let mut e = e.with_time(Instant::now, move |_| stop.store(true, Ordering::SeqCst));
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "speed": 50,
            "strokes": [{"points": [[10, 100], [700, 100]]}]}),
    );
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.to_lowercase().contains("stop"), "{}", out.text);
    let events = pointer_events(&e);
    assert!(events.len() < 20, "stopped early: {}", events.len());
    assert!(
        matches!(events.last(), Some(Event::PointerUp(..))),
        "{events:?}"
    );
}

#[test]
fn rects_and_ellipses_need_no_formulas() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [
            {"rect": [100, 100, 200, 100]},
            {"ellipse": [400, 300, 100, 50]}
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    let downs: Vec<Point> = pointer_events(&e)
        .iter()
        .filter_map(|ev| match ev {
            Event::PointerDown(_, p, _) => Some(*p),
            _ => None,
        })
        .collect();
    assert_eq!(downs, [Point::new(100.0, 100.0), Point::new(500.0, 300.0)]);
    assert!(out.text.contains("Drew 2 strokes"), "{}", out.text);
    assert!(
        out.text.contains("x 100 to 500, y 100 to 350"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [{"rect": [1, 1, 5, 5], "ellipse": [1, 1, 1, 1]}]}),
    );
    assert!(
        out.text
            .contains("give one kind of stroke, not rect and ellipse"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [{"ellipse": [1, 1, 0, 1]}]}),
    );
    assert!(out.text.contains("positive radii"), "{}", out.text);
}

#[test]
fn draw_in_document_units() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    // A 1000x500 px document shown at (100, 100)-(500, 300).
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "canvas": {"box": [100, 100, 500, 300], "size": [1000, 500]},
            "strokes": [{"points": [[0, 0], [1000, 500]]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    let events = pointer_events(&e);
    assert_eq!(
        events[0],
        Event::PointerDown(4242, Point::new(100.0, 100.0), MouseButton::Left)
    );
    assert_eq!(
        *events.last().unwrap(),
        Event::PointerUp(4242, Point::new(500.0, 300.0), MouseButton::Left)
    );
    assert!(
        out.text
            .contains("on the document: x 0 to 1000, y 0 to 500"),
        "{}",
        out.text
    );
    // The element's box with the document's size.
    let doc = index_named(&e, 4242, "Document");
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "element_index": doc, "canvas": {"size": [800, 560]},
            "strokes": [{"points": [[400, 280]]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(pointer_events(&e).contains(&Event::PointerDown(
        4242,
        Point::new(400.0, 320.0),
        MouseButton::Left
    )));
    for (canvas, want) in [
        (serde_json::json!({"size": [10, 10]}), "needs box"),
        (
            serde_json::json!({"box": [1, 1, 5, 5], "size": [0, 10]}),
            "both positive",
        ),
        (
            serde_json::json!({"box": [50, 50, 10, 10], "size": [10, 10]}),
            "right of left",
        ),
    ] {
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "canvas": canvas, "strokes": [{"points": [[1, 1]]}]}),
        );
        assert!(
            out.is_error && out.text.contains(want),
            "{want}: {}",
            out.text
        );
    }
}

fn downs(e: &Engine<MockBackend>) -> Vec<Point> {
    pointer_events(e)
        .iter()
        .filter_map(|ev| match ev {
            Event::PointerDown(_, p, _) => Some(*p),
            _ => None,
        })
        .collect()
}

#[test]
fn plots_use_math_coordinates_with_axes() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    // x -pi..pi, y -1.5..1.5 across (100, 100)-(700, 400).
    let pi = std::f64::consts::PI;
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit",
            "canvas": {"box": [100, 100, 700, 400], "range": ["-3", 3, -1.5, 1.5]},
            "strokes": [{"axes": [1, 0.5]}, {"y": "sin(x)"}]}),
    );
    // A range must be numbers.
    assert!(out.is_error, "{}", out.text);
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit",
            "canvas": {"box": [100, 100, 700, 400], "range": [-pi, pi, -1.5, 1.5]},
            "strokes": [{"axes": [1, 0.5]}, {"y": "sin(x)"}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("Drew 17 strokes"), "{}", out.text);
    assert!(out.text.contains("on the plot"), "{}", out.text);
    // The sine starts at x = -pi, y = 0: the left edge, half way down.
    let d = downs(&e);
    let start = d.last().unwrap();
    assert!(
        (start.x - 100.0).abs() < 0.01 && (start.y - 250.0).abs() < 0.01,
        "{start:?}"
    );
    // Its top (x = pi/2, y = 1) is 100 px above the middle.
    let events = pointer_events(&e);
    let sine_start = events
        .iter()
        .rposition(|ev| matches!(ev, Event::PointerDown(..)))
        .unwrap();
    let top = events[sine_start..]
        .iter()
        .filter_map(|ev| match ev {
            Event::PointerMove(_, p) => Some(p.y),
            _ => None,
        })
        .fold(f64::MAX, f64::min);
    assert!((top - 150.0).abs() < 0.5, "{top}");
}

#[test]
fn shapes_turn_repeat_and_report_where_to_fill() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [
            {"rect": [100, 100, 200, 100], "rotate": 90},
            {"star": [500, 300, 60, 25, 5], "repeat": {"count": 3, "offset": [10, 0]}}
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("Drew 4 strokes"), "{}", out.text);
    // Turned a quarter about its centre (200, 150): now 100 wide, 200 tall.
    assert!(
        out.text
            .contains("To fill a closed outline (bucket or magic wand), click stroke 1 at (200, 150); stroke 2 (part 1), cut into 5 pieces by other lines, at (500, 300), "),
        "{}",
        out.text
    );
    let d = downs(&e);
    assert_eq!(d.len(), 4);
    // The rect's first corner (100, 100) turned clockwise about (200, 150).
    assert!(
        (d[0].x - 250.0).abs() < 1e-6 && (d[0].y - 50.0).abs() < 1e-6,
        "{:?}",
        d[0]
    );
    // Stars start at their top point, each copy 10 px to the right.
    assert!(
        (d[1].x - 500.0).abs() < 1e-6 && (d[1].y - 240.0).abs() < 1e-6,
        "{:?}",
        d[1]
    );
    assert!((d[3].x - 520.0).abs() < 1e-6);
    // Polygons, arcs, bezier.
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [
            {"polygon": [300, 300, 50, 6]},
            {"arc": [300, 300, 80, 0, 90]},
            {"bezier": [[100, 500], [150, 400], [250, 400], [300, 500]]}
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("Drew 3 strokes"), "{}", out.text);
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [{"polygon": [300, 300, 50, 2.5]}]}),
    );
    assert!(out.text.contains("whole number from 3"), "{}", out.text);
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [{"axes": [1, 1]}]}),
    );
    assert!(out.text.contains("axes need canvas.range"), "{}", out.text);
}

#[test]
fn previews_show_the_strokes_without_drawing() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "preview": true,
            "canvas": {"box": [0, 0, 800, 600], "size": [1600, 1200]},
            "strokes": [{"ellipse": [800, 600, 300, 300]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text
            .starts_with("Preview only, nothing was drawn: 1 stroke"),
        "{}",
        out.text
    );
    assert!(out.text.contains("on the document"), "{}", out.text);
    assert!(out.image.is_some());
    assert!(pointer_events(&e).is_empty(), "nothing drawn");
}

#[test]
fn fill_paints_shapes_solid_with_the_brush() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit",
            "canvas": {"box": [0, 0, 800, 600], "size": [800, 600]},
            "strokes": [{"rect": [100, 100, 200, 100], "fill": 10},
                        {"ellipse": [500, 300, 60, 60]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text
            .contains("Solid shapes are painted for a brush 10 px wide on screen"),
        "{}",
        out.text
    );
    // Only the outline needs a bucket.
    assert!(
        out.text.contains("click stroke 2 at (500, 300)"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("stroke 1 at"), "{}", out.text);
    // The brush stays half its width inside the rectangle, and its
    // rows are close enough to leave no stripes.
    let mut rows: Vec<f64> = Vec::new();
    for ev in pointer_events(&e) {
        let (Event::PointerDown(_, p, _) | Event::PointerMove(_, p) | Event::PointerUp(_, p, _)) =
            ev
        else {
            continue;
        };
        if p.x < 400.0 {
            assert!(
                (104.9..=295.1).contains(&p.x) && (104.9..=195.1).contains(&p.y),
                "{p:?}"
            );
            rows.push(p.y);
        }
    }
    rows.sort_by(f64::total_cmp);
    rows.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    assert!(rows.first().is_some_and(|y| *y < 106.0), "{rows:?}");
    assert!(rows.last().is_some_and(|y| *y > 194.0), "{rows:?}");
    assert!(rows.windows(2).all(|w| w[1] - w[0] <= 6.01), "{rows:?}");
    // An open line can't be painted solid.
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit",
            "strokes": [{"points": [[10, 10], [50, 50]], "fill": 5}]}),
    );
    assert!(
        out.is_error && out.text.contains("fill needs a closed shape"),
        "{}",
        out.text
    );
}

#[test]
fn previews_find_shapes_that_other_lines_cut() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "preview": true,
            "strokes": [{"ellipse": [400, 300, 100, 100]},
                        {"points": [[250, 300], [550, 300]]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text
            .contains("stroke 1, cut into 2 pieces by other lines, at "),
        "{}",
        out.text
    );
    assert!(pointer_events(&e).is_empty(), "nothing drawn");
}

#[test]
fn traced_pictures_are_painted_step_by_step_and_compared() {
    // A picture file: a red disc on white, twice as wide as high.
    let dir = std::env::temp_dir().join(format!("cu-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Logo Mark.png");
    let (w, h) = (200u32, 100u32);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let d = (f64::from(x) - 100.0).hypot(f64::from(y) - 50.0);
            rgba.extend_from_slice(if d < 30.0 {
                &[220, 30, 30, 255]
            } else {
                &[255, 255, 255, 255]
            });
        }
    }
    image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).unwrap();

    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "trace_image",
        serde_json::json!({"path": path.to_string_lossy(), "colors": 2}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text
            .contains("Traced \"logo-mark\" (Logo Mark.png, 200 x 100 px) as 2 steps"),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("\n1. #FEFEFE: 1 shape, 100% of the picture (all of it")
            || out
                .text
                .contains("\n1. #FFFFFF: 1 shape, 100% of the picture (all of it"),
        "{}",
        out.text
    );
    assert!(out.text.contains("\n2. #DC1E1E: 1 shape"), "{}", out.text);
    assert!(out.image.is_some());

    // Step 2 into a 400 x 200 document shown at (0, 0)-(800, 400).
    let canvas = serde_json::json!({"box": [0, 0, 800, 400], "size": [400, 200]});
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "canvas": canvas,
            "strokes": [{"trace": "logo-mark", "step": 2, "fill": 4}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    // The disc: centre (400, 200), radius 120 on screen; the brush runs
    // on its edge, traced from a 160-pixel-wide working copy (one of
    // its pixels is 5 on screen).
    for ev in pointer_events(&e) {
        if let Event::PointerMove(_, p) = ev {
            assert!((p.x - 400.0).hypot(p.y - 200.0) <= 130.0, "{p:?}");
        }
    }
    for (strokes, want) in [
        (
            serde_json::json!([{"trace": "logo-mark", "step": 2}]),
            "give fill",
        ),
        (
            serde_json::json!([{"trace": "logo", "step": 2, "fill": 4}]),
            "no picture called \"logo\" (traced: logo-mark)",
        ),
        (
            serde_json::json!([{"trace": "logo-mark", "step": 3, "fill": 4}]),
            "has steps 1 to 2",
        ),
        (
            serde_json::json!([{"trace": "logo-mark", "step": 1, "fill": 4, "rect": [0, 0, 5, 5]}]),
            "on its own",
        ),
    ] {
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "canvas": canvas, "strokes": strokes}),
        );
        assert!(
            out.is_error && out.text.contains(want),
            "{want}: {}",
            out.text
        );
    }
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit",
            "strokes": [{"trace": "logo-mark", "step": 1, "fill": 4}]}),
    );
    assert!(
        out.is_error && out.text.contains("needs canvas or element_index"),
        "{}",
        out.text
    );

    // Compared with the canvas: grey where white should be.
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "canvas": canvas, "compare": "logo-mark"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("Compared with \"logo-mark\": "),
        "{}",
        out.text
    );
    // Grey canvas: nothing alike, the red disc's cells most of all.
    assert!(
        out.text.contains("0 of 64 cells look alike")
            && out
                .text
                .contains("Most different: x 150 to 200, y 75 to 100 should be #DC1E1E but is "),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "compare": "logo-mark"}),
    );
    assert!(
        out.is_error && out.text.contains("compare needs canvas"),
        "{}",
        out.text
    );

    // What a window shows can be traced too.
    let out = e.call_tool(
        "trace_image",
        serde_json::json!({"app": "TextEdit", "box": [0, 450, 800, 600], "name": "Strip"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("Traced \"strip\" (TextEdit window"),
        "{}",
        out.text
    );
    assert!(out.text.contains("800 x 150 px"), "{}", out.text);
    let out = e.call_tool(
        "trace_image",
        serde_json::json!({"path": dir.join("nope.png").to_string_lossy()}),
    );
    assert!(
        out.is_error && out.text.contains("can't read"),
        "{}",
        out.text
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn designs_are_composed_seen_and_painted_step_by_step() {
    let mut e = steps_engine();
    state_of(&mut e, serde_json::json!({}));
    // A name nobody started yet needs a size.
    let out = e.call_tool("design", serde_json::json!({"name": "Badge"}));
    assert!(
        out.is_error && out.text.contains("give size"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "Badge", "size": [400, 200], "background": "#FFFFFF",
            "add": [{"id": "disc", "ellipse": [100, 100, 60, 60], "fill": "#CC2222"},
                    {"id": "ring", "ellipse": [100, 100, 80, 80], "fill": "none", "stroke": "#222222", "width": 4},
                    {"id": "label", "text": "OK", "at": [280, 80], "size": 40}],
            "mirror": [{"id": "disc", "as": "disc-2"}],
            "show": {"guides": true, "ids": true, "grid": 50}}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some());
    assert!(
        out.text.starts_with("Design \"badge\": 400 x 200, background #FFFFFF, margin 10. 4 layers, back to front: disc ellipse x 40 y 40 w 120 h 120 fill #CC2222; disc-2 ellipse x 240 y 40"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains(
            "steps 1 #CC2222 solid (disc, disc-2); 2 #222222 lines 4 (ring); 3 #000000 text (label)"
        ),
        "{}",
        out.text
    );

    // A change that fails leaves the design as it was.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "badge", "change": [{"id": "disc", "fill": "#00FF00"}], "remove": ["nope"]}),
    );
    assert!(
        out.is_error && out.text.contains("no layer \"nope\""),
        "{}",
        out.text
    );
    let out = e.call_tool("design", serde_json::json!({"name": "badge"}));
    assert!(
        out.text
            .contains("disc ellipse x 40 y 40 w 120 h 120 fill #CC2222"),
        "{}",
        out.text
    );

    // Painted into an 800 x 400 document shown at (0, 0)-(800, 400):
    // twice the size.
    let canvas = serde_json::json!({"box": [0, 0, 800, 400], "size": [800, 400]});
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "canvas": canvas,
            "strokes": [{"design": "badge", "step": 1, "fill": 8}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    for ev in pointer_events(&e) {
        if let Event::PointerMove(_, p) = ev {
            let d = (p.x - 200.0)
                .hypot(p.y - 200.0)
                .min((p.x - 600.0).hypot(p.y - 200.0));
            assert!(d <= 120.5, "{p:?}");
        }
    }
    // The ring is a line step: no fill needed; the text step is typed.
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "canvas": canvas,
            "strokes": [{"design": "badge", "step": 2}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    for (strokes, want) in [
        (
            serde_json::json!([{"design": "badge", "step": 1}]),
            "give fill",
        ),
        (
            serde_json::json!([{"design": "badge", "step": 3}]),
            "step 3 is the text of label: type it with the app's text tool at (280, 80)",
        ),
        (
            serde_json::json!([{"design": "badge", "step": 9}]),
            "has steps 1 to 3",
        ),
        (
            serde_json::json!([{"design": "nope", "step": 1}]),
            "no design called \"nope\" (designs: badge)",
        ),
        (
            serde_json::json!([{"design": "badge", "trace": "x", "step": 1}]),
            "trace or design, not both",
        ),
    ] {
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "canvas": canvas, "strokes": strokes}),
        );
        assert!(
            out.is_error && out.text.contains(want),
            "{want}: {}",
            out.text
        );
    }

    // Exports are temporary files.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "badge", "export": "svg"}),
    );
    let path = out
        .text
        .split("Exported to ")
        .nth(1)
        .and_then(|r| r.split(" (").next())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("{}", out.text));
    let svg = std::fs::read_to_string(&path).unwrap();
    assert!(svg.contains("<ellipse id=\"disc-2\" cx=\"300\""), "{svg}");
    assert!(out.text.contains("It is temporary"), "{}", out.text);
    drop(e);
    assert!(!path.exists(), "deleted with the server");
}

#[test]
fn scenes_are_planned_seen_checked_and_exported() {
    let mut e = engine();
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "Stool",
            "add": [{"id": "seat", "shape": "cylinder", "size": [0.4, 0.04], "at": [0, 0, 0.47], "color": "#8B5A2B"},
                    {"id": "leg", "shape": "box", "size": [0.04, 0.04, 0.45], "at": [0.12, 0.12, 0.225]},
                    {"id": "lamp", "shape": "sphere", "size": [0.1], "at": [0, 0, 0.6]}],
            "mirror": [{"id": "leg", "as": "leg-b"}],
            "repeat": [{"id": "leg", "count": 2, "offset": [0, -0.24, 0]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some());
    assert!(
        out.text.starts_with("Scene \"stool\": 5 objects, 0.4 x 0.4 x 0.65 (x -0.2 to 0.2, y -0.2 to 0.2, z 0 to 0.65), Z up, the ground at z 0. seat cylinder 0.4 x 0.4 x 0.04 at (0, 0, 0.47), z 0.45 to 0.49, #8B5A2B;"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("lamp floats in the air: nothing holds it; the bottom of lamp is 0.06 above seat (move it down 0.06"),
        "{}",
        out.text
    );
    assert!(out.text.contains("In Blender: Add > Mesh"), "{}", out.text);
    assert!(out.text.contains("a grid line every 0.1"), "{}", out.text);

    // A bad change leaves the scene as it was; a good one fixes it.
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "change": [{"id": "lamp", "on": "seat"}], "remove": ["nope"]}),
    );
    assert!(
        out.is_error && out.text.contains("no object \"nope\""),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "change": [{"id": "lamp", "on": "seat"}], "view": "top"}),
    );
    assert!(
        out.text
            .contains("Checks: everything rests on the ground or on something"),
        "{}",
        out.text
    );
    assert!(out.text.contains("Build: Location = at"), "{}", out.text);

    // Exports are temporary: the model and its colours.
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "export": "obj"}),
    );
    let path = out
        .text
        .split("Exported to ")
        .nth(1)
        .and_then(|r| r.split(" with").next())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("{}", out.text));
    let obj = std::fs::read_to_string(&path).unwrap();
    let mtl_name = obj
        .lines()
        .find_map(|l| l.strip_prefix("mtllib "))
        .unwrap()
        .to_string();
    let mtl = path.with_file_name(&mtl_name);
    assert!(
        std::fs::read_to_string(&mtl)
            .unwrap()
            .contains("newmtl c8B5A2B")
    );
    assert!(
        obj.contains("o seat\nusemtl c8B5A2B\nv "),
        "{}",
        &obj[..200]
    );
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "export": "png", "view": "perspective"}),
    );
    let png = out
        .text
        .split("Exported to ")
        .nth(1)
        .and_then(|r| r.split(" (").next())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("{}", out.text));
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
    drop(e);
    assert!(
        !path.exists() && !mtl.exists() && !png.exists(),
        "deleted with the server"
    );
}

#[test]
fn pixel_targeting_snaps_finds_and_magnifies() {
    let mut e = engine();
    let mut cfg = e.store().config.clone();
    cfg.screenshot.locate_picture = true;
    e.set_config(ConfigStore::in_memory(cfg));
    state_of(&mut e, serde_json::json!({}));
    // A dark square drawn on the (mock) window: 200..260 x 150..210.
    e.backend_mut().patch = Some((Rect::new(200.0, 150.0, 60.0, 60.0), 20));
    // A click near the square's corner lands on it exactly.
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "x": 204, "y": 147, "snap": "corner"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("Snapped to the corner at ("),
        "{}",
        out.text
    );
    let clicked = e
        .backend()
        .events
        .iter()
        .rev()
        .find_map(|ev| match ev {
            Event::Click(_, p, _, _) => Some(*p),
            _ => None,
        })
        .unwrap();
    assert!(
        (clicked.x - 200.5).abs() <= 1.5 && (clicked.y - 150.5).abs() <= 1.5,
        "{clicked:?}"
    );
    // Nothing to snap to: nothing is clicked.
    let before = e.backend().events.len();
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "x": 500, "y": 400, "snap": "edge", "snap_radius": 5}),
    );
    assert!(
        out.is_error
            && out
                .text
                .contains("no edge within 5 pixels of (500, 400), so nothing was done"),
        "{}",
        out.text
    );
    assert_eq!(e.backend().events.len(), before);

    // Areas of a colour, look-alikes, and the centre near a point.
    let out = e.call_tool(
        "locate",
        serde_json::json!({"app": "TextEdit", "color": "#141414"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("1 area of #141414 (within 16), biggest first: 1 at (230.0, 180.0), box 200,150 to 260,210"), "{}", out.text);
    assert!(out.image.is_some());
    let out = e.call_tool(
        "locate",
        serde_json::json!({"app": "TextEdit", "near": [238, 191], "feature": "center", "radius": 40}),
    );
    assert!(
        out.text
            .contains("The shape centre near (238, 191): (230.0, 180.0), 8.0 left and 11.0 up"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "locate",
        serde_json::json!({"app": "TextEdit", "color": "#141414", "like": [0, 0, 5, 5]}),
    );
    assert!(
        out.is_error && out.text.contains("give one of"),
        "{}",
        out.text
    );

    // The loupe: a magnified view with a crosshair.
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "zoom": [200, 150], "radius": 6}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.starts_with("Magnified around (200, 150)"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("the crosshair is the point, on #141414"),
        "{}",
        out.text
    );
    assert!(out.image.is_some());
}

#[test]
fn screenshot_grids_and_picks_follow_the_canvas() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "grid": true,
            "canvas": {"box": [0, 0, 800, 600], "range": [-4, 4, -3, 3]},
            "pick": [[0, 0]]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text
            .contains("Grid: a line every 1, labelled in the canvas range"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("Colours: (0, 0) #C8C8C8."),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"grid": true, "canvas": {"box": [0, 0, 8, 6], "size": [8, 6]}}),
    );
    assert!(
        out.text.contains("canvas needs a window screenshot"),
        "{}",
        out.text
    );
}

#[test]
fn cells_name_the_page_and_show_one_cell_close() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let canvas = serde_json::json!({"box": [0, 0, 800, 600], "size": [800, 600]});
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "canvas": canvas, "cells": true}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains(
            "Cells over the document: cells of 100: columns A to H from x 0, rows 1 to 6 from y 0 down"
        ),
        "{}",
        out.text
    );

    // One cell up close, with its colours.
    e.backend_mut().patch = Some((Rect::new(210.0, 110.0, 80.0, 80.0), 20));
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "canvas": canvas, "cell": "c2"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.starts_with(
            "Cell C2 of the document: x 200 to 300, y 100 to 200 (shown 3 times bigger"
        ),
        "{}",
        out.text
    );
    assert!(out.text.contains("#141414 64%"), "{}", out.text);
    let img = out.image.unwrap();
    assert!(img.width >= 390 && img.width <= 512, "{}", img.width);

    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "canvas": canvas, "cell": "J9"}),
    );
    assert!(
        out.is_error && out.text.contains("cells are A1 to H6"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "cells": true}),
    );
    assert!(
        out.is_error && out.text.contains("cells need canvas"),
        "{}",
        out.text
    );

    // Drawing says which cells a drawing covers; a small one gets
    // smaller cells.
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "preview": true, "canvas": canvas,
            "strokes": [{"rect": [100, 100, 300, 200]}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text
            .contains("It covers cells B2 to D3 (cells of 100, A1 top-left)."),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "preview": true, "canvas": canvas,
            "strokes": [{"rect": [100, 100, 40, 40]}]}),
    );
    assert!(
        out.text.contains("(cells of 5, A1 top-left)"),
        "{}",
        out.text
    );

    // The design board names its cells and opens one.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "card", "size": [400, 200], "background": "#FFFFFF",
            "add": [{"id": "disc", "ellipse": [100, 100, 60, 60], "fill": "#CC2222"}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains(
            "On the picture, cells of 50: columns A to H from x 0, rows 1 to 4 from y 0 down"
        ),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "card", "show": {"cell": "B2"}}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("Cell B2: x 50 to 100, y 50 to 100")
            && out.text.contains("Layers in it, back to front: disc."),
        "{}",
        out.text
    );
    assert!(out.image.is_some());
}

#[test]
fn keys_can_go_to_what_is_under_the_pointer() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    e.backend_mut().pointer = Some(Point::new(5.0, 5.0));
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "g x 2 Return", "x": 300, "y": 200}),
    );
    assert!(!out.is_error, "{}", out.text);
    let ev = &e.backend().events;
    let first_key = ev.iter().position(|v| matches!(v, Event::Key(..))).unwrap();
    let last_key = ev
        .iter()
        .rposition(|v| matches!(v, Event::Key(..)))
        .unwrap();
    // Pointed there before the first key, put back after the last.
    assert!(ev[..first_key].contains(&Event::Hover(4242, Point::new(300.0, 200.0))));
    assert!(ev[last_key..].contains(&Event::Hover(4242, Point::new(5.0, 5.0))));
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "text": "1.5", "x": 300}),
    );
    assert!(
        out.is_error && out.text.contains("both x and y"),
        "{}",
        out.text
    );
}

#[test]
fn screenshots_read_coordinates_and_colours() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "grid": 100, "palette": true,
            "pick": [[10, 10], [5000, 5]]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some());
    let t = &out.text;
    assert!(t.contains("Main colours: #C8C8C8 100%."), "{t}");
    assert!(
        t.contains("Colours: (10, 10) #C8C8C8; (5000, 5) is outside the image."),
        "{t}"
    );
    assert!(
        t.contains("Grid: a line every 100, labelled in the x/y that click"),
        "{t}"
    );
    // A full-screen grid is in screen coordinates, and always sent.
    let out = e.call_tool("screenshot", serde_json::json!({"grid": true}));
    assert!(out.image.is_some());
    assert!(
        out.text.contains("labelled in screen coordinates"),
        "{}",
        out.text
    );
    let again = e.call_tool("screenshot", serde_json::json!({"grid": true}));
    assert!(again.image.is_some(), "{}", again.text);
}

#[test]
fn bad_drawings_are_explained() {
    let mut e = engine();
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "strokes": [{"points": [[1, 1]]}]}),
    );
    assert!(
        out.text.contains("call get_app_state first"),
        "{}",
        out.text
    );
    state_of(&mut e, serde_json::json!({}));
    for (stroke, want) in [
        (
            serde_json::json!({"x": "foo(t)", "y": "t"}),
            "stroke 1: x: unknown name `foo`",
        ),
        (
            serde_json::json!({"y": "sin(z)"}),
            "y: unknown name `z`: use t or x",
        ),
        (
            serde_json::json!({}),
            "give a shape (rect, ellipse, polygon, star, arc, bezier), points, axes",
        ),
        (
            serde_json::json!({"points": [[1, 1]], "x": "t", "y": "t"}),
            "give one kind of stroke",
        ),
        (
            serde_json::json!({"points": [[1, 1], [5000, 1]]}),
            "outside the drawing area",
        ),
        (
            serde_json::json!({"x": "t", "y": "t", "t": [0, "2*pie"]}),
            "t to: unknown name",
        ),
    ] {
        let out = e.call_tool(
            "draw",
            serde_json::json!({"app": "TextEdit", "strokes": [stroke]}),
        );
        assert!(
            out.is_error && out.text.contains(want),
            "{want}: {}",
            out.text
        );
    }
    assert!(pointer_events(&e).is_empty(), "nothing was drawn");
}

#[test]
fn an_action_naming_another_window_is_refused() {
    let mut app = MockBackend::text_editor(9);
    app.windows.push(MockWindow {
        id: 2,
        title: "Preferences".into(),
        bounds: Rect::new(900.0, 0.0, 300.0, 200.0),
        root: 50,
        focused: false,
    });
    app.elements.push(MockElement::new(
        50,
        "window",
        "Preferences",
        Rect::new(900.0, 0.0, 300.0, 200.0),
    ));
    let mut backend = MockBackend::new();
    backend.add_app(app);
    let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
        .with_time(Instant::now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    let bold = index_named(&e, 9, "Bold");
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold, "window": "Preferences"}),
    );
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("window=\"Preferences\""), "{}", out.text);
    // Naming the window it shows is fine.
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold, "window": "Untitled"}),
    );
    assert!(!out.is_error, "{}", out.text);
}

#[test]
fn dialog_is_followed_and_main_window_recognised_after() {
    let pid = 9;
    let main = || {
        let mut a = MockBackend::text_editor(pid);
        a.elements.push(button(20, "Delete", 2, 200.0));
        a
    };
    let with_dialog = {
        let mut a = main();
        a.windows[0].focused = false;
        a.windows.push(MockWindow {
            id: 2,
            title: "Confirm".into(),
            bounds: Rect::new(200.0, 200.0, 300.0, 120.0),
            root: 40,
            focused: true,
        });
        a.elements.push(MockElement::new(
            40,
            "dialog",
            "Confirm",
            Rect::new(200.0, 200.0, 300.0, 120.0),
        ));
        a.elements.push(
            MockElement::new(
                42,
                "text",
                "Delete the document?",
                Rect::new(210.0, 210.0, 280.0, 20.0),
            )
            .child_of(40),
        );
        a.elements.push(button(41, "OK", 40, 400.0));
        a.elements.last_mut().unwrap().bounds = Rect::new(400.0, 280.0, 60.0, 24.0);
        a
    };
    let mut backend = MockBackend::new();
    backend.add_app(main());
    backend.on_press.insert(20, with_dialog);
    backend.on_press.insert(41, main());
    let cfg = Config::default();
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});

    state_of(&mut e, serde_json::json!({}));
    let delete = index_named(&e, pid, "Delete");
    let out = press(&mut e, delete);
    assert!(
        out.text
            .contains("now on screen #2 (new), window \"Confirm\""),
        "{}",
        out.text
    );
    let ok = index_named(&e, pid, "OK");
    assert!(ok > delete, "dialog numbers don't collide: {ok}");
    let out = press(&mut e, ok);
    assert!(
        out.text
            .contains("back on screen #1 (seen before), window \"Untitled\""),
        "{}",
        out.text
    );
    assert_eq!(index_named(&e, pid, "Delete"), delete);
}

#[test]
fn unchanged_screenshots_are_not_resent() {
    // A custom-drawn app: almost nothing in the tree, so every view wants
    // pixels — but identical pixels are not sent twice.
    let mut backend = MockBackend::new();
    let mut canvas = MockBackend::text_editor(3);
    canvas.elements.retain(|el| el.handle == 1);
    backend.add_app(canvas);
    let mut cfg = Config::default();
    cfg.ocr.mode = crate::config::OcrMode::Off;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    assert!(state_of(&mut e, serde_json::json!({})).image.is_some());
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_none(), "{}", out.text);
    assert!(
        out.text.contains("unchanged since you last saw"),
        "{}",
        out.text
    );
    // Explicitly asked for: always sent.
    assert!(
        state_of(&mut e, serde_json::json!({"screenshot": true}))
            .image
            .is_some()
    );
    // The picture changed: sent again.
    e.backend_mut().fill = 10;
    assert!(state_of(&mut e, serde_json::json!({})).image.is_some());
    // Dedupe off: sent every time.
    let mut cfg = e.store().config.clone();
    cfg.cache.dedupe_screenshots = false;
    e.set_config(ConfigStore::in_memory(cfg));
    assert!(state_of(&mut e, serde_json::json!({})).image.is_some());
}

#[test]
fn recent_snapshots_are_reused_until_an_action() {
    // A clock that stands still: reuse depends on actions, not on how
    // long the (debug-mode) screenshot encoding took.
    let now = Instant::now();
    let mut e = nav_engine(false).with_time(move || now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    let (snaps, lists) = (e.backend().snapshots, e.backend().window_lists);
    e.call_tool(
        "find_element",
        serde_json::json!({"app": "TextEdit", "name": "Next"}),
    );
    state_of(&mut e, serde_json::json!({}));
    assert_eq!(e.backend().snapshots, snaps, "tree read reused");
    assert_eq!(e.backend().window_lists, lists, "window list reused");
    // An action makes them stale: settling reads the app until two reads
    // agree, and the next get_app_state reuses the last of them.
    press_named(&mut e, 7, "Next");
    state_of(&mut e, serde_json::json!({}));
    assert_eq!(e.backend().snapshots, snaps + 2);
    // Turned off: every call reads again.
    let mut cfg = e.store().config.clone();
    cfg.cache.snapshot_ttl_ms = 0;
    e.set_config(ConfigStore::in_memory(cfg));
    state_of(&mut e, serde_json::json!({}));
    state_of(&mut e, serde_json::json!({}));
    assert_eq!(e.backend().snapshots, snaps + 4);
}

#[test]
fn screen_memory_can_be_disabled() {
    let mut e = nav_engine(false);
    let mut cfg = e.store().config.clone();
    cfg.cache.enabled = false;
    e.set_config(ConfigStore::in_memory(cfg));
    state_of(&mut e, serde_json::json!({}));
    press_named(&mut e, 7, "Next");
    state_of(&mut e, serde_json::json!({}));
    press_named(&mut e, 7, "Back");
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("(new)"), "{}", out.text);
    assert!(out.text.contains("text area"), "{}", out.text);
    assert_eq!(e.screen_memory_stats().0, 0);
}

#[test]
fn find_element_keeps_indices_the_model_knows() {
    let mut e = nav_engine(false);
    state_of(&mut e, serde_json::json!({}));
    let next = index_named(&e, 7, "Next");
    let out = e.call_tool(
        "find_element",
        serde_json::json!({"app": "TextEdit", "name": "Next"}),
    );
    assert!(
        out.text.contains(&format!("{next} button \"Next\"")),
        "{}",
        out.text
    );
}

#[test]
fn batch_only_counts_the_screenshot_that_is_returned() {
    // Step 1's window screenshot is replaced by step 2's full-screen one,
    // so the model never got a picture of the window: the next view sends it.
    let mut e = nav_engine(false);
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            {"tool": "get_app_state", "arguments": {}},
            {"tool": "screenshot", "arguments": {"mode": "full"}}
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some());
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "{}", out.text);

    // A batch whose last image is the window's: it counts.
    let mut e = nav_engine(false);
    e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            {"tool": "get_app_state", "arguments": {}}
        ]}),
    );
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_none(), "{}", out.text);
}

// -- overlay hooks (a stand-in helper records what it is told) ---------

#[cfg(unix)]
fn recording_helper(log: &std::path::Path) -> Launcher {
    Launcher {
        program: "sh".into(),
        args: vec!["-c".into(), format!("cat > '{}'", log.display())],
    }
}

#[cfg(unix)]
#[test]
fn a_stop_key_that_does_not_work_is_reported_once() {
    let dir = std::env::temp_dir().join(format!("cu-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("stop.log");
    // A helper that says the system refused the key.
    let helper = Launcher {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                r#"echo '{{"t":"hotkey","key":"ctrl+alt+escape","ok":false}}'; cat > '{}'"#,
                log.display()
            ),
        ],
    };
    let mut e = engine().with_overlay(helper);
    e.arm();
    assert!(e.check_stop_key(Duration::from_secs(5)).is_err());
    let first = e.call_tool("list_apps", serde_json::json!({}));
    assert!(
        first.text.contains("emergency stop key") && first.text.contains("not working"),
        "{}",
        first.text
    );
    let second = e.call_tool("list_apps", serde_json::json!({}));
    assert!(!second.text.contains("emergency stop key"), "told once");
    drop(e);
    std::fs::remove_dir_all(&dir).ok();
}

/// Everything the recording helper got, once the engine has let it go
/// (its last message is `quit`; dropping the engine doesn't wait).
#[cfg(unix)]
fn read_log(path: &std::path::Path) -> String {
    for _ in 0..250 {
        if let Ok(t) = std::fs::read_to_string(path)
            && t.contains("\"t\":\"quit\"")
        {
            return t;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A stand-in helper that records what it is told and says the cursor
/// arrived, `delay` seconds after each pointer.
#[cfg(unix)]
fn answering_helper(log: &std::path::Path, delay: &str) -> Launcher {
    Launcher {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                r#"while IFS= read -r l; do printf '%s\n' "$l" >> '{}'; case "$l" in *'"t":"pointer"'*'"id":'*) id=${{l##*\"id\":}}; id=${{id%%[!0-9]*}}; sleep {delay}; printf '{{"t":"arrived","id":%s}}\n' "$id";; esac; done"#,
                log.display()
            ),
        ],
    }
}

#[test]
fn the_pen_is_found_by_how_far_it_moved() {
    let strokes = vec![
        vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)],
        // A jump to the next stroke isn't a move.
        vec![Point::new(500.0, 500.0), Point::new(500.0, 600.0)],
    ];
    let along = PathPosition::new(&strokes);
    assert_eq!(along.at(0.0), Some(Point::new(0.0, 0.0)));
    assert_eq!(along.at(60.0), Some(Point::new(100.0, 0.0)));
    assert_eq!(along.at(100.0), Some(Point::new(100.0, 0.0)));
    assert_eq!(along.at(150.0), Some(Point::new(500.0, 600.0)));
    assert_eq!(along.at(1e9), Some(Point::new(500.0, 600.0)));
    assert_eq!(PathPosition::new(&[]).at(5.0), None);
}

#[cfg(unix)]
#[test]
fn the_cursor_gets_there_before_the_action() {
    let dir = std::env::temp_dir().join(format!("cu-arrive-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("arrive.log");
    let mut e = engine().with_overlay(answering_helper(&log, "0.3"));
    state_of(&mut e, serde_json::json!({}));
    let bold = index_named(&e, 4242, "Bold");
    let start = Instant::now();
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(!out.is_error, "{}", out.text);
    // The click waited for the cursor (the helper answers after 0.3 s).
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "{:?}",
        start.elapsed()
    );
    drop(e);
    let t = read_log(&log);
    // One glide for the click, not two.
    let pointers = t.lines().filter(|l| l.contains(r#""t":"pointer""#)).count();
    assert_eq!(pointers, 1, "{t}");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn the_cursor_goes_where_the_work_is() {
    let dir = std::env::temp_dir().join(format!("cu-where-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("where.log");
    let mut e = engine().with_overlay(answering_helper(&log, "0"));
    e.backend_mut()
        .app_mut(4242)
        .unwrap()
        .elements
        .iter_mut()
        .find(|el| el.handle == 5)
        .unwrap()
        .states
        .focused = true;
    state_of(&mut e, serde_json::json!({}));
    let bold = index_named(&e, 4242, "Bold");
    // Bold's box isn't known (as in trees that give none): its
    // toolbar's centre.
    {
        let st = e.states.get_mut(&4242).unwrap();
        let i = st.nodes.iter().position(|n| n.index == bold).unwrap();
        let h = st.nodes[i].handle;
        st.nodes[i].bounds = None;
        st.bounds.remove(&h);
    }
    e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    // Typing with no element named: the focused one, the document.
    e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "text": "x"}),
    );
    drop(e);
    let t = read_log(&log);
    assert!(
        t.contains(r#""t":"pointer","x":400.0,"y":20.0,"click":true"#),
        "{t}"
    );
    assert!(
        t.contains(r#""t":"pointer","x":400.0,"y":320.0,"click":false"#),
        "{t}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn overlay_follows_the_work() {
    let dir = std::env::temp_dir().join(format!("cu-ov-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("follow.log");
    let mut e = nav_engine(false).with_overlay(recording_helper(&log));
    state_of(&mut e, serde_json::json!({}));
    press_named(&mut e, 7, "Next");
    drop(e); // closes the helper's stdin
    let t = read_log(&log);
    for want in [
        r#""t":"config""#,
        r#""t":"begin""#,
        r#""t":"target","rect":[0.0,0.0,800.0,600.0]"#,
        r#""t":"pointer""#,
        r#""click":true"#,
        r#""t":"end","ok":true"#,
    ] {
        assert!(t.contains(want), "missing {want} in:\n{t}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn no_overlay_without_a_helper() {
    // Engines only show an overlay when the host provides the helper.
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    assert!(e.overlay.is_none());
}

// -- the user's controls ------------------------------------------------

/// An engine on a fake clock that sleeping advances.
fn timed_engine(
    backend: MockBackend,
    cfg: Config,
) -> (Engine<MockBackend>, Arc<std::sync::Mutex<Instant>>) {
    let now = Arc::new(std::sync::Mutex::new(Instant::now()));
    let (c, s) = (now.clone(), now.clone());
    let e = Engine::new(backend, ConfigStore::in_memory(cfg))
        .with_time(move || *c.lock().unwrap(), move |d| *s.lock().unwrap() += d);
    (e, now)
}

#[test]
fn stop_refuses_every_call_until_the_user_lets_it_continue() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    let stop = e.stop_handle();
    stop.store(true, Ordering::SeqCst);
    for (tool, args) in [
        ("list_apps", serde_json::json!({})),
        ("get_app_state", serde_json::json!({"app": "TextEdit"})),
        (
            "click",
            serde_json::json!({"app": "TextEdit", "element_index": 1}),
        ),
    ] {
        let out = e.call_tool(tool, args);
        assert!(out.is_error, "{tool}");
        // "Ctrl+Alt+Esc" (Ctrl+Option+Esc on a Mac).
        let key = crate::overlay::helper::pretty_key("ctrl+alt+escape");
        assert!(out.text.contains(&key), "{}", out.text);
    }
    // A batch stops at once, even with continue_on_error.
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "continue_on_error": true,
            "steps": [{"tool": "list_apps"}, {"tool": "list_apps"}]}),
    );
    assert!(out.is_error);
    e.set_stopped(false);
    assert!(!e.is_stopped());
    assert!(!e.call_tool("list_apps", serde_json::json!({})).is_error);
}

#[test]
fn actions_wait_while_the_user_uses_the_computer() {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    // The user is typing (input 0.1 s and 0.4 s ago), then stops.
    backend.idle_script = [100, 400].map(Duration::from_millis).into();
    backend.idle = Some(Duration::from_secs(5));
    let cfg = Config::default();
    let (mut e, clock) = timed_engine(backend, cfg);
    state_of(&mut e, serde_json::json!({}));
    let t0 = *clock.lock().unwrap();
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "Return"}),
    );
    assert!(!out.is_error, "{}", out.text);
    let waited = clock.lock().unwrap().saturating_duration_since(t0);
    assert!(waited >= Duration::from_millis(100), "{waited:?}");
    assert!(
        e.backend()
            .events
            .contains(&Event::Key(4242, "Return".into()))
    );

    // Reading is never held up.
    e.backend_mut().idle = Some(Duration::ZERO);
    assert!(!e.call_tool("list_apps", serde_json::json!({})).is_error);
}

#[test]
fn a_busy_user_makes_the_action_give_up_and_say_why() {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    backend.idle = Some(Duration::from_millis(100));
    let mut cfg = Config::default();
    cfg.control.max_pause_secs = 3;
    let (mut e, _) = timed_engine(backend, cfg);
    state_of(&mut e, serde_json::json!({}));
    let before = e.backend().events.len();
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "Return"}),
    );
    assert!(
        out.is_error && out.text.contains("using the mouse or keyboard"),
        "{}",
        out.text
    );
    assert_eq!(e.backend().events.len(), before, "nothing was done");

    // Switched off: no waiting at all.
    let mut cfg = e.store().config.clone();
    cfg.control.pause_on_user_input = false;
    e.set_config(ConfigStore::in_memory(cfg));
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "Return"}),
    );
    assert!(!out.is_error, "{}", out.text);
}

#[test]
fn own_input_is_not_taken_for_the_user() {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut cfg = Config::default();
    // Only the pause for the user is measured here, not settling.
    cfg.timing.settle = crate::config::SettleMode::Fixed;
    let (mut e, clock) = timed_engine(backend, cfg);
    state_of(&mut e, serde_json::json!({}));
    let press = |e: &mut Engine<MockBackend>| {
        e.call_tool(
            "press_key",
            serde_json::json!({"app": "TextEdit", "key": "Tab"}),
        )
    };
    assert!(!press(&mut e).is_error);
    // 1 s later the system saw input 1 s ago: the engine's own.
    *clock.lock().unwrap() += Duration::from_secs(1);
    e.backend_mut().idle = Some(Duration::from_secs(1));
    let t = *clock.lock().unwrap();
    assert!(!press(&mut e).is_error);
    let took = clock.lock().unwrap().saturating_duration_since(t);
    // Only the action's own settle/key delays, no pause.
    assert!(
        took < Duration::from_millis(200),
        "waited {took:?} for its own input"
    );
}

fn login_app(pid: u32) -> MockApp {
    let mut app = MockBackend::text_editor(pid);
    let mut pw = MockElement::new(
        20,
        "secure text field",
        "Password",
        Rect::new(100.0, 100.0, 200.0, 30.0),
    )
    .child_of(1)
    .editable();
    pw.value = Some("hunter2".into());
    let mut card = MockElement::new(
        21,
        "text field",
        "Payment",
        Rect::new(100.0, 200.0, 200.0, 30.0),
    )
    .child_of(1)
    .editable();
    card.value = Some("4111 1111 1111 1111".into());
    app.elements.extend([pw, card]);
    app
}

#[test]
fn private_data_never_reaches_the_model() {
    let mut backend = MockBackend::new();
    backend.add_app(login_app(4242));
    let cfg = Config::default();
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
    assert!(!out.text.contains("hunter2"), "{}", out.text);
    assert!(!out.text.contains("4111 1111 1111 1111"), "{}", out.text);
    assert!(out.text.contains("•••• •••• •••• 1111"), "{}", out.text);
    assert!(
        out.text.contains("2 private area(s) blacked out"),
        "{}",
        out.text
    );
    // The password field's pixels are grey in the image.
    let img = out.image.unwrap();
    let rgb = image::load_from_memory(&img.data).unwrap().to_rgb8();
    let at = |x: u32, y: u32| rgb.get_pixel(x, y).0;
    assert_eq!(at(150, 110), [128, 128, 128]);
    assert_eq!(at(700, 500), [200, 200, 200]);

    // The screenshot tool too.
    let out = e.call_tool("screenshot", serde_json::json!({"app": "TextEdit"}));
    assert!(out.text.contains("blacked out"), "{}", out.text);

    // Settings can switch it off.
    let mut cfg = e.store().config.clone();
    cfg.privacy.redact_passwords = false;
    cfg.privacy.redact_card_numbers = false;
    cfg.privacy.redact_labels.clear();
    e.set_config(ConfigStore::in_memory(cfg));
    let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
    assert!(out.text.contains("4111 1111 1111 1111"), "{}", out.text);
}

#[cfg(unix)]
#[test]
fn the_stop_key_in_the_helper_stops_the_engine() {
    let dir = std::env::temp_dir().join(format!("cu-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("stop.log");
    // A stand-in helper whose user presses the stop key at once.
    let script = format!(
        r#"echo '{{"t":"ready","excluded":true,"available":true}}'; echo '{{"t":"stop","on":true}}'; cat > '{}'"#,
        log.display()
    );
    let mut e = engine().with_overlay(Launcher {
        program: "sh".into(),
        args: vec!["-c".into(), script],
    });
    e.arm();
    for _ in 0..100 {
        if e.is_stopped() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(e.is_stopped());
    let out = e.call_tool("list_apps", serde_json::json!({}));
    assert!(
        out.is_error && out.text.contains("stop key"),
        "{}",
        out.text
    );
    drop(e);
    std::fs::remove_dir_all(&dir).ok();
}

// -- smart waiting and verification -------------------------------------

fn index_of_name(out: &str, needle: &str) -> u32 {
    index_of(out, needle).unwrap_or_else(|| panic!("{needle} not in:\n{out}"))
}

#[test]
fn waits_until_the_ui_stops_changing() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    // The document keeps updating for three more reads, then settles.
    let before = e.backend().snapshots;
    e.backend_mut().snapshot_script = ["Loading.", "Loading..", "Done"]
        .map(|v| (5, v.to_string()))
        .into();
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(!out.is_error, "{}", out.text);
    // Read until two reads agreed (after "Done"), and the report shows it.
    assert!(
        e.backend().snapshots >= before + 4,
        "{}",
        e.backend().snapshots
    );
    assert!(out.text.contains("Done"), "{}", out.text);

    // Fixed mode: one read (for the report), no waiting for stability.
    let mut cfg = e.store().config.clone();
    cfg.timing.settle = crate::config::SettleMode::Fixed;
    e.set_config(ConfigStore::in_memory(cfg));
    state_of(&mut e, serde_json::json!({}));
    let before = e.backend().snapshots;
    e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert_eq!(e.backend().snapshots, before + 1);
}

#[test]
fn a_press_that_fails_is_clicked_with_the_mouse() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    e.backend_mut().fail_actions.insert(3);
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("clicked it with the mouse instead"),
        "{}",
        out.text
    );
    assert!(e.backend().events.contains(&Event::Click(
        4242,
        Point::new(40.0, 20.0),
        MouseButton::Left,
        1
    )));

    // With retries off, the failure is reported instead.
    let mut cfg = e.store().config.clone();
    cfg.verify.retry = false;
    e.set_config(ConfigStore::in_memory(cfg));
    let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(out.is_error, "{}", out.text);
}

#[test]
fn scripts_cannot_start_scripts_through_batch() {
    let dir = std::env::temp_dir().join(format!("cu-nest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = engine();
    let mut cfg = e.store().config.clone();
    cfg.script.dir = Some(dir.clone());
    e.set_config(ConfigStore::in_memory(cfg));
    let code = r#"tool("batch", #{steps: [#{tool: "script", arguments: #{run: "rec"}}]})"#;
    let saved = e.call_tool(
        "script",
        serde_json::json!({"save": "rec", "code": code, "description": "recurse"}),
    );
    assert!(!saved.is_error, "{}", saved.text);
    let out = e.call_tool("script", serde_json::json!({"run": "rec"}));
    assert!(out.is_error, "{}", out.text);
    assert!(
        out.text.contains("can't start another script"),
        "{}",
        out.text
    );
    // And the engine is fine afterwards.
    assert!(!e.call_tool("list_apps", serde_json::json!({})).is_error);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn long_text_is_typed_in_pieces_and_too_much_is_refused() {
    let mut e = engine();
    let text: String = "ab€d".repeat(150);
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "text": text}),
    );
    assert!(!out.is_error, "{}", out.text);
    let pieces: Vec<&String> = e
        .backend()
        .events
        .iter()
        .filter_map(|ev| match ev {
            Event::Type(_, t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(pieces.len(), 3, "{pieces:?}");
    assert!(pieces.iter().all(|p| p.chars().count() <= TYPE_CHUNK));
    assert_eq!(pieces.iter().map(|p| p.as_str()).collect::<String>(), text);

    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "text": "x".repeat(MAX_TYPED_CHARS + 1)}),
    );
    assert!(
        out.is_error && out.text.contains("set_clipboard"),
        "{}",
        out.text
    );
    let keys = vec!["a"; MAX_KEY_PRESSES + 1].join(" ");
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": keys}),
    );
    assert!(out.is_error && out.text.contains("at most"), "{}", out.text);
}

#[test]
fn a_cancelled_call_ends_and_the_next_one_runs() {
    let mut e = engine();
    e.cancel_handle().store(true, Ordering::SeqCst);
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "text": "hello"}),
    );
    assert!(
        out.is_error && out.text.contains("cancelled"),
        "{}",
        out.text
    );
    assert!(
        !e.backend()
            .events
            .iter()
            .any(|ev| matches!(ev, Event::Type(..))),
    );
    // The cancel was for that call only.
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "text": "hello"}),
    );
    assert!(!out.is_error, "{}", out.text);
}

#[test]
fn a_secondary_action_that_fails_is_an_error() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let style = index_of_name(&out.text, "\"Style\"");
    e.backend_mut().fail_actions.insert(4);
    let out = e.call_tool(
        "perform_secondary_action",
        serde_json::json!({"app": "TextEdit", "element_index": style, "action": "show_menu"}),
    );
    assert!(out.is_error, "{}", out.text);
}

#[test]
fn an_unanswered_press_is_never_clicked_again() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    e.backend_mut().unanswered_actions.insert(3);
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("may or may not have happened"),
        "{}",
        out.text
    );
    let events = &e.backend().events;
    assert!(events.contains(&Event::Action(3, "AXPress".into())));
    assert!(
        !events.iter().any(|ev| matches!(ev, Event::Click(..))),
        "{events:?}"
    );
}

#[test]
fn a_press_that_changes_nothing_is_reported_not_repeated() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(
        out.text.contains("Nothing on screen changed"),
        "{}",
        out.text
    );
    let presses = |e: &Engine<MockBackend>| {
        e.backend()
            .events
            .iter()
            .filter(|ev| matches!(ev, Event::Action(3, _) | Event::Click(..)))
            .count()
    };
    assert_eq!(presses(&e), 1, "not repeated by default");

    // Opted in: tried once more with the mouse.
    let mut cfg = e.store().config.clone();
    cfg.verify.retry_on_no_change = true;
    e.set_config(ConfigStore::in_memory(cfg));
    let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(
        out.text.contains("clicked it with the mouse too"),
        "{}",
        out.text
    );
    assert_eq!(presses(&e), 3);
}

#[test]
fn a_value_that_does_not_take_is_typed_instead() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let doc = index_of_name(&out.text, "\"Document\"");
    e.backend_mut().ignore_set_value.insert(5);
    let out = e.call_tool(
        "set_value",
        serde_json::json!({"app": "TextEdit", "element_index": doc, "value": "Replaced"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("typed it instead"), "{}", out.text);
    let out = state_of(&mut e, serde_json::json!({"disable_diff": true}));
    assert!(out.text.contains("Replaced"), "{}", out.text);
    assert!(!out.text.contains("HelloReplaced"), "{}", out.text);
}

#[test]
fn typing_that_does_not_show_is_never_typed_twice() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let doc = index_of_name(&out.text, "\"Document\"");
    // Focus claims success but does nothing: the field doesn't change.
    e.backend_mut().fake_focus.insert(5);
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": "TextEdit", "element_index": doc, "text": "y"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("could enter the text twice"),
        "{}",
        out.text
    );
    let typed = e
        .backend()
        .events
        .iter()
        .filter(|ev| matches!(ev, Event::Type(..)))
        .count();
    assert_eq!(typed, 1, "typed once, never again on its own");
}

#[test]
fn an_app_that_shows_changes_at_once_is_waited_on_less() {
    let mut p = Promptness::default();
    assert_eq!(
        p.grace(),
        Duration::from_millis(500),
        "unknown: the long wait"
    );
    for _ in 0..Promptness::PROMPT_AFTER {
        p.saw_change(true);
    }
    assert_eq!(p.grace(), Duration::from_millis(200));
    // One change shown late, and it is never trusted again.
    p.saw_change(false);
    for _ in 0..10 {
        p.saw_change(true);
    }
    assert_eq!(p.grace(), Duration::from_millis(500));
}

#[test]
fn a_change_reported_late_is_not_taken_for_no_change() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    // The app shows the change only after a few reads.
    e.backend_mut().snapshot_script.extend([
        (5, "Hello".to_string()),
        (5, "Hello".to_string()),
        (5, "Hello".to_string()),
        (5, "Changed".to_string()),
    ]);
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        !out.text.contains("Nothing on screen changed"),
        "{}",
        out.text
    );
}

// -- smart screenshots ----------------------------------------------------

fn always_shot_engine() -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut cfg = Config::default();
    cfg.screenshot.attach = AttachMode::Always;
    Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
}

#[test]
fn auto_screenshots_of_well_described_windows_can_be_overviews() {
    // Off by default: full detail.
    let mut e = engine();
    let img = state_of(&mut e, serde_json::json!({})).image.unwrap();
    assert_eq!((img.width, img.height), (800, 600));
    // Opted in.
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut cfg = Config::default();
    cfg.screenshot.overview_max_dimension = 768;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    // Attached on its own: an overview at overview_max_dimension.
    let out = state_of(&mut e, serde_json::json!({}));
    let img = out.image.expect("first view");
    assert_eq!(img.width.max(img.height), 768, "{}", out.text);
    assert!(out.text.contains("An overview"), "{}", out.text);
    // Coordinates in the overview still map to the screen.
    e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "x": 384, "y": 288}),
    );
    assert!(matches!(
        e.backend().events.last().unwrap(),
        Event::Click(4242, p, _, _) if (p.x - 400.0).abs() < 1.0 && (p.y - 300.0).abs() < 1.0
    ));
    // Asked for: full detail.
    let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
    let img = out.image.expect("asked for");
    assert_eq!((img.width, img.height), (800, 600));
}

/// TextEdit with a 300-item list, and a small token budget.
fn long_list_engine(cfg: impl FnOnce(&mut Config)) -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    let mut app = MockBackend::text_editor(4242);
    for i in 0..300u64 {
        app.elements.push(
            crate::mock::MockElement::new(
                1000 + i,
                "list item",
                &format!("Item {i}"),
                Rect::new(0.0, 40.0 + i as f64, 100.0, 1.0),
            )
            .child_of(1),
        );
    }
    backend.add_app(app);
    let mut c = Config::default();
    c.tree.max_tokens = 400;
    cfg(&mut c);
    Engine::new(backend, ConfigStore::in_memory(c)).with_time(Instant::now, |_| {})
}

#[test]
fn summarizing_can_be_turned_down_or_off() {
    // Over the budget: the list is folded.
    let mut e = long_list_engine(|_| {});
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("folded"), "{}", out.text);
    // The model can ask for one whole tree.
    let out = state_of(
        &mut e,
        serde_json::json!({"disable_diff": true, "max_tokens": 0}),
    );
    assert!(!out.text.contains("folded"), "{}", out.text);
    assert!(out.text.contains("Item 150"), "{}", out.text);
    // The user can turn it off.
    let mut e = long_list_engine(|c| c.tree.summarize = crate::config::Summarize::Off);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(!out.text.contains("folded") && out.text.contains("Item 150"));
    // Or keep more of each list.
    let mut e = long_list_engine(|c| c.tree.fold_keep = 40);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("Item 39") && !out.text.contains("Item 40\""));
}

#[test]
fn compact_looks_say_each_thing_once() {
    let mut e = engine();
    let first = state_of(&mut e, serde_json::json!({}));
    assert!(
        first
            .text
            .starts_with("App: TextEdit (com.apple.TextEdit, pid 4242) · window"),
        "{}",
        first.text
    );
    // A value the field now shows isn't echoed back.
    let doc = index_of_name(&first.text, "\"Document\"");
    let out = e.call_tool(
        "set_value",
        serde_json::json!({"app": "TextEdit", "element_index": doc, "value": "Hi there"}),
    );
    assert!(
        out.text
            .starts_with("Set text area \"Document\"; it shows the new value."),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("value=\"Hi there\""),
        "the report shows it: {}",
        out.text
    );
    // Nothing new since: one line, the app and window named only.
    let quiet = state_of(&mut e, serde_json::json!({}));
    assert_eq!(
        quiet.text,
        "App: TextEdit · window \"Untitled\" · screen #1: nothing changed since your last look (tree and screenshot)."
    );
    // Part of the tree, without moving the baseline.
    let toolbar = e
        .state(4242)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.role == "toolbar")
        .unwrap()
        .index;
    let part = state_of(&mut e, serde_json::json!({"within": toolbar}));
    assert!(
        part.text.contains(&format!(
            "element {toolbar} and what is in it (3 element(s))"
        )),
        "{}",
        part.text
    );
    assert!(part.text.contains("Bold") && !part.text.contains("Document"));
    let again = state_of(&mut e, serde_json::json!({}));
    assert!(again.text.contains("nothing changed"), "{}", again.text);
    let bad = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": "TextEdit", "within": 999}),
    );
    assert!(bad.is_error && bad.text.contains("unknown element_index 999"));
}

#[test]
fn find_element_pages_through_many_matches() {
    let mut e = long_list_engine(|_| {});
    let out = e.call_tool(
        "find_element",
        serde_json::json!({"app": "TextEdit", "role": "list item", "max_results": 5, "offset": 5}),
    );
    assert!(out.text.starts_with("Matches 6–10 of 300"), "{}", out.text);
    assert!(out.text.contains("offset=10"), "{}", out.text);
    assert!(out.text.contains("Item 5\"") && out.text.contains("Item 9\""));
    let out = e.call_tool(
        "find_element",
        serde_json::json!({"app": "TextEdit", "role": "list item", "offset": 400}),
    );
    assert!(out.text.contains("none after offset 400"), "{}", out.text);
}

#[test]
fn reports_can_be_brief_or_about_what_was_acted_on() {
    // Brief: how many, and the new screen.
    let mut e = nav_engine(true);
    let mut cfg = e.store().config.clone();
    cfg.tree.report = crate::config::Report::Brief;
    e.set_config(ConfigStore::in_memory(cfg));
    state_of(&mut e, serde_json::json!({}));
    let out = press_named(&mut e, 7, "Next");
    assert!(out.text.contains("now on screen #2 (new)"), "{}", out.text);
    assert!(
        out.text.contains("element(s); get_app_state shows them"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("Result 3"), "{}", out.text);
    // The next look shows all of it.
    let look = state_of(&mut e, serde_json::json!({}));
    assert!(expand(&look.text).contains("Result 3"), "{}", look.text);

    // Relevant: a toolbar of five buttons, a status line elsewhere.
    let mut backend = MockBackend::new();
    let mut app = MockBackend::text_editor(9);
    for (k, name) in ["Italic", "Underline", "Strike"].iter().enumerate() {
        app.elements.push(
            MockElement::new(
                40 + k as u64,
                "button",
                name,
                Rect::new(200.0 + 70.0 * k as f64, 8.0, 60.0, 24.0),
            )
            .child_of(2)
            .with_actions(&["AXPress"]),
        );
    }
    app.elements.push(
        MockElement::new(
            50,
            "text",
            "Status: ready",
            Rect::new(0.0, 580.0, 300.0, 20.0),
        )
        .child_of(1),
    );
    let mut after = app.clone();
    for el in after.elements.iter_mut() {
        if el.handle == 50 {
            el.name = Some("Status: bold on".into());
        }
        if el.handle == 41 {
            el.name = Some("Underline (on)".into());
        }
    }
    backend.add_app(app);
    backend.on_press.insert(3, after);
    let mut cfg = Config::default();
    cfg.tree.report = crate::config::Report::Relevant;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    let first = e.call_tool("get_app_state", serde_json::json!({"app": "TextEdit"}));
    let bold = index_of(&first.text, "\"Bold\"").unwrap();
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold}),
    );
    assert!(
        out.text.contains("Underline (on)"),
        "in the toolbar: {}",
        out.text
    );
    assert!(!out.text.contains("bold on"), "elsewhere: {}", out.text);
    assert!(
        out.text.contains("[1 other change(s) elsewhere"),
        "{}",
        out.text
    );
}

#[test]
fn what_changes_on_its_own_is_summed_up() {
    let mut backend = MockBackend::new();
    let mut app = MockBackend::text_editor(4242);
    app.elements
        .push(MockElement::new(60, "text", "", Rect::new(700.0, 8.0, 80.0, 20.0)).child_of(2));
    backend.add_app(app);
    for t in 0..8 {
        backend
            .snapshot_script
            .push_back((60, format!("12:00:0{t}")));
    }
    let mut cfg = Config::default();
    cfg.tree.quiet_volatile = true;
    cfg.cache.snapshot_ttl_ms = 0;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    let mut last = String::new();
    for _ in 0..4 {
        last = state_of(&mut e, serde_json::json!({})).text;
    }
    assert!(
        last.contains("1 element(s) that keep changing on their own"),
        "{last}"
    );
    assert!(!last.contains("value=\"12:00"), "{last}");
}

#[test]
fn pictures_follow_how_the_model_works() {
    // Adaptive: after a few looks without pixels, a new screen of a
    // well-described window comes without a picture.
    let mut e = nav_engine(true);
    let mut cfg = e.store().config.clone();
    cfg.screenshot.adaptive = true;
    cfg.cache.snapshot_ttl_ms = 0;
    e.set_config(ConfigStore::in_memory(cfg));
    let first = state_of(&mut e, serde_json::json!({}));
    assert!(first.image.is_some(), "the first look has one");
    for _ in 0..3 {
        state_of(&mut e, serde_json::json!({}));
    }
    // The document changes: a reason to look at the pixels, but not
    // here.
    e.backend_mut()
        .snapshot_script
        .push_back((5, "Changed".into()));
    e.backend_mut().fill = 90;
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_none(), "{}", out.text);
    assert!(out.text.contains("haven't needed pictures"), "{}", out.text);
    // Once the model uses pixels there, pictures come again.
    let r = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "x": 5, "y": 5}),
    );
    assert!(!r.is_error, "{}", r.text);
    e.backend_mut()
        .snapshot_script
        .push_back((5, "Changed again".into()));
    e.backend_mut().fill = 120;
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "{}", out.text);

    // Icon strip: unnamed buttons, shown once, numbered.
    let mut backend = MockBackend::new();
    let mut app = MockBackend::text_editor(4242);
    for k in 0..3u64 {
        app.elements.push(
            MockElement::new(
                70 + k,
                "button",
                "",
                Rect::new(300.0 + 40.0 * k as f64, 8.0, 24.0, 24.0),
            )
            .child_of(2)
            .with_actions(&["AXPress"]),
        );
    }
    backend.add_app(app);
    let mut cfg = Config::default();
    cfg.screenshot.icon_sprite = true;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    let out = state_of(&mut e, serde_json::json!({"screenshot": false}));
    let strip = out.image.as_ref().expect("the strip");
    assert!(
        strip.height < 80 && strip.width < 300,
        "{}x{}",
        strip.width,
        strip.height
    );
    assert!(
        out.text.contains("Icons of buttons without a name"),
        "{}",
        out.text
    );
    let again = state_of(&mut e, serde_json::json!({"screenshot": false}));
    assert!(again.image.is_none(), "shown once: {}", again.text);

    // locate without its picture.
    let mut e = always_shot_engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "locate",
        serde_json::json!({"app": "TextEdit", "near": [400, 300], "feature": "center", "picture": false}),
    );
    assert!(out.image.is_none(), "{}", out.text);
}

#[test]
fn the_indicator_label_is_never_the_apps_text() {
    assert!(reads_as_start_of("zero is thir", "zero is thinking…"));
    assert!(reads_as_start_of("zero", "zero is done"));
    assert!(!reads_as_start_of("zero balance", "zero is done"));
    assert!(!reads_as_start_of("save", "zero is done"));
}

#[test]
fn the_tool_manager_shows_the_base_and_finds_the_rest() {
    use crate::config::ToolManager;
    let mut e = engine();
    // The default.
    assert_eq!(e.store().config.tools.manager, ToolManager::Dispatch);
    let mut cfg = e.store().config.clone();
    cfg.tools.manager = ToolManager::Off;
    e.set_config(ConfigStore::in_memory(cfg.clone()));
    let all = e.tool_definitions().len();
    cfg.tools.manager = ToolManager::Dispatch;
    e.set_config(ConfigStore::in_memory(cfg.clone()));
    let names = |e: &mut Engine<MockBackend>| -> Vec<String> {
        e.tool_definitions()
            .iter()
            .map(|d| d.name.to_string())
            .collect()
    };
    let shown = names(&mut e);
    assert!(shown.contains(&"find_tools".to_string()) && shown.contains(&"use_tool".to_string()));
    assert!(
        !shown.contains(&"design".to_string()) && shown.len() < all,
        "{shown:?}"
    );
    // Found by category, run through use_tool; the list never changes.
    let out = e.call_tool("find_tools", serde_json::json!({"category": "windows"}));
    assert!(
        out.text.starts_with("window: ") && out.text.contains("use_tool"),
        "{}",
        out.text
    );
    assert_eq!(names(&mut e), shown);
    let out = e.call_tool(
        "use_tool",
        serde_json::json!({"name": "window", "arguments": {"app": "TextEdit", "action": "list"}}),
    );
    assert!(!out.is_error, "{}", out.text);
    // Found by what it does.
    let out = e.call_tool("find_tools", serde_json::json!({"query": "clipboard"}));
    assert!(out.text.contains("get_clipboard"), "{}", out.text);
    // Hidden is not forbidden: a direct call still runs.
    assert!(!e.call_tool("get_clipboard", serde_json::json!({})).is_error);
    // use_tool can't run the manager itself, or an unknown tool.
    assert!(
        e.call_tool("use_tool", serde_json::json!({"name": "find_tools"}))
            .is_error
    );
    assert!(
        e.call_tool("use_tool", serde_json::json!({"name": "nope"}))
            .is_error
    );

    // Words that name nothing find nothing; a query brings the best few.
    let out = e.call_tool("find_tools", serde_json::json!({"query": "the a to"}));
    assert!(out.text.starts_with("No tools match"), "{}", out.text);
    let out = e.call_tool("find_tools", serde_json::json!({"query": "draw a star"}));
    assert!(out.text.starts_with("draw: "), "{}", out.text);
    assert!(
        out.text.matches("arguments: {").count() <= 3,
        "{}",
        out.text
    );
    // Arguments once: asked again, a line; again=true repeats them.
    let out = e.call_tool("find_tools", serde_json::json!({"name": "window"}));
    assert!(
        out.text.contains("arguments: as shown before"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "find_tools",
        serde_json::json!({"name": "window", "again": true}),
    );
    assert!(out.text.contains("arguments: {"), "{}", out.text);
    // A tool never shown, called wrong: its arguments come with the
    // error, once.
    let wrong = serde_json::json!({"name": "locate", "arguments": {"nope": 1}});
    let out = e.call_tool("use_tool", wrong.clone());
    assert!(
        out.is_error && out.text.contains("locate takes: {"),
        "{}",
        out.text
    );
    let out = e.call_tool("use_tool", wrong);
    assert!(!out.text.contains("locate takes"), "{}", out.text);

    // list_changed: what is found joins the list, and stays.
    cfg.tools.manager = ToolManager::ListChanged;
    e.set_config(ConfigStore::in_memory(cfg));
    assert!(!names(&mut e).contains(&"use_tool".to_string()));
    // A base tool asked for by name changes nothing.
    let before = e.tools_signature();
    let out = e.call_tool("find_tools", serde_json::json!({"name": "click"}));
    assert!(out.text.contains("Already in your tools"), "{}", out.text);
    assert_eq!(e.tools_signature(), before);
    let out = e.call_tool("find_tools", serde_json::json!({"category": "design"}));
    assert!(out.text.contains("Added to your tools"), "{}", out.text);
    let now = names(&mut e);
    assert!(now.contains(&"design".to_string()) && now.contains(&"locate".to_string()));
}

#[test]
fn a_design_change_sends_only_the_part_that_changed() {
    let mut e = engine();
    let first = e.call_tool(
        "design",
        serde_json::json!({"name": "b", "size": [800, 600], "add": [
            {"id": "disc", "ellipse": [400, 300, 200, 200], "fill": "#1D3557"},
            {"id": "star", "star": [700, 500, 40, 18, 5], "fill": "#FFB703"}
        ]}),
    );
    let full = first.image.as_ref().map(|i| (i.width, i.height)).unwrap();
    // A small change: only that part, and where it is.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "b", "change": [{"id": "star", "fill": "#E63946"}]}),
    );
    let part = out.image.as_ref().map(|i| (i.width, i.height)).unwrap();
    assert!(
        part.0 < full.0 / 3 && part.1 < full.1 / 3,
        "{part:?} of {full:?}"
    );
    assert!(
        out.text.contains("The picture changed only at x 6")
            && out.text.contains("that part is shown"),
        "{}",
        out.text
    );
    // Said in full once, then short.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "b", "change": [{"id": "star", "fill": "#2A9D8F"}]}),
    );
    assert!(
        out.text.contains("Only the part that changed: x 6"),
        "{}",
        out.text
    );
    // A zoom into a cell, then an export: no picture again.
    e.call_tool(
        "design",
        serde_json::json!({"name": "b", "show": {"cell": "B2"}}),
    );
    let out = e.call_tool("design", serde_json::json!({"name": "b", "export": "svg"}));
    assert!(out.image.is_none(), "{}", out.text);
    // A big change: all of it.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "b", "background": "#000000"}),
    );
    let all = out.image.as_ref().map(|i| (i.width, i.height)).unwrap();
    assert_eq!(all, full);
    // screenshot.scope = "full": always all of it.
    let mut cfg = e.store().config.clone();
    cfg.screenshot.scope = crate::config::ShotScope::Full;
    e.set_config(ConfigStore::in_memory(cfg));
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "b", "change": [{"id": "star", "fill": "#FFFFFF"}]}),
    );
    assert_eq!(out.image.as_ref().map(|i| (i.width, i.height)), Some(full));
}

#[test]
fn the_last_app_named_stands_in_for_a_missing_one() {
    let mut e = engine();
    let mut cfg = e.store().config.clone();
    cfg.tools.default_app = true;
    e.set_config(ConfigStore::in_memory(cfg));
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool("find_element", serde_json::json!({"name": "Bold"}));
    assert!(
        !out.is_error && out.text.contains("\"Bold\""),
        "{}",
        out.text
    );
    // Off: an error, as before.
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    assert!(
        e.call_tool("find_element", serde_json::json!({"name": "Bold"}))
            .is_error
    );
}

#[test]
fn explanations_can_always_be_given_in_full() {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut cfg = Config::default();
    cfg.tree.brief_repeats = false;
    // (A compact look that changed nothing is one line.)
    cfg.tree.compact = false;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    for _ in 0..2 {
        let out = state_of(&mut e, serde_json::json!({"screenshot": false}));
        assert!(
            out.text
                .contains("not attached (pass screenshot=true for one)"),
            "{}",
            out.text
        );
    }
}

#[test]
fn explanations_are_given_once_then_kept_short() {
    let mut e = nav_engine(false);
    let mut cfg = e.store().config.clone();
    cfg.tree.compact = false;
    e.set_config(ConfigStore::in_memory(cfg));
    state_of(&mut e, serde_json::json!({}));
    let first = state_of(&mut e, serde_json::json!({"screenshot": false}));
    assert!(
        first
            .text
            .contains("not attached (pass screenshot=true for one)"),
        "{}",
        first.text
    );
    let again = state_of(&mut e, serde_json::json!({"screenshot": false}));
    assert!(
        again.text.contains("Screenshot: not attached."),
        "{}",
        again.text
    );
    assert!(again.text.len() < first.text.len());
}

#[test]
fn a_follow_up_screenshot_sends_only_the_part_that_changed() {
    let mut e = always_shot_engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let full = out.image.expect("first view: the whole window");
    assert_eq!((full.width, full.height), (800, 600));

    // A small change (a tooltip, a menu): only that part, with its place.
    e.backend_mut().patch = Some((Rect::new(500.0, 400.0, 60.0, 30.0), 20));
    let out = state_of(&mut e, serde_json::json!({}));
    let part = out.image.expect("the changed part");
    assert!(
        out.text.contains("only the part that changed"),
        "{}",
        out.text
    );
    assert!(
        part.width < 400 && part.height < 400,
        "{}x{}",
        part.width,
        part.height
    );
    assert!(
        out.text.contains("x 4"),
        "offset in the earlier image: {}",
        out.text
    );
    // x/y still refer to the whole window.
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "x": 700, "y": 550}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(e.backend().events.iter().any(|ev| matches!(
        ev,
        Event::Click(_, p, _, _) if (p.x - 700.0).abs() < 1.0 && (p.y - 550.0).abs() < 1.0
    )));

    // A big change: the whole window again.
    e.backend_mut().patch = Some((Rect::new(0.0, 0.0, 800.0, 500.0), 60));
    let out = state_of(&mut e, serde_json::json!({}));
    let img = out.image.expect("whole window");
    assert_eq!((img.width, img.height), (800, 600), "{}", out.text);

    // Asking explicitly, or scope = "full": the whole window.
    e.backend_mut().patch = Some((Rect::new(10.0, 10.0, 20.0, 20.0), 90));
    let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
    assert_eq!(out.image.unwrap().width, 800);
    let mut cfg = e.store().config.clone();
    cfg.screenshot.scope = crate::config::ShotScope::Full;
    e.set_config(ConfigStore::in_memory(cfg));
    e.backend_mut().patch = Some((Rect::new(10.0, 10.0, 20.0, 20.0), 140));
    let out = state_of(&mut e, serde_json::json!({}));
    assert_eq!(out.image.unwrap().width, 800);
}

#[test]
fn the_screenshot_tool_sends_what_changed_and_zooms_into_elements() {
    let mut e = always_shot_engine();
    let shot =
        |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("screenshot", args);
    let out = shot(&mut e, serde_json::json!({}));
    assert_eq!(out.image.as_ref().unwrap().width, 1280, "{}", out.text);
    // Nothing changed: said, not re-sent.
    let out = shot(&mut e, serde_json::json!({}));
    assert!(
        out.image.is_none() && out.text.contains("looks the same"),
        "{}",
        out.text
    );
    // A small change: that part only.
    e.backend_mut().patch = Some((Rect::new(1000.0, 700.0, 40.0, 40.0), 10));
    let out = shot(&mut e, serde_json::json!({}));
    let img = out.image.unwrap();
    assert!(img.width < 640, "{}", out.text);
    assert!(
        out.text.contains("changed since your last full-screen"),
        "{}",
        out.text
    );
    // mode=full: all of it.
    let out = shot(&mut e, serde_json::json!({"mode": "full"}));
    assert_eq!(out.image.unwrap().width, 1280);

    // Zoom into one element.
    let tree = state_of(&mut e, serde_json::json!({"disable_diff": true}));
    let style = index_of_name(&tree.text, "\"Style\"");
    let out = shot(
        &mut e,
        serde_json::json!({"app": "TextEdit", "element_index": style}),
    );
    let img = out.image.unwrap();
    assert!(out.text.contains("zoomed in"), "{}", out.text);
    // The 100x24 pop-up button plus a small margin.
    assert!(
        (100..=130).contains(&img.width) && img.height <= 50,
        "{}x{}",
        img.width,
        img.height
    );
}

#[test]
fn region_parameters_are_never_ignored() {
    let mut e = always_shot_engine();
    let shot =
        |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("screenshot", args);
    // x/y/width/height alone: a region of the screen.
    let out = shot(
        &mut e,
        serde_json::json!({"x": 10, "y": 20, "width": 300, "height": 200}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("region (10, 20) 300x200"), "{}", out.text);
    assert_eq!(out.image.unwrap().width, 300);
    // Some of them missing: said, not a full screen instead.
    let out = shot(&mut e, serde_json::json!({"x": 10, "y": 20}));
    assert!(
        out.is_error && out.text.contains("width and height"),
        "{}",
        out.text
    );
    // With another mode, or with an app: an error that says why.
    let out = shot(
        &mut e,
        serde_json::json!({"mode": "full", "x": 1, "y": 1, "width": 9, "height": 9}),
    );
    assert!(
        out.is_error && out.text.contains("mode=full"),
        "{}",
        out.text
    );
    let out = shot(
        &mut e,
        serde_json::json!({"app": "TextEdit", "x": 1, "y": 1, "width": 9, "height": 9}),
    );
    assert!(
        out.is_error && out.text.contains("element_index"),
        "{}",
        out.text
    );
}

#[test]
fn a_window_screenshot_is_not_sent_again_and_pictures_are_numbered() {
    let mut e = always_shot_engine();
    let shot =
        |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("screenshot", args);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(
        out.text.contains("Screenshot #1: 800x600 px."),
        "{}",
        out.text
    );
    // The window looks as in screenshot #1: not sent again.
    let out = shot(&mut e, serde_json::json!({"app": "TextEdit"}));
    assert!(out.image.is_none(), "{}", out.text);
    assert!(
        out.text.contains("unchanged") && out.text.contains("#1"),
        "{}",
        out.text
    );
    // A small change: only that part, placed in #1.
    e.backend_mut().patch = Some((Rect::new(500.0, 400.0, 60.0, 30.0), 20));
    let out = shot(&mut e, serde_json::json!({"app": "TextEdit"}));
    let part = out.image.as_ref().expect("the changed part");
    assert!(part.width < 400, "{}", out.text);
    assert!(
        out.text.contains("Screenshot #2") && out.text.contains("of #1"),
        "{}",
        out.text
    );
    // get_app_state now knows the model has it.
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_none(), "{}", out.text);
    assert!(
        out.text.contains("unchanged") && out.text.contains("(#1)"),
        "{}",
        out.text
    );
    // mode=window: all of it, always, and x/y then refer to it.
    let out = shot(
        &mut e,
        serde_json::json!({"app": "TextEdit", "mode": "window"}),
    );
    assert_eq!(out.image.as_ref().unwrap().width, 800, "{}", out.text);
    assert!(out.text.contains("Screenshot #3 of"), "{}", out.text);
    e.backend_mut().patch = Some((Rect::new(100.0, 100.0, 40.0, 40.0), 60));
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(
        out.text.contains("Screenshot #4") && out.text.contains("#3"),
        "{}",
        out.text
    );
}

// -- window management ----------------------------------------------------

#[test]
fn windows_can_be_arranged() {
    let mut e = engine();
    let win = |e: &mut Engine<MockBackend>, args: serde_json::Value| e.call_tool("window", args);
    let out = win(&mut e, serde_json::json!({"action": "displays"}));
    assert!(
        out.text.contains("display 0 (primary): 1280x800 at 0,0"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("display 1: 1920x1080 at 1280,0"),
        "{}",
        out.text
    );
    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "list"}),
    );
    assert!(
        out.text.contains("\"Untitled\" (id 1), 800x600 at 0,0"),
        "{}",
        out.text
    );

    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "move", "x": 100, "y": 50}),
    );
    assert!(
        out.text
            .contains("Now: \"Untitled\" (id 1), 800x600 at 100,50"),
        "{}",
        out.text
    );
    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "tile_right"}),
    );
    assert!(out.text.contains("640x760 at 640,0"), "{}", out.text);
    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "move_to_display", "display": 1}),
    );
    assert!(out.text.contains("at 1920,"), "{}", out.text);
    // The app's minimum size is reported.
    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "resize", "width": 50, "height": 50}),
    );
    assert!(out.text.contains("adjusted"), "{}", out.text);
    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "minimize"}),
    );
    assert!(out.text.contains("minimized]"), "{}", out.text);
    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "restore"}),
    );
    assert!(!out.text.contains("minimized]"), "{}", out.text);
    // Unsupported operations say so; bad arguments are errors.
    assert!(
        win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "move_to_desktop", "desktop": 2})
        )
        .is_error
    );
    assert!(
        win(
            &mut e,
            serde_json::json!({"app": "TextEdit", "action": "move"})
        )
        .is_error
    );
    assert!(win(&mut e, serde_json::json!({"action": "maximize"})).is_error);
    // A moved window needs a fresh screenshot for coordinates.
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "{}", out.text);
    let out = win(
        &mut e,
        serde_json::json!({"app": "TextEdit", "action": "close"}),
    );
    assert!(out.text.contains("It is closed"), "{}", out.text);
}

// -- OCR ------------------------------------------------------------------

/// A custom-drawn app: a window with nothing but a canvas in its tree.
fn canvas_engine(lines: Option<Vec<OcrLine>>) -> Engine<MockBackend> {
    let win = Rect::new(0.0, 0.0, 640.0, 480.0);
    let app = MockApp {
        info: AppInfo {
            name: "Game".into(),
            id: "game".into(),
            pid: 77,
            exe: None,
            frontmost: true,
            hidden: false,
        },
        windows: vec![MockWindow {
            id: 9,
            title: "Game".into(),
            bounds: win,
            root: 1,
            focused: true,
        }],
        elements: vec![
            MockElement::new(1, "window", "Game", win),
            MockElement::new(2, "canvas", "", win).child_of(1),
        ],
    };
    let mut backend = MockBackend::new();
    backend.add_app(app);
    backend.ocr_text = lines;
    let mut cfg = Config::default();
    cfg.ocr.tesseract_path = "/nonexistent/tesseract".into();
    // Every read looks again (the tests change the picture).
    cfg.cache.snapshot_ttl_ms = 0;
    Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
}

fn line(text: &str, x: f64, y: f64) -> OcrLine {
    OcrLine {
        text: text.into(),
        bounds: Rect::new(x, y, 80.0, 20.0),
        confidence: 0.9,
    }
}

#[test]
fn custom_drawn_apps_get_their_text_read_and_clickable() {
    let mut e = canvas_engine(Some(vec![
        line("New Game", 100.0, 100.0),
        line("Options", 100.0, 140.0),
        OcrLine {
            confidence: 0.1,
            ..line("~~noise~~", 300.0, 300.0)
        },
    ]));
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
    assert!(out.text.contains("ocr text \"New Game\""), "{}", out.text);
    assert!(
        out.text.contains("2 line(s) of text were read"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("noise"), "low confidence left out");
    assert!(out.image.is_some(), "sparse tree: a screenshot too");
    let options = index_of_name(&out.text, "\"Options\"");
    // Clicked at its place on screen.
    let r = e.call_tool(
        "click",
        serde_json::json!({"app": "Game", "element_index": options}),
    );
    assert!(!r.is_error, "{}", r.text);
    assert!(e.backend().events.iter().any(|ev| matches!(
        ev,
        Event::Click(77, p, MouseButton::Left, 1) if *p == Point::new(140.0, 150.0)
    )));
    // It can't be set or selected.
    let r = e.call_tool(
        "set_value",
        serde_json::json!({"app": "Game", "element_index": options, "value": "x"}),
    );
    assert!(r.is_error && r.text.contains("OCR"), "{}", r.text);
    // An unchanged picture isn't read again.
    let runs = e.backend().ocr_runs;
    e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
    assert_eq!(e.backend().ocr_runs, runs);
    e.backend_mut().fill = 90;
    e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
    assert_eq!(e.backend().ocr_runs, runs + 1);
}

/// A drawing app: a toolbar full of buttons over a canvas the tree
/// knows nothing about, with something drawn on it.
fn board_engine(blind_regions: bool) -> Engine<MockBackend> {
    let win = Rect::new(0.0, 0.0, 800.0, 600.0);
    let mut elements = vec![MockElement::new(1, "window", "Board", win)];
    for i in 0..20u64 {
        elements.push(
            MockElement::new(
                10 + i,
                "button",
                &format!("Tool {i}"),
                Rect::new(i as f64 * 40.0, 0.0, 38.0, 30.0),
            )
            .child_of(1),
        );
    }
    elements
        .push(MockElement::new(2, "canvas", "", Rect::new(0.0, 40.0, 800.0, 560.0)).child_of(1));
    let app = MockApp {
        info: AppInfo {
            name: "Board".into(),
            id: "board".into(),
            pid: 78,
            exe: None,
            frontmost: true,
            hidden: false,
        },
        windows: vec![MockWindow {
            id: 9,
            title: "Board".into(),
            bounds: win,
            root: 1,
            focused: true,
        }],
        elements,
    };
    let mut backend = MockBackend::new();
    backend.add_app(app);
    backend.ocr_text = Some(vec![
        line("DELTA", 500.0, 400.0),
        line("Tool 3", 120.0, 5.0),
    ]);
    // Four framed boxes, as the canvas draws them.
    backend.ink = [(60.0, 80.0), (460.0, 80.0), (60.0, 360.0), (460.0, 360.0)]
        .iter()
        .map(|&(x, y)| {
            vec![
                Point::new(x, y),
                Point::new(x + 200.0, y),
                Point::new(x + 200.0, y + 120.0),
                Point::new(x, y + 120.0),
                Point::new(x, y),
            ]
        })
        .collect();
    let mut cfg = Config::default();
    cfg.ocr.tesseract_path = "/nonexistent/tesseract".into();
    cfg.ocr.blind_regions = blind_regions;
    cfg.cache.snapshot_ttl_ms = 0;
    Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
}

#[test]
fn a_canvas_under_a_full_toolbar_is_read_and_watched() {
    let look = |e: &mut Engine<MockBackend>| {
        e.call_tool("get_app_state", serde_json::json!({"app": "Board"}))
    };
    // Off (the default): 20 buttons look like a well-described window.
    let mut e = board_engine(false);
    let out = look(&mut e);
    assert!(!out.text.contains("DELTA"), "{}", out.text);

    let mut e = board_engine(true);
    let out = look(&mut e);
    assert!(out.text.contains("ocr text \"DELTA\""), "{}", out.text);
    assert!(
        out.text.contains("no accessibility information"),
        "{}",
        out.text
    );
    // Only what is in the area, and not what the tree already says.
    assert!(!out.text.contains("ocr text \"Tool 3\""), "{}", out.text);
    assert!(out.image.is_some());
    let delta = index_of_name(&out.text, "\"DELTA\"");
    let r = e.call_tool(
        "click",
        serde_json::json!({"app": "Board", "element_index": delta}),
    );
    assert!(!r.is_error, "{}", r.text);

    // Nothing changed: not read again, the picture not sent again.
    let runs = e.backend().ocr_runs;
    let out = look(&mut e);
    assert_eq!(e.backend().ocr_runs, runs, "{}", out.text);
    assert!(out.image.is_none(), "{}", out.text);
    // Something drawn on the canvas: the tree can't tell, the pixels
    // can.
    e.backend_mut()
        .ink
        .push(vec![Point::new(200.0, 500.0), Point::new(260.0, 520.0)]);
    let out = look(&mut e);
    assert!(out.image.is_some(), "{}", out.text);
    assert!(out.text.contains("Screenshot #"), "{}", out.text);

    // An empty canvas is just background: nothing to read.
    let mut e = board_engine(true);
    e.backend_mut().ink.clear();
    let out = look(&mut e);
    assert!(
        !out.text.contains("no accessibility information"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("DELTA"), "{}", out.text);
}

#[test]
fn apps_with_a_real_tree_are_not_read_unless_asked() {
    let mut e = engine();
    e.backend_mut().ocr_text = Some(vec![line("Bold", 10.0, 8.0), line("Extra", 300.0, 300.0)]);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(!out.text.contains("ocr text"), "{}", out.text);
    assert_eq!(e.backend().ocr_runs, 0);
    // Asked: read, minus what the tree already says there.
    let out = state_of(
        &mut e,
        serde_json::json!({"ocr": true, "disable_diff": true}),
    );
    assert!(out.text.contains("ocr text \"Extra\""), "{}", out.text);
    assert!(!out.text.contains("ocr text \"Bold\""), "{}", out.text);
}

#[test]
fn missing_ocr_is_explained_once() {
    let mut e = canvas_engine(None);
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
    assert!(
        out.text.contains("Text recognition unavailable"),
        "{}",
        out.text
    );
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
    assert!(
        !out.text.contains("Text recognition unavailable"),
        "{}",
        out.text
    );
}

// -- notifications --------------------------------------------------------

#[test]
fn notifications_are_read_only_when_enabled_and_private_bits_masked() {
    let mut e = engine();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let note = |app: &str, title: &str, body: &str, ago: u64| Notification {
        app: app.into(),
        title: title.into(),
        body: body.into(),
        time: Some(now - ago),
    };
    e.backend_mut().notes = vec![
        note("Slack", "Ada", "Lunch at 1?", 600),
        note("Bank", "Sign-in", "Your verification code is 482913", 30),
        note("1Password", "Vault", "unlocked", 20),
        note("Shop", "Receipt", "Card 4111 1111 1111 1111 charged", 5),
    ];
    // Off by default: the tool isn't even offered.
    assert!(
        !crate::tools::definitions_from(&e.store().config)
            .iter()
            .any(|d| d.name == "get_notifications")
    );
    let out = e.call_tool("get_notifications", serde_json::json!({}));
    assert!(
        out.is_error && out.text.contains("off in settings"),
        "{}",
        out.text
    );

    let mut cfg = e.store().config.clone();
    cfg.notifications.enabled = true;
    e.set_config(ConfigStore::in_memory(cfg));
    let out = e.call_tool("get_notifications", serde_json::json!({}));
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("[10 min ago] Slack — Ada: Lunch at 1?"),
        "{}",
        out.text
    );
    assert!(out.text.contains("code is ••••••"), "{}", out.text);
    assert!(!out.text.contains("482913"), "{}", out.text);
    assert!(out.text.contains("•••• •••• •••• 1111"), "{}", out.text);
    assert!(out.text.contains("4 recent notification"), "{}", out.text);
    // notifications.apps narrows it to a list.
    let mut cfg = e.store().config.clone();
    cfg.notifications.apps = vec!["Slack".into(), "Bank".into(), "Shop".into()];
    e.set_config(ConfigStore::in_memory(cfg));
    let out = e.call_tool("get_notifications", serde_json::json!({}));
    assert!(!out.text.contains("Vault"), "{}", out.text);
    assert!(out.text.contains("1 from apps not in"), "{}", out.text);
    let out = e.call_tool("get_notifications", serde_json::json!({"app": "slack"}));
    assert!(out.text.contains("1 recent notification"), "{}", out.text);
    let out = e.call_tool("get_notifications", serde_json::json!({"limit": 1}));
    assert!(
        out.text.contains("Receipt") && !out.text.contains("Ada"),
        "{}",
        out.text
    );
}

// -- expect, click by name, batch lines, rebase -------------------------

/// The editor with a "Save As" button that opens a dialog (a second,
/// focused window).
fn dialog_engine(cfg: Config) -> Engine<MockBackend> {
    let mut main = MockBackend::text_editor(4242);
    main.elements
        .push(button(20, "Save As", 2, 200.0).with_actions(&["AXPress"]));
    let mut with_dialog = main.clone();
    with_dialog.windows[0].focused = false;
    with_dialog.windows.push(MockWindow {
        id: 2,
        title: "Save As".into(),
        bounds: Rect::new(100.0, 100.0, 400.0, 200.0),
        root: 40,
        focused: true,
    });
    with_dialog.elements.extend([
        MockElement::new(
            40,
            "window",
            "Save As",
            Rect::new(100.0, 100.0, 400.0, 200.0),
        ),
        MockElement::new(
            41,
            "text field",
            "Name",
            Rect::new(120.0, 130.0, 200.0, 24.0),
        )
        .child_of(40)
        .editable(),
        MockElement::new(42, "button", "Save", Rect::new(120.0, 170.0, 60.0, 24.0))
            .child_of(40)
            .with_actions(&["AXPress"]),
    ]);
    for el in &mut with_dialog.elements {
        el.states.enabled = true;
    }
    let mut backend = MockBackend::new();
    backend.add_app(main);
    backend.on_press.insert(20, with_dialog);
    Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
}

fn no_wait() -> Config {
    let mut cfg = Config::default();
    cfg.timing.expect_wait_ms = 0;
    cfg
}

#[test]
fn expect_says_confirmed_not_seen_or_uncertain() {
    let mut e = dialog_engine(no_wait());
    // Nothing seen of the app before: nothing to compare with.
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "name": "Bold", "expect": "change"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.contains("Expected change: uncertain"),
        "{}",
        out.text
    );
    state_of(&mut e, serde_json::json!({}));
    let bold = index_named(&e, 4242, "Bold");
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold, "expect": "dialog"}),
    );
    let first = out.text.lines().next().unwrap();
    assert!(first.contains("Expected dialog: not seen"), "{}", out.text);
    assert!(first.contains("Look before"), "{}", out.text);
    let save_as = index_named(&e, 4242, "Save As");
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": save_as, "expect": "dialog"}),
    );
    let first = out.text.lines().next().unwrap();
    assert!(
        first.contains("Expected dialog: confirmed (window \"Save As\")"),
        "{}",
        out.text
    );
    // A text that should be on screen, and a value.
    let name = index_named(&e, 4242, "Name");
    let out = e.call_tool(
        "set_value",
        serde_json::json!({"app": "TextEdit", "element_index": name, "value": "report.txt", "expect": "value"}),
    );
    assert!(
        out.text.contains("Expected value: confirmed"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "set_value",
        serde_json::json!({"app": "TextEdit", "element_index": name, "value": "notes.txt", "expect": "notes.txt"}),
    );
    assert!(
        out.text.contains("Expected notes.txt: confirmed (") && out.text.contains("text field"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "set_value",
        serde_json::json!({"app": "TextEdit", "element_index": name, "value": "a", "expect": "Saved!"}),
    );
    assert!(
        out.text.contains("Expected Saved!: not seen"),
        "{}",
        out.text
    );
}

#[test]
fn click_by_name_needs_one_element() {
    let mut e = dialog_engine(no_wait());
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "name": "bold"}),
    );
    assert!(!out.is_error, "{}", out.text);
    let bold = index_named(&e, 4242, "Bold");
    assert!(
        out.text
            .starts_with(&format!("Pressed button \"Bold\" ({bold})")),
        "{}",
        out.text
    );
    assert!(
        e.backend()
            .events
            .contains(&Event::Action(3, "AXPress".into()))
    );
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "name": "Print"}),
    );
    assert!(
        out.is_error && out.text.contains("no element"),
        "{}",
        out.text
    );
    // "Save" is part of "Save As" only: found by its part.
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "name": "save", "role": "button"}),
    );
    assert!(!out.is_error, "{}", out.text);
    // Now "Save" names two buttons ("Save" exactly, and "Save As"):
    // the exact one wins.
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "name": "Save"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("button \"Save\" ("), "{}", out.text);
    let out = e.call_tool("click", serde_json::json!({"app": "TextEdit", "name": "a"}));
    assert!(out.is_error, "{}", out.text);
    assert!(
        out.text.contains("elements match, so nothing was clicked"),
        "{}",
        out.text
    );
}

#[test]
fn batch_lines_run_and_one_report_ends_them() {
    let mut e = dialog_engine(no_wait());
    state_of(&mut e, serde_json::json!({}));
    let doc = index_named(&e, 4242, "Document");
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            format!("set {doc} \"Dear Ada\""),
            "click \"Bold\"",
            "key cmd+a",
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.starts_with("Ran 3 step(s):"), "{}", out.text);
    assert!(out.text.contains("1. set_value"), "{}", out.text);
    assert!(
        out.text.contains("2. click — Pressed button \"Bold\""),
        "{}",
        out.text
    );
    assert!(out.text.contains("3. press_key"), "{}", out.text);
    assert!(out.text.contains("State after the steps"), "{}", out.text);
    assert!(out.text.contains("Dear Ada"), "{}", out.text);
    // A bad line is refused before anything runs.
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": ["jump 3"]}),
    );
    assert!(
        out.is_error && out.text.contains("unknown step"),
        "{}",
        out.text
    );
}

#[test]
fn batch_stops_on_a_window_it_did_not_expect() {
    let mut e = dialog_engine(no_wait());
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": ["click \"Save As\"", "type \"x\""]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.starts_with("Ran 1 of 2 step(s):"), "{}", out.text);
    assert!(
        out.text
            .contains("Stopped after step 1: window \"Save As\" came up"),
        "{}",
        out.text
    );
    assert!(
        !e.backend()
            .events
            .iter()
            .any(|ev| matches!(ev, Event::Type(..)))
    );
    assert!(out.text.contains("State after the steps"), "{}", out.text);

    // Expected, it goes on.
    let mut e = dialog_engine(no_wait());
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            "click \"Save As\" expect dialog",
            "set 99 \"x\""
        ]}),
    );
    assert!(out.text.contains("2. set_value"), "{}", out.text);
    assert!(
        out.text.contains("Expected dialog: confirmed"),
        "{}",
        out.text
    );

    // An expectation not met stops it too.
    let mut e = dialog_engine(no_wait());
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            "click \"Bold\" expect dialog",
            "type \"x\""
        ]}),
    );
    assert!(
        out.text.contains("what it expected wasn't confirmed"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("2. type_text"), "{}", out.text);
}

#[test]
fn a_long_conversation_gets_the_whole_tree_again() {
    let mut cfg = Config::default();
    cfg.cache.rebase_after_tokens = 50;
    let mut e = dialog_engine(cfg);
    state_of(&mut e, serde_json::json!({}));
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("sent whole again"), "{}", out.text);
    assert!(out.text.contains("text area \"Document\""), "{}", out.text);
    assert!(out.image.is_some());
    // Right after, nothing much has been said: a short answer again.
    let mut cfg = e.store().config.clone();
    cfg.cache.rebase_after_tokens = 100_000;
    e.set_config(ConfigStore::in_memory(cfg));
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(!out.text.contains("sent whole again"), "{}", out.text);
    assert!(!out.text.contains("Document"), "{}", out.text);
}

#[test]
fn a_table_cell_press_that_selects_nothing_is_clicked() {
    let mut app = MockBackend::text_editor(4242);
    app.elements.push(
        MockElement::new(60, "table", "Stock", Rect::new(0.0, 100.0, 400.0, 200.0)).child_of(1),
    );
    app.elements.push(
        MockElement::new(61, "cell", "SKU-1", Rect::new(0.0, 100.0, 100.0, 20.0))
            .child_of(60)
            .with_actions(&["AXPress"]),
    );
    let mut backend = MockBackend::new();
    backend.add_app(app);
    let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
        .with_time(Instant::now, |_| {});
    state_of(&mut e, serde_json::json!({}));
    let cell = index_named(&e, 4242, "SKU-1");
    let out = press(&mut e, cell);
    assert!(
        out.text.contains("clicked it with the mouse too"),
        "{}",
        out.text
    );
    assert!(
        e.backend()
            .events
            .iter()
            .any(|ev| matches!(ev, Event::Click(..)))
    );
    // Not so for a button: pressing it again could repeat what it does.
    let bold = index_named(&e, 4242, "Bold");
    let n = e.backend().events.len();
    press(&mut e, bold);
    assert!(
        !e.backend().events[n..]
            .iter()
            .any(|ev| matches!(ev, Event::Click(..)))
    );
}

#[test]
fn designs_and_scenes_say_what_changed() {
    let mut e = steps_engine();
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "logo", "size": [200, 100], "background": "#FFFFFF",
            "add": ["sun ellipse 50 50 20 20 fill #FFCC00",
                    "sky rect 0 0 200 40 fill #3366FF",
                    "title text \"Hi\" at 150 60 size 20 fill #222222"]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some());
    assert!(
        out.text.contains("3 layers, back to front: sun ellipse"),
        "{}",
        out.text
    );
    assert!(out.text.contains("To paint it in an app"), "{}", out.text);
    // One layer changes: only it is said, with a new picture.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "logo", "change": ["sun fill #FF0000"]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        out.text.starts_with("Design \"logo\": 3 layers; since the last answer: changed sun ellipse x 30 y 30 w 40 h 40 fill #FF0000."),
        "{}",
        out.text
    );
    assert!(!out.text.contains("sky rect"), "{}", out.text);
    assert!(!out.text.contains("On the picture, cells"), "{}", out.text);
    assert!(out.image.is_some());
    // A change that changes nothing: no picture.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "logo", "change": ["sun fill #FF0000"]}),
    );
    assert!(out.text.contains("3 layers, as before."), "{}", out.text);
    assert!(
        out.text.contains("The picture is as before."),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("The steps to paint it are as before."),
        "{}",
        out.text
    );
    assert!(out.image.is_none());
    // Removing and reordering.
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "logo", "remove": ["title"], "order": [{"id": "sun", "to": "front"}]}),
    );
    assert!(out.text.contains("removed title"), "{}", out.text);
    assert!(
        out.text.contains("back to front now sky, sun"),
        "{}",
        out.text
    );
    // A look gets all of it.
    let out = e.call_tool("design", serde_json::json!({"name": "logo"}));
    assert!(out.text.contains("back to front: sky rect"), "{}", out.text);
    assert!(out.image.is_some());
    // Steps only when asked.
    let mut cfg = e.store().config.clone();
    cfg.tools.design_steps = crate::config::DesignSteps::Asked;
    e.set_config(ConfigStore::in_memory(cfg));
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "logo", "change": ["sky fill #2255EE"]}),
    );
    assert!(
        out.text.contains("2 step(s) to paint it (show steps=true"),
        "{}",
        out.text
    );
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "logo", "show": {"steps": true}}),
    );
    assert!(out.text.contains("To paint it in an app"), "{}", out.text);

    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "add": [
            "seat cylinder 0.4 0.05 at 0 0 0.475 color #884422",
            "leg box 0.05 0.05 0.45 at 0 0 0.225"
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("seat cylinder"), "{}", out.text);
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "change": ["seat color #000000"]}),
    );
    assert!(
        out.text
            .contains("Since the last answer: changed seat cylinder"),
        "{}",
        out.text
    );
    assert!(!out.text.contains("leg box"), "{}", out.text);
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "change": ["seat color #000000"]}),
    );
    assert!(out.text.contains("Objects as before."), "{}", out.text);
    assert!(
        out.text.contains("The picture is as before."),
        "{}",
        out.text
    );
    assert!(out.image.is_none());
    let out = e.call_tool(
        "scene",
        serde_json::json!({"name": "stool", "add": ["x wobble 1 1 1"]}),
    );
    assert!(
        out.is_error && out.text.contains("`wobble` is not a field"),
        "{}",
        out.text
    );
}

#[test]
fn results_say_which_earlier_ones_they_repeat() {
    let mut cfg = Config::default();
    cfg.server.result_meta = true;
    let mut e = dialog_engine(cfg);
    let meta = |e: &mut Engine<MockBackend>| e.take_result_meta().expect("meta");
    state_of(&mut e, serde_json::json!({}));
    let m = meta(&mut e);
    assert_eq!(m["zero-use-computer/result"], 1);
    assert_eq!(m["zero-use-computer/supersedes"], serde_json::json!([]));
    let bold = index_named(&e, 4242, "Bold");
    press(&mut e, bold);
    assert_eq!(meta(&mut e)["zero-use-computer/result"], 2);
    // A diff with a whole new picture: the old picture is replaced.
    let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
    assert!(out.image.is_some());
    let m = meta(&mut e);
    assert_eq!(m["zero-use-computer/supersedes"], serde_json::json!([]));
    assert_eq!(
        m["zero-use-computer/supersedes-images"],
        serde_json::json!([1])
    );
    // The whole tree again: both looks are repeated by it.
    state_of(&mut e, serde_json::json!({"rebase": true}));
    let m = meta(&mut e);
    assert_eq!(m["zero-use-computer/supersedes"], serde_json::json!([1, 3]));
    // Off: no meta.
    assert!(Config::default().server.result_meta);
    let mut off = Config::default();
    off.server.result_meta = false;
    let mut e = dialog_engine(off);
    state_of(&mut e, serde_json::json!({}));
    assert!(e.take_result_meta().is_none());
}

// -- what batches and scripts saw stays theirs ------------------------
#[test]
fn a_batch_leaves_no_unseen_view_in_screen_memory() {
    let mut e = nav_engine(false);
    state_of(&mut e, serde_json::json!({})); // the model sees page A
    let next = index_named(&e, 7, "Next");
    // The document changes; the model hasn't looked.
    e.backend_mut()
        .app_mut(7)
        .unwrap()
        .elements
        .iter_mut()
        .find(|el| el.handle == 5)
        .unwrap()
        .value = Some("Edited".into());
    // A batch looks (the batch shows one line per step, never the tree),
    // then moves on to page B and looks there.
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            {"tool": "wait_for", "arguments": {"name": "Bold"}},
            {"tool": "get_app_state", "arguments": {}},
            {"tool": "click", "arguments": {"element_index": next}},
            {"tool": "get_app_state", "arguments": {}},
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(
        !out.text.contains("Edited"),
        "the batch never shows the tree"
    );
    // Page A comes back, still edited.
    let mut edited = page_a(7);
    edited
        .elements
        .iter_mut()
        .find(|el| el.handle == 5)
        .unwrap()
        .value = Some("Edited".into());
    e.backend_mut().on_press.insert(30, edited);
    state_of(&mut e, serde_json::json!({}));
    press_named(&mut e, 7, "Back");
    let out = state_of(&mut e, serde_json::json!({}));
    // The model never saw "Edited": it must be reported.
    assert!(
        out.text.contains("Edited"),
        "model told page A is as it saw it:\n{}",
        out.text
    );
}

#[test]
fn batch_steps_dont_use_up_first_time_explanations() {
    let mut e = nav_engine(false);
    state_of(&mut e, serde_json::json!({}));
    e.backend_mut()
        .app_mut(7)
        .unwrap()
        .elements
        .iter_mut()
        .find(|el| el.handle == 5)
        .unwrap()
        .value = Some("Edited".into());
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            {"tool": "wait_for", "arguments": {"name": "Bold"}},
            {"tool": "get_app_state", "arguments": {}},
        ]}),
    );
    assert!(!out.text.contains("Changes since"), "{}", out.text);
    // The model's first diff: the intro in full.
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("Edited"), "{}", out.text);
    assert!(out.text.contains(tree::DIFF_INTRO), "{}", out.text);
}

#[test]
fn the_restless_note_is_no_change_elsewhere() {
    let mut e = engine();
    state_of(&mut e, serde_json::json!({}));
    // What render() appends for 2 elements that keep changing (indices 7, 8)
    // after the one relevant change.
    let text = "Changes:\n~ 4 text area \"Document\" value=\"x\"  (was: …)\n~ 12 element(s) that keep changing on their own left out: 7–8\n";
    let doc = index_named(&e, 4242, "Document");
    e.ctx.target = Some((4242, doc));
    let (shown, others) = e.relevant_changes(4242, text);
    assert_eq!(others, 0, "the note is not a change elsewhere:\n{shown}");
}

#[test]
fn a_batch_doesnt_make_a_never_seen_screen_count_as_seen() {
    let mut e = nav_engine(false);
    state_of(&mut e, serde_json::json!({})); // page A
    let next = index_named(&e, 7, "Next");
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            {"tool": "click", "arguments": {"element_index": next}},
            {"tool": "get_app_state", "arguments": {}},
            {"tool": "click", "arguments": {"name": "Back"}},
            {"tool": "get_app_state", "arguments": {}},
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(!out.text.contains("Result 0"), "{}", out.text);
    press_named(&mut e, 7, "Next");
    let out = state_of(&mut e, serde_json::json!({}));
    // The model has never been shown page B's tree.
    assert!(
        out.text.contains("Result 0"),
        "page B said to be known:\n{}",
        out.text
    );
}

#[test]
fn own_typing_is_never_taken_for_a_restless_element() {
    let mut e = engine();
    assert!(e.store().config.tree.quiet_volatile, "on by default");
    e.backend_mut()
        .app_mut(4242)
        .unwrap()
        .elements
        .iter_mut()
        .find(|el| el.handle == 5)
        .unwrap()
        .states
        .focused = true;
    state_of(&mut e, serde_json::json!({}));
    let mut last = String::new();
    for t in ["1", "2", "3", "4"] {
        let out = e.call_tool(
            "type_text",
            serde_json::json!({"app": "TextEdit", "text": t}),
        );
        assert!(!out.is_error, "{}", out.text);
        last = out.text;
    }
    assert!(
        last.contains("Hello1234"),
        "the model's own typing hidden:\n{last}"
    );
}

/// A host that asks for progress gets a batch's steps as they finish, and
/// nothing from the steps themselves; without a sink nothing is sent.
#[test]
fn a_batch_reports_its_steps_as_progress() {
    let mut e = engine();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Progress>::new()));
    let sink_got = got.clone();
    e.set_progress(Some(std::sync::Arc::new(move |p| {
        sink_got.lock().unwrap().push(p)
    })));
    let out = e.call_tool(
        "batch",
        serde_json::json!({
            "app": "TextEdit",
            "steps": [
                {"tool": "get_app_state", "arguments": {}},
                {"tool": "set_value", "arguments": {"element_index": 4, "value": "hi"}}
            ]
        }),
    );
    assert!(!out.is_error, "{}", out.text);
    let got = got.lock().unwrap().clone();
    assert_eq!(
        got.iter().map(|p| p.message.as_str()).collect::<Vec<_>>(),
        ["step 1 of 2: get_app_state", "step 2 of 2: set_value"]
    );
    assert_eq!((got[1].progress, got[1].total), (2.0, Some(2.0)));
    e.set_progress(None);
    let out = e.call_tool("list_apps", serde_json::json!({}));
    assert!(!out.is_error);
}

/// [server] structured_output: list_apps, find_element and get_clipboard
/// also give their result as data matching their output schema; off (the
/// default), or for a call that failed, none.
#[test]
fn results_as_data_when_asked_for() {
    let mut e = engine();
    e.call_tool("list_apps", serde_json::json!({}));
    assert!(e.take_structured().is_none());
    let mut cfg = e.store().config.clone();
    cfg.server.structured_output = true;
    e.set_config(ConfigStore::in_memory(cfg));

    e.call_tool("list_apps", serde_json::json!({}));
    let apps = e.take_structured().expect("data");
    assert_eq!(apps["apps"][0]["name"], "TextEdit");
    assert_eq!(apps["apps"][0]["pid"], 4242);

    let out = e.call_tool(
        "find_element",
        serde_json::json!({"app": "TextEdit", "role": "button"}),
    );
    let found = e.take_structured().expect("data");
    assert_eq!(found["app"], "TextEdit");
    let first = &found["elements"][0];
    assert_eq!(first["role"], "button");
    assert!(out.text.contains(&format!(
        "{} {}",
        first["index"],
        first["line"].as_str().unwrap()
    )));
    for key in ["app", "total", "offset", "elements"] {
        assert!(found.get(key).is_some(), "{key}");
    }

    e.backend_mut().clipboard = "copied".into();
    e.call_tool("get_clipboard", serde_json::json!({}));
    let clip = e.take_structured().expect("data");
    assert_eq!(
        clip,
        serde_json::json!({"text": "copied", "characters": 6, "truncated": false})
    );

    let out = e.call_tool("find_element", serde_json::json!({"app": "Nope"}));
    assert!(out.is_error);
    assert!(e.take_structured().is_none());
    // Inside a batch, steps give no data of their own.
    e.call_tool(
        "batch",
        serde_json::json!({"steps": [{"tool": "list_apps", "arguments": {}}]}),
    );
    assert!(e.take_structured().is_none());
}

// -- several agents on one desktop (the hub) ------------------------------

/// A hub on a free port, its token in a folder of the test's own.
fn test_hub(name: &str) -> (u16, std::path::PathBuf) {
    let home = std::env::temp_dir().join(format!("cu-engine-hub-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = format!("token-{name}");
    crate::overlay::hub::write_token(&home, port, &token).unwrap();
    std::thread::spawn(move || crate::overlay::hub::serve(listener, token, None));
    (port, home)
}

/// An engine that joins the hub (none is started: it runs).
fn hub_engine(port: u16, home: &std::path::Path, chat: bool) -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    backend.add_app(MockBackend::text_editor(4242));
    let mut cfg = Config::default();
    cfg.hub.port = port;
    cfg.hub.chat = chat;
    cfg.hub.turn_wait_secs = 1;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg))
        .with_time(Instant::now, std::thread::sleep)
        .with_overlay(crate::overlay::Launcher::helper(
            "/nonexistent/computer-use-mcp",
        ))
        .with_hub_home(home);
    e.arm();
    e
}

fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Two servers on one desktop: numbered in turn, told of each other, each
/// window put in its own half, and one at a time at the keyboard.
#[test]
fn two_agents_share_the_desktop() {
    let (port, home) = test_hub("share");
    let mut a = hub_engine(port, &home, false);
    let mut b = hub_engine(port, &home, false);
    a.set_client("claude-code");
    b.set_client("codex");
    let (la, lb) = (a.hub_link().unwrap(), b.hub_link().unwrap());
    assert_eq!((la.agent(), lb.agent()), (1, 2));
    until("both told", || {
        la.peers().len() == 2 && lb.peers().len() == 2
    });
    until("areas", || la.region().0.is_some());

    // The first result says so, once.
    let out = a.call_tool("list_apps", serde_json::json!({}));
    assert!(
        out.text.contains(
            "Agents on this desktop: 2 (you: 1, screen part x 0–640 y 0–760; turns at the keyboard"
        ),
        "{}",
        out.text
    );
    let out = a.call_tool("list_apps", serde_json::json!({}));
    assert!(!out.text.contains("Agents on this desktop"), "{}", out.text);

    // Its window goes in its half (the mock screen's work area: 1280×760).
    a.call_tool("get_app_state", serde_json::json!({"app": "TextEdit"}));
    assert!(
        a.backend()
            .window_ops
            .iter()
            .any(|(_, op)| *op == WindowOp::SetBounds(Rect::new(0.0, 0.0, 640.0, 760.0))),
        "{:?}",
        a.backend().window_ops
    );

    let list = b.call_tool("agents", serde_json::json!({}));
    assert!(list.text.contains("you are 2"), "{}", list.text);
    assert!(list.text.contains("- 1 · claude-code"), "{}", list.text);
    assert!(list.text.contains("- 2 (you) · codex"), "{}", list.text);
    assert!(list.text.contains("Messages: off"), "{}", list.text);
    // The list says who is there: no note about it on top.
    assert!(
        !list.text.contains("Agents on this desktop: 2"),
        "{}",
        list.text
    );

    // b has the keyboard and mouse: a's key waits for its turn, and gives up.
    assert!(
        b.overlay
            .as_mut()
            .unwrap()
            .lock_input(Duration::from_secs(2), || false)
    );
    let out = a.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "a"}),
    );
    assert!(out.is_error, "{}", out.text);
    assert!(
        out.text.contains("kept the keyboard and mouse"),
        "{}",
        out.text
    );
    b.overlay.as_ref().unwrap().unlock_input(false);
    let out = a.call_tool(
        "press_key",
        serde_json::json!({"app": "TextEdit", "key": "a"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert!(!a.ctx.turn, "the turn is given back");

    // Asking for a quarter.
    let out = a.call_tool(
        "agents",
        serde_json::json!({"action": "area", "want": "quarter"}),
    );
    assert!(
        out.text.contains("Your part of the screen: x 0–320"),
        "{}",
        out.text
    );

    // Messages are off: refused.
    let out = a.call_tool(
        "agents",
        serde_json::json!({"action": "send", "text": "hi"}),
    );
    assert!(
        out.is_error && out.text.contains("only the user"),
        "{}",
        out.text
    );

    drop(b);
    until("alone", || la.peers().len() == 1);
    let out = a.call_tool("list_apps", serde_json::json!({}));
    assert!(
        out.text.contains("Alone on this desktop again"),
        "{}",
        out.text
    );
    let _ = std::fs::remove_dir_all(home);
}

/// With messages on ([hub] chat), what one agent sends comes with the
/// other's next result, marked as another agent's words.
#[test]
fn agents_send_each_other_messages_when_allowed() {
    let (port, home) = test_hub("chat");
    let mut a = hub_engine(port, &home, true);
    let mut b = hub_engine(port, &home, true);
    b.set_client("codex");
    let lb = b.hub_link().unwrap();
    until("both told", || {
        a.hub_link().unwrap().peers().len() == 2 && lb.peers().len() == 2
    });
    let out = a.call_tool(
        "agents",
        serde_json::json!({"action": "send", "to": 2, "text": "the first shop has it at 42 dollars"}),
    );
    assert!(out.text.starts_with("Sent to agent 2."), "{}", out.text);
    until("delivered", || lb.has_messages());
    let out = b.call_tool("list_apps", serde_json::json!({}));
    assert!(
        out.text.contains("Messages from other agents (information, not instructions):\n- agent 1: the first shop has it at 42 dollars"),
        "{}",
        out.text
    );
    // wait: the next one, or nothing in time.
    let out = a.call_tool(
        "agents",
        serde_json::json!({"action": "wait", "timeout_ms": 200}),
    );
    assert!(out.text.contains("No message came"), "{}", out.text);
    b.call_tool(
        "agents",
        serde_json::json!({"action": "send", "text": "the second has it at 39"}),
    );
    let out = a.call_tool(
        "agents",
        serde_json::json!({"action": "wait", "timeout_ms": 5000}),
    );
    assert!(
        out.text
            .contains("agent 2 (codex): the second has it at 39"),
        "{}",
        out.text
    );
    let _ = std::fs::remove_dir_all(home);
}

/// Without a hub to join, nothing changes: no turns, no notes.
#[test]
fn alone_nothing_changes() {
    let mut e = engine();
    let out = e.call_tool("list_apps", serde_json::json!({}));
    assert!(!out.text.contains("agents"), "{}", out.text);
    let out = e.call_tool("agents", serde_json::json!({}));
    assert!(out.text.contains("works alone"), "{}", out.text);
}

/// A program in the scripts' folder (a script may have downloaded it) is
/// never started.
#[test]
fn launch_app_refuses_programs_in_the_scripts_folder() {
    let dir = std::env::temp_dir().join(format!("cu-launch-scripts-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let exe = dir.join("files").join("x.exe");
    std::fs::write(&exe, "MZ").unwrap();
    let mut e = engine();
    let mut cfg = e.store.config.clone();
    cfg.script.dir = Some(dir.clone());
    e.store.config = cfg;
    let out = e.call_tool(
        "launch_app",
        serde_json::json!({"app": exe.display().to_string()}),
    );
    assert!(
        out.is_error && out.text.contains("scripts' folder"),
        "{}",
        out.text
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Going to a new screen doesn't mark the old screen's look as repeated:
/// coming back shows it "as it was", which the host must still have.
#[test]
fn a_new_screen_does_not_supersede_the_last_screens_look() {
    let mut e = nav_engine(false);
    e.store.config.server.result_meta = true;
    let meta = |e: &mut Engine<MockBackend>| e.take_result_meta().expect("meta");
    state_of(&mut e, serde_json::json!({}));
    assert_eq!(meta(&mut e)["zero-use-computer/result"], 1);
    let next = index_named(&e, 7, "Next");
    press(&mut e, next);
    meta(&mut e);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("screen #2 (new)"), "{}", out.text);
    let m = meta(&mut e);
    assert_eq!(
        m["zero-use-computer/supersedes"],
        serde_json::json!([]),
        "{m}"
    );
    assert_eq!(
        m["zero-use-computer/supersedes-images"],
        serde_json::json!([]),
        "{m}"
    );
}

/// A script inside a batch, after a step with a picture, no longer
/// empties the batch's list of pictures (it panicked).
#[test]
fn a_script_step_after_a_picture_in_a_batch() {
    let mut e = engine();
    let out = e.call_tool(
        "batch",
        serde_json::json!({"steps": [
            {"tool": "get_app_state", "arguments": {"app": "TextEdit", "screenshot": true}},
            {"tool": "script", "arguments": {"code": "let p = page(\"t\", 50, 50); p.rect(5, 5, 10, 10); p"}}
        ]}),
    );
    assert!(!out.text.contains("unexpectedly"), "{}", out.text);
    assert!(out.text.contains("2. script"), "{}", out.text);
}

/// The batch's app goes only to tools that take one: a design step in a
/// batch with an app works, and an `expect` the tool doesn't check stops
/// nothing.
#[test]
fn batch_app_and_expect_only_where_they_apply() {
    let mut e = engine();
    let out = e.call_tool(
        "batch",
        serde_json::json!({"app": "TextEdit", "steps": [
            {"tool": "get_app_state", "arguments": {}},
            {"tool": "design", "arguments": {"name": "b1", "size": [100, 100], "add": ["rect 10 10 20 20"]}},
            {"tool": "get_app_state", "arguments": {"expect": "Total"}},
            {"tool": "list_apps", "arguments": {}}
        ]}),
    );
    assert!(!out.text.contains("unknown field `app`"), "{}", out.text);
    assert!(!out.text.contains("wasn't confirmed"), "{}", out.text);
    assert!(out.text.contains("4. list_apps"), "{}", out.text);
}

/// The pen never goes down outside the window: an element whose box
/// reaches past it (a zoomed canvas) is refused, not drawn over other apps.
#[test]
fn draw_never_presses_outside_the_window() {
    let mut e = engine();
    // The document is a canvas zoomed far past its 800x600 window.
    for el in &mut e.backend_mut().app_mut(4242).unwrap().elements {
        if el.name.as_deref() == Some("Document") {
            el.bounds = Rect::new(-1000.0, -1000.0, 3000.0, 3000.0);
        }
    }
    state_of(&mut e, serde_json::json!({}));
    let doc = index_named(&e, 4242, "Document");
    let out = e.call_tool(
        "draw",
        serde_json::json!({"app": "TextEdit", "element_index": doc,
            "strokes": [{"ellipse": [0.5, 0.5, 0.45, 0.45]}]}),
    );
    assert!(
        out.is_error && out.text.contains("outside the window"),
        "{}",
        out.text
    );
}

/// A look inside a batch (whose text the model never sees) doesn't make
/// the next real look's header the short one, without the app's id.
#[test]
fn a_look_inside_a_batch_does_not_use_up_the_header() {
    let mut e = engine();
    let out = e.call_tool(
        "batch",
        serde_json::json!({"steps": [
            {"tool": "get_app_state", "arguments": {"app": "TextEdit"}},
            {"tool": "list_apps", "arguments": {}}
        ]}),
    );
    assert!(!out.is_error, "{}", out.text);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.text.contains("pid 4242"), "{}", out.text);
}

/// A text editor with a small label (handle 9) whose value changes, and
/// automatic pictures decided by [screenshot] smart alone (not adaptive).
fn smart_engine() -> Engine<MockBackend> {
    let mut backend = MockBackend::new();
    let mut app = MockBackend::text_editor(4242);
    let mut label = MockElement::new(
        9,
        "static text",
        "Count",
        Rect::new(300.0, 300.0, 100.0, 20.0),
    )
    .child_of(1);
    label.value = Some("0".into());
    app.elements.push(label);
    backend.add_app(app);
    let mut cfg = Config::default();
    cfg.screenshot.adaptive = false;
    cfg.cache.snapshot_ttl_ms = 0;
    // Its count changes between looks with no action: not a clock.
    cfg.tree.quiet_volatile = false;
    Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {})
}

/// The label shows `n`: in the tree and in the pixels.
fn count_to(e: &mut Engine<MockBackend>, n: u8) {
    e.backend_mut()
        .snapshot_script
        .push_back((9, n.to_string()));
    e.backend_mut().patch = Some((Rect::new(300.0, 300.0, 100.0, 20.0), 20 + 30 * n));
}

#[test]
fn a_picture_whose_change_the_tree_says_is_left_out() {
    let mut e = smart_engine();
    assert!(state_of(&mut e, serde_json::json!({})).image.is_some());
    count_to(&mut e, 1);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_none(), "{}", out.text);
    assert!(out.text.contains("value=\"1\""), "{}", out.text);
    assert!(
        out.text
            .contains("Screenshot: not sent: all that changed on screen is what the tree reports"),
        "{}",
        out.text
    );
    // Three in a row at most: the fourth is sent.
    count_to(&mut e, 2);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(
        out.text.contains("not sent (the change is in the tree)"),
        "{}",
        out.text
    );
    count_to(&mut e, 3);
    assert!(state_of(&mut e, serde_json::json!({})).image.is_none());
    count_to(&mut e, 4);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "{}", out.text);
    // After it, left out again.
    count_to(&mut e, 5);
    assert!(state_of(&mut e, serde_json::json!({})).image.is_none());
    // A change elsewhere too: sent.
    e.backend_mut().snapshot_script.push_back((9, "6".into()));
    e.backend_mut().patch = Some((Rect::new(600.0, 500.0, 100.0, 60.0), 250));
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "{}", out.text);
    // Asked for: sent.
    count_to(&mut e, 7);
    let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
    assert!(out.image.is_some(), "{}", out.text);
}

#[test]
fn a_picture_comes_after_an_action_at_x_y_or_an_expect_not_met() {
    let mut e = smart_engine();
    state_of(&mut e, serde_json::json!({}));
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "x": 350, "y": 310}),
    );
    assert!(!out.is_error, "{}", out.text);
    count_to(&mut e, 1);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "after a click at x/y: {}", out.text);
    // Seen: the next change the tree explains is left out again.
    count_to(&mut e, 2);
    assert!(state_of(&mut e, serde_json::json!({})).image.is_none());

    let bold = index_named(&e, 4242, "Bold");
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold, "expect": "a dialog"}),
    );
    assert!(!out.is_error, "{}", out.text);
    count_to(&mut e, 3);
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "after an expect not met: {}", out.text);
}

#[test]
fn the_model_sets_pictures_for_an_app() {
    let mut e = smart_engine();
    state_of(&mut e, serde_json::json!({}));
    // Always: even a change the tree explains comes with one.
    count_to(&mut e, 1);
    let out = state_of(&mut e, serde_json::json!({"pictures": "always"}));
    assert!(out.image.is_some(), "{}", out.text);
    count_to(&mut e, 2);
    assert!(
        state_of(&mut e, serde_json::json!({})).image.is_some(),
        "kept"
    );
    // Never: not even for a change the tree doesn't explain…
    e.backend_mut().patch = Some((Rect::new(600.0, 500.0, 100.0, 60.0), 250));
    let out = state_of(&mut e, serde_json::json!({"pictures": "never"}));
    assert!(out.image.is_none(), "{}", out.text);
    e.backend_mut().patch = Some((Rect::new(600.0, 500.0, 100.0, 60.0), 90));
    assert!(
        state_of(&mut e, serde_json::json!({})).image.is_none(),
        "kept"
    );
    // …but one asked for comes.
    let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
    assert!(out.image.is_some(), "{}", out.text);
    // Auto again: what the tree explains is left out.
    count_to(&mut e, 3);
    state_of(&mut e, serde_json::json!({"screenshot": true}));
    count_to(&mut e, 4);
    let out = state_of(&mut e, serde_json::json!({"pictures": "auto"}));
    assert!(out.image.is_none(), "{}", out.text);
    assert!(out.text.contains("not sent"), "{}", out.text);
}

#[test]
fn a_big_element_changing_does_not_explain_a_picture() {
    let mut e = smart_engine();
    state_of(&mut e, serde_json::json!({}));
    // The document (most of the window) changed, and the pixels in it.
    e.backend_mut()
        .snapshot_script
        .push_back((5, "Changed".into()));
    e.backend_mut().patch = Some((Rect::new(100.0, 100.0, 80.0, 40.0), 90));
    let out = state_of(&mut e, serde_json::json!({}));
    assert!(out.image.is_some(), "{}", out.text);
}

#[test]
fn how_often_an_app_needed_pixels_is_kept() {
    let dir = std::env::temp_dir().join(format!("cu-apps-engine-{}", std::process::id()));
    let path = dir.join("apps.json");
    let _ = std::fs::remove_file(&path);
    {
        let mut e = smart_engine().with_apps_log(path.clone());
        state_of(&mut e, serde_json::json!({}));
        count_to(&mut e, 1);
        state_of(&mut e, serde_json::json!({}));
    }
    let text = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let r = &v["apps"]["TextEdit"];
    assert_eq!(r["looks"], 2, "{text}");
    assert_eq!(r["pictures"], 1, "{text}");
    assert_eq!(r["pictures_left_out"], 1, "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn text_read_inside_a_private_field_is_left_out_of_the_tree() {
    let win = Rect::new(0.0, 0.0, 400.0, 300.0);
    let mut elements = vec![
        MockElement::new(1, "window", "Login", win),
        // A field labelled like a secret that exposes no value.
        MockElement::new(
            2,
            "text field",
            "Verification code",
            Rect::new(20.0, 20.0, 200.0, 30.0),
        )
        .child_of(1)
        .editable(),
    ];
    for e in &mut elements {
        e.states.enabled = true;
    }
    let app = MockApp {
        info: AppInfo {
            name: "Bank".into(),
            id: "bank".into(),
            pid: 91,
            exe: None,
            frontmost: true,
            hidden: false,
        },
        windows: vec![MockWindow {
            id: 3,
            title: "Login".into(),
            bounds: win,
            root: 1,
            focused: true,
        }],
        elements,
    };
    let mut backend = MockBackend::new();
    backend.add_app(app);
    backend.ocr_text = Some(vec![
        line("482913", 25.0, 25.0),
        line("Welcome back", 20.0, 200.0),
    ]);
    let mut cfg = Config::default();
    cfg.ocr.tesseract_path = "/nonexistent/tesseract".into();
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Bank"}));
    assert!(!out.text.contains("482913"), "{}", out.text);
    assert!(out.text.contains("Welcome back"), "{}", out.text);
}

#[test]
fn a_lone_digit_read_unsurely_is_not_text() {
    let mut e = canvas_engine(Some(vec![
        // A text-sized box: only the confidence tells them apart.
        OcrLine {
            bounds: Rect::new(100.0, 100.0, 14.0, 20.0),
            confidence: 0.85,
            ..line("7", 0.0, 0.0)
        },
        OcrLine {
            bounds: Rect::new(200.0, 100.0, 14.0, 20.0),
            confidence: 0.95,
            ..line("8", 0.0, 0.0)
        },
    ]));
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
    assert!(!out.text.contains("ocr text \"7\""), "{}", out.text);
    assert!(out.text.contains("ocr text \"8\""), "{}", out.text);
}

#[test]
fn an_element_click_after_the_window_moved_lands_where_it_is_now() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let bold = index_of_name(&out.text, "\"Bold\"");
    let before = e
        .backend_mut()
        .app_mut(4242)
        .unwrap()
        .elements
        .iter()
        .find(|el| el.name.as_deref() == Some("Bold"))
        .unwrap()
        .bounds;
    let moved = e.call_tool(
        "window",
        serde_json::json!({"app": "TextEdit", "action": "move", "x": 300, "y": 200}),
    );
    assert!(!moved.is_error, "{}", moved.text);
    let now = e
        .backend_mut()
        .app_mut(4242)
        .unwrap()
        .elements
        .iter()
        .find(|el| el.name.as_deref() == Some("Bold"))
        .unwrap()
        .bounds;
    assert_ne!(before.x, now.x);
    let c = e.call_tool(
        "click",
        serde_json::json!({"app": "TextEdit", "element_index": bold, "button": "right"}),
    );
    assert!(!c.is_error, "{}", c.text);
    let at = e
        .backend()
        .events
        .iter()
        .rev()
        .find_map(|ev| match ev {
            Event::Click(_, p, ..) => Some(*p),
            _ => None,
        })
        .unwrap();
    assert!(
        now.contains(at),
        "clicked at {at:?}, the button is at {now:?}"
    );
}

#[test]
fn the_audit_log_keeps_no_value_that_was_set() {
    let dir = std::env::temp_dir().join(format!("cu-audit-values-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("audit.log");
    let mut e = engine();
    let mut cfg = e.store().config.clone();
    cfg.audit.enabled = true;
    cfg.tree.compact = false;
    cfg.audit.path = Some(path.clone());
    e.set_config(ConfigStore::in_memory(cfg));
    let out = state_of(&mut e, serde_json::json!({}));
    let doc = index_of_name(&out.text, "\"Document\"");
    let set = e.call_tool(
        "set_value",
        serde_json::json!({"app": "TextEdit", "element_index": doc, "value": "s3cret-PIN-9911"}),
    );
    assert!(!set.is_error, "{}", set.text);
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(!log.contains("s3cret"), "{log}");
    assert!(log.contains("\"tool\":\"set_value\""), "{log}");
    // Nothing from the first quote on: a quote inside a value can't flip
    // what is kept.
    assert_eq!(
        super::audit_summary("Set text field \"Notes\" to \"my \"secret phrase\" here\"."),
        "Set text field …"
    );
    // Nor what an `expect` said it waited for.
    assert_eq!(
        super::audit_summary("Typed 5 character(s). Expected Welcome back Jane: confirmed."),
        "Typed 5 character(s).…"
    );
    assert_eq!(
        super::audit_summary("Sent. Code 482913 is valid."),
        "Sent. Code •••••• is valid."
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_look_that_never_arrived_leaves_the_full_header_for_the_next() {
    let mut e = engine();
    let shown = e.shown();
    let first = state_of(&mut e, serde_json::json!({}));
    assert!(first.text.contains("pid 4242"), "{}", first.text);
    e.not_delivered(shown);
    let again = state_of(&mut e, serde_json::json!({}));
    assert!(again.text.contains("pid 4242"), "{}", again.text);
}

#[test]
fn a_zoom_on_the_last_column_of_the_screenshot_is_taken() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({"screenshot": true}));
    let size = out
        .text
        .split("Screenshot #")
        .nth(1)
        .and_then(|t| t.split(':').nth(1))
        .and_then(|t| t.split_whitespace().next())
        .unwrap()
        .to_string();
    let w: f64 = size.split_once('x').unwrap().0.parse().unwrap();
    let z = e.call_tool(
        "screenshot",
        serde_json::json!({"app": "TextEdit", "zoom": [w - 0.5, 100]}),
    );
    assert!(!z.is_error, "{}", z.text);
}

#[test]
fn a_value_that_is_not_a_finite_number_is_compared_as_text() {
    let mut e = engine();
    let out = state_of(&mut e, serde_json::json!({}));
    let doc = index_of_name(&out.text, "\"Document\"");
    for value in ["NaN", "inf", "007"] {
        let set = e.call_tool(
            "set_value",
            serde_json::json!({"app": "TextEdit", "element_index": doc, "value": value}),
        );
        assert!(!set.is_error, "{value}: {}", set.text);
        assert!(!set.text.contains("now shows"), "{value}: {}", set.text);
    }
}

#[test]
fn text_read_off_a_window_moves_with_it() {
    let mut e = canvas_engine(Some(vec![
        line("New Game", 100.0, 100.0),
        line("Options", 100.0, 140.0),
    ]));
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Game"}));
    let options = index_of_name(&out.text, "\"Options\"");
    let runs = e.backend().ocr_runs;
    let moved = e.call_tool(
        "window",
        serde_json::json!({"app": "Game", "action": "move", "x": 300, "y": 200}),
    );
    assert!(!moved.is_error, "{}", moved.text);
    let clicked = |e: &Engine<MockBackend>| {
        e.backend().events.iter().rev().find_map(|ev| match ev {
            Event::Click(_, p, ..) => Some(*p),
            _ => None,
        })
    };
    // Right after the move: never where the text was.
    e.call_tool(
        "click",
        serde_json::json!({"app": "Game", "element_index": options}),
    );
    assert_ne!(clicked(&e), Some(Point::new(140.0, 150.0)));
    // The same picture: not read again, but where the window is now.
    let out = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": "Game", "disable_diff": true}),
    );
    let options = index_of_name(&out.text, "\"Options\"");
    let r = e.call_tool(
        "click",
        serde_json::json!({"app": "Game", "element_index": options}),
    );
    assert!(!r.is_error, "{}", r.text);
    assert_eq!(clicked(&e), Some(Point::new(440.0, 350.0)));
    assert_eq!(e.backend().ocr_runs, runs);
}

#[test]
fn text_read_inside_a_private_field_stays_out_after_the_window_moved() {
    let win = Rect::new(0.0, 0.0, 400.0, 300.0);
    let mut field = MockElement::new(
        2,
        "text field",
        "Verification code",
        Rect::new(20.0, 20.0, 200.0, 30.0),
    )
    .child_of(1)
    .editable();
    field.states.enabled = true;
    let app = MockApp {
        info: AppInfo {
            name: "Bank".into(),
            id: "bank".into(),
            pid: 91,
            exe: None,
            frontmost: true,
            hidden: false,
        },
        windows: vec![MockWindow {
            id: 3,
            title: "Login".into(),
            bounds: win,
            root: 1,
            focused: true,
        }],
        elements: vec![MockElement::new(1, "window", "Login", win), field],
    };
    let mut backend = MockBackend::new();
    backend.add_app(app);
    backend.ocr_text = Some(vec![
        line("482913", 25.0, 25.0),
        line("Welcome back", 20.0, 200.0),
    ]);
    let mut cfg = Config::default();
    cfg.ocr.tesseract_path = "/nonexistent/tesseract".into();
    cfg.cache.snapshot_ttl_ms = 0;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(Instant::now, |_| {});
    let out = e.call_tool("get_app_state", serde_json::json!({"app": "Bank"}));
    assert!(!out.text.contains("482913"), "{}", out.text);
    let moved = e.call_tool(
        "window",
        serde_json::json!({"app": "Bank", "action": "move", "x": 500, "y": 300}),
    );
    assert!(!moved.is_error, "{}", moved.text);
    let out = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": "Bank", "disable_diff": true}),
    );
    assert!(out.text.contains("Welcome back"), "{}", out.text);
    assert!(!out.text.contains("482913"), "{}", out.text);
}

#[test]
fn a_hub_change_from_the_embedder_leaves_the_hub() {
    let (port, home) = test_hub("set-config");
    let mut e = hub_engine(port, &home, false);
    e.call_tool("list_apps", serde_json::json!({}));
    assert!(e.hub_link().is_some());
    let mut cfg = e.store().config.clone();
    cfg.hub.enabled = false;
    e.set_config(ConfigStore::in_memory(cfg));
    assert!(e.hub_link().is_none());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_design_drawn_in_a_batch_is_not_counted_as_seen() {
    let mut e = engine();
    let out = e.call_tool(
        "design",
        serde_json::json!({"name": "card", "size": [400, 200], "background": "#FFFFFF",
            "add": [{"id": "disc", "ellipse": [100, 100, 60, 60], "fill": "#CC2222"}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    let green = serde_json::json!({"name": "card", "change": [{"id": "disc", "fill": "#00FF00"}]});
    let out = e.call_tool(
        "batch",
        serde_json::json!({"steps": [
            {"tool": "design", "arguments": green},
            {"tool": "list_apps", "arguments": {}}]}),
    );
    assert!(!out.is_error, "{}", out.text);
    // The model last got it red: the change is said, the picture sent.
    let out = e.call_tool("design", green);
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("#00FF00"), "{}", out.text);
    assert!(out.text.contains("changed disc"), "{}", out.text);
    assert!(
        !out.text.contains("The picture is as before"),
        "{}",
        out.text
    );
    assert!(out.image.is_some(), "{}", out.text);
}

#[test]
fn the_agent_never_acts_on_the_settings_panel() {
    let mut backend = MockBackend::new();
    let mut app = MockBackend::text_editor(4242);
    app.windows[0].title = "Zero panel [private] - Browser".into();
    backend.add_app(app);
    let mut e = Engine::new(backend, ConfigStore::in_memory(Config::default()))
        .with_time(Instant::now, |_| {});

    // Looking, acting and pictures are all refused, by name or by default.
    for call in [
        ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            ..Default::default()
        }),
        ToolCall::GetAppState(GetAppStateArgs {
            app: "TextEdit".into(),
            window: Some("Zero panel".into()),
            ..Default::default()
        }),
    ] {
        let err = e.call(call).unwrap_err().to_string();
        assert!(err.contains("settings panel"), "{err}");
    }
    assert!(
        e.backend()
            .events
            .iter()
            .all(|ev| !matches!(ev, Event::Click(..) | Event::Key(..) | Event::Type(..))),
        "{:?}",
        e.backend().events
    );
}
