# computer-use

A Codex-style **computer use** capability for agents, in Rust. It gives any
agent the same desktop-control surface OpenAI's Codex app exposes — see and
operate real GUI apps by clicking, typing, reading on-screen state and running
multi-app workflows — built on each OS's native **accessibility API** plus
**screenshots**.

It is a standalone building block: run it as an **MCP server** (works with
Codex, Claude Code, or any MCP-capable agent), or embed the **library** in your
own agent.

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
  and sends only what changed: no new tree, and a new screenshot only if the
  pixels differ from the one the model has.
  See [Screen memory & caching](#screen-memory--caching).
- **The same ten tools** Codex's Computer Use plugin exposes — `list_apps`,
  `get_app_state`, `click`, `perform_secondary_action`, `set_value`,
  `select_text`, `scroll`, `drag`, `press_key`, `type_text` — plus `launch_app`,
  `draw` (shapes and parametric curves with the mouse held down),
  `trace_image` (a reference picture as flat colours to paint), `design` (a
  2D design board) and `scene` (a 3D model planned as solids), and `locate`
  (exact places to aim at).
- **On-screen indicator (beyond Codex).** Its own cursor, a glow around the
  screen and a status label in state colours, click-through and invisible to
  the agent's screenshots. See [On-screen indicator](#on-screen-indicator-overlay).
- **Safety lives in the agent, not the server.** The server has no
  approvals or action confirmations. Which apps the agent may use and which
  actions need the user's OK is set by the
  [`computer-use-security`](skills/computer-use-security/SKILL.md) skill (also
  summarised in the server's MCP instructions). The server keeps only what an
  agent can't do for itself: input goes only to the app it is meant for,
  `launch_app` never runs a command line, password fields and card numbers
  are masked, and the user has an emergency stop key.

## Tools

| Tool | What it does |
|------|--------------|
| `list_apps` | List running desktop apps (id, pid, window state). |
| `launch_app` | Start an app by its name in the app menu ("Google Chrome"), bundle id or executable (no arguments) and wait for a window. |
| `get_app_state` | The window's numbered accessibility tree **+ a screenshot**. Call first each turn. |
| `click` | Click an element by `element_index` (uses its accessibility action) or at `x`/`y` screenshot pixels; `snap` moves the point onto the nearest corner, edge, small shape's centre or colour first. |
| `perform_secondary_action` | A non-click action listed for the element (`show_menu`, `increment`, `expand`, `toggle`…). |
| `set_value` | Set a field's text, a slider, or a checkbox/switch directly. |
| `select_text` | Select a substring (or all) in a text element. |
| `scroll` | Scroll an element or the area at a point. |
| `drag` | Drag between elements or points (`snap` as for `click`). |
| `draw` | Draw with the mouse held down: rectangles (rounded), ellipses, arcs, regular polygons, stars, Bézier curves, smooth freehand strokes, parametric curves `x(t)`, `y(t)` and function plots `y = f(x)` with axes; any stroke can be rotated and repeated (rows, radial patterns). Coordinates in screenshot pixels, an element's box, the document's own units or a math range with y up (`canvas`). `fill` paints a closed shape solid with the brush (shapes painted back to front cover each other). `preview` shows the strokes over a screenshot first, on named cells (graph paper sized to the drawing); the result says which cells the drawing covers and where a bucket click fills each closed outline, checked on the real pixels (one click per piece when other lines cut it, or where a fill would leak out). |
| `trace_image` | Turn a reference picture (an image file, or what a window shows) into a few flat colours and shapes, as steps to paint back to front with `draw`; `screenshot` with `compare` then shows where the canvas still differs. |
| `design` | A design board, like Canva: a picture composed from layers (the shapes `draw` takes, and text) that can be added, changed, mirrored, aligned, distributed and reordered. Returns the rendered picture on named cells, the layers' boxes, checks (off the page, nearly centred, not quite symmetric, hard-to-read text) and the steps to paint it; then `draw` paints a step, or `export` writes a temporary SVG or PNG to import. |
| `scene` | A 3D model planned as solids (box, cylinder, sphere, cone, torus, plane) with exact sizes, centres and rotations, Z up. Returns the front, right and top views to one scale and a perspective view with shadows, the parts' extents, checks (what floats, sinks into the ground or runs into another part, and by how much) and the numbers to build it in Blender or another 3D app; `export` writes a temporary OBJ with its colours. |
| `locate` | Exact places in a window, in click coordinates: every area of a colour, every look-alike of an icon or marker, or the exact corner, edge or centre next to a rough point. |
| `press_key` | A key or shortcut, e.g. `cmd+s`, `ctrl+shift+t`, `Down Down Return`, `Numpad7`; `x`/`y` points the mouse there first (apps like Blender send keys to what is under the pointer). |
| `type_text` | Type into the focused element (`x`/`y` as for `press_key`). |
| `find_element` | Search the tree by role/name/text/editable; returns just the matches with their indices. |
| `wait_for` | Poll until an element (role/name/text + state) appears, with a timeout. |
| `screenshot` | Capture the **full screen**, a **screen region**, or a window; optional set-of-marks overlay, a labelled coordinate **grid**, the main colours (**palette**) and exact colours at points (**pick**); with `canvas` the grid and pick use the document's units or a plot's range, as `draw` does, **cells** lays named graph paper over the document and `cell` magnifies one cell. `zoom` magnifies around a point to aim exactly. |
| `batch` | Run several tools in one call (fill a form, then submit). |
| `script` | Run a small program inside the server for what the tools can't do in one call: loops and conditions over any tool, maths, data from files or the web, pictures built on a graph-paper page. `save` keeps a script as a **new tool** of its own. See [Scripts](#scripts). |
| `get_clipboard` / `set_clipboard` | Read/write the system clipboard. |

Beyond Codex's ten, the extra tools (`find_element`, `wait_for`, `batch`,
`script`, region/full `screenshot`, clipboard) cut round-trips and token
use, and an optional **audit log** records every call.

### Scripts

The tools cover what most tasks need; `script` covers the rest. The model
writes a short program (in [Rhai](https://rhai.rs), a sandboxed language
that reads like JavaScript) and the server runs it:

- **Any tool, with logic around it.** `tool("click", #{app: "Paint", x:
  10, y: 20})` runs a tool and gives its text; loops, conditions, retries
  and waits go around it. `elements(app, #{role: "button"})` gives the
  matching elements as data, and `colors(app, points)` the exact pixel
  colours. Every call is an ordinary tool call: the stop key, the pause
  while you work and the masking of private data all apply.
- **The graph-paper page.** `page("chess", 800, 800, #{cell: 100})` is a
  design on the design board with named cells (A1 to H8 here). A script
  fills, labels and measures cells by name (`p.fill_cell("C4",
  "#222222")`, `p.cell("C4")`) and adds any shape or text. The picture
  comes back with the result, and the page can be exported or painted into
  an app step by step like any design. `cells(w, h, size)` gives the same
  cells for any canvas, and `cell_size` on `design`, `draw` and `screenshot`
  makes them all name the same cells.
- **Data.** Whatever the model passes in `data` and `args`; files (CSV,
  JSON, text: read anywhere, written in the scripts' own folder by
  default); the web through `curl` (`fetch`, `fetch_json`, `download`);
  values remembered between runs; regular expressions, maths, random
  numbers, colours and dates.
- **New tools.** `script(save="chessboard", code=..., description=...,
  params=...)` keeps a script in `~/.computer-use/scripts/chessboard.rhai`.
  From then on it is a tool of its own (the server tells the client its
  tool list changed), it can be run by name, and other scripts can run it.
  The files are plain text you can read and edit.

A script can't reach the system except through these functions. Each run
has a time limit (`[script] max_seconds`, 5 minutes by default), and the
stop key ends it at once. `[script]` also sets which files scripts may use
(`none`, `workspace`, `read` or `all`) and whether they may use the web.
The function reference is
[`skills/computer-use/reference/scripts.md`](skills/computer-use/reference/scripts.md),
also returned by `script(help=true)`.

Files that `design` and `scene` export are temporary. They go to a
`computer-use-exports` folder in the system's temp directory and are
deleted when the server stops. Files there older than a day are removed,
and the folder is kept under 200 MB, so exports never pile up on disk.

The model should load two skills:

- [`skills/computer-use/SKILL.md`](skills/computer-use/SKILL.md): how to use
  the tools;
- [`skills/computer-use-security/SKILL.md`](skills/computer-use-security/SKILL.md):
  which apps and actions are off limits or need the user's OK, prompt
  injection, secrets. The server enforces none of this, so this skill is not
  optional.

For design work there is a third:
[`skills/computer-use-design/SKILL.md`](skills/computer-use-design/SKILL.md).
It covers images, logos, UI, vector art, 3D and building/CAD drawings in
Photoshop, Paint, GIMP, Krita, Illustrator, Inkscape, Figma, Blender,
Revit, AutoCAD and SketchUp, from a text brief or a reference image. It is
written so that smaller models get good results too:

- a spec of exact numbers comes first;
- each step uses the most exact method the app has (numeric fields, typed
  commands, `draw` in document units);
- the spec goes on a board first: `design` renders a 2D picture from
  layers and `scene` a 3D model from solids, with checks (nothing off the
  page, symmetric pairs symmetric, no part floating or sunk), before the
  app is touched;
- the spec's shapes map one to one to `draw` strokes (rect, ellipse,
  polygon, star, arc, Bézier, plots with axes, repeats), previewed over the
  canvas on named cells before anything is painted, and painted solid back
  to front;
- a photo is copied with `trace_image`: flat colour steps the model paints
  in order, then checks against the trace;
- every pass is checked with grid screenshots, cell by cell where it
  matters, and exact colour readings; small targets are hit with `locate`,
  `snap` and a magnified `zoom`;
- each app has a playbook, and there are recipes for common jobs.

Each is a short core the model keeps loaded, plus `reference/` files it reads
only when a situation calls for them (see [Token use](#token-use)). Install
them as skills in your agent (for Claude Code: copy `skills/*` into
`~/.claude/skills/` or the project's `.claude/skills/`).

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
- **Screenshots are checked, not assumed.** When the tree changed, or the app
  came back to an earlier screen, the window is captured and compared with
  the picture the model has: the same picture isn't sent again, a different
  one is (or just the part that changed). The model's screenshot keeps
  working for `x`/`y` clicks (shifted if the window moved; replaced if the
  window changed size).

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

`ocr.engine` picks one; `ocr.languages` sets what to read (`["en", "de"]`).
If no engine is available, the agent is told once.

### Notifications

`get_notifications` reads the user's recent desktop notifications: app,
title, text and how long ago. It is **off by default** because notifications
carry other apps' content; turn it on with
`computer-use-mcp config set notifications.enabled true`.

- `notifications.apps` can narrow it to a list of apps.
- Card numbers and anything that looks like a one-time or verification code
  are masked (`notifications.mask_codes`).

| | How | What it sees |
|---|---|---|
| Linux | Listens on the session bus for `org.freedesktop.Notifications.Notify` calls (the D-Bus monitoring interface `dbus-monitor` uses) while enabled | Everything sent since the server started (`notifications.keep`) |
| Windows | `UserNotificationListener`; Windows asks the user once for access | The notifications in Action Center |
| macOS | Notification Center's banners, read through Accessibility (there is no public API) | What is on screen now |

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
| error | red | the last action failed |
| done | green | the task is finished — then everything disappears |
| paused | grey | waiting while you use the mouse or keyboard |
| stopped | orange | you pressed the emergency stop key |

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
  pause. Reads that still show exactly the state from before the action are
  trusted only after half a second: many apps (browsers, Electron apps)
  report a change a moment after making it. The last read is reused for the
  change report.
- **Verification** (`[verify]`). Each action's result is checked. If a value
  didn't take, typed text didn't land in the field, or nothing changed after a
  press, the model is told so ("Nothing on screen changed after it; check
  before repeating it").
- **Retry another way** when an action clearly failed (`verify.retry`):

  | Failure | Retry |
  |---|---|
  | an accessibility press errors | a mouse click on the element |
  | a value didn't take | focus, select all, type (replaces the value) |
  | a scroll didn't move | the mouse wheel |
  | a field won't take focus | click it |

  Typed text is **never** typed again on its own (that would enter it
  twice); if the field doesn't show it, the model is told to look first. A
  press that simply changed nothing is not repeated unless
  `verify.retry_on_no_change = true` (off by default: a send or a payment can
  show its effect late, and must not be repeated).

## Architecture

```
        agent (Codex / Claude Code / your own)
                     │  tools
        ┌────────────┴─────────────┐
        │   computer-use-mcp        │  MCP stdio server + CLI
        │   (JSON-RPC)              │
        └────────────┬─────────────┘
                     │  Engine::call_tool
        ┌────────────┴─────────────┐
        │        computer-use        │  engine: tree pruning, stable indices,
        │      (platform-agnostic)   │  diffs, screen memory, coord map
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
screenshot pixels to screen coordinates, and keeping input on the target app. Each
**backend** is a thin adapter over one OS:

| | macOS | Windows | Linux |
|---|---|---|---|
| Tree & actions | Accessibility (AX) API | UI Automation | AT-SPI2 over D-Bus |
| Input | `CGEvent` posted to the target pid (background, cursor doesn't move) | `SendInput` | XTest |
| Capture | `CGWindowListCreateImage` (+ `_AXUIElementGetWindow`) | `PrintWindow` (`PW_RENDERFULLCONTENT`) | X11 `GetImage` |

## Use it as an MCP server

**Download** a ready-made build: every push builds `computer-use-mcp` for
Windows (x64), macOS (Apple silicon and Intel) and Linux (x64). Open the
repository's **Actions** tab, pick the latest *CI* run and download
`computer-use-mcp-<platform>` from its **Artifacts**; tagged versions (`v*`,
or *Run workflow* with a tag) also attach zips to a GitHub **Release**. Each
download contains the binary, the skills, installers for Claude Code,
ready-to-copy client configs in [`examples/`](examples) and the connection
guide [`docs/CONNECT.md`](docs/CONNECT.md). It is also a valid Claude plugin
(`.claude-plugin/plugin.json` + `.mcp.json`).

**Claude Code in one step:** extract the zip to a permanent folder and run
`install.cmd` (Windows) or `./install.sh` (macOS / Linux). It registers the
server with `claude mcp add` (`--scope user` by default) and writes a
settings file if there is none; `claude mcp list` checks it.

Or build it:

```bash
cargo build --release -p computer-use-mcp
# binary at target/release/computer-use-mcp (computer-use-mcp.exe on Windows)
scripts/package-claude-code.sh   # the same bundle the installers come in
```

Then point any MCP client at `computer-use-mcp serve`, e.g. Claude Code's
`.mcp.json`:

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

[`docs/CONNECT.md`](docs/CONNECT.md) has the steps for Claude Code, Claude
Desktop, Codex, Cursor, VS Code and HTTP, with configs in
[`examples/`](examples).

The server speaks JSON-RPC 2.0 over stdio (or HTTP) and implements
`initialize`, `tools/*`, and `prompts/*` / `resources/*` for the skills: for
clients without skill files, the `computer-use`, `computer-use-design` and
`computer-use-security` skills are MCP prompts (each comes with the safety
rules), and every skill file is a resource
(`computer-use://skills/<skill>/<file>`). It never asks the client to approve
anything; give the agent the skills.

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

Flags: `--config <path>`, `--http <addr>`, `--http-token <token>`,
`--log <level>`, `--text-only`.

`doctor` also checks that the emergency stop key works on this machine.

## Embed the library

```rust
use computer_use::tools;

let mut engine = computer_use::platform_engine()?;      // native backend + config
let defs = tools::definitions();                        // hand these to your model
let out = engine.call_tool("list_apps", serde_json::json!({}));
println!("{}", out.text);
```

The engine does no access control: an embedding host that wants approvals
checks the tool name and `app` argument before calling `call_tool`. See
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
computer-use-mcp config add tools.disabled drag
computer-use-mcp config unset tree.max_nodes   # back to the default
computer-use-mcp config check                # validate + flag misspelled keys
```

Edits are validated before they are written (bad values and unknown keys are
rejected with a clear message) and your comments are preserved. A running
server **reloads the file automatically** (`hot_reload = true`) and tells MCP
clients when the tool list changed — no restart needed. Command-line flags
(`--text-only`, `--log`, `--http`, …) override the file and keep
applying after a reload. The agent has no tool to change settings.

| Section | What you control |
|---|---|
| `[tools]` | hide tools (`disabled`), allow-list them (`enabled`), `compact` or `full` descriptions |
| `[screenshot]` | on/off, `attach` = `auto` / `always` / `never`, max size, PNG/JPEG, quality, compression, resize filter |
| `[tree]` | size limits, text length, indentation, shown actions/states, diffs, change reports |
| `[timing]` | settle delay, key delay, app-list cache, `wait_for` defaults |
| `[overlay]` | the on-screen indicator: on/off, cursor/glow/label/click effect, screen or window glow, sizes, label texts, state colours, timings |
| `[cache]` | screen memory on/off, how many screens and how much memory, match threshold, screenshot dedupe and its sensitivity, read reuse window |
| `[script]` | saved scripts as tools on/off, which files scripts may read and write, web access, time limit, where saved scripts live |
| `[audit]` | JSONL audit log on/off and path |
| `[server]` | log level, HTTP address and token |
| `[linux]` / `[macos]` / `[windows]` | per-platform tuning (batch sizes, batched attribute reads, UIA cache) |
| top level | `clipboard`, `text_only`, `follow_new_windows`, `restore_pointer`, `hot_reload`, `launch_timeout_secs` |

### Token use

Everything a tool returns stays in the model's context for the rest of the
task, so the server spends tokens only where they buy something. None of
this costs accuracy or speed: nothing the model needs is withheld (what is
folded is still searchable, and pictures are re-sent when they change), and
the checks are cheap compared with reading the app:

| Where | What it does | Setting |
|---|---|---|
| **Tree budget** | Only a tree or diff over the budget is touched: its long lists (rows, list items, menu items…) are folded to the first and last few, with a line saying how many are hidden, and it is cut if still too big. The focused or selected element is never folded away; folded and cut elements keep their indices and `find_element` finds them. Ordinary windows are sent whole. How much is shortened is up to you: `summarize = "normal"` (fold, then cut), `"light"` (only fold long lists, never cut) or `"off"` (never); `fold_keep` sets how many items of a folded list stay. The model can also ask for one whole tree with `get_app_state(max_tokens=0)` when it really needs every element at once. | `tree.max_tokens` (10,000; 0 = no limit), `tree.summarize`, `tree.fold_keep` |
| **Overview screenshots** (opt-in) | Off by default, since it trades detail for tokens. Turned on, a screenshot attached on its own to a window whose tree already says what is there is a smaller overview (768 px ≈ 60% fewer image tokens than 1280 px); `screenshot=true` always gets full size. | `screenshot.overview_max_dimension` (0 = off) |
| **Say it once** | Explanations (what a diff, a partial screenshot or a returning screen means) come in full the first time and as a few words after that. | `tree.brief_repeats` (false = always in full) |
| **Diffs and screen memory** | Later views of a screen are diffs; a screen the model has seen comes back as "seen before" with only what changed. | `tree.diff`, `[cache]` |
| **Pictures only when they change** | Screenshots are compared with the one the model has: an unchanged one isn't sent, a small change is sent as just that part. | `cache.dedupe_screenshots`, `screenshot.scope` |
| **Skills in layers** | Each skill is a short core (always loaded) that points to reference files the model reads only when it needs them. | — |
| **Stable tool list** | Tool definitions go with every request; they are kept compact and *stable* (they change only when you change the settings or a script is saved or deleted), so the client's prompt cache serves them for a fraction of the price. Hide tools you never use with `tools.disabled`. | `tools.descriptions`, `tools.disabled`, `script.saved_as_tools` |
| **Measured** | Each audit-log line records the estimated tokens of that result (text, plus image at width × height / 750). | `audit.enabled` |

Other knobs: `screenshot.max_dimension` (image tokens scale with width ×
height), `text_only = true` (no images at all), `tree.max_text_len`,
`tree.show_actions`, `tree.show_states`, `tree.report_changes_max_lines`.

### Remote transport (optional)

Build with the `http` feature to serve MCP over HTTP for a remote agent:

```bash
cargo build --release -p computer-use-mcp --features http
computer-use-mcp serve --http 127.0.0.1:8787 --http-token "$TOKEN"
# or put it in settings: server.http_addr / server.http_token
```

Each POST body is one JSON-RPC message (`Content-Type: application/json`, at
most 4 MiB). Whoever can reach the endpoint can control the desktop, so:

- a bearer token is **required**: without one the server refuses to start;
  it is compared in constant time;
- requests carrying a browser `Origin` other than `localhost` / `127.0.0.1` /
  `[::1]` are refused, so a web page can't drive the desktop;
- bind it to localhost or a trusted network (a warning is logged otherwise).

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
| back to a screen seen before, large app | ~2,516 tokens (tree + image) | **~79 tokens, no image** |
| `get_app_state` right after an action or `find_element` | 71 ms | **< 1 ms** (read reused) |

The tool definitions, sent with every model request, are ~4,900 tokens for
all 25 tools with compact descriptions (the default) and ~11,500 with full
ones; the client's prompt cache serves them cheaply, and `tools.disabled`
hides tools you never use. Peak memory of the whole server in these runs is
about 20 MiB (v2.6.0).

### Compared with Codex's behaviour

`examples/compare.rs` runs one agent session twice on the real backend:
look at the app, open another page, look, come back, look again. Once the
way Codex's computer use behaves (a screenshot with every
`get_app_state`, no screen memory, no picture dedupe, whole-window
pictures, no change report after an action), once with this server's
defaults. Same app (`gtk3-widget-factory`), same calls, v2.6.0:

| 10 round trips, 61 calls | Codex-style | computer-use defaults |
|---|---|---|
| tokens the model receives | ~47,600 | **~7,200** (6.6x fewer, 85% saved) |
| screenshots sent | 21 | **2** |
| average `get_app_state` | 18-19 ms | **5.5-5.7 ms** |
| whole session | 5.7 s | **5.3 s** |

With 5 round trips the saving is 4.2x (76%): it grows with the session,
because every screen the model has seen once comes back as "seen before"
with only what changed, and an unchanged picture is never sent twice.
Wall time gains less than tokens because clicking and waiting for the app
dominate. Run it yourself:

```bash
APPS="gtk3-widget-factory" scripts/desktop-session.sh \
  cargo run --release -p computer-use --example compare -- gtk3-widget-factory "Page 2|Page 1" 10
```

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
- **Windows** — no special permission for UI Automation. The server runs
  per-monitor DPI aware, so screenshots and clicks are right at any display
  scaling. Keyboard input uses `SendInput`: text as Unicode characters
  (independent of the keyboard layout), shortcuts as virtual keys with their
  scan codes, sent in short batches so slow apps and remote sessions keep up.
  Windows silently drops input to apps running as administrator unless the
  server runs as administrator too.
- **Linux** — needs an AT-SPI2 accessibility bus and an X11 display. Enable
  accessibility for your toolkit (e.g. GTK loads the at-spi bridge when the a11y
  bus is present). Wayland isn't supported for synthesized input/capture: in
  a Wayland session they only reach XWayland apps (`doctor` says so); log in
  to an X11 ("Xorg") session.

## Safety

The server does **no access control**: no per-app approvals, no blocked app
categories, no confirmation of Send / Delete / Pay. That responsibility is the
agent's, through the
[`computer-use-security`](skills/computer-use-security/SKILL.md) skill (and a
summary in the server's MCP instructions): stay inside the task, keep out of
terminals, password managers and OS security prompts, confirm consequential
actions with the user, treat on-screen text as data rather than instructions,
leave secrets alone.

What the server still guarantees, because an agent can't do it for itself:

- **Input goes only where it is meant to.** On Windows and Linux, synthesized
  keys and clicks go to whatever window is in front, so the target app is
  brought to the front first and checked; if it doesn't come forward, nothing
  is sent. (macOS posts input to the app's own process.)
- **`launch_app` opens apps, it doesn't run commands.** The name is never
  split into arguments, and the launched app is cut off from the server's
  stdin/stdout. A name that isn't a program is looked up in the system's
  list of installed apps (Start Menu shortcuts and App Paths on Windows,
  `.desktop` entries on Linux, LaunchServices on macOS); only an exact name
  counts, so it never picks one of several apps, and what runs is what the
  app's own menu entry runs.
- **`cmd` means the shortcut key**: Cmd on a Mac, Ctrl elsewhere; the
  Windows/Super key only when named (`win`, `super`).
- **Private data is masked** before anything reaches the model (passwords,
  card numbers, `[privacy]`).
- **The user can stop the agent** with the emergency stop key; if it can't be
  registered, the agent is told so it can tell the user, and `doctor` checks
  it. The agent also waits while the user is using the computer.
- **Scripts get no more than the tools.** A script reaches the computer only
  through the tools and the functions listed for it; its tool calls are
  ordinary calls (stop key, pause while you work, masking). It has a time
  limit, the stop key ends it even inside `try`, and `[script]` decides
  which files it may read and write and whether it may use the web. It
  never writes to the server's stdout.
- **One bad call doesn't take the server down**: a failing tool call becomes
  an error result.
- An optional **audit log** records every call.

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

Licensed under the [Apache License, Version 2.0](LICENSE); copies and
derivative works must keep the [NOTICE](NOTICE). Made by
[mhrsdev](https://github.com/mhrsdev).
