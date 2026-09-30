//! MCP stdio server: line-delimited JSON-RPC, the computer-use tools (see
//! `computer_use::tools::definitions`), and per-app approvals via MCP
//! elicitation.

use std::io::{BufRead, Write};
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use computer_use::engine::{ApprovalDecision, ApprovalRequest, Approver, Engine};
use computer_use::tools::ToolOutput;
use computer_use::{Backend, tools};
use serde_json::{Value, json};

use crate::catalog;
use crate::jsonrpc::*;

/// MCP protocol versions this server speaks, newest first.
pub(crate) const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const SERVER_NAME: &str = "computer-use";

/// The longest message line accepted on stdio (bytes, newline included).
pub(crate) const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// How long to wait for an approval answer when the engine has no setting.
const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// The protocol version to answer `initialize` with: the client's, when this
/// server supports it, otherwise the latest supported one.
pub(crate) fn negotiate_protocol(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| SUPPORTED_PROTOCOL_VERSIONS.iter().find(|v| **v == r))
        .copied()
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0])
}

/// Whether `name` is one of the tools this build knows (enabled or not).
pub(crate) fn is_known_tool(name: &str) -> bool {
    tools::definitions().iter().any(|d| d.name == name)
}

/// Run a tool, turning a panic inside it into an `isError` result instead of
/// taking the whole server down.
pub(crate) fn call_tool_caught<B: Backend>(
    engine: &mut Engine<B>,
    name: &str,
    args: Value,
    approver: &mut dyn Approver,
) -> Value {
    catch_tool(name, || engine.call_tool(name, args, approver)).to_mcp_result()
}

fn catch_tool(name: &str, f: impl FnOnce() -> ToolOutput) -> ToolOutput {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(out) => out,
        Err(panic) => {
            let why = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            log::error!("tool `{name}` panicked: {why}");
            ToolOutput {
                text: format!("internal error: the tool `{name}` failed unexpectedly ({why})"),
                image: None,
                is_error: true,
            }
        }
    }
}

/// What to do when policy needs approval but no elicitation-capable client is
/// available to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessApproval {
    Deny,
    Allow,
}

/// One line read from the client.
pub(crate) enum RawLine {
    Eof,
    Line(Vec<u8>),
    /// Longer than the cap; the rest of the line was discarded.
    TooLong,
}

/// Read one `\n`-terminated line of at most `max` bytes. An oversize line is
/// consumed up to its newline without being kept.
pub(crate) fn read_capped_line(reader: &mut impl BufRead, max: usize) -> std::io::Result<RawLine> {
    let mut buf = Vec::new();
    let mut too_long = false;
    let mut any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            if !any {
                return Ok(RawLine::Eof);
            }
            break;
        }
        any = true;
        let (len, found) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if !too_long {
            if buf.len() + len > max {
                too_long = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(&available[..len]);
            }
        }
        reader.consume(len);
        if found {
            break;
        }
    }
    Ok(if too_long {
        RawLine::TooLong
    } else {
        RawLine::Line(buf)
    })
}

/// Read lines from `reader` on a background thread, so the server can wait
/// for them with a timeout.
pub(crate) fn spawn_line_reader<R: BufRead + Send + 'static>(
    mut reader: R,
    max: usize,
) -> Receiver<std::io::Result<RawLine>> {
    let (tx, rx): (SyncSender<_>, _) = std::sync::mpsc::sync_channel(4);
    std::thread::Builder::new()
        .name("mcp-stdin".into())
        .spawn(move || {
            loop {
                let line = read_capped_line(&mut reader, max);
                let stop = matches!(line, Ok(RawLine::Eof) | Err(_));
                if tx.send(line).is_err() || stop {
                    break;
                }
            }
        })
        .expect("spawn stdin reader");
    rx
}

/// The next thing from the client.
enum Next {
    Msg(Incoming),
    Eof,
    TimedOut,
}

pub struct Server<R: BufRead, W: Write, B: Backend> {
    engine: Option<Engine<B>>,
    reader: R,
    /// Lines read on a background thread (see [`Server::stdio`]); when set,
    /// `reader` is unused and approval waits can time out.
    lines: Option<Receiver<std::io::Result<RawLine>>>,
    writer: W,
    max_line: usize,
    client_elicitation: bool,
    headless: HeadlessApproval,
    next_out_id: i64,
    shutdown: bool,
    /// Tool set last announced to the client, to detect settings changes.
    tools_sig: Option<String>,
    /// The `tools/call` being run, and whether the client cancelled it.
    current_call: Option<Value>,
    cancelled: bool,
    approval_timeout: Duration,
}

