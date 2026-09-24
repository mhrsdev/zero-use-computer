//! MCP stdio server: line-delimited JSON-RPC, the eleven computer-use tools,
//! and per-app approvals via MCP elicitation.

use std::io::{BufRead, Write};

use computer_use::engine::{ApprovalDecision, ApprovalRequest, Approver, Engine};
use computer_use::{Backend, tools};
use serde_json::{Value, json};

use crate::jsonrpc::*;

/// Default protocol version if the client doesn't send one.
const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "computer-use";

/// What to do when policy needs approval but no elicitation-capable client is
/// available to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessApproval {
    Deny,
    Allow,
}

pub struct Server<R: BufRead, W: Write, B: Backend> {
    engine: Option<Engine<B>>,
    reader: R,
    writer: W,
    client_elicitation: bool,
    headless: HeadlessApproval,
    next_out_id: i64,
    shutdown: bool,
}

impl<R: BufRead, W: Write, B: Backend> Server<R, W, B> {
    pub fn new(engine: Engine<B>, reader: R, writer: W, headless: HeadlessApproval) -> Self {
        Self {
            engine: Some(engine),
            reader,
            writer,
            client_elicitation: false,
            headless,
            next_out_id: 1,
            shutdown: false,
        }
    }

    /// Read and dispatch messages until stdin closes.
    pub fn run(&mut self) -> std::io::Result<()> {
        while !self.shutdown {
            match self.read_message()? {
                Some(msg) => self.dispatch(msg)?,
                None => break,
            }
        }
        Ok(())
    }

