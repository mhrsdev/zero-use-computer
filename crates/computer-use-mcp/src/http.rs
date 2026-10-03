//! Optional remote transport: MCP over HTTP (Streamable HTTP, MCP
//! 2025-03-26 and later). Enabled with the `http` cargo feature.
//!
//! * **POST** one JSON-RPC message or a batch. Requests are answered with
//!   JSON, or with a stream of events (`text/event-stream`) when the client
//!   takes one and asks for a call's progress: the progress, then the
//!   answer. Notifications and responses alone get 202.
//! * **GET** with `Accept: text/event-stream`: a stream on which the server
//!   tells the client its tool list changed.
//! * **DELETE** ends the session.
//!
//! `initialize` is answered with an `Mcp-Session-Id`. A request naming a
//! session the server doesn't know (it ended, or the server restarted) gets
//! 404, so the client starts again; one naming none is served as before.
//!
//! Requests are read on a thread of their own and answered, in order, by
//! the thread that called [`serve`] (the engine's): a cancel reaches the
//! call that is running, and a ping is answered at once.
//!
//! Whoever can reach the endpoint can control the desktop, so a bearer
//! token is required, requests from web pages on other origins are refused
//! (and, bound to this machine, those naming another host: DNS rebinding),
//! and it should be bound to localhost or a trusted network.

use std::collections::VecDeque;
use std::io::{Read as _, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use computer_use::Backend;
use computer_use::engine::Engine;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::core::{Cancels, Core, Notifier, PROTOCOL_VERSIONS, cancelled_request};
use crate::jsonrpc::{INVALID_REQUEST, Incoming, Payload, Response as RpcResponse, parse_payload};

/// Largest request body accepted (a JSON-RPC message is far smaller).
const MAX_BODY: u64 = 4 * 1024 * 1024;
/// Refused requests answered on threads of their own at once, at most.
const MAX_REFUSING: usize = 16;
/// Sessions remembered (the oldest is forgotten first).
const MAX_SESSIONS: usize = 64;
/// Event streams open at once (GET), at most.
const MAX_STREAMS: usize = 16;
/// A comment on every open stream this often, so one whose client left is
/// found and closed.
const KEEP_ALIVE: Duration = Duration::from_secs(25);
/// JSON-RPC: the request was cancelled (no answer was wanted, but HTTP
/// needs one).
const REQUEST_CANCELLED: i64 = -32800;

/// Serve MCP over HTTP until the process is stopped. `token` is required.
pub fn serve<B: Backend>(engine: Engine<B>, addr: &str, token: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!token.is_empty(), "serving over HTTP needs a bearer token");
    let server = Server::http(addr)
        .map_err(|e| anyhow::anyhow!("cannot bind HTTP server on {addr}: {e}"))?;
    log::info!("computer-use-mcp serving MCP over HTTP on {addr}");
    let local = match addr.parse::<SocketAddr>() {
        Ok(a) => a.ip().is_loopback(),
        Err(_) => addr.starts_with("localhost:"),
    };
    if !local {
        log::warn!(
            "{addr} is reachable from other machines: anyone there with the token can control this desktop"
        );
    }
    run(engine, Arc::new(server), token.to_string(), local, None);
    Ok(())
}

/// What the reading thread and the engine's share.
struct Shared {
    token: String,
    /// Bound to this machine: the `Host` must name it.
    local: bool,
    cancels: Cancels,
    engine_cancel: Arc<AtomicBool>,
    sessions: Mutex<VecDeque<String>>,
    /// Open GET streams, by session.
    streams: Mutex<Vec<(Option<String>, Sse)>>,
}

/// A POST for the engine to answer.
struct Job {
    /// None for notifications, already answered with 202.
    request: Option<Request>,
    items: Vec<Result<Incoming, Box<RpcResponse>>>,
    batch: bool,
    session: Option<String>,
    /// The session `initialize` starts, sent back as `Mcp-Session-Id`.
    new_session: Option<String>,
    /// Answer with an event stream (the client takes one and asked for
    /// progress).
    stream: bool,
}

