//! Typed decisions from a fast **decision model**: questions that want a
//! yes/no (a probability), one of some options, or a score on a scale,
//! asked about a *state* (some text, or an app's window as text).
//!
//! Two kinds of API are spoken:
//!
//! * `jev`: TypeSafe's System One API (`POST /v1/systemone`), made for
//!   exactly this: Jev answers in well under a second with calibrated
//!   probabilities and writes no text. Servers that speak the same API
//!   (local-jev, jeff, LiteLLM's pass-through…) work too.
//! * `openai`: any OpenAI-compatible chat API (`POST /chat/completions`):
//!   a fast chat model is asked for a JSON object with the answers.
//!
//! Requests go through `curl` (part of Windows 10 and later, macOS and most
//! Linux systems), with everything it is given — the address, the key and
//! the request — on its input, so the API key never shows in the list of
//! running programs. The user sets the model up on a page in their browser
//! ([`page`], `Ctrl+Alt+J`), so the key never goes through the chat.

pub mod page;

use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::config::DecisionConfig;
use crate::error::{Error, Result};

/// The most questions in one request (System One's limit).
pub const MAX_QUESTIONS: usize = 64;
/// The most options (or levels) of one question.
pub const MAX_OPTIONS: usize = 64;

/// Each question's name and its answer, in the questions' order.
pub type Answers = Vec<(String, Answer)>;
/// What asking once gives: the answers and how long they took.
pub type Asked = Result<(Answers, Duration)>;

/// What a question wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The probability that the answer is yes.
    YesNo,
    /// One of the options.
    Choice,
    /// A level on a scale (low to high), as a number.
    Score,
}

impl Kind {
    fn wire(self) -> &'static str {
        match self {
            Kind::YesNo => "noul",
            Kind::Choice => "choice",
            Kind::Score => "score",
        }
    }

    /// From what people (and models) call it.
    pub fn parse(s: &str) -> Option<Self> {
        Some(
            match s
                .trim()
                .to_ascii_lowercase()
                .replace(['-', ' '], "_")
                .as_str()
            {
                "yes_no" | "yesno" | "noul" | "bool" | "boolean" | "yes/no" => Kind::YesNo,
                "choice" | "choose" | "one_of" | "pick" | "category" | "classify" => Kind::Choice,
                "score" | "scale" | "rate" | "rating" | "level" => Kind::Score,
                _ => return None,
            },
        )
    }
}

/// One question.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    /// Its name in the answers ("answer" for a single question).
    pub name: String,
    pub kind: Kind,
    /// The question, or a statement to judge.
    pub text: String,
    /// Choice: the options, each with what it means ("" = its label says
    /// it). Score: the levels, low to high. Yes/no: nothing, or what a
    /// yes and a no mean (`("true", …)`, `("false", …)`).
    pub options: Vec<(String, String)>,
}

impl Question {
    pub fn yes_no(name: &str, text: &str) -> Self {
        Self {
            name: name.into(),
            kind: Kind::YesNo,
            text: text.into(),
            options: Vec::new(),
        }
    }

    pub fn choice(name: &str, text: &str, options: &[&str]) -> Self {
        Self {
            name: name.into(),
            kind: Kind::Choice,
            text: text.into(),
            options: options
                .iter()
                .map(|o| (o.to_string(), String::new()))
                .collect(),
        }
    }

    pub fn score(name: &str, text: &str, levels: &[&str]) -> Self {
        Self {
            name: name.into(),
            kind: Kind::Score,
            text: text.into(),
            options: levels
                .iter()
                .map(|o| (o.to_string(), String::new()))
                .collect(),
        }
    }

