//! What a run measured, one JSON line per run, and the summary table.

use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

use crate::api::{Client, Prices, Usage};
use crate::scenarios::Scenario;
use crate::session::{CallRecord, Session};

#[derive(Debug, Default)]
pub struct RunResult {
    pub label: String,
    pub mode: String,
    pub scenario: String,
    pub run: usize,
    pub preset: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Models that served the run's requests (more than one: a fallback).
    pub served: Vec<String>,
    pub fallback: bool,
    pub success: bool,
    /// Why the check failed.
    pub check: String,
    /// How the agent stopped: done, max turns, refusal, an API error…
    pub stop: String,
    pub final_text: String,
    pub turns: usize,
    pub seconds: f64,
    /// Real tokens over every request (real runs).
    pub usage: Usage,
    /// Real prompt tokens of each request.
    pub prompt_per_turn: Vec<u64>,
    /// Estimated tokens of the system prompt and tool definitions.
    pub fixed_tokens_est: usize,
    pub calls: Vec<CallRecord>,
}

impl RunResult {
    pub fn new(label: &str, mode: &str, scenario: &str, run: usize, preset: &str) -> Self {
        Self {
            label: label.into(),
            mode: mode.into(),
            scenario: scenario.into(),
            run,
            preset: preset.into(),
            ..Self::default()
        }
    }

    pub fn add_usage(&mut self, u: Usage) {
        self.usage.add(u);
        self.prompt_per_turn.push(u.prompt());
    }

    pub fn take_calls(&mut self, calls: &[CallRecord]) {
        self.calls = calls.to_vec();
    }

    fn sum(&self, f: impl Fn(&CallRecord) -> usize) -> usize {
        self.calls.iter().map(f).sum()
    }

    /// Estimated tokens of everything the tools returned.
    pub fn results_est(&self) -> usize {
        self.sum(|c| c.text_tokens + c.image_tokens)
    }

    pub fn images(&self) -> usize {
        self.calls.iter().filter(|c| c.image.is_some()).count()
    }

    /// Estimated input over the run without a prompt cache: every turn
    /// reads the fixed prefix and the conversation so far (each call's
    /// arguments and result), one call per turn.
    pub fn input_est(&self) -> usize {
        let mut history = 0;
        let mut total = 0;
        for c in &self.calls {
            total += self.fixed_tokens_est + history;
            history += c.args_tokens + c.text_tokens + c.image_tokens;
        }
        // The turn that ends the task reads it all once more.
        total + self.fixed_tokens_est + history
    }

    pub fn cost(&self) -> Option<f64> {
        let model = self.model.as_deref()?;
        Prices::for_model(model).map(|p| p.cost(&self.usage))
    }

    /// One line for the console.
    pub fn line(&self) -> String {
        let tokens = if self.mode == "real" {
            format!(
                "prompt {} (cached {}), output {}",
                self.usage.prompt(),
                self.usage.cache_read,
                self.usage.output
            )
        } else {
            format!(
                "results ~{} tok, input ~{} tok (est.)",
                self.results_est(),
                self.input_est()
            )
        };
        format!(
            "{:<7} #{:<2} {:<4} {:>2} turns {:>3} calls {:>2} images  {tokens}  {:.1}s  [{}]{}",
            self.scenario,
            self.run,
            if self.success { "ok" } else { "FAIL" },
            self.turns,
            self.calls.len(),
            self.images(),
            self.seconds,
            self.stop,
            if self.success {
                String::new()
            } else {
                format!(" {}", self.check)
            }
        )
    }

    pub fn to_json(&self) -> Value {
        let real: Vec<u64> = self.calls.iter().filter_map(|c| c.real_tokens).collect();
        json!({
            "label": self.label,
            "mode": self.mode,
            "scenario": self.scenario,
            "run": self.run,
            "preset": self.preset,
            "model": self.model,
            "effort": self.effort,
            "served": self.served,
            "fallback": self.fallback,
            "success": self.success,
            "check": self.check,
            "stop": self.stop,
            "final_text": self.final_text,
            "turns": self.turns,
            "calls": self.calls.len(),
            "errors": self.calls.iter().filter(|c| c.error).count(),
            "images": self.images(),
            "seconds": (self.seconds * 100.0).round() / 100.0,
            "usage": {
                "input": self.usage.input,
                "output": self.usage.output,
                "cache_read": self.usage.cache_read,
                "cache_creation": self.usage.cache_creation,
            },
            "prompt_per_turn": self.prompt_per_turn,
            "cost_usd": self.cost(),
            "estimated": {
                "fixed": self.fixed_tokens_est,
                "results": self.results_est(),
                "result_text": self.sum(|c| c.text_tokens),
                "result_images": self.sum(|c| c.image_tokens),
                "args": self.sum(|c| c.args_tokens),
                "input_no_cache": self.input_est(),
            },
            "calibration": (!real.is_empty()).then(|| json!({
                "estimated": self.calls.iter().filter(|c| c.real_tokens.is_some()).map(|c| c.text_tokens + c.image_tokens).sum::<usize>(),
                "real": real.iter().sum::<u64>(),
            })),
            "per_call": self.calls.iter().map(CallRecord::to_json).collect::<Vec<_>>(),
        })
    }
}

