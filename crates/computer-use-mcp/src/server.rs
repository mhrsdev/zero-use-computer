//! MCP over stdio: newline-delimited JSON-RPC. Messages are read on a
//! thread of their own, so a cancel or a ping is seen while a call runs;
//! the answers come from [`Core`].

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use computer_use::Backend;
use computer_use::engine::Engine;
use serde::Serialize;
use serde_json::Value;

use crate::core::{Cancels, Core, Notifier, cancelled_request};
use crate::jsonrpc::*;

pub struct Server<R: BufRead, W: Write, B: Backend> {
    core: Core<B>,
    /// Read on a thread of its own (taken by `run`), so a cancel reaches a
    /// call while it runs.
    reader: Option<R>,
    out: Out<W>,
    /// Set when the input ended and nobody is left to answer
    /// (`stop_when_input_ends`).
    closed: Option<Arc<AtomicBool>>,
}

/// The output, shared by the reader thread (pings, unreadable lines), the
/// server (answers) and a running call (its progress): one message a line,
/// never two mixed.
struct Out<W>(Arc<Mutex<W>>);

impl<W> Clone for Out<W> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<W: Write> Out<W> {
    fn send(&self, value: &impl Serialize) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut w = self.0.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(&line)?;
        w.flush()
    }
}

/// What the reader thread passes on.
enum Input {
    One(Incoming),
    Batch(Vec<Result<Incoming, Box<Response>>>),
}

impl<R: BufRead + Send + 'static, W: Write + Send + 'static, B: Backend> Server<R, W, B> {
    pub fn new(engine: Engine<B>, reader: R, writer: W) -> Self {
        Self {
            core: Core::new(engine, true),
            reader: Some(reader),
            out: Out(Arc::new(Mutex::new(writer))),
            closed: None,
        }
    }

    /// When the input ends, the client has gone (MCP's way of shutting a
    /// stdio server down): stop the call that is running, as the stop key
    /// would, and run none of those still waiting, rather than act on the
    /// desktop for nobody until the client kills the server mid-drag.
    pub fn stop_when_input_ends(mut self) -> Self {
        self.closed = Some(Arc::new(AtomicBool::new(false)));
        self
    }

    /// Read and dispatch messages until stdin closes. Messages are read on a
    /// thread of their own: a `notifications/cancelled` for the call that
    /// is running ends it, as the stop key would, and a `ping` is answered
    /// at once.
    pub fn run(&mut self) -> std::io::Result<()> {
        let Some(reader) = self.reader.take() else {
            return Ok(());
        };
        let engine_cancel = self.core.engine().cancel_handle();
        let cancels = Arc::new(Cancels::default());
        // Unbounded: a reader that waited for room would stop reading the
        // cancels for the call that is running.
        let (tx, rx) = channel();
        {
            let cancels = cancels.clone();
            let engine_cancel = engine_cancel.clone();
            let closed = self.closed.clone();
            let out = self.out.clone();
            std::thread::Builder::new()
                .name("mcp-reader".into())
                .spawn(move || {
                    read_loop(reader, &tx, &out, &cancels, &engine_cancel);
                    if let Some(closed) = closed {
                        log::info!("the client closed the input: stopping");
                        closed.store(true, Ordering::SeqCst);
                        engine_cancel.store(true, Ordering::SeqCst);
                    }
                })?;
        }
        let out = self.out.clone();
        let notify: Notifier = Arc::new(move |v| {
            let _ = out.send(&v);
        });
        let closed = || {
            self.closed
                .as_ref()
                .is_some_and(|c| c.load(Ordering::SeqCst))
        };
        while !self.core.is_shut_down() {
            if closed() {
                break;
            }
            let input = rx.recv();
            if closed() {
                break;
            }
            match input {
                Ok(Ok(Input::One(msg))) => {
                    let key = msg.id.clone().unwrap_or(Value::Null);
                    if let Some(resp) = self.core.handle(msg, &cancels, &key, Some(&notify)) {
                        self.out.send(&resp)?;
                    }
                }
                Ok(Ok(Input::Batch(items))) => {
                    let mut answers = Vec::new();
                    for item in items {
                        match item {
                            Ok(msg) => {
                                let key = msg.id.clone().unwrap_or(Value::Null);
                                answers.extend(self.core.handle(
                                    msg,
                                    &cancels,
                                    &key,
                                    Some(&notify),
                                ));
                            }
                            Err(reply) => answers.push(*reply),
                        }
                    }
                    // A batch of notifications gets no answer at all.
                    if !answers.is_empty() {
                        self.out.send(&answers)?;
                    }
                }
                Ok(Err(e)) => return Err(e),
                // The input ended.
                Err(_) => break,
            }
            // A hot-reloaded config can change which tools exist; tell the client.
            if let Some(note) = self.core.tools_changed() {
                self.out.send(&note)?;
            }
        }
        Ok(())
    }
}