/// Serve until `stop` is set, or for good.
fn run<B: Backend>(
    engine: Engine<B>,
    server: Arc<Server>,
    token: String,
    local: bool,
    stop: Option<Arc<AtomicBool>>,
) {
    let shared = Arc::new(Shared {
        token,
        local,
        cancels: Cancels::default(),
        engine_cancel: engine.cancel_handle(),
        sessions: Mutex::new(VecDeque::new()),
        streams: Mutex::new(Vec::new()),
    });
    let mut core = Core::new(engine, true);
    let (tx, rx) = channel::<Job>();
    {
        let shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("http-reader".into())
            .spawn(move || accept_loop(&server, &shared, &tx, stop.as_deref()));
        if let Err(e) = spawned {
            log::error!("cannot start the HTTP reader: {e}");
            return;
        }
    }
    // Ends when the reader stops (and drops its sender).
    for job in rx {
        answer(&mut core, &shared, job);
    }
}

fn accept_loop(server: &Server, shared: &Shared, tx: &Sender<Job>, stop: Option<&AtomicBool>) {
    let mut kept_alive = Instant::now();
    loop {
        if stop.is_some_and(|s| s.load(Ordering::SeqCst)) {
            return;
        }
        if kept_alive.elapsed() >= KEEP_ALIVE {
            kept_alive = Instant::now();
            lock(&shared.streams).retain_mut(|(_, s)| s.comment().is_ok());
        }
        let request = match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("http recv error: {e}");
                continue;
            }
        };
        if let Some(job) = triage(request, shared)
            && tx.send(job).is_err()
        {
            return;
        }
    }
}

/// Check a request, answer what needs no engine, and pass on the rest.
fn triage(mut request: Request, shared: &Shared) -> Option<Job> {
    if !authorized(&request, &shared.token) {
        refuse(request, 401, "unauthorized: send the bearer token");
        return None;
    }
    if !local_origin(&request) {
        refuse(request, 403, "requests from other origins are not allowed");
        return None;
    }
    if shared.local && !local_host(&request) {
        refuse(request, 403, "the Host header must name this machine");
        return None;
    }
    if let Some(v) = header(&request, "MCP-Protocol-Version")
        && !PROTOCOL_VERSIONS.contains(&v.trim())
    {
        let msg = format!(
            "unsupported MCP-Protocol-Version {v}: this server speaks {}",
            PROTOCOL_VERSIONS.join(", ")
        );
        refuse(request, 400, &msg);
        return None;
    }
    let session = header(&request, "Mcp-Session-Id").map(str::to_string);
    if let Some(s) = &session
        && !lock(&shared.sessions).contains(s)
    {
        refuse(request, 404, "unknown session: initialize again");
        return None;
    }
    match request.method() {
        Method::Get => {
            if !accepts(&request, "text/event-stream") {
                refuse(
                    request,
                    405,
                    "GET opens an event stream (Accept: text/event-stream)",
                );
                return None;
            }
            if lock(&shared.streams).len() >= MAX_STREAMS {
                refuse(request, 503, "too many event streams open");
                return None;
            }
            match Sse::start(request, None) {
                Ok(sse) => lock(&shared.streams).push((session, sse)),
                Err(e) => log::debug!("event stream not opened: {e}"),
            }
            return None;
        }
        Method::Delete => {
            let Some(s) = session else {
                refuse(request, 400, "DELETE needs an Mcp-Session-Id");
                return None;
            };
            lock(&shared.sessions).retain(|x| *x != s);
            for (_, sse) in extract(&shared.streams, |(o, _)| o.as_deref() == Some(&s)) {
                sse.end();
            }
            let _ = request.respond(Response::empty(200));
            return None;
        }
        Method::Post => {}
        _ => {
            refuse(request, 405, "use POST, GET or DELETE");
            return None;
        }
    }
    if !header(&request, "Content-Type")
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"))
    {
        refuse(request, 415, "Content-Type must be application/json");
        return None;
    }
    if request.body_length().is_some_and(|n| n as u64 > MAX_BODY) {
        refuse(request, 413, "request body too large");
        return None;
    }
    let mut body = String::new();
    let read = request
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_string(&mut body);
    if read.is_err() || body.len() as u64 > MAX_BODY {
        refuse(request, 400, "unreadable or too large body");
        return None;
    }
    let (items, batch) = match parse_payload(&body) {
        Ok(Payload::One(m)) => (vec![Ok(m)], false),
        Ok(Payload::Batch(items)) => (items, true),
        Err(reply) => {
            respond(request, 400, json!(*reply), &[]);
            return None;
        }
    };
    let key = |id: Value| json!([session, id]);
    for msg in items.iter().flatten() {
        if let Some(id) = cancelled_request(msg) {
            shared.cancels.cancel(key(id), &shared.engine_cancel);
        }
    }
    let is_request = |m: &Incoming| m.method.is_some() && m.id.is_some();
    let requests = items.iter().flatten().filter(|m| is_request(m)).count();
    let unusable = items.iter().any(Result::is_err);
    if requests == 0 && !unusable {
        // Notifications and responses: accepted. The engine still sees
        // those it acts on (the overlay's status).
        let _ = request.respond(Response::empty(202));
        let acted = items
            .iter()
            .flatten()
            .any(|m| m.method.is_some() && m.method.as_deref() != Some("notifications/cancelled"));
        return acted.then_some(Job {
            request: None,
            items,
            batch,
            session,
            new_session: None,
            stream: false,
        });
    }
    // A ping is answered here, so a client checking that the server is
    // alive isn't kept waiting by a long call.
    if let [Ok(m)] = items.as_slice()
        && !batch
        && m.method.as_deref() == Some("ping")
        && let Some(id) = m.id.clone()
    {
        respond(request, 200, json!(RpcResponse::ok(id, json!({}))), &[]);
        return None;
    }
    let initializes = items
        .iter()
        .flatten()
        .any(|m| m.method.as_deref() == Some("initialize"));
    let new_session = initializes.then(|| {
        let id = new_session_id();
        let mut sessions = lock(&shared.sessions);
        if sessions.len() >= MAX_SESSIONS {
            sessions.pop_front();
        }
        sessions.push_back(id.clone());
        id
    });
    let wants_progress = items.iter().flatten().any(|m| {
        m.method.as_deref() == Some("tools/call")
            && m.params
                .as_ref()
                .and_then(|p| p.get("_meta"))
                .is_some_and(|meta| meta.get("progressToken").is_some())
    });
    let stream = wants_progress
        && accepts(&request, "text/event-stream")
        && *request.http_version() >= tiny_http::HTTPVersion(1, 1);
    Some(Job {
        request: Some(request),
        items,
        batch,
        session: new_session.clone().or(session),
        new_session,
        stream,
    })
}

