//! Live end-to-end test of the Linux backend + engine against a real GTK app.
//!
//! It only runs when `COMPUTER_USE_LIVE=1` (the harness in
//! `tests/run_live_linux.sh` sets up Xvfb, D-Bus and AT-SPI and starts the
//! GTK fixture first). Everything else is a no-op so `cargo test` stays green
//! in environments without a display.

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use computer_use::Backend;
use computer_use::config::{Config, ConfigStore};
use computer_use::engine::Engine;
use computer_use::linux::LinuxBackend;
use computer_use::tools::ToolCall;

fn live() -> bool {
    std::env::var("COMPUTER_USE_LIVE").as_deref() == Ok("1")
}

fn engine() -> Engine<LinuxBackend> {
    let backend = LinuxBackend::new().expect("connect to AT-SPI/X11");
    let cfg = Config::default();
    Engine::new(backend, ConfigStore::in_memory(cfg))
}

/// Find our GTK fixture's app id by scanning windows for the "CU Test" title.
fn wait_for_app(e: &mut Engine<LinuxBackend>) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(apps) = e.backend_mut().list_apps() {
            for app in apps {
                if let Ok(windows) = e.backend_mut().list_windows(&app)
                    && windows.iter().any(|w| w.title.contains("CU Test"))
                {
                    return app.id;
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
        .call(ToolCall::GetAppState(
            computer_use::tools::GetAppStateArgs {
                app: app.into(),
                window: Some("CU Test".into()),
                disable_diff: true,
                ..Default::default()
            },
        ))
        .expect("get_app_state");
    assert!(!out.is_error, "get_app_state errored: {}", out.text);
    out.text
}

fn index_of(tree: &str, needle: &str) -> u32 {
    // Records (look-alike siblings on one line) read back one a line.
    computer_use::tree::index_of(tree, needle)
        .unwrap_or_else(|| panic!("no element matching `{needle}` in tree:\n{tree}"))
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
        .call(ToolCall::GetAppState(
            computer_use::tools::GetAppStateArgs {
                app: app.clone(),
                window: Some("CU Test".into()),
                disable_diff: true,
                ..Default::default()
            },
        ))
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
        .call(ToolCall::Click(computer_use::tools::ClickArgs {
            app: app.clone(),
            element_index: Some(button),
            ..Default::default()
        }))
        .unwrap();
    assert!(!out.is_error, "click failed: {}", out.text);
    std::thread::sleep(Duration::from_millis(150));
    let tree = state_text(&mut e, &app);
    assert!(tree.contains("clicked"), "status did not update:\n{tree}");

    // 3. set_value into the entry; the status label echoes it.
    let entry = index_of(&tree, "text field");
    let out = e
        .call(ToolCall::SetValue(computer_use::tools::SetValueArgs {
            app: app.clone(),
            element_index: entry,
            value: "hello atspi".into(),
            ..Default::default()
        }))
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
        .call(ToolCall::SetValue(computer_use::tools::SetValueArgs {
            app: app.clone(),
            element_index: check,
            value: "true".into(),
            ..Default::default()
        }))
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
        .call(ToolCall::PressKey(computer_use::tools::PressKeyArgs {
            app: app.clone(),
            key: "ctrl+a".into(),
            element_index: Some(entry),
            ..Default::default()
        }))
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
    );
    assert!(!out.is_error, "find_element: {}", out.text);
    assert!(out.text.contains("Click Me"), "find_element:\n{}", out.text);

    // wait_for: the entry is already present.
    let out = e.call_tool(
        "wait_for",
        serde_json::json!({"app": app, "role": "text field", "timeout_ms": 3000}),
    );
    assert!(!out.is_error, "wait_for: {}", out.text);

    // screenshot: full screen and a region (X11 GetImage path).
    let out = e.call_tool("screenshot", serde_json::json!({"mode": "full"}));
    assert!(out.image.is_some(), "full screenshot: {}", out.text);
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "region", "x": 0, "y": 0, "width": 200, "height": 120}),
    );
    assert!(out.image.is_some(), "region screenshot: {}", out.text);
    let img = out.image.unwrap();
    assert!(img.width > 0 && img.height > 0);

    // annotated window screenshot (set-of-marks overlay).
    let out = e.call_tool(
        "screenshot",
        serde_json::json!({"mode": "window", "app": app, "window": "CU Test", "annotate": true}),
    );
    assert!(out.image.is_some(), "annotated screenshot: {}", out.text);

    // clipboard round-trip via xclip (installed by the harness).
    let out = e.call_tool(
        "set_clipboard",
        serde_json::json!({"text": "computer-use clip"}),
    );
    assert!(!out.is_error, "set_clipboard: {}", out.text);
    let out = e.call_tool("get_clipboard", serde_json::json!({}));
    assert!(
        out.text.contains("computer-use clip"),
        "clipboard round-trip:\n{}",
        out.text
    );

    eprintln!("new-tools live test passed");
}

