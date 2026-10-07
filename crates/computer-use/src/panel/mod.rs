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

mod connecting;
mod help;
mod profiles;
mod raw;
mod schema;
mod settings;
mod status;
mod updates;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use self::schema::Kind;
use self::settings::{apply, fail};
use crate::config::{self, Config, ConfigStore, DecisionConfig};
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
/// The stack of a connection's thread (bytes).
const CONNECTION_STACK: usize = 512 * 1024;

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
    /// The server's folder (profiles and updates are kept in it).
    home: PathBuf,
    /// The program an update replaces: the one running.
    exe: PathBuf,
    /// Where the agents' settings are on this computer.
    env: crate::connect::Env,
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
        let mut env = crate::connect::Env::real();
        env.zero_home = home.to_path_buf();
        Self::bind_with(path, home, std::env::current_exe().unwrap_or_default(), env)
    }

    fn bind_with(
        path: Option<PathBuf>,
        home: &Path,
        exe: PathBuf,
        env: crate::connect::Env,
    ) -> Result<Bound> {
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
                home: home.to_path_buf(),
                exe,
                env,
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
        let mut trimmed = true;
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
                    let spawned = std::thread::Builder::new()
                        .name("panel-conn".into())
                        // A small stack: a request here is a few JSON
                        // documents.
                        .stack_size(CONNECTION_STACK)
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
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(30));
                    // A few seconds after the last request, once: give back
                    // what the requests used (the program goes on running).
                    let quiet = self
                        .page
                        .last
                        .lock()
                        .map(|t| t.elapsed() >= Duration::from_secs(3))
                        .unwrap_or(false);
                    if quiet && !trimmed {
                        crate::engine::trim_heap();
                        trimmed = true;
                    } else if !quiet {
                        trimmed = false;
                    }
                }
            }
        }
        self.page.alive.store(false, Ordering::SeqCst);
        // Give back what the page used: the program goes on running.
        crate::engine::trim_heap();
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
                "schema.json" => respond(
                    &mut stream,
                    200,
                    "application/json",
                    settings::schema_json().as_bytes(),
                ),
                "ping" => respond(&mut stream, 200, "text/plain", b"zero-panel"),
                r if r.starts_with("help/") => match help::page(&r[5..]) {
                    Some(h) => respond(&mut stream, 200, "text/html; charset=utf-8", h.as_bytes()),
                    None => respond(&mut stream, 404, "text/plain", b"not found"),
                },
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
        let (reply, done) = match self.route(&route, &body) {
            Some(r) => r,
            None => {
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
    /// Made once for each theme and accent, then reused: the page is 60 KB
    /// and a browser asks for it on every visit.
    fn shell(&self) -> Arc<String> {
        static CACHE: Mutex<Option<(String, Arc<String>)>> = Mutex::new(None);
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
        let key = format!("{theme} {accent}");
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, page)) = cache.as_ref()
            && *k == key
        {
            return page.clone();
        }
        let page = Arc::new(
            PAGE.replace("__THEME__", theme)
                .replace("__ACCENT__", accent)
                .replace("/*__SCRIPT__*/", APP),
        );
        *cache = Some((key, page.clone()));
        page
    }

    /// The answer to one POST route; the bool is "the user closed the page".
    fn route(&self, route: &str, body: &Value) -> Option<(Value, bool)> {
        let reply = match route {
            "state" => self.state(),
            "set" => self.set(body),
            "reset" => self.reset(body),
            "profiles" => self.with_config(|c| profiles::list(&self.home, c)),
            "profile_apply" => self.profile_apply(body),
            "profile_save" => {
                let label = body
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.with_config(|c| profiles::save(&self.home, c, label))
            }
            "profile_delete" => {
                let id = body.get("id").and_then(Value::as_str).unwrap_or_default();
                profiles::delete(&self.home, id)
            }
            "raw_get" => self.with_path(raw::view),
            "raw_check" | "raw_save" => {
                let text = body.get("text").and_then(Value::as_str).unwrap_or_default();
                let confirmed = body.get("confirmed").and_then(Value::as_bool) == Some(true);
                self.with_path(|p| raw::run(p, text, confirmed, route == "raw_save"))
            }
            "export" => self.with_config(export),
            "import" => self.import(body),
            "overview" => self.with_config(|c| status::overview(self.path.as_deref(), c)),
            "tools" => self.with_config(status::tool_list),
            "apps" => status::apps(),
            "audit" => {
                let n = body.get("lines").and_then(Value::as_u64).unwrap_or(100) as usize;
                self.with_config(|c| status::audit(c, n))
            }
            "path" => {
                let style = body.get("style").and_then(Value::as_str).unwrap_or("mixed");
                status::path_preview(style)
            }
            "connect_list" => connecting::list(&self.env, &self.exe),
            "connect_do" => connecting::act(&self.env, &self.exe, body),
            "update_status" => self.with_config(|c| updates::status(c, &self.updates_dir())),
            "update_check" => self.with_config(|c| updates::check(c, &self.updates_dir())),
            "update_install" => {
                let yes = body.get("confirmed").and_then(Value::as_bool) == Some(true);
                updates::install(&self.updates_dir(), &self.exe, yes)
            }
            "update_rollback" => {
                let yes = body.get("confirmed").and_then(Value::as_bool) == Some(true);
                updates::rollback(self.path.as_deref(), &self.updates_dir(), &self.exe, yes)
            }
            "test" => self.test(body),
            "save" => self.save(body),
            "remove" => self.remove(),
            "close" => return Some((json!({"ok": true}), true)),
            _ => return None,
        };
        Some((reply, false))
    }

    fn updates_dir(&self) -> PathBuf {
        self.home.join("updates")
    }

    fn with_config(&self, f: impl FnOnce(&Config) -> Value) -> Value {
        match self.config() {
            Ok(c) => f(&c),
            Err(e) => fail(&e),
        }
    }

    fn with_path(&self, f: impl FnOnce(&Path) -> Value) -> Value {
        match &self.path {
            Some(p) => f(p),
            None => fail("this server keeps its settings in memory, so there is no file"),
        }
    }

    /// The saved values that differ from the defaults, and who is asking.
    fn state(&self) -> Value {
        let cfg = match self.config() {
            Ok(c) => c,
            Err(e) => return fail(&e),
        };
        let d = &cfg.decision;
        json!({
            "ok": true,
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "theme": cfg.panel.theme,
            "accent": cfg.panel.accent,
            "values": settings::changed_values(&cfg),
            "decision": {
                "provider": d.provider,
                "base_url": d.base_url,
                "model": d.model,
                "key_hint": config::masked_key(&d.api_key),
                "key_env": d.api_key_env,
            },
        })
    }

    fn changes_of(body: &Value) -> std::result::Result<Vec<(String, Value)>, Value> {
        let Some(list) = body.get("changes").and_then(Value::as_array) else {
            return Err(fail("say which settings to change"));
        };
        list.iter()
            .map(|c| match c.get("key").and_then(Value::as_str) {
                Some(k) => Ok((
                    k.to_string(),
                    c.get("value").cloned().unwrap_or(Value::Null),
                )),
                None => Err(fail("a change has no key")),
            })
            .collect()
    }

    /// Change settings: `{changes: [{key, value}], confirmed}`; a `null`
    /// value puts the setting back to its default.
    fn set(&self, body: &Value) -> Value {
        let changes = match Self::changes_of(body) {
            Ok(c) => c,
            Err(e) => return e,
        };
        // A change with no value in it is a mistake, not a reset: only an
        // explicit `null` resets (the page uses `reset` for that).
        if changes.iter().any(|(_, v)| v.is_null()) {
            return fail("a change needs a value");
        }
        let confirmed = body.get("confirmed").and_then(Value::as_bool) == Some(true);
        self.with_path(|p| apply(p, &changes, confirmed))
    }

    /// Put settings back to their defaults: `{keys: [...], confirmed}`.
    fn reset(&self, body: &Value) -> Value {
        let Some(keys) = body.get("keys").and_then(Value::as_array) else {
            return fail("say which settings to reset");
        };
        let mut changes = Vec::new();
        for k in keys {
            let Some(k) = k.as_str() else {
                return fail("that is not a setting");
            };
            changes.push((k.to_string(), Value::Null));
        }
        let confirmed = body.get("confirmed").and_then(Value::as_bool) == Some(true);
        self.with_path(|p| apply(p, &changes, confirmed))
    }

    fn profile_apply(&self, body: &Value) -> Value {
        let id = body.get("id").and_then(Value::as_str).unwrap_or_default();
        let Some(changes) = profiles::changes_for(&self.home, id) else {
            return fail("there is no such profile");
        };
        let confirmed = body.get("confirmed").and_then(Value::as_bool) == Some(true);
        self.with_path(|p| apply(p, &changes, confirmed))
    }

    /// Settings typed or pasted as TOML (what `export` makes): each key
    /// is changed as if set on the page.
    fn import(&self, body: &Value) -> Value {
        let text = body.get("text").and_then(Value::as_str).unwrap_or_default();
        if text.len() > 64 * 1024 {
            return fail("that is too long to be a list of settings");
        }
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => return fail(&e.to_string()),
        };
        let mut changes = Vec::new();
        flatten("", &table, &mut changes);
        if changes.is_empty() {
            return fail("there are no settings in it");
        }
        let confirmed = body.get("confirmed").and_then(Value::as_bool) == Some(true);
        self.with_path(|p| apply(p, &changes, confirmed))
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