    fn check(&self) -> Result<()> {
        let bad = |why: String| {
            Err(Error::InvalidArgs(format!(
                "question \"{}\": {why}",
                self.name
            )))
        };
        if self.name.trim().is_empty() {
            return Err(Error::InvalidArgs("a question needs a name".into()));
        }
        if self.text.trim().is_empty() {
            return bad("it has no question text".into());
        }
        match self.kind {
            Kind::Choice | Kind::Score if self.options.len() < 2 => bad(format!(
                "a {} needs at least two {}",
                if self.kind == Kind::Choice {
                    "choice"
                } else {
                    "score"
                },
                if self.kind == Kind::Choice {
                    "options"
                } else {
                    "levels"
                }
            )),
            _ if self.options.len() > MAX_OPTIONS => {
                bad(format!("at most {MAX_OPTIONS} options or levels"))
            }
            Kind::Choice | Kind::Score => {
                let mut seen = std::collections::HashSet::new();
                for (label, _) in &self.options {
                    if label.trim().is_empty() {
                        return bad("an option is empty".into());
                    }
                    if !seen.insert(label.trim().to_lowercase()) {
                        return bad(format!("\"{label}\" is given twice"));
                    }
                }
                Ok(())
            }
            Kind::YesNo => Ok(()),
        }
    }
}

/// One answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The probability that the answer is yes.
    YesNo { yes: f64 },
    Choice {
        choice: String,
        confidence: Option<f64>,
        /// Each option's probability, most likely first.
        probabilities: Vec<(String, f64)>,
    },
    Score {
        /// 0 = the first (lowest) level; may be between levels.
        score: f64,
        /// The level nearest the score.
        level: String,
        confidence: Option<f64>,
    },
}

impl Answer {
    /// Short, for the model: `yes (0.91)`, `billing (0.88; next technical
    /// 0.12)`, `1.2 → Real problem (0.55)`.
    pub fn brief(&self) -> String {
        match self {
            Answer::YesNo { yes } => {
                format!("{} ({yes:.2})", if *yes >= 0.5 { "yes" } else { "no" })
            }
            Answer::Choice {
                choice,
                confidence,
                probabilities,
            } => {
                let p = probabilities
                    .iter()
                    .find(|(o, _)| o == choice)
                    .map(|(_, p)| *p)
                    .or(*confidence);
                let mut s = match p {
                    Some(p) => format!("{choice} ({p:.2}"),
                    None => format!("{choice} ("),
                };
                // A close second is worth knowing.
                if let Some((o, p)) = probabilities.iter().find(|(o, _)| o != choice)
                    && *p >= 0.15
                {
                    if !s.ends_with('(') {
                        s.push_str("; ");
                    }
                    s.push_str(&format!("next {o} {p:.2}"));
                }
                if s.ends_with('(') {
                    s.truncate(s.len() - 2);
                } else {
                    s.push(')');
                }
                s
            }
            Answer::Score {
                score,
                level,
                confidence,
            } => match confidence {
                Some(c) => format!("{score:.2} → {level} (confidence {c:.2})"),
                None => format!("{score:.2} → {level}"),
            },
        }
    }

    /// Yes (at least 0.5) for a yes/no answer.
    pub fn is_yes(&self) -> bool {
        matches!(self, Answer::YesNo { yes } if *yes >= 0.5)
    }

    /// As JSON (scripts).
    pub fn to_json(&self) -> Value {
        match self {
            Answer::YesNo { yes } => json!({"type": "yes_no", "yes": yes, "answer": *yes >= 0.5}),
            Answer::Choice {
                choice,
                confidence,
                probabilities,
            } => {
                let probs: Map<String, Value> = probabilities
                    .iter()
                    .map(|(o, p)| (o.clone(), json!(p)))
                    .collect();
                json!({"type": "choice", "answer": choice, "confidence": confidence, "probabilities": probs})
            }
            Answer::Score {
                score,
                level,
                confidence,
            } => {
                json!({"type": "score", "answer": score, "level": level, "confidence": confidence})
            }
        }
    }
}

/// Which kind of API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Jev,
    OpenAi,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "jev" | "typesafe" | "systemone" | "system_one" => Some(Provider::Jev),
            "openai" | "openai-compatible" | "chat" => Some(Provider::OpenAi),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Provider::Jev => "jev",
            Provider::OpenAi => "openai",
        }
    }

    pub fn default_base(self) -> &'static str {
        match self {
            Provider::Jev => "https://api.typesafe.ai",
            Provider::OpenAi => "https://api.openai.com/v1",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Jev => "jev-latest",
            Provider::OpenAi => "",
        }
    }
}

/// A decision model, ready to ask.
#[derive(Clone)]
pub struct Decider {
    provider: Provider,
    url: String,
    model: String,
    key: String,
    timeout: Duration,
    max_state: usize,
    /// Requests at once when several states are judged.
    pub parallel: usize,
}

