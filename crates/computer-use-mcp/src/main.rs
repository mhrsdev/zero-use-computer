//! `computer-use-mcp` — an MCP stdio server (and small CLI) that gives any
//! MCP-capable agent the Codex-style computer-use tools.

#[cfg(feature = "http")]
mod http;
mod jsonrpc;
mod server;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use computer_use::config::{ApprovalMode, ConfigStore};
use computer_use::engine::{AllowApprover, Engine};
use computer_use::{Backend, tools};
use serde_json::{Value, json};

use server::{HeadlessApproval, Server};

#[derive(Parser)]
#[command(
    name = "computer-use-mcp",
    version,
    about = "Codex-style computer use over MCP: control desktop apps via their accessibility tree + screenshots."
)]
struct Cli {
    #[command(flatten)]
    common: Common,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Args, Clone)]
struct Common {
    /// Path to config.toml (defaults to $COMPUTER_USE_HOME/config.toml or ~/.computer-use/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Approval policy for controlling apps.
    #[arg(long, global = true, value_enum)]
    approval: Option<ApprovalArg>,

    /// Pre-approve an app for this run (repeatable): name, id or pid.
    #[arg(long = "allow", global = true)]
    allow: Vec<String>,

    /// What to do when approval is needed but the client can't be asked.
    #[arg(long, global = true, value_enum, default_value_t = HeadlessArg::Deny)]
    headless_approve: HeadlessArg,

    /// Serve MCP over HTTP on this address (e.g. 127.0.0.1:8787) instead of
    /// stdio. Requires the `http` build feature. Approvals are non-interactive.
    #[arg(long, global = true)]
    http: Option<String>,

    /// Bearer token required on the HTTP endpoint (or $COMPUTER_USE_HTTP_TOKEN).
    #[arg(long, global = true)]
    http_token: Option<String>,

    /// Log level (error, warn, info, debug, trace). Logs go to stderr.
    #[arg(long, global = true, default_value = "warn")]
    log: String,
}

#[derive(Copy, Clone, ValueEnum)]
enum ApprovalArg {
    Prompt,
    Allowlist,
    AllowAll,
}

impl From<ApprovalArg> for ApprovalMode {
    fn from(a: ApprovalArg) -> Self {
        match a {
            ApprovalArg::Prompt => ApprovalMode::Prompt,
            ApprovalArg::Allowlist => ApprovalMode::Allowlist,
            ApprovalArg::AllowAll => ApprovalMode::AllowAll,
        }
    }
}

#[derive(Copy, Clone, ValueEnum)]
enum HeadlessArg {
    Deny,
    Allow,
}

