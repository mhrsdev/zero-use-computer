//! The overlay helper process: reads [`Cmd`]s from stdin, keeps the state
//! (colours, timers, cursor glide, click ripple) and draws it on a native
//! [`Surface`]. It exits as soon as stdin closes or its parent is gone.

use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tiny_skia::{Color, Pixmap};

use super::draw::{self, CURSOR_BOX};
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

/// The texts of an on-screen confirmation.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub title: String,
    pub message: String,
    pub allow: String,
    pub deny: String,
}

/// A platform's way of putting images on screen: always on top,
/// click-through, never activated, and (where the OS allows) left out of
/// screen captures. Coordinates are the engine's screen units.
pub trait Surface {
    /// Whether screen captures leave the overlay out by themselves.
    fn excluded_from_capture(&self) -> bool;
    /// The main screen, in screen units.
    fn screen(&self) -> Rect;
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
    /// Ask the user to allow something; the answer comes back from `pump`.
    fn confirm(&mut self, id: u64, ask: &Ask);
    /// Handle native events; returns answered confirmations.
    fn pump(&mut self) -> Vec<(u64, bool)>;
    fn close(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Off,
    Thinking,
    Working,
    Approval,
    Danger,
    Error,
    Done,
}

struct Colors {
    thinking: Color,
    working: Color,
    approval: Color,
    danger: Color,
    error: Color,
    done: Color,
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
            approval: c(&cfg.color_approval, "#FFE600"),
            danger: c(&cfg.color_danger, "#000000"),
            error: c(&cfg.color_error, "#E53935"),
            done: c(&cfg.color_done, "#2E7D32"),
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
pub struct Machine {
    cfg: OverlayConfig,
    colors: Colors,
    pub phase: Phase,
    since: Instant,
    busy: bool,
    last_end: Option<Instant>,
    danger: Option<String>,
    approval: Option<String>,
    target: Option<Rect>,
    glide: Option<Glide>,
    click_pending: bool,
    ripple_at: Option<Instant>,
}

impl Machine {
    pub fn new(cfg: OverlayConfig, now: Instant) -> Self {
        Self {
            colors: Colors::from(&cfg),
            cfg,
            phase: Phase::Off,
            since: now,
            busy: false,
            last_end: None,
            danger: None,
            approval: None,
            target: None,
            glide: None,
            click_pending: false,
            ripple_at: None,
        }
    }

    fn set_phase(&mut self, phase: Phase, now: Instant) {
        if self.phase != phase {
            self.phase = phase;
            self.since = now;
        }
    }

    /// The phase that applies while a call runs.
    fn active_phase(&self) -> Phase {
        if self.approval.is_some() {
            Phase::Approval
        } else if self.danger.is_some() {
            Phase::Danger
        } else {
            Phase::Working
        }
    }

    /// Apply a command. Returns a confirmation to ask on screen, if any.
    pub fn apply(&mut self, cmd: Cmd, now: Instant) -> Option<(u64, String)> {
        match cmd {
            Cmd::Config { config } => {
                self.colors = Colors::from(&config);
                self.cfg = *config;
                if !self.cfg.enabled {
                    self.set_phase(Phase::Off, now);
                }
            }
            Cmd::Begin => {
                self.busy = true;
                let p = self.active_phase();
                self.set_phase(p, now);
            }
            Cmd::End { ok } => {
                self.busy = false;
                self.danger = None;
                self.approval = None;
                self.last_end = Some(now);
                self.set_phase(if ok { Phase::Thinking } else { Phase::Error }, now);
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
            Cmd::Danger { action } => {
                self.danger = action;
                if self.busy {
                    let p = self.active_phase();
                    self.set_phase(p, now);
                }
            }
            Cmd::Approval { id, action, ask } => {
                self.approval = Some(action.clone());
                self.set_phase(Phase::Approval, now);
                if ask {
                    return Some((id, action));
                }
            }
            Cmd::ApprovalDone => {
                self.approval = None;
                let p = if self.busy {
                    self.active_phase()
                } else {
                    Phase::Thinking
                };
                self.set_phase(p, now);
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
                    Status::Hidden => Phase::Off,
                };
                self.set_phase(p, now);
            }
            Cmd::Hide { .. } | Cmd::Show | Cmd::Quit => {}
        }
        None
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
                self.set_phase(Phase::Thinking, now);
            }
            Phase::Thinking
                if !self.busy && idle >= Duration::from_millis(self.cfg.done_after_ms) =>
            {
                self.set_phase(Phase::Done, now);
            }
            Phase::Done if since >= Duration::from_millis(self.cfg.done_linger_ms) => {
                self.set_phase(Phase::Off, now);
                self.glide = None;
            }
            _ => {}
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
                || self.phase == Phase::Approval)
    }

