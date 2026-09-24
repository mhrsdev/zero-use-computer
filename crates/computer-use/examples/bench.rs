//! Benchmark the real platform backend: speed, token cost and peak memory.
//!
//! ```text
//! cargo run --release -p computer-use --example bench -- <app> [window] [iterations]
//! ```
//!
//! Reports, per operation: wall time (mean / min), backend IPC round trips
//! (Linux), output size in characters and estimated tokens, screenshot size and
//! estimated image tokens, plus the size of the tool definitions and the
//! process's peak resident memory.

use std::time::{Duration, Instant};

use computer_use::config::{ApprovalMode, Config, ConfigStore};
use computer_use::engine::{AllowApprover, Engine};
use computer_use::tools;
use serde_json::json;

fn ipc() -> u64 {
    #[cfg(target_os = "linux")]
    {
        computer_use::linux::ipc_calls()
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// Peak resident set size in KiB (Linux), if available.
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

/// Rough text token estimate (~4 chars per token).
fn text_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

/// Anthropic's image token estimate: width * height / 750.
fn image_tokens(w: u32, h: u32) -> u64 {
    (u64::from(w) * u64::from(h)).div_ceil(750)
}

struct Sample {
    time: Duration,
    ipc: u64,
    chars: usize,
    image: Option<(u32, u32, usize)>,
}

fn run(
    engine: &mut Engine<Box<dyn computer_use::Backend>>,
    tool: &str,
    args: serde_json::Value,
) -> Sample {
    let ipc0 = ipc();
    let t0 = Instant::now();
    let out = engine.call_tool(tool, args, &mut AllowApprover);
    let time = t0.elapsed();
    if out.is_error {
        eprintln!("  ! {tool} error: {}", out.text);
    }
    Sample {
        time,
        ipc: ipc() - ipc0,
        chars: out.text.chars().count(),
        image: out.image.map(|i| (i.width, i.height, i.data.len())),
    }
}

fn report(name: &str, samples: &[Sample]) {
    let n = samples.len().max(1) as u32;
    let total: Duration = samples.iter().map(|s| s.time).sum();
    let min = samples.iter().map(|s| s.time).min().unwrap_or_default();
    let last = samples.last().unwrap();
    let mut line = format!(
        "{name:<26} mean {:>7.1} ms  min {:>7.1} ms  ipc {:>5}  text {:>6} chars (~{:>5} tok)",
        (total / n).as_secs_f64() * 1000.0,
        min.as_secs_f64() * 1000.0,
        last.ipc,
        last.chars,
        text_tokens(last.chars),
    );
    if let Some((w, h, bytes)) = last.image {
        line.push_str(&format!(
            "  img {w}x{h} {:.0} KiB (~{} tok)",
            bytes as f64 / 1024.0,
            image_tokens(w, h)
        ));
    }
    println!("{line}");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let app = args.next().unwrap_or_else(|| {
        eprintln!("usage: bench <app> [window] [iterations]");
        std::process::exit(2);
    });
    let window = args.next().filter(|w| !w.is_empty() && w != "-");
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);

    let config_path = std::env::var_os("BENCH_CONFIG");
    let mut store = match &config_path {
        Some(p) => ConfigStore::load(Some(std::path::Path::new(p))).expect("config"),
        None => ConfigStore::in_memory(Config::default()),
    };
    store.config.approvals.mode = ApprovalMode::AllowAll;
    store.config.audit.enabled = false;
    let backend = computer_use::platform_backend().expect("backend");
    let mut engine = Engine::new(backend, store);

    let win_arg = |extra: serde_json::Value| {
        let mut v = json!({"app": app});
        if let Some(w) = &window {
            v["window"] = json!(w);
        }
        if let serde_json::Value::Object(m) = extra {
            for (k, val) in m {
                v[k] = val;
            }
        }
        v
    };

    println!("benchmark: app={app} window={window:?} iterations={iters}\n");

    let full = tools::model_visible_len(&tools::definitions());
    let compact = tools::model_visible_len(&tools::definitions_for(&engine.store().config.tools));
    println!(
        "tool definitions (sent with every model request): full {} chars (~{} tok), configured {} chars (~{} tok)\n",
        full,
        text_tokens(full),
        compact,
        text_tokens(compact)
    );

    let s: Vec<Sample> = (0..iters)
        .map(|_| run(&mut engine, "list_apps", json!({})))
        .collect();
    report("list_apps", &s);

    let s: Vec<Sample> = (0..iters)
        .map(|_| {
            run(
                &mut engine,
                "get_app_state",
                win_arg(json!({"disable_diff": true})),
            )
        })
        .collect();
    report("get_app_state (full)", &s);

    let s: Vec<Sample> = (0..iters)
        .map(|_| run(&mut engine, "get_app_state", win_arg(json!({}))))
        .collect();
    report("get_app_state (diff)", &s);

    let s: Vec<Sample> = (0..iters)
        .map(|_| {
            run(
                &mut engine,
                "find_element",
                win_arg(json!({"role": "button"})),
            )
        })
        .collect();
    report("find_element(button)", &s);

    let s: Vec<Sample> = (0..iters)
        .map(|_| run(&mut engine, "screenshot", json!({"mode": "full"})))
        .collect();
    report("screenshot (full)", &s);

    if let Some(kib) = peak_rss_kib() {
        println!("\npeak RSS: {:.1} MiB", kib as f64 / 1024.0);
    }
}