/// Answer a job's requests with the engine, in order.
fn answer<B: Backend>(core: &mut Core<B>, shared: &Shared, job: Job) {
    let session_header: Vec<(&str, String)> = job
        .new_session
        .iter()
        .map(|s| ("Mcp-Session-Id", s.clone()))
        .collect();
    let headers: Vec<(&str, &str)> = session_header
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .collect();
    let key = |id: &Option<Value>| json!([job.session, id.clone().unwrap_or(Value::Null)]);
    let sse = match job.request {
        Some(request) if job.stream => match Sse::start(request, Some(&headers)) {
            Ok(sse) => Some(Arc::new(Mutex::new(sse))),
            Err(e) => {
                log::debug!("the client left before its answer: {e}");
                return;
            }
        },
        Some(request) => {
            let mut answers = Vec::new();
            let mut cancelled = Vec::new();
            for item in job.items {
                match item {
                    Ok(msg) => {
                        let (id, k) = (msg.id.clone(), key(&msg.id));
                        let request = msg.method.is_some() && id.is_some();
                        match core.handle(msg, &shared.cancels, &k, None) {
                            Some(r) => answers.push(r),
                            None if request => cancelled.extend(id),
                            None => {}
                        }
                    }
                    Err(r) => answers.push(*r),
                }
            }
            let body = if job.batch {
                (!answers.is_empty()).then(|| json!(answers))
            } else {
                // HTTP needs an answer even for a cancelled request.
                answers.pop().map(|a| json!(a)).or_else(|| {
                    cancelled.pop().map(|id| {
                        json!(RpcResponse::err(id, REQUEST_CANCELLED, "request cancelled"))
                    })
                })
            };
            match body {
                Some(body) => respond(request, 200, body, &headers),
                None => {
                    let _ = request.respond(Response::empty(202));
                }
            }
            announce(core, shared);
            return;
        }
        None => None,
    };
    let notify: Option<Notifier> = sse.clone().map(|sse| -> Notifier {
        Arc::new(move |v: Value| {
            let _ = lock(&sse).event(&v);
        })
    });
    for item in job.items {
        let answer = match item {
            Ok(msg) => {
                let k = key(&msg.id);
                core.handle(msg, &shared.cancels, &k, notify.as_ref())
            }
            Err(r) => Some(*r),
        };
        if let (Some(sse), Some(a)) = (&sse, answer) {
            let _ = lock(sse).event(&json!(a));
        }
    }
    drop(notify);
    if let Some(sse) = sse.and_then(Arc::into_inner) {
        sse.into_inner().unwrap_or_else(|e| e.into_inner()).end();
    }
    announce(core, shared);
}