impl std::fmt::Debug for Decider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the key.
        f.debug_struct("Decider")
            .field("provider", &self.provider)
            .field("url", &self.url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// The default settings key as this system names it.
pub const SETTINGS_KEY: &str = if cfg!(target_os = "macos") {
    "Ctrl+Option+J"
} else {
    "Ctrl+Alt+J"
};

/// How to reach the decision model when none is set up.
pub const NOT_SET_UP: &str = "no decision model is set up. Ask the user to press Ctrl+Alt+J: a page opens in their browser where they add one (TypeSafe's Jev, or any OpenAI-compatible model) and its API key, which then never goes through the chat. Or decide setup=\"open\" opens that page for them.";

impl Decider {
    /// The decision model the settings name (`None` when there is none).
    pub fn from_config(c: &DecisionConfig) -> Result<Option<Self>> {
        if c.provider.trim().is_empty() {
            return Ok(None);
        }
        let provider = Provider::parse(&c.provider).ok_or_else(|| {
            Error::Config(format!(
                "decision.provider must be \"jev\" or \"openai\" (got \"{}\")",
                c.provider
            ))
        })?;
        let base = if c.base_url.trim().is_empty() {
            provider.default_base().to_string()
        } else {
            c.base_url.trim().to_string()
        };
        let model = if c.model.trim().is_empty() {
            provider.default_model().to_string()
        } else {
            c.model.trim().to_string()
        };
        if model.is_empty() {
            return Err(Error::Config(
                "the decision model has no model name (decision.model): an OpenAI-compatible API needs one".into(),
            ));
        }
        let mut key = c.api_key.trim().to_string();
        let env = c.api_key_env.trim();
        if key.is_empty() && !env.is_empty() {
            key = std::env::var(env).unwrap_or_default().trim().to_string();
            if key.is_empty() {
                return Err(Error::Config(format!(
                    "the decision model's key is to come from the environment variable {env}, which is not set for the server"
                )));
            }
        }
        if key.chars().any(char::is_control) {
            return Err(Error::Config(
                "the decision model's API key must be one line".into(),
            ));
        }
        Ok(Some(Self {
            provider,
            url: endpoint(provider, &base),
            model,
            key,
            timeout: Duration::from_millis(c.timeout_ms.clamp(100, 600_000)),
            max_state: c.max_state_chars.clamp(100, 1_000_000),
            parallel: c.parallel.clamp(1, 64),
        }))
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// `jev-latest at api.typesafe.ai`.
    pub fn label(&self) -> String {
        let host = self
            .url
            .split("://")
            .nth(1)
            .unwrap_or(&self.url)
            .split('/')
            .next()
            .unwrap_or_default();
        format!("{} at {host}", self.model)
    }

    pub fn has_key(&self) -> bool {
        !self.key.is_empty()
    }

    /// Ask `questions` about `state`. The answers in the questions' order,
    /// and how long it took. `halted` ends the wait early (the stop key).
    pub fn ask(
        &self,
        state: &str,
        questions: &[Question],
        halted: &(dyn Fn() -> bool + Sync),
    ) -> Asked {
        if questions.is_empty() {
            return Err(Error::InvalidArgs("no question to ask".into()));
        }
        if questions.len() > MAX_QUESTIONS {
            return Err(Error::InvalidArgs(format!(
                "at most {MAX_QUESTIONS} questions at once"
            )));
        }
        let mut names = std::collections::HashSet::new();
        for q in questions {
            q.check()?;
            if !names.insert(q.name.as_str()) {
                return Err(Error::InvalidArgs(format!(
                    "two questions are named \"{}\"",
                    q.name
                )));
            }
        }
        let state = clip(state, self.max_state);
        let start = Instant::now();
        let answers = match self.provider {
            Provider::Jev => {
                let body = systemone_body(&self.model, &state, questions);
                let (code, text) = self.post(&body, halted)?;
                check_status(code, &text, self)?;
                let v: Value = serde_json::from_str(&text).map_err(|_| {
                    Error::ActionFailed(format!(
                        "the decision model's answer isn't JSON: {}",
                        excerpt(&text)
                    ))
                })?;
                parse_systemone(&v, questions)?
            }
            Provider::OpenAi => {
                let mut body = chat_body(&self.model, &state, questions, true);
                let (mut code, mut text) = self.post(&body, halted)?;
                // Some models take no temperature (reasoning models): ask
                // again without one.
                if code == 400 && text.contains("temperature") {
                    body = chat_body(&self.model, &state, questions, false);
                    (code, text) = self.post(&body, halted)?;
                }
                check_status(code, &text, self)?;
                let v: Value = serde_json::from_str(&text).map_err(|_| {
                    Error::ActionFailed(format!(
                        "the decision model's answer isn't JSON: {}",
                        excerpt(&text)
                    ))
                })?;
                parse_chat(&v, questions)?
            }
        };
        Ok((answers, start.elapsed()))
    }

    /// The same questions about each of `states`, several at once
    /// (`parallel`). Each state's answers (or what went wrong) in order.
    pub fn ask_each(
        &self,
        states: &[String],
        questions: &[Question],
        halted: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Asked> {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = AtomicUsize::new(0);
        let results: Vec<Mutex<Option<Asked>>> = states.iter().map(|_| Mutex::new(None)).collect();
        let workers = self.parallel.min(states.len()).max(1);
        std::thread::scope(|s| {
            for _ in 0..workers {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        if i >= states.len() {
                            break;
                        }
                        let r = if halted() {
                            Err(Error::ActionFailed("stopped".into()))
                        } else {
                            self.ask(&states[i], questions, halted)
                        };
                        if let Ok(mut slot) = results[i].lock() {
                            *slot = Some(r);
                        }
                    }
                });
            }
        });
        results
            .into_iter()
            .map(|m| {
                m.into_inner()
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| Err(Error::Internal("no answer".into())))
            })
            .collect()
    }

    /// POST `body` (JSON); the HTTP status and the reply.
    fn post(&self, body: &Value, halted: &(dyn Fn() -> bool + Sync)) -> Result<(u16, String)> {
        let mut config = String::new();
        config.push_str(&format!("url = {}\n", quote(&self.url)));
        if !self.key.is_empty() {
            config.push_str(&format!(
                "header = {}\n",
                quote(&format!("Authorization: Bearer {}", self.key))
            ));
        }
        config.push_str("header = \"Content-Type: application/json\"\n");
        config.push_str("header = \"Accept: application/json\"\n");
        config.push_str(&format!(
            "user-agent = {}\n",
            quote(concat!(
                "computer-use/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/mhrsdev/zero-use-computer)"
            ))
        ));
        config.push_str(&format!("data-binary = {}\n", quote(&body.to_string())));
        curl(&config, self.timeout, halted)
    }
}

