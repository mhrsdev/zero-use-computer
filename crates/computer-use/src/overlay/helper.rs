//! The overlay helper process: reads [`Cmd`]s from stdin, keeps the state
//! (colours, timers, cursor glide, click ripple) and draws it on a native
//! [`Surface`]. It exits as soon as stdin closes or its parent is gone.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tiny_skia::{Color, Pixmap};

use super::draw;
use super::text::Fonts;
use super::{Cmd, Reply, Status};
use crate::config::OverlayConfig;
use crate::types::Rect;

/// One on-screen piece of the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    Top,
    Right,
    Bottom,
    Left,
    Label,
    Cursor,
}

const EDGES: [Layer; 4] = [Layer::Top, Layer::Right, Layer::Bottom, Layer::Left];

/// The global keys the helper listens for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hotkey {
    /// The emergency stop key.
    Stop,
    /// Opens the decision model's settings page.
    Settings,
}

impl Hotkey {
    pub const ALL: [Hotkey; 2] = [Hotkey::Stop, Hotkey::Settings];

    pub fn index(self) -> usize {
        match self {
            Hotkey::Stop => 0,
            Hotkey::Settings => 1,
        }
    }
}

/// Something that happened on the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceEvent {
    /// The user pressed one of the global keys.
    Hotkey(Hotkey),
}