/// Tell every open stream the tool list changed, if it did.
fn announce<B: Backend>(core: &mut Core<B>, shared: &Shared) {
    if let Some(note) = core.tools_changed() {
        lock(&shared.streams).retain_mut(|(_, s)| s.event(&note).is_ok());
    }
}

/// A `text/event-stream` answer, written straight to the connection in
/// chunks, each sent as it is written (`tiny_http` would hold small ones
/// back).
struct Sse {
    w: Box<dyn Write + Send>,
}

impl Sse {
    fn start(request: Request, headers: Option<&[(&str, &str)]>) -> std::io::Result<Self> {
        let mut w = request.into_writer();
        let mut head = String::from(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n",
        );
        for (k, v) in headers.unwrap_or_default() {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        w.write_all(head.as_bytes())?;
        w.flush()?;
        Ok(Self { w })
    }

    fn event(&mut self, message: &Value) -> std::io::Result<()> {
        self.chunk(&format!("event: message\ndata: {message}\n\n"))
    }

    fn comment(&mut self) -> std::io::Result<()> {
        self.chunk(": keep-alive\n\n")
    }

    fn chunk(&mut self, text: &str) -> std::io::Result<()> {
        write!(self.w, "{:x}\r\n{text}\r\n", text.len())?;
        self.w.flush()
    }

    fn end(mut self) {
        let _ = self.w.write_all(b"0\r\n\r\n");
        let _ = self.w.flush();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Take the items matching `pred` out of the list.
fn extract<T>(list: &Mutex<Vec<T>>, pred: impl Fn(&T) -> bool) -> Vec<T> {
    let mut list = lock(list);
    let (taken, kept) = std::mem::take(&mut *list).into_iter().partition(pred);
    *list = kept;
    taken
}

/// A session id no one can guess: 128 bits from the standard library's
/// randomly keyed hasher, never the same twice in a process.
fn new_session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0..2u8)
        .map(|half| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u8(half);
            h.write_u64(n);
            h.write_u128(nanos);
            h.write_u32(std::process::id());
            format!("{:016x}", h.finish())
        })
        .collect()
}

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// The client takes `mime` (`Accept`, any of its listed types).
fn accepts(request: &Request, mime: &str) -> bool {
    header(request, "Accept").is_some_and(|v| {
        v.split(',').any(|t| {
            t.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case(mime)
        })
    })
}