fn valid_accent(a: &str) -> bool {
    a.len() == 7 && a.starts_with('#') && a[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Nested TOML as dotted keys with JSON values.
fn flatten(prefix: &str, table: &toml::Table, out: &mut Vec<(String, Value)>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::Table(t) => flatten(&key, t, out),
            v => out.push((key, serde_json::to_value(v).unwrap_or(Value::Null))),
        }
    }
}

fn insert_dotted(table: &mut toml::Table, key: &str, v: toml::Value) {
    match key.split_once('.') {
        None => {
            table.insert(key.to_string(), v);
        }
        Some((head, rest)) => {
            let next = table
                .entry(head.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(t) = next {
                insert_dotted(t, rest, v);
            }
        }
    }
}

/// The settings that differ from the defaults, as a file others can import
/// (no secrets, nothing for the decision model's card).
fn export(cfg: &Config) -> Value {
    let values = settings::changed_values(cfg);
    let mut table = toml::Table::new();
    let mut skipped = 0;
    for (key, value) in values.as_object().into_iter().flatten() {
        let Some(e) = schema::entry(key) else {
            continue;
        };
        if matches!(e.kind, Kind::Secret | Kind::Custom) {
            skipped += 1;
            continue;
        }
        let Ok(v) = toml::Value::try_from(value) else {
            continue;
        };
        insert_dotted(&mut table, key, v);
    }
    json!({
        "ok": true,
        "text": toml::to_string_pretty(&table).unwrap_or_default(),
        "skipped_secrets": skipped,
    })
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
const APP: &str = include_str!("app.js");

#[cfg(test)]
mod tests;
