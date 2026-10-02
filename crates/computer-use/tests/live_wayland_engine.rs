//! Live end-to-end test of the engine in a Wayland session (headless sway:
//! the wlroots protocols Hyprland speaks too) against the GTK fixture
//! running as a native Wayland client:
//!
//! ```text
//! APPS="python3.12 crates/computer-use/tests/fixtures/gtk_app.py" \
//!   scripts/wayland-session.sh env COMPUTER_USE_LIVE_WAYLAND=1 \
//!   cargo test -p computer-use --test live_wayland_engine -- --nocapture
//! ```
//!
//! Without `COMPUTER_USE_LIVE_WAYLAND=1` it does nothing.

#![cfg(target_os = "linux")]

use std::process::Command;
use std::time::{Duration, Instant};

use computer_use::Backend;
use computer_use::config::{Config, ConfigStore};
use computer_use::engine::Engine;
use computer_use::linux::LinuxBackend;
use computer_use::tools::{GetAppStateArgs, ToolCall};
use computer_use::types::{AppInfo, InputTarget, MouseButton, Point, SnapshotOptions, WindowInfo};

fn live() -> bool {
    std::env::var("COMPUTER_USE_LIVE_WAYLAND").as_deref() == Ok("1")
}

fn swaymsg(args: &[&str]) -> String {
    let out = Command::new("swaymsg")
        .args(args)
        .output()
        .expect("swaymsg");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Where sway has the fixture's window content: (x, y, w, h).
fn sway_rect() -> (f64, f64, f64, f64) {
    let tree: serde_json::Value =
        serde_json::from_str(&swaymsg(&["-t", "get_tree"])).expect("sway tree");
    fn find(n: &serde_json::Value) -> Option<(f64, f64, f64, f64)> {
        if n["name"] == "CU Test" {
            let r = &n["rect"];
            let w = &n["window_rect"];
            let f = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0);
            return Some((
                f(&r["x"]) + f(&w["x"]),
                f(&r["y"]) + f(&w["y"]),
                f(&w["width"]),
                f(&w["height"]),
            ));
        }
        for key in ["nodes", "floating_nodes"] {
            if let Some(kids) = n[key].as_array() {
                for k in kids {
                    if let Some(r) = find(k) {
                        return Some(r);
                    }
                }
            }
        }
        None
    }
    find(&tree).expect("the fixture's window in sway's tree")
}

fn engine() -> Engine<LinuxBackend> {
    let backend = LinuxBackend::new().expect("connect to AT-SPI");
    let mut cfg = Config::default();
    cfg.overlay.enabled = false;
    // Nothing else uses the keyboard or mouse here.
    cfg.control.pause_on_user_input = false;
    Engine::new(backend, ConfigStore::in_memory(cfg))
}

fn fixture(e: &mut Engine<LinuxBackend>) -> (AppInfo, WindowInfo) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(apps) = e.backend_mut().list_apps() {
            for app in apps {
                if let Ok(windows) = e.backend_mut().list_windows(&app)
                    && let Some(w) = windows.into_iter().find(|w| w.title.contains("CU Test"))
                {
                    return (app, w);
                }
            }
        }
        assert!(Instant::now() < deadline, "the GTK fixture never appeared");
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// The status label's text, read afresh.
fn status(e: &mut Engine<LinuxBackend>, app: &AppInfo, w: &WindowInfo) -> String {
    let nodes = e
        .backend_mut()
        .snapshot(
            app,
            w,
            &SnapshotOptions {
                max_nodes: 500,
                max_depth: 30,
            },
        )
        .expect("snapshot");
    nodes
        .iter()
        .filter_map(|n| n.name.clone())
        .find(|n| n.starts_with("status:") || n.starts_with("entry:") || n.starts_with("feature:"))
        .unwrap_or_default()
}

