//! Live tests of the Wayland protocol client against a real compositor: a
//! headless sway (wlroots, the protocols Hyprland speaks too) with
//! gtk3-widget-factory running, checked against grim's screenshots, sway's
//! IPC and the app's accessibility tree:
//!
//! ```text
//! APPS=gtk3-widget-factory scripts/wayland-session.sh \
//!   cargo test -p computer-use --test live_wayland -- --ignored --nocapture --test-threads=1
//! ```
//!
//! (`CU_SCALE=1.5` runs it on a fractionally scaled output; there the
//! typing test can lose gtk3-widget-factory itself: GTK 3 crashes when its
//! accessibility is read, by the Python reader these tests use, in the
//! middle of a burst of keys, and GTK is slow on this software-rendered
//! compositor at fractional scales. The engine's own test,
//! `live_wayland_engine`, types and reads at 1.5 without it.) Outside such
//! a session every test returns at once.
//!
//! The client module is private to the library, so it is compiled into
//! this test from its source file, with the crate paths it uses pointed at
//! the library's public modules.

#![cfg(target_os = "linux")]

mod error {
    pub use computer_use::error::*;
}
mod keys {
    pub use computer_use::keys::*;
}
mod types {
    pub use computer_use::types::*;
}

#[allow(dead_code)]
#[path = "../src/linux/wayland/proto.rs"]
mod proto;

use std::process::Command;
use std::time::{Duration, Instant};

use computer_use::error::Error;
use computer_use::keys::parse_combo;
use computer_use::types::{Capture, Rect};
use image::RgbaImage;
use proto::Wl;
use serde::Deserialize;
use serde_json::Value;

const APP: &str = "gtk3-widget-factory";
/// Where the app's window is put (floating), in the layout.
const WINDOW_AT: (i32, i32) = (20, 20);

fn in_session() -> bool {
    let ok =
        std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("SWAYSOCK").is_some();
    if !ok {
        eprintln!("not in a sway session (run it through scripts/wayland-session.sh): skipped");
    }
    ok
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// `swaymsg -r <args>`, parsed.
fn swaymsg(args: &[&str]) -> Value {
    let out = Command::new("swaymsg")
        .arg("-r")
        .args(args)
        .output()
        .expect("swaymsg runs");
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn sway_outputs() -> Vec<Value> {
    swaymsg(&["-t", "get_outputs"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|o| o["active"].as_bool() == Some(true))
        .collect()
}

fn rect_of(v: &Value) -> Rect {
    let n = |k: &str| v[k].as_f64().unwrap_or(0.0);
    Rect::new(n("x"), n("y"), n("width"), n("height"))
}

/// The window of `app_id` in sway's tree: where sway puts it, and its
/// own size (larger when it doesn't fit the output).
fn sway_window(app_id: &str) -> Option<(Rect, Rect)> {
    fn find(n: &Value, app_id: &str) -> Option<(Rect, Rect)> {
        if n["app_id"].as_str() == Some(app_id) {
            return Some((rect_of(&n["rect"]), rect_of(&n["geometry"])));
        }
        ["nodes", "floating_nodes"]
            .iter()
            .filter_map(|k| n[*k].as_array())
            .flatten()
            .find_map(|c| find(c, app_id))
    }
    find(&swaymsg(&["-t", "get_tree"]), app_id)
}

/// grim's screenshot of `r` (at the highest output scale, like ours).
fn grim(r: Rect) -> RgbaImage {
    let geometry = format!("{},{} {}x{}", r.x, r.y, r.width, r.height);
    let out = Command::new("grim")
        .args(["-g", &geometry, "-t", "png", "-"])
        .output()
        .expect("grim runs");
    assert!(
        out.status.success(),
        "grim: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    image::load_from_memory(&out.stdout)
        .expect("grim gives a PNG")
        .to_rgba8()
}

/// Over the area both images cover: (share of pixels that differ by more
/// than 24 in a channel, mean absolute channel difference).
fn difference(cap: &Capture, theirs: &RgbaImage) -> (f64, f64) {
    let w = cap.width.min(theirs.width());
    let h = cap.height.min(theirs.height());
    let (mut off, mut sum) = (0usize, 0u64);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * cap.width + x) * 4) as usize;
            let b = theirs.get_pixel(x, y).0;
            let d: Vec<u8> = (0..3).map(|c| cap.rgba[i + c].abs_diff(b[c])).collect();
            sum += d.iter().map(|x| u64::from(*x)).sum::<u64>();
            if d.iter().any(|x| *x > 24) {
                off += 1;
            }
        }
    }
    let px = (w * h).max(1) as usize;
    (off as f64 / px as f64, sum as f64 / (px * 3) as f64)
}