/// The bearer token matches (compared in constant time).
fn authorized(request: &Request, expected: &str) -> bool {
    let Some(given) = header(request, "Authorization").and_then(bearer) else {
        return false;
    };
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token of `Bearer <token>`; the scheme's case doesn't matter (RFC
/// 7235), and neither do spaces around it.
fn bearer(value: &str) -> Option<&str> {
    let value = value.trim();
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// `localhost`, `127.0.0.1` or `::1`, from a URL's or a header's host part
/// (with or without a port).
fn is_local_name(host: &str) -> bool {
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

/// No `Origin` (not a browser), or a page on this machine. Browsers send
/// `Origin` with cross-site requests; this stops a web page from driving the
/// desktop (DNS rebinding included).
fn local_origin(request: &Request) -> bool {
    let Some(origin) = header(request, "Origin") else {
        return true;
    };
    is_local_name(
        origin
            .split("://")
            .nth(1)
            .unwrap_or("")
            .trim_end_matches('/'),
    )
}

/// The `Host` names this machine (or is missing: HTTP/1.0). A page on a
/// name that resolves here (DNS rebinding) sends its own name.
fn local_host(request: &Request) -> bool {
    header(request, "Host").is_none_or(|h| is_local_name(h.trim()))
}

/// Refuse a request: the HTTP status, and a JSON-RPC error saying why.
fn refuse(request: Request, status: u16, why: &str) {
    let body = json!(RpcResponse::err(Value::Null, INVALID_REQUEST, why));
    let auth = [("WWW-Authenticate", "Bearer")];
    respond(
        request,
        status,
        body,
        if status == 401 { &auth } else { &[] },
    );
}

fn respond(request: Request, status: u16, body: Value, headers: &[(&str, &str)]) {
    let data = body.to_string();
    let mut response = Response::from_string(data)
        .with_status_code(status)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    for (k, v) in headers {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            response = response.with_header(h);
        }
    }
    let send = move || {
        let _ = request.respond(response);
    };
    // A refused request may still be sending its body, which tiny_http
    // reads to the end once it is answered: that happens on a thread of its
    // own, so a client sending gigabytes can't hold up the others. At most
    // a few such threads at once; past that, refusals wait their turn.
    static REFUSING: AtomicUsize = AtomicUsize::new(0);
    if status >= 400 && REFUSING.fetch_add(1, Ordering::SeqCst) < MAX_REFUSING {
        let spawned = std::thread::Builder::new()
            .name("http-refuse".into())
            .spawn(move || {
                send();
                REFUSING.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            REFUSING.fetch_sub(1, Ordering::SeqCst);
        }
    } else {
        if status >= 400 {
            REFUSING.fetch_sub(1, Ordering::SeqCst);
        }
        send();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bearer_scheme_in_any_case() {
        assert_eq!(bearer("Bearer abc"), Some("abc"));
        assert_eq!(bearer("bearer abc"), Some("abc"));
        assert_eq!(bearer("  BEARER   abc "), Some("abc"));
        assert_eq!(bearer("Basic abc"), None);
        assert_eq!(bearer("Bearer "), None);
        assert_eq!(bearer("Bearerabc"), None);
    }

    fn request(headers: &[(&str, &str)]) -> Request {
        let mut test = tiny_http::TestRequest::new().with_method(Method::Post);
        for (k, v) in headers {
            test = test.with_header(Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap());
        }
        test.into()
    }

    #[test]
    fn token_is_required_and_exact() {
        assert!(authorized(
            &request(&[("Authorization", "Bearer s3cret")]),
            "s3cret"
        ));
        assert!(!authorized(
            &request(&[("Authorization", "Bearer s3cre")]),
            "s3cret"
        ));
        assert!(!authorized(
            &request(&[("Authorization", "s3cret")]),
            "s3cret"
        ));
        assert!(!authorized(&request(&[]), "s3cret"));
    }

    #[test]
    fn only_local_pages_may_call() {
        assert!(local_origin(&request(&[])));
        for ok in [
            "http://localhost:3000",
            "http://127.0.0.1",
            "http://[::1]:8787",
        ] {
            assert!(local_origin(&request(&[("Origin", ok)])), "{ok}");
        }
        for bad in [
            "https://evil.example",
            "http://localhost.evil.example",
            "null",
        ] {
            assert!(!local_origin(&request(&[("Origin", bad)])), "{bad}");
        }
    }

    #[test]
    fn only_this_machine_as_the_host() {
        for ok in ["localhost:8787", "127.0.0.1", "[::1]:8787", "LOCALHOST"] {
            assert!(local_host(&request(&[("Host", ok)])), "{ok}");
        }
        for bad in ["evil.example:8787", "localhost.evil.example", "10.0.0.2"] {
            assert!(!local_host(&request(&[("Host", bad)])), "{bad}");
        }
    }

    #[test]
    fn accept_lists_are_read_by_type() {
        let r = request(&[("Accept", "application/json, text/event-stream;q=0.9")]);
        assert!(accepts(&r, "text/event-stream") && accepts(&r, "application/json"));
        assert!(!accepts(
            &request(&[("Accept", "application/json")]),
            "text/event-stream"
        ));
    }

    #[test]
    fn session_ids_differ_and_are_128_bits() {
        let (a, b) = (new_session_id(), new_session_id());
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // -- end to end, over a socket --------------------------------------

    use computer_use::config::{Config, ConfigStore, ToolManager};
    use computer_use::mock::MockBackend;
    use std::io::{BufRead as _, BufReader};
    use std::net::TcpStream;

    const TOKEN: &str = "t0ken";

    fn engine(scripts: &std::path::Path) -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut config = Config::default();
        config.script.dir = Some(scripts.to_path_buf());
        config.tools.manager = ToolManager::Off;
        Engine::new(backend, ConfigStore::in_memory(config)).with_time(Instant::now, |_| {})
    }

    /// Serve on a free port while `client` runs; its panics fail the test.
    fn serving(name: &str, client: impl FnOnce(SocketAddr) + Send + 'static) {
        let dir = std::env::temp_dir().join(format!("cu-http-{name}-{}", std::process::id()));
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let addr = server.server_addr().to_ip().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let client = std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| client(addr)));
            done.store(true, Ordering::SeqCst);
            r
        });
        run(engine(&dir), server, TOKEN.into(), true, Some(stop));
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(p) = client.join().unwrap() {
            std::panic::resume_unwind(p);
        }
    }

    struct Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Reply {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
        fn json(&self) -> Value {
            serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
        }
        /// The messages of an event stream.
        fn events(&self) -> Vec<Value> {
            self.body
                .lines()
                .filter_map(|l| l.strip_prefix("data: "))
                .map(|d| serde_json::from_str(d).unwrap())
                .collect()
        }
    }

    fn open(
        addr: SocketAddr,
        method: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> BufReader<TcpStream> {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut req = format!(
            "{method} /mcp HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("Host")) {
            req.push_str(&format!("Host: {addr}\r\n"));
        }
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).unwrap();
        BufReader::new(s)
    }

    fn read_head(r: &mut BufReader<TcpStream>) -> (u16, Vec<(String, String)>) {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let status = line.split(' ').nth(1).unwrap().parse().unwrap();
        let mut headers = Vec::new();
        loop {
            line.clear();
            r.read_line(&mut line).unwrap();
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            let (k, v) = l.split_once(':').unwrap();
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
        (status, headers)
    }

    /// The next chunk of a chunked body (None at its end).
    fn read_chunk(r: &mut BufReader<TcpStream>) -> Option<String> {
        let mut line = String::new();
        r.read_line(&mut line).ok()?;
        let len = usize::from_str_radix(line.trim(), 16).ok()?;
        let mut buf = vec![0; len + 2];
        r.read_exact(&mut buf).ok()?;
        (len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned())
    }

    fn send(addr: SocketAddr, method: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut r = open(addr, method, headers, body);
        let (status, headers) = read_head(&mut r);
        let chunked = headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("Transfer-Encoding") && v.contains("chunked"));
        let body = if chunked {
            std::iter::from_fn(|| read_chunk(&mut r)).collect()
        } else {
            let mut b = String::new();
            let _ = r.read_to_string(&mut b);
            b
        };
        Reply {
            status,
            headers,
            body,
        }
    }

    const AUTH: (&str, &str) = ("Authorization", "Bearer t0ken");
    const JSON: (&str, &str) = ("Content-Type", "application/json");

    fn post(addr: SocketAddr, session: Option<&str>, body: Value) -> Reply {
        let mut headers = vec![
            AUTH,
            JSON,
            ("Accept", "application/json, text/event-stream"),
        ];
        if let Some(s) = session {
            headers.push(("Mcp-Session-Id", s));
        }
        send(addr, "POST", &headers, &body.to_string())
    }

    fn initialize(addr: SocketAddr) -> String {
        let r = post(
            addr,
            None,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(
            r.json()["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
        r.header("Mcp-Session-Id").expect("a session").to_string()
    }

    #[test]
    fn sessions_batches_and_refusals() {
        serving("sessions", |addr| {
            let ping = json!({"jsonrpc":"2.0","id":5,"method":"ping"}).to_string();
            let r = send(addr, "POST", &[JSON], &ping);
            assert_eq!(r.status, 401);
            assert_eq!(r.header("WWW-Authenticate"), Some("Bearer"));
            assert_eq!(r.json()["error"]["code"], INVALID_REQUEST);
            let r = send(addr, "POST", &[AUTH, JSON, ("Host", "evil.example")], &ping);
            assert_eq!(r.status, 403);

            let session = initialize(addr);
            assert_eq!(session.len(), 32);
            let r = send(
                addr,
                "POST",
                &[AUTH, JSON, ("MCP-Protocol-Version", "1999-01-01")],
                &ping,
            );
            assert_eq!(r.status, 400, "{}", r.body);
            let r = post(
                addr,
                Some("feedbeef"),
                json!({"jsonrpc":"2.0","id":5,"method":"ping"}),
            );
            assert_eq!(r.status, 404);

            let r = post(
                addr,
                Some(&session),
                json!([
                    {"jsonrpc":"2.0","id":2,"method":"ping"},
                    {"jsonrpc":"2.0","method":"notifications/initialized"},
                    {"jsonrpc":"2.0","id":3,"method":"tools/list"}
                ]),
            );
            assert_eq!(r.status, 200);
            let answers = r.json();
            assert_eq!(answers.as_array().unwrap().len(), 2, "{answers}");
            assert!(answers[1]["result"]["tools"].as_array().unwrap().len() > 10);

            let r = post(
                addr,
                Some(&session),
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            );
            assert_eq!(r.status, 202);
            let r = send(addr, "POST", &[AUTH, JSON], "{oops");
            assert_eq!(r.status, 400);
            assert_eq!(r.json()["error"]["code"], crate::jsonrpc::PARSE_ERROR);

            let r = send(addr, "DELETE", &[AUTH, ("Mcp-Session-Id", &session)], "");
            assert_eq!(r.status, 200);
            let r = post(
                addr,
                Some(&session),
                json!({"jsonrpc":"2.0","id":5,"method":"ping"}),
            );
            assert_eq!(r.status, 404);
        });
    }

    #[test]
    fn progress_comes_as_events_and_a_cancel_ends_the_call() {
        serving("progress", |addr| {
            let session = initialize(addr);
            let steps = json!([
                {"tool": "get_app_state", "arguments": {}},
                {"tool": "set_value", "arguments": {"element_index": 4, "value": "hi"}}
            ]);
            let r = post(
                addr,
                Some(&session),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                    "name":"batch","arguments":{"app":"TextEdit","steps":steps},
                    "_meta":{"progressToken":7}}}),
            );
            assert_eq!(r.status, 200);
            assert_eq!(r.header("Content-Type"), Some("text/event-stream"));
            let events = r.events();
            assert_eq!(events.len(), 3, "{}", r.body);
            assert_eq!(events[0]["method"], "notifications/progress");
            assert_eq!(events[1]["params"]["progressToken"], 7);
            assert_eq!(events[2]["id"], 2);
            assert_eq!(events[2]["result"]["isError"], false);

            // A call that runs until it is cancelled, from another request.
            let s = session.clone();
            let call = std::thread::spawn(move || {
                post(
                    addr,
                    Some(&s),
                    json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{
                        "name":"script","arguments":{"code":"let n = 0; loop { n += 1; }"}}}),
                )
            });
            std::thread::sleep(Duration::from_millis(400));
            let started = Instant::now();
            let r = post(
                addr,
                Some(&session),
                json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9}}),
            );
            assert_eq!(r.status, 202);
            let r = call.join().unwrap();
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the cancel didn't end it"
            );
            assert_eq!(r.json()["error"]["code"], REQUEST_CANCELLED, "{}", r.body);
            // A ping isn't kept waiting either way.
            let r = post(
                addr,
                Some(&session),
                json!({"jsonrpc":"2.0","id":10,"method":"ping"}),
            );
            assert_eq!(r.json()["id"], 10);
        });
    }

    #[test]
    fn a_changed_tool_list_is_announced_on_the_stream() {
        serving("stream", |addr| {
            let session = initialize(addr);
            let r = send(addr, "GET", &[AUTH, ("Mcp-Session-Id", &session)], "");
            assert_eq!(r.status, 405);
            let mut stream = open(
                addr,
                "GET",
                &[
                    AUTH,
                    ("Accept", "text/event-stream"),
                    ("Mcp-Session-Id", &session),
                ],
                "",
            );
            let (status, headers) = read_head(&mut stream);
            assert_eq!(status, 200);
            assert!(headers.iter().any(|(_, v)| v == "text/event-stream"));
            post(
                addr,
                Some(&session),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            );
            let r = post(
                addr,
                Some(&session),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"script","arguments":{
                    "save":"double","description":"Twice a number",
                    "params":{"n":{"type":"number"}},"code":"args.n * 2"}}}),
            );
            assert_eq!(r.json()["result"]["isError"], false, "{}", r.body);
            let event = read_chunk(&mut stream).expect("an event");
            assert!(
                event.contains("notifications/tools/list_changed"),
                "{event}"
            );
        });
    }
}
