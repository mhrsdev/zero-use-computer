# Connecting an MCP client

`computer-use-mcp` speaks MCP over **stdio** (the default) and, when built
with `--features http`, over **HTTP**. Any MCP client can use it. Copy-paste
configs for the common ones are in [`examples/`](../examples); this page
explains them.

Replace `/path/to/computer-use-mcp` with where you put the binary
(`computer-use-mcp.exe` on Windows). On Windows use double backslashes in JSON
(`C:\\Tools\\computer-use-mcp.exe`).

## 0. Check the machine first

```bash
computer-use-mcp doctor
```

It reports the platform, whether the accessibility/screen permissions are
granted, the settings file in use, and how many tools are exposed. Fix what it
flags before connecting a client:

| OS | What is needed |
|---|---|
| **macOS** | System Settings ▸ Privacy & Security ▸ **Accessibility** and **Screen Recording** for the app that *launches* the server (Terminal, Claude Desktop, Codex, your IDE). Restart that app afterwards. |
| **Windows** | Nothing to grant. Apps running as administrator can't be controlled from a normal process. |
| **Linux** | An **X11 or XWayland** session with the AT-SPI accessibility bus running (GNOME: Settings ▸ Accessibility; `gsettings set org.gnome.desktop.interface toolkit-accessibility true`). Native Wayland input/capture isn't supported. |

## 1. Approvals when no human is at the keyboard

The first use of each app asks for approval through the client (MCP
elicitation) or an on-screen dialog. Clients that can't show a prompt need a
policy instead:

```
computer-use-mcp --approval allow-all serve          # every app not blocked by policy
computer-use-mcp --allow "TextEdit" --allow Notepad serve   # only these apps
```

If a client can't show an approval prompt and no on-screen dialog is available, the tool
reply says *nobody could be asked* (it is not a refusal) and names the app's id: pass
`--allow "<id>"` or `--approval allow-all`, or add it to `approvals.always_allow`.

Terminals, password managers, OS security prompts and the agent's own host app
stay blocked unless you change `[sensitive]` in the settings.

## 2. Clients

### Claude Code

```bash
claude mcp add computer-use -- /path/to/computer-use-mcp serve
# available in every project:
claude mcp add --scope user computer-use -- /path/to/computer-use-mcp serve
```

or a project file `.mcp.json` ([`examples/claude-code.mcp.json`](../examples/claude-code.mcp.json)).
Check it with `claude mcp list`, then `/mcp` inside a session.

### Claude Desktop

Edit `claude_desktop_config.json`
(macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`,
Windows: `%APPDATA%\Claude\claude_desktop_config.json`) — see
[`examples/claude-desktop.json`](../examples/claude-desktop.json) — and restart
the app. On macOS grant the permissions above to **Claude**.

### Codex (CLI / IDE)

```bash
codex mcp add computer-use -- /path/to/computer-use-mcp serve
```

or add to `~/.codex/config.toml`
([`examples/codex.config.toml`](../examples/codex.config.toml)).

### Cursor

`~/.cursor/mcp.json` (all projects) or `.cursor/mcp.json` (one project):
[`examples/cursor.mcp.json`](../examples/cursor.mcp.json).

### VS Code (GitHub Copilot agent mode)

`.vscode/mcp.json` — note the key is `servers`, not `mcpServers`:
[`examples/vscode.mcp.json`](../examples/vscode.mcp.json).

### Anything else

Run `computer-use-mcp serve` as a stdio subprocess. Smoke test by hand:

```bash
printf '%s\n' \
 '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
 '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
 '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
 | computer-use-mcp serve
```

## 3. HTTP (remote agent, same machine or trusted network)

Build with the feature, pick a token, and keep it on loopback:

```bash
cargo build --release -p computer-use-mcp --features http
export COMPUTER_USE_HTTP_TOKEN="$(openssl rand -hex 24)"
computer-use-mcp --approval allow-all serve --http 127.0.0.1:8787
```

The endpoint is `http://127.0.0.1:8787/mcp` with
`Authorization: Bearer <token>`. Put the token in the environment rather than
on the command line (it would show in `ps`). The server refuses a non-loopback
address without a token, rejects browser origins that aren't local, and has no
TLS — for anything beyond localhost put it behind an SSH tunnel or a TLS proxy.

Client entries: [`examples/http.mcp.json`](../examples/http.mcp.json) (Cursor /
generic) and, for Claude Code,
`claude mcp add --transport http computer-use http://127.0.0.1:8787/mcp --header "Authorization: Bearer $COMPUTER_USE_HTTP_TOKEN"`.

## 4. The built-in skills over MCP

Besides the `skill` tool, the server offers the per-OS how-to playbooks as MCP
**prompts** (`prompts/list`, `prompts/get`: pick one in your client's prompt
menu) and **resources** (`computer-use://skills/<name>`, Markdown). They follow
`skills = true|false` in the settings.

## 5. Settings

`computer-use-mcp config` shows and edits `~/.computer-use/config.toml`
(`COMPUTER_USE_HOME` moves it). The defaults are sensible; see the README for
every key.