    fn read_message(&mut self) -> std::io::Result<Option<Incoming>> {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                return Ok(None); // EOF
            }
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Incoming>(&line) {
                Ok(msg) => return Ok(Some(msg)),
                Err(e) => {
                    log::warn!("dropping unparseable message: {e}");
                    self.write_msg(&Response::err(
                        Value::Null,
                        PARSE_ERROR,
                        format!("parse error: {e}"),
                    ))?;
                }
            }
        }
    }

    fn write_msg(&mut self, value: &impl serde::Serialize) -> std::io::Result<()> {
        let s = serde_json::to_string(value).expect("serialize json-rpc");
        self.writer.write_all(s.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    fn dispatch(&mut self, msg: Incoming) -> std::io::Result<()> {
        if msg.is_response() {
            // No outstanding request expects a top-level response here.
            return Ok(());
        }
        let Some(method) = msg.method.clone() else {
            return Ok(());
        };
        if msg.is_notification() {
            self.handle_notification(&method, msg.params);
            return Ok(());
        }
        let id = msg.id.clone().unwrap_or(Value::Null);
        let params = msg.params.unwrap_or(Value::Null);
        let response = self.handle_request(&method, params, id.clone());
        if let Some(resp) = response {
            self.write_msg(&resp)?;
        }
        Ok(())
    }

    fn handle_notification(&mut self, method: &str, _params: Option<Value>) {
        match method {
            "notifications/initialized" | "initialized" => {}
            "notifications/cancelled" => {}
            other => log::debug!("ignoring notification {other}"),
        }
    }

    fn handle_request(&mut self, method: &str, params: Value, id: Value) -> Option<Response> {
        match method {
            "initialize" => Some(Response::ok(id, self.initialize(&params))),
            "ping" => Some(Response::ok(id, json!({}))),
            "tools/list" => Some(Response::ok(id, self.tools_list())),
            "tools/call" => Some(self.tools_call(params, id)),
            "shutdown" => {
                self.shutdown = true;
                Some(Response::ok(id, Value::Null))
            }
            // Not implemented but harmless to answer emptily.
            "resources/list" => Some(Response::ok(id, json!({"resources": []}))),
            "prompts/list" => Some(Response::ok(id, json!({"prompts": []}))),
            other => Some(Response::err(
                id,
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    fn initialize(&mut self, params: &Value) -> Value {
        self.client_elicitation = params
            .get("capabilities")
            .and_then(|c| c.get("elicitation"))
            .is_some();
        let protocol = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION)
            .to_string();
        json!({
            "protocolVersion": protocol,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
            "instructions": instructions(),
        })
    }

    fn tools_list(&self) -> Value {
        let tools: Vec<Value> = tools::definitions()
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
        json!({ "tools": tools })
    }

    fn tools_call(&mut self, params: Value, id: Value) -> Response {
        let name = match params.get("name").and_then(Value::as_str) {
            Some(n) => n.to_string(),
            None => return Response::err(id, INVALID_PARAMS, "tools/call requires `name`"),
        };
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let mut engine = self.engine.take().expect("engine present");
        let mut approver = McpApprover { server: self };
        let out = engine.call_tool(&name, args, &mut approver);
        self.engine = Some(engine);
        Response::ok(id, out.to_mcp_result())
    }

    /// Ask the client to approve controlling `app` via MCP elicitation.
    fn elicit(&mut self, app_name: &str, app_id: &str, tool: &str) -> ApprovalDecision {
        if !self.client_elicitation {
            return match self.headless {
                HeadlessApproval::Allow => ApprovalDecision::Session,
                HeadlessApproval::Deny => ApprovalDecision::Deny,
            };
        }
        let out_id = json!(format!("elicit-{}", self.next_id()));
        let req = OutgoingRequest {
            jsonrpc: JSONRPC,
            id: out_id.clone(),
            method: "elicitation/create".into(),
            params: json!({
                "message": format!(
                    "Allow computer use to control \"{app_name}\" (id: {app_id})? Requested by tool `{tool}`.",
                ),
                "requestedSchema": {
                    "type": "object",
                    "properties": {
                        "approve": {
                            "type": "boolean",
                            "title": "Allow control",
                            "description": format!("Let the agent see and control {app_name}."),
                        },
                        "remember": {
                            "type": "boolean",
                            "title": "Always allow this app",
                            "description": "Remember this choice for this app in the config.",
                            "default": false,
                        }
                    },
                    "required": ["approve"]
                }
            }),
        };
        if self.write_msg(&req).is_err() {
            return ApprovalDecision::Deny;
        }
        self.await_elicit_response(&out_id)
    }

    fn await_elicit_response(&mut self, want: &Value) -> ApprovalDecision {
        loop {
            let msg = match self.read_message() {
                Ok(Some(m)) => m,
                Ok(None) => {
                    self.shutdown = true;
                    return ApprovalDecision::Deny;
                }
                Err(_) => return ApprovalDecision::Deny,
            };
            if msg.is_response() {
                if msg.id.as_ref() == Some(want) {
                    return decode_elicit(msg.result, msg.error);
                }
                continue; // stray response
            }
            // A request arrived while we're waiting. Keep ping alive; refuse
            // anything that would re-enter the engine.
            if let Some(method) = msg.method.clone() {
                if msg.id.is_none() {
                    self.handle_notification(&method, msg.params);
                    continue;
                }
                let id = msg.id.clone().unwrap_or(Value::Null);
                let resp = match method.as_str() {
                    "ping" => Response::ok(id, json!({})),
                    _ => Response::err(
                        id,
                        INTERNAL_ERROR,
                        "server is waiting for the user's approval decision",
                    ),
                };
                let _ = self.write_msg(&resp);
            }
        }
    }

    /// Confirm a guarded on-screen action (the guard), via elicitation.
    fn confirm(&mut self, summary: &str) -> bool {
        if !self.client_elicitation {
            return self.headless == HeadlessApproval::Allow;
        }
        let out_id = json!(format!("confirm-{}", self.next_id()));
        let req = OutgoingRequest {
            jsonrpc: JSONRPC,
            id: out_id.clone(),
            method: "elicitation/create".into(),
            params: json!({
                "message": format!("Confirm this action? The agent is about to {summary}."),
                "requestedSchema": {
                    "type": "object",
                    "properties": {
                        "confirm": {
                            "type": "boolean",
                            "title": "Proceed",
                            "description": "Allow this consequential action.",
                        }
                    },
                    "required": ["confirm"]
                }
            }),
        };
        if self.write_msg(&req).is_err() {
            return false;
        }
        !matches!(self.await_elicit_response(&out_id), ApprovalDecision::Deny)
    }

    fn next_id(&mut self) -> i64 {
        let id = self.next_out_id;
        self.next_out_id += 1;
        id
    }
}

fn decode_elicit(result: Option<Value>, error: Option<Value>) -> ApprovalDecision {
    if error.is_some() {
        return ApprovalDecision::Deny;
    }
    let Some(result) = result else {
        return ApprovalDecision::Deny;
    };
    let action = result
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("cancel");
    if action != "accept" {
        return ApprovalDecision::Deny;
    }
    let content = result.get("content").cloned().unwrap_or(json!({}));
    // `approve` for app access, `confirm` for the action guard.
    let approve = content
        .get("approve")
        .or_else(|| content.get("confirm"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !approve {
        return ApprovalDecision::Deny;
    }
    let remember = content
        .get("remember")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if remember {
        ApprovalDecision::Always
    } else {
        ApprovalDecision::Session
    }
}

/// Bridges the engine's approval callback to MCP elicitation.
struct McpApprover<'a, R: BufRead, W: Write, B: Backend> {
    server: &'a mut Server<R, W, B>,
}

impl<R: BufRead, W: Write, B: Backend> Approver for McpApprover<'_, R, W, B> {
    fn request(&mut self, req: &ApprovalRequest<'_>) -> ApprovalDecision {
        self.server.elicit(&req.app.name, &req.app.id, req.tool)
    }

    fn confirm_action(&mut self, summary: &str) -> bool {
        self.server.confirm(summary)
    }
}

fn instructions() -> String {
    "Control desktop apps through their accessibility tree plus screenshots. \
     On every turn, call get_app_state(app) first: it returns the app's numbered \
     accessibility tree and a screenshot. Act on elements by their element_index \
     (click, set_value, perform_secondary_action, select_text, scroll, drag, \
     press_key, type_text); indices are only valid until the next get_app_state, \
     which afterwards returns a diff. Prefer element_index over x/y coordinates. \
     Terminals, credential/security prompts and the agent's own app cannot be \
     controlled. The first use of each app may prompt the user for approval."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer_use::config::{ApprovalMode, Config, ConfigStore};
    use computer_use::engine::Engine;
    use computer_use::mock::MockBackend;
    use std::io::Cursor;

    fn engine(mode: ApprovalMode) -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut cfg = Config::default();
        cfg.approvals.mode = mode;
        Engine::new(backend, ConfigStore::in_memory(cfg)).with_time(std::time::Instant::now, |_| {})
    }

    /// Run a scripted client conversation, return the lines the server wrote.
    fn converse(mode: ApprovalMode, headless: HeadlessApproval, input: &str) -> Vec<Value> {
        let reader = Cursor::new(input.to_string());
        let mut out: Vec<u8> = Vec::new();
        {
            let mut server = Server::new(engine(mode), reader, &mut out, headless);
            server.run().unwrap();
        }
        String::from_utf8(out)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn line(method: &str, id: i64, params: Value) -> String {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string() + "\n"
    }

    #[test]
    fn initialize_list_and_call() {
        let input = format!(
            "{}{}{}",
            line(
                "initialize",
                1,
                json!({"protocolVersion":"2025-06-18","capabilities":{}})
            ),
            line("tools/list", 2, json!({})),
            line("tools/call", 3, json!({"name":"list_apps","arguments":{}})),
        );
        let out = converse(ApprovalMode::AllowAll, HeadlessApproval::Deny, &input);
        assert_eq!(out.len(), 3);
        // initialize
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "computer-use");
        // tools/list has 11 tools
        assert_eq!(out[1]["result"]["tools"].as_array().unwrap().len(), 17);
        // list_apps ran
        let text = out[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("TextEdit"));
        assert_eq!(out[2]["result"]["isError"], false);
    }

    #[test]
    fn get_app_state_returns_image_content() {
        let input = format!(
            "{}{}",
            line("initialize", 1, json!({"capabilities":{}})),
            line(
                "tools/call",
                2,
                json!({"name":"get_app_state","arguments":{"app":"TextEdit"}})
            ),
        );
        let out = converse(ApprovalMode::AllowAll, HeadlessApproval::Deny, &input);
        let content = out[1]["result"]["content"].as_array().unwrap();
        assert!(content.iter().any(|c| c["type"] == "text"));
        assert!(
            content
                .iter()
                .any(|c| c["type"] == "image" && c["mimeType"] == "image/png")
        );
    }

    #[test]
    fn unknown_method_is_error() {
        let input = line("frobnicate", 1, json!({}));
        let out = converse(ApprovalMode::AllowAll, HeadlessApproval::Deny, &input);
        assert_eq!(out[0]["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn elicitation_approves_and_controls() {
        // Client advertises elicitation; when the server asks, it accepts.
        // Sequence: initialize, then tools/call get_app_state, then the
        // elicitation response (id must match "elicit-1").
        let input = format!(
            "{}{}{}",
            line("initialize", 1, json!({"capabilities":{"elicitation":{}}})),
            line("tools/call", 2, json!({"name":"get_app_state","arguments":{"app":"TextEdit"}})),
            json!({"jsonrpc":"2.0","id":"elicit-1","result":{"action":"accept","content":{"approve":true}}}).to_string() + "\n",
        );
        let out = converse(ApprovalMode::Prompt, HeadlessApproval::Deny, &input);
        // The server should have emitted an elicitation/create request...
        assert!(out.iter().any(|m| m["method"] == "elicitation/create"));
        // ...and then the tool result for id 2, not an error.
        let call = out.iter().find(|m| m["id"] == 2).unwrap();
        assert_eq!(call["result"]["isError"], false);
    }

    #[test]
    fn elicitation_denied_blocks_call() {
        let input = format!(
            "{}{}{}",
            line("initialize", 1, json!({"capabilities":{"elicitation":{}}})),
            line(
                "tools/call",
                2,
                json!({"name":"get_app_state","arguments":{"app":"TextEdit"}})
            ),
            json!({"jsonrpc":"2.0","id":"elicit-1","result":{"action":"decline"}}).to_string()
                + "\n",
        );
        let out = converse(ApprovalMode::Prompt, HeadlessApproval::Deny, &input);
        let call = out.iter().find(|m| m["id"] == 2).unwrap();
        assert_eq!(call["result"]["isError"], true);
        assert!(
            call["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("denied")
        );
    }

    #[test]
    fn headless_deny_without_elicitation() {
        let input = format!(
            "{}{}",
            line("initialize", 1, json!({"capabilities":{}})),
            line(
                "tools/call",
                2,
                json!({"name":"get_app_state","arguments":{"app":"TextEdit"}})
            ),
        );
        let out = converse(ApprovalMode::Prompt, HeadlessApproval::Deny, &input);
        assert_eq!(out[1]["result"]["isError"], true);

        let out = converse(ApprovalMode::Prompt, HeadlessApproval::Allow, &input);
        assert_eq!(out[1]["result"]["isError"], false);
    }
}
