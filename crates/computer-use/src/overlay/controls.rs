//! The label's two buttons (`overlay.show_buttons`) and what the pointer
//! does with them, for the helper and the hub alike:
//!
//! * the stop button: a click stops every agent, as the stop key does;
//!   under the pointer it lists the agents, and a row stops just that one
//!   (or lets a stopped one continue);
//! * the settings button: opens the settings panel, as the settings key
//!   does.
//!
//! Only the buttons and the list take the pointer; the rest of the overlay
//! stays click-through. While the engine moves the real mouse
//! ([`super::Cmd::Mouse`]) they don't either: its clicks go through to the
//! app below, and never work them.

use std::time::{Duration, Instant};

use super::draw::{self, Button, MenuRow, Press};
use super::helper::{Layer, Part, Pointer, Surface, pretty_key};
use super::text::Fonts;
use crate::types::{Point, Rect};

/// The list stays this long after the pointer left it and the stop button
/// (crossing the gap between them, or a slip).
const LINGER: Duration = Duration::from_millis(400);
/// Within this of a click that did something, a click does nothing (the
/// second half of a double click would undo the first).
const QUIET: Duration = Duration::from_millis(400);
/// Between the stop button and its list, and a button and its tag.
const GAP: f64 = 4.0;

/// The list and the tag are the overlay's, not one agent's.
const LIST: Layer = Layer::new(0, Part::Menu);
const TIP: Layer = Layer::new(0, Part::Tip);

/// What a click asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Stop every agent (the stop button, or the list's "All agents").
    StopAll,
    /// Let every agent continue ("All agents" when all are stopped).
    ContinueAll,
    /// Stop this agent only.
    Stop(u32),
    /// Let this agent, stopped, continue.
    Continue(u32),
    /// Open the settings panel.
    Settings,
}

/// A row of the stop button's list: one agent, or all of them (`None`).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub agent: Option<u32>,
    pub look: MenuRow,
}

/// What a click on `row` asks for.
fn row_action(row: &Row) -> Action {
    match (row.agent, row.look.stopped) {
        (None, false) => Action::StopAll,
        (None, true) => Action::ContinueAll,
        (Some(a), false) => Action::Stop(a),
        (Some(a), true) => Action::Continue(a),
    }
}

/// The texts beside the buttons: the list's first line (what the stop
/// button does) and the settings button's tag, each with its key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Texts {
    pub stop: String,
    pub settings: String,
}

impl Texts {
    /// For `stop_key` and `settings_key` ("" = none), with `many` agents.
    pub fn new(stop_key: &str, settings_key: &str, many: bool) -> Self {
        let with = |what: &str, key: &str| match key.trim() {
            "" => what.to_string(),
            k => format!("{what} · {}", pretty_key(k)),
        };
        Self {
            stop: with(if many { "Stop all" } else { "Stop" }, stop_key),
            settings: with("Settings", settings_key),
        }
    }
}

/// Something the pointer can be over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    /// A button beside this agent's label.
    Button(u32, Button),
    /// A row of the list.
    Row(usize),
    /// The list, outside its rows.
    List,
}

/// What the list was drawn from: its rows, its first line, the row under
/// the pointer, the scale (per cent) and its button's place.
type ListKey = (Vec<Row>, String, Option<(usize, Press)>, u32, (i64, i64));

/// The list as shown: what it was drawn from, where it is, and its rows'
/// boxes (screen units).
struct Shown {
    key: ListKey,
    at: Rect,
    rows: Vec<(Rect, Row)>,
    live: bool,
}

/// The pointer on the buttons and the stop button's list.
#[derive(Default)]
pub struct Controls {
    /// What the pointer is over, and what it pressed (a click is a press
    /// and a release on the same).
    over: Option<Spot>,
    pressed: Option<Spot>,
    /// The list hangs from this agent's stop button…
    open: Option<u32>,
    /// …and the pointer left them at this time (it closes a moment later).
    left: Option<Instant>,
    list: Option<Shown>,
    /// The settings button's tag as shown: its text, place and scale.
    tip: Option<(String, (i64, i64), u32)>,
    /// The engine moves the real mouse: nothing takes the pointer.
    blocked: bool,
    /// The last click that did something.
    acted: Option<Instant>,
}

impl Controls {
    /// The engine moves the real mouse (`on`), or is done with it. While
    /// it does, the list and the tag go (from `s`, when there is a
    /// display) and no click counts; the owner lets the pointer through
    /// the buttons too ([`super::helper::Painter::set_buttons`]).
    pub fn block(&mut self, on: bool, s: Option<&mut dyn Surface>) {
        self.blocked = on;
        if on {
            self.over = None;
            self.pressed = None;
            self.open = None;
            self.left = None;
            if let Some(s) = s {
                self.hide_list(s);
                self.hide_tip(s);
            }
        }
    }