/// The request's address: the API's base with the endpoint added (unless
/// it is there already).
fn endpoint(provider: Provider, base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    match provider {
        Provider::Jev => {
            if base.ends_with("/systemone") {
                base.to_string()
            } else if base.ends_with("/v1") {
                format!("{base}/systemone")
            } else {
                format!("{base}/v1/systemone")
            }
        }
        Provider::OpenAi => {
            if base.ends_with("/chat/completions") {
                base.to_string()
            } else {
                format!("{base}/chat/completions")
            }
        }
    }
}

/// A state longer than `max` characters: its start and its end.
fn clip(state: &str, max: usize) -> String {
    let n = state.chars().count();
    if n <= max {
        return state.to_string();
    }
    let head = max * 2 / 3;
    let tail = max.saturating_sub(head + 3);
    let start: String = state.chars().take(head).collect();
    let end: String = state.chars().skip(n - tail).collect();
    format!("{start}\n…\n{end}")
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
            '\u{b}' => out.push_str("\\v"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The most of a reply read.
const MAX_REPLY: u64 = 4 * 1024 * 1024;

/// Run curl with `config` on its input; the HTTP status and the reply.
fn curl(
    config: &str,
    timeout: Duration,
    halted: &(dyn Fn() -> bool + Sync),
) -> Result<(u16, String)> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--proto", "=http,https", "--max-redirs", "0"])
        .args(["--max-time", &format!("{:.1}", timeout.as_secs_f64())])
        .args(["-w", "\n%{http_code}", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        Error::ActionFailed(if e.kind() == std::io::ErrorKind::NotFound {
            "the decision model is reached with curl (part of Windows 10 and later, macOS and most Linux systems), and it isn't installed here".into()
        } else {
            format!("can't run curl: {e}")
        })
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let data = config.as_bytes().to_vec();
        // From a thread: curl reads it all before it starts, but a big
        // request must not fill the pipe while this thread waits.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let stdout = child.stdout.take().map(|out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.take(MAX_REPLY).read_to_end(&mut buf);
            buf
        })
    });
    let stderr = child.stderr.take().map(|err| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.take(64 * 1024).read_to_string(&mut s);
            s
        })
    });
    let status = loop {
        if halted() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::ActionFailed(
                "stopped while waiting for the decision model".into(),
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::ActionFailed(format!("curl failed: {e}")));
            }
        }
    };
    let mut out = stdout.and_then(|t| t.join().ok()).unwrap_or_default();
    let err = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
    let cut = out.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let code: u16 = String::from_utf8_lossy(&out[cut..])
        .trim()
        .parse()
        .unwrap_or(0);
    out.truncate(cut);
    if code == 0 {
        let err = err.trim().trim_start_matches("curl: ").to_string();
        return Err(Error::ActionFailed(match status.code() {
            Some(28) => format!(
                "the decision model didn't answer within {:.0} s (decision.timeout_ms)",
                timeout.as_secs_f64()
            ),
            _ if err.is_empty() => format!(
                "couldn't reach the decision model (curl exit {})",
                status.code().unwrap_or(-1)
            ),
            _ => format!("couldn't reach the decision model: {err}"),
        }));
    }
    let text = String::from_utf8(out)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    Ok((code, text))
}

