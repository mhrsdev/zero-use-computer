//! The settings panel: a page this program serves on 127.0.0.1 and opens in
//! the user's browser, from the settings key (`control.settings_hotkey`,
//! Ctrl+Alt+J), `decide setup="open"` or `computer-use-mcp settings`. Every
//! setting is on it, with the decision model's API key; changes are saved
//! in the settings file (comments kept) and a running server uses them at
//! once (hot reload).
//!
//! Only this user's browser gets in: the page lives behind a 128-bit token
//! in its path (kept in a file only this user can read, so the address is
//! the same every time), on the loopback interface; a request for another
//! host name (DNS rebinding) or from another site is refused; it listens
//! only while it is open and closes after a while unused. Saved secrets
//! (the API key, the HTTP token) are never sent back to the page, only
//! their last four characters.
//!
//! The agent never uses it: the page's window title carries a mark that
//! the engine refuses to act on (see [`is_panel_window`]).

mod schema;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use self::schema::{ENTRIES, Entry, GROUPS, Kind};
use crate::config::{self, Config, ConfigStore, DecisionConfig, Edit};
use crate::decision::{Decider, Provider, page as model};
use crate::error::{Error, Result};

/// The text the page's title carries (lower case).
pub const WINDOW_MARK: &str = "zero panel [private]";

/// Whether a window is the panel. The agent is never allowed to act on it:
/// it could change its own limits there.
pub fn is_panel_window(title: &str) -> bool {
    title.to_lowercase().contains(WINDOW_MARK)
}

/// The largest request read.
const MAX_REQUEST: usize = 64 * 1024;
/// Connections served at once, at most (a browser opens a few spare ones).
const MAX_CONNECTIONS: usize = 16;

struct Running {
    url: String,
    alive: Arc<AtomicBool>,
}

/// The panel being served by this process, if any (one at a time).
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Show the panel in the user's browser, on the page `tab` (a group's name
/// as in the address: `decision-model`; empty: the first), serving it from
/// a thread of this process, or showing the one already served (by this
/// process or another). Its address, and whether a browser could be
/// opened on it. `path` is the settings file it saves to (`None`:
/// settings kept in memory, which the panel can only show).
pub fn open(path: Option<PathBuf>, tab: &str) -> Result<(String, std::result::Result<(), String>)> {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = running.as_ref()
        && r.alive.load(Ordering::SeqCst)
    {
        let url = r.url.clone();
        drop(running);
        let shown = open_browser(&with_tab(&url, tab)).map_err(|e| e.to_string());
        return Ok((url, shown));
    }
    let url = match Server::bind(path, &config::home_dir())? {
        Bound::Mine(server) => {
            let url = server.url.clone();
            let alive = server.page.alive.clone();
            std::thread::Builder::new()
                .name("panel".into())
                .spawn(move || server.run())
                .map_err(|e| Error::Platform(format!("can't serve the panel: {e}")))?;
            *running = Some(Running {
                url: url.clone(),
                alive,
            });
            url
        }
        Bound::Elsewhere(url) => url,
    };
    drop(running);
    let shown = open_browser(&with_tab(&url, tab)).map_err(|e| e.to_string());
    if let Err(e) = &shown {
        log::warn!("settings panel: {e}");
    }
    Ok((url, shown))
}

/// Serve the panel until the user closes it (or leaves it unused), calling
/// `ready` with its address first (`computer-use-mcp settings`). When
/// another process already serves it, that one is shown instead.
pub fn serve(path: Option<PathBuf>, browser: bool, ready: impl FnOnce(&str)) -> Result<()> {
    match Server::bind(path, &config::home_dir())? {
        Bound::Mine(server) => {
            ready(&server.url);
            if browser {
                open_browser(&server.url)?;
            }
            server.run();
        }
        Bound::Elsewhere(url) => {
            ready(&url);
            if browser {
                open_browser(&url)?;
            }
        }
    }
    Ok(())
}

fn with_tab(url: &str, tab: &str) -> String {
    if tab.is_empty() {
        url.to_string()
    } else {
        format!("{url}#{tab}")
    }
}