/// Capture `r` and compare it with grim's. Where the region's edges fall
/// between pixels (fractional scales), the compositor rounds them to whole
/// pixels for us while grim interpolates, so edges differ by half a pixel.
fn matches_grim(wl: &mut Wl, r: Rect, what: &str) {
    let cap = wl.capture(r).expect("capture");
    let shot = grim(cap.bounds);
    let (share, mean) = difference(&cap, &shot);
    let b = cap.bounds;
    let s = f64::from(cap.width) / b.width;
    let aligned = [b.x, b.y, b.x + b.width, b.y + b.height]
        .iter()
        .all(|v| (v * s - (v * s).round()).abs() < 1e-6);
    println!(
        "{what}: capture of {:?} -> {}x{} (grim {}x{}), {:.3}% pixels differ, mean diff {mean:.2}{}",
        cap.bounds,
        cap.width,
        cap.height,
        shot.width(),
        shot.height(),
        share * 100.0,
        if aligned {
            ""
        } else {
            " (edges between pixels)"
        }
    );
    assert_eq!(cap.rgba.len(), (cap.width * cap.height * 4) as usize);
    if aligned {
        assert_eq!((cap.width, cap.height), shot.dimensions(), "{what}: size");
    } else {
        // grim rounds a size like 799.5 pixels one way, we the other.
        assert!(cap.width.abs_diff(shot.width()) <= 1 && cap.height.abs_diff(shot.height()) <= 1);
    }
    let (most, mean_most) = if aligned { (0.01, 2.0) } else { (0.08, 8.0) };
    assert!(
        share < most && mean < mean_most,
        "{what}: differs from grim"
    );
}

/// A node of the app's accessibility tree (showing ones only).
#[derive(Debug, Clone, Deserialize)]
struct Node {
    role: String,
    name: String,
    /// Extents relative to the app's surface (GTK on Wayland doesn't know
    /// where its window is), which includes the client-side shadow.
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    checked: bool,
    /// Can take the focus (an insensitive widget can't).
    sensitive: bool,
    focusable: bool,
    parent: String,
    text: Option<String>,
    value: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
}

const ATSPI_DUMP: &str = r#"
import gi, json, sys
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi
S = Atspi.StateType
out = []
def walk(n, parent, depth):
    if n is None or depth > 60:
        return
    ss = n.get_state_set()
    if ss.contains(S.SHOWING):
        comp = n.get_component_iface()
        e = comp.get_extents(Atspi.CoordType.WINDOW) if comp else None
        d = {'role': n.get_role_name(), 'name': n.get_name() or '', 'parent': parent,
             'x': e.x if e else 0, 'y': e.y if e else 0,
             'w': e.width if e else 0, 'h': e.height if e else 0,
             'checked': ss.contains(S.CHECKED), 'sensitive': ss.contains(S.SENSITIVE),
             'focusable': ss.contains(S.FOCUSABLE),
             'text': None, 'value': None, 'min': None, 'max': None}
        if n.get_text_iface() is not None:
            d['text'] = Atspi.Text.get_text(n, 0, Atspi.Text.get_character_count(n))
        if n.get_value_iface() is not None:
            d['value'] = Atspi.Value.get_current_value(n)
            d['min'] = Atspi.Value.get_minimum_value(n)
            d['max'] = Atspi.Value.get_maximum_value(n)
        out.append(d)
    role = n.get_role_name()
    for j in range(n.get_child_count()):
        walk(n.get_child_at_index(j), role, depth + 1)