impl From<HeadlessArg> for HeadlessApproval {
    fn from(a: HeadlessArg) -> Self {
        match a {
            HeadlessArg::Deny => HeadlessApproval::Deny,
            HeadlessArg::Allow => HeadlessApproval::Allow,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP stdio server (default).
    Serve,
    /// List running apps.
    Apps,
    /// Print an app's accessibility tree (and optionally save its screenshot).
    State {
        /// App name, id or pid.
        app: String,
        /// Window id or title substring.
        #[arg(long)]
        window: Option<String>,
        /// Save the screenshot to this file (PNG/JPEG per config).
        #[arg(long)]
        screenshot: Option<PathBuf>,
    },
    /// Run a single tool with JSON arguments.
    Call {
        /// Tool name (e.g. click, set_value).
        tool: String,
        /// Arguments as a JSON object.
        #[arg(default_value = "{}")]
        args: String,
    },
    /// Print the tool definitions as JSON.
    Tools,
    /// Report platform, permissions and config.
    Doctor,
}

fn build_engine(common: &Common) -> Result<Engine<Box<dyn Backend>>> {
    let mut store =
        ConfigStore::load(common.config.as_deref()).with_context(|| "loading configuration")?;
    if let Some(mode) = common.approval {
        store.config.approvals.mode = mode.into();
    }
    let backend = computer_use::platform_backend().with_context(|| {
        format!(
            "initializing the {} computer-use backend",
            computer_use::PLATFORM
        )
    })?;
    let mut engine = Engine::new(backend, store);
    for app in &common.allow {
        engine.allow_for_session(app);
    }
    Ok(engine)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&cli.common.log))
        .target(env_logger::Target::Stderr)
        .format_timestamp_millis()
        .init();

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(&cli.common),
        Command::Apps => run_and_print(&cli.common, "list_apps", json!({})),
        Command::State {
            app,
            window,
            screenshot,
        } => state(&cli.common, &app, window, screenshot),
        Command::Call { tool, args } => {
            let args: Value =
                serde_json::from_str(&args).with_context(|| "parsing --args as JSON")?;
            run_and_print(&cli.common, &tool, args)
        }
        Command::Tools => {
            let defs = tools::definitions();
            let json: Vec<Value> = defs
                .iter()
                .map(|d| {
                    json!({
                        "name": d.name,
                        "description": d.description,
                        "inputSchema": d.input_schema,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json)?);
            Ok(())
        }
        Command::Doctor => doctor(&cli.common),
    }
}

fn serve(common: &Common) -> Result<()> {
    let engine = build_engine(common)?;

    if let Some(addr) = &common.http {
        return serve_http(common, engine, addr);
    }

    log::info!(
        "computer-use-mcp {} serving on stdio ({} backend)",
        env!("CARGO_PKG_VERSION"),
        computer_use::PLATFORM
    );
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = Server::new(
        engine,
        stdin.lock(),
        stdout.lock(),
        common.headless_approve.into(),
    );
    server.run().context("serving MCP over stdio")
}

#[cfg(feature = "http")]
fn serve_http(common: &Common, engine: Engine<Box<dyn Backend>>, addr: &str) -> Result<()> {
    let token = common
        .http_token
        .clone()
        .or_else(|| std::env::var("COMPUTER_USE_HTTP_TOKEN").ok())
        .filter(|t| !t.is_empty());
    let allow = matches!(common.headless_approve, HeadlessArg::Allow);
    http::serve(engine, addr, token, allow)
}

#[cfg(not(feature = "http"))]
fn serve_http(_common: &Common, _engine: Engine<Box<dyn Backend>>, _addr: &str) -> Result<()> {
    anyhow::bail!("this build has no HTTP support; rebuild with `--features http`")
}

fn run_and_print(common: &Common, tool: &str, args: Value) -> Result<()> {
    let mut engine = build_engine(common)?;
    let out = engine.call_tool(tool, args, &mut AllowApprover);
    if let Some(img) = &out.image {
        eprintln!("[screenshot: {} {}x{}]", img.mime, img.width, img.height);
    }
    println!("{}", out.text);
    if out.is_error {
        std::process::exit(1);
    }
    Ok(())
}

fn state(
    common: &Common,
    app: &str,
    window: Option<String>,
    screenshot: Option<PathBuf>,
) -> Result<()> {
    let mut engine = build_engine(common)?;
    let mut args = serde_json::Map::new();
    args.insert("app".into(), json!(app));
    if let Some(w) = window {
        args.insert("window".into(), json!(w));
    }
    args.insert("disable_diff".into(), json!(true));
    let out = engine.call_tool("get_app_state", Value::Object(args), &mut AllowApprover);
    println!("{}", out.text);
    if let (Some(path), Some(img)) = (screenshot, &out.image) {
        let mut f =
            std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        f.write_all(&img.data)?;
        eprintln!("saved screenshot to {}", path.display());
    }
    if out.is_error {
        std::process::exit(1);
    }
    Ok(())
}

fn doctor(common: &Common) -> Result<()> {
    println!("computer-use-mcp {}", env!("CARGO_PKG_VERSION"));
    println!("platform: {}", computer_use::PLATFORM);
    let cfg_path = common
        .config
        .clone()
        .unwrap_or_else(computer_use::config::default_config_path);
    println!("config:   {}", cfg_path.display());
    println!(
        "managed:  {}",
        computer_use::config::managed_config_path().display()
    );

    match build_engine(common) {
        Ok(mut engine) => {
            println!("backend:  ok");
            println!("permissions:");
            for p in engine.permissions() {
                let mark = if p.granted { "✓" } else { "✗" };
                println!("  {mark} {} — {}", p.name, p.detail);
            }
            match engine.call_tool("list_apps", json!({}), &mut AllowApprover) {
                out if !out.is_error => {
                    let n = out.text.lines().count().saturating_sub(1);
                    println!("apps:     {n} visible");
                }
                out => println!("apps:     error: {}", out.text),
            }
        }
        Err(e) => println!("backend:  ERROR: {e:#}"),
    }
    Ok(())
}
