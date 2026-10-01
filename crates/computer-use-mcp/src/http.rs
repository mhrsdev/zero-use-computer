//! Optional remote transport: MCP over HTTP (JSON-RPC POST).
//!
//! Enabled with the `http` cargo feature. Each POST body is one JSON-RPC
//! message; the response is the JSON-RPC reply. Whoever can reach the
//! endpoint can control the desktop, so a bearer token is required, requests
//! from web pages on other origins are refused, and it should be bound to
//! localhost or a trusted network.

use std::io::Read as _;
use std::net::SocketAddr;

use computer_use::Backend;
use computer_use::engine::Engine;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::jsonrpc::{INVALID_PARAMS, Incoming, METHOD_NOT_FOUND, parse_message};
use crate::server::{instructions, negotiate_protocol, unknown_tool};

/// Largest request body accepted (a JSON-RPC message is far smaller).
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// Serve MCP over HTTP until the process is stopped. `token` is required.
pub fn serve(mut engine: Engine<Box<dyn Backend>>, addr: &str, token: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!token.is_empty(), "serving over HTTP needs a bearer token");
    let server = Server::http(addr)
        .map_err(|e| anyhow::anyhow!("cannot bind HTTP server on {addr}: {e}"))?;
    log::info!("computer-use-mcp serving MCP over HTTP on {addr}");
    if addr
        .parse::<SocketAddr>()
        .is_ok_and(|a| !a.ip().is_loopback())
    {
        log::warn!(
            "{addr} is reachable from other machines: anyone there with the token can control this desktop"
        );
    }

    loop {
        let mut request = match server.recv() {
            Ok(r) => r,
            Err(e) => {
                log::warn!("http recv error: {e}");
                continue;
            }
        };
        if !authorized(&request, token) {
            respond(request, 401, json!({"error": "unauthorized"}));
            continue;
        }
        if !local_origin(&request) {
            respond(
                request,
                403,
                json!({"error": "requests from other origins are not allowed"}),
            );
            continue;
        }
        if *request.method() != Method::Post {
            respond(request, 405, json!({"error": "use POST"}));
            continue;
        }
        if !header(&request, "Content-Type")
            .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"))
        {
            respond(
                request,
                415,
                json!({"error": "Content-Type must be application/json"}),
            );
            continue;
        }
        if request.body_length().is_some_and(|n| n as u64 > MAX_BODY) {
            respond(request, 413, json!({"error": "request body too large"}));
            continue;
        }
        let mut body = String::new();
        let read = request
            .as_reader()
            .take(MAX_BODY + 1)
            .read_to_string(&mut body);
        if read.is_err() || body.len() as u64 > MAX_BODY {
            respond(
                request,
                400,
                json!({"error": "unreadable or too large body"}),
            );
            continue;
        }
        match handle(&mut engine, &body) {
            Some(reply) => respond(request, 200, reply),
            None => {
                // A notification: accepted, nothing to return.
                let _ = request.respond(Response::empty(202));
            }
        }
    }
}

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// The bearer token matches (compared in constant time).
fn authorized(request: &Request, expected: &str) -> bool {
    let Some(given) = header(request, "Authorization").and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// No `Origin` (not a browser), or a page on this machine. Browsers send
/// `Origin` with cross-site requests; this stops a web page from driving the
/// desktop (DNS rebinding included).
fn local_origin(request: &Request) -> bool {
    let Some(origin) = header(request, "Origin") else {
        return true;
    };
    let host = origin
        .split("://")
        .nth(1)
        .unwrap_or("")
        .trim_end_matches('/');
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
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

/// Dispatch one JSON-RPC message; `None` for a notification.
fn handle(engine: &mut Engine<Box<dyn Backend>>, body: &str) -> Option<Value> {
    let msg: Incoming = match parse_message(body) {
        Ok(m) => m,
        Err(reply) => return Some(serde_json::to_value(*reply).unwrap_or(Value::Null)),
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

    Some(match method.as_str() {
        "initialize" => reply(
            id,
            json!({
                "protocolVersion": negotiate_protocol(params.get("protocolVersion").and_then(Value::as_str)),
                "capabilities": crate::catalog::capabilities(false),
                "serverInfo": {"name": "computer-use", "title": "computer-use (mhrsdev)", "version": env!("CARGO_PKG_VERSION")},
                "instructions": instructions(),
            }),
        ),
        "ping" => reply(id, json!({})),
        "tools/list" => {
            engine.reload_if_changed();
            let list: Vec<Value> = engine
                .tool_definitions()
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
            Some(name) if unknown_tool(engine, name).is_some() => {
                error(id, INVALID_PARAMS, &format!("unknown tool: {name}"))
            }
            Some(name) => {
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let out = engine.call_tool(name, args);
                reply(id, out.to_mcp_result())
            }
            None => error(id, INVALID_PARAMS, "tools/call requires `name`"),
        },
        other => match crate::catalog::handle(other, &params) {
            Some(Ok(result)) => reply(id, result),
            Some(Err((code, message))) => error(id, code, &message),
            None => error(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