/// Read messages and pass them on until the input ends. A line that is too
/// long, not UTF-8 or not a JSON-RPC message is answered with an error and
/// skipped: one bad line never ends the session. Cancels are applied and
/// pings answered here, while the server may be busy with a call.
fn read_loop<R: BufRead, W: Write>(
    mut reader: R,
    tx: &Sender<std::io::Result<Input>>,
    out: &Out<W>,
    cancels: &Cancels,
    engine_cancel: &AtomicBool,
) {
    let reply = |r: &Response| {
        log::warn!("dropping a bad message: {:?}", r.error);
        out.send(r).is_ok()
    };
    loop {
        let input = match read_capped_line(&mut reader, MAX_LINE_BYTES) {
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
            Ok(RawLine::Eof) => return,
            Ok(RawLine::TooLong) => {
                let r = Response::err(
                    Value::Null,
                    INVALID_REQUEST,
                    format!("message too long (over {MAX_LINE_BYTES} bytes)"),
                );
                if !reply(&r) {
                    return;
                }
                continue;
            }
            Ok(RawLine::Line(bytes)) => match std::str::from_utf8(&bytes) {
                Err(_) => {
                    let r = Response::err(
                        Value::Null,
                        PARSE_ERROR,
                        "parse error: message is not valid UTF-8",
                    );
                    if !reply(&r) {
                        return;
                    }
                    continue;
                }
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => match parse_payload(line) {
                    Ok(Payload::One(msg)) => {
                        if let Some(id) = cancelled_request(&msg) {
                            cancels.cancel(id, engine_cancel);
                        }
                        // Answered here, so a client checking that the
                        // server is alive isn't kept waiting by a long call.
                        if msg.method.as_deref() == Some("ping")
                            && let Some(id) = msg.id.clone()
                        {
                            if out.send(&Response::ok(id, serde_json::json!({}))).is_err() {
                                return;
                            }
                            continue;
                        }
                        Input::One(msg)
                    }
                    Ok(Payload::Batch(items)) => {
                        for msg in items.iter().flatten() {
                            if let Some(id) = cancelled_request(msg) {
                                cancels.cancel(id, engine_cancel);
                            }
                        }
                        Input::Batch(items)
                    }
                    Err(r) => {
                        if !reply(&r) {
                            return;
                        }
                        continue;
                    }
                },
            },
        };
        if tx.send(Ok(input)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use computer_use::config::{Config, ConfigStore};
    use computer_use::engine::Engine;
    use computer_use::mock::MockBackend;
    use serde_json::json;
    use std::io::Cursor;

    /// What the server wrote (shared with the server, which needs a
    /// writer it can hand to its threads).
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Buf {
        fn take(&self) -> Vec<u8> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    impl std::io::Write for Buf {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn engine() -> Engine<MockBackend> {
        engine_with_scripts(&std::env::temp_dir().join("cu-server-tests-no-scripts"))
    }

    /// An engine whose saved scripts live in `dir` (never the user's own).
    fn engine_with_scripts(dir: &std::path::Path) -> Engine<MockBackend> {
        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let mut config = Config::default();
        config.script.dir = Some(dir.to_path_buf());
        // Every tool listed (the tool manager has tests of its own).
        config.tools.manager = computer_use::config::ToolManager::Off;
        Engine::new(backend, ConfigStore::in_memory(config))
            .with_time(std::time::Instant::now, |_| {})
    }

    /// Run a scripted client conversation, return the lines the server wrote.
    fn converse(input: &str) -> Vec<Value> {
        let reader = Cursor::new(input.to_string());
        let out = Buf::default();
        {
            let mut server = Server::new(engine(), reader, out.clone());
            server.run().unwrap();
        }
        String::from_utf8(out.take())
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
        let out = converse(&input);
        assert_eq!(out.len(), 3);
        // initialize
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "computer-use");
        // tools/list has every tool the default settings expose
        let mut every = computer_use::Config::default();
        every.tools.manager = computer_use::config::ToolManager::Off;
        let all = computer_use::tools::definitions_from(&every).len();
        assert_eq!(out[1]["result"]["tools"].as_array().unwrap().len(), all);
        // list_apps ran
        let text = out[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("TextEdit"));
        assert_eq!(out[2]["result"]["isError"], false);
    }

    /// A conversation with an engine set up by `cfg`.
    fn converse_with(input: &str, cfg: impl FnOnce(&mut computer_use::Config)) -> Vec<Value> {
        let mut e = engine();
        let mut c = e.store().config.clone();
        cfg(&mut c);
        e.set_config(ConfigStore::in_memory(c));
        let reader = Cursor::new(input.to_string());
        let out = Buf::default();
        {
            let mut server = Server::new(e, reader, out.clone());
            server.run().unwrap();
        }
        String::from_utf8(out.take())
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn instructions_can_be_short_or_left_out() {
        use computer_use::config::Instructions;
        let init = line("initialize", 1, json!({"capabilities":{}}));
        let full = converse(&init)[0]["result"]["instructions"]
            .as_str()
            .unwrap()
            .len();
        let out = converse_with(&init, |c| c.server.instructions = Instructions::Short);
        let short = out[0]["result"]["instructions"].as_str().unwrap();
        assert!(
            short.len() * 3 < full && short.contains("confirm"),
            "{short}"
        );
        let out = converse_with(&init, |c| c.server.instructions = Instructions::Off);
        assert!(out[0]["result"].get("instructions").is_none());
    }

    #[test]
    fn by_default_the_list_is_the_base_and_the_rest_runs_through_use_tool() {
        let input = format!(
            "{}{}{}{}{}",
            line("initialize", 1, json!({"capabilities":{}})),
            line("tools/list", 2, json!({})),
            line(
                "tools/call",
                3,
                json!({"name":"find_tools","arguments":{"query":"arrange the windows"}})
            ),
            line(
                "tools/call",
                4,
                json!({"name":"use_tool","arguments":{"name":"window","arguments":{"app":"TextEdit","action":"list"}}})
            ),
            line("tools/list", 5, json!({})),
        );
        let out = converse_with(&input, |c| {
            c.tools.manager = computer_use::config::ToolManager::default()
        });
        let names = |v: &Value| -> Vec<String> {
            v["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect()
        };
        let first = names(&out[1]);
        assert!(first.contains(&"use_tool".to_string()) && !first.contains(&"design".to_string()));
        let found = out[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(found.starts_with("window: "), "{found}");
        assert_eq!(out[3]["result"]["isError"], false, "{:?}", out[3]);
        // The list never changed, so no notification and the same tools.
        assert!(
            !out.iter()
                .any(|m| m["method"] == "notifications/tools/list_changed")
        );
        assert_eq!(names(&out[4]), first);
    }

    #[test]
    fn found_tools_join_the_list_and_the_client_is_told() {
        use computer_use::config::ToolManager;
        let input = format!(
            "{}{}{}{}",
            line("initialize", 1, json!({"capabilities":{}})),
            line("tools/list", 2, json!({})),
            line(
                "tools/call",
                3,
                json!({"name":"find_tools","arguments":{"category":"windows"}})
            ),
            line("tools/list", 4, json!({})),
        );
        let out = converse_with(&input, |c| c.tools.manager = ToolManager::ListChanged);
        let names = |v: &Value| -> Vec<String> {
            v["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert!(!names(&out[1]).contains(&"window".to_string()));
        assert!(
            out.iter()
                .any(|m| m["method"] == "notifications/tools/list_changed")
        );
        let last = out.iter().rev().find(|m| m["id"] == 4).unwrap();
        assert!(names(last).contains(&"window".to_string()));
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
        let out = converse(&input);
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
        std::fs::write(
            &path,
            "[tree]\nmax_nodes = 300\n[tools]\nmanager = \"off\"\n",
        )
        .unwrap();

        let mut backend = MockBackend::new();
        backend.add_app(MockBackend::text_editor(4242));
        let store = ConfigStore::load(Some(&path)).unwrap();
        let engine = Engine::new(backend, store).with_time(std::time::Instant::now, |_| {});

        // A reader that rewrites the config between the two requests (once
        // the first is answered: input is read ahead, on a thread).
        struct Script {
            lines: Vec<String>,
            path: std::path::PathBuf,
            step: usize,
            buf: Vec<u8>,
            at: usize,
            answered: std::sync::mpsc::Receiver<()>,
        }
        // Tells the reader when the answer to request 2 is out.
        struct Out {
            buf: Buf,
            answered: Option<std::sync::mpsc::Sender<()>>,
        }
        impl std::io::Write for Out {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.buf.write(data)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                if String::from_utf8_lossy(&self.buf.0.lock().unwrap()).contains("\"id\":2")
                    && let Some(tx) = self.answered.take()
                {
                    let _ = tx.send(());
                }
                Ok(())
            }
        }
        impl std::io::Read for Script {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                unreachable!()
            }
        }
        impl std::io::BufRead for Script {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                if self.at == self.buf.len() {
                    if self.step == 2 {
                        let _ = self.answered.recv_timeout(Duration::from_secs(10));
                        // Disable a tool and bump the mtime so the change is seen.
                        std::fs::write(
                            &self.path,
                            "[tree]\nmax_nodes = 300\n[tools]\nmanager = \"off\"\ndisabled = [\"drag\"]\n",
                        )?;
                        let f = std::fs::File::options().write(true).open(&self.path)?;
                        f.set_modified(SystemTime::now() + Duration::from_secs(5))?;
                    }
                    self.buf = self
                        .lines
                        .get(self.step)
                        .map(|l| l.as_bytes().to_vec())
                        .unwrap_or_default();
                    self.at = 0;
                    self.step += 1;
                }
                Ok(&self.buf[self.at..])
            }
            fn consume(&mut self, n: usize) {
                self.at += n;
            }
        }
        let (tx, answered) = std::sync::mpsc::channel();
        let reader = Script {
            lines: vec![
                line("initialize", 1, json!({"capabilities":{}})),
                line("tools/list", 2, json!({})),
                line("tools/call", 3, json!({"name":"list_apps","arguments":{}})),
                line("tools/list", 4, json!({})),
            ],
            path: path.clone(),
            step: 0,
            buf: Vec::new(),
            at: 0,
            answered,
        };
        let out = Buf::default();
        {
            let writer = Out {
                buf: out.clone(),
                answered: Some(tx),
            };
            let mut server = Server::new(engine, reader, writer);
            server.run().unwrap();
        }
        let msgs: Vec<Value> = String::from_utf8(out.take())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let first = msgs.iter().find(|m| m["id"] == 2).unwrap();
        let mut every = computer_use::Config::default();
        every.tools.manager = computer_use::config::ToolManager::Off;
        let all = computer_use::tools::definitions_from(&every).len();
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
    fn a_saved_script_becomes_a_tool_and_the_client_is_told() {
        let dir = std::env::temp_dir().join(format!("cu-server-saved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let input = format!(
            "{}{}{}{}{}",
            line("initialize", 1, json!({"capabilities":{}})),
            line("tools/list", 2, json!({})),
            line(
                "tools/call",
                3,
                json!({"name": "script", "arguments": {
                    "save": "double", "description": "Twice a number",
                    "params": {"n": {"type": "number"}}, "code": "args.n * 2"
                }})
            ),
            line("tools/list", 4, json!({})),
            line(
                "tools/call",
                5,
                json!({"name": "double", "arguments": {"n": 21}})
            ),
        );
        let out = Buf::default();
        {
            let mut server =
                Server::new(engine_with_scripts(&dir), Cursor::new(input), out.clone());
            server.run().unwrap();
        }
        let msgs: Vec<Value> = String::from_utf8(out.take())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(
            msgs.iter()
                .any(|m| m["method"] == "notifications/tools/list_changed"),
            "{msgs:#?}"
        );
        let listed = msgs.iter().find(|m| m["id"] == 4).unwrap();
        let tool = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "double")
            .expect("the saved script is listed");
        assert_eq!(tool["inputSchema"]["properties"]["n"]["type"], "number");
        let ran = msgs.iter().find(|m| m["id"] == 5).unwrap();
        let text = ran["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Result: 42"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skills_are_prompts_and_resources() {
        let input = format!(
            "{}{}{}",
            line("initialize", 1, json!({"capabilities":{}})),
            line("prompts/get", 2, json!({"name":"computer-use-security"})),
            line(
                "resources/read",
                3,
                json!({"uri":"computer-use://skills/nope/SKILL.md"})
            ),
        );
        let out = converse(&input);
        let caps = &out[0]["result"]["capabilities"];
        assert!(caps["prompts"].is_object() && caps["resources"].is_object());
        let text = out[1]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("security rules"), "{text}");
        assert_eq!(out[2]["error"]["code"], crate::catalog::RESOURCE_NOT_FOUND);
    }

    #[test]
    fn unknown_method_is_error() {
        let input = line("frobnicate", 1, json!({}));
        let out = converse(&input);
        assert_eq!(out[0]["error"]["code"], METHOD_NOT_FOUND);
        let input = line("tools/call", 2, json!({"name": "frobnicate"}));
        let out = converse(&input);
        assert_eq!(out[0]["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn bad_lines_are_answered_and_skipped() {
        let mut input = b"{oops\n\xff\xfe\n[1]\n".to_vec();
        input.extend_from_slice(line("ping", 7, json!({})).as_bytes());
        let out = Buf::default();
        {
            let mut server = Server::new(engine(), Cursor::new(input), out.clone());
            server.run().unwrap();
        }
        let msgs: Vec<Value> = String::from_utf8(out.take())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let codes: Vec<&Value> = msgs.iter().map(|m| &m["error"]["code"]).collect();
        assert_eq!(codes[..2], [&json!(PARSE_ERROR), &json!(PARSE_ERROR)]);
        // A batch of one unusable message: an array of one error (answered
        // by the server, so it may come after the ping, answered at once).
        let batch = msgs.iter().find(|m| m.is_array()).expect("{msgs:?}");
        assert_eq!(batch[0]["error"]["code"], INVALID_REQUEST);
        let ping = msgs.iter().find(|m| m["id"] == 7).expect("{msgs:?}");
        assert_eq!(ping["result"], json!({}));
        assert_eq!(msgs.len(), 4);
    }

    /// JSON-RPC batches (MCP 2025-03-26): one array of answers, in order,
    /// with none for notifications; a batch of notifications gets nothing.
    #[test]
    fn a_batch_is_answered_with_one_array() {
        let batch = json!([
            {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{}}},
            {"jsonrpc":"2.0","method":"notifications/initialized"},
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":{}}},
            {"jsonrpc":"2.0","id":3,"method":"nope"}
        ]);
        let only_notes = json!([{"jsonrpc":"2.0","method":"notifications/initialized"}]);
        let input = format!("{batch}\n{only_notes}\n");
        let msgs = converse(&input);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        let answers = msgs[0].as_array().unwrap();
        assert_eq!(answers.len(), 3);
        assert_eq!(answers[0]["result"]["protocolVersion"], "2025-03-26");
        assert!(
            answers[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("TextEdit")
        );
        assert_eq!(answers[2]["error"]["code"], METHOD_NOT_FOUND);
    }

    /// A call that asks for progress (`_meta.progressToken`) gets
    /// `notifications/progress` before its answer; one that doesn't, none.
    #[test]
    fn progress_comes_before_the_answer() {
        let steps = json!([
            {"tool": "get_app_state", "arguments": {}},
            {"tool": "set_value", "arguments": {"element_index": 4, "value": "hi"}}
        ]);
        let input = format!(
            "{}{}{}",
            line(
                "initialize",
                1,
                json!({"protocolVersion":"2025-06-18","capabilities":{}})
            ),
            line(
                "tools/call",
                2,
                json!({"name":"batch","arguments":{"app":"TextEdit","steps":steps},"_meta":{"progressToken":"p1"}})
            ),
            line(
                "tools/call",
                3,
                json!({"name":"batch","arguments":{"app":"TextEdit","steps":steps}})
            ),
        );
        let msgs = converse(&input);
        let progress: Vec<&Value> = msgs
            .iter()
            .filter(|m| m["method"] == "notifications/progress")
            .collect();
        assert_eq!(progress.len(), 2, "{msgs:#?}");
        assert_eq!(progress[0]["params"]["progressToken"], "p1");
        assert_eq!(progress[1]["params"]["progress"], 2.0);
        assert_eq!(progress[1]["params"]["total"], 2.0);
        assert_eq!(progress[1]["params"]["message"], "step 2 of 2: set_value");
        let at = |pred: &dyn Fn(&Value) -> bool| msgs.iter().position(pred).unwrap();
        assert!(at(&|m| m["method"] == "notifications/progress") < at(&|m| m["id"] == 2));
    }

    /// A ping is answered while a long call runs.
    #[test]
    fn a_ping_is_answered_during_a_long_call() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let dir = std::env::temp_dir().join(format!("cu-ping-{}", std::process::id()));
        let engine = engine_with_scripts(&dir);
        let out = Buf::default();
        let seen = out.clone();
        let client = std::thread::spawn(move || {
            let call = line(
                "tools/call",
                1,
                json!({"name":"script","arguments":{"code":"let n = 0; loop { n += 1; }"}}),
            );
            writer.write_all(call.as_bytes()).unwrap();
            writer
                .write_all(line("ping", 2, json!({})).as_bytes())
                .unwrap();
            let start = std::time::Instant::now();
            let answered = loop {
                if String::from_utf8_lossy(&seen.0.lock().unwrap()).contains(r#""id":2"#) {
                    break true;
                }
                if start.elapsed() > std::time::Duration::from_secs(10) {
                    break false;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            };
            let cancel = json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}});
            writer.write_all(format!("{cancel}\n").as_bytes()).unwrap();
            answered
        });
        {
            let mut server = Server::new(engine, std::io::BufReader::new(reader), out.clone());
            server.run().unwrap();
        }
        assert!(client.join().unwrap(), "the ping waited for the call");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn protocol_version_is_negotiated() {
        use crate::core::negotiate_protocol;
        assert_eq!(negotiate_protocol(Some("2025-03-26")), "2025-03-26");
        assert_eq!(negotiate_protocol(Some("1999-01-01")), "2025-06-18");
        assert_eq!(negotiate_protocol(None), "2025-06-18");
    }

    #[test]
    fn a_request_cancelled_before_it_starts_is_never_run() {
        let cancel =
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}});
        let call = line(
            "tools/call",
            5,
            json!({"name":"type_text","arguments":{"app":"TextEdit","text":"hi"}}),
        );
        let input = format!("{cancel}\n{call}{}", line("ping", 6, json!({})));
        let msgs = converse(&input);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert_eq!(msgs[0]["id"], 6);
    }

    #[test]
    fn a_client_that_goes_away_stops_the_work() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let dir = std::env::temp_dir().join(format!("cu-gone-{}", std::process::id()));
        let engine = engine_with_scripts(&dir);
        let client = std::thread::spawn(move || {
            let call = line(
                "tools/call",
                1,
                json!({"name":"script","arguments":{"code":"let n = 0; loop { n += 1; }"}}),
            );
            // A second call waits behind the first; then the client leaves.
            let typing = line(
                "tools/call",
                2,
                json!({"name":"type_text","arguments":{"app":"TextEdit","text":"hi"}}),
            );
            writer.write_all(call.as_bytes()).unwrap();
            writer.write_all(typing.as_bytes()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(writer);
        });
        let started = std::time::Instant::now();
        let out = Buf::default();
        {
            let mut server = Server::new(engine, std::io::BufReader::new(reader), out.clone())
                .stop_when_input_ends();
            server.run().unwrap();
        }
        client.join().unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "the script went on after the client left"
        );
        let text = String::from_utf8(out.take()).unwrap();
        assert!(!text.contains(r#""id":2"#), "the waiting call ran: {text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cancel_ends_the_call_that_is_running() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let dir = std::env::temp_dir().join(format!("cu-cancel-{}", std::process::id()));
        let engine = engine_with_scripts(&dir);
        let client = std::thread::spawn(move || {
            let call = line(
                "tools/call",
                1,
                json!({"name":"script","arguments":{"code":"let n = 0; loop { n += 1; }"}}),
            );
            writer.write_all(call.as_bytes()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            let cancel = json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}});
            writer.write_all(format!("{cancel}\n").as_bytes()).unwrap();
            writer
                .write_all(line("ping", 2, json!({})).as_bytes())
                .unwrap();
        });
        let started = std::time::Instant::now();
        let out = Buf::default();
        {
            let mut server = Server::new(engine, std::io::BufReader::new(reader), out.clone());
            server.run().unwrap();
        }
        client.join().unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "the cancel didn't end the script"
        );
        let msgs: Vec<Value> = String::from_utf8(out.take())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // No answer for the cancelled call; the next one is answered.
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert_eq!(msgs[0]["id"], 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
