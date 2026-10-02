//! The agent benchmark: real tasks in real apps, measured by what they
//! cost to finish.
//!
//! Each run starts the scenario's app afresh (bench/fixtures/bench_app.py),
//! gives an agent the task, and checks the app's own record of what was done
//! at the end: success is never taken from what the model says.
//!
//! * **Real** (the default): Claude through the Messages API, with the
//!   server's tool definitions and the computer-use skills as its system
//!   prompt. Records the tokens the API reports for every request: input,
//!   output, cache reads and cache writes, and which model served it.
//! * **Scripted** (`--scripted`): a fixed way through each task, as a careful
//!   agent would call the tools, with no model and no key. Its token figures
//!   are *estimates* (text ≈ 4 characters a token, an image width × height
//!   / 750) and are reported as such.
//!
//! The decision model is not used: it is optional, and a run measures the
//! tools alone (a `--config` file can still set one up).
//!
//! ```text
//! bench/run.sh --scripted --label v3.2
//! ANTHROPIC_API_KEY=… bench/run.sh --runs 5 --label v3.2
//! ```
//!
//! See bench/README.md for every option.

mod api;
mod remote;
mod report;
mod scenarios;
mod session;

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};

use crate::report::RunResult;
use crate::scenarios::Scenario;
use crate::session::Session;

struct Options {
    scenarios: Vec<&'static Scenario>,
    runs: usize,
    scripted: bool,
    model: String,
    effort: Option<String>,
    max_turns: usize,
    config: Option<PathBuf>,
    preset: String,
    /// Scripted runs: "step" (one action a call) or "batch" (v3.6).
    plan: String,
    label: String,
    out: PathBuf,
    python: Option<String>,
    calibrate: bool,
    verbose: bool,
    /// A server binary of any version, spoken to over MCP, instead of the
    /// engine built in (`--server`), with its arguments (`--server-args`).
    server: Option<String>,
    server_args: Vec<String>,
    server_config: Option<PathBuf>,
    /// The skills folder the model gets (`--skills`): the version's own.
    skills: Option<PathBuf>,
}

const USAGE: &str =
    "usage: agent_bench [--scripted] [--scenarios form,table,board,shapes,orders,long|all]
                   [--runs N] [--model ID] [--effort low|medium|high|xhigh|max]
                   [--max-turns N] [--config FILE] [--preset default|codex]
                   [--plan step|batch] [--label NAME] [--out DIR] [--python PY] [--calibrate] [--verbose]
                   [--server BIN [--server-args \"ARGS\"] [--server-config FILE] [--skills DIR]]";

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        scenarios: scenarios::ALL.iter().collect(),
        runs: 1,
        scripted: false,
        model: "claude-opus-5-5".into(),
        effort: Some("medium".into()),
        max_turns: 40,
        config: None,
        preset: "default".into(),
        plan: "step".into(),
        label: "dev".into(),
        out: PathBuf::from("target/bench"),
        python: None,
        calibrate: false,
        verbose: false,
        server: None,
        server_args: Vec::new(),
        server_config: None,
        skills: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--scripted" => o.scripted = true,
            "--calibrate" => o.calibrate = true,
            "--verbose" | "-v" => o.verbose = true,
            "--scenarios" => {
                let v = value("--scenarios")?;
                if v != "all" {
                    o.scenarios = v
                        .split(',')
                        .map(|id| {
                            scenarios::ALL
                                .iter()
                                .find(|s| s.id == id.trim())
                                .ok_or(format!("unknown scenario {id}"))
                        })
                        .collect::<Result<_, _>>()?;
                }
            }
            "--runs" => o.runs = value("--runs")?.parse().map_err(|_| "--runs: a number")?,
            "--max-turns" => {
                o.max_turns = value("--max-turns")?
                    .parse()
                    .map_err(|_| "--max-turns: a number")?
            }
            "--model" => o.model = value("--model")?,
            "--effort" => {
                let e = value("--effort")?;
                o.effort = (e != "default").then_some(e);
            }
            "--config" => o.config = Some(PathBuf::from(value("--config")?)),
            "--preset" => o.preset = value("--preset")?,
            "--plan" => {
                o.plan = value("--plan")?;
                if o.plan != "step" && o.plan != "batch" {
                    return Err("--plan: step or batch".into());
                }
            }
            "--label" => o.label = value("--label")?,
            "--out" => o.out = PathBuf::from(value("--out")?),
            "--python" => o.python = Some(value("--python")?),
            "--server" => o.server = Some(value("--server")?),
            "--server-args" => {
                o.server_args = value("--server-args")?
                    .split_whitespace()
                    .map(String::from)
                    .collect()
            }
            "--skills" => o.skills = Some(PathBuf::from(value("--skills")?)),
            "--server-config" => o.server_config = Some(PathBuf::from(value("--server-config")?)),
            "--help" | "-h" => return Err(USAGE.into()),
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
    }
    Ok(o)
}

