//! The hub: one process every engine on this desktop shares, whichever
//! client started it (Claude Code, Codex, a subagent of either).
//!
//! * The first engine to start begins the hub (`computer-use-mcp hub`) and
//!   is agent 1; the next ones join it in turn (2, 3…), and a number freed
//!   by an agent that left goes to the next to join.
//! * It draws the overlay for all of them: a cursor each, tagged with its
//!   number once there are two or more (with one, "Zero" as ever), its
//!   border and its label.
//! * One stop key stops them all (a second engine with an overlay of its
//!   own couldn't even register it: the system gives a key to one program).
//! * It shares the screen out (each agent can ask for a full, half, third
//!   or quarter screen; what doesn't fit is shared out evenly), gives the
//!   keyboard and mouse to one agent at a time, and passes their messages.
//!
//! It listens on a port of this computer only (`hub.port`), and only to
//! those that read its token: a file in the server's folder that only this
//! user can read. It ends a few seconds after the last agent leaves.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::helper::{Hotkey, Machine, Painter, Surface, SurfaceEvent};
use super::text::Fonts;
use super::{AreaWant, Cmd, HUB_PROTO, Launcher, Message, Peer, Reply};
use crate::config::OverlayConfig;
use crate::types::Rect;

/// The hub's token, in the server's folder (`hub-<port>.token`: hubs on
/// two ports, after the port was changed, never share one).
const TOKEN_FILE: &str = "hub";
/// How long an agent may keep the keyboard and mouse without a word
/// (engines say they still act every few seconds): one that went quiet is
/// stuck, and the turn goes on.
const MAX_HOLD: Duration = Duration::from_secs(30);
/// …and at most this long in all, however it keeps saying so.
const MAX_HOLD_IN_ALL: Duration = Duration::from_secs(600);
/// The hub ends this long after the last agent left…
const LINGER: Duration = Duration::from_secs(3);
/// …or when nobody came this long after it started.
const FIRST_WAIT: Duration = Duration::from_secs(15);
/// A screenshot's hiding ends by itself after this, should the agent never
/// say it is done.
const MAX_HIDE: Duration = Duration::from_secs(3);
/// Longest message passed on (characters).
pub const MAX_MESSAGE: usize = 1000;
/// Messages one agent may send a minute.
pub const MESSAGES_A_MINUTE: usize = 20;

/// Where the token of the hub on `port` is.
pub fn token_path(home: &Path, port: u16) -> PathBuf {
    home.join(format!("{TOKEN_FILE}-{port}.token"))
}

// ---------------------------------------------------------------------------
// Joining (the engine's side)

/// How an engine joins the hub.
#[derive(Debug, Clone)]
pub struct JoinOptions {
    pub port: u16,
    /// The server's folder, where the hub keeps its token.
    pub home: PathBuf,
    pub client: String,
    /// The number this engine had (from a hub that has gone).
    pub want: Option<u32>,
    /// The screen's work area, for sharing it out.
    pub screen: Option<Rect>,
}