desk = Atspi.get_desktop(0)
for i in range(desk.get_child_count()):
    app = desk.get_child_at_index(i)
    if app is not None and app.get_name() == sys.argv[1]:
        walk(app, '', 0)
print(json.dumps(out))
"#;

/// A Python that can import gi with the Atspi typelib: `CU_PYTHON`, else
/// the system's 3.12 where there is one, else `python3`.
fn python() -> String {
    std::env::var("CU_PYTHON").unwrap_or_else(|_| {
        if std::path::Path::new("/usr/bin/python3.12").exists() {
            "/usr/bin/python3.12".into()
        } else {
            "python3".into()
        }
    })
}

/// The app's showing widgets, over AT-SPI. Patient: an app busy laying out
/// a lot of text can leave AT-SPI calls waiting for D-Bus's 25 s timeout.
fn widgets() -> Vec<Node> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let out = Command::new(python())
            .args(["-c", ATSPI_DUMP, APP])
            .output()
            .expect("a Python with gi and the Atspi typelib");
        let nodes: Vec<Node> = serde_json::from_slice(&out.stdout).unwrap_or_default();
        if !nodes.is_empty() {
            return nodes;
        }
        assert!(
            Instant::now() < deadline,
            "{APP} isn't on the accessibility bus: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        sleep(300);
    }
}

/// Poll the widgets until `found` finds what it wants.
fn wait_for<T>(what: &str, found: impl FnMut(&[Node]) -> Option<T>) -> T {
    poll_for(found).unwrap_or_else(|| panic!("timed out waiting for {what}"))
}

/// Poll the widgets until `found` finds what it wants; None after 15 s.
fn poll_for<T>(mut found: impl FnMut(&[Node]) -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let nodes = widgets();
        if let Some(t) = found(&nodes) {
            return Some(t);
        }
        if Instant::now() >= deadline {
            return None;
        }
        sleep(300);
    }
}

/// The app's window, floating at `WINDOW_AT`.
struct Factory {
    /// The window as sway places it.
    window: Rect,
    /// The client-side shadow around it in the app's surface.
    margin: (f64, f64),
}

impl Factory {
    fn place() -> Self {
        let (x, y) = WINDOW_AT;
        swaymsg(&[&format!(
            "[app_id={APP}] floating enable, move position {x} {y}"
        )]);
        sleep(300);
        let (window, own) = sway_window(APP).unwrap_or_else(|| {
            panic!("{APP} isn't running (APPS={APP} scripts/wayland-session.sh)")
        });
        let frame = widgets()
            .into_iter()
            .find(|n| n.role == "frame")
            .expect("the app's frame");
        let margin = ((frame.w - own.width) / 2.0, (frame.h - own.height) / 2.0);
        Self { window, margin }
    }

    /// Where a point of a node is in the layout: `fx`, `fy` from its
    /// left/top edge as a share of its size.
    fn at(&self, n: &Node, fx: f64, fy: f64) -> (f64, f64) {
        (
            self.window.x + n.x - self.margin.0 + n.w * fx,
            self.window.y + n.y - self.margin.1 + n.h * fy,
        )
    }

    fn center(&self, n: &Node) -> (f64, f64) {
        self.at(n, 0.5, 0.5)
    }
}

fn node<'a>(nodes: &'a [Node], role: &str, name: &str) -> Option<&'a Node> {
    nodes.iter().find(|n| n.role == role && n.name == name)
}

/// Show one of the app's pages ("Page 1".."Page 3") by clicking its tab.
fn show_page(wl: &mut Wl, f: &Factory, page: &str) {
    let nodes = widgets();
    let tab = node(&nodes, "radio button", page)
        .expect("the page's tab")
        .clone();
    if !tab.checked {
        let (x, y) = f.center(&tab);
        wl.click(x, y, 1, 1).expect("click");
        wait_for(page, |n| {
            node(n, "radio button", page)
                .filter(|t| t.checked)
                .map(|_| ())
        });
    }
}