fn excerpt(text: &str) -> String {
    let t: String = text.trim().chars().take(300).collect();
    if t.is_empty() { "(nothing)".into() } else { t }
}

/// An HTTP error as the model (and the user) can act on it.
fn check_status(code: u16, text: &str, d: &Decider) -> Result<()> {
    if (200..300).contains(&code) {
        return Ok(());
    }
    // The API's own message, when it sends one.
    let detail = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            let e = v.get("error").unwrap_or(&v);
            e.get("message")
                .or_else(|| e.get("detail"))
                .or_else(|| v.get("detail"))
                .map(|m| match m {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
        })
        .unwrap_or_else(|| excerpt(text));
    let detail: String = detail.chars().take(400).collect();
    Err(Error::ActionFailed(match code {
        401 | 403 => format!(
            "the decision model ({}) refused the API key (HTTP {code}: {detail}). The user can fix it on the settings page ({SETTINGS_KEY}).",
            d.label()
        ),
        404 => format!(
            "the decision model ({}) wasn't found (HTTP 404: {detail}): the address or the model name is wrong. The user can fix it on the settings page ({SETTINGS_KEY}).",
            d.label()
        ),
        422 | 400 => format!("the decision model rejected the request (HTTP {code}): {detail}"),
        429 => format!(
            "the decision model is limiting requests (HTTP 429: {detail}); wait a moment, or ask fewer questions at once"
        ),
        _ => format!("the decision model failed (HTTP {code}): {detail}"),
    }))
}

// -- System One (Jev) ---------------------------------------------------------

fn systemone_body(model: &str, state: &str, questions: &[Question]) -> Value {
    let mut qs = Map::new();
    for q in questions {
        let mut o = Map::new();
        o.insert("type".into(), json!(q.kind.wire()));
        o.insert("instructions".into(), json!(q.text));
        match q.kind {
            Kind::YesNo => {
                let get = |k: &str| {
                    q.options
                        .iter()
                        .find(|(l, _)| l.eq_ignore_ascii_case(k))
                        .map(|(_, d)| d.clone())
                };
                if let (Some(t), Some(f)) = (get("true"), get("false")) {
                    o.insert("criteria".into(), json!({"true": t, "false": f}));
                }
            }
            Kind::Choice => {
                let c: Map<String, Value> = q
                    .options
                    .iter()
                    .map(|(l, d)| {
                        let d = if d.trim().is_empty() { l } else { d };
                        (l.clone(), json!(d))
                    })
                    .collect();
                o.insert("criteria".into(), Value::Object(c));
            }
            Kind::Score => {
                let levels: Vec<Value> = q
                    .options
                    .iter()
                    .map(|(l, d)| {
                        if d.trim().is_empty() {
                            json!(l)
                        } else {
                            json!(format!("{l}: {d}"))
                        }
                    })
                    .collect();
                o.insert("criteria".into(), Value::Array(levels));
            }
        }
        qs.insert(q.name.clone(), Value::Object(o));
    }
    json!({"model": model, "state": state, "questions": qs})
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
    .filter(|f: &f64| f.is_finite())
}

