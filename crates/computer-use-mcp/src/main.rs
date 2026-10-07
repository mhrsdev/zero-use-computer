//! `computer-use-mcp` — an MCP stdio server (and small CLI) that gives any
//! MCP-capable agent the Codex-style computer-use tools.

mod catalog;
mod core;
#[cfg(feature = "http")]
mod http;
mod jsonrpc;
mod server;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use computer_use::config::{self, Config, ConfigStore, Edit};
use computer_use::engine::Engine;
use computer_use::{Backend, tools};
use serde_json::{Value, json};

use server::Server;

#[derive(Parser)]
#[command(
    name = "computer-use-mcp",
    version,
    about = "Codex-style computer use over MCP: control desktop apps via their accessibility tree + screenshots.",
    after_help = "computer-use by mhrsdev: https://github.com/mhrsdev/zero-use-computer"
)]
struct Cli {
    #[command(flatten)]
    common: Common,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Command-line overrides. Anything not given here comes from config.toml.
#[derive(Args, Clone)]
struct Common {
    /// Path to config.toml (defaults to $COMPUTER_USE_HOME/config.toml or ~/.computer-use/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Override server.http_addr: serve MCP over HTTP on this address
    /// (e.g. 127.0.0.1:8787). Requires the `http` build feature.
    #[arg(long, global = true)]
    http: Option<String>,

    /// Override server.http_token. Other users of this computer can read a
    /// command line (`ps`): prefer $COMPUTER_USE_HTTP_TOKEN or the settings
    /// file.
    #[arg(long, global = true)]
    http_token: Option<String>,

    /// Override server.log (error, warn, info, debug, trace). Logs go to stderr.
    #[arg(long, global = true)]
    log: Option<String>,

    /// Override text_only = true (never send screenshots).
    #[arg(long, global = true)]
    text_only: bool,