    /// Whether the engine moves the real mouse now.
    pub fn blocked(&self) -> bool {
        self.blocked
    }

    /// Something shows the pointer's moves (paint often).
    pub fn busy(&self) -> bool {
        self.open.is_some() || self.over.is_some() || self.pressed.is_some()
    }

    /// How `agent`'s buttons show (stop, settings).
    pub fn look(&self, agent: u32) -> [Press; 2] {
        let of = |b: Button| {
            let spot = Some(Spot::Button(agent, b));
            if self.pressed == spot && self.over == spot {
                Press::Down
            } else if self.over == spot {
                Press::Hover
            } else {
                Press::Rest
            }
        };
        [of(Button::Stop), of(Button::Settings)]
    }

    /// What is at (x, y): a row of the list, the list, or a button
    /// (`buttons`: each agent's, stop then settings).
    fn spot(&self, x: f64, y: f64, buttons: &[(u32, [Rect; 2])]) -> Option<Spot> {
        let p = Point::new(x, y);
        if let Some(l) = &self.list {
            if let Some(i) = l.rows.iter().position(|(r, _)| r.contains(p)) {
                return Some(Spot::Row(i));
            }
            if l.at.contains(p) {
                return Some(Spot::List);
            }
        }
        buttons.iter().find_map(|(agent, [stop, gear])| {
            if stop.contains(p) {
                Some(Spot::Button(*agent, Button::Stop))
            } else if gear.contains(p) {
                Some(Spot::Button(*agent, Button::Settings))
            } else {
                None
            }
        })
    }

    /// The pointer did `p` (`buttons`: where each agent's are): what a
    /// click asks for, if it was one.
    pub fn pointer(
        &mut self,
        p: Pointer,
        buttons: &[(u32, [Rect; 2])],
        now: Instant,
    ) -> Option<Action> {
        if self.blocked {
            // Not the user's: the engine's own input.
            self.over = None;
            self.pressed = None;
            return None;
        }
        let mut clicked = None;
        match p {
            Pointer::Move(x, y) => self.over = self.spot(x, y, buttons),
            Pointer::Leave => self.over = None,
            Pointer::Press(x, y) => {
                self.over = self.spot(x, y, buttons);
                self.pressed = self.over.filter(|s| *s != Spot::List);
            }
            Pointer::Release(x, y) => {
                self.over = self.spot(x, y, buttons);
                if let Some(spot) = self.pressed.take().filter(|s| Some(*s) == self.over) {
                    clicked = match spot {
                        Spot::Button(_, Button::Stop) => Some(Action::StopAll),
                        Spot::Button(_, Button::Settings) => Some(Action::Settings),
                        Spot::Row(i) => self
                            .list
                            .as_ref()
                            .and_then(|l| l.rows.get(i))
                            .map(|(_, row)| row_action(row)),
                        Spot::List => None,
                    };
                }
            }
        }
        // The list opens on the stop button and stays while the pointer
        // is on it or the list.
        match self.over {
            Some(Spot::Button(agent, Button::Stop)) => {
                self.open = Some(agent);
                self.left = None;
            }
            Some(Spot::Row(_) | Spot::List) => self.left = None,
            _ => {
                if self.open.is_some() && self.left.is_none() {
                    self.left = Some(now);
                }
            }
        }
        let action = clicked?;
        if self
            .acted
            .is_some_and(|t| now.saturating_duration_since(t) < QUIET)
        {
            return None;
        }
        self.acted = Some(now);
        Some(action)
    }