fn parse_systemone(v: &Value, questions: &[Question]) -> Result<Answers> {
    let answers = v.get("answers").and_then(Value::as_object).ok_or_else(|| {
        Error::ActionFailed(format!(
            "the decision model's reply has no answers: {}",
            excerpt(&v.to_string())
        ))
    })?;
    let mut out = Vec::with_capacity(questions.len());
    for q in questions {
        let a = answers.get(&q.name).ok_or_else(|| {
            Error::ActionFailed(format!("the decision model didn't answer \"{}\"", q.name))
        })?;
        let answer = match q.kind {
            Kind::YesNo => {
                let yes = a
                    .get("noul")
                    .and_then(num)
                    .or_else(|| num(a))
                    .ok_or_else(|| bad_answer(&q.name, a))?;
                Answer::YesNo {
                    yes: yes.clamp(0.0, 1.0),
                }
            }
            Kind::Choice => {
                let choice = a
                    .get("choice")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad_answer(&q.name, a))?
                    .to_string();
                let mut probabilities = probs(a.get("probabilities"));
                // Its own options' labels (an option may come back as the
                // label it was sent with).
                let choice = q
                    .options
                    .iter()
                    .find(|(l, _)| l.eq_ignore_ascii_case(&choice))
                    .map(|(l, _)| l.clone())
                    .unwrap_or(choice);
                for (o, _) in &mut probabilities {
                    if let Some((l, _)) = q.options.iter().find(|(l, _)| l.eq_ignore_ascii_case(o))
                    {
                        *o = l.clone();
                    }
                }
                Answer::Choice {
                    choice,
                    confidence: a.get("confidence").and_then(num),
                    probabilities,
                }
            }
            Kind::Score => {
                let score = a
                    .get("score")
                    .and_then(num)
                    .ok_or_else(|| bad_answer(&q.name, a))?;
                score_answer(q, score, a.get("confidence").and_then(num))
            }
        };
        out.push((q.name.clone(), answer));
    }
    Ok(out)
}

fn bad_answer(name: &str, a: &Value) -> Error {
    Error::ActionFailed(format!(
        "the decision model's answer to \"{name}\" makes no sense: {}",
        excerpt(&a.to_string())
    ))
}

/// `{"billing": 0.88, ...}`, most likely first.
fn probs(v: Option<&Value>) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = v
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| num(v).map(|p| (k.clone(), p)))
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

fn score_answer(q: &Question, score: f64, confidence: Option<f64>) -> Answer {
    let top = q.options.len().saturating_sub(1) as f64;
    let score = score.clamp(0.0, top);
    let level = q
        .options
        .get(score.round() as usize)
        .map(|(l, _)| l.clone())
        .unwrap_or_default();
    Answer::Score {
        score,
        level,
        confidence,
    }
}

// -- OpenAI-compatible chat ---------------------------------------------------

const CHAT_SYSTEM: &str = "You are a decision model. You read a state and answer typed questions about it. Reply with one JSON object and nothing else: each question's name mapped to its answer. A yes/no question's answer is the probability (a number from 0 to 1) that the answer is yes. A choice question's answer is exactly one of its options, as written. A score question's answer is the number of the level that fits (0 = the first, lowest level; a fraction between two levels is allowed).";