/// What the benchmark tells the model, before the instructions and skills.
const FRAMING: &str = "You operate a Linux desktop with the computer-use tools. Do the user's task, then reply with one short line saying what you did, without calling more tools. Nobody can answer questions: the task is exactly what the user asked for.";

/// A skill without its front matter.
fn skill_body(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    match text
        .strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
    {
        Some((_, body)) => body.trim().to_string(),
        None => text.trim().to_string(),
    }
}

/// The skills a client that loaded them gives the model: `computer-use`
/// and `computer-use-security` (an early release's single `SKILL.md` when
/// the folder has that instead).
fn skills_text(dir: Option<&std::path::Path>) -> String {
    let Some(dir) = dir else {
        return format!(
            "{}\n\n{}",
            skill_body(include_str!("../../../../skills/computer-use/SKILL.md")),
            skill_body(include_str!(
                "../../../../skills/computer-use-security/SKILL.md"
            )),
        );
    };
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().map(|t| skill_body(&t));
    let layered: Vec<String> = ["computer-use", "computer-use-security"]
        .iter()
        .filter_map(|s| read(dir.join(s).join("SKILL.md")))
        .collect();
    if layered.is_empty() {
        read(dir.join("SKILL.md")).unwrap_or_default()
    } else {
        layered.join("\n\n")
    }
}

/// The system prompt: the framing, the server's MCP instructions (as a
/// client puts them in front of the model) and the skills.
fn system_prompt(instructions: &str, skills: &str) -> String {
    [FRAMING, instructions, skills]
        .iter()
        .filter(|t| !t.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn main() {
    let o = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let Some(python) = session::find_python(o.python.as_deref()) else {
        eprintln!(
            "no Python with GTK 3 (python3-gi, gir1.2-gtk-3.0) found; give one with --python"
        );
        std::process::exit(2);
    };
    let fixture = std::env::var_os("BENCH_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/fixtures/bench_app.py")
        });
    let client = if o.scripted {
        None
    } else {
        match api::Client::from_env(&o.model, o.effort.clone()) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    };
    let work = std::env::temp_dir().join(format!("agent-bench-{}", std::process::id()));
    let _ = std::fs::create_dir_all(work.join("scripts"));
    let cfg = match session::config(o.config.as_deref(), &o.preset, &work.join("scripts")) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let skills = skills_text(o.skills.as_deref());
    let server = o.server.as_ref().map(|program| remote::Launch {
        program: program.clone(),
        args: o.server_args.clone(),
        home: work.join("server"),
        config: o.server_config.clone(),
    });
    let mode = if o.scripted { "scripted" } else { "real" };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dir = o.out.join(&o.label);
    let _ = std::fs::create_dir_all(&dir);
    let jsonl = dir.join(format!("{mode}-{stamp}.jsonl"));
    println!(
        "agent bench: {mode}{}, label {}, preset {}, {} scenario(s) x {} run(s)\nresults: {}\n",
        client
            .as_ref()
            .map(|c| format!(
                " ({}, effort {})",
                c.model,
                c.effort.as_deref().unwrap_or("default")
            ))
            .unwrap_or_else(|| format!(" (plan {})", o.plan)),
        o.label,
        o.preset,
        o.scenarios.len(),
        o.runs,
        jsonl.display()
    );

    let mut results: Vec<RunResult> = Vec::new();
    for sc in &o.scenarios {
        for run in 1..=o.runs {
            let state = work.join(format!("{}-{run}.json", sc.id));
            let mut session = match Session::start(
                &python,
                &fixture,
                sc.fixture,
                sc.app,
                cfg.clone(),
                server.as_ref(),
                state,
            ) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("{} #{run}: {e}", sc.id);
                    continue;
                }
            };
            session.verbose = o.verbose;
            let defs = session.engine.tool_definitions();
            let tools: Vec<Value> = defs
                .iter()
                .map(|d| json!({"name": d.name, "description": d.description, "input_schema": d.input_schema}))
                .collect();
            let instructions = session.engine.instructions();
            let system = system_prompt(&instructions, &skills);
            let est = computer_use::text::estimate_tokens;
            let tools_tokens = est(&Value::Array(tools.clone()).to_string());
            let fixed_tokens = est(&system) + tools_tokens;
            let t0 = Instant::now();
            let mut r = RunResult::new(&o.label, mode, sc.id, run, &o.preset);
            r.fixed_tokens_est = fixed_tokens;
            r.prefix = json!({
                "framing": est(FRAMING),
                "instructions": est(&instructions),
                "skills": est(&skills),
                "tools": tools_tokens,
                "tool_count": tools.len(),
            });
            match &client {
                None => {
                    let way = if o.plan == "batch" {
                        sc.batched
                    } else {
                        sc.scripted
                    };
                    let outcome = way(&mut session);
                    r.stop = match outcome {
                        Ok(()) => "done".into(),
                        Err(e) => format!("script: {e}"),
                    };
                    // One model turn per call.
                    r.turns = session.calls.len();
                }
                Some(c) => {
                    run_model(c, &system, &tools, sc, &mut session, &mut r, o.max_turns);
                    r.model = Some(c.model.clone());
                    r.effort = c.effort.clone();
                    if o.calibrate {
                        report::calibrate(c, &mut session);
                    }
                }
            }
            // Give the app a moment to write what the last action did.
            std::thread::sleep(std::time::Duration::from_millis(300));
            r.seconds = t0.elapsed().as_secs_f64();
            let state = session.state().unwrap_or(Value::Null);
            match (sc.check)(&state) {
                Ok(()) => r.success = true,
                Err(e) => r.check = e,
            }
            r.take_calls(&session.calls);
            println!("{}", r.line());
            report::append_jsonl(&jsonl, &r.to_json());
            results.push(r);
        }
    }
    let summary = report::summary(&results, &o.scenarios);
    println!("\n{summary}");
    let md = dir.join(format!("{mode}-{stamp}.md"));
    let _ = std::fs::write(&md, &summary);
    println!("summary: {}", md.display());
    let _ = std::fs::remove_dir_all(&work);
}

