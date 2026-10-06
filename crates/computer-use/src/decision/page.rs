//! The decision model's settings page: a small web page this program
//! serves on 127.0.0.1 and opens in the user's browser — from the settings
//! key (`control.settings_hotkey`, Ctrl+Alt+J), `decide setup="open"` or
//! `computer-use-mcp settings`. The user picks the model and types its API
//! key there, so the key never goes through the chat; it is saved in the
//! settings file (readable by its owner only), and a running server uses it
//! at once (hot reload).
//!
//! Only this user's browser gets in: the page lives at a random address
//! (a 128-bit token in its path) on the loopback interface, a request for
//! another host name (DNS rebinding) or from another site is refused, and
//! the page goes away after a while unused. The saved key is never sent
//! back to the page, only its last four characters.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Decider, Provider, Question};
use crate::config::{self, ConfigStore, DecisionConfig, Edit};
use crate::error::{Error, Result};

/// The page closes after this long without a request.
const IDLE: Duration = Duration::from_secs(15 * 60);
/// The largest request read.
const MAX_REQUEST: usize = 64 * 1024;

struct Running {
    url: String,
    alive: Arc<AtomicBool>,
}

/// The page being served, if any (one at a time).
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Show the settings page in the user's browser, serving it from a thread
/// of this process (or showing the one already served). Its address, and
/// whether a browser could be opened on it. `path` is the settings file it
/// saves to (`None`: settings kept in memory, which the page can only show).
pub fn open(path: Option<PathBuf>) -> Result<(String, std::result::Result<(), String>)> {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = running.as_ref()
        && r.alive.load(Ordering::SeqCst)
    {
        let url = r.url.clone();
        drop(running);
        let shown = open_browser(&url).map_err(|e| e.to_string());
        return Ok((url, shown));
    }
    let server = Server::bind(path)?;
    let url = server.url.clone();
    let alive = server.page.alive.clone();
    std::thread::Builder::new()
        .name("settings-page".into())
        .spawn(move || server.run())
        .map_err(|e| Error::Platform(format!("can't serve the settings page: {e}")))?;
    *running = Some(Running {
        url: url.clone(),
        alive,
    });
    drop(running);
    let shown = open_browser(&url).map_err(|e| e.to_string());
    if let Err(e) = &shown {
        log::warn!("settings page: {e}");
    }
    Ok((url, shown))
}

/// Serve the page until the user closes it (or leaves it unused), calling
/// `ready` with its address first (`computer-use-mcp settings`).
pub fn serve(path: Option<PathBuf>, browser: bool, ready: impl FnOnce(&str)) -> Result<()> {
    let server = Server::bind(path)?;
    ready(&server.url);
    if browser {
        open_browser(&server.url)?;
    }
    server.run();
    Ok(())
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
    alive: Arc<AtomicBool>,
    /// When the last request came.
    last: Mutex<Instant>,
    /// One change to the settings file at a time.
    editing: Mutex<()>,
}

/// Connections served at once, at most (a browser opens a few spare ones).
const MAX_CONNECTIONS: usize = 16;

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