fn wait_status(e: &mut Engine<LinuxBackend>, app: &AppInfo, w: &WindowInfo, want: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = status(e, app, w);
        if s == want || Instant::now() > deadline {
            return s;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn engine_over_wayland() {
    if !live() {
        return;
    }
    // Float the window somewhere known, away from the corner, so its
    // elements' coordinates must be converted to be right.
    let mut e = engine();
    let (app, _) = fixture(&mut e);
    swaymsg(&["[title=\"CU Test\"] floating enable, resize set 420 320, move position 230 140"]);
    std::thread::sleep(Duration::from_millis(500));
    let (app, w) = {
        let (_, w) = fixture(&mut e);
        (app, w)
    };
    let (sx, sy, sw, sh) = sway_rect();
    let b = w.bounds.expect("window bounds");
    println!("window {b:?}; sway {sx},{sy} {sw}x{sh}");
    assert!((b.x - sx).abs() < 1.0 && (b.y - sy).abs() < 1.0, "{b:?}");
    assert!(
        (b.width - sw).abs() < 1.0 && (b.height - sh).abs() < 1.0,
        "{b:?}"
    );

    // The elements are inside the window, where they are drawn.
    let nodes = e
        .backend_mut()
        .snapshot(
            &app,
            &w,
            &SnapshotOptions {
                max_nodes: 500,
                max_depth: 30,
            },
        )
        .expect("snapshot");
    let button = nodes
        .iter()
        .find(|n| n.name.as_deref() == Some("Click Me"))
        .and_then(|n| n.bounds)
        .expect("Click Me");
    println!("Click Me at {button:?}");
    assert!(
        button.x >= b.x && button.y >= b.y && button.x + button.width <= b.x + b.width,
        "{button:?} outside {b:?}"
    );

    // A click at its screen position (the virtual pointer) presses it.
    let target = InputTarget {
        pid: app.pid,
        window_id: Some(w.id),
        window_handle: Some(w.handle),
    };
    let centre = Point::new(
        button.x + button.width / 2.0,
        button.y + button.height / 2.0,
    );
    e.backend_mut()
        .click(&target, centre, MouseButton::Left, 1)
        .expect("click");
    assert_eq!(
        wait_status(&mut e, &app, &w, "status: clicked"),
        "status: clicked"
    );

    // The canvas has no accessible children: only a click at its place
    // reaches it.
    let canvas = nodes
        .iter()
        .find(|n| n.role == "drawing area" || n.native_role == "drawing area")
        .and_then(|n| n.bounds)
        .expect("canvas");
    let at = Point::new(canvas.x + 20.0, canvas.y + canvas.height / 2.0);
    e.backend_mut()
        .click(&target, at, MouseButton::Left, 1)
        .expect("click canvas");
    assert_eq!(
        wait_status(&mut e, &app, &w, "status: canvas clicked"),
        "status: canvas clicked"
    );

    // Through the engine: type into the entry (focus by accessibility,
    // keys by the virtual keyboard), any script.
    let tree = e
        .call(ToolCall::GetAppState(GetAppStateArgs {
            app: app.id.clone(),
            window: Some("CU Test".into()),
            disable_diff: true,
            ..Default::default()
        }))
        .expect("get_app_state");
    assert!(!tree.is_error, "{}", tree.text);
    assert!(
        tree.image.is_some(),
        "a screenshot of the window: {}",
        tree.text
    );
    let entry = tree
        .text
        .lines()
        .find(|l| l.contains("text field"))
        .and_then(|l| l.split_whitespace().next())
        .and_then(|t| t.parse::<u32>().ok())
        .expect("entry index");
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": app.id, "element_index": entry, "text": "héllo سلام ✓"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(
        wait_status(&mut e, &app, &w, "entry: héllo سلام ✓"),
        "entry: héllo سلام ✓"
    );
    let out = e.call_tool(
        "press_key",
        serde_json::json!({"app": app.id, "key": "ctrl+a"}),
    );
    assert!(!out.is_error, "{}", out.text);
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": app.id, "text": "replaced"}),
    );
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(
        wait_status(&mut e, &app, &w, "entry: replaced"),
        "entry: replaced"
    );

    // The window tool moves it through the compositor.
    let out = e.call_tool(
        "window",
        serde_json::json!({"app": app.id, "action": "move", "x": 300, "y": 200}),
    );
    assert!(!out.is_error, "{}", out.text);
    let (mx, my, _, _) = sway_rect();
    assert!(
        (mx - 300.0).abs() < 2.0 && (my - 200.0).abs() < 2.0,
        "{mx},{my}"
    );

    // The app with the keyboard is marked frontmost.
    let apps = e.backend_mut().list_apps().expect("apps");
    assert!(
        apps.iter().any(|a| a.pid == app.pid && a.frontmost),
        "{apps:?}"
    );

    screen_only_app(&mut e);
}

/// An app with no accessibility (the foot terminal): listed from the
/// compositor, seen in screenshots, used with the keyboard.
fn screen_only_app(e: &mut Engine<LinuxBackend>) {
    if Command::new("foot").arg("--version").output().is_err() {
        eprintln!("no foot terminal: the screen-only app check is skipped");
        return;
    }
    swaymsg(&["exec", "foot"]);
    let deadline = Instant::now() + Duration::from_secs(15);
    let foot = loop {
        let apps = e.backend_mut().list_apps().expect("apps");
        if let Some(a) = apps.into_iter().find(|a| a.id == "foot") {
            break a;
        }
        assert!(Instant::now() < deadline, "foot never showed up");
        std::thread::sleep(Duration::from_millis(200));
    };
    let mut out = e.call_tool(
        "get_app_state",
        serde_json::json!({"app": foot.pid.to_string(), "screenshot": true}),
    );
    // Its window may take a moment to be drawn.
    for _ in 0..20 {
        if !out.is_error {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
        out = e.call_tool(
            "get_app_state",
            serde_json::json!({"app": foot.pid.to_string(), "screenshot": true}),
        );
    }
    assert!(!out.is_error, "{}", out.text);
    assert!(out.image.is_some(), "no screenshot of the screen-only app");
    let marker = std::env::temp_dir().join(format!("cu-foot-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let out = e.call_tool(
        "type_text",
        serde_json::json!({"app": foot.pid.to_string(), "text": format!("echo typed > {}\n", marker.display())}),
    );
    assert!(!out.is_error, "{}", out.text);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the typing never reached foot");
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(&marker);
}
