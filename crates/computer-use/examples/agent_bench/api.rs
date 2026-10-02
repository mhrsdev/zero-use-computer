//! The real model: Claude's Messages API, reached with `curl` like the
//! decision model (no HTTP library in the build). The key goes to curl on
//! its input, never on a command line.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

/// Token counts of one request, as the API reports them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
}

impl Usage {
    pub fn of(v: &Value) -> Self {
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_creation: n("cache_creation_input_tokens"),
        }
    }

    pub fn add(&mut self, o: Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_creation += o.cache_creation;
    }

    /// Everything the model read on this request (cached or not).
    pub fn prompt(&self) -> u64 {
        self.input + self.cache_read + self.cache_creation
    }
}

/// Prices per million tokens, for the cost column.
#[derive(Debug, Clone, Copy)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// 5-minute cache writes.
    pub cache_write: f64,
}

impl Prices {
    /// Anthropic's first-party list prices (per million tokens) for the
    /// models a benchmark is likely to use; `BENCH_PRICES` overrides them
    /// ("input,output,cache_read,cache_write").
    pub fn for_model(model: &str) -> Option<Self> {
        if let Ok(s) = std::env::var("BENCH_PRICES") {
            let v: Vec<f64> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            if let [input, output, cache_read, cache_write] = v[..] {
                return Some(Self {
                    input,
                    output,
                    cache_read,
                    cache_write,
                });
            }
        }
        let (input, output, cache_read) = match model {
            "claude-opus-5-5" => (4.0, 20.0, 0.20),
            "claude-sonnet-5-5" | "claude-sonnet-5" => (2.0, 10.0, 0.20),
            "claude-haiku-4-5" => (1.0, 5.0, 0.10),
            _ => return None,
        };
        Some(Self {
            input,
            output,
            cache_read,
            cache_write: input * 1.25,
        })
    }

    pub fn cost(&self, u: &Usage) -> f64 {
        (u.input as f64 * self.input
            + u.output as f64 * self.output
            + u.cache_read as f64 * self.cache_read
            + u.cache_creation as f64 * self.cache_write)
            / 1e6
    }
}

/// Models that take the server-side refusal fallback (`fallbacks:
/// "default"`). The benchmark records which model served each turn, so a
/// fallback never passes unnoticed.
fn takes_fallbacks(model: &str) -> bool {
    matches!(
        model,
        "claude-fable-5-1" | "claude-opus-5-5" | "claude-opus-5" | "claude-sonnet-5-5"
    )
}

pub struct Client {
    url: String,
    auth: String,
    pub model: String,
    pub effort: Option<String>,
    pub timeout: Duration,
}

impl Client {
    /// From the environment: `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`)
    /// and `ANTHROPIC_BASE_URL` (default https://api.anthropic.com).
    pub fn from_env(model: &str, effort: Option<String>) -> Result<Self, String> {
        let auth = if let Ok(k) = std::env::var("ANTHROPIC_API_KEY")
            && !k.trim().is_empty()
        {
            format!("x-api-key: {}", k.trim())
        } else if let Ok(t) = std::env::var("ANTHROPIC_AUTH_TOKEN")
            && !t.trim().is_empty()
        {
            format!("Authorization: Bearer {}", t.trim())
        } else {
            return Err("set ANTHROPIC_API_KEY (or ANTHROPIC_AUTH_TOKEN) to run the real model; --scripted runs without one".into());
        };
        let base = std::env::var("BENCH_API_URL")
            .or_else(|_| std::env::var("ANTHROPIC_BASE_URL"))
            .unwrap_or_else(|_| "https://api.anthropic.com".into());
        Ok(Self {
            url: format!("{}/v1/messages", base.trim_end_matches('/')),
            auth,
            model: model.to_string(),
            effort,
            timeout: Duration::from_secs(600),
        })
    }

    /// One request. `system` and `tools` are the stable prefix: a cache
    /// breakpoint on the last system block covers both, and automatic
    /// caching follows the growing conversation.
    pub fn create(
        &self,
        system: &str,
        tools: &[Value],
        messages: &[Value],
    ) -> Result<Value, String> {
        let mut body = json!({
            "model": self.model,
            "max_tokens": 16000,
            "system": [{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}],
            "tools": tools,
            "messages": messages,
            "cache_control": {"type": "ephemeral"},
        });
        if let Some(effort) = &self.effort {
            body["output_config"] = json!({"effort": effort});
        }
        let mut headers = vec![
            self.auth.clone(),
            "anthropic-version: 2023-06-01".to_string(),
            "content-type: application/json".to_string(),
        ];
        if takes_fallbacks(&self.model) {
            body["fallbacks"] = json!("default");
            headers.push("anthropic-beta: server-side-fallback-2026-07-01".into());
        }
        let mut last = String::new();
        for attempt in 0..4 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
            }
            let (code, text) = post(&self.url, &headers, &body.to_string(), self.timeout)?;
            if code == 200 {
                return serde_json::from_str(&text).map_err(|e| format!("bad reply: {e}"));
            }
            last = format!(
                "HTTP {code}: {}",
                text.chars().take(400).collect::<String>()
            );
            // Rate limits, overload and server errors are worth another try.
            if !(code == 429 || code == 529 || code >= 500) {
                break;
            }
        }
        Err(last)
    }
}

impl Client {
    /// The real number of tokens `content` (one user message) costs, from
    /// the token-counting endpoint.
    pub fn count(&self, content: &Value) -> Result<u64, String> {
        let url = format!("{}/count_tokens", self.url);
        let body = json!({
            "model": self.model,
            "messages": [{"role": "user", "content": content}],
        });
        let headers = vec![
            self.auth.clone(),
            "anthropic-version: 2023-06-01".to_string(),
            "content-type: application/json".to_string(),
        ];
        let (code, text) = post(&url, &headers, &body.to_string(), self.timeout)?;
        if code != 200 {
            return Err(format!(
                "HTTP {code}: {}",
                text.chars().take(200).collect::<String>()
            ));
        }
        serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["input_tokens"].as_u64())
            .ok_or_else(|| "no input_tokens in the reply".to_string())
    }
}

/// A curl config value: quoted, with `\` and `"` escaped.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// POST `body`; the HTTP status and the reply. The body goes through a
/// temporary file (requests carry screenshots and can be large).
fn post(
    url: &str,
    headers: &[String],
    body: &str,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let dir = std::env::temp_dir().join(format!("agent-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("request.json");
    std::fs::write(&file, body).map_err(|e| e.to_string())?;
    let mut config = format!("url = {}\n", quote(url));
    for h in headers {
        config.push_str(&format!("header = {}\n", quote(h)));
    }
    config.push_str(&format!(
        "data-binary = {}\n",
        quote(&format!("@{}", file.display()))
    ));
    let mut child = Command::new("curl")
        .args(["-sS", "--proto", "=https,http", "--max-time"])
        .arg(format!("{:.0}", timeout.as_secs_f64()))
        .args(["-w", "\n%{http_code}", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config.as_bytes());
    }
    let mut out = Vec::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_end(&mut out);
    }
    let mut err = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }
    let _ = child.wait();
    let _ = std::fs::remove_file(&file);
    let cut = out.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let code: u16 = String::from_utf8_lossy(&out[cut..])
        .trim()
        .parse()
        .unwrap_or(0);
    out.truncate(cut);
    if code == 0 {
        return Err(format!("curl failed: {}", err.trim()));
    }
    Ok((code, String::from_utf8_lossy(&out).into_owned()))
}
