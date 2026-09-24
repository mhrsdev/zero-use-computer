//! Optional remote transport: MCP over HTTP (JSON-RPC POST).
//!
//! Enabled with the `http` cargo feature. Each POST body is one JSON-RPC
//! message; the response is the JSON-RPC reply. Approvals have no interactive
//! channel here, so app access follows a fixed policy (`--approval allow-all`
//! for unattended use) rather than prompting. Protect the endpoint with a
//! bearer token and bind it to localhost or a trusted network.

use computer_use::engine::{AllowApprover, Approver, DenyApprover, Engine};
use computer_use::{Backend, tools};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::jsonrpc::{INVALID_PARAMS, Incoming, METHOD_NOT_FOUND};
use crate::server::instructions;

const PROTOCOL_VERSION: &str = "2025-06-18";

/// Serve MCP over HTTP until the process is stopped.
pub fn serve(
    mut engine: Engine<Box<dyn Backend>>,
    addr: &str,
    token: Option<String>,
    allow: bool,
) -> anyhow::Result<()> {
    let server = Server::http(addr)
        .map_err(|e| anyhow::anyhow!("cannot bind HTTP server on {addr}: {e}"))?;
    log::info!("computer-use-mcp serving MCP over HTTP on {addr}");
    if token.is_none() {
        log::warn!("no --http-token set: the endpoint is unauthenticated");
    }

    loop {
        let mut request = match server.recv() {
            Ok(r) => r,
            Err(e) => {
                log::warn!("http recv error: {e}");
                continue;
            }
        };
        if let Some(expected) = &token
            && !authorized(&request, expected)
        {
            respond(request, 401, json!({"error": "unauthorized"}));
            continue;
        }
        if *request.method() != Method::Post {
            respond(request, 405, json!({"error": "use POST"}));
            continue;
        }
        let mut body = String::new();
        if request.as_reader().read_to_string(&mut body).is_err() {
            respond(request, 400, json!({"error": "unreadable body"}));
            continue;
        }
        match handle(&mut engine, &body, allow) {
            Some(reply) => respond(request, 200, reply),
            None => {
                // A notification: acknowledge with no content.
                let _ = request.respond(Response::empty(204));
            }
        }
    }
}

fn authorized(request: &Request, expected: &str) -> bool {
    request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && h.value.as_str() == format!("Bearer {expected}"))
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
fn handle(engine: &mut Engine<Box<dyn Backend>>, body: &str, allow: bool) -> Option<Value> {
    let msg: Incoming = match serde_json::from_str(body) {
        Ok(m) => m,
        Err(e) => {
            return Some(error(
                Value::Null,
                INVALID_PARAMS,
                &format!("parse error: {e}"),
            ));
        }
    };
    let method = msg.method.clone()?;
    let id = msg.id.clone();
    // Notifications carry no id and expect no response.
    let id = id?;
    let params = msg.params.unwrap_or(Value::Null);

    let mut allow_ap = AllowApprover;
    let mut deny_ap = DenyApprover;
    let approver: &mut dyn Approver = if allow { &mut allow_ap } else { &mut deny_ap };

    Some(match method.as_str() {
        "initialize" => reply(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
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
            Some(name) => {
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let out = engine.call_tool(name, args, approver);
                reply(id, out.to_mcp_result())
            }
            None => error(id, INVALID_PARAMS, "tools/call requires `name`"),
        },
        other => error(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
    })
}