impl Server {
    fn bind(path: Option<PathBuf>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| Error::Platform(format!("can't serve the settings page: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| Error::Platform(e.to_string()))?
            .port();
        let token = token();
        let host = format!("127.0.0.1:{port}");
        Ok(Self {
            url: format!("http://{host}/{token}/"),
            listener,
            page: Arc::new(Page {
                host,
                token,
                path,
                alive: Arc::new(AtomicBool::new(true)),
                last: Mutex::new(Instant::now()),
                editing: Mutex::new(()),
            }),
        })
    }

    /// Serve until the page is closed or left unused. Each connection has
    /// a thread of its own: a browser keeps spare connections open without
    /// sending anything on them, and they must not hold up the page.
    fn run(self) {
        use std::sync::atomic::AtomicUsize;
        let _ = self.listener.set_nonblocking(true);
        let open = Arc::new(AtomicUsize::new(0));
        let idle = |p: &Page| p.last.lock().map(|t| t.elapsed() >= IDLE).unwrap_or(true);
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
                        .name("settings-page-conn".into())
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
        if req.method == "GET" && route.is_empty() {
            let page = self.page();
            respond(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                page.as_bytes(),
            );
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
            "test" => (self.test(&body), false),
            "save" => (self.save(&body), false),
            "remove" => (self.remove(), false),
            "chat" => (self.chat(&body), false),
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

    /// The settings as saved now (defaults without a file).
    fn current(&self) -> DecisionConfig {
        match &self.path {
            Some(p) => ConfigStore::load(Some(p))
                .map(|s| s.config.decision)
                .unwrap_or_default(),
            None => DecisionConfig::default(),
        }
    }

    /// The page's form merged over the saved settings (an empty key keeps
    /// the saved one).
    fn merged(&self, body: &Value) -> std::result::Result<DecisionConfig, String> {
        let mut d = self.current();
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
        json!(match try_model(&d) {
            Ok(text) => json!({"ok": true, "message": text}),
            Err(e) => json!({"ok": false, "error": e}),
        })
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
        let _one = self.editing.lock();
        match save_settings(path, &d) {
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
        let _one = self.editing.lock();
        match remove_settings(path) {
            Ok(()) => {
                json!({"ok": true, "message": "Removed. The agent has no decision model now."})
            }
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        }
    }

    /// Let the agents on this desktop message each other, or not ([hub]
    /// chat): the user's choice, made here, never through the chat.
    fn chat(&self, body: &Value) -> Value {
        let Some(on) = body.get("on").and_then(Value::as_bool) else {
            return json!({"ok": false, "error": "say on: true or false"});
        };
        let Some(path) = &self.path else {
            return json!({"ok": false, "error": "this server has no settings file"});
        };
        match config::edit_file_many(path, &[("hub.chat", Edit::Set(on.to_string()))]) {
            Ok(()) => json!({"ok": true, "message": if on {
                "Agents on this desktop may now send each other short messages."
            } else {
                "Agents can no longer send each other messages."
            }}),
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        }
    }

    fn page(&self) -> String {
        let d = self.current();
        let chat = match &self.path {
            Some(p) => ConfigStore::load(Some(p))
                .map(|s| s.config.hub.chat)
                .unwrap_or_default(),
            None => false,
        };
        let settings = json!({
            "provider": d.provider,
            "base_url": d.base_url,
            "model": d.model,
            "key_hint": config::masked_key(&d.api_key),
            "key_env": d.api_key_env,
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "chat": chat,
        });
        // In a <script>: nothing in it may end the script element.
        let settings = settings.to_string().replace('<', "\\u003c");
        PAGE.replace("__SETTINGS__", &settings)
    }
}

/// Save the decision model's settings (all or none).
pub fn save_settings(path: &Path, d: &DecisionConfig) -> Result<()> {
    let text = |v: &str| {
        if v.trim().is_empty() {
            Edit::Unset
        } else {
            Edit::SetText(v.trim().to_string())
        }
    };
    let mut edits = vec![
        ("decision.provider", text(&d.provider)),
        ("decision.base_url", text(&d.base_url)),
        ("decision.model", text(&d.model)),
        ("decision.api_key", text(&d.api_key)),
    ];
    if d.api_key_env.trim().is_empty() {
        edits.push(("decision.api_key_env", Edit::Unset));
    }
    config::edit_file_many(path, &edits)
}

/// Forget the decision model.
pub fn remove_settings(path: &Path) -> Result<()> {
    config::edit_file_many(
        path,
        &[
            ("decision.provider", Edit::Unset),
            ("decision.base_url", Edit::Unset),
            ("decision.model", Edit::Unset),
            ("decision.api_key", Edit::Unset),
            ("decision.api_key_env", Edit::Unset),
        ],
    )
}

/// Ask the model one easy question; what to show the user.
pub fn try_model(d: &DecisionConfig) -> std::result::Result<String, String> {
    let decider = Decider::from_config(d)
        .map_err(|e| e.to_string())?
        .ok_or("choose a kind of model first")?;
    try_decider(&decider, &|| false)
}

/// Ask this model one easy question; what to show the user. `halted`
/// stops the wait (the stop key, the client's cancel).
pub fn try_decider(
    decider: &Decider,
    halted: &(dyn Fn() -> bool + Sync),
) -> std::result::Result<String, String> {
    let q = [Question::yes_no("sky", "Is the sky in this text blue?")];
    let (answers, took) = decider
        .ask("The sky over the sea is clear and blue today.", &q, halted)
        .map_err(|e| e.to_string())?;
    let a = answers.first().map(|(_, a)| a.brief()).unwrap_or_default();
    Ok(format!(
        "{} answered in {} ms (a test question, answer {a}).",
        decider.label(),
        took.as_millis()
    ))
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
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

const PAGE: &str = include_str!("page.html");

#[cfg(test)]
mod tests {
    use super::*;

    fn request(server_host: &str, raw: &str) -> String {
        let mut s = TcpStream::connect(server_host).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    fn start(path: Option<PathBuf>) -> (String, String, Arc<AtomicBool>) {
        let server = Server::bind(path).unwrap();
        let (host, token, alive) = (
            server.page.host.clone(),
            server.page.token.clone(),
            server.page.alive.clone(),
        );
        std::thread::spawn(move || server.run());
        (host, token, alive)
    }

    #[test]
    fn only_the_page_itself_gets_in() {
        let dir = std::env::temp_dir().join(format!("cu-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[decision]\nprovider = \"jev\"\napi_key = \"sk-saved-abcd1234\"\n",
        )
        .unwrap();
        let (host, token, alive) = start(Some(path.clone()));

        // The page, without the key (only its end).
        let page = request(
            &host,
            &format!("GET /{token}/ HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(
            page.contains("••••1234") && !page.contains("sk-saved"),
            "the key leaked"
        );

        // Without the token, another host name, or from another site: no.
        let no_token = request(&host, &format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n"));
        assert!(no_token.starts_with("HTTP/1.1 404"), "{no_token}");
        let rebound = request(
            &host,
            &format!("GET /{token}/ HTTP/1.1\r\nHost: evil.example:80\r\n\r\n"),
        );
        assert!(rebound.starts_with("HTTP/1.1 421"), "{rebound}");
        let body = r#"{"provider":"openai","model":"m","api_key":"sk-new-key-5678"}"#;
        let cross = request(
            &host,
            &format!(
                "POST /{token}/save HTTP/1.1\r\nHost: {host}\r\nOrigin: https://evil.example\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(cross.starts_with("HTTP/1.1 403"), "{cross}");
        let form = request(
            &host,
            &format!(
                "POST /{token}/save HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(form.starts_with("HTTP/1.1 403"), "{form}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("sk-saved"));

        // The page's own save.
        let saved = request(
            &host,
            &format!(
                "POST /{token}/save HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(saved.contains("\"ok\":true"), "{saved}");
        let cfg = ConfigStore::load(Some(&path)).unwrap().config.decision;
        assert_eq!(cfg.provider, "openai");
        assert_eq!(cfg.model, "m");
        assert_eq!(cfg.api_key, "sk-new-key-5678");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
        }

        // An empty key keeps the saved one.
        let body = r#"{"provider":"openai","model":"m2","api_key":""}"#;
        request(
            &host,
            &format!(
                "POST /{token}/save HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        let cfg = ConfigStore::load(Some(&path)).unwrap().config.decision;
        assert_eq!(
            (cfg.model.as_str(), cfg.api_key.as_str()),
            ("m2", "sk-new-key-5678")
        );

        // Another provider with no key typed: the old one doesn't go along.
        let body = r#"{"provider":"jev","api_key":""}"#;
        request(
            &host,
            &format!(
                "POST /{token}/save HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        let cfg = ConfigStore::load(Some(&path)).unwrap().config.decision;
        assert_eq!((cfg.provider.as_str(), cfg.api_key.as_str()), ("jev", ""));

        // The agents' messages: switched on and off here, by the user.
        let page = request(
            &host,
            &format!("GET /{token}/ HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(page.contains("\"chat\":false"), "{page}");
        for on in [true, false] {
            let body = format!(r#"{{"on":{on}}}"#);
            let r = request(
                &host,
                &format!(
                    "POST /{token}/chat HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                ),
            );
            assert!(r.contains("\"ok\":true"), "{r}");
            assert_eq!(ConfigStore::load(Some(&path)).unwrap().config.hub.chat, on);
        }

        // Remove, then close.
        let body = "{}";
        let removed = request(
            &host,
            &format!(
                "POST /{token}/remove HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(removed.contains("\"ok\":true"), "{removed}");
        assert_eq!(
            ConfigStore::load(Some(&path)).unwrap().config.decision,
            DecisionConfig::default()
        );
        request(
            &host,
            &format!(
                "POST /{token}/close HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
            ),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while alive.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_silent_connection_doesnt_hold_up_the_page() {
        let (host, token, alive) = start(None);
        // A browser's spare connection: open, and nothing sent on it.
        let _spare = TcpStream::connect(&host).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        let page = request(
            &host,
            &format!("GET /{token}/ HTTP/1.1\r\nHost: {host}\r\n\r\n"),
        );
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        alive.store(false, Ordering::SeqCst);
    }

    #[test]
    fn tokens_differ() {
        let (a, b) = (token(), token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
