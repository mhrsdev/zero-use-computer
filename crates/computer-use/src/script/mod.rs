//! Scripts: small programs the agent writes and the server runs, for what
//! the tools can't do in one call: loops and conditions over tool calls,
//! maths, data from files or the web, pictures built on the graph-paper
//! page. Saved scripts become tools of their own.
//!
//! The language is Rhai (sandboxed: no system access but the functions
//! registered in [`api`]). A script runs on a thread of its own; every tool
//! call it makes is sent back to the engine as a [`Request`], so it is an
//! ordinary tool call — the stop key, the pause while the user works and
//! the masking all apply. What a script does alone (maths, text, files, the
//! web) never touches the engine.
//!
//! zero-use-computer · mhrsdev

mod api;
mod io;
mod library;
mod page;

pub use library::{Library, Saved, valid_name};

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use rhai::EvalAltResult;
use serde_json::Value;

use crate::config::ScriptFiles;

/// The function reference (`script` with `help: true`).
pub const HELP: &str = include_str!("../../../../skills/computer-use/reference/scripts.md");

/// What a script asks of the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Run a tool: `{"ok", "text", "image"}` (an image id or null).
    Tool { name: String, args: Value },
    /// The elements of an app's window that match.
    Elements {
        app: String,
        window: Option<String>,
        role: Option<String>,
        name: Option<String>,
        text: Option<String>,
        editable: bool,
        max: usize,
    },
    /// The colours at points of a window (x/y as click takes them).
    Colors {
        app: String,
        window: Option<String>,
        points: Vec<(f64, f64)>,
    },
    /// A design: its size, cells, layers and paint steps.
    Design { name: String },
    /// Return this image (an id from a tool call) with the script's result.
    Show { image: u64 },
    /// Return this picture file with the script's result.
    ShowFile { path: PathBuf },
}

/// The engine's answer: a value for the script, or an error message.
pub type Reply = Result<Value, String>;

pub enum Msg {
    Ask(Request),
    Done(Outcome),
}

/// How a run ended.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    /// What the script printed.
    pub output: String,
    /// Its last value, written out (none when it ends without one).
    pub value: Option<String>,
    /// Why it failed, with where.
    pub error: Option<String>,
    /// Tool calls it made.
    pub calls: usize,
    /// The user's stop key ended it.
    pub stopped: bool,
}

/// What a script may use.
#[derive(Debug, Clone)]
pub struct Env {
    pub files: ScriptFiles,
    pub web: bool,
    /// Saved scripts (and `files/`, the scripts' own folder, and the
    /// memory file).
    pub library: PathBuf,
    pub max_seconds: u64,
    /// The user's stop key.
    pub stop: Arc<AtomicBool>,
    /// Tools that take an `app` (set_app fills it in).
    pub app_tools: Vec<String>,
    /// The decision model, for decide(), ask(), choose() and score().
    pub decision: crate::config::DecisionConfig,
    /// The settings key as the user knows it ("Ctrl+Alt+J"; `None`: there
    /// is none), for the decision model's errors.
    pub settings_key: Option<String>,
    /// Never read or written by a script, whatever `[script] files` says:
    /// the server's folder (its settings, with the decision model's key,
    /// the hub's token, the memory) and the settings file. The scripts'
    /// own folder inside it stays theirs.
    pub private: Vec<PathBuf>,
}

impl Env {
    /// The scripts' own folder for files.
    pub fn workspace(&self) -> PathBuf {
        self.library.join("files")
    }
}

pub struct Job {
    pub code: String,
    /// The saved script's name, for messages.
    pub source: Option<String>,
    /// `args` in the script (a map).
    pub args: Value,
    /// `data` in the script (anything).
    pub data: Value,
    pub env: Env,
}

/// A script running on its thread.
pub struct Running {
    pub rx: Receiver<Msg>,
    pub tx: Sender<Reply>,
    /// Set by the engine when the script is out of time.
    pub abort: Arc<AtomicBool>,
    /// The script's stop flag (`Env::stop`): set by the engine on the stop
    /// key or a cancel.
    pub halt: Arc<AtomicBool>,
    pub deadline: Instant,
}

/// Start `job` on a thread of its own.
pub fn start(job: Job) -> std::io::Result<Running> {
    let (to_engine, rx) = channel();
    let (tx, from_engine) = channel();
    let abort = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_secs(job.env.max_seconds.max(1));
    let flag = abort.clone();
    let halt = job.env.stop.clone();
    std::thread::Builder::new()
        .name("script".into())
        // Deeply nested script calls recurse in the interpreter.
        .stack_size(32 << 20)
        .spawn(move || {
            let done = to_engine.clone();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                api::run(job, to_engine, from_engine, flag, deadline)
            }))
            .unwrap_or_else(|panic| {
                let what = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                Outcome {
                    error: Some(format!("the script failed unexpectedly ({what})")),
                    ..Default::default()
                }
            });
            let _ = done.send(Msg::Done(outcome));
        })?;
    Ok(Running {
        rx,
        tx,
        abort,
        halt,
        deadline,
    })
}

/// Whether `code` is a script that parses: `Err` says where it doesn't.
pub fn check(code: &str) -> Result<(), String> {
    let engine = api::bare_engine();
    let scope = api::base_scope(Value::Null, Value::Null);
    engine
        .compile_with_scope(&scope, code)
        .map(|_| ())
        .map_err(|e| describe_parse(&e, code))
}

/// "Syntax error: ... (line 3, position 9)" and the line itself.
fn describe_parse(e: &rhai::ParseError, code: &str) -> String {
    let mut msg = format!("Syntax error: {e}");
    if let Some(line) = e.position().line() {
        push_line(&mut msg, code, line);
    }
    msg
}

/// Longest error message kept (a script may throw any text).
const MAX_ERROR: usize = 8_000;

/// A runtime error, with the line of `code` (whose source name is
/// `source`) where it happened.
fn describe_error(e: &EvalAltResult, code: &str, source: Option<&str>) -> String {
    let mut cur = e;
    let mut elsewhere = false;
    loop {
        match cur {
            EvalAltResult::ErrorInFunctionCall(_, src, inner, _) => {
                elsewhere |= !src.is_empty() && Some(src.as_str()) != source;
                cur = inner;
            }
            EvalAltResult::ErrorInModule(_, inner, _) => {
                elsewhere = true;
                cur = inner;
            }
            _ => break,
        }
    }
    let mut msg = e.to_string();
    // Cut by characters (not bytes), so a message that is short enough but
    // not ASCII is kept whole.
    if let Some((end, _)) = msg.char_indices().nth(MAX_ERROR) {
        let all = msg.chars().count();
        msg.truncate(end);
        msg.push_str(&format!("… ({all} characters in all)"));
    }
    if matches!(cur, EvalAltResult::ErrorFunctionNotFound(..)) {
        msg.push_str(". script(help=true) lists every function; maths on whole numbers works, and sin(1) is sin(1.0)");
    }
    if !elsewhere && let Some(line) = cur.position().line() {
        push_line(&mut msg, code, line);
    }
    msg
}

fn push_line(msg: &mut String, code: &str, line: usize) {
    if let Some(text) = code.lines().nth(line.saturating_sub(1)) {
        let text: String = text.trim_end().chars().take(160).collect();
        msg.push_str(&format!("\n  {line} | {text}"));
    }
}

#[cfg(test)]
mod tests;