pub fn append_jsonl(path: &Path, v: &Value) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{v}");
    }
}

/// Median, min and max of some numbers, as "median (min–max)".
fn spread(mut v: Vec<f64>, digits: usize) -> String {
    if v.is_empty() {
        return "–".into();
    }
    v.sort_by(f64::total_cmp);
    let med = if v.len() % 2 == 1 {
        v[v.len() / 2]
    } else {
        (v[v.len() / 2 - 1] + v[v.len() / 2]) / 2.0
    };
    let f = |x: f64| format!("{x:.digits$}");
    if v.len() == 1 {
        f(med)
    } else {
        format!("{} ({}–{})", f(med), f(v[0]), f(v[v.len() - 1]))
    }
}

/// The summary table, in Markdown: one row per scenario, median (min–max)
/// over its runs.
pub fn summary(results: &[RunResult], scenarios: &[&Scenario]) -> String {
    let Some(first) = results.first() else {
        return "No runs finished.\n".into();
    };
    let real = first.mode == "real";
    let mut out = String::new();
    out.push_str(&format!(
        "## {} — {} runs{}\n\n",
        first.label,
        first.mode,
        first
            .model
            .as_ref()
            .map(|m| format!(
                ", {m}, effort {}",
                first.effort.as_deref().unwrap_or("default")
            ))
            .unwrap_or_default()
    ));
    if !real {
        out.push_str("Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.\n\n");
    }
    if real {
        out.push_str("| scenario | success | prompt tokens | of which cached | output tokens | cost $ | turns | calls | images | seconds |\n|---|---|---|---|---|---|---|---|---|---|\n");
    } else {
        out.push_str("| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |\n|---|---|---|---|---|---|---|---|\n");
    }
    for sc in scenarios {
        let rs: Vec<&RunResult> = results.iter().filter(|r| r.scenario == sc.id).collect();
        if rs.is_empty() {
            continue;
        }
        let ok = rs.iter().filter(|r| r.success).count();
        let col =
            |f: &dyn Fn(&RunResult) -> f64, d: usize| spread(rs.iter().map(|r| f(r)).collect(), d);
        if real {
            out.push_str(&format!(
                "| {} | {ok}/{} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                sc.id,
                rs.len(),
                col(&|r| r.usage.prompt() as f64, 0),
                col(&|r| r.usage.cache_read as f64, 0),
                col(&|r| r.usage.output as f64, 0),
                if rs.iter().all(|r| r.cost().is_some()) {
                    col(&|r| r.cost().unwrap_or(0.0), 3)
                } else {
                    "–".into()
                },
                col(&|r| r.turns as f64, 0),
                col(&|r| r.calls.len() as f64, 0),
                col(&|r| r.images() as f64, 0),
                col(&|r| r.seconds, 1),
            ));
        } else {
            out.push_str(&format!(
                "| {} | {ok}/{} | {} | {} | {} | {} | {} | {} |\n",
                sc.id,
                rs.len(),
                col(&|r| r.results_est() as f64, 0),
                col(&|r| r.input_est() as f64, 0),
                col(&|r| r.calls.len() as f64, 0),
                col(&|r| r.images() as f64, 0),
                col(&|r| r.sum(|c| c.image_tokens) as f64, 0),
                col(&|r| r.seconds, 1),
            ));
        }
    }
    out.push('\n');
    for sc in scenarios {
        out.push_str(&format!("- `{}`: {}\n", sc.id, sc.about));
    }
    out.push_str(&format!(
        "\nFixed prefix (system prompt + tool definitions): ~{} tokens (est.).\n",
        first.fixed_tokens_est
    ));
    if results.iter().any(|r| r.fallback) {
        out.push_str(
            "Some runs were served partly by a fallback model (see `served` in the JSON lines).\n",
        );
    }
    let (est, real_tok): (usize, u64) = results
        .iter()
        .flat_map(|r| r.calls.iter())
        .filter_map(|c| c.real_tokens.map(|t| (c.text_tokens + c.image_tokens, t)))
        .fold((0, 0), |(a, b), (e, t)| (a + e, b + t));
    if real_tok > 0 {
        out.push_str(&format!(
            "Calibration: the tools' results were estimated at {est} tokens and counted at {real_tok} ({:.2}x).\n",
            real_tok as f64 / est.max(1) as f64
        ));
    }
    out
}

/// Count every result's real tokens with the token-counting endpoint, to
/// compare with the estimate the server's budgets use.
pub fn calibrate(client: &Client, session: &mut Session) {
    let Ok(base) = client.count(&json!([{"type": "text", "text": "."}])) else {
        return;
    };
    for (rec, out) in session.calls.iter_mut().zip(&session.outputs) {
        let mut blocks = vec![json!({"type": "text", "text": out.text})];
        if let Some(img) = &out.image {
            blocks.push(json!({
                "type": "image",
                "source": {"type": "base64", "media_type": img.mime, "data": img.base64()},
            }));
        }
        if let Ok(n) = client.count(&Value::Array(blocks)) {
            rec.real_tokens = Some(n.saturating_sub(base));
        }
    }
}