fn chat_body(model: &str, state: &str, questions: &[Question], temperature: bool) -> Value {
    let mut prompt = String::new();
    prompt.push_str("State:\n\"\"\"\n");
    prompt.push_str(state);
    prompt.push_str("\n\"\"\"\n\nQuestions:\n");
    let mut example = Map::new();
    for q in questions {
        let name = serde_json::to_string(&q.name).unwrap_or_default();
        match q.kind {
            Kind::YesNo => {
                prompt.push_str(&format!("- {name} (yes/no): {}\n", q.text));
                for (l, d) in &q.options {
                    let side = if l.eq_ignore_ascii_case("true") {
                        "yes"
                    } else {
                        "no"
                    };
                    prompt.push_str(&format!("  {side} means: {d}\n"));
                }
                example.insert(q.name.clone(), json!(0.0));
            }
            Kind::Choice => {
                prompt.push_str(&format!("- {name} (choice): {}\n  options:\n", q.text));
                for (l, d) in &q.options {
                    let l = serde_json::to_string(l).unwrap_or_default();
                    if d.trim().is_empty() {
                        prompt.push_str(&format!("  {l}\n"));
                    } else {
                        prompt.push_str(&format!("  {l}: {d}\n"));
                    }
                }
                example.insert(q.name.clone(), json!("<one option>"));
            }
            Kind::Score => {
                prompt.push_str(&format!("- {name} (score): {}\n  levels:\n", q.text));
                for (i, (l, d)) in q.options.iter().enumerate() {
                    if d.trim().is_empty() {
                        prompt.push_str(&format!("  {i} = {l}\n"));
                    } else {
                        prompt.push_str(&format!("  {i} = {l}: {d}\n"));
                    }
                }
                example.insert(q.name.clone(), json!(0));
            }
        }
    }
    prompt.push_str(&format!(
        "\nReply with JSON shaped like {}",
        Value::Object(example)
    ));
    let mut body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": CHAT_SYSTEM},
            {"role": "user", "content": prompt}
        ],
        "stream": false
    });
    if temperature {
        body["temperature"] = json!(0);
    }
    body
}

/// The first `{…}` in a reply (models wrap JSON in prose or fences).
fn json_in(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start)
        .then(|| serde_json::from_str(&text[start..=end]).ok())
        .flatten()
}

fn parse_chat(v: &Value, questions: &[Question]) -> Result<Answers> {
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            Error::ActionFailed(format!(
                "the decision model's reply has no message: {}",
                excerpt(&v.to_string())
            ))
        })?;
    let answers = json_in(content)
        .and_then(|v| v.as_object().cloned())
        .ok_or_else(|| {
            Error::ActionFailed(format!(
                "the decision model didn't answer with JSON: {}",
                excerpt(content)
            ))
        })?;
    let mut out = Vec::with_capacity(questions.len());
    for q in questions {
        let a = answers
            .get(&q.name)
            .or_else(|| {
                // A name in another case.
                answers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&q.name))
                    .map(|(_, v)| v)
            })
            .ok_or_else(|| {
                Error::ActionFailed(format!("the decision model didn't answer \"{}\"", q.name))
            })?;
        let answer = match q.kind {
            Kind::YesNo => {
                let yes = match a {
                    Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                        "yes" | "true" | "y" => Some(1.0),
                        "no" | "false" | "n" => Some(0.0),
                        other => other.parse().ok(),
                    },
                    other => num(other),
                }
                .ok_or_else(|| bad_answer(&q.name, a))?;
                Answer::YesNo {
                    yes: yes.clamp(0.0, 1.0),
                }
            }
            Kind::Choice => {
                let pick = match a {
                    Value::String(s) => option_named(q, s),
                    Value::Number(n) => n
                        .as_u64()
                        .and_then(|i| q.options.get(i as usize))
                        .map(|(l, _)| l.clone()),
                    _ => None,
                }
                .ok_or_else(|| {
                    Error::ActionFailed(format!(
                        "the decision model answered \"{}\" with {}, which is none of its options",
                        q.name,
                        excerpt(&a.to_string())
                    ))
                })?;
                Answer::Choice {
                    choice: pick,
                    confidence: None,
                    probabilities: Vec::new(),
                }
            }
            Kind::Score => {
                let score = match a {
                    Value::String(s) => option_named(q, s)
                        .and_then(|l| q.options.iter().position(|(o, _)| *o == l))
                        .map(|i| i as f64)
                        .or_else(|| s.trim().parse().ok()),
                    other => num(other),
                }
                .ok_or_else(|| bad_answer(&q.name, a))?;
                score_answer(q, score, None)
            }
        };
        out.push((q.name.clone(), answer));
    }
    Ok(out)
}