    /// Override server.instructions: full, short (the client loads the
    /// skills, which say the rest) or off.
    #[arg(long, global = true, value_parser = ["full", "short", "off"])]
    instructions: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server (default): stdio, or HTTP when server.http_addr is set.
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
    /// Print the tool definitions the model will see (per your settings).
    Tools {
        /// Print all tools with full descriptions, ignoring [tools] settings.
        #[arg(long)]
        all: bool,
    },
    /// View and change settings.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Report platform, permissions and config.
    Doctor,
    /// Look for a newer release now and download it (it goes in as
    /// `update.install` says, after the computer restarts by default).
    Update {
        /// Only say whether there is a newer release.
        #[arg(long)]
        check: bool,
        /// Put a downloaded update in place now (download it first if
        /// needed); start your MCP client again to use it.
        #[arg(long)]
        install: bool,
        /// Go back to the version the last update replaced, and don't take
        /// that update again (sets update.skip_version).
        #[arg(long)]
        rollback: bool,
    },
    /// Add this program to the agents you use (Claude Code, Claude Desktop,
    /// Codex, Cursor, VS Code), or take it out. Only its own entry is
    /// touched; the settings file of each is copied to `.bak` first.
    Install {
        /// Which agent: claude-code, claude-desktop, codex, cursor or
        /// vscode. Repeat for several. Without it: every one found here.
        #[arg(long = "client", value_name = "NAME")]
        clients: Vec<String>,
        /// Take this program out of them instead.
        #[arg(long)]
        remove: bool,
        /// Only say which agents are here, whether the program is in each,
        /// and what would be written.
        #[arg(long)]
        list: bool,
    },
    /// Put a shortcut that opens the settings panel on the desktop (and,
    /// with --menu or --both, with the system's apps), or take it away.
    Shortcut {
        /// With the system's apps (Start menu, ~/Applications, the app
        /// menu) instead of the desktop.
        #[arg(long)]
        menu: bool,
        /// On the desktop and with the apps.
        #[arg(long)]
        both: bool,
        /// Take it away instead.
        #[arg(long)]
        remove: bool,
        /// Only say where they are.
        #[arg(long)]
        list: bool,
    },
    /// Open the settings panel in your browser (what Ctrl+Alt+J does), and
    /// serve it until you press Done.
    Settings {
        /// Print the page's address instead of opening a browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// (internal) The on-screen overlay helper the server starts. `--demo`
    /// shows every state once, to check how it looks on this machine.
    #[command(hide = true)]
    Overlay {
        #[arg(long)]
        parent: Option<u32>,
        #[arg(long)]
        demo: bool,
    },
    /// (internal) The hub every server on this desktop shares: one overlay
    /// with a cursor per agent, one stop key, turns at the keyboard and
    /// mouse. The first server to start runs it.
    #[command(hide = true)]
    Hub {
        #[arg(long)]
        port: u16,
        #[arg(long)]
        home: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the config file path.
    Path,
    /// Write a documented config file listing every option with its default.
    Init {
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Print the effective settings (your file merged over the defaults).
    Show {
        /// Print the built-in defaults instead.
        #[arg(long)]
        defaults: bool,
    },
    /// List every setting key.
    Keys,
    /// Print one setting, e.g. `config get screenshot.attach`.
    Get { key: String },
    /// Change a setting, e.g. `config set screenshot.attach always`.
    Set { key: String, value: String },
    /// Remove a setting from your file so it returns to its default.
    Unset { key: String },
    /// Add an item to a list setting, e.g. `config add tools.disabled drag`.
    Add { key: String, item: String },
    /// Remove an item from a list setting.
    Remove { key: String, item: String },
    /// Validate the file and report unknown (misspelled) keys.
    Check,
}

fn config_path(common: &Common) -> PathBuf {
    common
        .config
        .clone()
        .unwrap_or_else(config::default_config_path)
}

/// Apply command-line overrides on top of the file settings.
fn apply_overrides(common: &Common) -> impl Fn(&mut Config) + Send + 'static {
    let http = common.http.clone();
    let http_token = common.http_token.clone();
    let log = common.log.clone();
    let text_only = common.text_only;
    let instructions = common.instructions.as_deref().map(|i| match i {
        "short" => computer_use::config::Instructions::Short,
        "off" => computer_use::config::Instructions::Off,
        _ => computer_use::config::Instructions::Full,
    });
    move |c: &mut Config| {
        if let Some(i) = instructions {
            c.server.instructions = i;
        }
        if let Some(a) = &http {
            c.server.http_addr = a.clone();
        }
        if let Some(t) = &http_token {
            c.server.http_token = t.clone();
        }
        if let Some(l) = &log {
            c.server.log = l.clone();
        }
        if text_only {
            c.text_only = true;
        }
        // Over HTTP the server tells only the clients that keep an event
        // stream open (GET) that its tool list changed, and not every
        // client does: found tools are run through use_tool instead.
        if !c.server.http_addr.is_empty()
            && c.tools.manager == computer_use::config::ToolManager::ListChanged
        {
            c.tools.manager = computer_use::config::ToolManager::Dispatch;
        }
    }
}

/// The tools a model is served with these settings: what `serve` lists
/// before any `find_tools` (the tool manager's, saved scripts included).
fn served_tools(common: &Common, store: &ConfigStore) -> Vec<tools::ToolDefinition> {
    Engine::new(computer_use::mock::MockBackend::new(), store.clone())
        .with_overrides(apply_overrides(common))
        .tool_definitions()
}

fn load_store(common: &Common) -> Result<ConfigStore> {
    let path = config_path(common);
    let mut store = ConfigStore::load(Some(&path)).with_context(|| "loading configuration")?;
    apply_overrides(common)(&mut store.config);
    Ok(store)
}

/// How often each app needed pixels, in the server's folder.
const APPS_LOG: &str = "apps.json";

fn build_engine(common: &Common, store: ConfigStore) -> Result<Engine<Box<dyn Backend>>> {
    let backend = computer_use::platform_backend().with_context(|| {
        format!(
            "initializing the {} computer-use backend",
            computer_use::PLATFORM
        )
    })?;
    Ok(Engine::new(backend, store).with_overrides(apply_overrides(common)))
}

/// Log at `level` (`server.log`), or at what `$COMPUTER_USE_LOG` says.
/// Not `$RUST_LOG`: one exported for another program (`trace`, say) would
/// flood the client's log with this server's every message.
fn init_logging(level: &str) {
    env_logger::Builder::from_env(env_logger::Env::new().filter_or("COMPUTER_USE_LOG", level))
        .target(env_logger::Target::Stderr)
        .format_timestamp_millis()
        .init();
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // The hub runs on its own: no logging, no config.
    if let Some(Command::Hub { port, home }) = &cli.command {
        let mut args = vec!["--port".to_string(), port.to_string()];
        if let Some(h) = home {
            args.extend(["--home".to_string(), h.display().to_string()]);
        }
        std::process::exit(computer_use::overlay::hub::run(&args));
    }
    // The overlay helper talks JSON on stdout: no logging, no config.
    if let Some(Command::Overlay { parent, demo }) = &cli.command {
        let mut args = Vec::new();
        if let Some(p) = parent {
            args.extend(["--parent".to_string(), p.to_string()]);
        }
        if *demo {
            args.push("--demo".into());
        }
        let code = computer_use::overlay::helper::run(&args);
        std::process::exit(code);
    }

    // Adding it to the agents needs no settings.
    if let Some(Command::Install {
        clients,
        remove,
        list,
    }) = &cli.command
    {
        return install_cmd(clients, *remove, *list);
    }

    if let Some(Command::Shortcut {
        menu,
        both,
        remove,
        list,
    }) = &cli.command
    {
        return shortcut_cmd(*menu, *both, *remove, *list);
    }

    // Settings commands work even when the file is invalid.
    if let Some(Command::Config { action }) = &cli.command {
        init_logging(cli.common.log.as_deref().unwrap_or("error"));
        return config_cmd(&cli.common, action);
    }

    let command = cli.command.unwrap_or(Command::Serve);
    // Serving with a broken settings file starts with the defaults and
    // says why (the client would only see "server failed to start");
    // the other commands stop on it.
    let (store, problem) = match load_store(&cli.common) {
        Ok(store) => (store, None),
        Err(e) if matches!(command, Command::Serve) => {
            let mut store = ConfigStore {
                config: Default::default(),
                path: Some(config_path(&cli.common)),
            };
            apply_overrides(&cli.common)(&mut store.config);
            (store, Some(format!("{e:#}")))
        }
        Err(e) => return Err(e),
    };
    init_logging(&store.config.server.log);
    if let Some(p) = &problem {
        log::error!("{p}; serving with the default settings");
    }
    warn_unknown_keys(&config_path(&cli.common));

    if matches!(command, Command::Serve) {
        install_waiting_update(&store.config);
        // Look for updates a while from now, then now and then, with the
        // settings of the moment (none when the file can't be read).
        let path = config_path(&cli.common);
        computer_use::update::watch(move || {
            ConfigStore::load(Some(&path)).map_or(
                config::UpdateConfig {
                    enabled: false,
                    ..Default::default()
                },
                |s| s.config.update,
            )
        });
    }

    match command {
        Command::Serve => serve(&cli.common, store, problem),
        Command::Update {
            check,
            install,
            rollback,
        } => update_cmd(
            &store.config,
            &config_path(&cli.common),
            check,
            install,
            rollback,
        ),
        Command::Apps => run_and_print(&cli.common, store, "list_apps", json!({})),
        Command::State {
            app,
            window,
            screenshot,
        } => state(&cli.common, store, &app, window, screenshot),
        Command::Call { tool, args } => {
            let args: Value =
                serde_json::from_str(&args).with_context(|| "parsing --args as JSON")?;
            run_and_print(&cli.common, store, &tool, args)
        }
        Command::Tools { all } => {
            let defs = if all {
                tools::definitions()
            } else {
                served_tools(&cli.common, &store)
            };
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
            eprintln!(
                "{} tools, ~{} tokens per model request",
                defs.len(),
                tools::model_visible_len(&defs).div_ceil(4)
            );
            Ok(())
        }
        Command::Config { .. }
        | Command::Overlay { .. }
        | Command::Hub { .. }
        | Command::Install { .. }
        | Command::Shortcut { .. } => {
            unreachable!("handled above")
        }
        Command::Doctor => doctor(&cli.common, store),
        Command::Settings { no_browser } => {
            let path = config_path(&cli.common);
            computer_use::panel::serve(Some(path), !no_browser, |url| {
                println!("The settings panel: {url}");
                println!(
                    "(only this computer can open it; press Done on the page, or Ctrl+C, when finished)"
                );
            })?;
            Ok(())
        }
    }
}

/// Before serving: an update waiting since before the computer restarted
/// (or as `update.install` says) takes this program's place, and the
/// server starts again as the new version. Whatever fails, this one serves.
fn install_waiting_update(cfg: &Config) {
    use computer_use::update;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    update::tidy(&exe);
    let dir = update::updates_dir();
    let Some(p) = update::pending(&dir).filter(|_| cfg.update.enabled) else {
        return;
    };
    if !update::due(&p, cfg.update.install) {
        log::info!(
            "updates: {} is waiting; it goes in {}",
            p.version,
            update::when(cfg.update.install)
        );
        return;
    }
    match update::install(&p, &exe, &dir) {
        Ok(()) => {
            log::info!("updates: {} is in place; starting it", p.version);
            restart(&exe);
        }
        Err(e) => log::warn!("updates: couldn't put {} in place: {e}", p.version),
    }
}

/// Run `exe` (the new version) with this one's arguments in this one's
/// place: Unix replaces the process; Windows runs it with the same input
/// and output and exits with its code. Returns only if it can't start.
fn restart(exe: &std::path::Path) {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = std::process::Command::new(exe).args(&args).exec();
        log::error!("updates: can't start the new version: {e}");
    }
    #[cfg(not(unix))]
    match std::process::Command::new(exe).args(&args).status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(e) => log::error!("updates: can't start the new version: {e}"),
    }
}

/// `shortcut`: a shortcut that opens the panel, made or taken away.
fn shortcut_cmd(menu: bool, both: bool, remove: bool, list: bool) -> Result<()> {
    use computer_use::shortcut::{self, Place, Places};
    let places = Places::real();
    if list {
        for s in shortcut::status(&places) {
            let state = match (s.exists, s.ours) {
                (false, _) => "not there".to_string(),
                (true, false) => "another file of that name".to_string(),
                (true, true) => match &s.starts {
                    Some(p) => format!("there; starts {p}"),
                    None => "there".to_string(),
                },
            };
            let at = s
                .path
                .map_or("this system has none".into(), |p| p.display().to_string());
            println!("{:<12} {state}\n{:<12} {at}", s.place.label(places.os), "");
        }
        return Ok(());
    }
    let chosen: Vec<Place> = match (menu, both) {
        (_, true) => Place::ALL.to_vec(),
        (true, false) => vec![Place::Menu],
        (false, false) => vec![Place::Desktop],
    };
    let program = if remove {
        None
    } else {
        let env = computer_use::connect::Env::real();
        let (p, note) = computer_use::connect::program_for_clients(&env, &std::env::current_exe()?);
        if let Some(n) = note {
            println!("{n}");
        }
        Some(p)
    };
    let mut failed = 0;
    for place in chosen {
        let done = match &program {
            None => shortcut::remove(&places, place).map(|had| {
                if had {
                    format!("Removed the {} shortcut.", place.label(places.os))
                } else {
                    format!("No {} shortcut to remove.", place.label(places.os))
                }
            }),
            Some(p) => shortcut::create(&places, place, p)
                .map(|f| format!("Made {}: it opens the settings panel.", f.display())),
        };
        match done {
            Ok(m) => println!("✓ {m}"),
            Err(e) => {
                failed += 1;
                println!("✗ {}: {e}", place.label(places.os));
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} of them failed");
    }
    Ok(())
}

/// `install`: add the program to the agents, or take it out.
fn install_cmd(names: &[String], remove: bool, list: bool) -> Result<()> {
    use computer_use::connect::{self, Client, Env, State};
    let env = Env::real();
    let exe = std::env::current_exe()?;
    let planned = connect::stable_program_path(&env);
    let mut chosen = Vec::new();
    for n in names {
        chosen.push(Client::parse(n).ok_or_else(|| {
            anyhow::anyhow!(
                "\"{n}\" is not an agent: claude-code, claude-desktop, codex, cursor or vscode"
            )
        })?);
    }
    let all = connect::detect(&env, &planned);
    if list {
        for i in &all {
            let state = match &i.state {
                State::NotFound => "not found here".to_string(),
                State::NotInstalled => "here; not added".to_string(),
                State::Installed => "added".to_string(),
                State::Different(p) => format!("added, but pointing at {p}"),
                State::Unreadable(why) => format!("can't be changed safely: {why}"),
            };
            println!("{:<26} {state}\n{:<26} {}", i.client.label(), "", i.where_);
        }
        println!("\nThe program would be registered as {}", planned.display());
        return Ok(());
    }
    // Without a name: every agent that is here (and, for removing, has it).
    if chosen.is_empty() {
        chosen = all
            .iter()
            .filter(|i| {
                matches!(
                    (&i.state, remove),
                    (State::Installed | State::Different(_), true)
                        | (State::NotInstalled | State::Different(_), false)
                )
            })
            .map(|i| i.client)
            .collect();
        if chosen.is_empty() {
            println!(
                "No agent to {} (run with --list to see what is here).",
                if remove {
                    "remove it from"
                } else {
                    "add it to"
                }
            );
            return Ok(());
        }
    }
    let (program, note) = if remove {
        (exe.clone(), None)
    } else {
        connect::program_for_clients(&env, &exe)
    };
    if let Some(n) = note {
        println!("{n}");
    }
    let mut failed = 0;
    for c in chosen {
        let done = if remove {
            connect::remove(&env, c)
        } else {
            connect::install(&env, c, &program)
        };
        match done {
            Ok(m) => println!("✓ {m}"),
            Err(e) => {
                failed += 1;
                println!("✗ {}: {e}", c.label());
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} of them failed");
    }
    Ok(())
}

fn update_cmd(
    cfg: &Config,
    path: &std::path::Path,
    check: bool,
    install: bool,
    rollback: bool,
) -> Result<()> {
    use computer_use::update::{self, Found};
    let current = update::Version::current();
    let dir = update::updates_dir();
    if rollback {
        let exe = std::env::current_exe()?;
        let (from, to) = update::rollback(&exe, &dir)?;
        // The update that was undone is not taken again.
        config::edit_file(
            path,
            "update.skip_version",
            config::Edit::SetText(from.to_string()),
        )?;
        println!(
            "{to} is back in place of {from} ({}); start your MCP client again to use it. {from} will not be taken again (update.skip_version)",
            exe.display()
        );
        return Ok(());
    }
    if check {
        match update::latest(&cfg.update)? {
            Some(r) => println!(
                "{} is out (this is {current}); `update` downloads it",
                r.version
            ),
            None => println!("{current} is the latest"),
        }
        return Ok(());
    }
    let waiting = match update::check_now_forced(&cfg.update)? {
        Found::UpToDate => {
            println!("{current} is the latest");
            update::pending(&dir)
        }
        Found::Waiting(p) => Some(p),
    };
    let Some(p) = waiting else {
        return Ok(());
    };
    if install {
        let exe = std::env::current_exe()?;
        update::install(&p, &exe, &dir)?;
        println!(
            "{} is in place ({}); start your MCP client again to use it",
            p.version,
            exe.display()
        );
    } else {
        println!(
            "{} is downloaded ({}); it goes in {}",
            p.version,
            p.dir.display(),
            update::when(cfg.update.install)
        );
    }
    Ok(())
}

/// The settings as they may be shown: an API key and the HTTP token only
/// by their ends.
fn shown(mut cfg: Config) -> Config {
    if !cfg.decision.api_key.is_empty() {
        cfg.decision.api_key = config::masked_key(&cfg.decision.api_key);
    }
    if !cfg.server.http_token.is_empty() {
        cfg.server.http_token = config::masked_key(&cfg.server.http_token);
    }
    cfg
}

fn warn_unknown_keys(path: &std::path::Path) {
    if let Ok(text) = config::read_text(path) {
        for key in config::unknown_keys(&text) {
            log::warn!("{}: unknown setting `{key}` (ignored)", path.display());
        }
    }
}

fn config_cmd(common: &Common, action: &ConfigCmd) -> Result<()> {
    let path = config_path(common);
    match action {
        ConfigCmd::Path => println!("{}", path.display()),
        ConfigCmd::Init { force } => {
            config::write_template(&path, *force)?;
            println!("wrote {}", path.display());
        }
        ConfigCmd::Show { defaults } => {
            let cfg = if *defaults {
                Config::default()
            } else {
                ConfigStore::load(Some(&path))?.config
            };
            print!("{}", toml::to_string_pretty(&shown(cfg))?);
        }
        ConfigCmd::Keys => {
            for key in config::known_keys() {
                println!("{key}");
            }
        }
        ConfigCmd::Get { key } => {
            let cfg = shown(ConfigStore::load(Some(&path))?.config);
            match config::get_value(&cfg, key) {
                Some(v) => println!("{v}"),
                // A known setting with no value (`script.dir`, `audit.path`).
                None if config::known_keys().iter().any(|k| k == key) => {}
                None => anyhow::bail!("unknown setting `{key}` (see `config keys`)"),
            }
        }
        ConfigCmd::Set { key, value } => {
            config::edit_file(&path, key, Edit::Set(value.clone()))?;
            print_setting(&path, key)?;
        }
        ConfigCmd::Unset { key } => {
            config::edit_file(&path, key, Edit::Unset)?;
            print_setting(&path, key)?;
        }
        ConfigCmd::Add { key, item } => {
            config::edit_file(&path, key, Edit::Add(item.clone()))?;
            print_setting(&path, key)?;
        }
        ConfigCmd::Remove { key, item } => {
            config::edit_file(&path, key, Edit::Remove(item.clone()))?;
            print_setting(&path, key)?;
        }
        ConfigCmd::Check => {
            let text = config::read_text(&path).unwrap_or_default();
            ConfigStore::load(Some(&path))?;
            let unknown = config::unknown_keys(&text);
            if unknown.is_empty() {
                println!("{}: ok", path.display());
            } else {
                for key in &unknown {
                    println!("{}: unknown setting `{key}` (ignored)", path.display());
                }
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn print_setting(path: &std::path::Path, key: &str) -> Result<()> {
    let cfg = shown(ConfigStore::load(Some(path))?.config);
    if let Some(v) = config::get_value(&cfg, key) {
        println!("{key} = {v}");
    }
    Ok(())
}

/// The engine with the on-screen overlay, which runs as this same program in
/// helper mode and also listens for the user's emergency stop key.
fn with_overlay(engine: Engine<Box<dyn Backend>>) -> Engine<Box<dyn Backend>> {
    match std::env::current_exe() {
        Ok(exe) => engine.with_overlay(computer_use::overlay::Launcher::helper(exe)),
        Err(e) => {
            log::warn!("no overlay or stop key: cannot find this program ({e})");
            engine
        }
    }
}

fn serve(common: &Common, store: ConfigStore, problem: Option<String>) -> Result<()> {
    let server_cfg = store.config.server.clone();
    // Before anything is started: no child may hold the client's pipes.
    computer_use::backend::keep_stdio_private();
    let mut engine =
        with_overlay(build_engine(common, store)?).with_apps_log(config::home_dir().join(APPS_LOG));
    if let Some(p) = problem {
        engine = engine.with_settings_problem(p);
    }
    // Listen for the stop key from the start, not only from the first call.
    engine.arm();

    if !server_cfg.http_addr.is_empty() {
        // --http-token first, then the environment, then the settings file.
        let token = common
            .http_token
            .clone()
            .filter(|t| !t.is_empty())
            .or_else(|| {
                std::env::var("COMPUTER_USE_HTTP_TOKEN")
                    .ok()
                    .filter(|t| !t.is_empty())
            })
            .or_else(|| Some(server_cfg.http_token.clone()).filter(|t| !t.is_empty()))
            .context(
                "serving over HTTP needs a bearer token: set server.http_token, --http-token or $COMPUTER_USE_HTTP_TOKEN",
            )?;
        return serve_http(engine, &server_cfg.http_addr, token);
    }

    log::info!(
        "computer-use-mcp {} serving on stdio ({} backend)",
        env!("CARGO_PKG_VERSION"),
        computer_use::PLATFORM
    );
    // Read on a thread of the server's own (so the client can cancel a
    // call while it runs).
    let stdin = std::io::BufReader::new(std::io::stdin());
    // The server writes whole lines under a lock of its own (answers,
    // pings and a call's progress come from different threads).
    let mut server = Server::new(engine, stdin, std::io::stdout()).stop_when_input_ends();
    server.run().context("serving MCP over stdio")
}

#[cfg(feature = "http")]
fn serve_http(engine: Engine<Box<dyn Backend>>, addr: &str, token: String) -> Result<()> {
    http::serve(engine, addr, &token)
}

#[cfg(not(feature = "http"))]
fn serve_http(_engine: Engine<Box<dyn Backend>>, _addr: &str, _token: String) -> Result<()> {
    anyhow::bail!("this build has no HTTP support; rebuild with `--features http`")
}

fn run_and_print(common: &Common, store: ConfigStore, tool: &str, args: Value) -> Result<()> {
    let mut engine = build_engine(common, store)?;
    let out = engine.call_tool(tool, args);
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
    store: ConfigStore,
    app: &str,
    window: Option<String>,
    screenshot: Option<PathBuf>,
) -> Result<()> {
    let mut engine = build_engine(common, store)?;
    let mut args = serde_json::Map::new();
    args.insert("app".into(), json!(app));
    if let Some(w) = window {
        args.insert("window".into(), json!(w));
    }
    args.insert("disable_diff".into(), json!(true));
    if screenshot.is_some() {
        args.insert("screenshot".into(), json!(true));
    }
    let out = engine.call_tool("get_app_state", Value::Object(args));
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

/// The apps `list_apps` listed (one `- ` line each; a note may follow).
fn listed_apps(text: &str) -> usize {
    text.lines().filter(|l| l.starts_with("- ")).count()
}

fn doctor(common: &Common, store: ConfigStore) -> Result<()> {
    println!(
        "computer-use-mcp {} (mhrsdev/zero-use-computer)",
        env!("CARGO_PKG_VERSION")
    );
    println!("platform: {}", computer_use::PLATFORM);
    let cfg_path = config_path(common);
    let exists = cfg_path.exists();
    println!(
        "config:   {}{}",
        cfg_path.display(),
        if exists {
            ""
        } else {
            " (not created; defaults in use — run `config init`)"
        }
    );
    if let Ok(text) = config::read_text(&cfg_path) {
        for key in config::unknown_keys(&text) {
            println!("          ! unknown setting `{key}`");
        }
    }
    let defs = served_tools(common, &store);
    let c = &store.config;
    println!(
        "tools:    {} exposed ({:?} descriptions, ~{} tokens/request)",
        defs.len(),
        c.tools.descriptions,
        tools::model_visible_len(&defs).div_ceil(4)
    );
    println!(
        "images:   {} (attach {:?}, max {} px)",
        if c.text_only || !c.screenshot.enabled {
            "off"
        } else {
            "on"
        },
        c.screenshot.attach,
        c.screenshot.max_dimension
    );
    if c.screenshot.record_apps {
        let apps = computer_use::apps_log::summary(&config::home_dir().join(APPS_LOG), 5);
        if apps.is_empty() {
            println!("pixels:   no app has needed them often yet ({APPS_LOG})");
        } else {
            println!("pixels:   the apps whose tree said least ({APPS_LOG}):");
            for line in apps {
                println!("  {line}");
            }
        }
    }

    // Updates.
    let u = &c.update;
    let waiting = computer_use::update::pending(&computer_use::update::updates_dir());
    println!(
        "updates:  {}{}",
        if u.enabled {
            format!(
                "on (a look {} min after the server starts, then every {} h, from {})",
                u.check_after_mins, u.check_every_hours, u.repo
            )
        } else {
            "off".into()
        },
        match &waiting {
            Some(p) => format!(
                "; {} downloaded, it goes in {}",
                p.version,
                computer_use::update::when(u.install)
            ),
            None => String::new(),
        }
    );

    // Several agents on this desktop share one hub.
    let mut hub_running = false;
    if c.hub.enabled {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], c.hub.port));
        let running =
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300))
                .is_ok();
        hub_running = running;
        println!(
            "agents:   hub on port {} {}; messages between agents {}",
            c.hub.port,
            if running {
                "running (another server uses it)"
            } else {
                "not running (the first server starts it)"
            },
            if c.hub.chat { "on" } else { "off" }
        );
    } else {
        println!("agents:   no hub (hub.enabled = false): an overlay of each server's own");
    }

    // As the user's keyboard names it ("Control+Option+Esc" on a Mac).
    let stop_key = computer_use::overlay::helper::pretty_key(c.control.stop_hotkey.trim());
    let settings_key = computer_use::overlay::helper::pretty_key(c.control.settings_hotkey.trim());
    let decision = c.decision.clone();
    // doctor is not an agent: joining the hub would split the screen of
    // the agents at work, move their windows and draw another cursor.
    let mut store = store;
    store.config.hub.enabled = false;
    match build_engine(common, store) {
        Ok(engine) => {
            let mut engine = with_overlay(engine);
            println!("backend:  ok");
            println!("permissions:");
            for p in engine.permissions() {
                let mark = if p.granted { "✓" } else { "✗" };
                println!("  {mark} {} — {}", p.name, p.detail);
            }
            match engine.call_tool("list_apps", json!({})) {
                out if !out.is_error => {
                    println!("apps:     {} visible", listed_apps(&out.text));
                }
                out => println!("apps:     error: {}", out.text),
            }
            if stop_key.is_empty() {
                println!("stop key: none (control.stop_hotkey is empty)");
            } else if hub_running {
                // The hub holds the key for every agent: a second
                // registration here would be refused, though it works.
                println!(
                    "stop key: {stop_key} — held by the running hub for the agents at work (checked when the first server starts)"
                );
            } else {
                match engine.check_stop_key(std::time::Duration::from_secs(3)) {
                    Ok(()) => println!("stop key: ✓ {stop_key}"),
                    Err(why) => println!("stop key: ✗ {stop_key} — {why}"),
                }
            }
            if settings_key.is_empty() {
                println!(
                    "settings key: none (control.settings_hotkey is empty; `computer-use-mcp settings` opens the page)"
                );
            } else if hub_running {
                println!("settings key: {settings_key} — held by the running hub");
            } else {
                match engine.settings_key_ok(std::time::Duration::from_secs(3)) {
                    Some(true) => println!(
                        "settings key: ✓ {settings_key} opens the decision model's settings page"
                    ),
                    Some(false) => println!(
                        "settings key: ✗ {settings_key} — the system refused it (another program may use it); set control.settings_hotkey to another combination, or run `computer-use-mcp settings`"
                    ),
                    None => println!(
                        "settings key: ? {settings_key} — the helper didn't confirm it in time"
                    ),
                }
            }
        }
        Err(e) => println!("backend:  ERROR: {e:#}"),
    }
    match computer_use::decision::Decider::from_config(&decision) {
        Ok(None) => println!(
            "decision: none — press {} (or run `computer-use-mcp settings`) to add one",
            if settings_key.is_empty() {
                "nothing".to_string()
            } else {
                settings_key.clone()
            }
        ),
        Ok(Some(d)) => match computer_use::decision::page::try_model(&decision) {
            Ok(msg) => println!("decision: ✓ {msg}"),
            Err(e) => println!("decision: ✗ {} — {e}", d.label()),
        },
        Err(e) => println!("decision: ✗ {e}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_after_the_apps_is_not_counted_as_one() {
        let text = "2 running apps:\n- Files (id: files, pid: 1)\n- Text (id: text, pid: 2) [frontmost]\n\nNote: on Wayland, windows can't be moved.";
        assert_eq!(listed_apps(text), 2);
        assert_eq!(listed_apps("No GUI apps are currently running."), 0);
    }

    #[test]
    fn the_http_token_is_shown_only_by_its_end() {
        let mut cfg = Config::default();
        cfg.server.http_token = "0123456789abcdef".into();
        cfg.decision.api_key = "sk-0123456789".into();
        let cfg = shown(cfg);
        assert_eq!(cfg.server.http_token, "••••cdef");
        assert_eq!(cfg.decision.api_key, "••••6789");
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(!text.contains("0123456789abcdef"), "{text}");
    }
}