/// Open `url` in the user's default browser.
pub fn open_browser(url: &str) -> Result<()> {
    crate::launch::open_url(url)
}

/// 128 random bits as hex.
fn token() -> String {
    let mut bytes = [0u8; 16];
    #[cfg(unix)]
    let filled =
        std::fs::File::open("/dev/urandom").is_ok_and(|mut f| f.read_exact(&mut bytes).is_ok());
    #[cfg(not(unix))]
    let filled = false;
    if !filled {
        // The standard library seeds its hash keys from the system's
        // random source: hashing with fresh keys gives unguessable bits.
        use std::hash::{BuildHasher, Hasher};
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_usize(i);
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            );
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Where the token of the panel on `port` is kept.
fn token_path(home: &Path, port: u16) -> PathBuf {
    home.join(format!("panel-{port}.token"))
}

/// The panel's token on `port`: the one kept from before, else a new one
/// (kept, readable by this user only, so the address stays the same).
fn persistent_token(home: &Path, port: u16) -> String {
    let file = token_path(home, port);
    if let Ok(t) = std::fs::read_to_string(&file) {
        let t = t.trim();
        if t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return t.to_string();
        }
    }
    let t = token();
    if std::fs::create_dir_all(home).is_ok() && std::fs::write(&file, &t).is_ok() {
        config::owner_only(&file);
    }
    t
}

/// Whether a panel with this token answers on `port` (another process of
/// this user serving it).
fn probe(port: u16, token: &str) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(800)));
    let req = format!(
        "GET /{token}/ping HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut out = String::new();
    let _ = s.take(512).read_to_string(&mut out);
    out.starts_with("HTTP/1.1 200") && out.contains("zero-panel")
}

enum Bound {
    Mine(Server),
    /// Another process serves it, at this address.
    Elsewhere(String),
}

struct Server {
    listener: TcpListener,
    url: String,
    page: Arc<Page>,
}

/// What every connection's thread shares.
struct Page {
    host: String,
    token: String,
    path: Option<PathBuf>,
    idle: Duration,
    alive: Arc<AtomicBool>,
    /// When the last request came.
    last: Mutex<Instant>,
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The panel's own settings as saved now (defaults without a file).
fn panel_settings(path: Option<&Path>) -> config::PanelConfig {
    ConfigStore::load(path)
        .map(|s| s.config.panel)
        .unwrap_or_default()
}

impl Server {
    fn bind(path: Option<PathBuf>, home: &Path) -> Result<Bound> {
        let cfg = panel_settings(path.as_deref());
        let token = persistent_token(home, cfg.port);
        let (listener, token) = match TcpListener::bind(("127.0.0.1", cfg.port)) {
            Ok(l) => (l, token),
            Err(_) if probe(cfg.port, &token) => {
                return Ok(Bound::Elsewhere(format!(
                    "http://127.0.0.1:{}/{token}/",
                    cfg.port
                )));
            }
            // Taken by another program: a free port, and a token for it
            // alone.
            Err(_) => (
                TcpListener::bind("127.0.0.1:0")
                    .map_err(|e| Error::Platform(format!("can't serve the panel: {e}")))?,
                self::token(),
            ),
        };
        let port = listener
            .local_addr()
            .map_err(|e| Error::Platform(e.to_string()))?
            .port();
        let host = format!("127.0.0.1:{port}");
        Ok(Bound::Mine(Self {
            url: format!("http://{host}/{token}/"),
            listener,
            page: Arc::new(Page {
                host,
                token,
                path,
                idle: Duration::from_secs(cfg.idle_minutes.clamp(1, 240) * 60),
                alive: Arc::new(AtomicBool::new(true)),
                last: Mutex::new(Instant::now()),
            }),
        }))
    }

