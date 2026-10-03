# Connecting an MCP client

`computer-use-mcp` speaks MCP over **stdio** (the default) and, when built
with `--features http`, over **HTTP**. Any MCP client can use it. Copy-paste
configs for the common ones are in [`examples/`](../examples); this page
explains them.

Replace `/path/to/computer-use-mcp` with where you put the binary
(`computer-use-mcp.exe` on Windows). In JSON on Windows, double the
backslashes (`C:\\Tools\\computer-use-mcp.exe`).

## 0. Check the machine first

```bash
computer-use-mcp doctor
```

It reports the platform, whether the accessibility and screen permissions are
granted, whether the emergency stop key works, the settings file in use, and
how many tools are exposed. Fix what it flags before connecting a client:

| OS | What is needed |
|---|---|
| **macOS** | System Settings ▸ Privacy & Security ▸ **Accessibility** and **Screen Recording** for the app that *launches* the server (Terminal, Claude Desktop, Codex, your IDE). Restart that app afterwards. |
| **Windows** | Nothing to grant. Apps running as administrator can't be controlled from a normal process. |
| **Linux** | An **X11 or XWayland** session with the AT-SPI accessibility bus running. The server switches accessibility on when it is off (the desktop remembers it; GNOME: `gsettings set org.gnome.desktop.interface toolkit-accessibility false` turns it off again) and starts the apps it opens with it on. Without the bus it still starts: screenshots, input and windows work, the tree doesn't. Native Wayland input and capture aren't supported. |

## 1. Give the agent the skills

The server asks nobody for permission: which apps the agent may use and which
actions need the user's OK is up to the agent. Give it both skills from
[`skills/`](../skills):

- `computer-use`: how to work with the tools while spending few tokens;
- `computer-use-security`: the safety rules. Load it with the first one;
- `computer-use-design` (for design work): spec first, exact methods per
  app, checks with grid screenshots and colour readings, playbooks for
  Photoshop, Paint, Blender, Revit, AutoCAD and others.

Clients with Agent Skills (Claude Code, the Claude apps) load them from the
`skills/` folder; the installers and the plugin bundle include it. For other
clients the server offers the same files over MCP (section 4).

## 2. Clients

### Claude Code

The release zip has installers: `install.cmd` (Windows) or `./install.sh`
(macOS / Linux), from a permanent folder. By hand:

```bash
claude mcp add --scope user computer-use -- /path/to/computer-use-mcp serve
```

or a project file `.mcp.json` ([`examples/claude-code.mcp.json`](../examples/claude-code.mcp.json)).
Check it with `claude mcp list`, then `/mcp` inside a session. The zip is
also a Claude plugin: `claude --plugin-dir <folder>`, or upload it where an
app offers *Upload local plugin*.

### Claude Desktop

Edit `claude_desktop_config.json`
(macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`,
Windows: `%APPDATA%\Claude\claude_desktop_config.json`); see
[`examples/claude-desktop.json`](../examples/claude-desktop.json). Restart
the app. On macOS grant the permissions above to **Claude**.

### Codex (CLI / IDE)

```bash
codex mcp add computer-use -- /path/to/computer-use-mcp serve
```

or add [`examples/codex.config.toml`](../examples/codex.config.toml) to
`~/.codex/config.toml`.

### Cursor

`~/.cursor/mcp.json` (all projects) or `.cursor/mcp.json` (one project):
[`examples/cursor.mcp.json`](../examples/cursor.mcp.json).

### VS Code (GitHub Copilot agent mode)

`.vscode/mcp.json`; the key is `servers`, not `mcpServers`:
[`examples/vscode.mcp.json`](../examples/vscode.mcp.json).

### Anything else

Run `computer-use-mcp serve` as a stdio subprocess. A smoke test by hand:

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
computer-use-mcp serve --http 127.0.0.1:8787
```

The endpoint is `http://127.0.0.1:8787/mcp` with
`Authorization: Bearer <token>`. Put the token in the environment rather
than on the command line (it would show in `ps`). The server refuses to
start without a token, rejects browser origins that aren't local, and has no
TLS: beyond localhost, put it behind an SSH tunnel or a TLS proxy.

It is MCP's Streamable HTTP: sessions (`Mcp-Session-Id`), batches, progress
as an event stream, cancels that reach the running call, and a GET stream
that announces a changed tool list
([details](GUIDE.md#remote-transport-optional)).

Client entries: [`examples/http.mcp.json`](../examples/http.mcp.json)
(Cursor / generic) and, for Claude Code,
`claude mcp add --transport http computer-use http://127.0.0.1:8787/mcp --header "Authorization: Bearer $COMPUTER_USE_HTTP_TOKEN"`.

## 4. The skills over MCP

For clients without skill files, the server offers them as MCP
**prompts** (`prompts/list`, `prompts/get`: `computer-use`,
`computer-use-design` and `computer-use-security`, in your client's prompt
or slash-command menu; each brings the safety rules along) and
**resources** (`resources/list`, `resources/read`): every file as
`computer-use://skills/<skill>/<file>`, Markdown. A skill's links to its
`reference/*.md` files resolve to resources, so an agent reads one only when
it needs it. Nothing is sent to the model unless the user or the agent asks
for it, and the tool list stays the same.

## 5. Settings

`computer-use-mcp config` shows and edits `~/.computer-use/config.toml`
(`COMPUTER_USE_HOME` moves it); `computer-use-mcp config init` writes one
listing every option with its default. The defaults are sensible; see the
README for every key.