fn click_named(e: &mut Engine<LinuxBackend>, app: &str, tree: &str, name: &str) -> String {
    let index = index_of(tree, name);
    let out = e.call_tool(
        "click",
        serde_json::json!({"app": app, "element_index": index}),
    );
    assert!(!out.is_error, "click {name}: {}", out.text);
    out.text
}

#[test]
fn screen_memory_over_real_backend() {
    if !live() {
        return;
    }
    // The tree's screen memory; with blind areas on, the canvas's pixels
    // are checked on every look and the page transition shows in them.
    let mut e = engine();
    let mut cfg = e.store().config.clone();
    cfg.ocr.blind_regions = false;
    e.set_config(ConfigStore::in_memory(cfg));
    let app = wait_for_app(&mut e);

    let first = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": app, "window": "CU Test"}),
    );
    assert!(!first.is_error, "{}", first.text);
    let tree = first.text.clone();
    let click_me = index_of(&tree, "Click Me");

    // Another page of the same window: a new screen.
    let out = click_named(&mut e, &app, &tree, "Next page");
    std::thread::sleep(Duration::from_millis(150));
    eprintln!("--- after Next page ---\n{out}");
    assert!(out.contains("(new)"), "expected a new screen:\n{out}");
    assert!(out.contains("Detail line"), "page 2 not shown:\n{out}");

    // Back: recognised, nothing to re-read.
    let out = click_named(&mut e, &app, &out, "Back");
    eprintln!("--- after Back ---\n{out}");
    assert!(out.contains("(seen before)"), "not recognised:\n{out}");
    let t = Instant::now();
    let again = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": app, "window": "CU Test"}),
    );
    eprintln!(
        "--- get_app_state after Back ({:?}) ---\n{}",
        t.elapsed(),
        again.text
    );
    assert!(again.image.is_none(), "screenshot re-sent:\n{}", again.text);
    assert!(
        !again.text.contains("Click Me"),
        "tree re-sent:\n{}",
        again.text
    );
    let found = e.call_tool(
        "find_element",
        serde_json::json!({"app": app, "name": "Click Me"}),
    );
    assert_eq!(
        index_of(&found.text, "Click Me"),
        click_me,
        "{}",
        found.text
    );

    // A modal dialog, then back to the main window.
    let out = click_named(&mut e, &app, &tree, "Open dialog");
    std::thread::sleep(Duration::from_millis(300));
    eprintln!("--- after Open dialog ---\n{out}");
    assert!(
        out.contains("now on screen") && out.contains("CU Dialog") && out.contains("button \"OK\""),
        "the report didn't follow the dialog:\n{out}"
    );
    let out = click_named(&mut e, &app, &out, "OK");
    std::thread::sleep(Duration::from_millis(300));
    eprintln!("--- after OK ---\n{out}");
    assert!(
        out.contains("back on screen #1 (seen before), window \"CU Test\""),
        "main window not recognised after the dialog:\n{out}"
    );
    let found = e.call_tool(
        "find_element",
        serde_json::json!({"app": app, "name": "Click Me"}),
    );
    assert_eq!(
        index_of(&found.text, "Click Me"),
        click_me,
        "{}",
        found.text
    );
    eprintln!("screen-memory live test passed");
}