impl<W: Write, B: Backend> Server<std::io::Empty, W, B> {
    /// A server reading stdin on a background thread, so that waiting for an
    /// approval answer can time out even when the client goes silent.
    pub fn stdio(engine: Engine<B>, writer: W, headless: HeadlessApproval) -> Self {
        let rx = spawn_line_reader(std::io::BufReader::new(std::io::stdin()), MAX_LINE_BYTES);
        Self::from_lines(engine, rx, writer, headless)
    }

    pub(crate) fn from_lines(
        engine: Engine<B>,
        lines: Receiver<std::io::Result<RawLine>>,
        writer: W,
        headless: HeadlessApproval,
    ) -> Self {
        let mut s = Server::new(engine, std::io::empty(), writer, headless);
        s.lines = Some(lines);
        s
    }
}

impl<R: BufRead, W: Write, B: Backend> Server<R, W, B> {
    pub fn new(engine: Engine<B>, reader: R, writer: W, headless: HeadlessApproval) -> Self {
        Self {
            engine: Some(engine),
            reader,
            lines: None,
            writer,
            max_line: MAX_LINE_BYTES,
            client_elicitation: false,
            headless,
            next_out_id: 1,
            shutdown: false,
            tools_sig: None,
            current_call: None,
            cancelled: false,
            approval_timeout: DEFAULT_APPROVAL_TIMEOUT,
        }
    }

    /// Read and dispatch messages until stdin closes.
    pub fn run(&mut self) -> std::io::Result<()> {
        while !self.shutdown {
            match self.read_message(None)? {
                Next::Msg(msg) => self.dispatch(msg)?,
                Next::Eof => break,
                Next::TimedOut => unreachable!("no deadline"),
            }
        }
        Ok(())
    }

    fn read_raw(&mut self, deadline: Option<Instant>) -> std::io::Result<Option<RawLine>> {
        let Some(rx) = &self.lines else {
            return read_capped_line(&mut self.reader, self.max_line).map(Some);
        };
        let got = match deadline {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
        };
        match got {
            Ok(line) => line.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Ok(Some(RawLine::Eof)),
        }
    }