    fn color(&self) -> Color {
        let c = &self.colors;
        match self.phase {
            Phase::Off | Phase::Working => c.working,
            Phase::Thinking => c.thinking,
            Phase::Approval => c.approval,
            Phase::Danger => c.danger,
            Phase::Error => c.error,
            Phase::Done => c.done,
        }
    }

    fn label_text(&self) -> String {
        let cfg = &self.cfg;
        let (template, action) = match self.phase {
            Phase::Off | Phase::Working => (&cfg.label_working, None),
            Phase::Thinking => (&cfg.label_thinking, None),
            Phase::Approval => (&cfg.label_approval, self.approval.as_deref()),
            Phase::Danger => (&cfg.label_danger, self.danger.as_deref()),
            Phase::Error => (&cfg.label_error, None),
            Phase::Done => (&cfg.label_done, None),
        };
        template.replace("{action}", &crate::tree::truncate(action.unwrap_or(""), 60))
    }

    /// What should be on screen now.
    pub fn scene(&self, now: Instant) -> Scene {
        if self.phase == Phase::Off || !self.cfg.enabled {
            return Scene::default();
        }
        let mut color = self.color();
        let mut pulse = 0.0;
        if self.phase == Phase::Approval {
            // Breathe so a pending approval catches the eye.
            let t = now.saturating_duration_since(self.since).as_secs_f32();
            pulse = (0.5 - 0.5 * (t * std::f32::consts::TAU).cos()).clamp(0.0, 1.0);
            color = draw::lighten(color, 0.45 * pulse);
        }
        let ripple = self.ripple_at.map(|t| {
            (now.saturating_duration_since(t).as_secs_f32() / RIPPLE.as_secs_f32()).min(0.999)
        });
        Scene {
            border: self.cfg.show_border.then_some((self.target, color)),
            label: self
                .cfg
                .show_label
                .then(|| (self.label_text(), self.color())),
            cursor: if self.cfg.show_cursor {
                self.cursor_pos(now).map(|p| CursorLook {
                    pos: p,
                    ring: color,
                    body: self.colors.cursor,
                    ripple,
                    pulse,
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
    pub pulse: f32,
}

/// What is on screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
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
#[derive(Default)]
pub struct Painter {
    border: Option<([i64; 16], [u8; 4], u32)>,
    label: Option<(String, [u8; 4], i64, i64)>,
    label_size: (f64, f64),
    cursor_img: Option<([u8; 4], [u8; 4], i32, i32)>,
    cursor_pos: Option<(i64, i64)>,
}

impl Painter {
    pub fn paint(
        &mut self,
        scene: &Scene,
        cfg: &OverlayConfig,
        fonts: &Fonts,
        s: &mut dyn Surface,
    ) {
        let scale = if cfg.scale > 0.0 {
            cfg.scale as f32
        } else {
            s.render_scale()
        };
        let ppu = s.px_per_unit().max(0.1);
        let screen = s.screen();

        // Border: a glow along the screen edges fading inward, or around the
        // target window (outside it where there is room, else inside).
        let unit = f64::from(scale) / f64::from(ppu);
        let core = (f64::from(cfg.border_width.max(1)) * unit).max(1.0);
        let band = (f64::from(cfg.glow_size) * unit).max(core);
        let px = |v: f64| ((v * f64::from(ppu)).round().max(1.0)) as u32;
        let window_mode = cfg.border_target == crate::config::BorderTarget::Window;
        let area = |target: Option<Rect>| match target {
            Some(r) if window_mode => r,
            _ => screen,
        };
        // Where the label goes: centred on `cx`, above `top` if it fits, else at `inside`.
        let mut place = (
            screen.x + screen.width / 2.0,
            screen.y,
            screen.y + s.top_inset() + core + 10.0,
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
            let edges: [(f64, f64, f64, f64, u8); 4] = if r == screen {
                [
                    (sx, sy, screen.width, band, 0),
                    (sr - band, sy, band, screen.height, 1),
                    (sx, sb - band, screen.width, band, 2),
                    (sx, sy, band, screen.height, 3),
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
                let key = (key, color_key(color), (core * 100.0) as u32);
                if self.border != Some(key) {
                    let core_px = (core * f64::from(ppu)) as f32;
                    for (layer, (x, y, w, h, strong)) in EDGES.iter().zip(edges) {
                        if w < 1.0 || h < 1.0 {
                            s.hide(*layer);
                            continue;
                        }
                        let img = draw::edge(px(w), px(h), color, strong, core_px);
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
                let key = (text.clone(), color_key(*color));
                let redraw = self
                    .label
                    .as_ref()
                    .is_none_or(|(t, c, _, _)| (t, c) != (&key.0, &key.1));
                let mut img = None;
                if redraw {
                    let pm = draw::label(fonts, text, scale, *color);
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
                        if self.label.as_ref().is_some_and(|l| (l.2, l.3) != pos) {
                            s.move_to(Layer::Label, x, y);
                        }
                    }
                }
                self.label = Some((key.0, key.1, pos.0, pos.1));
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
                    (c.pulse * 10.0) as i32,
                );
                let half = f64::from(CURSOR_BOX * scale) / 2.0 / f64::from(ppu);
                let (x, y) = (c.pos.0 - half, c.pos.1 - half);
                let pos = (x.round() as i64, y.round() as i64);
                if self.cursor_img != Some(key) {
                    let img = draw::cursor(scale, c.body, c.ring, c.ripple, c.pulse);
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
                        return;
                    }
                }
            }
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
            while let Ok(Input::Cmd(_)) = rx.recv() {}
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
    let mut last_parent_check = Instant::now();

    'main: loop {
        let now = Instant::now();
        let wait = if machine.animating(now) {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(100)
        };
        let mut first = match rx.recv_timeout(wait) {
            Ok(i) => Some(i),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        loop {
            let input = match first.take() {
                Some(i) => i,
                None => match rx.try_recv() {
                    Ok(i) => i,
                    Err(_) => break,
                },
            };
            let cmd = match input {
                Input::Eof | Input::Cmd(Cmd::Quit) => break 'main,
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
                    if let Cmd::Config { config } = &other
                        && config.font != font_path
                    {
                        font_path = config.font.clone();
                        fonts = Fonts::load(&font_path);
                        painter = Painter::default();
                    }
                    if let Some((id, action)) = machine.apply(other, Instant::now()) {
                        let cfg = machine.config();
                        let ask = Ask {
                            title: cfg.label_working.clone(),
                            message: cfg.label_approval.replace("{action}", &action),
                            allow: cfg.label_allow.clone(),
                            deny: cfg.label_deny.clone(),
                        };
                        surface.confirm(id, &ask);
                    }
                }
            }
        }

        for (id, ok) in surface.pump() {
            reply(&Reply::Answer { id, ok });
        }
        if let Some(pid) = parent
            && last_parent_check.elapsed() >= Duration::from_millis(500)
        {
            last_parent_check = Instant::now();
            if !super::process_alive(pid) {
                break;
            }
        }
        let now = Instant::now();
        machine.tick(now);
        let scene = machine.scene(now);
        painter.paint(&scene, machine.config(), &fonts, surface.as_mut());
    }
    surface.close();
    0
}

fn open_surface() -> Result<Box<dyn Surface>, String> {
    #[cfg(target_os = "linux")]
    {
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
    send(Cmd::Approval {
        id: 1,
        action: "press button \"Send\"".into(),
        ask: false,
    });
    pause(1800);
    send(Cmd::ApprovalDone);
    send(Cmd::Danger {
        action: Some("press button \"Send\"".into()),
    });
    send(Cmd::Pointer {
        x: 600.0,
        y: 520.0,
        click: true,
    });
    pause(1500);
    send(Cmd::End { ok: false });
    pause(2800);
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
        assert_eq!(m.phase, Phase::Off, "done → gone");
        assert_eq!(m.scene(at(1700)), Scene::default());

        // Errors show red for a while, then back to thinking.
        m.apply(Cmd::Begin, at(2000));
        m.apply(Cmd::End { ok: false }, at(2010));
        assert_eq!(m.phase, Phase::Error);
        m.tick(at(2400));
        assert_eq!(m.phase, Phase::Thinking);

        // Approval, then a sensitive action.
        m.apply(Cmd::Begin, at(3000));
        let ask = m.apply(
            Cmd::Approval {
                id: 7,
                action: "press \"Delete\"".into(),
                ask: true,
            },
            at(3000),
        );
        assert_eq!(ask, Some((7, "press \"Delete\"".to_string())));
        assert_eq!(m.phase, Phase::Approval);
        let s = m.scene(at(3000));
        assert!(s.label.unwrap().0.contains("Delete"));
        m.apply(
            Cmd::Danger {
                action: Some("press \"Delete\"".into()),
            },
            at(3100),
        );
        m.apply(Cmd::ApprovalDone, at(3100));
        assert_eq!(m.phase, Phase::Danger);
        m.apply(Cmd::End { ok: true }, at(3200));
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
        fn confirm(&mut self, _: u64, _: &Ask) {}
        fn pump(&mut self) -> Vec<(u64, bool)> {
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
        p.paint(&m.scene(t0), m.config(), &fonts, &mut s);
        assert!(
            s.calls[n..].iter().any(|c| c.starts_with("show Top")),
            "recoloured"
        );
        m.apply(
            Cmd::Status {
                state: Status::Hidden,
            },
            t0,
        );
        p.paint(&m.scene(t0), m.config(), &fonts, &mut s);
        assert!(!p.showing());
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