/// The overlay (run from the `computer-use-mcp` binary named by
/// `COMPUTER_USE_OVERLAY_BIN`) shows while the engine works, but is never in
/// the engine's own screenshots, and synthesized clicks leave the user's
/// mouse where it was.
#[test]
fn overlay_is_left_out_of_screenshots_and_the_mouse_stays_put() {
    if !live() {
        return;
    }
    let Some(bin) = std::env::var_os("COMPUTER_USE_OVERLAY_BIN") else {
        eprintln!("skipping: COMPUTER_USE_OVERLAY_BIN not set");
        return;
    };
    let mut e = engine().with_overlay(computer_use::overlay::Launcher::helper(bin));
    let app = wait_for_app(&mut e);

    // The screen glow (blue while working) runs along the left screen edge,
    // over the fixture window at x = 0; the engine's screenshot must not show it.
    let out = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": app, "window": "CU Test", "screenshot": true}),
    );
    assert!(!out.is_error, "{}", out.text);
    std::thread::sleep(Duration::from_millis(400));
    let img = image::load_from_memory(&out.image.expect("screenshot").data)
        .expect("decode")
        .to_rgb8();
    let px = img.get_pixel(2, img.height() / 2).0;
    let blue = px[2] > 150 && px[0] < 120;
    let gold = px[0] > 150 && px[2] < 80;
    assert!(!blue && !gold, "overlay captured in the screenshot: {px:?}");

    // A coordinate click puts the real pointer back.
    let (conn, screen) = x11rb::connect(None).expect("X");
    use x11rb::protocol::xproto::ConnectionExt as _;
    let root = x11rb::connection::Connection::setup(&conn).roots[screen].root;
    let pointer = |c: &x11rb::rust_connection::RustConnection| {
        let r = c.query_pointer(root).unwrap().reply().unwrap();
        (r.root_x, r.root_y)
    };
    let before = pointer(&conn);
    let out = e.call_tool("click", serde_json::json!({"app": app, "x": 200, "y": 200}));
    assert!(!out.is_error, "{}", out.text);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(pointer(&conn), before, "the user's mouse was moved");
    eprintln!("overlay live test passed");
}

/// Press a key combination through XTest, as a user would on the keyboard.
fn press_keys(keysyms: &[u32]) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::protocol::xtest::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None).expect("X");
    let root = conn.setup().roots[screen].root;
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .unwrap()
        .reply()
        .unwrap();
    let per = usize::from(map.keysyms_per_keycode);
    let code = |sym: u32| {
        min + map
            .keysyms
            .chunks(per)
            .position(|c| c.contains(&sym))
            .expect("keysym on keyboard") as u8
    };
    let codes: Vec<u8> = keysyms.iter().map(|s| code(*s)).collect();
    // Like a person: one key at a time; and the server must have handled
    // the events before this connection closes (or it drops them).
    let events = codes
        .iter()
        .map(|c| (2u8, *c))
        .chain(codes.iter().rev().map(|c| (3u8, *c)));
    for (kind, c) in events {
        conn.xtest_fake_input(kind, c, 0, root, 0, 0, 0).unwrap();
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(15));
    }
    conn.get_input_focus().unwrap().reply().unwrap();
}

#[test]
fn stop_key_and_idle_time_over_x11() {
    if !live() {
        return;
    }
    let Some(bin) = std::env::var_os("COMPUTER_USE_OVERLAY_BIN") else {
        eprintln!("skipping: COMPUTER_USE_OVERLAY_BIN not set");
        return;
    };
    let mut e = engine().with_overlay(computer_use::overlay::Launcher::helper(bin));
    // The helper starts listening for Ctrl+Alt+Esc (a debug build takes a
    // moment to start and load its fonts).
    e.arm();
    std::thread::sleep(Duration::from_millis(2500));
    const CTRL: u32 = 0xffe3;
    const ALT: u32 = 0xffe9;
    const ESC: u32 = 0xff1b;
    let wait_until = |e: &Engine<LinuxBackend>, stopped: bool| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while e.is_stopped() != stopped && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        e.is_stopped() == stopped
    };
    press_keys(&[CTRL, ALT, ESC]);
    assert!(wait_until(&e, true), "the stop key did not stop the agent");
    let out = e.call_tool("list_apps", serde_json::json!({}));
    assert!(
        out.is_error && out.text.contains("Ctrl+Alt+Esc"),
        "{}",
        out.text
    );
    std::thread::sleep(Duration::from_millis(500));
    press_keys(&[CTRL, ALT, ESC]);
    assert!(wait_until(&e, false), "pressing it again did not resume");
    let out = e.call_tool("list_apps", serde_json::json!({}));
    assert!(!out.is_error, "{}", out.text);

    // The key presses count as input: the idle time is short now.
    let idle = e
        .backend_mut()
        .user_idle()
        .expect("MIT-SCREEN-SAVER idle time");
    assert!(idle < Duration::from_secs(5), "{idle:?}");

    // The "user" just typed: an action waits until they have been idle for
    // control.resume_after_idle_ms (1.5 s), then runs.
    let app = wait_for_app(&mut e);
    state_text(&mut e, &app);
    press_keys(&[0xffe1]); // Shift
    let t = Instant::now();
    let out = e.call_tool("press_key", serde_json::json!({"app": app, "key": "Tab"}));
    assert!(!out.is_error, "{}", out.text);
    let waited = t.elapsed();
    assert!(
        waited >= Duration::from_millis(1200),
        "acted while the user was typing ({waited:?})"
    );
    eprintln!("stop key live test passed (idle {idle:?}, paused {waited:?})");
}