/// An entry on page 1 (left column, not in a combo box) that can take
/// the focus (the page also shows insensitive ones).
fn entry(nodes: &[Node]) -> Node {
    let mut entries: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.role == "text" && n.parent != "combo box" && n.w > 300.0)
        .filter(|n| n.sensitive && n.focusable && n.x < 100.0)
        .collect();
    entries.sort_by(|a, b| a.y.total_cmp(&b.y));
    (*entries.first().expect("an entry on page 1")).clone()
}

fn same_place(a: &Node, b: &Node) -> bool {
    a.role == b.role && a.x == b.x && a.y == b.y
}

/// A horizontal slider (GtkScale) on page 1, the lowest one.
fn slider(nodes: &[Node]) -> Node {
    nodes
        .iter()
        .filter(|n| n.role == "slider" && n.w > n.h * 3.0 && n.value.is_some())
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .expect("a horizontal slider")
        .clone()
}

fn fraction(n: &Node) -> f64 {
    let (v, lo, hi) = (n.value.unwrap(), n.min.unwrap(), n.max.unwrap());
    (v - lo) / (hi - lo)
}

fn combo(s: &str) -> computer_use::keys::KeyCombo {
    parse_combo(s).expect("a key combo")
}

#[test]
#[ignore]
fn outputs_and_layout_match_sway() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    println!("compositor offers: {:?}", wl.protocols());
    let outs = wl.outputs();
    println!("outputs: {outs:?}, layout {:?}", wl.layout());
    let sway = sway_outputs();
    assert_eq!(outs.len(), sway.len());
    for s in &sway {
        let name = s["name"].as_str().unwrap();
        let o = outs.iter().find(|o| o.name == name).expect("same names");
        assert_eq!(o.logical, rect_of(&s["rect"]), "{name}");
        assert_eq!(o.scale, s["scale"].as_f64().unwrap(), "{name}");
    }
    let union = sway
        .iter()
        .map(|s| rect_of(&s["rect"]))
        .fold(None, |u: Option<Rect>, r| {
            Some(match u {
                None => r,
                Some(u) => {
                    let (x0, y0) = (u.x.min(r.x), u.y.min(r.y));
                    let x1 = (u.x + u.width).max(r.x + r.width);
                    let y1 = (u.y + u.height).max(r.y + r.height);
                    Rect::new(x0, y0, x1 - x0, y1 - y0)
                }
            })
        });
    assert_eq!(Some(wl.layout()), union);
    assert!(wl.can_capture() && wl.can_point() && wl.can_type());
    assert!(!wl.lost());
}

#[test]
#[ignore]
fn capture_matches_grim() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    Factory::place();
    let layout = wl.layout();
    let scale = wl.outputs()[0].scale;
    matches_grim(&mut wl, Rect::new(10.0, 10.0, 400.0, 300.0), "a region");
    let t = Instant::now();
    matches_grim(&mut wl, layout, "the whole layout");
    println!("whole layout capture + grim took {:?}", t.elapsed());
    // Hanging off the layout: cut to it.
    let r = Rect::new(layout.width - 100.0, layout.height - 50.0, 300.0, 300.0);
    let cap = wl.capture(r).expect("capture");
    assert_eq!(
        cap.bounds,
        Rect::new(layout.width - 100.0, layout.height - 50.0, 100.0, 50.0)
    );
    assert_eq!(cap.width, (100.0 * scale).round() as u32);
    matches_grim(&mut wl, r, "a region hanging off the layout");
    // Entirely outside: an error, not an empty image.
    let outside = wl.capture(Rect::new(layout.width + 10.0, 0.0, 50.0, 50.0));
    assert!(matches!(outside, Err(Error::InvalidArgs(_))), "{outside:?}");
    // Time of a plain capture.
    let t = Instant::now();
    for _ in 0..5 {
        wl.capture(Rect::new(0.0, 0.0, 800.0, 500.0))
            .expect("capture");
    }
    println!("800x500 capture: {:?} each", t.elapsed() / 5);
}