/// The agent loop: ask, run the tools it calls, hand back what they
/// returned, until it stops calling tools.
#[allow(clippy::too_many_arguments)]
fn run_model(
    client: &api::Client,
    system: &str,
    tools: &[Value],
    sc: &Scenario,
    session: &mut Session,
    r: &mut RunResult,
    max_turns: usize,
) {
    let mut messages = vec![json!({"role": "user", "content": sc.task})];
    r.stop = "max turns".into();
    for _ in 0..max_turns {
        let reply = match client.create(system, tools, &messages) {
            Ok(v) => v,
            Err(e) => {
                r.stop = format!("api error: {e}");
                return;
            }
        };
        r.turns += 1;
        let usage = api::Usage::of(&reply["usage"]);
        r.add_usage(usage);
        if session.verbose {
            for b in reply["content"].as_array().into_iter().flatten() {
                if b["type"] == "text" {
                    eprintln!("model: {}", b["text"].as_str().unwrap_or(""));
                }
            }
        }
        if let Some(m) = reply["model"].as_str()
            && !r.served.iter().any(|s| s == m)
        {
            r.served.push(m.to_string());
        }
        let content = reply["content"].clone();
        if content
            .as_array()
            .is_some_and(|a| a.iter().any(|b| b["type"] == "fallback"))
        {
            r.fallback = true;
        }
        messages.push(json!({"role": "assistant", "content": content}));
        match reply["stop_reason"].as_str() {
            Some("refusal") => {
                r.stop = "refusal".into();
                return;
            }
            Some("max_tokens") => {
                r.stop = "max_tokens".into();
                return;
            }
            _ => {}
        }
        let uses: Vec<&Value> = content
            .as_array()
            .map(|a| a.iter().filter(|b| b["type"] == "tool_use").collect())
            .unwrap_or_default();
        if uses.is_empty() {
            r.stop = "done".into();
            r.final_text = content
                .as_array()
                .and_then(|a| a.iter().rev().find(|b| b["type"] == "text"))
                .and_then(|b| b["text"].as_str())
                .unwrap_or("")
                .chars()
                .take(300)
                .collect();
            return;
        }
        let mut results = Vec::new();
        for u in uses {
            let name = u["name"].as_str().unwrap_or("");
            let out = session.call_raw(name, u["input"].clone());
            let mut blocks = vec![json!({"type": "text", "text": out.text})];
            if let Some(img) = &out.image {
                blocks.push(json!({
                    "type": "image",
                    "source": {"type": "base64", "media_type": img.mime, "data": img.base64()},
                }));
            }
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": u["id"],
                "content": blocks,
                "is_error": out.is_error,
            }));
        }
        messages.push(json!({"role": "user", "content": results}));
    }
}
