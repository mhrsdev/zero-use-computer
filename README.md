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
computer-use-mcp tools                          # print tool JSON schemas
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

## Configuration

`~/.computer-use/config.toml` (override with `$COMPUTER_USE_HOME` or `--config`):

```toml
[approvals]
mode = "prompt"                 # prompt | allowlist | allow-all
always_allow = ["com.apple.TextEdit"]
always_deny  = []

[screenshot]
enabled = true
max_dimension = 1280            # longest edge sent to the model
format = "png"                  # png | jpeg

[tree]
max_nodes = 1200
diff = true
```

Admins can enforce policy via a managed config (`/etc/computer-use/managed.toml`
on Linux, `/Library/Application Support/ComputerUse/managed.toml` on macOS,
`%ProgramData%\ComputerUse\managed.toml` on Windows) with `denied_apps`,
`allowed_apps`, or a forced `approval_mode`.

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

Terminals, password managers, and OS authentication/consent prompts are
hard-blocked and can never be automated, along with the agent's own app. The
first use of any other app is gated by approval. The model is instructed (see
the skill) to pause before consequential actions such as sending, purchasing or
deleting.

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