    /// Show the list (under the stop button the pointer is on: `rows`)
    /// and the settings button's tag, or take them away; after the labels
    /// are painted, so `buttons` says where their buttons are now.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        buttons: &[(u32, [Rect; 2])],
        rows: &[Row],
        texts: &Texts,
        cfg_scale: f64,
        fonts: &Fonts,
        s: &mut dyn Surface,
        now: Instant,
    ) {
        if self.blocked {
            return;
        }
        if self
            .left
            .is_some_and(|t| now.saturating_duration_since(t) >= LINGER)
        {
            self.open = None;
            self.left = None;
        }
        let scale_at = |s: &dyn Surface, r: &Rect| {
            draw::sane_scale(if cfg_scale > 0.0 {
                cfg_scale as f32
            } else {
                s.scale_at(r.x, r.y)
            })
        };
        let at =
            |agent: u32, i: usize| buttons.iter().find(|(a, _)| *a == agent).map(|(_, b)| b[i]);
        // Its button gone (the label faded out): the list goes too.
        let anchor = self.open.and_then(|a| at(a, 0));
        if anchor.is_none() {
            self.open = None;
        }
        match anchor.filter(|_| !rows.is_empty()) {
            Some(button) => {
                let scale = scale_at(&*s, &button);
                let hover = match (self.over, self.pressed) {
                    (Some(Spot::Row(i)), Some(Spot::Row(j))) if i == j => Some((i, Press::Down)),
                    (Some(Spot::Row(i)), _) => Some((i, Press::Hover)),
                    _ => None,
                };
                let key = (
                    rows.to_vec(),
                    texts.stop.clone(),
                    hover,
                    (scale * 100.0).round() as u32,
                    (button.x.round() as i64, button.y.round() as i64),
                );
                if self.list.as_ref().is_none_or(|l| l.key != key) {
                    self.show_list(key, button, fonts, scale, s);
                }
            }
            None => self.hide_list(s),
        }
        // The settings button's tag, under it.
        let tip = match self.over {
            Some(Spot::Button(agent, Button::Settings)) => at(agent, 1),
            _ => None,
        };
        match tip {
            Some(gear) => {
                let scale = scale_at(&*s, &gear);
                let img = draw::tip(fonts, &texts.settings, scale);
                let ppu = f64::from(s.px_per_unit().max(0.1));
                let (w, h) = (f64::from(img.width()) / ppu, f64::from(img.height()) / ppu);
                let (x, y) = place(s.screen(), gear, w, h, true);
                let key = (
                    texts.settings.clone(),
                    (x.round() as i64, y.round() as i64),
                    (scale * 100.0).round() as u32,
                );
                if self.tip.as_ref() != Some(&key) {
                    s.show(TIP, &img, x, y);
                    self.tip = Some(key);
                }
            }
            None => self.hide_tip(s),
        }
    }

    fn show_list(
        &mut self,
        key: ListKey,
        button: Rect,
        fonts: &Fonts,
        scale: f32,
        s: &mut dyn Surface,
    ) {
        let looks: Vec<MenuRow> = key.0.iter().map(|r| r.look.clone()).collect();
        let (img, boxes) = draw::menu(fonts, &key.1, &looks, scale, key.2);
        let ppu = f64::from(s.px_per_unit().max(0.1));
        let (w, h) = (f64::from(img.width()) / ppu, f64::from(img.height()) / ppu);
        let (x, y) = place(s.screen(), button, w, h, false);
        s.show(LIST, &img, x, y);
        let live = self.list.as_ref().is_some_and(|l| l.live);
        if !live {
            s.set_live(LIST, true);
        }
        let unit = |v: f32| f64::from(v) / ppu;
        let rows = boxes
            .iter()
            .zip(&key.0)
            .map(|(b, row)| {
                let r = Rect::new(x + unit(b[0]), y + unit(b[1]), unit(b[2]), unit(b[3]));
                (r, row.clone())
            })
            .collect();
        self.list = Some(Shown {
            key,
            at: Rect::new(x, y, w, h),
            rows,
            live: true,
        });
    }

    fn hide_list(&mut self, s: &mut dyn Surface) {
        if let Some(l) = self.list.take() {
            if l.live {
                s.set_live(LIST, false);
            }
            s.hide(LIST);
        }
    }

    fn hide_tip(&mut self, s: &mut dyn Surface) {
        if self.tip.take().is_some() {
            s.hide(TIP);
        }
    }
}