    /// Serve until the panel is closed or left unused. Each connection has
    /// a thread of its own: a browser keeps spare connections open without
    /// sending anything on them, and they must not hold up the page.
    fn run(self) {
        use std::sync::atomic::AtomicUsize;
        let _ = self.listener.set_nonblocking(true);
        let open = Arc::new(AtomicUsize::new(0));
        let idle = |p: &Page| p.last.lock().map(|t| t.elapsed() >= p.idle).unwrap_or(true);
        while self.page.alive.load(Ordering::SeqCst) && !idle(&self.page) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                        continue; // dropped: the browser tries again
                    }
                    let _ = stream.set_nonblocking(false);
                    open.fetch_add(1, Ordering::SeqCst);
                    let (page, conns) = (self.page.clone(), open.clone());
                    let spawned =
                        std::thread::Builder::new()
                            .name("panel-conn".into())
                            .spawn(move || {
                                if page.handle(stream) == Some(true) {
                                    page.alive.store(false, Ordering::SeqCst);
                                }
                                conns.fetch_sub(1, Ordering::SeqCst);
                            });
                    if spawned.is_err() {
                        open.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(30)),
            }
        }
        self.page.alive.store(false, Ordering::SeqCst);
    }
}

/// A pointer's picture, for the picker.
fn pointer_png(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "crystal" => include_bytes!("../../assets/cursors/crystal.png"),
        "paper" => include_bytes!("../../assets/cursors/paper.png"),
        "jelly" => include_bytes!("../../assets/cursors/jelly.png"),
        "ice" => include_bytes!("../../assets/cursors/ice.png"),
        "metal" => include_bytes!("../../assets/cursors/metal.png"),
        "orbit" => include_bytes!("../../assets/cursors/orbit.png"),
        _ => return None,
    })
}

impl Page {
    /// Answer one request; `Some(true)` when the user closed the page.
    fn handle(&self, mut stream: TcpStream) -> Option<bool> {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let req = match read_request(&stream) {
            Ok(r) => r,
            Err(code) => {
                respond(&mut stream, code, "text/plain", b"bad request");
                return None;
            }
        };
        if let Ok(mut t) = self.last.lock() {
            *t = Instant::now();
        }
        // DNS rebinding: a page of another site under a name that points
        // here sends its own Host.
        if req.header("host") != Some(self.host.as_str()) {
            respond(&mut stream, 421, "text/plain", b"wrong host");
            return None;
        }
        let prefix = format!("/{}/", self.token);
        let Some(route) = req.path.strip_prefix(&prefix) else {
            respond(&mut stream, 404, "text/plain", b"not found");
            return None;
        };
        let route = route.split('?').next().unwrap_or_default().to_string();
        if req.method == "GET" {
            match route.as_str() {
                "" => {
                    let page = self.shell();
                    respond(
                        &mut stream,
                        200,
                        "text/html; charset=utf-8",
                        page.as_bytes(),
                    );
                }
                "ping" => respond(&mut stream, 200, "text/plain", b"zero-panel"),
                r => match r
                    .strip_prefix("cursor/")
                    .and_then(|n| n.strip_suffix(".png"))
                    .and_then(pointer_png)
                {
                    Some(png) => respond(&mut stream, 200, "image/png", png),
                    None => respond(&mut stream, 404, "text/plain", b"not found"),
                },
            }
            return None;
        }
        if req.method != "POST" {
            respond(&mut stream, 405, "text/plain", b"method not allowed");
            return None;
        }
        // Only the page itself: same origin, JSON (a form on another site
        // can't send that without asking first, and it is never allowed).
        let origin_ok = req
            .header("origin")
            .is_none_or(|o| o == format!("http://{}", self.host));
        let site_ok = req
            .header("sec-fetch-site")
            .is_none_or(|s| s == "same-origin" || s == "none");
        let json_ok = req
            .header("content-type")
            .is_some_and(|c| c.to_ascii_lowercase().starts_with("application/json"));
        if !(origin_ok && site_ok && json_ok) {
            respond(&mut stream, 403, "text/plain", b"forbidden");
            return None;
        }
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let (reply, done) = match route.as_str() {
            "state" => (self.state(), false),
            "set" => (self.set(&body), false),
            "reset" => (self.reset(&body), false),
            "test" => (self.test(&body), false),
            "save" => (self.save(&body), false),
            "remove" => (self.remove(), false),
            "close" => (json!({"ok": true}), true),
            _ => {
                respond(&mut stream, 404, "text/plain", b"not found");
                return None;
            }
        };
        respond(
            &mut stream,
            200,
            "application/json",
            reply.to_string().as_bytes(),
        );
        Some(done)
    }