/// Restores the output's scale and transform.
struct OutputBack(String, f64);

impl Drop for OutputBack {
    fn drop(&mut self) {
        swaymsg(&[&format!(
            "output {} scale {} transform normal",
            self.0, self.1
        )]);
        swaymsg(&["output HEADLESS-2 unplug"]);
        sleep(300);
    }
}

#[test]
#[ignore]
fn scale_transform_and_hotplug_are_followed() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    let first = wl.outputs()[0].clone();
    let _back = OutputBack(first.name.clone(), first.scale);
    Factory::place();
    // On whole pixels at 1.25 (multiples of 4 logical units).
    let region = Rect::new(32.0, 32.0, 360.0, 240.0);

    swaymsg(&[&format!("output {} scale 1.25", first.name)]);
    sleep(300);
    let o = wl.outputs()[0].clone();
    println!("at scale 1.25: {o:?}");
    assert_eq!(o.scale, 1.25);
    assert_eq!(o.logical.width, 1024.0);
    matches_grim(&mut wl, region, "scale 1.25");
    matches_grim(&mut wl, Rect::new(30.0, 30.0, 361.0, 241.0), "scale 1.25");

    for t in ["90", "flipped-90", "180", "flipped"] {
        swaymsg(&[&format!("output {} transform {t}", first.name)]);
        sleep(400);
        let o = wl.outputs()[0].clone();
        println!("transform {t}: {o:?}");
        if t.ends_with("90") {
            assert_eq!((o.logical.width, o.logical.height), (640.0, 1024.0));
        }
        matches_grim(&mut wl, region, &format!("transform {t}"));
    }
    swaymsg(&[&format!("output {} scale 1 transform normal", first.name)]);

    // A second output appears (hotplug), at scale 2 to the right.
    swaymsg(&["create_output"]);
    sleep(300);
    let second = sway_outputs()
        .into_iter()
        .find(|s| s["name"].as_str() != Some(first.name.as_str()))
        .expect("a second output");
    let name2 = second["name"].as_str().unwrap().to_string();
    swaymsg(&[&format!(
        "output {name2} resolution 800x600 position 1280 0 scale 2"
    )]);
    sleep(500);
    let outs = wl.outputs();
    println!("after hotplug: {outs:?}");
    assert_eq!(outs.len(), 2);
    let o2 = outs
        .iter()
        .find(|o| o.name == name2)
        .expect("the new output");
    assert_eq!(o2.logical, Rect::new(1280.0, 0.0, 400.0, 300.0));
    assert_eq!(o2.scale, 2.0);
    assert_eq!(wl.layout(), Rect::new(0.0, 0.0, 1680.0, 800.0));
    // Spanning both: at scale 2, the first output's part upscaled, the
    // part below the second output black.
    let span = Rect::new(1180.0, 200.0, 200.0, 200.0);
    let cap = wl.capture(span).expect("capture across outputs");
    assert_eq!((cap.width, cap.height), (400, 400));
    let px = |x: u32, y: u32| {
        let i = ((y * cap.width + x) * 4) as usize;
        cap.rgba[i..i + 4].to_vec()
    };
    assert_eq!(px(399, 399), vec![0, 0, 0, 255], "outside every output");
    let shot = grim(span);
    assert_eq!(shot.dimensions(), (400, 400));
    let (share, mean) = difference(&cap, &shot);
    println!(
        "across outputs: {:.2}% pixels differ from grim, mean diff {mean:.2}",
        share * 100.0
    );
    assert!(mean < 6.0);
    swaymsg(&[&format!("output {name2} unplug")]);
    sleep(400);
    assert_eq!(wl.outputs().len(), 1, "the unplugged output is gone");
}