/// Keys meant for an app never land in another window: a window in front
/// that is no accessible app's (a terminal) is listed as frontmost, so the
/// app is brought forward first. And they type what was asked whatever
/// keyboard layout and Caps Lock the user has on, which are left as they
/// were.
#[test]
fn keys_reach_the_app_whatever_window_and_layout_are_active() {
    if !live() {
        return;
    }
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xkb::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, ModMask, PropMode,
        WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as _;

    let mut e = engine();
    let app = wait_for_app(&mut e);
    let tree = state_text(&mut e, &app);
    let entry = index_of(&tree, "text field");
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": app, "text": "x", "element_index": entry}),
    );
    assert!(!out.is_error, "{}", out.text);

    // A "terminal": a window of this test, which isn't on the
    // accessibility bus, takes the keyboard.
    let (conn, screen) = x11rb::connect(None).expect("X");
    let root = conn.setup().roots[screen].root;
    let win = conn.generate_id().unwrap();
    conn.create_window(
        0,
        win,
        root,
        600,
        500,
        300,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(EventMask::KEY_PRESS),
    )
    .unwrap();
    let wm_pid = conn
        .intern_atom(false, b"_NET_WM_PID")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    conn.change_property32(
        PropMode::REPLACE,
        win,
        wm_pid,
        AtomEnum::CARDINAL,
        &[std::process::id()],
    )
    .unwrap();
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        b"faketerm\0FakeTerm\0",
    )
    .unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)
        .unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    let out = e.call_tool("list_apps", serde_json::json!({}));
    let front: Vec<&str> = out
        .text
        .lines()
        .filter(|l| l.contains("frontmost"))
        .collect();
    assert!(
        front.len() == 1 && front[0].contains("FakeTerm"),
        "{}",
        out.text
    );

    // The user has a second (Russian) layout on for a and b, and Caps Lock.
    conn.xkb_use_extension(1, 0).unwrap().reply().unwrap();
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .unwrap()
        .reply()
        .unwrap();
    let per = usize::from(map.keysyms_per_keycode);
    let code_of = |sym: u32| min + map.keysyms.chunks(per).position(|c| c[0] == sym).unwrap() as u8;
    let (a, b) = (code_of(0x61), code_of(0x62));
    let row = |code: u8| map.keysyms[usize::from(code - min) * per..][..per].to_vec();
    let originals = [(a, row(a)), (b, row(b))];
    conn.change_keyboard_mapping(1, a, 4, &[0x61, 0x41, 0x6c6, 0x6e6])
        .unwrap();
    conn.change_keyboard_mapping(1, b, 4, &[0x62, 0x42, 0x6c9, 0x6e9])
        .unwrap();
    let kbd = xkb::ID::USE_CORE_KBD.into();
    let lock = |group: xkb::Group, caps: bool| {
        let on = if caps {
            ModMask::LOCK
        } else {
            ModMask::from(0u16)
        };
        conn.xkb_latch_lock_state(
            kbd,
            ModMask::LOCK,
            on,
            true,
            group,
            ModMask::from(0u16),
            false,
            0,
        )
        .unwrap();
        conn.get_input_focus().unwrap().reply().unwrap();
    };
    lock(xkb::Group::M2, true);
    std::thread::sleep(Duration::from_millis(200));

    let out = e.call_tool("type_text", serde_json::json!({"app": app, "text": "aB"}));
    std::thread::sleep(Duration::from_millis(200));
    let st = conn.xkb_get_state(kbd).unwrap().reply().unwrap();
    lock(xkb::Group::M1, false);
    for (code, syms) in &originals {
        conn.change_keyboard_mapping(1, *code, per as u8, syms)
            .unwrap();
    }
    conn.destroy_window(win).unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    assert!(!out.is_error, "{}", out.text);
    let mut stray = 0;
    while let Some(ev) = conn.poll_for_event().unwrap() {
        if let Event::KeyPress(_) = ev {
            stray += 1;
        }
    }
    assert_eq!(stray, 0, "keys went to the window in front, not the app");
    assert_eq!(st.locked_group, xkb::Group::M2, "the user's layout");
    assert_ne!(
        st.locked_mods & ModMask::LOCK,
        ModMask::from(0u16),
        "the user's Caps Lock"
    );
    let tree = state_text(&mut e, &app);
    assert!(tree.contains("entry: xaB"), "typed:\n{tree}");
    // Clear the entry for the other tests.
    let entry = index_of(&tree, "text field");
    e.call_tool(
        "set_value",
        serde_json::json!({"app": app, "element_index": entry, "value": ""}),
    );
    eprintln!("keys-to-the-app live test passed");
}