    /// The settings as saved now.
    fn config(&self) -> std::result::Result<Config, String> {
        match &self.path {
            Some(p) => ConfigStore::load(Some(p))
                .map(|s| s.config)
                .map_err(|e| e.to_string()),
            None => Ok(Config::default()),
        }
    }

    /// The page, with the saved theme in it so it never flashes.
    fn shell(&self) -> String {
        let p = panel_settings(self.path.as_deref());
        let theme = match p.theme.as_str() {
            t @ ("light" | "dark") => t,
            _ => "system",
        };
        let accent = if valid_accent(&p.accent) {
            p.accent.as_str()
        } else {
            "#1A73E8"
        };
        PAGE.replace("__THEME__", theme)
            .replace("__ACCENT__", accent)
    }

    /// Everything the page shows.
    fn state(&self) -> Value {
        let cfg = match self.config() {
            Ok(c) => c,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        let now = toml::Value::try_from(&cfg).unwrap_or(toml::Value::Boolean(false));
        let def = toml::Value::try_from(Config::default()).unwrap_or(toml::Value::Boolean(false));
        let entries: Vec<Value> = ENTRIES
            .iter()
            .map(|e| entry_json(e, lookup(&now, e.key), lookup(&def, e.key)))
            .collect();
        let d = &cfg.decision;
        json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "theme": cfg.panel.theme,
            "accent": cfg.panel.accent,
            "groups": GROUPS,
            "blurbs": blurbs(),
            "entries": entries,
            "decision": {
                "provider": d.provider,
                "base_url": d.base_url,
                "model": d.model,
                "key_hint": config::masked_key(&d.api_key),
                "key_env": d.api_key_env,
            },
        })
    }

    /// Change settings: `{changes: [{key, value}], confirmed}`.
    fn set(&self, body: &Value) -> Value {
        let Some(path) = &self.path else {
            return fail("this server keeps its settings in memory, so they can't be saved here");
        };
        let Some(changes) = body.get("changes").and_then(Value::as_array) else {
            return fail("say which settings to change");
        };
        let def = toml::Value::try_from(Config::default()).unwrap_or(toml::Value::Boolean(false));
        let mut edits: Vec<(&'static str, Edit)> = Vec::new();
        let mut protected = false;
        for c in changes {
            let Some(key) = c.get("key").and_then(Value::as_str) else {
                return fail("a change has no key");
            };
            let Some(entry) = schema::entry(key) else {
                return fail(&format!("`{key}` is not a setting"));
            };
            if entry.kind == Kind::Custom {
                return fail(&format!("`{key}` is changed on its own page"));
            }
            let value = c.get("value").unwrap_or(&Value::Null);
            match coerce(entry, lookup(&def, key), value) {
                Ok(Some(edit)) => {
                    protected |= entry.confirm;
                    edits.push((entry.key, edit));
                }
                Ok(None) => {}
                Err(e) => return fail(&format!("{key}: {e}")),
            }
        }
        if protected && body.get("confirmed").and_then(Value::as_bool) != Some(true) {
            return fail("these settings need the user's confirmation first");
        }
        if edits.is_empty() {
            return self.values(&[]);
        }
        if let Err(e) = config::edit_file_many(path, &edits) {
            return fail(&e.to_string());
        }
        let keys: Vec<&str> = edits.iter().map(|(k, _)| *k).collect();
        self.values(&keys)
    }

    /// Put settings back to their defaults: `{keys: [...], confirmed}`.
    fn reset(&self, body: &Value) -> Value {
        let Some(path) = &self.path else {
            return fail("this server keeps its settings in memory, so they can't be saved here");
        };
        let Some(keys) = body.get("keys").and_then(Value::as_array) else {
            return fail("say which settings to reset");
        };
        let mut edits: Vec<(&'static str, Edit)> = Vec::new();
        let mut protected = false;
        for k in keys {
            let Some(entry) = k.as_str().and_then(schema::entry) else {
                return fail("that is not a setting");
            };
            if entry.kind == Kind::Custom {
                return fail(&format!("`{}` is changed on its own page", entry.key));
            }
            protected |= entry.confirm;
            edits.push((entry.key, Edit::Unset));
        }
        if protected && body.get("confirmed").and_then(Value::as_bool) != Some(true) {
            return fail("these settings need the user's confirmation first");
        }
        if let Err(e) = config::edit_file_many(path, &edits) {
            return fail(&e.to_string());
        }
        let keys: Vec<&str> = edits.iter().map(|(k, _)| *k).collect();
        self.values(&keys)
    }

    /// The saved values of `keys`, for the page to show.
    fn values(&self, keys: &[&str]) -> Value {
        let cfg = match self.config() {
            Ok(c) => c,
            Err(e) => return fail(&e),
        };
        let now = toml::Value::try_from(&cfg).unwrap_or(toml::Value::Boolean(false));
        let mut values = serde_json::Map::new();
        for k in keys {
            if let Some(e) = schema::entry(k) {
                values.insert((*k).into(), shown(e, lookup(&now, k)));
            }
        }
        let mut reply = json!({"ok": true, "values": values});
        if keys.iter().any(|k| k.starts_with("panel.")) {
            reply["theme"] = json!(cfg.panel.theme);
            reply["accent"] = json!(cfg.panel.accent);
        }
        reply
    }

    // The decision model's page.

    /// The page's form merged over the saved settings (an empty key keeps
    /// the saved one).
    fn merged(&self, body: &Value) -> std::result::Result<DecisionConfig, String> {
        let mut d = self.config()?.decision;
        let was_provider = d.provider.clone();
        let was_url = d.base_url.clone();
        let field = |k: &str| {
            body.get(k)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
        };
        if let Some(p) = field("provider") {
            if Provider::parse(&p).is_none() {
                return Err("choose a kind of model".into());
            }
            if p != d.provider {
                // Another provider's address and model don't carry over.
                d.base_url.clear();
                d.model.clear();
            }
            d.provider = p;
        }
        if let Some(u) = field("base_url") {
            let low = u.to_ascii_lowercase();
            if !u.is_empty() && !(low.starts_with("http://") || low.starts_with("https://")) {
                return Err("the address must start with https:// (or http:// for a server on this computer)".into());
            }
            d.base_url = u;
        }
        if let Some(m) = field("model") {
            d.model = m;
        }
        if let Some(k) = field("api_key").filter(|k| !k.is_empty()) {
            d.api_key = k;
            d.api_key_env.clear();
        } else if d.provider != was_provider || d.base_url != was_url {
            // One provider's key, or one address's, is never sent to
            // another: a new address needs its key typed again.
            d.api_key.clear();
            d.api_key_env.clear();
        }
        for v in [&d.base_url, &d.model, &d.api_key] {
            if v.chars().any(char::is_control) {
                return Err("each field must be one line".into());
            }
        }
        Ok(d)
    }

    fn test(&self, body: &Value) -> Value {
        let d = match self.merged(body) {
            Ok(d) => d,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        match model::try_model(&d) {
            Ok(text) => json!({"ok": true, "message": text}),
            Err(e) => json!({"ok": false, "error": e}),
        }
    }

    fn save(&self, body: &Value) -> Value {
        let Some(path) = &self.path else {
            return json!({"ok": false, "error": "this server keeps its settings in memory, so they can't be saved here"});
        };
        let d = match self.merged(body) {
            Ok(d) => d,
            Err(e) => return json!({"ok": false, "error": e}),
        };
        if let Err(e) = Decider::from_config(&d) {
            return json!({"ok": false, "error": e.to_string()});
        }
        match model::save_settings(path, &d) {
            Ok(()) => json!({
                "ok": true,
                "message": format!("Saved to {}. The agent can use it now.", path.display()),
                "key_hint": config::masked_key(&d.api_key),
            }),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        }
    }

    fn remove(&self) -> Value {
        let Some(path) = &self.path else {
            return json!({"ok": false, "error": "this server keeps its settings in memory"});
        };
        match model::remove_settings(path) {
            Ok(()) => {
                json!({"ok": true, "message": "Removed. The agent has no decision model now."})
            }
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        }
    }
}

fn fail(error: &str) -> Value {
    json!({"ok": false, "error": error})
}

fn valid_accent(a: &str) -> bool {
    a.len() == 7 && a.starts_with('#') && a[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A short line under each group's name.
fn blurbs() -> Value {
    json!({
        "General": "Switches that apply to the whole program.",
        "Pointer": "How the agent's own pointer looks and moves on screen. Your real mouse is not touched.",
        "Real mouse": "How the real mouse moves when an action has to use it.",
        "Overlay": "The border, label and colours shown while the agent works.",
        "Screenshots": "When pictures are sent to the model, and how big. The biggest lever on image tokens.",
        "Accessibility tree": "How much of the app's tree the model reads, and how it is shortened.",
        "Tools and tokens": "Which tools the model sees. The tool list is sent with every request.",
        "Timing": "Pauses and waits around actions.",
        "Screen memory": "Remembering screens the model has already seen.",
        "Text on screen (OCR)": "Reading text off the screen for apps whose tree says little.",
        "Decision model": "A fast model that answers small questions about what is on screen.",
        "Privacy": "What is masked before anything reaches the model. Changes ask you to confirm.",
        "Your control": "The emergency stop and the pause while you use the computer. Changes ask you to confirm.",
        "Several agents": "Subagents, or Claude Code beside Codex, on one desktop.",
        "Notifications": "Reading desktop notifications.",
        "Scripts": "Small programs the agent writes and the server runs.",
        "Checking actions": "Verifying that an action did what it should.",
        "Audit log": "A record of every call, without the data.",
        "Server": "Logging, the instructions sent to clients and the optional HTTP transport.",
        "Updates": "How the program keeps itself up to date. Changes ask you to confirm.",
        "Platform": "Settings for one operating system.",
        "Panel": "This page.",
    })
}

/// A setting as the page shows it: a secret only by its last characters,
/// and the decision model's (shown by its own card) not at all.
fn shown(e: &Entry, v: Option<&toml::Value>) -> Value {
    match (e.kind, v) {
        (Kind::Custom, _) => json!(""),
        (Kind::Secret, Some(toml::Value::String(s))) => json!(config::masked_key(s)),
        (Kind::Secret, _) => json!(""),
        (_, Some(v)) => serde_json::to_value(v).unwrap_or(Value::Null),
        (_, None) => json!(""),
    }
}

fn lookup<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut v = root;
    for part in key.split('.') {
        v = v.get(part)?;
    }
    Some(v)
}

fn entry_json(e: &Entry, now: Option<&toml::Value>, def: Option<&toml::Value>) -> Value {
    let type_name = match e.kind {
        Kind::Choice(_) => "choice",
        Kind::Range { .. } => "range",
        Kind::Color => "color",
        Kind::Hotkey => "hotkey",
        Kind::Secret => "secret",
        Kind::Custom => "custom",
        Kind::Number { .. } | Kind::Auto => match def {
            Some(toml::Value::Boolean(_)) => "bool",
            Some(toml::Value::Integer(_)) => "int",
            Some(toml::Value::Float(_)) => "float",
            Some(toml::Value::Array(_)) => "list",
            _ => "string",
        },
    };
    let value = shown(e, now);
    let default = shown(e, def);
    let mut j = json!({
        "key": e.key,
        "group": e.group,
        "help": e.help,
        "type": type_name,
        "value": value,
        "default": default,
        "changed": value != default,
        "restart": e.restart,
        "confirm": e.confirm,
        "advanced": e.advanced,
    });
    match e.kind {
        Kind::Choice(c) => j["choices"] = json!(c),
        Kind::Range {
            min,
            max,
            step,
            unit,
        } => {
            j["min"] = json!(min);
            j["max"] = json!(max);
            j["step"] = json!(step);
            j["unit"] = json!(unit);
        }
        Kind::Number { unit } => j["unit"] = json!(unit),
        _ => {}
    }
    j
}

/// A value from the page as an edit of the settings file, checked against
/// the setting's type: `None` for nothing to change (an empty secret).
fn coerce(
    e: &Entry,
    def: Option<&toml::Value>,
    v: &Value,
) -> std::result::Result<Option<Edit>, String> {
    let line = |s: &str| -> std::result::Result<String, String> {
        if s.chars().any(char::is_control) {
            return Err("must be one line of text".into());
        }
        Ok(s.to_string())
    };
    if let Kind::Secret = e.kind {
        let s = v.as_str().ok_or("must be text")?;
        if s.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Edit::SetText(line(s)?)));
    }
    if let Kind::Choice(c) = e.kind {
        let s = v.as_str().ok_or("must be text")?;
        if !c.contains(&s) {
            return Err(format!("must be one of {}", c.join(", ")));
        }
        return Ok(Some(Edit::SetText(s.to_string())));
    }
    match def {
        Some(toml::Value::Boolean(_)) => v
            .as_bool()
            .map(|b| Some(Edit::Set(b.to_string())))
            .ok_or_else(|| "must be on or off".into()),
        Some(toml::Value::Integer(_)) => {
            let n = number(v)?;
            if n.fract() != 0.0 || !(0.0..=9.0e15).contains(&n) {
                return Err("must be a whole number, not negative".into());
            }
            range_check(e, n)?;
            Ok(Some(Edit::Set(format!("{}", n as i64))))
        }
        Some(toml::Value::Float(_)) => {
            let n = number(v)?;
            range_check(e, n)?;
            Ok(Some(Edit::Set(format!("{n:?}"))))
        }
        Some(toml::Value::Array(_)) => {
            let items = v.as_array().ok_or("must be a list")?;
            let mut out = Vec::new();
            for i in items {
                let s = i.as_str().ok_or("a list holds text")?;
                out.push(toml::Value::String(line(s.trim())?).to_string());
            }
            Ok(Some(Edit::Set(format!("[{}]", out.join(", ")))))
        }
        // Text, and settings that are unset by default (a path).
        _ => {
            let s = v.as_str().ok_or("must be text")?;
            let s = line(s.trim())?;
            if s.is_empty() && def.is_none() {
                return Ok(Some(Edit::Unset));
            }
            Ok(Some(Edit::SetText(s)))
        }
    }
}

fn number(v: &Value) -> std::result::Result<f64, String> {
    v.as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "must be a number".into())
}

