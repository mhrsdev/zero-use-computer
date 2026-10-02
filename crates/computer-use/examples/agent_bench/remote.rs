//! A server of any version, run as its own process and spoken to over MCP
//! (stdio): the benchmark's way to measure earlier releases with the same
//! scenarios (`--server`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use computer_use::imaging::EncodedImage;
use computer_use::tools::{ToolDefinition, ToolOutput};
use serde_json::{Value, json};

/// How the server is started: its program, its arguments (before `serve`),
/// the folder it keeps its settings in and the settings it gets.
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub home: std::path::PathBuf,
    /// Settings for it (`--server-config`), copied in as its config.toml.
    pub config: Option<std::path::PathBuf>,
}

pub struct Remote {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    next_id: u64,
    /// What `initialize` said: the server's instructions.
    pub instructions: String,
    tools: Option<Vec<ToolDefinition>>,
}

impl Remote {
    pub fn start(launch: &Launch) -> Result<Self, String> {
        let _ = std::fs::create_dir_all(&launch.home);
        let config = launch.home.join("config.toml");
        if let Some(from) = &launch.config {
            std::fs::copy(from, &config)
                .map_err(|e| format!("can't copy {}: {e}", from.display()))?;
        }
        let mut child = Command::new(&launch.program)
            .arg("--config")
            .arg(&config)
            .args(&launch.args)
            .arg("serve")
            .env("COMPUTER_USE_HOME", &launch.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("can't start {}: {e}", launch.program))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && tx.send(v).is_err()
                {
                    break;
                }
            }
        });
        let mut r = Remote {
            child,
            stdin,
            lines,
            next_id: 0,
            instructions: String::new(),
            tools: None,
        };
        let init = r.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "agent-bench", "version": "1"},
            }),
        )?;
        r.instructions = init["instructions"].as_str().unwrap_or("").to_string();
        r.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(r)
    }

    fn send(&mut self, v: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{v}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("server gone: {e}"))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let msg = self
                .lines
                .recv_timeout(Duration::from_secs(180))
                .map_err(|_| format!("no answer to {method}"))?;
            if msg["method"] == "notifications/tools/list_changed" {
                self.tools = None;
                continue;
            }
            if msg["id"] != json!(id) {
                continue;
            }
            if let Some(e) = msg.get("error") {
                return Err(format!("{method}: {e}"));
            }
            return Ok(msg["result"].clone());
        }
    }

    /// The tools the server lists now (asked again when it says they
    /// changed).
    pub fn tool_definitions(&mut self) -> Vec<ToolDefinition> {
        if self.tools.is_none() {
            let listed = self.request("tools/list", json!({})).unwrap_or_default();
            let defs = listed["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| ToolDefinition {
                    name: t["name"].as_str().unwrap_or("").to_string().into(),
                    title: t["title"].as_str().unwrap_or("").to_string().into(),
                    description: t["description"].as_str().unwrap_or("").to_string().into(),
                    input_schema: t["inputSchema"].clone(),
                    annotations: t["annotations"].clone(),
                })
                .collect();
            self.tools = Some(defs);
        }
        self.tools.clone().unwrap_or_default()
    }

    pub fn call_tool(&mut self, name: &str, args: Value) -> ToolOutput {
        let result = match self.request("tools/call", json!({"name": name, "arguments": args})) {
            Ok(r) => r,
            Err(e) => {
                return ToolOutput {
                    text: e,
                    image: None,
                    is_error: true,
                };
            }
        };
        let mut text = Vec::new();
        let mut image = None;
        for c in result["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("text") => text.push(c["text"].as_str().unwrap_or("").to_string()),
                Some("image") => image = decode_image(c),
                _ => {}
            }
        }
        ToolOutput {
            text: text.join("\n"),
            image,
            is_error: result["isError"].as_bool().unwrap_or(false),
        }
    }
}

fn decode_image(c: &Value) -> Option<EncodedImage> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(c["data"].as_str()?)
        .ok()?;
    let mime = match c["mimeType"].as_str()? {
        "image/jpeg" => "image/jpeg",
        _ => "image/png",
    };
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&data))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    Some(EncodedImage {
        mime,
        data,
        width,
        height,
    })
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