#[test]
fn follow_up_screenshots_send_only_what_changed() {
    if !live() {
        return;
    }
    let backend = LinuxBackend::new().expect("connect to AT-SPI/X11");
    let mut cfg = Config::default();
    cfg.screenshot.attach = computer_use::config::AttachMode::Always;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg));
    let app = wait_for_app(&mut e);
    // Park the pointer off the window: hover highlights are real changes too.
    {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;
        let (conn, screen) = x11rb::connect(None).expect("X");
        let root = conn.setup().roots[screen].root;
        conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, 1200, 1000)
            .unwrap();
        conn.get_input_focus().unwrap().reply().unwrap();
        std::thread::sleep(Duration::from_millis(200));
    }
    let state = |e: &mut Engine<LinuxBackend>| {
        e.call_tool(
            "get_app_state",
            serde_json::json!({"app": app, "window": "CU Test", "disable_diff": true}),
        )
    };
    let out = state(&mut e);
    let save = |name: &str, out: &computer_use::ToolOutput| {
        if let (Some(img), Ok(dir)) = (&out.image, std::env::var("CU_SHOT_DIR")) {
            std::fs::write(format!("{dir}/{name}.png"), &img.data).ok();
        }
    };
    save("before", &out);
    let full = out.image.as_ref().expect("first screenshot").width;
    // Tick the checkbox: a small part of the window changes.
    let check = index_of(&out.text, "Enable feature");
    let r = e.call_tool(
        "click",
        serde_json::json!({"app": app, "element_index": check}),
    );
    assert!(!r.is_error, "{}", r.text);
    let out = state(&mut e);
    save("after", &out);
    eprintln!(
        "{}",
        out.text.lines().take(3).collect::<Vec<_>>().join("\n")
    );
    let part = out.image.as_ref().expect("a screenshot");
    assert!(
        out.text.contains("only the part that changed") && part.width < full,
        "{}",
        out.text
    );
    // Put it back.
    let check = index_of(&out.text, "Enable feature");
    e.call_tool(
        "click",
        serde_json::json!({"app": app, "element_index": check}),
    );
    eprintln!(
        "smart screenshot live test passed ({full} px wide → {} px)",
        part.width
    );
}

