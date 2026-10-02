//! One run: a fresh fixture app, a fresh engine, and a record of every
//! tool call (what it returned and what that costs).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use computer_use::config::{Config, ConfigStore};
use computer_use::engine::Engine;
use computer_use::tools::ToolOutput;
use serde_json::{Value, json};

pub type Eng = Engine<Box<dyn computer_use::Backend>>;

/// One tool call as the model would receive it.
#[derive(Debug, Clone)]
pub struct CallRecord {
    pub tool: String,
    /// Estimated tokens of the arguments (what the model writes).
    pub args_tokens: usize,
    /// Estimated tokens of the text returned.
    pub text_tokens: usize,
    /// Estimated tokens of the image returned (width × height / 750).
    pub image_tokens: usize,
    pub image: Option<(u32, u32)>,
    pub error: bool,
    pub ms: f64,
    /// Real tokens of the result (`--calibrate`), when counted.
    pub real_tokens: Option<u64>,
}

impl CallRecord {
    pub fn to_json(&self) -> Value {
        json!({
            "tool": self.tool,
            "args_tok": self.args_tokens,
            "text_tok": self.text_tokens,
            "image_tok": self.image_tokens,
            "image": self.image.map(|(w, h)| json!([w, h])),
            "error": self.error,
            "ms": (self.ms * 10.0).round() / 10.0,
            "real_tok": self.real_tokens,
        })
    }
}

pub struct Session {
    pub engine: Eng,
    pub app: String,
    pub calls: Vec<CallRecord>,
    /// Text of every result so far, newest last (what the model has seen).
    pub seen: Vec<String>,
    /// Every result in full (`--calibrate` counts their real tokens).
    pub outputs: Vec<ToolOutput>,
    fixture: Child,
    pub state_file: PathBuf,
    /// Print every call and its result (`--verbose`).
    pub verbose: bool,
}

/// Settings for a run: defaults (or a file) with the parts that must not
/// leak in from the machine: no audit log, no saved scripts as tools, no
/// decision model unless the file sets one.
pub fn config(file: Option<&Path>, preset: &str, scripts: &Path) -> Result<Config, String> {
    let mut cfg = match file {
        Some(p) => {
            ConfigStore::load(Some(p))
                .map_err(|e| e.to_string())?
                .config
        }
        None => Config::default(),
    };
    match preset {
        "default" => {}
        // The way Codex's computer use behaves, as examples/compare.rs
        // simulates it: a screenshot with every look, no screen memory, no
        // picture dedupe, whole-window pictures, no change report.
        "codex" => {
            cfg.screenshot.attach = computer_use::config::AttachMode::Always;
            cfg.screenshot.scope = computer_use::config::ShotScope::Full;
            cfg.cache.enabled = false;
            cfg.cache.dedupe_screenshots = false;
            cfg.tree.report_changes = false;
        }
        other => return Err(format!("unknown preset {other} (default, codex)")),
    }
    cfg.audit.enabled = false;
    cfg.hot_reload = false;
    cfg.script.dir = Some(scripts.to_path_buf());
    Ok(cfg)
}

impl Session {
    /// Start the scenario's app and an engine that sees it.
    pub fn start(
        python: &str,
        fixture: &Path,
        scenario: &str,
        app: &str,
        cfg: Config,
        state_file: PathBuf,
    ) -> Result<Self, String> {
        let _ = std::fs::remove_file(&state_file);
        let child = Command::new(python)
            .arg(fixture)
            .args(["--scenario", scenario, "--state"])
            .arg(&state_file)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("can't start the fixture with {python}: {e}"))?;
        let backend = computer_use::platform_backend().map_err(|e| e.to_string())?;
        let engine = Engine::new(backend, ConfigStore::in_memory(cfg));
        let mut s = Self {
            engine,
            app: app.to_string(),
            calls: Vec::new(),
            seen: Vec::new(),
            outputs: Vec::new(),
            fixture: child,
            state_file,
            verbose: false,
        };
        // Wait until the app is listed and its window answers.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let out = s.engine.call_tool("list_apps", json!({}));
            if out
                .text
                .lines()
                .any(|l| l.starts_with(&format!("- {app} ")))
                && s.state().is_some()
            {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("{app} didn't show up: {}", out.text));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        // A fresh engine for the run: nothing it learnt while waiting.
        let backend = computer_use::platform_backend().map_err(|e| e.to_string())?;
        let cfg = s.engine.store().config.clone();
        s.engine = Engine::new(backend, ConfigStore::in_memory(cfg));
        Ok(s)
    }

    /// What the app wrote about what was done.
    pub fn state(&self) -> Option<Value> {
        let text = std::fs::read_to_string(&self.state_file).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Run a tool and record it; the result's text, or its error.
    pub fn call(&mut self, tool: &str, args: Value) -> Result<String, String> {
        let out = self.call_raw(tool, args);
        if out.is_error {
            Err(format!("{tool}: {}", out.text))
        } else {
            Ok(out.text)
        }
    }

    /// Run a tool and record it, whatever it returns.
    pub fn call_raw(&mut self, tool: &str, args: Value) -> ToolOutput {
        let args_tokens = computer_use::text::estimate_tokens(&args.to_string());
        if self.verbose {
            eprintln!("> {tool} {args}");
        }
        let t0 = Instant::now();
        let out = self.engine.call_tool(tool, args);
        if self.verbose {
            let img = out
                .image
                .as_ref()
                .map(|i| format!(" [image {}x{}]", i.width, i.height))
                .unwrap_or_default();
            eprintln!("{}{img}\n", out.text);
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let image_tokens = out
            .image
            .as_ref()
            .map_or(0, |i| (i.width as usize * i.height as usize).div_ceil(750));
        self.calls.push(CallRecord {
            tool: tool.to_string(),
            args_tokens,
            text_tokens: computer_use::text::estimate_tokens(&out.text),
            image_tokens,
            image: out.image.as_ref().map(|i| (i.width, i.height)),
            error: out.is_error,
            ms,
            real_tokens: None,
        });
        self.seen.push(out.text.clone());
        self.outputs.push(out.clone());
        out
    }

    /// The element index on the newest line the tools showed that
    /// `pred` accepts (lines of trees, diffs and find_element results:
    /// `[+~] <index> <role> "<name>" …`).
    pub fn index(&self, pred: impl Fn(&str) -> bool) -> Option<u32> {
        for text in self.seen.iter().rev() {
            // Records (look-alike siblings on one line) one a line.
            let text = computer_use::tree::expand(text);
            for line in text.lines().rev() {
                let l = line.trim_start();
                let l = l
                    .strip_prefix("+ ")
                    .or_else(|| l.strip_prefix("~ "))
                    .unwrap_or(l);
                let Some((num, rest)) = l.split_once(' ') else {
                    continue;
                };
                if let Ok(i) = num.parse::<u32>()
                    && pred(rest)
                {
                    return Some(i);
                }
            }
        }
        None
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.fixture.kill();
        let _ = self.fixture.wait();
    }
}

/// A Python that can import GTK 3.
pub fn find_python(given: Option<&str>) -> Option<String> {
    let mut cands: Vec<String> = given.map(String::from).into_iter().collect();
    if let Ok(p) = std::env::var("BENCH_PYTHON") {
        cands.push(p);
    }
    for c in [
        "python3",
        "python3.13",
        "python3.12",
        "python3.11",
        "python3.10",
    ] {
        cands.push(c.into());
    }
    cands.into_iter().find(|c| {
        Command::new(c)
            .args([
                "-c",
                "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}