/// A platform's way of putting images on screen: always on top,
/// click-through, never activated, and (where the OS allows) left out of
/// screen captures. Coordinates are the engine's screen units.
pub trait Surface {
    /// Whether screen captures leave the overlay out by themselves.
    fn excluded_from_capture(&self) -> bool;
    /// The area the overlay covers, in screen units (every monitor, where
    /// it spans several); everything drawn stays inside it.
    fn screen(&self) -> Rect;
    /// The screen the glow runs around and the label sits on when there is
    /// no target window (the primary monitor where `screen` spans several).
    fn main_screen(&self) -> Rect {
        self.screen()
    }
    /// Space taken at the top of the screen by the system (the macOS menu
    /// bar), kept clear of the label.
    fn top_inset(&self) -> f64 {
        0.0
    }
    /// How large to draw (1 = 100%).
    fn render_scale(&self) -> f32;
    /// Image pixels per screen unit (2 on a Retina Mac, else 1).
    fn px_per_unit(&self) -> f32;
    /// Show `img` for `layer` with its top-left corner at (x, y).
    fn show(&mut self, layer: Layer, img: &Pixmap, x: f64, y: f64);
    /// Move a shown layer without changing its image.
    fn move_to(&mut self, layer: Layer, x: f64, y: f64);
    fn hide(&mut self, layer: Layer);
    /// Hide (or restore) everything at once, e.g. around a screenshot.
    fn set_hidden(&mut self, hidden: bool);
    /// Set the whole overlay's opacity natively; `false` if unsupported
    /// (the painter then redraws faded images instead).
    fn set_opacity(&mut self, _opacity: f32) -> bool {
        false
    }
    /// Listen for one of the global keys anywhere on the system (`None`
    /// stops listening). Only these key combinations are received, never
    /// other keys. Returns whether the system accepted it.
    fn set_hotkey(&mut self, _which: Hotkey, _combo: Option<crate::keys::KeyCombo>) -> bool {
        false
    }
    /// Handle native events; returns global key presses.
    fn pump(&mut self) -> Vec<SurfaceEvent>;
    fn close(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Off,
    Thinking,
    Working,
    Error,
    Done,
    /// Waiting while the user uses the mouse/keyboard.
    Paused,
    /// Stopped with the emergency stop key.
    Stopped,
}

/// How long "stopped" stays on screen when nothing else happens.
const STOPPED_HOLD: Duration = Duration::from_millis(5000);

/// A key combination as people write it: "ctrl+alt+escape" → "Ctrl+Alt+Esc".
pub fn pretty_key(key: &str) -> String {
    key.split('+')
        .map(|p| {
            let p = p.trim();
            match p.to_ascii_lowercase().as_str() {
                "escape" | "esc" => "Esc".to_string(),
                "ctrl" | "control" => "Ctrl".to_string(),
                "cmd" | "command" => {
                    if cfg!(target_os = "macos") {
                        "Cmd".to_string()
                    } else {
                        "Ctrl".to_string()
                    }
                }
                "meta" | "super" | "win" => {
                    if cfg!(target_os = "macos") {
                        "Cmd".to_string()
                    } else if cfg!(windows) {
                        "Win".to_string()
                    } else {
                        "Super".to_string()
                    }
                }
                "alt" | "option" | "opt" => {
                    if cfg!(target_os = "macos") {
                        "Option".to_string()
                    } else {
                        "Alt".to_string()
                    }
                }
                _ => {
                    let mut c = p.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().chain(c).collect(),
                        None => String::new(),
                    }
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

struct Colors {
    thinking: Color,
    working: Color,
    error: Color,
    done: Color,
    paused: Color,
    stopped: Color,
    cursor: Color,
}

impl Colors {
    fn from(cfg: &OverlayConfig) -> Self {
        let c = |s: &str, d: &str| {
            draw::parse_color(s)
                .or_else(|| draw::parse_color(d))
                .unwrap_or(Color::BLACK)
        };
        Self {
            thinking: c(&cfg.color_thinking, "#D4A017"),
            working: c(&cfg.color_working, "#1E88E5"),
            error: c(&cfg.color_error, "#E53935"),
            done: c(&cfg.color_done, "#2E7D32"),
            paused: c(&cfg.color_paused, "#78909C"),
            stopped: c(&cfg.color_stopped, "#FF6D00"),
            cursor: c(&cfg.cursor_color, "#9C27B0"),
        }
    }
}

struct Glide {
    from: (f64, f64),
    to: (f64, f64),
    start: Instant,
}

const RIPPLE: Duration = Duration::from_millis(450);

/// The overlay's state, independent of any platform.
/// A value easing from one level to another over time.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    from: f32,
    to: f32,
    start: Instant,
    dur: Duration,
}

impl Ramp {
    fn at(v: f32, now: Instant) -> Self {
        Self {
            from: v,
            to: v,
            start: now,
            dur: Duration::ZERO,
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        if self.dur.is_zero() {
            return 1.0;
        }
        (now.saturating_duration_since(self.start).as_secs_f32() / self.dur.as_secs_f32())
            .clamp(0.0, 1.0)
    }

    fn value(&self, now: Instant) -> f32 {
        // Smoothstep: starts and ends gently.
        let t = self.progress(now);
        let e = t * t * (3.0 - 2.0 * t);
        self.from + (self.to - self.from) * e
    }

    fn done(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: f32, y: f32| x + (y - x) * t;
    Color::from_rgba(
        m(a.red(), b.red()),
        m(a.green(), b.green()),
        m(a.blue(), b.blue()),
        m(a.alpha(), b.alpha()),
    )
    .unwrap_or(b)
}

pub struct Machine {
    cfg: OverlayConfig,
    colors: Colors,
    pub phase: Phase,
    since: Instant,
    /// Overall opacity (fade in / fade out).
    fade: Ramp,
    /// Fading out; the overlay goes away when the fade ends.
    leaving: bool,
    /// State colour blend: from `color_from` toward `color_to`.
    color_from: Color,
    color_to: Color,
    color_ramp: Ramp,
    busy: bool,
    last_end: Option<Instant>,
    target: Option<Rect>,
    glide: Option<Glide>,
    click_pending: bool,
    ripple_at: Option<Instant>,
    /// Waiting for the user to stop using the mouse/keyboard.
    paused: bool,
    /// Stopped with the stop key.
    stopped: bool,
    /// The stop key, for the label.
    hotkey: String,
}

impl Machine {
    pub fn new(cfg: OverlayConfig, now: Instant) -> Self {
        let colors = Colors::from(&cfg);
        let start = colors.working;
        Self {
            colors,
            cfg,
            phase: Phase::Off,
            since: now,
            fade: Ramp::at(0.0, now),
            leaving: false,
            color_from: start,
            color_to: start,
            color_ramp: Ramp::at(1.0, now),
            busy: false,
            last_end: None,
            target: None,
            glide: None,
            click_pending: false,
            ripple_at: None,
            paused: false,
            stopped: false,
            hotkey: String::new(),
        }
    }

    /// Whether the agent is stopped (the stop key toggles this).
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    fn set_stopped(&mut self, on: bool, now: Instant) {
        self.stopped = on;
        if on {
            self.set_phase(Phase::Stopped, now);
        } else if self.busy {
            let p = self.active_phase();
            self.set_phase(p, now);
        } else {
            // Nothing running: just go away gently.
            let d = Duration::from_millis(self.cfg.fade_out_ms);
            self.leave(now, d);
        }
    }

    /// A timer moved the state on (no new activity): a fade-out continues.
    fn advance(&mut self, phase: Phase, now: Instant) {
        if self.phase != phase {
            self.phase = phase;
            self.since = now;
        }
        if phase == Phase::Off {
            self.fade = Ramp::at(0.0, now);
            self.leaving = false;
        }
    }

    /// New activity puts the overlay in `phase` (appearing if needed).
    fn set_phase(&mut self, phase: Phase, now: Instant) {
        let appearing = self.phase == Phase::Off && phase != Phase::Off;
        if self.phase != phase {
            self.phase = phase;
            self.since = now;
        }
        if appearing {
            // Start from the new state's colour, no blend.
            let c = self.color();
            (self.color_from, self.color_to) = (c, c);
            self.color_ramp = Ramp::at(1.0, now);
        }
        if phase == Phase::Off {
            self.fade = Ramp::at(0.0, now);
            self.leaving = false;
        } else {
            self.show(now);
        }
    }

    /// Fade in (or back in, if it was fading out).
    fn show(&mut self, now: Instant) {
        if self.fade.to < 1.0 || self.leaving {
            let from = self.fade.value(now);
            self.fade = Ramp {
                from,
                to: 1.0,
                start: now,
                dur: Duration::from_millis(self.cfg.fade_in_ms),
            };
        }
        self.leaving = false;
    }

    /// Fade out; the overlay is gone once the fade ends.
    pub fn leave(&mut self, now: Instant, dur: Duration) {
        if self.phase == Phase::Off || self.leaving {
            return;
        }
        let from = self.fade.value(now);
        self.fade = Ramp {
            from,
            to: 0.0,
            start: now,
            dur,
        };
        self.leaving = true;
    }

    /// The phase that applies while a call runs.
    fn active_phase(&self) -> Phase {
        if self.stopped {
            Phase::Stopped
        } else if self.paused {
            Phase::Paused
        } else {
            Phase::Working
        }
    }

    /// Apply a command.
    pub fn apply(&mut self, cmd: Cmd, now: Instant) {
        match cmd {
            Cmd::Config {
                config,
                hotkey,
                stopped,
                ..
            } => {
                self.colors = Colors::from(&config);
                self.cfg = *config;
                self.hotkey = hotkey;
                if !self.cfg.enabled {
                    self.set_phase(Phase::Off, now);
                }
                if stopped != self.stopped {
                    self.set_stopped(stopped, now);
                }
            }
            Cmd::Paused { on } => {
                self.paused = on;
                if self.busy || on {
                    let p = self.active_phase();
                    self.set_phase(p, now);
                }
            }
            Cmd::Stopped { on } => self.set_stopped(on, now),
            Cmd::Begin => {
                self.busy = true;
                let p = self.active_phase();
                self.set_phase(p, now);
            }
            Cmd::End { ok } => {
                self.busy = false;
                self.paused = false;
                self.last_end = Some(now);
                let p = if self.stopped {
                    Phase::Stopped
                } else if ok {
                    Phase::Thinking
                } else {
                    Phase::Error
                };
                self.set_phase(p, now);
            }
            Cmd::Target { rect } => {
                self.target = rect.map(|r| Rect::new(r[0], r[1], r[2], r[3]));
            }
            Cmd::Pointer { x, y, click } => {
                let from = self.cursor_pos(now).unwrap_or((x, y));
                self.glide = Some(Glide {
                    from,
                    to: (x, y),
                    start: now,
                });
                self.click_pending = click && self.cfg.click_effect;
                if self.phase == Phase::Off {
                    let p = self.active_phase();
                    self.set_phase(p, now);
                }
            }
            Cmd::Status { state } => {
                let p = match state {
                    Status::Thinking => {
                        self.last_end = Some(now);
                        Phase::Thinking
                    }
                    Status::Working => Phase::Working,
                    Status::Done => Phase::Done,
                    Status::Error => {
                        self.last_end = Some(now);
                        Phase::Error
                    }
                    Status::Hidden => {
                        let d = Duration::from_millis(self.cfg.fade_out_ms);
                        self.leave(now, d);
                        return;
                    }
                };
                self.set_phase(p, now);
            }
            Cmd::Hide { .. } | Cmd::Show | Cmd::Quit => {}
        }
    }

    /// Advance timers: error → thinking, idle thinking → done → off.
    pub fn tick(&mut self, now: Instant) {
        let since = now.saturating_duration_since(self.since);
        let idle = self
            .last_end
            .map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
        match self.phase {
            Phase::Error
                if !self.busy && since >= Duration::from_millis(self.cfg.error_hold_ms) =>
            {
                self.advance(Phase::Thinking, now);
            }
            Phase::Thinking
                if !self.busy && idle >= Duration::from_millis(self.cfg.done_after_ms) =>
            {
                self.advance(Phase::Done, now);
            }
            Phase::Done if since >= Duration::from_millis(self.cfg.done_linger_ms) => {
                let d = Duration::from_millis(self.cfg.fade_out_ms);
                self.leave(now, d);
            }
            Phase::Stopped if !self.busy && since >= STOPPED_HOLD => {
                let d = Duration::from_millis(self.cfg.fade_out_ms);
                self.leave(now, d);
            }
            _ => {}
        }
        if self.leaving && self.fade.done(now) {
            self.advance(Phase::Off, now);
            self.glide = None;
        }
        // Blend toward the current state's colour.
        let target = self.color();
        if color_key(target) != color_key(self.color_to) {
            self.color_from = self.shown_color(now);
            self.color_to = target;
            self.color_ramp = Ramp {
                from: 0.0,
                to: 1.0,
                start: now,
                dur: Duration::from_millis(self.cfg.transition_ms),
            };
        }
        // The click ripple starts when the glide arrives.
        if self.click_pending && self.glide_progress(now) >= 1.0 {
            self.click_pending = false;
            self.ripple_at = Some(now);
        }
        if self
            .ripple_at
            .is_some_and(|t| now.saturating_duration_since(t) >= RIPPLE)
        {
            self.ripple_at = None;
        }
    }

    fn glide_progress(&self, now: Instant) -> f64 {
        let Some(g) = &self.glide else { return 1.0 };
        let ms = self.cfg.move_ms.max(1) as f64;
        (now.saturating_duration_since(g.start).as_secs_f64() * 1000.0 / ms).min(1.0)
    }

    fn cursor_pos(&self, now: Instant) -> Option<(f64, f64)> {
        let g = self.glide.as_ref()?;
        let t = self.glide_progress(now);
        let e = 1.0 - (1.0 - t).powi(3); // ease-out
        Some((
            g.from.0 + (g.to.0 - g.from.0) * e,
            g.from.1 + (g.to.1 - g.from.1) * e,
        ))
    }

    /// Something is moving (draw at frame rate).
    pub fn animating(&self, now: Instant) -> bool {
        self.phase != Phase::Off
            && (self.glide_progress(now) < 1.0
                || self.click_pending
                || self.ripple_at.is_some()
                || !self.fade.done(now)
                || !self.color_ramp.done(now))
    }

    /// The state colour as currently shown (mid-blend while changing).
    fn shown_color(&self, now: Instant) -> Color {
        mix(self.color_from, self.color_to, self.color_ramp.value(now))
    }

    fn color(&self) -> Color {
        let c = &self.colors;
        match self.phase {
            Phase::Off | Phase::Working => c.working,
            Phase::Thinking => c.thinking,
            Phase::Error => c.error,
            Phase::Done => c.done,
            Phase::Paused => c.paused,
            Phase::Stopped => c.stopped,
        }
    }

    fn label_text(&self) -> String {
        let cfg = &self.cfg;
        let template = match self.phase {
            Phase::Off | Phase::Working => &cfg.label_working,
            Phase::Thinking => &cfg.label_thinking,
            Phase::Error => &cfg.label_error,
            Phase::Done => &cfg.label_done,
            Phase::Paused => &cfg.label_paused,
            Phase::Stopped => &cfg.label_stopped,
        };
        template.replace("{hotkey}", &pretty_key(&self.hotkey))
    }

    /// What should be on screen now.
    pub fn scene(&self, now: Instant) -> Scene {
        if self.phase == Phase::Off || !self.cfg.enabled {
            return Scene::default();
        }
        let color = self.shown_color(now);
        let ripple = self.ripple_at.map(|t| {
            (now.saturating_duration_since(t).as_secs_f32() / RIPPLE.as_secs_f32()).min(0.999)
        });
        Scene {
            opacity: self.fade.value(now).clamp(0.0, 1.0),
            border: self.cfg.show_border.then_some((self.target, color)),
            label: self.cfg.show_label.then(|| (self.label_text(), color)),
            cursor: if self.cfg.show_cursor {
                self.cursor_pos(now).map(|p| CursorLook {
                    pos: p,
                    ring: color,
                    body: self.colors.cursor,
                    ripple,
                })
            } else {
                None
            },
        }
    }

    pub fn config(&self) -> &OverlayConfig {
        &self.cfg
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorLook {
    pub pos: (f64, f64),
    pub ring: Color,
    pub body: Color,
    pub ripple: Option<f32>,
}

/// What is on screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// Overall opacity, 0–1 (fading in or out).
    pub opacity: f32,
    /// Target rect (None = screen) and colour.
    pub border: Option<(Option<Rect>, Color)>,
    pub label: Option<(String, Color)>,
    pub cursor: Option<CursorLook>,
}

fn color_key(c: Color) -> [u8; 4] {
    let c = c.to_color_u8();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

/// Turns scenes into surface calls, redrawing only what changed.
/// What the border was drawn with: band rects, colour, core width, opacity.
type BorderKey = ([i64; 16], [u8; 4], u32, u8);
/// What the label was drawn with: text, colour, opacity, and its position.
type LabelKey = (String, [u8; 4], u8, i64, i64);
/// What the cursor image was drawn with: ring, body, ripple, opacity.
type CursorKey = ([u8; 4], [u8; 4], i32, u8);

#[derive(Default)]
pub struct Painter {
    border: Option<BorderKey>,
    label: Option<LabelKey>,
    label_size: (f64, f64),
    cursor_img: Option<CursorKey>,
    cursor_pos: Option<(i64, i64)>,
    /// Where the arrow tip sits in the cursor image (screen units).
    cursor_hot: (f64, f64),
    /// Whether the surface fades natively, and the opacity last given to it.
    native_fade: Option<bool>,
    opacity: f32,
}

/// Scale an image's (premultiplied) pixels by `a`.
fn faded(mut pm: Pixmap, a: f32) -> Pixmap {
    if a < 0.999 {
        for v in pm.data_mut() {
            *v = (f32::from(*v) * a).round() as u8;
        }
    }
    pm
}

impl Painter {
    pub fn paint(
        &mut self,
        scene: &Scene,
        cfg: &OverlayConfig,
        fonts: &Fonts,
        s: &mut dyn Surface,
    ) {
        let scale = draw::sane_scale(if cfg.scale > 0.0 {
            cfg.scale as f32
        } else {
            s.render_scale()
        });
        let ppu = s.px_per_unit().max(0.1);
        let screen = s.screen();
        let main = s.main_screen();

        // Fading: natively where the platform can, else by redrawing.
        let o = (scene.opacity * 48.0).round() / 48.0;
        match self.native_fade {
            None => {
                self.native_fade = Some(s.set_opacity(o));
                self.opacity = o;
            }
            Some(true) if o != self.opacity => {
                s.set_opacity(o);
                self.opacity = o;
            }
            _ => {}
        }
        let alpha = if self.native_fade == Some(true) {
            1.0
        } else {
            o
        };
        let ak = (alpha * 48.0) as u8;

        // Border: a glow along the screen edges fading inward, or around the
        // target window (outside it where there is room, else inside).
        let unit = f64::from(scale) / f64::from(ppu);
        let core = (f64::from(cfg.border_width.max(1)) * unit).max(1.0);
        let band = (f64::from(cfg.glow_size) * unit).max(core);
        let px = |v: f64| ((v * f64::from(ppu)).round().max(1.0)) as u32;
        let window_mode = cfg.border_target == crate::config::BorderTarget::Window;
        let area = |target: Option<Rect>| match target {
            Some(r) if window_mode => r,
            _ => main,
        };
        // Where the label goes: centred on `cx`, above `top` if it fits, else at `inside`.
        let mut place = (
            main.x + main.width / 2.0,
            main.y,
            main.y + s.top_inset() + core + 10.0,
        );
        let bands = scene.border.map(|(target, color)| {
            let r = area(target);
            let (sx, sy, sr, sb) = (
                screen.x,
                screen.y,
                screen.x + screen.width,
                screen.y + screen.height,
            );
            let (rr, rb) = (r.x + r.width, r.y + r.height);
            let edges: [(f64, f64, f64, f64, u8); 4] = if r == main {
                let (mx, my, mr, mb) = (main.x, main.y, main.x + main.width, main.y + main.height);
                [
                    (mx, my, main.width, band, 0),
                    (mr - band, my, band, main.height, 1),
                    (mx, mb - band, main.width, band, 2),
                    (mx, my, band, main.height, 3),
                ]
            } else {
                let room = |v: f64| v >= band / 2.0;
                let top = if room(r.y - sy) {
                    let t = band.min(r.y - sy);
                    (r.x - band, r.y - t, r.width + 2.0 * band, t, 2)
                } else {
                    (r.x, r.y, r.width, band, 0)
                };
                let bottom = if room(sb - rb) {
                    (r.x - band, rb, r.width + 2.0 * band, band.min(sb - rb), 0)
                } else {
                    (r.x, rb - band, r.width, band, 2)
                };
                let left = if room(r.x - sx) {
                    let t = band.min(r.x - sx);
                    (r.x - t, r.y, t, r.height, 1)
                } else {
                    (r.x, r.y, band, r.height, 3)
                };
                let right = if room(sr - rr) {
                    (rr, r.y, band.min(sr - rr), r.height, 3)
                } else {
                    (rr - band, r.y, band, r.height, 1)
                };
                place = (r.x + r.width / 2.0, top.1, r.y + core + 8.0);
                [top, right, bottom, left]
            };
            // Keep every band on screen.
            let edges = edges.map(|(x, y, w, h, strong)| {
                let (x0, y0) = (x.max(sx), y.max(sy));
                let (x1, y1) = ((x + w).min(sr), (y + h).min(sb));
                (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0), strong)
            });
            (edges, color)
        });
        match bands {
            Some((edges, color)) => {
                let mut key = [0i64; 16];
                for (i, e) in edges.iter().enumerate() {
                    key[i * 4..i * 4 + 4]
                        .copy_from_slice(&[e.0 as i64, e.1 as i64, e.2 as i64, e.3 as i64]);
                }
                let key = (key, color_key(color), (core * 100.0) as u32, ak);
                if self.border != Some(key) {
                    let core_px = (core * f64::from(ppu)) as f32;
                    for (layer, (x, y, w, h, strong)) in EDGES.iter().zip(edges) {
                        if w < 1.0 || h < 1.0 {
                            s.hide(*layer);
                            continue;
                        }
                        let img = faded(draw::edge(px(w), px(h), color, strong, core_px), alpha);
                        s.show(*layer, &img, x, y);
                    }
                    self.border = Some(key);
                }
            }
            None => {
                if self.border.take().is_some() {
                    for l in EDGES {
                        s.hide(l);
                    }
                }
            }
        }

        // Label: centred over the target, above it when there is room.
        match &scene.label {
            Some((text, color)) if !text.is_empty() => {
                let key = (text.clone(), color_key(*color), ak);
                let redraw = self
                    .label
                    .as_ref()
                    .is_none_or(|(t, c, a, _, _)| (t, c, *a) != (&key.0, &key.1, key.2));
                let mut img = None;
                if redraw {
                    let pm = faded(draw::label(fonts, text, scale, *color), alpha);
                    self.label_size = (
                        f64::from(pm.width()) / f64::from(ppu),
                        f64::from(pm.height()) / f64::from(ppu),
                    );
                    img = Some(pm);
                }
                let (lw, lh) = self.label_size;
                let (cx, top, inside) = place;
                let x = (cx - lw / 2.0)
                    .min(screen.x + screen.width - lw)
                    .max(screen.x);
                let above = top - lh - 4.0;
                let y = if above >= screen.y { above } else { inside };
                let pos = (x.round() as i64, y.round() as i64);
                match img {
                    Some(pm) => s.show(Layer::Label, &pm, x, y),
                    None => {
                        if self.label.as_ref().is_some_and(|l| (l.3, l.4) != pos) {
                            s.move_to(Layer::Label, x, y);
                        }
                    }
                }
                self.label = Some((key.0, key.1, key.2, pos.0, pos.1));
            }
            _ => {
                if self.label.take().is_some() {
                    s.hide(Layer::Label);
                }
            }
        }

        // Cursor: redraw the image only when its look changes; move it cheaply.
        match scene.cursor {
            Some(c) => {
                let key = (
                    color_key(c.ring),
                    color_key(c.body),
                    c.ripple.map_or(-1, |r| (r * 30.0) as i32),
                    ak,
                );
                let redraw = self.cursor_img != Some(key);
                let art = redraw
                    .then(|| draw::cursor(fonts, &cfg.cursor_tag, scale, c.body, c.ring, c.ripple));
                if let Some(a) = &art {
                    self.cursor_hot = (
                        f64::from(a.hotspot.0) / f64::from(ppu),
                        f64::from(a.hotspot.1) / f64::from(ppu),
                    );
                }
                let (x, y) = (c.pos.0 - self.cursor_hot.0, c.pos.1 - self.cursor_hot.1);
                let pos = (x.round() as i64, y.round() as i64);
                if let Some(a) = art {
                    let img = faded(a.image, alpha);
                    s.show(Layer::Cursor, &img, x, y);
                    self.cursor_img = Some(key);
                } else if self.cursor_pos != Some(pos) {
                    s.move_to(Layer::Cursor, x, y);
                }
                self.cursor_pos = Some(pos);
            }
            None => {
                if self.cursor_img.take().is_some() {
                    self.cursor_pos = None;
                    s.hide(Layer::Cursor);
                }
            }
        }
    }

    /// Draw everything again on the next paint (e.g. after new settings).
    /// What is on screen stays until then rather than being hidden first:
    /// hiding and showing again in one go can flicker, and replays a
    /// compositor's open/close animation.
    pub fn redraw(&mut self) {
        // Keys no scene matches: each part is drawn anew, or hidden if it
        // is no longer wanted.
        if self.border.is_some() {
            self.border = Some(([i64::MIN; 16], [0; 4], 0, 0));
        }
        if self.label.is_some() {
            self.label = Some((String::new(), [0; 4], 0, i64::MIN, i64::MIN));
        }
        if self.cursor_img.is_some() {
            self.cursor_img = Some(([0; 4], [0; 4], i32::MIN, 0));
        }
    }

    /// Anything on screen?
    pub fn showing(&self) -> bool {
        self.border.is_some() || self.label.is_some() || self.cursor_img.is_some()
    }
}

enum Input {
    Cmd(Cmd),
    Eof,
}

fn reply(r: &Reply) {
    if let Ok(line) = serde_json::to_string(r) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// Entry point of `computer-use-mcp overlay [--parent PID] [--demo]`.
pub fn run(args: &[String]) -> i32 {
    let parent = args
        .iter()
        .position(|a| a == "--parent")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u32>().ok());
    let demo = args.iter().any(|a| a == "--demo");
    // AppKit calls can block the main loop: exit even then.
    #[cfg(target_os = "macos")]
    super::macos::start_watchdog(parent);

    let (tx, rx) = mpsc::channel::<Input>();
    if demo {
        std::thread::spawn(move || demo_script(tx));
    } else {
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if let Ok(cmd) = serde_json::from_str::<Cmd>(&line) {
                    let quit = cmd == Cmd::Quit;
                    if tx.send(Input::Cmd(cmd)).is_err() || quit {
                        #[cfg(target_os = "macos")]
                        super::macos::input_closed();
                        return;
                    }
                }
            }
            #[cfg(target_os = "macos")]
            super::macos::input_closed();
            let _ = tx.send(Input::Eof);
        });
    }

    let mut surface: Box<dyn Surface> = match open_surface() {
        Ok(s) => s,
        Err(e) => {
            // No display: stay quiet so the engine carries on without us.
            eprintln!("overlay unavailable: {e}");
            reply(&Reply::Ready {
                excluded: true,
                available: false,
            });
            // The stop key needs the same display connection, so it can't
            // be listened for either: say so, so the engine can tell the user.
            while let Ok(Input::Cmd(cmd)) = rx.recv() {
                if let Cmd::Config {
                    hotkey,
                    settings_key,
                    ..
                } = cmd
                {
                    if !hotkey.trim().is_empty() {
                        reply(&Reply::Hotkey {
                            key: hotkey,
                            ok: false,
                        });
                    }
                    if !settings_key.trim().is_empty() {
                        reply(&Reply::SettingsKey {
                            key: settings_key,
                            ok: false,
                        });
                    }
                }
            }
            return 1;
        }
    };
    reply(&Reply::Ready {
        excluded: surface.excluded_from_capture(),
        available: true,
    });

    let mut machine = Machine::new(OverlayConfig::default(), Instant::now());
    let mut fonts = Fonts::load("");
    let mut font_path = String::new();
    let mut painter = Painter::default();
    let mut hidden = false;
    let mut hotkey_now = String::new();
    let mut settings_now = String::new();
    /// Within this of the previous one, a key press is key repeat.
    const HOTKEY_QUIET: Duration = Duration::from_millis(400);
    let mut last_hotkey: Option<Instant> = None;
    let mut last_settings: Option<Instant> = None;
    let mut last_parent_check = Instant::now();
    // Set once told to stop: fade out, then exit.
    let mut quitting: Option<Instant> = None;
    let quit_fade = |m: &Machine| Duration::from_millis(m.config().fade_out_ms.min(700));

    loop {
        let now = Instant::now();
        let wait = if machine.animating(now) {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(100)
        };
        let mut first = if quitting.is_some() {
            std::thread::sleep(Duration::from_millis(16));
            None
        } else {
            match rx.recv_timeout(wait) {
                Ok(i) => Some(i),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => Some(Input::Eof),
            }
        };
        while quitting.is_none() {
            let input = match first.take() {
                Some(i) => i,
                None => match rx.try_recv() {
                    Ok(i) => i,
                    Err(_) => break,
                },
            };
            let cmd = match input {
                Input::Eof | Input::Cmd(Cmd::Quit) => {
                    // Never vanish abruptly: fade out first.
                    quitting = Some(Instant::now());
                    let d = quit_fade(&machine);
                    machine.leave(Instant::now(), d);
                    break;
                }
                Input::Cmd(c) => c,
            };
            match cmd {
                Cmd::Hide { id } => {
                    let shown = painter.showing() && !hidden;
                    if shown {
                        surface.set_hidden(true);
                    }
                    hidden = true;
                    reply(&Reply::Hidden { id, shown });
                }
                Cmd::Show => {
                    if hidden {
                        surface.set_hidden(false);
                    }
                    hidden = false;
                }
                other => {
                    if let Cmd::Config {
                        config,
                        hotkey,
                        settings_key,
                        ..
                    } = &other
                    {
                        if config.font != font_path {
                            font_path = config.font.clone();
                            fonts = Fonts::load(&font_path);
                        }
                        if *hotkey != hotkey_now {
                            hotkey_now = hotkey.clone();
                            let combo = crate::keys::parse_combo(hotkey.trim()).ok();
                            let ok = surface.set_hotkey(Hotkey::Stop, combo);
                            if !hotkey.trim().is_empty() {
                                reply(&Reply::Hotkey {
                                    key: hotkey.clone(),
                                    ok,
                                });
                            }
                        }
                        if *settings_key != settings_now {
                            settings_now = settings_key.clone();
                            let combo = crate::keys::parse_combo(settings_key.trim()).ok();
                            let ok = surface.set_hotkey(Hotkey::Settings, combo);
                            if !settings_key.trim().is_empty() {
                                reply(&Reply::SettingsKey {
                                    key: settings_key.clone(),
                                    ok,
                                });
                            }
                        }
                        // Redraw everything with the new settings.
                        painter.redraw();
                    }
                    machine.apply(other, Instant::now());
                }
            }
        }

        for ev in surface.pump() {
            match ev {
                SurfaceEvent::Hotkey(Hotkey::Settings) => {
                    let repeat = last_settings.is_some_and(|t| t.elapsed() < HOTKEY_QUIET);
                    last_settings = Some(Instant::now());
                    if quitting.is_none() && !repeat {
                        reply(&Reply::Settings);
                    }
                }
                SurfaceEvent::Hotkey(Hotkey::Stop) => {
                    // Presses in quick succession are key repeat, not a
                    // second press. Every press restarts the quiet time, so
                    // holding the key down can't toggle stop off again.
                    let repeat = last_hotkey.is_some_and(|t| t.elapsed() < HOTKEY_QUIET);
                    last_hotkey = Some(Instant::now());
                    if quitting.is_none() && !repeat {
                        // Shown at once, whatever the engine is doing.
                        let on = !machine.stopped();
                        machine.apply(Cmd::Stopped { on }, Instant::now());
                        reply(&Reply::Stop { on });
                    }
                }
            }
        }
        if let Some(pid) = parent
            && last_parent_check.elapsed() >= Duration::from_millis(500)
        {
            last_parent_check = Instant::now();
            if !super::process_alive(pid) && quitting.is_none() {
                quitting = Some(Instant::now());
                let d = quit_fade(&machine);
                machine.leave(Instant::now(), d);
            }
        }
        let now = Instant::now();
        machine.tick(now);
        let scene = machine.scene(now);
        painter.paint(&scene, machine.config(), &fonts, surface.as_mut());
        if let Some(t) = quitting
            && (machine.phase == Phase::Off || t.elapsed() > Duration::from_secs(2))
        {
            break;
        }
    }
    surface.close();
    0
}

fn open_surface() -> Result<Box<dyn Surface>, String> {
    #[cfg(target_os = "linux")]
    {
        // Wayland compositors with layer-shell (Hyprland, sway, KDE…) get
        // a native overlay; the rest (GNOME, X11 desktops) the X11 one.
        if crate::linux::wayland::session() {
            match super::wayland::WaylandSurface::open() {
                Ok(s) => return Ok(Box::new(s)),
                Err(e) => eprintln!("overlay: no Wayland overlay ({e}); trying X11"),
            }
        }
        super::linux::X11Surface::open().map(|s| Box::new(s) as Box<dyn Surface>)
    }
    #[cfg(target_os = "macos")]
    {
        super::macos::MacSurface::open().map(|s| Box::new(s) as Box<dyn Surface>)
    }
    #[cfg(target_os = "windows")]
    {
        super::windows::WinSurface::open().map(|s| Box::new(s) as Box<dyn Surface>)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err("no overlay on this platform".into())
    }
}

/// `--demo`: walk through every state so the look can be checked.
fn demo_script(tx: mpsc::Sender<Input>) {
    let send = |c: Cmd| tx.send(Input::Cmd(c)).is_ok();
    let pause = |ms: u64| std::thread::sleep(Duration::from_millis(ms));
    let mut cfg = OverlayConfig {
        done_after_ms: 1500,
        ..OverlayConfig::default()
    };
    if let Ok(label) = std::env::var("OVERLAY_DEMO_LABEL") {
        cfg.label_working = label;
    }
    send(Cmd::Config {
        config: Box::new(cfg),
        hotkey: "ctrl+alt+escape".into(),
        settings_key: String::new(),
        stopped: false,
    });
    send(Cmd::Target {
        rect: Some([200.0, 160.0, 640.0, 420.0]),
    });
    send(Cmd::Begin);
    for (x, y, click) in [
        (300.0, 260.0, false),
        (520.0, 300.0, true),
        (700.0, 480.0, true),
    ] {
        send(Cmd::Pointer { x, y, click });
        pause(900);
    }
    send(Cmd::End { ok: true });
    pause(1500);
    send(Cmd::Begin);
    send(Cmd::Pointer {
        x: 600.0,
        y: 520.0,
        click: true,
    });
    pause(1500);
    send(Cmd::End { ok: false });
    pause(2800);
    send(Cmd::Begin);
    send(Cmd::Paused { on: true });
    pause(1500);
    send(Cmd::Paused { on: false });
    send(Cmd::End { ok: true });
    send(Cmd::Stopped { on: true });
    pause(1800);
    send(Cmd::Stopped { on: false });
    pause(600);
    send(Cmd::Status {
        state: Status::Done,
    });
    pause(2000);
    let _ = tx.send(Input::Eof);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OverlayConfig {
        OverlayConfig {
            done_after_ms: 1000,
            done_linger_ms: 500,
            error_hold_ms: 300,
            move_ms: 100,
            fade_in_ms: 100,
            fade_out_ms: 400,
            transition_ms: 100,
            ..OverlayConfig::default()
        }
    }

    #[test]
    fn states_follow_the_work() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut m = Machine::new(cfg(), t0);
        assert_eq!(m.scene(t0), Scene::default(), "hidden until used");

        m.apply(Cmd::Begin, at(0));
        assert_eq!(m.phase, Phase::Working);
        m.apply(Cmd::End { ok: true }, at(10));
        assert_eq!(m.phase, Phase::Thinking);
        m.tick(at(500));
        assert_eq!(m.phase, Phase::Thinking);
        m.tick(at(1100));
        assert_eq!(m.phase, Phase::Done, "idle → done");
        m.tick(at(1700));
        assert_eq!(m.phase, Phase::Done, "fading out, not gone at once");
        let fading = m.scene(at(1900)).opacity;
        assert!(fading > 0.0 && fading < 1.0, "{fading}");
        m.tick(at(2150));
        assert_eq!(m.phase, Phase::Off, "done → faded → gone");
        assert_eq!(m.scene(at(2150)), Scene::default());

        // Errors show red for a while, then back to thinking.
        m.apply(Cmd::Begin, at(2000));
        m.apply(Cmd::End { ok: false }, at(2010));
        assert_eq!(m.phase, Phase::Error);
        m.tick(at(2400));
        assert_eq!(m.phase, Phase::Thinking);

        // Explicit done from the host.
        m.apply(
            Cmd::Status {
                state: Status::Done,
            },
            at(3300),
        );
        assert_eq!(m.phase, Phase::Done);
    }

    #[test]
    fn pause_and_stop_show_and_clear() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut m = Machine::new(cfg(), t0);
        m.apply(
            Cmd::Config {
                config: Box::new(cfg()),
                hotkey: "ctrl+alt+escape".into(),
                settings_key: String::new(),
                stopped: false,
            },
            t0,
        );
        m.apply(Cmd::Begin, t0);
        m.apply(Cmd::Paused { on: true }, at(10));
        assert_eq!(m.phase, Phase::Paused);
        assert!(m.scene(at(10)).label.unwrap().0.contains("Paused"));
        m.apply(Cmd::Paused { on: false }, at(20));
        assert_eq!(m.phase, Phase::Working);

        // The stop key: shown at once, stays through the end of the call.
        m.apply(Cmd::Stopped { on: true }, at(30));
        assert_eq!(m.phase, Phase::Stopped);
        let label = m.scene(at(30)).label.unwrap().0;
        assert!(label.contains(&pretty_key("ctrl+alt+escape")), "{label}");
        m.apply(Cmd::End { ok: false }, at(40));
        assert_eq!(m.phase, Phase::Stopped);
        assert!(m.stopped());
        // Fades out after a while, but stays stopped.
        m.tick(at(40) + STOPPED_HOLD);
        m.tick(at(60) + STOPPED_HOLD + Duration::from_millis(500));
        assert_eq!(m.phase, Phase::Off);
        assert!(m.stopped());
        // Any new call shows it again.
        m.apply(Cmd::Begin, at(7000));
        assert_eq!(m.phase, Phase::Stopped);
        m.apply(Cmd::Stopped { on: false }, at(7100));
        assert_eq!(m.phase, Phase::Working);
        assert!(!m.stopped());
        assert_eq!(pretty_key("ctrl+shift+f12"), "Ctrl+Shift+F12");
    }

    #[test]
    fn cursor_glides_then_ripples() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut m = Machine::new(cfg(), t0);
        m.apply(Cmd::Begin, t0);
        m.apply(
            Cmd::Pointer {
                x: 10.0,
                y: 10.0,
                click: false,
            },
            t0,
        );
        m.apply(
            Cmd::Pointer {
                x: 110.0,
                y: 10.0,
                click: true,
            },
            at(0),
        );
        let mid = m.scene(at(50)).cursor.unwrap();
        assert!(mid.pos.0 > 10.0 && mid.pos.0 < 110.0, "{:?}", mid.pos);
        assert!(mid.ripple.is_none(), "no ripple before arriving");
        m.tick(at(120));
        let end = m.scene(at(130)).cursor.unwrap();
        assert_eq!(end.pos, (110.0, 10.0));
        assert!(end.ripple.is_some(), "ripple on arrival");
        m.tick(at(700));
        assert!(m.scene(at(700)).cursor.unwrap().ripple.is_none());
    }

    #[test]
    fn disabled_parts_are_left_out() {
        let t0 = Instant::now();
        let mut m = Machine::new(
            OverlayConfig {
                show_cursor: false,
                show_label: false,
                ..cfg()
            },
            t0,
        );
        m.apply(Cmd::Begin, t0);
        m.apply(
            Cmd::Pointer {
                x: 1.0,
                y: 1.0,
                click: true,
            },
            t0,
        );
        let s = m.scene(t0);
        assert!(s.cursor.is_none() && s.label.is_none() && s.border.is_some());
    }

    #[test]
    fn fades_in_and_out_instead_of_popping() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut m = Machine::new(cfg(), t0);
        m.apply(Cmd::Begin, t0);
        assert!(m.scene(t0).opacity < 0.05, "starts transparent");
        assert!((m.scene(at(150)).opacity - 1.0).abs() < 1e-3, "fully in");

        // Hidden: fades out, then is gone.
        m.apply(
            Cmd::Status {
                state: Status::Hidden,
            },
            at(200),
        );
        m.tick(at(300));
        let mid = m.scene(at(400)).opacity;
        assert!(mid > 0.2 && mid < 0.8, "{mid}");
        assert!(m.animating(at(400)));
        m.tick(at(700));
        assert_eq!(m.phase, Phase::Off);

        // Coming back while fading reverses the fade.
        m.apply(Cmd::Begin, at(800));
        m.apply(
            Cmd::Status {
                state: Status::Hidden,
            },
            at(1000),
        );
        m.apply(Cmd::Begin, at(1100));
        m.tick(at(1300));
        assert_ne!(m.phase, Phase::Off);
        assert!((m.scene(at(1300)).opacity - 1.0).abs() < 1e-3);
    }