/// The option a model's answer names (exactly, in another case, or as
/// part of a longer answer).
fn option_named(q: &Question, said: &str) -> Option<String> {
    let said = said.trim().trim_matches(['"', '\'', '.']);
    if let Some((l, _)) = q.options.iter().find(|(l, _)| l == said) {
        return Some(l.clone());
    }
    if let Some((l, _)) = q.options.iter().find(|(l, _)| l.eq_ignore_ascii_case(said)) {
        return Some(l.clone());
    }
    let low = said.to_lowercase();
    let found: Vec<&String> = q
        .options
        .iter()
        .map(|(l, _)| l)
        .filter(|l| low.contains(&l.to_lowercase()))
        .collect();
    match found.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

// -- questions from a tool call -----------------------------------------------

/// Options as given: `["a", "b"]` or `{"a": "what a means", ...}`.
pub fn options_from(v: &Value) -> Result<Vec<(String, String)>> {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|i| match i {
                Value::String(s) => Ok((s.trim().to_string(), String::new())),
                Value::Number(n) => Ok((n.to_string(), String::new())),
                other => Err(Error::InvalidArgs(format!(
                    "an option must be text (got {other})"
                ))),
            })
            .collect(),
        Value::Object(m) => Ok(m
            .iter()
            .map(|(k, v)| {
                let d = match v {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                (k.trim().to_string(), d)
            })
            .collect()),
        other => Err(Error::InvalidArgs(format!(
            "options must be a list of labels or an object of label: meaning (got {other})"
        ))),
    }
}

/// One question from `{"type", "question" (or "instructions"), "options"
/// (or "criteria"), "scale", "yes", "no"}`.
pub fn question_from(name: &str, v: &Value) -> Result<Question> {
    let text = v
        .get("question")
        .or_else(|| v.get("instructions"))
        .or_else(|| v.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let options = v.get("options").or_else(|| v.get("criteria"));
    let scale = v.get("scale").or_else(|| v.get("levels"));
    let kind = match v.get("type").and_then(Value::as_str) {
        Some(t) => Kind::parse(t).ok_or_else(|| {
            Error::InvalidArgs(format!(
                "question \"{name}\": type must be yes_no, choice or score (got \"{t}\")"
            ))
        })?,
        None if scale.is_some() => Kind::Score,
        None if options.is_some_and(|o| {
            o.is_array() || o.as_object().is_some_and(|m| !m.contains_key("true"))
        }) =>
        {
            Kind::Choice
        }
        None => Kind::YesNo,
    };
    let options = match kind {
        Kind::YesNo => {
            let mut o = Vec::new();
            let crit = options.and_then(Value::as_object);
            let side = |k: &str, alt: &str| {
                crit.and_then(|c| c.get(k))
                    .or_else(|| v.get(alt))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            if let (Some(t), Some(f)) = (side("true", "yes"), side("false", "no")) {
                o.push(("true".to_string(), t));
                o.push(("false".to_string(), f));
            }
            o
        }
        Kind::Choice => options_from(options.ok_or_else(|| {
            Error::InvalidArgs(format!("question \"{name}\" (choice) needs options"))
        })?)?,
        Kind::Score => options_from(scale.or(options).ok_or_else(|| {
            Error::InvalidArgs(format!(
                "question \"{name}\" (score) needs a scale: its levels, low to high"
            ))
        })?)?,
    };
    let q = Question {
        name: name.to_string(),
        kind,
        text,
        options,
    };
    q.check()?;
    Ok(q)
}

/// Questions from `{"name": {...}, ...}`.
pub fn questions_from(v: &Value) -> Result<Vec<Question>> {
    let m = v.as_object().ok_or_else(|| {
        Error::InvalidArgs(
            "questions must be an object: {\"name\": {\"type\": \"yes_no\"|\"choice\"|\"score\", \"question\": \"…\", \"options\": [...], \"scale\": [...]}}".into(),
        )
    })?;
    if m.is_empty() {
        return Err(Error::InvalidArgs("questions is empty".into()));
    }
    m.iter().map(|(k, v)| question_from(k, v)).collect()
}

#[cfg(test)]
pub(crate) mod tests;