    /// The next valid message. Malformed ones are answered with a JSON-RPC
    /// error and skipped. With a `deadline`, gives up at that time (only
    /// possible when lines come from a background reader).
    fn read_message(&mut self, deadline: Option<Instant>) -> std::io::Result<Next> {
        loop {
            let bytes = match self.read_raw(deadline)? {
                None => return Ok(Next::TimedOut),
                Some(RawLine::Eof) => return Ok(Next::Eof),
                Some(RawLine::TooLong) => {
                    log::warn!("dropping a message longer than {} bytes", self.max_line);
                    self.write_msg(&Response::err(
                        Value::Null,
                        INVALID_REQUEST,
                        format!("message too long (limit {} bytes)", self.max_line),
                    ))?;
                    continue;
                }
                Some(RawLine::Line(b)) => b,
            };
            let line = match String::from_utf8(bytes) {
                Ok(l) => l,
                Err(e) => {
                    log::warn!("dropping a message that is not UTF-8: {e}");
                    self.write_msg(&Response::err(
                        Value::Null,
                        PARSE_ERROR,
                        "parse error: message is not valid UTF-8",
                    ))?;
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            match parse_message(&line) {
                Ok(msg) => return Ok(Next::Msg(msg)),
                Err(resp) => {
                    log::warn!("rejecting message: {:?}", resp.error);
                    self.write_msg(&resp)?;
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
            "notifications/cancelled" => {
                let target = params.as_ref().and_then(|p| p.get("requestId"));
                if target.is_some() && target == self.current_call.as_ref() {
                    log::info!("the client cancelled the running tools/call");
                    self.cancelled = true;
                }
            }
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
            "tools/call" => self.tools_call(params, id),
            STATUS_METHOD => Some(match self.set_status(Some(&params)) {
                Ok(()) => Response::ok(id, json!({})),
                Err(e) => Response::err(id, INVALID_PARAMS, e),
            }),
            "shutdown" => {
                self.shutdown = true;
                Some(Response::ok(id, Value::Null))
            }
            // The built-in skills, as prompts and resources.
            "prompts/list" => Some(Response::ok(id, catalog::prompts_list(self.skills_on()))),
            "prompts/get" => Some(match catalog::prompts_get(self.skills_on(), &params) {
                Ok(v) => Response::ok(id, v),
                Err((code, msg)) => Response::err(id, code, msg),
            }),
            "resources/list" => Some(Response::ok(id, catalog::resources_list(self.skills_on()))),
            "resources/templates/list" => Some(Response::ok(id, json!({"resourceTemplates": []}))),
            "resources/read" => Some(match catalog::resources_read(self.skills_on(), &params) {
                Ok(v) => Response::ok(id, v),
                Err((code, msg)) => Response::err(id, code, msg),
            }),
            other => Some(Response::err(
                id,
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        }
    }

    /// Whether the built-in skills are offered (`skills` in the settings).
    fn skills_on(&self) -> bool {
        self.engine.as_ref().is_none_or(|e| e.store().config.skills)
    }

    fn initialize(&mut self, params: &Value) -> Value {
        self.client_elicitation = params
            .get("capabilities")
            .and_then(|c| c.get("elicitation"))
            .is_some();
        let protocol = negotiate_protocol(params.get("protocolVersion").and_then(Value::as_str));
        json!({
            "protocolVersion": protocol,
            "capabilities": catalog::capabilities(self.skills_on(), true),
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

    /// Run a tool. `None` when the client cancelled the call meanwhile (a
    /// cancelled request gets no response).
    fn tools_call(&mut self, params: Value, id: Value) -> Option<Response> {
        let name = match params.get("name").and_then(Value::as_str) {
            Some(n) => n.to_string(),
            None => {
                return Some(Response::err(
                    id,
                    INVALID_PARAMS,
                    "tools/call requires `name`",
                ));
            }
        };
        if !is_known_tool(&name) {
            return Some(Response::err(
                id,
                INVALID_PARAMS,
                format!("unknown tool: {name}"),
            ));
        }
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let mut engine = self.engine.take().expect("engine present");
        let secs = engine.store().config.overlay.confirm_timeout_secs;
        self.approval_timeout = if secs == 0 {
            DEFAULT_APPROVAL_TIMEOUT
        } else {
            Duration::from_secs(secs)
        };
        self.current_call = Some(id.clone());
        self.cancelled = false;
        let mut approver = McpApprover { server: self };
        let result = call_tool_caught(&mut engine, &name, args, &mut approver);
        self.engine = Some(engine);
        self.current_call = None;
        if std::mem::take(&mut self.cancelled) {
            return None;
        }
        Some(Response::ok(id, result))
    }

    /// Ask the client to approve controlling `app` via MCP elicitation.
    fn elicit(&mut self, app_name: &str, app_id: &str, tool: &str) -> ApprovalDecision {
        if !self.client_elicitation {
            return match self.headless {
                HeadlessApproval::Allow => ApprovalDecision::Session,
                HeadlessApproval::Deny => ApprovalDecision::Deny,
            };
        }
        if self.cancelled {
            return ApprovalDecision::Deny;
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

    /// Wait for the answer to elicitation `want`. Denies when the client
    /// cancels the tool call, closes the stream, or does not answer within
    /// `approval_timeout` (the timeout needs the background reader of
    /// [`Server::stdio`]; a plain blocking reader can only notice it when the
    /// next message arrives).
    fn await_elicit_response(&mut self, want: &Value) -> ApprovalDecision {
        let deadline = Instant::now() + self.approval_timeout;
        loop {
            if self.cancelled {
                return ApprovalDecision::Deny;
            }
            if Instant::now() >= deadline {
                log::warn!("no approval answer in time; denying");
                return ApprovalDecision::Deny;
            }
            let msg = match self.read_message(Some(deadline)) {
                Ok(Next::Msg(m)) => m,
                Ok(Next::TimedOut) => continue,
                Ok(Next::Eof) => {
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

    /// Confirm a guarded on-screen action (the guard, `guard.mode = "ask"`),
    /// via elicitation. Without an elicitation-capable client this refuses:
    /// `headless_approve = allow` covers app access only, never guarded
    /// actions.
    fn confirm(&mut self, summary: &str) -> bool {
        if !self.client_elicitation || self.cancelled {
            return false;
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
            pos: usize,
            rewritten: bool,
        }
        impl std::io::Read for Script {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                unreachable!()
            }
        }
        impl std::io::BufRead for Script {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                if self.step == 2 && !self.rewritten {
                    self.rewritten = true;
                    // Disable a tool and bump the mtime so the change is seen.
                    std::fs::write(
                        &self.path,
                        "[approvals]\nmode = \"allow_all\"\n[tools]\ndisabled = [\"drag\"]\n",
                    )?;
                    let f = std::fs::File::options().write(true).open(&self.path)?;
                    f.set_modified(SystemTime::now() + Duration::from_secs(5))?;
                }
                Ok(self
                    .lines
                    .get(self.step)
                    .map_or(&[][..], |l| &l.as_bytes()[self.pos..]))
            }
            fn consume(&mut self, n: usize) {
                self.pos += n;
                if self
                    .lines
                    .get(self.step)
                    .is_some_and(|l| self.pos >= l.len())
                {
                    self.step += 1;
                    self.pos = 0;
                }
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
            pos: 0,
            rewritten: false,
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

    /// Like `converse`, but with raw bytes.
    fn converse_bytes(input: &[u8], max_line: usize) -> Vec<Value> {
        let mut out: Vec<u8> = Vec::new();
        {
            let mut server = Server::new(
                engine(ApprovalMode::AllowAll),
                Cursor::new(input.to_vec()),
                &mut out,
                HeadlessApproval::Deny,
            );
            server.max_line = max_line;
            server.run().unwrap();
        }
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn invalid_utf8_is_a_parse_error_not_fatal() {
        let mut input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\xff\"}\n".to_vec();
        input.extend_from_slice(line("ping", 2, json!({})).as_bytes());
        let out = converse_bytes(&input, MAX_LINE_BYTES);
        assert_eq!(out[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(out[0]["id"], Value::Null);
        assert_eq!(out[1]["id"], 2);
        assert!(out[1]["result"].is_object());
    }

    #[test]
    fn oversize_line_is_discarded() {
        let big = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":{{\"x\":\"{}\"}}}}\n",
            "a".repeat(500)
        );
        let input = format!("{big}{}", line("ping", 2, json!({})));
        let out = converse_bytes(input.as_bytes(), 200);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0]["error"]["code"], INVALID_REQUEST);
        assert_eq!(out[1]["id"], 2);
    }

    #[test]
    fn invalid_requests_get_errors() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":5}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2}\n",
            "[{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"ping\"}]\n",
            "{\"id\":5,\"method\":\"ping\"}\n",
            "{not json\n",
        );
        let out = converse(ApprovalMode::AllowAll, HeadlessApproval::Deny, input);
        let got: Vec<(Value, Value)> = out
            .iter()
            .map(|m| (m["id"].clone(), m["error"]["code"].clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (json!(3), json!(INVALID_REQUEST)),
                (json!(2), json!(INVALID_REQUEST)),
                (Value::Null, json!(INVALID_REQUEST)),
                (json!(5), json!(INVALID_REQUEST)),
                (Value::Null, json!(PARSE_ERROR)),
            ]
        );
    }

    #[test]
    fn protocol_version_is_negotiated() {
        for (asked, answer) in [
            (json!("2024-11-05"), "2024-11-05"),
            (json!("2025-03-26"), "2025-03-26"),
            (json!("2025-06-18"), "2025-06-18"),
            (json!("2099-01-01"), "2025-06-18"),
            (Value::Null, "2025-06-18"),
        ] {
            let input = line("initialize", 1, json!({"protocolVersion": asked}));
            let out = converse(ApprovalMode::AllowAll, HeadlessApproval::Deny, &input);
            assert_eq!(out[0]["result"]["protocolVersion"], answer);
        }
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let input = line("tools/call", 1, json!({"name":"frobnicate"}));
        let out = converse(ApprovalMode::AllowAll, HeadlessApproval::Deny, &input);
        assert_eq!(out[0]["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn tool_panic_becomes_error_result() {
        let out = catch_tool("boom", || panic!("kaboom"));
        assert!(out.is_error);
        assert!(out.text.contains("kaboom"));
        let out = catch_tool("fine", || ToolOutput::text("ok"));
        assert!(!out.is_error);
    }

    #[test]
    fn cancelled_call_during_approval_is_denied_and_unanswered() {
        let input = format!(
            "{}{}{}{}",
            line("initialize", 1, json!({"capabilities":{"elicitation":{}}})),
            line(
                "tools/call",
                2,
                json!({"name":"get_app_state","arguments":{"app":"TextEdit"}})
            ),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}})
                .to_string()
                + "\n",
            line("ping", 3, json!({})),
        );
        let out = converse(ApprovalMode::Prompt, HeadlessApproval::Deny, &input);
        assert!(out.iter().any(|m| m["method"] == "elicitation/create"));
        assert!(!out.iter().any(|m| m["id"] == 2), "{out:#?}");
        assert!(out.iter().any(|m| m["id"] == 3));
    }

    #[test]
    fn silent_client_approval_times_out() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let mut cfg = Config::default();
        cfg.approvals.mode = ApprovalMode::Prompt;
        cfg.overlay.confirm_timeout_secs = 1;
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let engine = Engine::new(backend, ConfigStore::in_memory(cfg))
            .with_time(std::time::Instant::now, |_| {});
        for l in [
            line("initialize", 1, json!({"capabilities":{"elicitation":{}}})),
            line(
                "tools/call",
                2,
                json!({"name":"get_app_state","arguments":{"app":"TextEdit"}}),
            ),
        ] {
            tx.send(Ok(RawLine::Line(l.into_bytes()))).unwrap();
        }
        let (out_tx, out_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            let mut server = Server::from_lines(engine, rx, &mut out, HeadlessApproval::Deny);
            let _ = server.run();
            drop(server);
            let _ = out_tx.send(out);
        });
        // The client never answers the elicitation; the call must still end.
        std::thread::sleep(Duration::from_millis(1500));
        drop(tx);
        let out = out_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let msgs: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let call = msgs
            .iter()
            .find(|m| m["id"] == 2)
            .expect("tool call answered");
        assert_eq!(call["result"]["isError"], true);
    }

    #[test]
    fn headless_allow_does_not_confirm_guarded_actions() {
        let mut server = Server::new(
            engine(ApprovalMode::AllowAll),
            Cursor::new(Vec::new()),
            Vec::new(),
            HeadlessApproval::Allow,
        );
        assert!(!server.confirm("press Send"));
    }

    #[test]
    fn capped_line_reader() {
        let mut r = Cursor::new(b"abc\ndefghij\nk".to_vec());
        assert!(matches!(read_capped_line(&mut r, 4).unwrap(), RawLine::Line(l) if l == b"abc\n"));
        assert!(matches!(
            read_capped_line(&mut r, 4).unwrap(),
            RawLine::TooLong
        ));
        assert!(matches!(read_capped_line(&mut r, 4).unwrap(), RawLine::Line(l) if l == b"k"));
        assert!(matches!(read_capped_line(&mut r, 4).unwrap(), RawLine::Eof));
    }

    #[test]
    fn skills_are_offered_as_prompts_and_resources() {
        let input = format!(
            "{}{}{}{}{}{}",
            line(
                "initialize",
                1,
                json!({"protocolVersion":"2025-06-18","capabilities":{}})
            ),
            line("prompts/list", 2, json!({})),
            line("prompts/get", 3, json!({"name": "files"})),
            line("resources/list", 4, json!({})),
            line(
                "resources/read",
                5,
                json!({"uri": "computer-use://skills/files"})
            ),
            line(
                "resources/read",
                6,
                json!({"uri": "computer-use://skills/nope"})
            ),
        );
        let out = converse(ApprovalMode::AllowAll, HeadlessApproval::Deny, &input);
        let caps = &out[0]["result"]["capabilities"];
        assert!(
            caps["prompts"].is_object() && caps["resources"].is_object(),
            "{caps}"
        );
        assert!(out[1]["result"]["prompts"].as_array().unwrap().len() >= 10);
        let text = out[2]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert!(text.starts_with("# "));
        assert!(!out[3]["result"]["resources"].as_array().unwrap().is_empty());
        assert_eq!(
            out[4]["result"]["contents"][0]["text"].as_str().unwrap(),
            text
        );
        assert_eq!(out[5]["error"]["code"], -32002);
    }
}