/// Where a `w` x `h` box goes by `button`: under it (above it when there
/// is no room below), its left edge with the button's (or its right, when
/// it would run off the screen; centred on it when `centred`), on screen.
fn place(screen: Rect, button: Rect, w: f64, h: f64, centred: bool) -> (f64, f64) {
    let (right, bottom) = (screen.x + screen.width, screen.y + screen.height);
    let mut x = if centred {
        button.x + (button.width - w) / 2.0
    } else {
        button.x
    };
    if x + w > right {
        x = button.x + button.width - w;
    }
    let x = x.min(right - w).max(screen.x);
    let below = button.y + button.height + GAP;
    let y = if below + h <= bottom {
        below
    } else {
        (button.y - GAP - h).max(screen.y)
    };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, Pixmap};

    /// Records what is shown and which layers take the pointer.
    #[derive(Default)]
    struct Fake {
        shown: Vec<(Layer, f64, f64, u32, u32)>,
        live: std::collections::HashSet<Layer>,
    }

    impl Surface for Fake {
        fn excluded_from_capture(&self) -> bool {
            true
        }
        fn screen(&self) -> Rect {
            Rect::new(0.0, 0.0, 1280.0, 800.0)
        }
        fn render_scale(&self) -> f32 {
            1.0
        }
        fn px_per_unit(&self) -> f32 {
            1.0
        }
        fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64) {
            self.shown.retain(|s| s.0 != layer);
            self.shown.push((layer, x, y, img.width(), img.height()));
        }
        fn move_to(&mut self, _: Layer, _: f64, _: f64) {}
        fn hide(&mut self, layer: Layer) {
            self.shown.retain(|s| s.0 != layer);
        }
        fn set_hidden(&mut self, _: bool) {}
        fn pointer_input(&self) -> bool {
            true
        }
        fn set_live(&mut self, layer: Layer, live: bool) {
            if live {
                self.live.insert(layer);
            } else {
                self.live.remove(&layer);
            }
        }
        fn pump(&mut self) -> Vec<super::super::helper::SurfaceEvent> {
            Vec::new()
        }
        fn close(&mut self) {}
    }

    fn row(agent: Option<u32>, stopped: bool) -> Row {
        Row {
            agent,
            look: MenuRow {
                title: agent.map_or("All agents".into(), |a| format!("{a} · codex")),
                detail: "Firefox".into(),
                color: Color::from_rgba8(30, 136, 229, 255),
                stopped,
            },
        }
    }

    fn center(r: Rect) -> (f64, f64) {
        (r.x + r.width / 2.0, r.y + r.height / 2.0)
    }

    #[test]
    fn the_stop_button_stops_all_and_its_list_stops_one() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let (mut c, mut s, fonts) = (Controls::default(), Fake::default(), Fonts::default());
        let stop = Rect::new(500.0, 10.0, 28.0, 28.0);
        let gear = Rect::new(800.0, 10.0, 28.0, 28.0);
        let buttons = [(2, [stop, gear])];
        let rows = [row(None, false), row(Some(1), false), row(Some(2), true)];
        let texts = Texts::new("ctrl+alt+escape", "ctrl+alt+j", true);
        let (sx, sy) = center(stop);

        // On the stop button: lit, and the list shows under it, taking
        // the pointer.
        assert_eq!(c.pointer(Pointer::Move(sx, sy), &buttons, at(0)), None);
        assert_eq!(c.look(2), [Press::Hover, Press::Rest]);
        assert_eq!(c.look(1), [Press::Rest, Press::Rest]);
        c.paint(&buttons, &rows, &texts, 0.0, &fonts, &mut s, at(0));
        let list = *s.shown.iter().find(|l| l.0 == LIST).expect("the list");
        assert!(list.2 >= stop.y + stop.height, "under the button: {list:?}");
        assert!(s.live.contains(&LIST));
        let boxes: Vec<Rect> = c.list.as_ref().unwrap().rows.iter().map(|r| r.0).collect();
        assert_eq!(boxes.len(), 3);
        assert!(boxes.iter().all(|b| b.height >= 24.0 && b.width >= 24.0));

        // A click on the button itself: every agent.
        assert_eq!(c.pointer(Pointer::Press(sx, sy), &buttons, at(10)), None);
        assert_eq!(c.look(2)[0], Press::Down);
        assert_eq!(
            c.pointer(Pointer::Release(sx, sy), &buttons, at(20)),
            Some(Action::StopAll)
        );
        // Across the gap to the list: it stays.
        assert_eq!(c.pointer(Pointer::Leave, &buttons, at(30)), None);
        c.paint(&buttons, &rows, &texts, 0.0, &fonts, &mut s, at(200));
        assert!(c.list.is_some());
        // A row stops that agent only; a stopped one's row lets it go on.
        let (x1, y1) = center(boxes[1]);
        c.pointer(Pointer::Move(x1, y1), &buttons, at(500));
        c.pointer(Pointer::Press(x1, y1), &buttons, at(510));
        assert_eq!(
            c.pointer(Pointer::Release(x1, y1), &buttons, at(520)),
            Some(Action::Stop(1))
        );
        let (x2, y2) = center(boxes[2]);
        c.pointer(Pointer::Press(x2, y2), &buttons, at(1000));
        assert_eq!(
            c.pointer(Pointer::Release(x2, y2), &buttons, at(1010)),
            Some(Action::Continue(2))
        );
        // A second click at once is a double click: nothing.
        c.pointer(Pointer::Press(x2, y2), &buttons, at(1100));
        assert_eq!(
            c.pointer(Pointer::Release(x2, y2), &buttons, at(1110)),
            None
        );
        // Pressed on one row, let go on another: nothing.
        let (x0, y0) = center(boxes[0]);
        c.pointer(Pointer::Press(x0, y0), &buttons, at(2000));
        assert_eq!(
            c.pointer(Pointer::Release(x1, y1), &buttons, at(2010)),
            None
        );
        // Away from both: the list goes a moment later.
        c.pointer(Pointer::Move(10.0, 400.0), &buttons, at(3000));
        c.paint(&buttons, &rows, &texts, 0.0, &fonts, &mut s, at(3100));
        assert!(c.list.is_some());
        c.paint(&buttons, &rows, &texts, 0.0, &fonts, &mut s, at(3500));
        assert!(c.list.is_none());
        assert!(!s.shown.iter().any(|l| l.0 == LIST));
        assert!(!s.live.contains(&LIST));
    }

    #[test]
    fn the_settings_button_opens_settings_and_names_itself() {
        let t0 = Instant::now();
        let (mut c, mut s, fonts) = (Controls::default(), Fake::default(), Fonts::default());
        let gear = Rect::new(800.0, 10.0, 28.0, 28.0);
        let buttons = [(0, [Rect::new(500.0, 10.0, 28.0, 28.0), gear])];
        let texts = Texts::new("ctrl+alt+escape", "ctrl+alt+j", false);
        let (gx, gy) = center(gear);
        c.pointer(Pointer::Move(gx, gy), &buttons, t0);
        c.paint(
            &buttons,
            &[row(Some(0), false)],
            &texts,
            0.0,
            &fonts,
            &mut s,
            t0,
        );
        // Its tag shows under it, and no list (that is the stop button's).
        let tag = *s.shown.iter().find(|l| l.0 == TIP).expect("the tag");
        assert!(tag.2 > gear.y);
        assert!(!s.shown.iter().any(|l| l.0 == LIST));
        assert!(!s.live.contains(&TIP), "a tag never takes the pointer");
        c.pointer(Pointer::Press(gx, gy), &buttons, t0);
        assert_eq!(
            c.pointer(Pointer::Release(gx, gy), &buttons, t0),
            Some(Action::Settings)
        );
        assert_eq!(
            texts.settings,
            format!("Settings · {}", pretty_key("ctrl+alt+j"))
        );
        assert!(texts.stop.starts_with("Stop ·"), "{}", texts.stop);
    }

    #[test]
    fn while_the_engine_moves_the_mouse_nothing_takes_a_click() {
        let t0 = Instant::now();
        let (mut c, mut s, fonts) = (Controls::default(), Fake::default(), Fonts::default());
        let stop = Rect::new(500.0, 10.0, 28.0, 28.0);
        let buttons = [(1, [stop, Rect::new(800.0, 10.0, 28.0, 28.0)])];
        let rows = [row(Some(1), false)];
        let texts = Texts::new("", "", false);
        let (sx, sy) = center(stop);
        c.pointer(Pointer::Move(sx, sy), &buttons, t0);
        c.paint(&buttons, &rows, &texts, 0.0, &fonts, &mut s, t0);
        assert!(s.live.contains(&LIST));
        // The engine is about to click there: the list goes at once.
        c.block(true, Some(&mut s));
        assert!(s.shown.is_empty() && s.live.is_empty());
        c.pointer(Pointer::Press(sx, sy), &buttons, t0);
        assert_eq!(c.pointer(Pointer::Release(sx, sy), &buttons, t0), None);
        c.paint(&buttons, &rows, &texts, 0.0, &fonts, &mut s, t0);
        assert!(s.shown.is_empty());
        // Done with it: the user's clicks count again.
        c.block(false, Some(&mut s));
        c.pointer(Pointer::Press(sx, sy), &buttons, t0);
        assert_eq!(
            c.pointer(Pointer::Release(sx, sy), &buttons, t0),
            Some(Action::StopAll)
        );
    }

    #[test]
    fn the_list_stays_on_screen() {
        let screen = Rect::new(0.0, 0.0, 1280.0, 800.0);
        // At the right edge: its right edge with the button's.
        let (x, y) = place(
            screen,
            Rect::new(1260.0, 10.0, 20.0, 20.0),
            200.0,
            100.0,
            false,
        );
        assert_eq!((x, y), (1080.0, 34.0));
        // At the bottom: above the button.
        let (_, y) = place(
            screen,
            Rect::new(10.0, 780.0, 20.0, 20.0),
            200.0,
            100.0,
            false,
        );
        assert_eq!(y, 676.0);
    }
}
