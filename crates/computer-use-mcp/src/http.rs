//! Optional remote transport: MCP over HTTP (JSON-RPC POST).
//!
//! Enabled with the `http` cargo feature. Each POST body is one JSON-RPC
//! message; the response is the JSON-RPC reply. Approvals have no interactive
//! channel here, so app access follows a fixed policy (`--approval allow-all`
//! for unattended use) rather than prompting, and guarded actions
//! (`guard.mode = "ask"`) are refused. Protect the endpoint with a bearer
//! token and bind it to localhost or a trusted network.
//!
//! Requests are checked (Origin, bearer token, method, Content-Type,
//! Content-Length) on the accept thread, from headers alone; each accepted
//! request's body is then read on its own worker thread, so a slow client
//! cannot stall the others. Tool calls themselves run one at a time on the
//! calling thread, which owns the engine.

use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use computer_use::engine::{ApprovalDecision, ApprovalRequest, Approver, Engine};
use computer_use::{Backend, tools};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::catalog;
use crate::jsonrpc::{INVALID_PARAMS, METHOD_NOT_FOUND, parse_message};
use crate::server::{call_tool_caught, instructions, is_known_tool, negotiate_protocol};

/// Largest accepted request body.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Most requests whose bodies are being read or handled at once.
const MAX_WORKERS: usize = 32;

/// A request body to dispatch, and where to send the reply.
struct Job {
    body: String,
    reply: mpsc::Sender<Option<Value>>,
}

/// Serve MCP over HTTP until the process is stopped.
pub fn serve(
    mut engine: Engine<Box<dyn Backend>>,
    addr: &str,
    token: Option<String>,
    allow: bool,
) -> anyhow::Result<()> {
    let loopback = is_loopback_addr(addr);
    if token.is_none() && !loopback {
        anyhow::bail!(
            "refusing to serve MCP over HTTP on non-loopback address {addr} without a token; \
             set COMPUTER_USE_HTTP_TOKEN (or server.http_token), or bind to 127.0.0.1"
        );
    }
    let server = Server::http(addr)
        .map_err(|e| anyhow::anyhow!("cannot bind HTTP server on {addr}: {e}"))?;
    log::info!("computer-use-mcp serving MCP over HTTP on {addr}");
    if token.is_none() {
        log::warn!(
            "no HTTP token set: the endpoint is unauthenticated (any local process can use it); \
             set COMPUTER_USE_HTTP_TOKEN to require one"
        );
    }

    let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
    let token = token.map(Arc::new);
    std::thread::Builder::new()
        .name("mcp-http-accept".into())
        .spawn(move || accept_loop(server, token, jobs_tx))
        .map_err(|e| anyhow::anyhow!("cannot start the HTTP accept thread: {e}"))?;

    // The engine stays on this thread; requests are handled one at a time.
    for job in jobs_rx {
        let reply = handle(&mut engine, &job.body, allow);
        let _ = job.reply.send(reply);
    }
    anyhow::bail!("the HTTP server stopped")
}