/// Connect to the hub, starting it if none answers, and join it: the
/// connection, its reader (past the welcome) and this engine's number.
pub(crate) fn connect(
    launcher: &Launcher,
    opts: &JoinOptions,
) -> std::io::Result<(TcpStream, BufReader<TcpStream>, u32)> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, opts.port));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut started = false;
    loop {
        let error = match TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
            Ok(stream) => match handshake(stream, opts) {
                Ok(joined) => return Ok(joined),
                // Another protocol: no use trying again.
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported => return Err(e),
                // A hub just started may not have written its token yet.
                Err(e) => e,
            },
            Err(e) => {
                if !started {
                    started = true;
                    start_hub(launcher, opts)?;
                }
                e
            }
        };
        if Instant::now() >= deadline {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn handshake(
    stream: TcpStream,
    opts: &JoinOptions,
) -> std::io::Result<(TcpStream, BufReader<TcpStream>, u32)> {
    let token = std::fs::read_to_string(token_path(&opts.home, opts.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_nodelay(true)?;
    let hello = Cmd::Hello {
        token: token.trim().to_string(),
        client: opts.client.clone(),
        pid: std::process::id(),
        want: opts.want,
        screen: opts.screen.map(|r| [r.x, r.y, r.width, r.height]),
        proto: HUB_PROTO,
    };
    let mut w = stream.try_clone()?;
    writeln!(w, "{}", serde_json::to_string(&hello)?)?;
    w.flush()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    match serde_json::from_str::<Reply>(&line) {
        Ok(Reply::Welcome { agent, proto }) if proto == HUB_PROTO => {
            stream.set_read_timeout(None)?;
            Ok((stream, reader, agent))
        }
        Ok(Reply::Welcome { proto, .. }) => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("the hub speaks version {proto}, this server {HUB_PROTO}"),
        )),
        Ok(Reply::Refused { why }) => Err(std::io::Error::other(why)),
        _ => Err(std::io::Error::other("the hub didn't answer")),
    }
}

/// Start the hub in a process of its own: not the engine's child for good
/// (it stays while other agents use it), and not stopped with the client's
/// terminal.
fn start_hub(launcher: &Launcher, opts: &JoinOptions) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(&launcher.program);
    cmd.arg("hub")
        .arg("--port")
        .arg(opts.port.to_string())
        .arg("--home")
        .arg(&opts.home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        // What it says (no display, a port taken) goes to a file beside
        // its token, for `doctor` and for whoever wonders.
        .current_dir(&opts.home)
        .stderr(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(opts.home.join("hub.log"))
                .map(std::process::Stdio::from)
                .unwrap_or_else(|_| std::process::Stdio::null()),
        );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    let mut child = cmd.spawn()?;
    // Reaped when it ends, so it never lingers as a zombie.
    let _ = std::thread::Builder::new()
        .name("hub-reap".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

// ---------------------------------------------------------------------------
// The hub's bookkeeping (no I/O, so it is tested on its own)

/// One agent, as the hub knows it.
#[derive(Debug, Clone, Default)]
struct Agent {
    client: String,
    app: String,
    want: AreaWant,
    /// Messages sent in the last minute.
    sent: VecDeque<Instant>,
}

/// Who has the keyboard and mouse, and who waits for them.
#[derive(Debug, Default)]
struct Turns {
    /// The agent, its request, since when it has the turn, and when it
    /// last said it still acts.
    holder: Option<(u32, u64, Instant, Instant)>,
    waiting: VecDeque<(u32, u64)>,
}

/// What the hub keeps: the agents, the screen, the turns.
#[derive(Debug, Default)]
pub struct HubState {
    agents: BTreeMap<u32, Agent>,
    screen: Option<Rect>,
    turns: Turns,
}

/// A turn given: to this agent, for its request `id`.
pub type Grant = (u32, u64);

impl HubState {
    /// A new agent: the number it asks for if free, else the lowest free.
    pub fn join(&mut self, want: Option<u32>, client: &str) -> u32 {
        let n = want
            .filter(|n| *n > 0 && !self.agents.contains_key(n))
            .unwrap_or_else(|| (1..).find(|n| !self.agents.contains_key(n)).unwrap_or(1));
        self.agents.insert(
            n,
            Agent {
                client: client.to_string(),
                ..Agent::default()
            },
        );
        n
    }

    /// An agent left: its turn (if it had one) goes to the next.
    pub fn leave(&mut self, agent: u32) -> Option<Grant> {
        self.agents.remove(&agent);
        self.unlock(agent)
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn ids(&self) -> Vec<u32> {
        self.agents.keys().copied().collect()
    }

    pub fn set_screen(&mut self, screen: Rect) {
        if self.screen.is_none() && screen.width > 0.0 && screen.height > 0.0 {
            self.screen = Some(screen);
        }
    }

    /// Ask for the keyboard and mouse: given at once when free.
    pub fn lock(&mut self, agent: u32, id: u64, now: Instant) -> Option<Grant> {
        if !self.agents.contains_key(&agent) {
            return None;
        }
        match self.turns.holder {
            None => {
                self.turns.holder = Some((agent, id, now, now));
                Some((agent, id))
            }
            // Its own again (an engine that gave up waiting and asked anew).
            Some((h, _, _, _)) if h == agent => {
                self.turns.holder = Some((agent, id, now, now));
                Some((agent, id))
            }
            Some(_) => {
                self.turns.waiting.retain(|(a, _)| *a != agent);
                self.turns.waiting.push_back((agent, id));
                None
            }
        }
    }

    /// Done with them (or no longer waiting): the next in line gets them.
    pub fn unlock(&mut self, agent: u32) -> Option<Grant> {
        self.turns.waiting.retain(|(a, _)| *a != agent);
        if self.turns.holder.is_some_and(|(h, _, _, _)| h == agent) {
            self.turns.holder = None;
            return self.next_turn(Instant::now());
        }
        None
    }

    /// The agent that has the keyboard and mouse.
    pub fn holder(&self) -> Option<u32> {
        self.turns.holder.map(|(a, ..)| a)
    }

    /// The holder says it still acts (a long typing, a drawing).
    pub fn hold(&mut self, agent: u32, now: Instant) {
        if let Some((a, _, _, heard)) = self.turns.holder.as_mut()
            && *a == agent
        {
            *heard = now;
        }
    }

    /// A turn whose agent went quiet (or kept it far too long) ends: the
    /// agent may be stuck. It is told, so it never acts on a turn it lost.
    pub fn expire(&mut self, now: Instant) -> Option<(u32, Option<Grant>)> {
        match self.turns.holder {
            Some((a, _, since, heard))
                if now.saturating_duration_since(heard) >= MAX_HOLD
                    || now.saturating_duration_since(since) >= MAX_HOLD_IN_ALL =>
            {
                self.turns.holder = None;
                Some((a, self.next_turn(now)))
            }
            _ => None,
        }
    }

    fn next_turn(&mut self, now: Instant) -> Option<Grant> {
        while let Some((a, id)) = self.turns.waiting.pop_front() {
            if self.agents.contains_key(&a) {
                self.turns.holder = Some((a, id, now, now));
                return Some((a, id));
            }
        }
        None
    }

    /// The agent asks for a part of the screen.
    pub fn want(&mut self, agent: u32, want: AreaWant) {
        if let Some(a) = self.agents.get_mut(&agent) {
            a.want = want;
        }
    }

    /// Note who the agent works for and on; true when it changed.
    pub fn describe(&mut self, agent: u32, client: Option<&str>, app: Option<&str>) -> bool {
        let Some(a) = self.agents.get_mut(&agent) else {
            return false;
        };
        let mut changed = false;
        if let Some(c) = client.filter(|c| *c != a.client) {
            a.client = c.to_string();
            changed = true;
        }
        if let Some(p) = app.filter(|p| *p != a.app) {
            a.app = p.to_string();
            changed = true;
        }
        changed
    }

    /// Whether the agent may send another message now (and count it).
    pub fn may_send(&mut self, agent: u32, now: Instant) -> bool {
        let Some(a) = self.agents.get_mut(&agent) else {
            return false;
        };
        while a
            .sent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60))
        {
            a.sent.pop_front();
        }
        if a.sent.len() >= MESSAGES_A_MINUTE {
            return false;
        }
        a.sent.push_back(now);
        true
    }

    pub fn client(&self, agent: u32) -> String {
        self.agents
            .get(&agent)
            .map(|a| a.client.clone())
            .unwrap_or_default()
    }

    /// Every agent, with its part of the screen.
    pub fn peers(&self) -> Vec<Peer> {
        let areas = self.layout();
        self.agents
            .iter()
            .map(|(n, a)| Peer {
                agent: *n,
                client: a.client.clone(),
                app: a.app.clone(),
                area: areas
                    .get(n)
                    .and_then(|(r, _)| *r)
                    .map(|r| [r.x, r.y, r.width, r.height]),
            })
            .collect()
    }

    /// Each agent's part of the screen (None: all of it) and whether it is
    /// what it asked for. What was asked for is given when it all fits
    /// (and leaves those that asked for nothing an eighth of the screen
    /// each, at least); otherwise the hub shares the screen out evenly.
    pub fn layout(&self) -> BTreeMap<u32, (Option<Rect>, bool)> {
        let ids = self.ids();
        let mut out = BTreeMap::new();
        if ids.len() <= 1 {
            for id in ids {
                let asked = self.agents[&id].want;
                out.insert(id, (None, matches!(asked, AreaWant::Auto | AreaWant::Full)));
            }
            return out;
        }
        let Some(screen) = self.screen else {
            for id in ids {
                out.insert(id, (None, false));
            }
            return out;
        };
        let wants: Vec<Option<f64>> = ids
            .iter()
            .map(|id| self.agents[id].want.fraction())
            .collect();
        let asked: f64 = wants.iter().flatten().sum();
        let free = wants.iter().filter(|w| w.is_none()).count();
        let any = free < wants.len();
        let fits = any
            && asked <= 1.0 + 1e-9
            && (free == 0 || (1.0 - asked) / free as f64 >= 0.125 - 1e-9);
        if !fits {
            for (id, r) in ids.iter().zip(grid(screen, ids.len())) {
                let granted = self.agents[id].want == AreaWant::Auto;
                out.insert(*id, (Some(r), granted));
            }
            return out;
        }
        let shares: Vec<f64> = wants
            .iter()
            .map(|w| w.unwrap_or((1.0 - asked) / free.max(1) as f64))
            .collect();
        let equal = shares.iter().all(|s| (s - shares[0]).abs() < 1e-9)
            && (shares[0] * shares.len() as f64 - 1.0).abs() < 1e-9;
        let rects = if equal {
            grid(screen, ids.len())
        } else {
            slices(screen, &shares)
        };
        for (id, r) in ids.iter().zip(rects) {
            out.insert(*id, (Some(r), true));
        }
        out
    }
}

/// `n` equal parts of `area`: side by side up to three, then rows of a grid.
fn grid(area: Rect, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let wide = area.width >= area.height;
    let (along, across) = if n <= 3 {
        (n, 1)
    } else {
        let a = (n as f64).sqrt().ceil() as usize;
        (a, n.div_ceil(a))
    };
    let (cols, rows) = if wide {
        (along, across)
    } else {
        (across, along)
    };
    let (w, h) = (area.width / cols as f64, area.height / rows as f64);
    (0..n)
        .map(|i| {
            let (c, r) = (i % cols, i / cols);
            Rect::new(area.x + c as f64 * w, area.y + r as f64 * h, w, h)
        })
        .map(round)
        .collect()
}

/// Parts of `area` with these shares of it, in the agents' order (so
/// agent 1 stays on the left or top), each cut off the longer side of what
/// is left; what no one asked for stays empty.
fn slices(area: Rect, shares: &[f64]) -> Vec<Rect> {
    let mut left = area;
    let mut remaining = 1.0f64;
    let mut out = vec![area; shares.len()];
    for i in 0..shares.len() {
        let part = (shares[i] / remaining).clamp(0.0, 1.0);
        let r = if left.width >= left.height {
            let w = left.width * part;
            let r = Rect::new(left.x, left.y, w, left.height);
            left = Rect::new(left.x + w, left.y, left.width - w, left.height);
            r
        } else {
            let h = left.height * part;
            let r = Rect::new(left.x, left.y, left.width, h);
            left = Rect::new(left.x, left.y + h, left.width, left.height - h);
            r
        };
        out[i] = round(r);
        remaining = (remaining - shares[i]).max(1e-9);
    }
    out
}

fn round(r: Rect) -> Rect {
    let (x0, y0) = (r.x.round(), r.y.round());
    let (x1, y1) = ((r.x + r.width).round(), (r.y + r.height).round());
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

// ---------------------------------------------------------------------------
// The hub process

/// What the connections tell the main loop.
enum Event {
    Joined {
        conn: u64,
        hello: Cmd,
        out: mpsc::Sender<String>,
    },
    Cmd {
        conn: u64,
        cmd: Cmd,
    },
    Left {
        conn: u64,
    },
}

/// Entry point of `computer-use-mcp hub --port PORT --home DIR`.
pub fn run(args: &[String]) -> i32 {
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let Some(port) = arg("--port").and_then(|p| p.parse::<u16>().ok()) else {
        eprintln!("hub: --port is required");
        return 2;
    };
    let home = arg("--home")
        .map(PathBuf::from)
        .unwrap_or_else(crate::config::home_dir);
    // The port is the lock: a second hub started at the same time can't
    // have it, and leaves the first one's token alone.
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("hub: port {port}: {e}");
            return 1;
        }
    };
    let token = new_token();
    if let Err(e) = write_token(&home, port, &token) {
        eprintln!("hub: cannot write the token: {e}");
        return 1;
    }
    #[cfg(target_os = "macos")]
    super::macos::start_watchdog(None);
    let surface = match super::helper::open_surface() {
        Ok(s) => Some(s),
        Err(e) => {
            // No display: still numbers, turns, areas and messages.
            eprintln!("hub: no overlay ({e})");
            None
        }
    };
    serve(listener, token.clone(), surface);
    // Only its own: a hub started since (after this one let the port go)
    // may have written another.
    let path = token_path(&home, port);
    if std::fs::read_to_string(&path).is_ok_and(|t| t.trim() == token) {
        let _ = std::fs::remove_file(path);
    }
    #[cfg(target_os = "macos")]
    super::macos::input_closed();
    0
}

/// A token no one can guess: 256 bits from the standard library's
/// randomly keyed hasher.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0..4u8)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u8(i);
            h.write_u128(nanos);
            h.write_u32(std::process::id());
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Write the token where only this user can read it.
pub fn write_token(home: &Path, port: u16, token: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let path = token_path(home, port);
    let tmp = home.join(format!("{TOKEN_FILE}-{port}.{}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, &path)
}

/// Accept agents and serve them until the last has gone.
pub fn serve(listener: TcpListener, token: String, surface: Option<Box<dyn Surface>>) {
    let (tx, rx) = mpsc::channel::<Event>();
    {
        let tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("hub-accept".into())
            .spawn(move || accept(listener, &token, &tx));
        if let Err(e) = spawned {
            eprintln!("hub: {e}");
            return;
        }
    }
    drop(tx);
    Hub::new(surface).run(&rx);
}

fn accept(listener: TcpListener, token: &str, tx: &mpsc::Sender<Event>) {
    let mut next = 0u64;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        next += 1;
        let conn = next;
        let token = token.to_string();
        let tx = tx.clone();
        let _ = std::thread::Builder::new()
            .name("hub-conn".into())
            .spawn(move || connection(stream, conn, &token, &tx));
    }
}

/// One agent's connection: its hello, then its commands, until it goes.
fn connection(stream: TcpStream, conn: u64, token: &str, tx: &mpsc::Sender<Event>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut write_half = stream;
    let refuse = |w: &mut TcpStream, why: &str| {
        let r = Reply::Refused { why: why.into() };
        if let Ok(line) = serde_json::to_string(&r) {
            let _ = writeln!(w, "{line}");
        }
    };
    // Read before the token is checked: at most a hello's worth.
    let mut line = String::new();
    if std::io::Read::take(&mut reader, 64 * 1024)
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let hello = match serde_json::from_str::<Cmd>(&line) {
        Ok(h @ Cmd::Hello { .. }) => h,
        _ => {
            refuse(&mut write_half, "say hello first");
            return;
        }
    };
    if let Cmd::Hello {
        token: given,
        proto,
        ..
    } = &hello
    {
        if !same(given.as_bytes(), token.as_bytes()) {
            refuse(&mut write_half, "wrong token");
            return;
        }
        if *proto != HUB_PROTO {
            // Welcomed with the hub's version, so the engine knows why.
            let r = Reply::Welcome {
                agent: 0,
                proto: HUB_PROTO,
            };
            if let Ok(line) = serde_json::to_string(&r) {
                let _ = writeln!(write_half, "{line}");
            }
            return;
        }
    }
    let _ = reader.get_ref().set_read_timeout(None);
    // Written on a thread of its own: an agent slow to read never holds
    // the hub (or the others) up.
    let (out, lines) = mpsc::channel::<String>();
    let _ = std::thread::Builder::new()
        .name("hub-write".into())
        .spawn(move || {
            for line in lines {
                if writeln!(write_half, "{line}").is_err() || write_half.flush().is_err() {
                    break;
                }
            }
            let _ = write_half.shutdown(std::net::Shutdown::Both);
        });
    if tx.send(Event::Joined { conn, hello, out }).is_err() {
        return;
    }
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(cmd) = serde_json::from_str::<Cmd>(&line) else {
            continue;
        };
        let quit = cmd == Cmd::Quit;
        if tx.send(Event::Cmd { conn, cmd }).is_err() || quit {
            break;
        }
    }
    let _ = tx.send(Event::Left { conn });
}

/// Compared in constant time.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// One agent's overlay.
struct Look {
    machine: Machine,
    painter: Painter,
}

struct Hub {
    state: HubState,
    surface: Option<Box<dyn Surface>>,
    fonts: Fonts,
    font_path: Option<String>,
    looks: BTreeMap<u32, Look>,
    conns: HashMap<u64, u32>,
    outs: HashMap<u32, mpsc::Sender<String>>,
    /// The parts of the screen last told to each agent.
    told: HashMap<u32, (Option<Rect>, bool)>,
    /// Agents hiding the overlay for a screenshot, since when.
    hiders: HashMap<u32, Instant>,
    hidden: bool,
    /// The stop key and the settings key as registered, and whether the
    /// system took them.
    hotkey: Option<(String, bool)>,
    settings_key: Option<(String, bool)>,
    stopped: bool,
    last_hotkey: Option<Instant>,
    last_settings: Option<Instant>,
    started: Instant,
    empty_since: Option<Instant>,
    /// Someone has joined since the hub started.
    joined: bool,
}

impl Hub {
    fn new(mut surface: Option<Box<dyn Surface>>) -> Self {
        if let Some(s) = surface.as_mut() {
            // Each agent's fades are drawn: the surface's one opacity
            // would be everyone's.
            s.set_opacity(1.0);
        }
        Self {
            state: HubState::default(),
            surface,
            fonts: Fonts::load(""),
            font_path: None,
            looks: BTreeMap::new(),
            conns: HashMap::new(),
            outs: HashMap::new(),
            told: HashMap::new(),
            hiders: HashMap::new(),
            hidden: false,
            hotkey: None,
            settings_key: None,
            stopped: false,
            last_hotkey: None,
            last_settings: None,
            started: Instant::now(),
            empty_since: None,
            joined: false,
        }
    }

    fn reply(&self, agent: u32, r: &Reply) {
        if let (Some(out), Ok(line)) = (self.outs.get(&agent), serde_json::to_string(r)) {
            let _ = out.send(line);
        }
    }

    fn broadcast(&self, r: &Reply, except: Option<u32>) {
        for a in self.outs.keys().filter(|a| Some(**a) != except) {
            self.reply(*a, r);
        }
    }

    fn run(&mut self, rx: &mpsc::Receiver<Event>) {
        /// Within this of the previous one, a key press is key repeat.
        const HOTKEY_QUIET: Duration = Duration::from_millis(400);
        loop {
            let now = Instant::now();
            let animating = self.looks.values().any(|l| l.machine.animating(now));
            let wait = Duration::from_millis(if animating { 16 } else { 100 });
            let mut first = match rx.recv_timeout(wait) {
                Ok(e) => Some(e),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            loop {
                let event = match first.take() {
                    Some(e) => e,
                    None => match rx.try_recv() {
                        Ok(e) => e,
                        Err(_) => break,
                    },
                };
                self.event(event);
            }
            let events = self.surface.as_mut().map(|s| s.pump()).unwrap_or_default();
            for ev in events {
                match ev {
                    SurfaceEvent::Hotkey(Hotkey::Stop) => {
                        let repeat = self.last_hotkey.is_some_and(|t| t.elapsed() < HOTKEY_QUIET);
                        self.last_hotkey = Some(Instant::now());
                        if !repeat {
                            self.stop(!self.stopped);
                        }
                    }
                    SurfaceEvent::Hotkey(Hotkey::Settings) => {
                        let repeat = self
                            .last_settings
                            .is_some_and(|t| t.elapsed() < HOTKEY_QUIET);
                        self.last_settings = Some(Instant::now());
                        // One page, opened by the first agent.
                        if !repeat && let Some(first) = self.state.ids().first() {
                            self.reply(*first, &Reply::Settings);
                        }
                    }
                }
            }
            let now = Instant::now();
            if let Some((lost, next)) = self.state.expire(now) {
                self.reply(lost, &Reply::Revoked);
                if let Some((agent, id)) = next {
                    self.reply(agent, &Reply::Granted { id });
                }
            }
            self.hiders
                .retain(|_, since| now.saturating_duration_since(*since) < MAX_HIDE);
            self.update_hidden();
            self.paint(now);
            if self.state.is_empty() {
                let since = *self.empty_since.get_or_insert(now);
                if (self.joined && now.saturating_duration_since(since) >= LINGER)
                    || (!self.joined && now.saturating_duration_since(self.started) >= FIRST_WAIT)
                {
                    return;
                }
            } else {
                self.empty_since = None;
            }
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Joined { conn, hello, out } => {
                let Cmd::Hello {
                    client,
                    want,
                    screen,
                    ..
                } = hello
                else {
                    return;
                };
                if let Some(s) = screen {
                    self.state.set_screen(Rect::new(s[0], s[1], s[2], s[3]));
                }
                let agent = self.state.join(want, &client);
                self.joined = true;
                self.conns.insert(conn, agent);
                self.outs.insert(agent, out);
                self.reply(
                    agent,
                    &Reply::Welcome {
                        agent,
                        proto: HUB_PROTO,
                    },
                );
                self.reply(
                    agent,
                    &Reply::Ready {
                        excluded: self
                            .surface
                            .as_ref()
                            .is_none_or(|s| s.excluded_from_capture()),
                        available: self.surface.is_some(),
                    },
                );
                self.looks.insert(
                    agent,
                    Look {
                        machine: Machine::new(OverlayConfig::default(), Instant::now()),
                        painter: Painter::for_agent(agent),
                    },
                );
                if self.stopped {
                    self.reply(agent, &Reply::Stop { on: true });
                }
                self.agents_changed();
            }
            Event::Cmd { conn, cmd } => {
                if let Some(agent) = self.conns.get(&conn).copied() {
                    self.command(agent, cmd);
                }
            }
            Event::Left { conn } => {
                let Some(agent) = self.conns.remove(&conn) else {
                    return;
                };
                self.outs.remove(&agent);
                self.hiders.remove(&agent);
                self.told.retain(|a, _| *a != agent);
                if let Some(mut look) = self.looks.remove(&agent)
                    && let Some(s) = self.surface.as_mut()
                {
                    look.painter.clear(s.as_mut());
                }
                if let Some((next, id)) = self.state.leave(agent) {
                    self.reply(next, &Reply::Granted { id });
                }
                self.agents_changed();
            }
        }
    }

    fn command(&mut self, agent: u32, cmd: Cmd) {
        let now = Instant::now();
        match cmd {
            Cmd::Hello { .. } | Cmd::Quit => {}
            Cmd::Client { name } => {
                if self.state.describe(agent, Some(&name), None) {
                    self.agents_changed();
                }
            }
            Cmd::Doing { app } => {
                if self.state.describe(agent, None, Some(&app)) {
                    self.broadcast(
                        &Reply::Agents {
                            agents: self.state.peers(),
                        },
                        None,
                    );
                }
            }
            Cmd::Area { want } => {
                self.state.want(agent, want);
                self.areas_changed();
                // Answered even when nothing changed, so it isn't waited on.
                if let Some(area) = self.told.get(&agent).copied() {
                    self.reply(
                        agent,
                        &Reply::Region {
                            rect: area.0.map(|r| [r.x, r.y, r.width, r.height]),
                            granted: area.1,
                        },
                    );
                }
            }
            Cmd::Hold => self.state.hold(agent, now),
            Cmd::Lock { id } => {
                if let Some((a, id)) = self.state.lock(agent, id, now) {
                    self.reply(a, &Reply::Granted { id });
                }
            }
            Cmd::Unlock { acted } => {
                // Its input is done: not the user's, for the others.
                if acted && self.state.holder() == Some(agent) {
                    self.broadcast(&Reply::Input { agent }, Some(agent));
                }
                if let Some((next, id)) = self.state.unlock(agent) {
                    self.reply(next, &Reply::Granted { id });
                }
            }
            Cmd::Send { to, text } => {
                if !self.state.may_send(agent, now) {
                    return;
                }
                let text: String = text.chars().take(MAX_MESSAGE).collect();
                let m = Reply::Message(Message {
                    from: agent,
                    client: self.state.client(agent),
                    text,
                });
                match to {
                    Some(n) if n != agent => self.reply(n, &m),
                    Some(_) => {}
                    None => self.broadcast(&m, Some(agent)),
                }
            }
            Cmd::Hide { id } => {
                let shown = self.looks.values().any(|l| l.painter.showing()) && !self.hidden;
                self.hiders.insert(agent, now);
                self.update_hidden();
                self.reply(agent, &Reply::Hidden { id, shown });
            }
            Cmd::Show => {
                self.hiders.remove(&agent);
                self.update_hidden();
            }
            Cmd::Config {
                config,
                hotkey,
                settings_key,
                stopped,
            } => {
                // An agent that is stopped (a hub started again under it)
                // stops the others too: the stop key is everyone's.
                if stopped && !self.stopped {
                    self.stop(true);
                }
                if self.font_path.as_deref() != Some(&config.font) {
                    self.font_path = Some(config.font.clone());
                    self.fonts = Fonts::load(&config.font);
                }
                let ok = self.register(Hotkey::Stop, &hotkey);
                if !hotkey.trim().is_empty() {
                    self.reply(agent, &Reply::Hotkey { key: hotkey, ok });
                }
                let ok = self.register(Hotkey::Settings, &settings_key);
                if !settings_key.trim().is_empty() {
                    self.reply(
                        agent,
                        &Reply::SettingsKey {
                            key: settings_key.clone(),
                            ok,
                        },
                    );
                }
                if let Some(look) = self.looks.get_mut(&agent) {
                    look.machine.apply(
                        Cmd::Config {
                            config,
                            hotkey: self.hotkey.clone().map(|h| h.0).unwrap_or_default(),
                            settings_key,
                            stopped: stopped || self.stopped,
                        },
                        now,
                    );
                    look.painter.redraw();
                }
            }
            other => {
                if let Some(look) = self.looks.get_mut(&agent) {
                    look.machine.apply(other, now);
                }
            }
        }
    }

    /// Register a global key, the first time one is asked for: whether
    /// `key` is the one that works.
    fn register(&mut self, which: Hotkey, key: &str) -> bool {
        let key = key.trim();
        let slot = match which {
            Hotkey::Stop => &mut self.hotkey,
            Hotkey::Settings => &mut self.settings_key,
        };
        if key.is_empty() {
            return false;
        }
        // The same key, registered: nothing to do. A key that failed, or
        // another (the user changed it), is registered anew: the last
        // asked for wins.
        if let Some((k, true)) = slot
            && k.eq_ignore_ascii_case(key)
        {
            return true;
        }
        let combo = crate::keys::parse_combo(key).ok();
        let ok = self
            .surface
            .as_mut()
            .is_some_and(|s| s.set_hotkey(which, combo));
        *slot = Some((key.to_string(), ok));
        ok
    }

    /// The stop key: every agent stops (or may go on).
    fn stop(&mut self, on: bool) {
        self.stopped = on;
        let now = Instant::now();
        for look in self.looks.values_mut() {
            look.machine.apply(Cmd::Stopped { on }, now);
        }
        self.broadcast(&Reply::Stop { on }, None);
    }

    fn update_hidden(&mut self) {
        let hide = !self.hiders.is_empty();
        if hide != self.hidden {
            self.hidden = hide;
            if let Some(s) = self.surface.as_mut() {
                s.set_hidden(hide);
            }
        }
    }

    /// Someone joined or left: numbers on the cursors, the list, the areas.
    fn agents_changed(&mut self) {
        let many = self.state.len() >= 2;
        for (n, look) in &mut self.looks {
            look.painter.set_tag(many.then(|| n.to_string()));
            look.machine.set_badge(&if many {
                format!("{n} · ")
            } else {
                String::new()
            });
        }
        self.broadcast(
            &Reply::Agents {
                agents: self.state.peers(),
            },
            None,
        );
        self.areas_changed();
    }

    fn areas_changed(&mut self) {
        for (agent, area) in self.state.layout() {
            // Each agent's glow and label in its own part of the screen.
            if let Some(look) = self.looks.get_mut(&agent) {
                look.painter.set_area(area.0);
            }
            if self.told.get(&agent) != Some(&area) {
                self.told.insert(agent, area);
                self.reply(
                    agent,
                    &Reply::Region {
                        rect: area.0.map(|r| [r.x, r.y, r.width, r.height]),
                        granted: area.1,
                    },
                );
            }
        }
    }

    fn paint(&mut self, now: Instant) {
        let mut arrived = Vec::new();
        for (agent, look) in &mut self.looks {
            look.machine.tick(now);
            if let Some(s) = self.surface.as_mut() {
                let scene = look.machine.scene(now);
                look.painter
                    .paint(&scene, look.machine.config(), &self.fonts, s.as_mut());
            }
            if let Some(id) = look.machine.arrived(now) {
                arrived.push((*agent, id));
            }
        }
        for (agent, id) in arrived {
            self.reply(agent, &Reply::Arrived { id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::new(0.0, 0.0, 1920.0, 1080.0)
    }

    fn state(n: usize) -> HubState {
        let mut s = HubState::default();
        s.set_screen(screen());
        for _ in 0..n {
            s.join(None, "test");
        }
        s
    }

    #[test]
    fn numbers_in_turn_and_freed_ones_again() {
        let mut s = HubState::default();
        assert_eq!(s.join(None, "claude-code"), 1);
        assert_eq!(s.join(None, "codex"), 2);
        assert_eq!(s.join(None, "codex"), 3);
        s.leave(2);
        assert_eq!(s.join(None, "x"), 2);
        assert_eq!(s.join(None, "x"), 4);
        // A hub started again gives an agent its old number if free.
        assert_eq!(s.join(Some(7), "x"), 7);
        assert_eq!(s.join(Some(1), "x"), 5);
    }

    #[test]
    fn the_screen_is_shared_evenly() {
        let one = state(1).layout();
        assert_eq!(one[&1], (None, true));
        let two = state(2).layout();
        assert_eq!(two[&1].0, Some(Rect::new(0.0, 0.0, 960.0, 1080.0)));
        assert_eq!(two[&2].0, Some(Rect::new(960.0, 0.0, 960.0, 1080.0)));
        let three = state(3).layout();
        assert_eq!(three[&3].0, Some(Rect::new(1280.0, 0.0, 640.0, 1080.0)));
        let four = state(4).layout();
        assert_eq!(four[&1].0, Some(Rect::new(0.0, 0.0, 960.0, 540.0)));
        assert_eq!(four[&4].0, Some(Rect::new(960.0, 540.0, 960.0, 540.0)));
        assert!(four.values().all(|(_, granted)| *granted));
    }

    #[test]
    fn a_part_asked_for_is_given_when_it_fits() {
        let mut s = state(3);
        s.want(1, AreaWant::Half);
        let l = s.layout();
        assert_eq!(l[&1], (Some(Rect::new(0.0, 0.0, 960.0, 1080.0)), true));
        // The others share the other half: a quarter each, one above the other.
        assert_eq!(l[&2].0, Some(Rect::new(960.0, 0.0, 960.0, 540.0)));
        assert_eq!(l[&3].0, Some(Rect::new(960.0, 540.0, 960.0, 540.0)));

        // Two halves and a quarter don't fit: shared evenly, and those that
        // asked are told they didn't get it.
        s.want(2, AreaWant::Half);
        s.want(3, AreaWant::Quarter);
        let l = s.layout();
        assert_eq!(l[&1], (Some(Rect::new(0.0, 0.0, 640.0, 1080.0)), false));
        assert!(!l[&3].1);

        // The whole screen with others there: not given.
        let mut s = state(2);
        s.want(2, AreaWant::Full);
        assert!(!s.layout()[&2].1);
    }

    #[test]
    fn one_agent_at_a_time_at_the_keyboard() {
        let mut s = state(3);
        let now = Instant::now();
        assert_eq!(s.lock(1, 10, now), Some((1, 10)));
        assert_eq!(s.lock(2, 20, now), None);
        assert_eq!(s.lock(3, 30, now), None);
        // In turn; one that gave up waiting is skipped.
        assert_eq!(s.unlock(3), None);
        assert_eq!(s.unlock(1), Some((2, 20)));
        // An agent that leaves gives its turn on.
        assert_eq!(s.lock(1, 11, now), None);
        assert_eq!(s.leave(2), Some((1, 11)));
        // One that says it still acts keeps it…
        assert_eq!(s.lock(3, 31, now), None);
        let later = Instant::now() + MAX_HOLD - Duration::from_secs(1);
        s.hold(1, later);
        assert_eq!(s.expire(later + Duration::from_secs(2)), None);
        // …but not once it goes quiet (it is told), nor past the limit in all.
        assert_eq!(
            s.expire(later + MAX_HOLD + Duration::from_secs(1)),
            Some((1, Some((3, 31))))
        );
        // (given when the first lost it, at that moment)
        let start = later + MAX_HOLD + Duration::from_secs(1);
        let mut held = start;
        while held < start + MAX_HOLD_IN_ALL {
            held += Duration::from_secs(20);
            s.hold(3, held);
        }
        assert_eq!(s.expire(held), Some((3, None)));
    }

    #[test]
    fn messages_are_limited() {
        let mut s = state(2);
        let now = Instant::now();
        for _ in 0..MESSAGES_A_MINUTE {
            assert!(s.may_send(1, now));
        }
        assert!(!s.may_send(1, now));
        assert!(s.may_send(1, now + Duration::from_secs(61)));
    }

    /// A display that refuses the stop key `taken` (another program has
    /// it) and takes any other.
    struct Display {
        taken: &'static str,
        keys: Vec<String>,
    }

    impl Surface for Display {
        fn excluded_from_capture(&self) -> bool {
            true
        }
        fn screen(&self) -> Rect {
            screen()
        }
        fn render_scale(&self) -> f32 {
            1.0
        }
        fn px_per_unit(&self) -> f32 {
            1.0
        }
        fn show(&mut self, _: super::super::helper::Layer, _: &tiny_skia::Pixmap, _: f64, _: f64) {}
        fn move_to(&mut self, _: super::super::helper::Layer, _: f64, _: f64) {}
        fn hide(&mut self, _: super::super::helper::Layer) {}
        fn set_hidden(&mut self, _: bool) {}
        fn set_hotkey(&mut self, _: Hotkey, combo: Option<crate::keys::KeyCombo>) -> bool {
            let Some(c) = combo else { return false };
            let ok = crate::keys::parse_combo(self.taken).ok().as_ref() != Some(&c);
            if ok {
                self.keys.push(c.to_string());
            }
            ok
        }
        fn pump(&mut self) -> Vec<SurfaceEvent> {
            Vec::new()
        }
        fn close(&mut self) {}
    }

    /// A stop key that failed is tried again when another is asked for
    /// (it used to stay failed for the hub's life, leaving no stop key),
    /// and an agent that comes stopped stops the others.
    #[test]
    fn the_stop_key_is_registered_again_and_stopping_is_shared() {
        let display = Display {
            taken: "ctrl+alt+escape",
            keys: Vec::new(),
        };
        let mut hub = Hub::new(Some(Box::new(display)));
        assert!(!hub.register(Hotkey::Stop, "ctrl+alt+escape"));
        assert!(hub.register(Hotkey::Stop, "ctrl+alt+f12"));
        assert!(hub.register(Hotkey::Stop, "ctrl+alt+f12"));
        assert_eq!(hub.hotkey, Some(("ctrl+alt+f12".into(), true)));

        let (tx, _rx) = mpsc::channel();
        hub.event(Event::Joined {
            conn: 1,
            hello: Cmd::Hello {
                token: String::new(),
                client: "codex".into(),
                pid: 1,
                want: None,
                screen: None,
                proto: HUB_PROTO,
            },
            out: tx,
        });
        assert!(!hub.stopped);
        hub.command(
            1,
            Cmd::Config {
                config: Box::default(),
                hotkey: "ctrl+alt+f12".into(),
                settings_key: String::new(),
                stopped: true,
            },
        );
        assert!(hub.stopped);
    }

    // -- the hub over its socket, without a display --------------------

    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    struct Running {
        port: u16,
        home: PathBuf,
    }

    fn hub(name: &str) -> Running {
        let home = std::env::temp_dir().join(format!("cu-hub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = new_token();
        write_token(&home, port, &token).unwrap();
        std::thread::spawn(move || serve(listener, token, None));
        Running { port, home }
    }

    fn join(h: &Running, client: &str) -> super::super::Overlay {
        let opts = JoinOptions {
            port: h.port,
            home: h.home.clone(),
            client: client.into(),
            want: None,
            screen: Some(screen()),
        };
        // No hub to start: it runs.
        let launcher = Launcher::helper("/nonexistent/computer-use-mcp");
        super::super::Overlay::join_hub(
            &launcher,
            &opts,
            &OverlayConfig::default(),
            &super::super::Keys::default(),
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .unwrap()
    }

    fn until(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn agents_join_share_turns_and_talk() {
        let h = hub("talk");
        let mut one = join(&h, "claude-code");
        let mut two = join(&h, "codex");
        let (l1, l2) = (one.hub().unwrap().clone(), two.hub().unwrap().clone());
        assert_eq!((l1.agent(), l2.agent()), (1, 2));
        until("both know both", || {
            l1.peers().len() == 2 && l2.peers().len() == 2
        });
        assert_eq!(l1.peers()[1].client, "codex");
        until("areas", || l2.region().0.is_some());
        assert_eq!(
            l2.region(),
            (Some(Rect::new(960.0, 0.0, 960.0, 1080.0)), true)
        );

        // Turns: two waits while one has the keyboard and mouse.
        assert!(one.lock_input(Duration::from_secs(2), || false));
        assert!(!two.lock_input(Duration::from_millis(300), || false));
        let waiting = std::thread::spawn(move || {
            let got = two.lock_input(Duration::from_secs(5), || false);
            (got, two)
        });
        std::thread::sleep(Duration::from_millis(200));
        one.unlock_input(true);
        let (got, two) = waiting.join().unwrap();
        assert!(got);
        // One's input isn't taken for the user's by two.
        until("input noted", || l2.others_input().is_some());
        two.unlock_input(true);

        // Messages, to one agent and to all.
        two.send(&Cmd::Send {
            to: Some(1),
            text: "found it: 42 dollars".into(),
        });
        until("message", || l1.has_messages());
        let (m, left) = l1.take_messages(10);
        assert_eq!(left, 0);
        assert_eq!((m[0].from, m[0].client.as_str()), (2, "codex"));
        assert_eq!(m[0].text, "found it: 42 dollars");
        assert!(!l2.has_messages());

        // Asking for half of the screen.
        one.send(&Cmd::Area {
            want: AreaWant::Quarter,
        });
        until("quarter", || {
            l1.region() == (Some(Rect::new(0.0, 0.0, 480.0, 1080.0)), true)
        });

        // One leaves: two is alone, with all of the screen.
        drop(one);
        until("alone", || l2.peers().len() == 1);
        until("whole screen", || l2.region().0.is_none());
        let _ = std::fs::remove_dir_all(&h.home);
    }

    /// Eight agents come and go and take turns, over and over: never two
    /// at the keyboard at once, and the hub keeps answering.
    #[test]
    fn many_agents_come_go_and_take_turns() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let h = hub("churn");
        let busy = Arc::new(AtomicBool::new(false));
        let turns = Arc::new(AtomicUsize::new(0));
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let (port, home) = (h.port, h.home.clone());
                let (busy, turns) = (busy.clone(), turns.clone());
                std::thread::spawn(move || {
                    for round in 0..6 {
                        let opts = JoinOptions {
                            port,
                            home: home.clone(),
                            client: format!("worker-{i}"),
                            want: None,
                            screen: Some(screen()),
                        };
                        let mut o = super::super::Overlay::join_hub(
                            &Launcher::helper("/nonexistent/computer-use-mcp"),
                            &opts,
                            &OverlayConfig::default(),
                            &super::super::Keys::default(),
                            Arc::new(AtomicBool::new(false)),
                            None,
                        )
                        .unwrap_or_else(|e| panic!("worker {i} round {round}: {e}"));
                        for _ in 0..3 {
                            if !o.lock_input(Duration::from_secs(10), || false) {
                                continue;
                            }
                            // Alone with the keyboard and mouse, or the test fails.
                            assert!(!busy.swap(true, Ordering::SeqCst), "two at once");
                            std::thread::sleep(Duration::from_millis(2));
                            busy.store(false, Ordering::SeqCst);
                            turns.fetch_add(1, Ordering::SeqCst);
                            o.unlock_input(true);
                        }
                        // Some leave while others still wait.
                        drop(o);
                    }
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        assert_eq!(turns.load(Ordering::SeqCst), 8 * 6 * 3);
        let _ = std::fs::remove_dir_all(&h.home);
    }

    #[test]
    fn a_wrong_token_is_refused() {
        let h = hub("token");
        std::fs::write(token_path(&h.home, h.port), "not it").unwrap();
        let opts = JoinOptions {
            port: h.port,
            home: h.home.clone(),
            client: "x".into(),
            want: None,
            screen: None,
        };
        let stream = TcpStream::connect(("127.0.0.1", h.port)).unwrap();
        let e = handshake(stream, &opts).unwrap_err();
        assert!(e.to_string().contains("wrong token"), "{e}");
        let _ = std::fs::remove_dir_all(&h.home);
    }

    #[cfg(unix)]
    #[test]
    fn only_this_user_reads_the_token() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = std::env::temp_dir().join(format!("cu-hub-perm-{}", std::process::id()));
        write_token(&home, 1, "t").unwrap();
        let mode = std::fs::metadata(token_path(&home, 1))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&home);
    }
}
