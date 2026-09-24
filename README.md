# computer-use

A Codex-style **computer use** capability for agents, in Rust. It gives any
agent the same desktop-control surface OpenAI's Codex app exposes — see and
operate real GUI apps by clicking, typing, reading on-screen state and running
multi-app workflows — built on each OS's native **accessibility API** plus
**screenshots**, with **per-app approvals**.

It is a standalone building block: run it as an **MCP server** (works with
Codex, Claude Code, or any MCP-capable agent), or embed the **library** in your
own agent.

> فارسی: راهنمای فارسی در [README.fa.md](README.fa.md).

## Why it mirrors Codex

Codex's computer use is accessibility-first: it reads an app's accessibility
tree, acts on elements semantically, and only falls back to pixels when it must.
This project follows the same architecture and behaviour:

- **Accessibility tree + screenshot.** `get_app_state` returns a pruned, numbered
  accessibility tree *and* a screenshot of the window. Actions target an
  `element_index` and go through the platform's native action (press, set value,
  toggle…), so they work on background/occluded windows.
- **Turn-scoped indices with diffs.** Element indices are valid only until the
  next `get_app_state`; subsequent calls return a **diff** (unless
  `disable_diff` is set), keeping token use low.
- **The same ten tools** Codex's Computer Use plugin exposes — `list_apps`,
  `get_app_state`, `click`, `perform_secondary_action`, `set_value`,
  `select_text`, `scroll`, `drag`, `press_key`, `type_text` — plus `launch_app`.
- **Approvals & safety.** Each app is approved before it is controlled (once /
  for the session / always), and terminals, credential & OS-security prompts,
  and the agent's own host app can never be controlled.

## Tools

| Tool | What it does |
|------|--------------|
| `list_apps` | List running desktop apps (id, pid, window state). |
| `launch_app` | Start an app by name/bundle id/executable and wait for a window. |
| `get_app_state` | The window's numbered accessibility tree **+ a screenshot**. Call first each turn. |
| `click` | Click an element by `element_index` (uses its accessibility action) or at `x`/`y` screenshot pixels. |
| `perform_secondary_action` | A non-click action listed for the element (`show_menu`, `increment`, `expand`, `toggle`…). |
| `set_value` | Set a field's text, a slider, or a checkbox/switch directly. |
| `select_text` | Select a substring (or all) in a text element. |
| `scroll` | Scroll an element or the area at a point. |
| `drag` | Drag between elements or points. |
| `press_key` | A key or shortcut, e.g. `cmd+s`, `ctrl+shift+t`, `Down Down Return`. |
| `type_text` | Type into the focused element. |
| `find_element` | Search the tree by role/name/text/editable; returns just the matches with their indices. |
| `wait_for` | Poll until an element (role/name/text + state) appears, with a timeout. |
| `screenshot` | Capture the **full screen**, a **screen region**, or a window; optional set-of-marks overlay. |
| `batch` | Run several tools in one call (fill a form, then submit). |
| `get_clipboard` / `set_clipboard` | Read/write the system clipboard. |

Beyond Codex's ten, the extra tools (`find_element`, `wait_for`, `batch`,
region/full `screenshot`, clipboard) cut round-trips and token use, and the
engine adds two safety layers Codex leaves to the model: an **action guard**
that confirms consequential presses (Send / Delete / Pay …) and an optional
**audit log**.

The full operating contract the model should follow is in
[`skill/SKILL.md`](skill/SKILL.md).

## Architecture

```
        agent (Codex / Claude Code / your own)
                     │  tools
        ┌────────────┴─────────────┐
        │   computer-use-mcp        │  MCP stdio server + CLI
        │   (JSON-RPC, elicitation) │
        └────────────┬─────────────┘
                     │  Engine::call_tool
        ┌────────────┴─────────────┐
        │        computer-use        │  engine: tree pruning, stable indices,
        │      (platform-agnostic)   │  diffs, approvals policy, coordinate map
        └───┬───────────┬───────────┬┘
            │           │           │   Backend trait
     ┌──────┴──┐  ┌─────┴────┐  ┌───┴──────┐
     │ macOS   │  │ Windows  │  │ Linux    │
     │ AX API  │  │ UI Auto- │  │ AT-SPI2  │
     │ CGEvent │  │ mation   │  │ (D-Bus)  │
     │ CGWindow│  │ SendInput│  │ XTest    │
     │ List    │  │ PrintWin │  │ X11 img  │
     └─────────┘  └──────────┘  └──────────┘
```

The **engine** is platform-agnostic and holds all the behaviour that must be
identical everywhere: pruning the raw accessibility tree into a sparse numbered
outline, assigning stable per-turn element indices, diffing snapshots, mapping
screenshot pixels to screen coordinates, and enforcing the approval policy. Each
**backend** is a thin adapter over one OS:

| | macOS | Windows | Linux |
|---|---|---|---|
| Tree & actions | Accessibility (AX) API | UI Automation | AT-SPI2 over D-Bus |
| Input | `CGEvent` posted to the target pid (background, cursor doesn't move) | `SendInput` | XTest |
| Capture | `CGWindowListCreateImage` (+ `_AXUIElementGetWindow`) | `PrintWindow` (`PW_RENDERFULLCONTENT`) | X11 `GetImage` |

## Use it as an MCP server

Build:

```bash
cargo build --release -p computer-use-mcp
# binary at target/release/computer-use-mcp
```

**Claude Code** (`.mcp.json` or your MCP config):

```json
{
  "mcpServers": {
    "computer-use": {
      "command": "/path/to/target/release/computer-use-mcp",
      "args": ["serve"]
    }
  }
}
```

**Codex** (`~/.codex/config.toml`):

```toml
[mcp_servers.computer-use]
command = "/path/to/target/release/computer-use-mcp"
args = ["serve"]
```

Any MCP client works — the server speaks JSON-RPC 2.0 over stdio and implements
`initialize`, `tools/list`, `tools/call`, and per-app approval via
`elicitation/create` (clients that support elicitation get an approval prompt;
others fall back to the `--headless-approve` policy).

### CLI

The same binary is a handy CLI:

```bash
computer-use-mcp doctor                       # platform, permissions, config
computer-use-mcp apps                          # list running apps
computer-use-mcp state "TextEdit" --screenshot shot.png
computer-use-mcp call click '{"app":"TextEdit","element_index":3}'
computer-use-mcp tools                          # tool definitions the model will see
computer-use-mcp config show                    # settings (see "Settings" below)
```

Flags: `--config <path>`, `--approval prompt|allowlist|allow-all`,
`--allow <app>` (repeatable, pre-approve for the run),
`--headless-approve deny|allow`.

## Embed the library

```rust
use computer_use::{AllowApprover, tools};

let mut engine = computer_use::platform_engine()?;      // native backend + config
let defs = tools::definitions();                        // hand these to your model
let out = engine.call_tool("list_apps", serde_json::json!({}), &mut AllowApprover);
println!("{}", out.text);
```

Plug in your own approval UI by implementing the `Approver` trait. See
[`crates/computer-use/examples/mock_session.rs`](crates/computer-use/examples/mock_session.rs)
for a full, runnable session against the in-memory mock backend:

```bash
cargo run -p computer-use --example mock_session
```

## Settings

Every behaviour is a setting in `~/.computer-use/config.toml` (override the
location with `$COMPUTER_USE_HOME` or `--config`). Start from the documented
template — it lists **every option with its default** and a comment:

```bash
computer-use-mcp config init                 # write the documented template
computer-use-mcp config show                 # effective settings
computer-use-mcp config keys                 # every setting key
computer-use-mcp config get screenshot.attach
computer-use-mcp config set screenshot.attach always
computer-use-mcp config set sensitive.terminals ask
computer-use-mcp config add approvals.always_allow com.googlecode.iterm2
computer-use-mcp config add tools.disabled drag
computer-use-mcp config unset tree.max_nodes   # back to the default
computer-use-mcp config check                # validate + flag misspelled keys
```

Edits are validated before they are written (bad values and unknown keys are
rejected with a clear message) and your comments are preserved. A running
server **reloads the file automatically** (`hot_reload = true`) and tells MCP
clients when the tool list changed — no restart needed. Command-line flags
(`--approval`, `--text-only`, `--log`, `--http`, …) override the file and keep
applying after a reload. The agent has no tool to change settings.

| Section | What you control |
|---|---|
| `[approvals]` | `prompt` / `allowlist` / `allow_all`, per-app `always_allow` / `always_deny`, agent host apps |
| `[sensitive]` | terminals, credential stores, OS security prompts, agent apps, own process: `block` / `ask` / `allow` each, plus your own patterns |
| `[guard]` | confirm/allow/block consequential presses, and the keyword list |
| `[tools]` | hide tools (`disabled`), allow-list them (`enabled`), `compact` or `full` descriptions |
| `[screenshot]` | on/off, `attach` = `auto` / `always` / `never`, max size, PNG/JPEG, quality, compression, resize filter |
| `[tree]` | size limits, text length, indentation, shown actions/states, diffs, change reports |
| `[timing]` | settle delay, key delay, app-list cache, `wait_for` defaults |
| `[audit]` | JSONL audit log on/off and path |
| `[server]` | headless approval policy, log level, HTTP address and token |
| `[linux]` / `[macos]` / `[windows]` | per-platform tuning (batch sizes, batched attribute reads, UIA cache) |
| top level | `clipboard`, `text_only`, `hot_reload`, `launch_timeout_secs` |

Sensitive apps are blocked by default; open them up one category or one app at
a time. Admins can enforce policy via a managed config
(`/etc/computer-use/managed.toml` on Linux,
`/Library/Application Support/ComputerUse/managed.toml` on macOS,
`%ProgramData%\ComputerUse\managed.toml` on Windows) with `denied_apps`,
`allowed_apps`, or a forced `approval_mode` — these always win over user config.

### Token-saving knobs

- `tools.descriptions = "compact"` (default) — the tool list goes out with every
  model request; compact cuts it from ~3,300 to ~1,360 tokens. Hiding tools you
  don't need (`tools.disabled`) saves more.
- `screenshot.attach = "auto"` (default) — images only for a first view, a large
  change, or a tree with almost no interactive elements; `get_app_state` diffs on
  an unchanged window cost ~60 tokens instead of ~1,300.
- `screenshot.max_dimension` — image tokens scale with width × height
  (1024 px ≈ 36% fewer than 1280 px). `text_only = true` removes images entirely.
- `tree.max_text_len`, `tree.show_actions`, `tree.show_states`,
  `tree.report_changes_max_lines` trim the text further.

### Remote transport (optional)

Build with the `http` feature to serve MCP over HTTP for a remote agent:

```bash
cargo build --release -p computer-use-mcp --features http
computer-use-mcp serve --http 127.0.0.1:8787 --http-token "$TOKEN" --approval allow-all
# or put it in settings: server.http_addr / server.http_token
```

Each POST body is one JSON-RPC message. There is no interactive approval
channel over HTTP, so run it with a deliberate approval policy, require a bearer
token, and bind it to localhost or a trusted network.

## Performance

Measured with `examples/bench.rs` on Linux (Xvfb, AT-SPI) against a small GTK
dialog and `gtk3-widget-factory` (~500 accessible elements). Token counts are
estimates (text ≈ 4 chars/token, images ≈ width × height / 750).

| | before | after |
|---|---|---|
| `get_app_state`, large app | 533 ms | **87 ms** |
| repeat `get_app_state` (diff), large app | 507 ms, ~1,346 tokens | **67 ms, ~66 tokens** |
| `find_element`, large app | 473 ms | **72 ms** |
| repeat `get_app_state`, small dialog | 19 ms, ~219 tokens | **2.4 ms, ~58 tokens** |
| tool definitions per model request | ~3,300 tokens | **~1,360 tokens** |
| peak memory | 19.7 MiB | **16.3 MiB** |

Where the gains come from: the Linux walker pipelines its AT-SPI queries (a
batch of elements with every query in flight at once) instead of one round trip
at a time; macOS reads each element's attributes in one batched AX call;
Windows fetches a whole window with one UI Automation cache request; captures
are downscaled with a fast area filter and encoded without copying the pixel
buffer; and the engine reuses the running-app list and moves (never clones) its
cached tree. The macOS and Windows paths are type-checked but not yet measured
on real hardware.

Run it yourself:

```bash
APPS="gtk3-widget-factory" scripts/desktop-session.sh \
  cargo run --release -p computer-use --example bench -- gtk3-widget-factory
```

## Platform setup

- **macOS** — grant the host app **Accessibility** and **Screen Recording** in
  System Settings ▸ Privacy & Security. Input is posted to the target process,
  so the user's cursor doesn't move.
- **Windows** — no special permission for UI Automation; coordinate input uses
  `SendInput` on the active desktop.
- **Linux** — needs an AT-SPI2 accessibility bus and an X11 display. Enable
  accessibility for your toolkit (e.g. GTK loads the at-spi bridge when the a11y
  bus is present). Wayland isn't supported for synthesized input/capture; use an
  X11 (or XWayland) session.

## Safety

Sensitive apps — terminals, password managers, OS authentication/consent
prompts, agent host apps, and the agent's own process — are **blocked by
default**, but this is policy, not a hard wall: each category can be set to
`ask` or `allow`, or a single app permitted via `approvals.always_allow`, so you
grant access deliberately from settings (admin `managed.toml` still overrides).
The first use of any other app is gated by approval. On top of that, the
**action guard** confirms consequential presses (Send / Delete / Pay …) at the
engine level — not just by trusting the model — and an optional **audit log**
records every call. The model is also instructed (see the skill) to pause before
such actions.

## Development

```bash
cargo test                                   # engine + server unit tests
cargo clippy --all-targets
# live Linux test against a real GTK app under Xvfb:
bash crates/computer-use/tests/run_live_linux.sh
# cross type-check the other backends:
cargo check -p computer-use --target aarch64-apple-darwin
cargo check -p computer-use --target x86_64-pc-windows-msvc
```

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE) at your option.