fn accept_loop(server: Server, token: Option<Arc<String>>, jobs: mpsc::Sender<Job>) {
    let busy = Arc::new(AtomicUsize::new(0));
    loop {
        let request = match server.recv() {
            Ok(r) => r,
            Err(e) => {
                log::warn!("http recv error: {e}");
                continue;
            }
        };
        let Some(request) = precheck(request, token.as_deref().map(String::as_str)) else {
            continue;
        };
        if busy.fetch_add(1, Ordering::SeqCst) >= MAX_WORKERS {
            busy.fetch_sub(1, Ordering::SeqCst);
            respond(
                request,
                503,
                json!({"error": "too many requests in progress"}),
            );
            continue;
        }
        let jobs = jobs.clone();
        let done = busy.clone();
        let spawned = std::thread::Builder::new()
            .name("mcp-http-worker".into())
            .spawn(move || {
                work(request, &jobs);
                done.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            busy.fetch_sub(1, Ordering::SeqCst);
            log::warn!("cannot start an HTTP worker: {e}");
        }
    }
}

/// Header-only checks, before any body is read. Answers and returns `None`
/// when the request is refused.
fn precheck(request: Request, token: Option<&str>) -> Option<Request> {
    if let Some(origin) = header(&request, "Origin")
        && !is_loopback_origin(origin)
    {
        respond(request, 403, json!({"error": "forbidden origin"}));
        return None;
    }
    if let Some(expected) = token
        && !authorized(header(&request, "Authorization"), expected)
    {
        respond(request, 401, json!({"error": "unauthorized"}));
        return None;
    }
    if *request.method() != Method::Post {
        respond(request, 405, json!({"error": "use POST"}));
        return None;
    }
    if !is_json_content_type(header(&request, "Content-Type")) {
        respond(
            request,
            415,
            json!({"error": "Content-Type must be application/json"}),
        );
        return None;
    }
    if request.body_length().is_some_and(|n| n > MAX_BODY_BYTES) {
        respond(request, 413, json!({"error": "request body too large"}));
        return None;
    }
    Some(request)
}

/// Read the body, hand it to the engine thread, answer with its reply.
fn work(mut request: Request, jobs: &mpsc::Sender<Job>) {
    let mut bytes = Vec::new();
    let read = request
        .as_reader()
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() {
        respond(request, 400, json!({"error": "unreadable body"}));
        return;
    }
    if bytes.len() > MAX_BODY_BYTES {
        respond(request, 413, json!({"error": "request body too large"}));
        return;
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let (tx, rx) = mpsc::channel();
    if jobs.send(Job { body, reply: tx }).is_err() {
        respond(request, 503, json!({"error": "server is shutting down"}));
        return;
    }
    match rx.recv() {
        Ok(Some(reply)) => respond(request, 200, reply),
        // A notification or a response: accepted, nothing to return.
        Ok(None) => {
            let _ = request.respond(Response::empty(202));
        }
        Err(_) => respond(request, 500, json!({"error": "internal error"})),
    }
}

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// `Authorization: Bearer <token>` (scheme case-insensitive), compared in
/// constant time.
fn authorized(value: Option<&str>, expected: &str) -> bool {
    let Some(value) = value.map(str::trim) else {
        return false;
    };
    let Some((scheme, given)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && constant_time_eq(given.trim().as_bytes(), expected.as_bytes())
}

/// Equality whose running time depends only on the lengths.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = u8::from(a.len() != b.len());
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

fn is_json_content_type(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        v.split(';')
            .next()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
    })
}

/// Whether a browser `Origin` is a local page: http(s)://localhost,
/// 127.0.0.1 or [::1], any port.
fn is_loopback_origin(origin: &str) -> bool {
    let origin = origin.trim().to_ascii_lowercase();
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let host = if let Some(v6) = rest.strip_prefix('[') {
        match v6.split_once(']') {
            Some((h, after)) if after.is_empty() || after.starts_with(':') => {
                return h == "::1" && port_ok(after);
            }
            _ => return false,
        }
    } else {
        let (h, after) = match rest.find(':') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if !port_ok(after) {
            return false;
        }
        h
    };
    host == "localhost" || host == "127.0.0.1"
}

/// Empty, or `:<digits>`.
fn port_ok(after: &str) -> bool {
    after.is_empty()
        || after
            .strip_prefix(':')
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether every address `addr` resolves to is a loopback one.
fn is_loopback_addr(addr: &str) -> bool {
    match addr.to_socket_addrs() {
        Ok(addrs) => {
            let ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
            !ips.is_empty() && ips.iter().all(IpAddr::is_loopback)
        }
        Err(_) => false,
    }
}

/// App access follows the fixed policy; guarded actions (`guard.mode =
/// "ask"`) are refused, since nobody can be asked over this transport.
struct HttpApprover {
    allow: bool,
}

impl Approver for HttpApprover {
    fn request(&mut self, _req: &ApprovalRequest<'_>) -> ApprovalDecision {
        if self.allow {
            ApprovalDecision::Session
        } else {
            // HTTP has no prompt channel: nobody was asked.
            ApprovalDecision::Unavailable
        }
    }
    fn confirm_action(&mut self, _summary: &str) -> bool {
        false
    }
    fn interactive(&self) -> bool {
        self.allow
    }
}

fn respond(request: Request, status: u16, body: Value) {
    let data = body.to_string();
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = request.respond(
        Response::from_string(data)
            .with_status_code(status)
            .with_header(header),
    );
}

fn reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Dispatch one JSON-RPC message; `None` for a notification or a response.
fn handle<B: Backend>(engine: &mut Engine<B>, body: &str, allow: bool) -> Option<Value> {
    let msg = match parse_message(body) {
        Ok(m) => m,
        Err(resp) => return Some(serde_json::to_value(resp).expect("serialize json-rpc")),
    };
    let method = msg.method.clone()?;
    let params = msg.params.unwrap_or(Value::Null);
    if method == crate::server::STATUS_METHOD {
        let state = params
            .get("state")
            .and_then(Value::as_str)
            .map(str::parse::<computer_use::overlay::Status>);
        let result = match state {
            Some(Ok(s)) => {
                engine.set_status(s);
                Ok(())
            }
            Some(Err(e)) => Err(e),
            None => Err("`state` is required".to_string()),
        };
        let id = msg.id.clone()?;
        return Some(match result {
            Ok(()) => reply(id, json!({})),
            Err(e) => error(id, INVALID_PARAMS, &e),
        });
    }
    // Notifications carry no id and expect no response.
    let id = msg.id.clone()?;

    let mut approver = HttpApprover { allow };

    Some(match method.as_str() {
        "initialize" => reply(
            id,
            json!({
                "protocolVersion": negotiate_protocol(
                    params.get("protocolVersion").and_then(Value::as_str),
                ),
                "capabilities": catalog::capabilities(engine.store().config.skills, false),
                "serverInfo": {"name": "computer-use", "version": env!("CARGO_PKG_VERSION")},
                "instructions": instructions(),
            }),
        ),
        "ping" => reply(id, json!({})),
        "tools/list" => {
            engine.reload_if_changed();
            let list: Vec<Value> = tools::definitions_from(&engine.store().config)
                .into_iter()
                .map(|d| {
                    json!({
                        "name": d.name,
                        "title": d.title,
                        "description": d.description,
                        "inputSchema": d.input_schema,
                        "annotations": d.annotations,
                    })
                })
                .collect();
            reply(id, json!({"tools": list}))
        }
        "tools/call" => match params.get("name").and_then(Value::as_str) {
            Some(name) if !is_known_tool(name) => {
                error(id, INVALID_PARAMS, &format!("unknown tool: {name}"))
            }
            Some(name) => {
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                reply(id, call_tool_caught(engine, name, args, &mut approver))
            }
            None => error(id, INVALID_PARAMS, "tools/call requires `name`"),
        },
        "prompts/list" => reply(id, catalog::prompts_list(engine.store().config.skills)),
        "prompts/get" => match catalog::prompts_get(engine.store().config.skills, &params) {
            Ok(v) => reply(id, v),
            Err((code, msg)) => error(id, code, &msg),
        },
        "resources/list" => reply(id, catalog::resources_list(engine.store().config.skills)),
        "resources/templates/list" => reply(id, json!({"resourceTemplates": []})),
        "resources/read" => match catalog::resources_read(engine.store().config.skills, &params) {
            Ok(v) => reply(id, v),
            Err((code, msg)) => error(id, code, &msg),
        },
        other => error(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::{INVALID_REQUEST, PARSE_ERROR};
    use computer_use::config::{ApprovalMode, Config, ConfigStore};
    use computer_use::mock::MockBackend;

    fn engine() -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::AllowAll;
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(std::time::Instant::now, |_| {})
    }

    fn call(body: &str) -> Option<Value> {
        handle(&mut engine(), body, false)
    }

    #[test]
    fn json_rpc_errors() {
        assert_eq!(call("{oops").unwrap()["error"]["code"], PARSE_ERROR);
        assert_eq!(call("[]").unwrap()["error"]["code"], INVALID_REQUEST);
        let r = call(r#"{"jsonrpc":"2.0","id":2}"#).unwrap();
        assert_eq!(
            (r["error"]["code"].clone(), r["id"].clone()),
            (json!(INVALID_REQUEST), json!(2))
        );
        let r = call(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope"}}"#)
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        // Notifications and responses get no body (202).
        assert!(call(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(call(r#"{"jsonrpc":"2.0","id":"x","result":{}}"#).is_none());
    }

    #[test]
    fn negotiates_protocol_and_calls_tools() {
        let r = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        let r = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        let r =
            call(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_apps"}}"#)
                .unwrap();
        assert_eq!(r["result"]["isError"], false);
    }

    #[test]
    fn http_approver_never_confirms_guarded_actions() {
        let mut ap = HttpApprover { allow: true };
        assert!(!ap.confirm_action("press Send"));
    }

    #[test]
    fn origins() {
        for ok in [
            "http://localhost",
            "http://localhost:3000",
            "https://127.0.0.1:8787",
            "http://[::1]:9",
            "HTTP://LOCALHOST",
        ] {
            assert!(is_loopback_origin(ok), "{ok}");
        }
        for bad in [
            "null",
            "http://evil.com",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "http://localhost:80@evil.com",
            "http://[::2]",
            "file://",
            "",
        ] {
            assert!(!is_loopback_origin(bad), "{bad}");
        }
    }

    #[test]
    fn auth_and_content_type() {
        assert!(authorized(Some("Bearer s3cret"), "s3cret"));
        assert!(authorized(Some("bearer s3cret"), "s3cret"));
        assert!(!authorized(Some("Bearer s3cre"), "s3cret"));
        assert!(!authorized(Some("Basic s3cret"), "s3cret"));
        assert!(!authorized(Some("s3cret"), "s3cret"));
        assert!(!authorized(None, "s3cret"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(is_json_content_type(Some("application/json")));
        assert!(is_json_content_type(Some(
            "Application/JSON; charset=utf-8"
        )));
        assert!(!is_json_content_type(Some("text/plain")));
        assert!(!is_json_content_type(None));
    }

    #[test]
    fn loopback_bind_addresses() {
        assert!(is_loopback_addr("127.0.0.1:8787"));
        assert!(is_loopback_addr("[::1]:8787"));
        assert!(!is_loopback_addr("0.0.0.0:8787"));
        assert!(!is_loopback_addr("192.168.1.2:8787"));
    }

    #[test]
    fn refuses_public_bind_without_token() {
        let backend: Box<dyn Backend> = Box::new(MockBackend::new());
        let engine = Engine::new(backend, ConfigStore::in_memory(Config::default()));
        let err = serve(engine, "0.0.0.0:0", None, false).unwrap_err();
        assert!(err.to_string().contains("without a token"), "{err}");
    }

    /// A live server on an ephemeral port, answering with a mock engine.
    fn start(token: Option<&str>) -> std::net::SocketAddr {
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        let (tx, rx) = mpsc::channel::<Job>();
        let token = token.map(|t| Arc::new(t.to_string()));
        std::thread::spawn(move || accept_loop(server, token, tx));
        std::thread::spawn(move || {
            let mut engine = engine();
            for job in rx {
                let _ = job.reply.send(handle(&mut engine, &job.body, false));
            }
        });
        addr
    }

    fn status(addr: std::net::SocketAddr, headers: &str, body: &str) -> u16 {
        use std::io::Write;
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(
            s,
            "POST /mcp HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out.split(' ').nth(1).unwrap_or("0").parse().unwrap()
    }

    const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    const JSON: &str = "Content-Type: application/json\r\n";

    #[test]
    fn live_request_checks() {
        let addr = start(Some("tok"));
        let auth = "Authorization: Bearer tok\r\n";
        assert_eq!(status(addr, &format!("{JSON}{auth}"), PING), 200);
        assert_eq!(status(addr, JSON, PING), 401);
        assert_eq!(
            status(addr, &format!("{JSON}authorization: BEARER tok\r\n"), PING),
            200
        );
        assert_eq!(
            status(addr, &format!("Content-Type: text/plain\r\n{auth}"), PING),
            415
        );
        assert_eq!(
            status(
                addr,
                &format!("{JSON}{auth}Origin: http://evil.example\r\n"),
                PING
            ),
            403
        );
        assert_eq!(
            status(
                addr,
                &format!("{JSON}{auth}Origin: http://localhost:5173\r\n"),
                PING
            ),
            200
        );
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(status(addr, &format!("{JSON}{auth}"), note), 202);
    }

    #[test]
    fn oversize_body_is_refused_before_reading() {
        use std::io::Write;
        let addr = start(None);
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(
            s,
            "POST / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{JSON}Content-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 413"), "{out}");
    }

    #[test]
    fn stalled_client_does_not_block_others() {
        use std::io::Write;
        let addr = start(None);
        // Promise a body and never send it.
        let mut stalled = std::net::TcpStream::connect(addr).unwrap();
        write!(
            stalled,
            "POST / HTTP/1.1\r\nHost: x\r\n{JSON}Content-Length: 100\r\n\r\n{{"
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(status(addr, JSON, PING), 200);
        drop(stalled);
    }

    #[test]
    fn http_serves_skills_as_prompts_and_resources() {
        let mut e = engine();
        let ask = |e: &mut Engine<MockBackend>, body: &str| handle(e, body, true).expect("a reply");
        let init = ask(
            &mut e,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert!(init["result"]["capabilities"]["prompts"].is_object());
        let list = ask(
            &mut e,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/list"}"#,
        );
        assert!(!list["result"]["resources"].as_array().unwrap().is_empty());
        let got = ask(
            &mut e,
            r#"{"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"files"}}"#,
        );
        assert!(
            got["result"]["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .starts_with("# ")
        );
        let bad = ask(
            &mut e,
            r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"x"}}"#,
        );
        assert_eq!(bad["error"]["code"], -32002);
    }
}