#[test]
#[ignore]
fn click_switches_pages() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    let f = Factory::place();
    show_page(&mut wl, &f, "Page 1");
    let nodes = widgets();
    let tab = node(&nodes, "radio button", "Page 2")
        .expect("Page 2")
        .clone();
    assert!(!tab.checked);
    let (x, y) = f.center(&tab);
    let t = Instant::now();
    wl.click(x, y, 1, 1).expect("click");
    println!("click at ({x}, {y}) took {:?}", t.elapsed());
    wait_for("Page 2 checked", |n| {
        node(n, "radio button", "Page 2")
            .filter(|t| t.checked)
            .map(|_| ())
    });
    show_page(&mut wl, &f, "Page 1");
    // Bad buttons are refused.
    assert!(matches!(wl.click(x, y, 7, 1), Err(Error::InvalidArgs(_))));
}

#[test]
#[ignore]
fn typing_and_keys_reach_a_gtk_entry() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    let f = Factory::place();
    show_page(&mut wl, &f, "Page 1");
    let field = entry(&widgets());
    let (x, y) = f.center(&field);
    wl.click(x, y, 1, 1).expect("click into the entry");
    sleep(200);
    // Read once the app is done with the keys: GTK 3's accessibility
    // bridge, queried while an entry still takes in typed text, can crash
    // the app (seen here with or without keymap changes).
    let read = |want: &str| {
        // A read while GTK 3 still works through a burst of keys can crash
        // it (its accessibility bridge; seen with any text, on this headless
        // software-rendered compositor mostly at fractional scales, where
        // GTK is slow): give it time first.
        sleep(1000 + 25 * want.chars().count() as u64);
        let mut last = None;
        poll_for(|n| {
            last = n
                .iter()
                .find(|e| same_place(e, &field))
                .and_then(|e| e.text.clone());
            last.clone().filter(|t| t == want)
        })
        .unwrap_or_else(|| panic!("the entry reads {last:?}, not {want:?}"))
    };

    if std::env::var("CU_AWAY").is_ok() {
        wl.move_pointer(1.0, 1.0).expect("move away");
    }
    let text = "سلام ✓ héllo 日本語 😀";
    wl.press(&combo("ctrl+a")).expect("ctrl+a");
    let t = Instant::now();
    wl.type_text(text).expect("type");
    println!("typed {} chars in {:?}", text.chars().count(), t.elapsed());
    println!("entry reads {:?}", read(text));

    // A combo: select all, then typing replaces it.
    wl.press(&combo("ctrl+a")).expect("ctrl+a");
    wl.type_text("Replaced, with Tab-free ASCII!")
        .expect("type");
    read("Replaced, with Tab-free ASCII!");

    // Named keys: BackSpace, Home, End, shift+Left (selection).
    wl.press(&combo("Backspace")).expect("backspace");
    read("Replaced, with Tab-free ASCII");
    wl.press(&combo("Home")).expect("home");
    wl.type_text("» ").expect("type");
    read("» Replaced, with Tab-free ASCII");
    wl.press(&combo("End")).expect("end");
    for _ in 0..5 {
        wl.press(&combo("shift+Left")).expect("shift+left");
    }
    wl.type_text("text").expect("type");
    read("» Replaced, with Tab-free text");

    // Many different characters: past the keymap's room, so typed with a
    // keymap made afresh partway.
    let many: String = (0..200u32)
        .filter_map(|i| char::from_u32(0x4e00 + i * 3))
        .collect();
    wl.press(&combo("ctrl+a")).expect("ctrl+a");
    let t = Instant::now();
    wl.type_text(&many).expect("type many");
    println!("typed 200 distinct CJK chars in {:?}", t.elapsed());
    read(&many);
    // (Delete is the widget factory's own shortcut: Backspace clears.)
    wl.press(&combo("ctrl+a")).expect("ctrl+a");
    wl.press(&combo("Backspace")).expect("backspace");
    read("");

    // Control characters other than the editing ones are refused.
    assert!(matches!(wl.type_text("a\u{1}"), Err(Error::InvalidArgs(_))));
}