    #[test]
    fn colours_blend_between_states() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut m = Machine::new(cfg(), t0);
        m.apply(Cmd::Begin, t0);
        m.tick(at(10));
        let blue = m.scene(at(10)).border.unwrap().1;
        m.apply(Cmd::End { ok: false }, at(20));
        m.tick(at(20));
        let mid = m.scene(at(70)).border.unwrap().1;
        let red = m.scene(at(200)).border.unwrap().1;
        assert!(mid.red() > blue.red() && mid.red() < red.red(), "{mid:?}");
    }

    /// Records surface calls.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
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
            self.calls.push(format!(
                "show {layer:?} {}x{} @{x:.0},{y:.0}",
                img.width(),
                img.height()
            ));
        }
        fn move_to(&mut self, layer: Layer, x: f64, y: f64) {
            self.calls.push(format!("move {layer:?} @{x:.0},{y:.0}"));
        }
        fn hide(&mut self, layer: Layer) {
            self.calls.push(format!("hide {layer:?}"));
        }
        fn set_hidden(&mut self, _: bool) {}
        fn pump(&mut self) -> Vec<SurfaceEvent> {
            Vec::new()
        }
        fn close(&mut self) {}
    }

    #[test]
    fn painter_draws_border_outside_the_target_and_only_on_change() {
        let t0 = Instant::now();
        let mut m = Machine::new(
            OverlayConfig {
                border_target: crate::config::BorderTarget::Window,
                ..cfg()
            },
            t0,
        );
        m.apply(
            Cmd::Target {
                rect: Some([100.0, 100.0, 400.0, 300.0]),
            },
            t0,
        );
        m.apply(Cmd::Begin, t0);
        let mut s = Fake::default();
        let mut p = Painter::default();
        let fonts = Fonts::default();
        p.paint(&m.scene(t0), m.config(), &fonts, &mut s);
        // A 36 px glow above the window, spanning past its corners.
        assert!(
            s.calls.contains(&"show Top 472x36 @64,64".to_string()),
            "{:?}",
            s.calls
        );
        assert!(s.calls.iter().any(|c| c.starts_with("show Label")));
        let n = s.calls.len();
        p.paint(&m.scene(t0), m.config(), &fonts, &mut s);
        assert_eq!(s.calls.len(), n, "nothing changed, nothing redrawn");
        m.apply(Cmd::End { ok: true }, t0);
        let later = t0 + Duration::from_millis(500);
        m.tick(later);
        p.paint(&m.scene(later), m.config(), &fonts, &mut s);
        assert!(
            s.calls[n..].iter().any(|c| c.starts_with("show Top")),
            "recoloured"
        );
        m.apply(
            Cmd::Status {
                state: Status::Hidden,
            },
            later,
        );
        let gone = later + Duration::from_millis(600);
        m.tick(gone);
        p.paint(&m.scene(gone), m.config(), &fonts, &mut s);
        assert!(!p.showing());
    }

    #[test]
    fn new_settings_redraw_in_place_without_hiding_first() {
        let t0 = Instant::now();
        let mut m = Machine::new(cfg(), t0);
        m.apply(Cmd::Begin, t0);
        m.apply(
            Cmd::Pointer {
                x: 50.0,
                y: 50.0,
                click: false,
            },
            t0,
        );
        let later = t0 + Duration::from_millis(500);
        m.tick(later);
        let (mut s, mut p, fonts) = (Fake::default(), Painter::default(), Fonts::default());
        p.paint(&m.scene(later), m.config(), &fonts, &mut s);
        let n = s.calls.len();

        p.redraw();
        p.paint(&m.scene(later), m.config(), &fonts, &mut s);
        let again = &s.calls[n..];
        assert!(again.iter().all(|c| c.starts_with("show")), "{again:?}");
        for part in ["Top", "Left", "Label", "Cursor"] {
            assert!(again.iter().any(|c| c.contains(part)), "{part}: {again:?}");
        }

        // A part switched off by the new settings goes away.
        let n = s.calls.len();
        p.redraw();
        let no_label = OverlayConfig {
            show_label: false,
            ..cfg()
        };
        m.apply(
            Cmd::Config {
                config: Box::new(no_label),
                hotkey: String::new(),
                settings_key: String::new(),
                stopped: false,
            },
            later,
        );
        p.paint(&m.scene(later), m.config(), &fonts, &mut s);
        assert!(s.calls[n..].contains(&"hide Label".to_string()));
    }

    #[test]
    fn screen_glow_runs_along_the_screen_edges() {
        let t0 = Instant::now();
        let mut m = Machine::new(cfg(), t0);
        m.apply(Cmd::Begin, t0);
        let mut s = Fake::default();
        Painter::default().paint(&m.scene(t0), m.config(), &Fonts::default(), &mut s);
        for want in [
            "show Top 1280x36 @0,0",
            "show Right 36x800 @1244,0",
            "show Bottom 1280x36 @0,764",
            "show Left 36x800 @0,0",
        ] {
            assert!(s.calls.contains(&want.to_string()), "{want}: {:?}", s.calls);
        }
    }
}
