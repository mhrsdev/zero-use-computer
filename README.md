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
- **Screen memory (beyond Codex).** When the app returns to a screen the model
  has already seen — it went back a page, closed a dialog, reopened a panel —
  the engine recognises it, gives back the element indices the model saw then,
  and sends only what changed: no new tree, no new screenshot to re-analyse.
  See [Screen memory & caching](#screen-memory--caching).
- **The same ten tools** Codex's Computer Use plugin exposes — `list_apps`,
  `get_app_state`, `click`, `perform_secondary_action`, `set_value`,
  `select_text`, `scroll`, `drag`, `press_key`, `type_text` — plus `launch_app`.
- **On-screen indicator (beyond Codex).** Its own cursor, a glow around the
  screen and a status label in state colours, click-through and invisible to
  the agent's screenshots. See [On-screen indicator](#on-screen-indicator-overlay).
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

## Screen memory & caching

An agent spends most of its tokens and time re-reading screens. The engine
keeps several layers of cache so it never makes the model look at the same
thing twice:

| Layer | What it saves | Setting |
|---|---|---|
| **Screen memory** | Every screen the model has been shown is remembered compactly (per element: hashes, its index, its rendered line). When a view matches a remembered screen, its old indices come back and only the difference from *what the model saw then* is sent. | `[cache] enabled`, `max_screens`, `max_memory_kb`, `match_threshold` |
| **Screenshot dedupe** | A 64-cell luminance fingerprint of every screenshot sent. An identical picture of the same screen is not encoded or sent again (`screenshot=true` still forces one). | `dedupe_screenshots`, `pixel_grid`, `pixel_tolerance` |
| **Read reuse** | A tree snapshot / window list taken moments ago is reused when no action ran since (e.g. `find_element` then `get_app_state`, or `get_app_state` right after an action's change report). Any action invalidates it. | `snapshot_ttl_ms` |
| **App list** | The running-app list is reused briefly between calls. | `timing.app_cache_ms` |

A round trip — look at a screen, press a button, confirm, come back — goes
like this:

```text
get_app_state  → App: Mail … · screen #1 (new)        full tree + screenshot
click "Delete" → State after the action: now on screen #2 (new), window "Confirm":
                 6 dialog "Confirm" / 7 text "Delete 3 messages?" / 8 button "OK"
click 8        → State after the action: back on screen #1 (seen before), window "Inbox":
                 Changes since you last saw screen #1 (+ added, ~ changed, - removed) …
                 - 14 list item "Meeting notes"
get_app_state  → … · screen #1
                 Screenshot: not attached …  Tree: No changes …
```

How it works:

- **Recognition.** Each element gets an identity key (the backend's own id, or
  a hash chain of role + label + position among siblings) and a *shape* (that
  structural hash). A view is matched to a remembered screen by the Jaccard
  similarity of its shapes (`match_threshold`, default 0.8), so a dialog that
  was closed and opened again is recognised even though its accessibility
  objects are new.
- **Indices never change meaning.** Numbers are handed out once per app and
  never reused. A returning screen's elements get the numbers the model saw
  then; a stale number from a screen that has gone away fails with "unknown
  element_index" instead of clicking something else.
- **The model's view is the baseline.** Diffs are computed against what the
  model was actually shown, not against the last internal read. So
  `find_element`, `wait_for` and a truncated change report never hide a
  change from the next `get_app_state`, and only the image that actually came
  back from a `batch` counts as seen.
- **Dialogs are followed.** When an action opens a new window (a dialog, a
  menu), the change report and the next `get_app_state` switch to it
  (`follow_new_windows`); when it closes, the window below is recognised.
- **Screenshots stay valid.** A returning screen's old screenshot keeps working
  for `x`/`y` clicks (shifted if the window moved; dropped, and a new one sent,
  if the window changed size).

Measured on `gtk3-widget-factory` switching between two pages (`BENCH_NAV`,
below): coming back to a page costs **~79 tokens** instead of **~2,500** (1,250
text + 1,265 image), and is ~15% faster because nothing is encoded. The memory
holds ~7.5 KiB per screen; peak process memory is unchanged.

### Text read off the screen (OCR)

Some apps show almost nothing to accessibility APIs: games, canvases, remote
desktops, some custom toolkits. For those, the engine reads the text off the
window. The lines become `ocr text` elements in the tree: numbered like any
other element, found by `find_element`, and clicked by `element_index` (at
their place on screen). They can't be set or selected.

It runs by itself when a window has fewer than `ocr.sparse_threshold`
interactive elements (`ocr.mode = "auto"`). The agent can ask for it with
`get_app_state(ocr: true)`; `"always"` and `"off"` are also possible. Lines
that repeat what the tree already says there are left out, as is glyph noise.
A picture that hasn't changed isn't read again, and the picture read is also
used as the screenshot. Card numbers read this way are masked like any other
text (`[privacy]`).

| Engine | Where |
|---|---|
| `Windows.Media.Ocr` (the languages the user installed) | Windows |
| Vision framework (`VNRecognizeTextRequest`, accurate) | macOS |
| Tesseract (`tesseract` on `PATH`, `ocr.tesseract_path`) | Linux, and anywhere the built-in engine is missing |

`ocr.engine` picks one; `ocr.languages` sets what to read (`["en", "fa"]`).
If no engine is available, the agent is told once.

### Windows and screens

The `window` tool arranges app windows. Its actions:

- `list`: the app's windows, with position, size and state;
- `focus`, `move` (x, y, optionally width, height), `resize`;
- `maximize`, `minimize`, `restore`, `fullscreen` / `exit_fullscreen`,
  `close` (as the close button does, so the app can still ask to save);
- `tile_left` / `tile_right` / `tile_top` / `tile_bottom` (half of a display)
  and `center`;
- `move_to_display`, `move_to_desktop`;
- `displays`: every monitor's area and usable area (without task bar, dock or
  menu bar), plus the virtual desktops.

Positions are screen coordinates, the same as the window line of
`get_app_state`. Each result reports where the window really ended up, and
notes when the app or window manager adjusted it (a minimum size, say).
After a move the next `get_app_state` takes a fresh screenshot.

| | Linux (X11) | Windows | macOS |
|---|---|---|---|
| Placement, state, close | EWMH/ICCCM messages to the window manager (plain X requests without one) | `SetWindowPos`, `ShowWindow`, `WM_CLOSE` | the window's AX position, size, minimized and full-screen attributes, its close button |
| Displays | RandR, work area from `_NET_WORKAREA` | monitor list with work areas | CoreGraphics, AppKit's visible frame |
| Virtual desktops | `_NET_WM_DESKTOP` | not available (Windows only lets a program move its own windows) | not available (no public API for Spaces) |

### Smart screenshots: the whole window, or just the part that changed

When a screenshot is attached and the model already has a picture of the same
screen, the engine compares the new pixels with that picture
(`screenshot.scope = "auto"`):

- **Only a small part changed** (a menu, a tooltip, a ticked box, a new line
  of text): only that part is sent, with a margin (`region_padding`,
  `region_min_size`), at the same scale. The model is told where it goes:
  "the area x 400–600, y 300–500 of your earlier screenshot". Coordinates keep
  referring to the whole screenshot, so nothing shifts.
- **Much of it changed** (more than `region_max_ratio` of the window), the
  window changed size, the screen is new, or the model asked
  (`screenshot: true`): the whole window is sent.
- **Nothing changed**: nothing is sent.

The `screenshot` tool works the same way for the whole screen: `mode: "auto"`
(the default without `app`) sends only what changed since the last
full-screen screenshot, and `mode: "full"` always sends all of it. The model
can also pick the part itself: `element_index` zooms into one element of a
window at full resolution, for small text. `scope = "full"` turns the
automatic choice off.

## On-screen indicator (overlay)

While the agent works, the user sees what it is doing — without it ever taking
their mouse:

- **The agent's own cursor.** A separate pointer — a rounded arrowhead in its
  own colour with a soft shadow, a white edge, an outline and glow in the
  state colour, and a small name tag ("Zero", `cursor_tag`) — glides to where
  the agent acts and ripples where it clicks. The real mouse is never moved, locked or restyled:
  element actions go through accessibility APIs, and the coordinate fallbacks
  on Windows and Linux put the pointer straight back (`restore_pointer`;
  macOS posts events to the app without moving it).
- **A glow around the screen** (or around the window being worked on:
  `overlay.border_target = "window"`) and a small **label** such as
  "Zero is using the computer".
- **State colours**, for the glow, the label and the cursor's ring:

| State | Colour | When |
|---|---|---|
| thinking | gold | between actions, while the model works out its next step |
| working | blue | an action is running |
| waiting for approval | yellow (pulsing) | an app or an action needs the user's OK |
| sensitive | black | a guarded action runs (send, delete, pay, confirm…) or the agent acts in a sensitive app the user opened up |
| error | red | the last action failed |
| done | green | the task is finished — then everything disappears |
| paused | grey | waiting while you use the mouse or keyboard |
| stopped | orange | you pressed the emergency stop key |

- **Approvals on screen.** A sensitive action waits for the user. When the
  agent's client can ask (MCP elicitation) it asks there; when it can't, it
  asks on the screen — an `NSAlert` on macOS, the overlay's own panel on
  Windows and X11 (`overlay.confirm_on_screen`: `never` / `when_no_client` /
  `always`). No answer in `confirm_timeout_secs` means no.
- **Nothing pops.** It fades in when it appears, blends from one state colour
  to the next, and fades out slowly when the work is done (`fade_in_ms`,
  `transition_ms`, `fade_out_ms`). All of its texts are English by default
  and configurable (`label_*`).

How it stays out of the way:

- It runs in a **separate helper process** (`computer-use-mcp overlay`). The
  engine never waits on it; if it fails to start or crashes, computer use
  carries on without it.
- If the server exits, the helper fades out and quits; if the server is
  killed, the helper's input pipe closes and it does the same; and if the
  helper itself is killed, its windows belong to its process, so the OS
  removes them. **Nothing is ever left on screen.** It also fades away on
  "done", and after `done_after_ms` without any action.
- Its windows are **click-through**, never take focus, and are **left out of
  the agent's screenshots**: `WDA_EXCLUDEFROMCAPTURE` on Windows,
  `sharingType = none` on macOS, and on X11 (which can't exclude a window)
  it is hidden for the instant of a capture.
- Native on each OS: layered windows on Windows, `NSWindow`s on macOS,
  override-redirect windows with an empty input shape on X11 (smooth glow with
  a compositing manager; a solid band without one).

A host agent that knows more (e.g. when its model is generating, or when the
task is complete) can say so with a JSON-RPC notification:

```json
{"jsonrpc": "2.0", "method": "computer_use/status", "params": {"state": "thinking"}}
```

(`thinking`, `working`, `done`, `error`, `hidden`; the library has
`Engine::set_status`). Every text, colour, size and timing is in `[overlay]`;
`computer-use-mcp overlay --demo` shows each state once on your screen.

## You stay in control

- **Emergency stop key** — `Ctrl+Alt+Esc` by default (`Ctrl+Option+Esc` on a
  Mac; `control.stop_hotkey`),
  from any app. The agent stops at once: every tool call is refused with a
  message telling the model that the user stopped it and to ask how to
  proceed; a batch, a wait or an on-screen question in progress ends too.
  The label turns orange ("Zero stopped. Press Ctrl+Alt+Esc to let it
  continue"). Press the key again to let it continue. The overlay helper
  registers exactly that one combination with the OS (`RegisterHotKey` on
  Windows, a passive key grab on X11, `RegisterEventHotKey` on macOS), so it
  receives that key and nothing else. If another program already owns the
  combination, a warning is logged: pick another one. Hosts can stop the agent
  too (`Engine::stop_handle`, `set_stopped`).
- **Pause while you work** (`control.pause_on_user_input`, on by default).
  Before each action, the engine checks how long ago anyone last used the
  mouse or keyboard (the system idle time: `GetLastInputInfo`,
  `CGEventSourceSecondsSinceLastEventType`, the X11 screen-saver idle
  counter). While you are active it waits (grey "Paused while you use the
  computer"). It continues once you have been idle for `resume_after_idle_ms`.
  After `max_pause_secs` it gives up and tells the model why. Only the time of
  the last input is read — never which key or where — and the engine's own
  synthesized input is not taken for yours. Reading (trees, screenshots) is
  never held up.
- **Private data is kept from the model** (`[privacy]`). These are masked in
  element text and blacked out of every screenshot before it is sent:
  - password fields (never even their length);
  - payment card numbers (13–19 digits passing the Luhn check; text keeps
    the last four digits);
  - fields whose label contains `cvv`, `security code`, `one-time code`…
    (`redact_labels`).

  Tool results say how many areas were hidden. `style = "fill"` (a solid box,
  default) or `"pixelate"`; each rule can be switched off. Full-screen
  captures use the private areas of the app windows the agent has read.

## Waits for the UI, checks its work

- **Smart waiting.** After an action the engine doesn't just sleep for a fixed
  time. It re-reads the app until two reads in a row agree (the UI has
  finished reacting), up to `timing.settle_max_ms`. That is
  `timing.settle = "adaptive"`; `"fixed"` goes back to a plain `settle_ms`
  pause. The last read is reused for the change report, so it costs about one
  extra read per action.
- **Verification** (`[verify]`). Each action's result is checked. If a value
  didn't take, typed text didn't land in the field, or nothing changed after a
  press, the model is told so ("Nothing on screen changed after it; check
  before repeating it").
- **Retry another way** when an action clearly failed (`verify.retry`):

  | Failure | Retry |
  |---|---|
  | an accessibility press errors | a mouse click on the element |
  | a value didn't take | focus, select all, type |
  | typed text went nowhere | click into the field and type again |
  | a scroll didn't move | the mouse wheel |
  | a field won't take focus | click it |

  A press that simply changed nothing is not repeated unless
  `verify.retry_on_no_change = true`. Guarded actions (send, pay, delete…) are
  never repeated.

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
        │      (platform-agnostic)   │  diffs, screen memory, approvals, coord map
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

**Download** a ready-made binary: every push builds `computer-use-mcp` for
Windows (x64), macOS (Apple silicon and Intel) and Linux (x64) — open the
repository's **Actions** tab, pick the latest *CI* run and download
`computer-use-mcp-<platform>` from its **Artifacts** (tagged versions `v*`
also attach zips to a GitHub **Release**). Each download contains the binary,
[`mcp.example.json`](mcp.example.json), the READMEs and the skill file.

Or build it:

```bash
cargo build --release -p computer-use-mcp
# binary at target/release/computer-use-mcp (computer-use-mcp.exe on Windows)
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
| `[overlay]` | the on-screen indicator: on/off, cursor/glow/label/click effect, screen or window glow, sizes, label texts, state colours, timings, on-screen approvals |
| `[cache]` | screen memory on/off, how many screens and how much memory, match threshold, screenshot dedupe and its sensitivity, read reuse window |
| `[audit]` | JSONL audit log on/off and path |
| `[server]` | headless approval policy, log level, HTTP address and token |
| `[linux]` / `[macos]` / `[windows]` | per-platform tuning (batch sizes, batched attribute reads, UIA cache) |
| top level | `clipboard`, `text_only`, `follow_new_windows`, `restore_pointer`, `hot_reload`, `launch_timeout_secs` |

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
- `cache.enabled` + `cache.dedupe_screenshots` (default on) — a screen the
  model already saw costs ~80 tokens instead of a full tree and image; an
  unchanged screenshot is never sent twice.
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
| peak memory | 19.7 MiB | **16.4 MiB** |
| back to a screen seen before, large app | ~2,516 tokens (tree + image) | **~79 tokens, no image** |
| `get_app_state` right after an action or `find_element` | 71 ms | **< 1 ms** (read reused) |

Where the gains come from: the Linux walker pipelines its AT-SPI queries (a
batch of elements with every query in flight at once) instead of one round trip
at a time; macOS reads each element's attributes in one batched AX call;
Windows fetches a whole window with one UI Automation cache request; captures
are downscaled with a fast area filter and encoded without copying the pixel
buffer (alpha is dropped in place, no second full-size buffer); the engine
reuses the running-app list, recent reads and remembered screens, and moves
(never clones) its cached tree. The macOS and Windows paths are type-checked but not yet measured
on real hardware.

Run it yourself:

```bash
APPS="gtk3-widget-factory" BENCH_NAV="Page 2|Page 1" scripts/desktop-session.sh \
  cargo run --release -p computer-use --example bench -- gtk3-widget-factory
```

`BENCH_NAV` names two buttons that switch between screens; the benchmark then
compares coming back to a screen with the screen memory off and on.

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
such actions. The user can stop the agent at any moment with the stop key; it
waits while they use the computer; and password fields and card numbers are
never sent to the model (see "You stay in control").

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
