# Zero Use Computer

**Desktop control for AI agents.** Your agent reads the app the way a
screen reader does, acts on real controls, and looks at pixels only when
they matter. One Rust binary, any MCP client, Windows, macOS and Linux.

[Download v3.7.0](https://github.com/mhrsdev/zero-use-computer/releases/latest)
· [Connect a client](docs/CONNECT.md)
· [Guide](docs/GUIDE.md)
· [Benchmarks](bench/README.md)
· [Changelog](CHANGELOG.md)

---

Most computer-use agents send a screenshot every turn and guess where to
click. Zero gives the model the window's accessibility tree with numbered
elements, so `click(12)` presses the Save button through the OS's own
accessibility API. Most actions work on windows in the background without
touching the user's mouse, and the whole loop costs a fraction of the
tokens.

```text
> get_app_state(app="Settings")
App: Settings · window "Settings" · screen #1 (new)
Screenshot #1: 720x360 px.
 26 button "Save"
 27 checkbox "Subscribe to newsletter" (unchecked)
 5 × radio button:
 28 "Canada" (checked) · 29 "Germany" (unchecked) · 31 "Japan" (unchecked) …
 34 text field "Email"
 36 text field "Full name"

> batch(["set 36 \"Ada Lovelace\"", "set 34 \"ada@example.com\"",
         "click \"Japan\"", "click \"Subscribe to newsletter\"", "click \"Save\""])
Ran 5 step(s):
1. set_value — Set text field "Full name"; it shows the new value.
…
State after the steps:
~ 27 checkbox "Subscribe to newsletter" (checked)
~ 31 radio button "Japan" (checked)
~ 36 text field "Full name" value="Ada Lovelace"
```

(Trimmed from a real run of the benchmark's form task: two calls.)

## What makes it different

- **It remembers screens.** Go back a page or close a dialog and the model
  gets "screen #1 (seen before)": the indices it already knows and only
  what changed. A picture the model already has is never sent twice.
- **Each action reports its own result.** The state after a click comes
  with the click, and `expect` says whether the dialog opened or the value
  stuck (confirmed, not seen, or uncertain). No look-again-to-be-sure turns.
- **A small tool list that can reach everything.** The model starts with
  the 15 tools most tasks use. Drawing, design, 3D, exact aiming, windows,
  scripts and the clipboard are one `find_tools` away, and the list never
  changes, so the prompt cache holds.
- **It reads what the tree can't.** Canvases and painted text are read off
  the screen and become clickable `ocr text` elements.
- **You stay in control.** An on-screen indicator, an emergency stop key
  (Ctrl+Alt+Esc), a pause while you use the mouse, masked passwords and
  card numbers, and keystrokes that only ever go to the app they're meant
  for. Which apps the agent may touch is set by a
  [security skill](skills/computer-use-security/SKILL.md).

## Numbers

Six real tasks in a GTK app (a form, a 300-row table, a canvas, a dialog
flow, a longer session), success checked from the app's own record.
Scripted runs, three each; tokens are estimates (4 characters a token,
an image width × height / 750, no prompt cache).
[Full tables](bench/results/v3.7-releases/step.md).

| | v3.0 | v3.6 | **v3.7** |
|---|---|---|---|
| Sent with every request (instructions, skills, tools) | 6,718 | 7,783 | **4,938** |
| Tool definitions | 4,363 | 4,990 | **2,167** |
| Input to finish four tasks | 349,873 | 367,006 | **250,701** |

v3.7 sends **37% less per request and 32% less per task** than v3.6. With
`batch`, all six tasks: 325,339 → 225,628 (−31%).

### Against Codex-style computer use

Codex's computer use is the model this project started from: the same ten
core tools, accessibility first. The benchmark can run Zero the way Codex
behaves (a screenshot with every look, no screen memory, no change report
after an action, every tool listed) and compare. This is a **simulation of
Codex's behaviour on this server, not a run of Codex**.
[Full table](bench/results/v3.7-codex/compare.md).

| Five tasks (form, table, board, shapes, long) | Codex-style (simulated) | Zero v3.7 | Zero v3.7 + `batch` |
|---|---|---|---|
| Input tokens | 364,354 | **245,147** (−33%) | **183,860** (−50%) |
| Screenshots sent | 11 | **5** | **5** |
| Image tokens | 7,702 | **3,330** (−57%) | **3,330** |
| Tool calls | 41 | 36 | **26** |

| | Codex-style | Zero |
|---|---|---|
| Each look | tree and a screenshot | the tree or what changed; a picture only when it adds something |
| Coming back to a screen | everything again | "seen before", only the changes |
| After an action | look again | the change comes with the action |
| Checking an action worked | look and compare | `expect`: confirmed, not seen, uncertain |
| Several known steps | one call each | one `batch`, which stops on an unexpected window |
| Text only in pixels | in the screenshot | read off the screen, clickable |
| Tool list | every tool, every request | 15 tools plus `find_tools`; the rest on demand |
| Clients | | any MCP client: Claude Code, Codex, Cursor, VS Code, Claude Desktop |
| Extras | | 2D design board, 3D scene planner, exact aiming, scripts, a fast decision model |

## Quick start

1. Download the zip for your system from the
   [latest release](https://github.com/mhrsdev/zero-use-computer/releases/latest)
   and extract it somewhere permanent.
2. **Claude Code:** run `./install.sh` (macOS, Linux) or `install.cmd`
   (Windows). It registers the server; `claude mcp list` shows it.
   **Anything else:** point your client at `computer-use-mcp serve`.
   Configs for Codex, Cursor, VS Code and Claude Desktop are in
   [`examples/`](examples) and [docs/CONNECT.md](docs/CONNECT.md).
3. Give the agent the [skills](skills/) (the zip is also a Claude plugin
   that brings them).

`computer-use-mcp doctor` checks permissions and the stop key. On macOS,
allow Accessibility and Screen Recording for the app that starts the
server. On Linux it needs the AT-SPI bus (X11, or Wayland with Hyprland,
sway and other wlroots compositors).

Build it yourself:

```bash
cargo build --release -p computer-use-mcp
```

Or embed the library (`computer-use`) in your own agent:
[Guide › Embed the library](docs/GUIDE.md#embed-the-library).

## Docs

- [Guide](docs/GUIDE.md): every tool, setting, platform detail and
  architecture note.
- [Connect a client](docs/CONNECT.md) · [Upgrading](docs/MIGRATING.md) ·
  [Roadmap](docs/ROADMAP.md)
- [Benchmarks](bench/README.md): how tasks are measured, and how to run
  them against any release.
- Skills: [computer-use](skills/computer-use/SKILL.md),
  [security](skills/computer-use-security/SKILL.md),
  [design](skills/computer-use-design/SKILL.md).

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