#[test]
fn window_management_over_x11() {
    if !live() {
        return;
    }
    let mut e = engine();
    let app = wait_for_app(&mut e);
    let win = |e: &mut Engine<LinuxBackend>, extra: serde_json::Value| {
        let mut args = serde_json::json!({"app": app, "window": "CU Test"});
        for (k, v) in extra.as_object().unwrap() {
            args[k] = v.clone();
        }
        let out = e.call_tool("window", args);
        eprintln!("{}", out.text.trim_end());
        out
    };
    let out = e.call_tool("window", serde_json::json!({"action": "displays"}));
    assert!(
        out.text.contains("display 0 (primary): 1280x1024 at 0,0"),
        "{}",
        out.text
    );

    let out = win(
        &mut e,
        serde_json::json!({"action": "move", "x": 200, "y": 150}),
    );
    assert!(out.text.contains("at 200,150"), "{}", out.text);
    let out = win(
        &mut e,
        serde_json::json!({"action": "resize", "width": 520, "height": 380}),
    );
    assert!(out.text.contains("520x380 at 200,150"), "{}", out.text);
    let out = win(&mut e, serde_json::json!({"action": "tile_right"}));
    assert!(out.text.contains("640x1024 at 640,0"), "{}", out.text);
    let out = win(&mut e, serde_json::json!({"action": "minimize"}));
    assert!(!out.is_error, "{}", out.text);
    let out = win(&mut e, serde_json::json!({"action": "restore"}));
    assert!(!out.is_error, "{}", out.text);
    // Back where the other tests expect it.
    let out = win(
        &mut e,
        serde_json::json!({"action": "move", "x": 0, "y": 0, "width": 420, "height": 320}),
    );
    assert!(out.text.contains("420x320 at 0,0"), "{}", out.text);
    let out = win(&mut e, serde_json::json!({"action": "focus"}));
    assert!(!out.is_error, "{}", out.text);
    eprintln!("window management live test passed");
}

#[test]
fn ocr_reads_and_clicks_custom_drawn_text() {
    if !live() {
        return;
    }
    if std::process::Command::new("tesseract")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: tesseract not installed");
        return;
    }
    let mut e = engine();
    let app = wait_for_app(&mut e);
    let out = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": app, "window": "CU Test", "ocr": true, "disable_diff": true}),
    );
    assert!(!out.is_error, "{}", out.text);
    if let (Some(img), Ok(dir)) = (&out.image, std::env::var("CU_SHOT_DIR")) {
        std::fs::write(format!("{dir}/ocr-window.png"), &img.data).ok();
    }
    let line = out
        .text
        .lines()
        .find(|l| l.contains("ocr text") && l.contains("1234"))
        .unwrap_or_else(|| panic!("canvas text not read:\n{}", out.text));
    eprintln!("read: {}", line.trim());
    // What the tree already says is not repeated.
    assert!(!out.text.contains("ocr text \"Click Me\""), "{}", out.text);
    let index = index_of(&out.text, "1234");
    let r = e.call_tool(
        "click",
        serde_json::json!({"app": app, "window": "CU Test", "element_index": index}),
    );
    assert!(!r.is_error, "{}", r.text);
    let tree = state_text(&mut e, &app);
    assert!(tree.contains("canvas clicked"), "{tree}");
    eprintln!("OCR live test passed");
}

#[test]
fn notifications_are_heard_on_the_session_bus() {
    if !live() {
        return;
    }
    let backend = LinuxBackend::new().expect("connect to AT-SPI/X11");
    let mut cfg = Config::default();
    cfg.notifications.enabled = true;
    let mut e = Engine::new(backend, ConfigStore::in_memory(cfg));
    std::thread::sleep(Duration::from_millis(500));
    // Stand in for the notification server, so the call has somewhere to go.
    let server = zbus::blocking::Connection::session().expect("session bus");
    server
        .request_name("org.freedesktop.Notifications")
        .expect("own the notification service name");
    let sent = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--timeout",
            "1",
            "--dest",
            "org.freedesktop.Notifications",
            "--object-path",
            "/org/freedesktop/Notifications",
            "--method",
            "org.freedesktop.Notifications.Notify",
            "Chat",
            "uint32 0",
            "",
            "Hello from <b>Ada</b>",
            "Your verification code is 482913",
            "@as []",
            "@a{sv} {}",
            "int32 5000",
        ])
        .output()
        .expect("gdbus");
    eprintln!("gdbus: {}", String::from_utf8_lossy(&sent.stderr).trim());
    let deadline = Instant::now() + Duration::from_secs(5);
    let out = loop {
        let out = e.call_tool("get_notifications", serde_json::json!({}));
        if out.text.contains("Hello") || Instant::now() > deadline {
            break out;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    eprintln!("{}", out.text.trim_end());
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.contains("Chat — Hello from Ada"), "{}", out.text);
    assert!(
        out.text.contains("••••••") && !out.text.contains("482913"),
        "{}",
        out.text
    );
    eprintln!("notifications live test passed");
}