fn range_check(e: &Entry, n: f64) -> std::result::Result<(), String> {
    if let Kind::Range { min, max, .. } = e.kind
        && !(min..=max).contains(&n)
    {
        return Err(format!("must be between {min} and {max}"));
    }
    Ok(())
}

fn read_request(stream: &TcpStream) -> std::result::Result<Request, u16> {
    let mut r = BufReader::new(stream.take(MAX_REQUEST as u64 + 8192));
    let mut line = String::new();
    r.read_line(&mut line).map_err(|_| 400u16)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(400u16)?.to_string();
    let path = parts.next().ok_or(400u16)?.to_string();
    let mut headers = Vec::new();
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|_| 400u16)? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if headers.len() > 64 {
            return Err(431);
        }
        let (k, v) = h.split_once(':').ok_or(400u16)?;
        let (k, v) = (k.trim().to_string(), v.trim().to_string());
        if k.eq_ignore_ascii_case("content-length") {
            len = v.parse().map_err(|_| 400u16)?;
        }
        headers.push((k, v));
    }
    if len > MAX_REQUEST {
        return Err(413);
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|_| 400u16)?;
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

fn respond(stream: &mut TcpStream, code: u16, kind: &str, body: &[u8]) {
    let reason = match code {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        421 => "Misdirected Request",
        431 => "Request Header Fields Too Large",
        _ => "Bad Request",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src 'self'; connect-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

const PAGE: &str = include_str!("index.html");

#[cfg(test)]
mod tests;