#[test]
#[ignore]
fn drag_and_draw_move_a_slider() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    let f = Factory::place();
    show_page(&mut wl, &f, "Page 1");
    let s = slider(&widgets());
    println!("slider {s:?}, at {:.2}", fraction(&s));
    let layout = wl.layout();
    let to = f.at(&s, 0.7, 0.5);
    assert!(to.0 < layout.x + layout.width, "the slider is on screen");
    wl.drag(f.at(&s, 0.1, 0.5), to).expect("drag");
    let v = wait_for("the slider to move right", |n| {
        n.iter()
            .find(|x| same_place(x, &s))
            .map(fraction)
            .filter(|v| (0.55..0.85).contains(v))
    });
    println!("after a drag to 70%: {v:.2}");

    // Drawing a stroke with the button held moves it too; pace is asked
    // before every move.
    let stroke: Vec<(f64, f64)> = (0..=10)
        .map(|i| f.at(&s, 0.6 - f64::from(i) * 0.04, 0.5))
        .collect();
    let mut paces = 0;
    wl.draw(&[stroke], 1, &mut |_| {
        paces += 1;
        Ok(())
    })
    .expect("draw");
    assert_eq!(paces, 11);
    let v = wait_for("the slider to move left", |n| {
        n.iter()
            .find(|x| same_place(x, &s))
            .map(fraction)
            .filter(|v| (0.05..0.35).contains(v))
    });
    println!("after a stroke to 20%: {v:.2}");

    // Stopped midway: the error comes back and the button is up again
    // (a later click works as a click).
    let mut calls = 0;
    let stopped = wl.draw(
        &[vec![
            f.at(&s, 0.3, 0.5),
            f.at(&s, 0.4, 0.5),
            f.at(&s, 0.5, 0.5),
        ]],
        1,
        &mut |_| {
            calls += 1;
            if calls > 2 {
                Err(Error::Stopped("test".into()))
            } else {
                Ok(())
            }
        },
    );
    assert!(matches!(stopped, Err(Error::Stopped(_))), "{stopped:?}");
    show_page(&mut wl, &f, "Page 2");
    show_page(&mut wl, &f, "Page 1");
}

#[test]
#[ignore]
fn scroll_turns_a_spin_button() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    let f = Factory::place();
    show_page(&mut wl, &f, "Page 1");
    let nodes = widgets();
    let spin = nodes
        .iter()
        .find(|n| n.role == "spin button" && n.value.is_some())
        .expect("a spin button")
        .clone();
    let before = spin.value.unwrap();
    let (x, y) = f.center(&spin);
    wl.scroll(x, y, 0, -3).expect("scroll up");
    let up = wait_for("the spin button to go up", |n| {
        n.iter()
            .find(|x| same_place(x, &spin))
            .and_then(|x| x.value)
            .filter(|v| *v > before)
    });
    wl.scroll(x, y, 0, 2).expect("scroll down");
    let down = wait_for("the spin button to go down", |n| {
        n.iter()
            .find(|x| same_place(x, &spin))
            .and_then(|x| x.value)
            .filter(|v| *v < up)
    });
    println!("spin button: {before} -> {up} (3 up) -> {down} (2 down)");
    wl.scroll(x, y, 0, 1).expect("scroll back");
}

#[test]
#[ignore]
fn idle_needs_input_idle_notifications() {
    if !in_session() {
        return;
    }
    let mut wl = Wl::connect().expect("connect");
    let v2 = wl
        .protocols()
        .iter()
        .any(|(i, v)| i == "ext_idle_notifier_v1" && *v >= 2);
    let t = Instant::now();
    let first = wl.idle();
    println!(
        "idle() = {first:?} (took {:?}; ext-idle-notify v2 offered: {v2})",
        t.elapsed()
    );
    if !v2 {
        // v1 only: idle inhibitors would hide activity, so no answer.
        assert_eq!(first, None);
        return;
    }
    let first = first.expect("an idle time");
    sleep(1200);
    let later = wl.idle().expect("an idle time");
    println!("1.2 s later: {later:?}");
    assert!(later >= first + Duration::from_millis(1000));
    wl.move_pointer(5.0, 5.0).expect("move");
    sleep(50);
    let after = wl.idle().expect("an idle time");
    println!("after input: {after:?}");
    assert!(after < Duration::from_millis(500));
}
