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
    /// Tool set last announced to the client, to detect settings changes.
    tools_sig: Option<String>,
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
            tools_sig: None,
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
        // A hot-reloaded config can change which tools exist; tell the client.
        if let Some(prev) = self.tools_sig.clone() {
            let now = self.tools_signature();
            if now != prev {
                self.tools_sig = Some(now);
                self.write_msg(&json!({
                    "jsonrpc": JSONRPC,
                    "method": "notifications/tools/list_changed"
                }))?;
            }
        }
        Ok(())
    }

    fn handle_notification(&mut self, method: &str, params: Option<Value>) {
        match method {
            "notifications/initialized" | "initialized" => {}
            "notifications/cancelled" => {}
            STATUS_METHOD | "notifications/computer_use/status" => {
                let _ = self.set_status(params.as_ref());
            }
            other => log::debug!("ignoring notification {other}"),
        }
    }

    /// `computer_use/status {"state": "thinking" | "working" | "done" |
    /// "error" | "hidden"}` — lets a host agent drive the on-screen overlay.
    fn set_status(&mut self, params: Option<&Value>) -> Result<(), String> {
        let state = params
            .and_then(|p| p.get("state"))
            .and_then(Value::as_str)
            .ok_or("`state` is required")?
            .parse::<computer_use::overlay::Status>()?;
        if let Some(engine) = self.engine.as_mut() {
            engine.set_status(state);
        }
        Ok(())
    }

    fn handle_request(&mut self, method: &str, params: Value, id: Value) -> Option<Response> {
        match method {
            "initialize" => Some(Response::ok(id, self.initialize(&params))),
            "ping" => Some(Response::ok(id, json!({}))),
            "tools/list" => Some(Response::ok(id, self.tools_list())),
            "tools/call" => Some(self.tools_call(params, id)),
            STATUS_METHOD => Some(match self.set_status(Some(&params)) {
                Ok(()) => Response::ok(id, json!({})),
                Err(e) => Response::err(id, INVALID_PARAMS, e),
            }),
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
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
            "instructions": instructions(),
        })
    }

    fn tools_signature(&self) -> String {
        let engine = self.engine.as_ref().expect("engine present");
        tools::definitions_from(&engine.store().config)
            .iter()
            .map(|d| format!("{}:{}", d.name, d.description))
            .collect::<Vec<_>>()
            .join("|")
    }

    fn tools_list(&mut self) -> Value {
        self.engine
            .as_mut()
            .expect("engine present")
            .reload_if_changed();
        self.tools_sig = Some(self.tools_signature());
        let engine = self.engine.as_mut().expect("engine present");
        let tools: Vec<Value> = tools::definitions_from(&engine.store().config)
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

    fn interactive(&self) -> bool {
        // The client can ask, or the headless policy is to allow anyway.
        self.server.client_elicitation || self.server.headless == HeadlessApproval::Allow
    }
}

/// Host → server: what the agent is doing, for the on-screen overlay.
pub(crate) const STATUS_METHOD: &str = "computer_use/status";

pub(crate) fn instructions() -> String {
    "Control desktop apps through their accessibility tree plus screenshots. \
     On every turn, call get_app_state(app) first: it returns the app's numbered \
     accessibility tree and a screenshot. Act on elements by their element_index \
     (click, set_value, perform_secondary_action, select_text, scroll, drag, \
     press_key, type_text); indices are only valid until the next get_app_state, \
     which afterwards returns a diff. Prefer element_index over x/y coordinates. \
     Use find_element and wait_for to target elements without reading the whole \
     tree, batch to run several actions at once, screenshot for a full/region/\
     window image, and get_clipboard/set_clipboard for text. list_folder/read_file \
     read files without a file manager, and skill() lists built-in how-to \
     playbooks for this OS (files, browser, settings, dialogs…): read the \
     matching one before an unfamiliar task. Terminals, \
     credential and OS-security prompts, and the agent's own app are blocked by \
     default (the user can allow them in settings); the first use of each app may \
     prompt for approval, and consequential actions may ask for confirmation."
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
        // tools/list has every tool the default settings expose
        let all = computer_use::tools::definitions_from(&computer_use::Config::default()).len();
        assert_eq!(out[1]["result"]["tools"].as_array().unwrap().len(), all);
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
    fn settings_change_is_hot_reloaded_and_announced() {
        use std::time::{Duration, SystemTime};
        let dir = std::env::temp_dir().join(format!("cu-hot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[approvals]\nmode = \"allow_all\"\n").unwrap();

        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let store = ConfigStore::load(Some(&path)).unwrap();
        let engine = Engine::new(backend, store).with_time(std::time::Instant::now, |_| {});

        // A reader that rewrites the config between the two requests.
        struct Script {
            lines: Vec<String>,
            path: std::path::PathBuf,
            step: usize,
        }
        impl std::io::Read for Script {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                unreachable!()
            }
        }
        impl std::io::BufRead for Script {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                unreachable!()
            }
            fn consume(&mut self, _n: usize) {}
            fn read_line(&mut self, buf: &mut String) -> std::io::Result<usize> {
                if self.step == 2 {
                    // Disable a tool and bump the mtime so the change is seen.
                    std::fs::write(
                        &self.path,
                        "[approvals]\nmode = \"allow_all\"\n[tools]\ndisabled = [\"drag\"]\n",
                    )?;
                    let f = std::fs::File::options().write(true).open(&self.path)?;
                    f.set_modified(SystemTime::now() + Duration::from_secs(5))?;
                }
                let Some(line) = self.lines.get(self.step) else {
                    return Ok(0);
                };
                self.step += 1;
                buf.push_str(line);
                Ok(line.len())
            }
        }
        let reader = Script {
            lines: vec![
                line("initialize", 1, json!({"capabilities":{}})),
                line("tools/list", 2, json!({})),
                line("tools/call", 3, json!({"name":"list_apps","arguments":{}})),
                line("tools/list", 4, json!({})),
            ],
            path: path.clone(),
            step: 0,
        };
        let mut out: Vec<u8> = Vec::new();
        {
            let mut server = Server::new(engine, reader, &mut out, HeadlessApproval::Deny);
            server.run().unwrap();
        }
        let msgs: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let first = msgs.iter().find(|m| m["id"] == 2).unwrap();
        let all = computer_use::tools::definitions_from(&computer_use::Config::default()).len();
        assert_eq!(first["result"]["tools"].as_array().unwrap().len(), all);
        assert!(
            msgs.iter()
                .any(|m| m["method"] == "notifications/tools/list_changed"),
            "{msgs:#?}"
        );
        let second = msgs.iter().find(|m| m["id"] == 4).unwrap();
        let names: Vec<&str> = second["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(!names.contains(&"drag"));
        std::fs::remove_dir_all(&dir).ok();
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
