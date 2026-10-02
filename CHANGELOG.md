# Changelog

## v3.7.0

Fewer tokens on every request, less work done twice, and every
token-saving setting on by default. The tool list a model gets with
every request is less than half what it was, without taking any tool
away: the tools most tasks don't need are found when they are needed.
Upgrading: [docs/MIGRATING.md](docs/MIGRATING.md#upgrading-to-v37).

Measured (scripted, three runs each, every figure an estimate; the
releases run as they ship, over MCP, with their own skills:
[bench/results/v3.7-releases](bench/results/v3.7-releases/step.md)):

| | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| sent with every request (instructions + skills + tools) | 4,063 | 6,718 | 7,783 | **4,938** |
| of it, the tool definitions | 1,834 (22 tools) | 4,363 (24) | 4,990 (25) | **2,167** (17 listed) |
| input over form, table, orders and long (no cache) | 248,228 | 349,873 | 367,006 | **250,701** |

Against v3.6.0: −37% per request, −32% over those tasks (−31% over all
six with `batch`); against v3.0.0 −26% and −28%. v0.1.0 had a third of
the tools (no drawing, design, 3D, `locate` or scripts: it can't do the
two canvas tasks) and sends about as much (+1% for v3.7).

Against Codex's behaviour, simulated on this server (a screenshot with
every look, no screen memory, no change report, every tool listed; not a
run of Codex): over five tasks −33% input, 5 screenshots instead of 11,
and −50% with `batch` ([bench/results/v3.7-codex](bench/results/v3.7-codex/compare.md)).

### On by default

What v3.6 added behind settings is on now; each can be turned off:
lean tool schemas (`tools.descriptions = "lean"`), action reports of the
changes around what was acted on (`tree.report = "relevant"`), elements
that keep changing summed up (`tree.quiet_volatile`), fewer automatic
pictures while the model doesn't use pixels (`screenshot.adaptive`),
`locate` without its picture (`screenshot.locate_picture = false`), blind
areas read and watched (`ocr.blind_regions`), paint steps when asked
(`tools.design_steps = "asked"`) and `_meta` for hosts that trim their
context (`server.result_meta`).

### The tool manager, on by default (`tools.manager = "dispatch"`)

- The model gets the tools most tasks use (looking, finding, waiting,
  clicking, typing, keys, scrolling, dragging, `batch`, `screenshot`),
  plus `find_tools` and `use_tool`. Drawing, the design board, 3D,
  `locate`, windows, scripts, the clipboard, notifications and decisions
  are found by name, category or what they do, and run through
  `use_tool`. The list never changes, so a client's prompt cache holds;
  nothing hidden is forbidden.
- `find_tools` names every tool it can find, by category, so one can be
  asked for by name (`find_tools(name="locate")`); a query ignores words
  that name nothing and returns the best three; a tool's arguments are
  shown once (`again=true` repeats them).
- A hidden tool called with wrong arguments gets its arguments with the
  error, once: no `find_tools` needed first.
- `decide` is listed once a decision model is set up (with compact
  descriptions too). `window` no longer repeats its list of actions.
- A small preset, or an `enabled` list of base tools only, gets no
  manager. `tools.manager = "off"` lists every tool, as before.

### Instructions and skills

- The instructions and `get_app_state` no longer say to look "on every
  turn": look first, then read the state each action returns, as the
  skill says. A look the report already gave costs a whole request.
- `--instructions full|short|off`; the Claude Code plugin, which brings
  the skills, starts the server with short ones (−394 tokens a request).
- The `computer-use` skill: the tool manager in one place, the rules the
  security skill makes referred to, the decision model in a few lines
  (details in `reference/decisions.md`). The design skill says where its
  tools are.

### Results said once

- A diff's intro: in full once, its legend once more, then "Changes:".
- "get_app_state shows them", "call get_app_state for the rest", draw's
  preview legend, the loupe's and a zoomed screenshot's notes: in full the
  first time, then short (`tree.brief_repeats = false` keeps them whole).

### Less work done twice

- The tool lists are built once and kept until a setting, a saved script
  or a found category changes them; the server no longer rebuilds them
  after every request (a ping included).
- Waiting after an action: reads that still show the state from before
  come further apart, so an action that changed nothing walks the tree
  fewer times.
- Blind areas' OCR is kept per area, by its exact pixels: a caret or a
  clock elsewhere no longer makes every area be read again. Tesseract's
  two readings of an area run side by side, one thread each
  (`OMP_THREAD_LIMIT=1`, unless set: two processes' OpenMP threads
  spinning against each other took 20 s for a 0.15 s reading).
- `wait_for(until)`: the window just looked at is the one asked about,
  and the same text isn't asked about again for 5 s.
- `batch` doesn't make the steps' own change reports (never shown).
- `screenshot(app)` uses the picture this call's look already took.

Time (tools only, scripted): orders 1.5 s → 1.2 s, board 1.5 s → 1.2 s
(a look that reads painted text ~0.9 s → ~0.6 s), the others as before.

### Benchmark

- `agent_bench --server BIN [--server-args …] [--server-config FILE]
  [--skills DIR]` runs the scenarios against any release over MCP, with
  its own instructions and skills, and reports the fixed prefix by part.
- `bench/compare.py` puts runs side by side, section by section.

### Fixes

- Text read off the screen that is the on-screen indicator's own label
  ("Zero is thinking…", even misread) is never taken for the app's text:
  where a capture catches the indicator, blind areas read it.
- A just-started indicator helper gets up to a second to hide before the
  first picture (then 150 ms, as before).

Costs, published with the gains:

- Blind areas on by default read canvases and painted text on every look:
  with `batch`, the six tasks take 15.7 s of tool time instead of 13.7 s
  (`ocr.blind_regions = false` for the old speed). Step by step, 11.1 s
  against 11.0 s.

- A tool the model doesn't see takes one `find_tools` call the first time
  (about 155 tokens of result): shapes and board need 4–5 calls instead
  of 3–4 when they use `locate`. Wrong calls and retries a real model
  makes with the manager are still to be counted (real-model runs).
- The full instructions grew by 53 tokens (they say how hidden tools are
  found).
- Board's OCR reading varies from run to run in v3.6 and v3.7 alike (one
  run in three, or two, falls back to `locate`).

## v3.6.0

Everything the [roadmap](docs/ROADMAP.md) planned for v3.6 to v4.0, in
one release, on top of v3.5's measuring: trees and reports that say each
thing once, OCR that suits the content, a lighter tool surface, actions
that check their result and need fewer round trips, design answers that
say what changed, and optional layers no feature depends on. Upgrading:
[docs/MIGRATING.md](docs/MIGRATING.md).

What it doesn't settle yet, and the roadmap keeps open: the real-model
A/B runs that decide which of the opt-in settings become defaults (the
benchmark is ready for them, a key is all it needs), and tests on real
Windows and macOS hardware (the Windows and macOS code builds and passes
its unit tests in CI, as before).

### Trees and reports: each thing said once (`tree.compact`, on)

- Look-alike siblings are **records**: the roles once (`7 × button:`,
  `12 × list item › text:`), then each one's index and what is its own;
  a **table** is its column names once and a row a line. Folding keeps
  whole rows.
- Flags and actions the role implies are left out: a text field is
  editable unless it says `read-only`; a checkbox toggles.
- Diffs list elements added together under their parent once, and many
  removed ones as ranges of indices. A look at a window as it was names
  only the app and window; a look that changed nothing is one line.
- `set_value` doesn't echo a value the field now shows.
- `tree::expand` and `tree::index_of` read records back one a line, for
  code that reads trees.

### Looking at less

- `get_app_state(within=index)`: one element and what is in it, without
  moving the base of later diffs.
- `get_app_state(about="…")`: only the parts of the window about that,
  the others folded to a line each; by the words (names and values), or
  by the decision model's judgment when one is set up.
- `find_element(offset)` pages through many matches.
- `tree.report`: `"full"` (default), `"relevant"` (the changes around what
  the action acted on, added elements, the focused one, and a count of the
  rest) or `"brief"` (how many, and a new screen). `tree.quiet_volatile`
  (off): elements that keep changing on their own summed up in a line.
- When the pixels change where the tree reports nothing (GTK keeps a
  filtered table's old cell text over AT-SPI), `get_app_state` says where,
  and to trust the screenshot there.

### OCR

- Every reading takes out thin lines that cross most of the picture (graph
  paper, rulers, table borders), then reads it as it is and enlarged,
  keeping the surer line of each place: the benchmark's board now reads
  all eight labels.
- A strip of one or two lines is read as a line (Tesseract mode 7), the
  rest as sparse text; shapes and rulers' evenly stepped numbers are left
  out of every reading; a line read with low confidence says `unsure
  reading`.

### Pictures

- `screenshot.adaptive` (off): once the model has looked at an app a few
  times without using its pixels, a well-described window's automatic
  picture is held back (and the result says so once); any use of pixels
  brings them back.
- `locate` can answer without its picture (`picture: false`, or
  `screenshot.locate_picture = false`).
- `screenshot.icon_sprite` (off, experimental): with no screenshot
  attached, a strip of the buttons that have no name, numbered with their
  indices, each shown once.

### The tool surface

- `tools.descriptions = "lean"`: nested objects as the list of their keys,
  no `window` (still accepted) and no defaults in the schemas, `decide`
  only once a decision model is set up.
- `tools.manager` (off): `"dispatch"` shows the base tools and
  `find_tools`, which returns the others (by category or by what they do)
  with their arguments, run through `use_tool`; the list never changes, so
  any client and its prompt cache keep working. `"list_changed"`: a
  category found joins the list for good, and the client is told. Hidden
  is not forbidden: a direct call still runs, and the settings still
  decide what may run.
- `tools.preset = "small"`: ten tools for smaller models.
- `tools.default_app`: a call without `app` acts on the app last named.
- `server.instructions`: `"short"` or `"off"` for clients that load the
  skills. The computer-use skill is shorter and says the new ways.
- Definitions per request (estimated): full 13,088 tokens, compact 4,986,
  lean 3,715, small 1,049.

### Actions: checked, and fewer round trips

- **`expect`** on `click`, `set_value`, `type_text`, `press_key` and
  `perform_secondary_action`: `"dialog"`, `"menu"`, `"change"`,
  `"value"`, `"gone"`, or a text that should then be on screen. The server
  waits for it (up to `timing.expect_wait_ms`, 2 s) and says on the
  action's line: confirmed, not seen, or uncertain (and not to repeat what
  isn't confirmed without looking).
- **`click(name, role)`**: when one element has that name (an exact name
  wins over a part of one); several are listed and nothing is clicked.
- **`batch` lines**: `click 12`, `click "Save"`, `double 12`, `right 12`,
  `set 4 "Ada"`, `type "text"`, `key cmd+s`, `scroll 7 down 2`, `select 4
  "word"`, `action 9 show_menu`, `wait "Saved"`, `find "Total"`, `look`,
  each with an optional `expect …`. A batch stops when a step brings up a
  window it didn't expect (`through_windows: true` goes on) or what a step
  expected isn't confirmed, and ends with **one report** of what the steps
  changed.
- **`launch_app`** returns the app's first state (`tools.launch_look`).
- A **table cell** whose press changed nothing is clicked with the mouse
  (GTK's press doesn't select the row; selecting is safe to repeat).
- **Long conversations**: `cache.rebase_after_tokens` (off) sends a whole
  tree and a picture again once that many tokens have gone by since the
  app's tree was last sent whole; `get_app_state(rebase=true)` asks for it.

### Design and scene

- After the first answer, only what changed: the layers (objects)
  changed, added or removed and the new order; checks and paint steps
  "as before"; no picture when its pixels are the ones sent last. A call
  that changes nothing gets everything.
- `tools.design_steps = "asked"` (default `"always"`): the paint steps
  only with `show: {"steps": true}`.
- Layers, scene objects and `draw` strokes can be **lines**, the way the
  listings show them: `"sun ellipse 80 20 12 12 fill #ffcc00"`, `"seat box
  0.5 0.5 0.05 at 0 0 0.45 color #884422"`, `"rect 10 10 50 30"`.

### Optional layers and hosts

- `decide(app, pick, read=true)`: the element's whole text or value, read
  by the server (extraction without reading the window).
- Linux: a control without a name gets the name of the label it is
  labelled by (AT-SPI relation; GTK's mnemonic labels). The benchmark's
  form fields are `text field "Full name"` now, not a nameless field.
- `server.result_meta` (off): each tool result's `_meta` has a number and
  the earlier results it repeats whole
  (`zero-use-computer/supersedes`) or whose pictures it replaces
  (`zero-use-computer/supersedes-images`), so a host that trims its
  context can drop them.

### Benchmark

- `--plan batch`: scripted ways through that use batch lines, clicks by
  name and `expect`, next to the one-action-a-call way (`--plan step`).
- `bench/configs/lean.toml` (every opt-in that saves tokens) and
  `manager.toml` (the same with the tool manager), for A/B runs.
- Scripted results (estimates, all 90 runs successful): over the six
  tasks, result tokens −25% with the defaults and −35% with lean settings
  and batches against v3.2; calls 42 → 27 with batches; the 300-row
  table −39%, the canvas with painted labels −41% (one picture instead of
  two). Costs: the fixed prefix +5% with the defaults (~6,941 → ~7,309
  tokens; lean ~6,040, with the manager ~4,397), half a second per look
  at a canvas with blind areas on. [bench/results](bench/results/README.md).

## v3.5.0 (released as part of v3.6.0)

The first step of the road to v4.0 ([roadmap](docs/ROADMAP.md)): spend
fewer tokens on finishing a task, measured with the real model before
anything is turned on by default.

### Measured, not assumed

- **`bench/`**: an agent benchmark. Six tasks in real apps (a form, a
  300-row table, a canvas with painted labels under a full toolbar, a
  canvas without text, a mixed form-and-dialog task, a longer session),
  from one GTK fixture app that records what was done: success is checked
  from the app, never from what the model says. Each run starts the app
  afresh.
- **Real runs** drive Claude through the Messages API (`curl`, the key on
  its input) and record every request's input, output, cache-read and
  cache-write tokens, the model that served it and the cost; `--calibrate`
  counts each tool result's real tokens to check the server's estimates.
  **Scripted runs** need no key and give estimates, labelled as such.
- `bench/results/v3.2` is the scripted v3.2 baseline, taken before any
  change below; `bench/results/README.md` sets every change against it,
  costs included.
- The README's comparison with Codex is labelled for what it is: a
  simulation of Codex's behaviour with this server, with estimated tokens.

### Screenshots

- `x`/`y`/`width`/`height` without a mode is a region of the screen. With
  another mode, or with `app`, it is an error that says what to use
  instead; they used to be silently ignored.
- `screenshot(app)` goes the way `get_app_state`'s picture goes: nothing
  when the window looks as in the model's last picture of that screen,
  only the part that changed when that is small, else all of it, which
  then is the picture `x`/`y` refer to. `mode: "window"` still always
  sends all of it.
- Every window and full-screen screenshot is numbered ("Screenshot #7"),
  and a changed part names the one it patches.

### Blind areas (opt-in: `ocr.blind_regions`)

- How many elements a window has no longer decides alone whether its text
  is read: the areas the tree says nothing about (an element with nothing
  informative in or under it, such as a drawing area or a picture, and
  what no element covers) are found however full the rest of the window
  is, and kept only if they show something, never the empty margins of a
  form.
- Just those areas are read off the screen; Tesseract reads each as it is
  and enlarged and keeps the surer line of each place (enlarged alone, a
  canvas's framed labels came out as "Ecce"), and leaves out lines that
  look like shapes rather than text.
- `get_app_state` says once per screen that part of the window has no
  accessibility information, and checks those pixels on every look (an
  unchanged picture is still not sent again).
- Scripted benchmark: the canvas task takes 2 calls and 1 picture instead
  of 4 and 2 (−47% result tokens), the mixed task 4 calls instead of 5;
  it costs about half a second of reading on canvas windows. Off until a
  real-model run shows it doesn't cost success.

## v3.2.0

A decision model for speed, set up with Ctrl+Alt+J; apps without
accessibility on Linux; fixes from testing on Hyprland with Firefox
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.1.0...v3.2.0)).

### Decision model (Jev)

- **`decide`**: typed answers from a fast decision model — yes/no (the
  probability of yes), one of some options, a score on a scale — about
  text or JSON, **each of many items in parallel** (with a summary), or an
  app's window (which never enters the conversation). `pick` returns the
  `element_index` of the element a description means.
- **`wait_for(until=…)`**: waits until the model answers yes about the
  window ("have the results loaded?").
- **Scripts**: `ask`, `choose`, `score`, `decide`, `decide_each`.
- Speaks **TypeSafe's System One API** (Jev, and servers that speak it:
  local-jev, jeff, LiteLLM) and any **OpenAI-compatible** chat API (a JSON
  answer from a small, fast model).
- **Ctrl+Alt+J** (`control.settings_hotkey`) opens a settings page in the
  browser, from any app: kind of model, address, model, key; Test and
  Save; used at once. Registered on X11, Hyprland and sway (a compositor
  binding), Windows and macOS like the stop key. `computer-use-mcp
  settings` opens it too. The skill suggests it once when a task would
  gain from it; the agent sets the model from the chat only when the user
  asks.
- The key stays private: kept in `config.toml` (then readable by its owner
  only), given to `curl` on its input, never shown back (the page,
  `doctor`, `config show/get` and the tool show its last four characters).
  The page lives on 127.0.0.1 at a random address, refuses other sites and
  host names (DNS rebinding), and closes after 15 minutes unused.
- `doctor` reports the settings key and asks the model a test question.

### Linux

- **Apps without accessibility** (terminals such as foot, kitty, xterm;
  some Electron apps) are listed from the compositor (Hyprland, sway) or
  the window manager (X11, including apps that never say their pid, found
  through XRes), and their windows are used through screenshots (and OCR
  when Tesseract is installed), the mouse and the keyboard. Before, they
  only showed up when in front, and then as an error.
- "Desktop" (listed when no app window has the keyboard) now says what it
  is and what to do instead, not "not supported on this platform".
- Browsers' pages show their address (Firefox's `DocURL`, Chromium's
  `URI`) as the value of the web area.
- Actions without a proper name (Firefox gives some a key binding as their
  name, `;;`) are named from their description, or dropped; an unnamed
  default action is still clickable.
- A Wayland desktop that keeps input and screenshots from other programs
  (GNOME, KDE Plasma) is named once in `list_apps`, with what still works.

### Everywhere

- `launch_app("https://…")` opens a web address in the default browser.
- Browser notes in the skill: one way to enter an address (never set and
  typed), the address in the tree, Wayland, and never working around the
  tools with ydotool or wtype.

## v3.1.0

Wayland: Hyprland, sway and the other wlroots-family compositors work in
full, and the overlay stops flickering under compositors on every system
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v3.0.0...v3.1.0)).

### Hyprland, sway (Wayland)

Before, in a Wayland session only accessibility actions worked for native
Wayland apps: clicks at a place, typing, screenshots and window changes
only reached XWayland apps, element positions were off (a Wayland app
only knows where things are inside its own window), and the stop key
mostly didn't fire.

- **Windows come from the compositor** (Hyprland's or sway's IPC): where
  each window is, which has the keyboard, and focusing, moving, resizing,
  maximizing, minimizing, full screen, closing and moving to a workspace.
  Element positions are placed on screen from there, allowing for the
  shadow apps like GTK draw around their windows.
- **Input through the compositor**: clicks, drags, drawing and scrolling
  through wlr-virtual-pointer; keys and text through a virtual keyboard
  with a keymap made for what is typed, so any text (Persian, CJK, emoji)
  comes out exactly whatever the keyboard layout. XWayland apps too.
- **Screenshots through wlr-screencopy**, at each output's own scale
  (fractional scales included), across several outputs.
- **The user's idle time** from ext-idle-notify, so the pause while you
  use the mouse or keyboard works; without it the engine doesn't guess.
- **The overlay is a layer-shell surface**: made once, never unmapped
  (hiding for a screenshot shows a transparent buffer, in about 1 ms), so
  Hyprland never replays its open and close animations on it; sharp at
  fractional scales; click-through; on every output. Layer rules can
  target the `computer-use` namespace.
- **The stop key is bound in the compositor** while the server runs (and
  bound again if the compositor reloads its config), never over a binding
  of yours.
- `doctor` says what the compositor offers. Other Wayland desktops (GNOME,
  KDE Plasma) keep the previous behaviour: accessibility actions for every
  app, input and screenshots for XWayland apps.

Tested live on headless sway (the wlroots protocols Hyprland speaks too),
at scales 1, 1.25 and 1.5, and in CI. Hyprland's IPC is tested against a
simulated server: please report anything that behaves differently there.

### Overlay on every system

- **X11**: under a compositor (picom, xcompmgr, KWin…) the overlay faded
  in and out around every screenshot (sometimes still visible in it) and
  got shadows; without one, it flashed black when shown again. Its
  windows now stay mapped and are emptied instead; the screenshot waits
  until the compositor has drawn that; images are in place before a
  window shows; compositors are asked for no shadow, and the windows are
  named `computer-use-overlay` for compositor rules.
- **macOS**: a border around a window on a secondary display was cut off
  entirely; the overlay now spans every display. Window animations off,
  and it stays up when another app hides the others.
- **Windows**: no show/hide animation and no rounded corners on
  Windows 11.
- Reloading the settings no longer hides and re-shows everything.

## v3.0.0

Stability first: no call can hang the server or run for hours, a busy or
hung app is told apart from one that failed, and an action that was sent
but not answered is never repeated. Fewer tokens and less memory on top
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.6.0...v3.0.0)).

### Stability, everywhere

- **An action sent but not answered is never done twice.** A press, value
  or secondary action the app didn't answer in time (busy, hung, a dialog
  opened) is reported as "sent, may or may not have happened; look before
  repeating it", and is not retried with a mouse click or by typing.
- **Clients can cancel.** The server reads its input on a thread of its
  own: `notifications/cancelled` ends the call that is running (a long
  drawing, a script, a wait) as the stop key would, and a request cancelled
  before it starts never runs. What a dropped answer would have shown is
  sent in full by the next look.
- **Every call is bounded**: `type_text` up to 100,000 characters (typed in
  pieces, the stop key checked between them), `press_key` 500 presses,
  `scroll` 50 pages, `get_clipboard` 30,000 characters, design pages 500
  layers, pictures 50 megapixels; huge angles, brushes and noise pictures
  no longer stall drawing, scenes or pixel searches; calls nest at most 8
  deep and a script can't start another script.
- **Subprocesses can't hang it**: Tesseract runs with a 60 s limit, `curl`
  is killed by the stop key or a cancel, and every child is waited for.
- **Files**: scripts read and write only regular files (never a device or
  a pipe); settings are written atomically, and a settings file caught
  half-saved is not taken; the audit log rotates at 10 MB.
- **Coordinates follow a window that moved**, so the model's screenshot
  still names the right places.
- The overlay helper's command queue is bounded; a helper that stops
  reading is replaced. Refused HTTP requests are answered off the main
  loop. On Windows, apps the server starts no longer inherit its stdio
  pipes (the client always sees the server end).

### Windows

- UI Automation calls are bounded (CUIAutomation8 timeouts, a 10 s budget
  per tree read that returns what it has); VARIANTs are freed.
- A hung window (`IsHungAppWindow`) is never waited on: window changes are
  asynchronous and checked, captures of it come off the screen (only when
  nothing covers it), and its actions report "not responding".
- Focus works past the foreground lock without stray key presses; a
  minimized window must be restored before input goes to it.
- Points off every monitor are refused; the wheel is clamped; ghost
  windows (cloaked, overlays, untitled tool popups) are left out; the
  pointer is put back after the click lands; handles of closed apps are
  dropped; capture fixes (DC leak, GdiFlush, overflow); notification reads
  are bounded; COM runs multithreaded, with shell launches on a short STA
  thread.

### macOS

- Accessibility calls are bounded on every element (a system-wide
  messaging timeout); an app that times out ends the tree read at once
  with what was read, and presses that time out are "sent, not answered".
- Every Objective-C call runs in an autorelease pool (no growing memory in
  a long session); Screen Recording permission is checked and reported.
- Clicks, drags and scrolls carry no held modifier keys, and scrolls land
  at the target point; focus also sets AXFrontmost and waits for full
  screen to end before moving a window.
- Values are type-checked before use; pixel formats are read as they are;
  concealed clipboard content (passwords) is refused; the overlay follows
  screen changes and exits if its parent is gone.

### Linux

- Keys go only to the app they are for: the window that has the keyboard
  comes from the window manager (`_NET_ACTIVE_WINDOW`), a terminal or
  other app without accessibility counts as in front, and focusing checks
  the window really got the keyboard.
- Typing works whatever keyboard layout is active (the group is locked to
  the first layout and Caps Lock released while typing, then restored).
- AT-SPI calls are bounded: a press or value the app doesn't answer is
  "sent, not answered"; slow elements are retried in smaller batches; a
  tree read returns what it has after 8 s; a busy app (a modal dialog
  open) stays listed.
- Huge lists (a 100,000-row table) are read by index, first 256 rows:
  0.97 s instead of 8.2 s.
- A broken AT-SPI or X connection is replaced on the next call; the X
  event queue is drained and the keymap reloaded when it changes.
- Clipboard helpers give up after 3 s and are always reaped; `wl-*` only
  under Wayland. Under Wayland, input to apps without an XWayland window
  is refused instead of going astray.
- Actions a toolkit names with a sentence (GTK's table cells) get short
  names.

### Fewer tokens

- After an action's result showed the start of a new screen, the next
  `get_app_state` sends only the rest.
- A changed line in a diff ends with only what differed (`…unchecked)`),
  not the whole old line.
- How-to paragraphs in `design`, `trace_image` and OCR headers are said
  once per session.
- Tool definitions share repeated schemas (compact mode).

Measured with `examples/compare.rs` (gtk3-widget-factory, 10 round trips,
61 calls): **6,821 tokens** against 46,280 the way Codex's computer use
behaves (**6.8x fewer, 85% saved**; v2.6.0: 7,164), 2 screenshots instead
of 21, `get_app_state` in ~5.6 ms instead of ~17 ms. Tool definitions:
~4,400 tokens per request (v2.6.0: ~4,900).

### Less memory

- Fonts are memory-mapped instead of read into the heap; freed memory is
  given back to the system after big calls.
- Measured over a session (look, screenshots, design, scene, script):
  the server holds **14.7 MiB** (v2.6.0: 24.3), the overlay helper 10.9
  MiB (13.1), peak 22 MiB.

### Notes

- Linux is tested live (Xvfb, AT-SPI, GTK). The Windows and macOS changes
  are type-checked and reviewed but not yet run on real hardware: please
  report anything that behaves differently.

## v2.6.0

Scripts: the model writes a small program and the server runs it, for
what the tools can't do in one call, on the graph-paper page or anywhere
else; and a saved script becomes a tool of its own
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.5.7...v2.6.0)).

### New

- **`script`** runs a program in [Rhai](https://rhai.rs), a sandboxed
  language that reads like JavaScript. A script can:
  - run any tool with logic around it: `tool(name, #{...})` gives the
    tool's text and stops the script on a failure, `try_tool` gives
    `#{ok, text, image}`, `set_app` fills in the app;
  - read the screen as data: `elements(app, #{role, name, text})` gives
    the matching elements with their states and boxes, `colors(app,
    points)` the exact pixel colours;
  - draw on **the graph-paper page**: `page(name, w, h, #{cell: size})`
    is a design-board design with named cells. `p.fill_cell("C4",
    colour)`, `p.text_in("C4", text)`, `p.cell("C4")`, `p.at(x, y)` and
    every shape and text the board has (`rect`, `circle`, `line`, `path`,
    `polygon`, `star`, `arc`, `curve`, `text`, `layer`). The picture comes
    back with the result, and `p.steps()` and `p.export("png")` take it
    into an app. `cells(w, h, size)` gives the same cells for any canvas;
  - use data: `data` and `args` from the model; files (`read_text`,
    `read_json`, `read_csv`, `write_text`, `write_json`, `write_csv`,
    `list_files`); the web through `curl` (`fetch`, `fetch_json`,
    `download`); `remember`/`recall` between runs; JSON and CSV, regular
    expressions, `numbers(text)`, maths that takes whole numbers too,
    random numbers, colours (`rgb`, `hsl`, `mix`) and dates;
  - run saved scripts (`run(name, args)`) and import them as libraries.
- **Saved scripts are new tools.** `script(save=name, code, description,
  params)` checks the script and keeps it in
  `~/.computer-use/scripts/<name>.rhai`, plain text the user can edit. It
  is then a tool of its own with its own arguments (the server tells the
  client its tool list changed), and `run`, `list`, `show` and `delete`
  manage saved scripts.
- **Errors say where.** A script that doesn't parse is refused before
  anything runs; misspelt variables are caught then too. A failure gives
  the line and its code, and what the script printed before it.
- **`cell_size`** on `design`, `draw` and `screenshot` fixes the cells'
  size, so all of them (and a script's page) name the same cells: an 800 x
  800 board with `cell_size: 100` is a chessboard, A1 to H8.
- **`[script]` settings**: saved scripts as tools on or off; which files
  scripts may use (`none`, `workspace`, `read` — the default: read any
  file, write in the scripts' own folder — or `all`); web access; the
  time limit (`max_seconds`, 300); the scripts' folder.

### Safety

- A script reaches the computer only through the tools and its listed
  functions. Its tool calls are ordinary calls: the stop key, the pause
  while the user works and private-data masking apply. The stop key and
  the time limit end a script even inside `try`, and a script never
  writes to the server's stdout.
- The security skill covers scripts: read only the files the task needs,
  write in the scripts' folder unless asked, fetch only what the task
  needs and never send the user's data to a site unless that is the task,
  no secrets in saved scripts or memory, and a loop of consequential
  actions still needs the user's go-ahead.

### Other

- **License: Apache-2.0** (it was MIT or Apache-2.0), with a NOTICE file
  that copies and derivative works must keep. The release zips include
  both. The project now lives at
  [github.com/mhrsdev/zero-use-computer](https://github.com/mhrsdev/zero-use-computer).
- What the server writes says where it comes from: exported SVG, PNG and
  OBJ files name computer-use, as do `--help`, `doctor`, the MCP server
  title and the web requests scripts make.
- `examples/compare.rs` measures one agent session in Codex's behaviour
  and with this server's defaults: over 10 round trips the model gets
  ~85% fewer tokens (6.6x) and 2 screenshots instead of 21 (README,
  "Compared with Codex's behaviour").

### Docs

- New reference: `skills/computer-use/reference/scripts.md` (also
  `script(help=true)` and an MCP resource): the language in short, every
  function, the page, saving tools, examples and rules. The README, the
  skills and the server's MCP instructions mention scripts.

## v2.5.7

See a drawing before it is built: a design board for 2D, a scene for 3D,
graph-paper cells to place and check every part, and exact aiming at small
targets
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.5.1...v2.5.7)).

### New

- **`design`**: a design board, like Canva. A picture is built from layers
  (the shapes `draw` takes, and text) that can be added, changed, removed,
  mirrored (the other eye or ear), aligned, distributed and reordered. Each
  call returns:
  - the rendered picture;
  - every layer with its box and colours;
  - checks: off the page or into the margin, nearly centred, pairs not
    quite symmetric, hard-to-read or overlapping text, too many colours;
  - the steps to paint it.

  `show` adds a grid, the layers' ids or guides.
- **`draw` strokes `{"design": name, "step": n, "fill": w}`** paint one
  colour step of a design, fitted into the canvas.
- **`scene`**: a 3D model planned as solids before it is built in an app.
  - Solids: box, cylinder, sphere, cone, torus and plane, each with a
    size, a centre and a rotation (Z up, the ground at z 0).
  - Editing: add, change, remove, mirror and repeat (in a row, or around
    an axis). `on` sets one part on top of another.
  - The picture: the front, right and top views to one scale with a grid,
    and a perspective view with shadows straight down.
  - Checks: parts that float (and how far above what), sink into the
    ground or run into each other (and how deep).
  - Building it: the numbers for Blender's Location, Rotation and
    Dimensions fields, or for any other app.
- **Exports are temporary.**
  - `design` writes SVG or PNG; `scene` writes OBJ (with an MTL file of
    its colours) or PNG.
  - They go to `computer-use-exports` in the system's temp folder and are
    deleted when the server stops.
  - Files older than a day are removed, the folder is kept under 200 MB,
    and nothing is ever overwritten.
- **Cells: graph paper for drawing.** The area is cut into square cells
  named like a chessboard (columns A, B…, rows 1, 2… from the top left),
  sized so what is drawn spans about eight of them.
  - The `draw` preview shows them, and every `draw` result says which
    cells the drawing covers.
  - `screenshot` with `canvas` takes `cells: true` to lay them over the
    document, and `cell: "C4"` to magnify one cell with a fine grid in the
    document's units and its main colours.
  - The design board shows them too; `show: {"cell": "C4"}` opens one with
    the layers in it.
- **Pixel targeting:**
  - `screenshot` `zoom: [x, y]` magnifies around a point, each screen
    pixel a square, with a crosshair and a grid in click coordinates.
  - `locate` finds exact places in a window: every area of a colour, every
    look-alike of an icon or marker, or the exact corner, edge or centre
    next to a rough point.
  - `click` and `drag` take `snap` (`corner`, `edge`, `center` or a
    colour) to move onto that point before acting.

### Changed

- **The design skill** plans every drawing on the board first: `design`
  for 2D, `scene` for 3D.
  - A new reference page, `reference/board.md`.
  - Recipes for a badge painted from a design and a 3D model built from a
    scene.
  - Checks for the board, cell by cell and in 3D.
  - "From a scene" sections in the Blender and SketchUp playbooks.
- **The computer-use skill** covers cells, `locate`, `snap`, `zoom`,
  `design` and `scene`.
- **The security skill**: exported files are temporary; finished work is
  saved only where the user asked.

## v2.5.1

Better drawing and copying of pictures, from a test where a smaller model
copied a photo of a cat in Paint
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.5.0...v2.5.1)).

### New

- **`trace_image`**: turns a reference picture (an image file, or part of
  a window) into a few flat colours and shapes, as numbered steps to paint
  back to front. It returns the steps and a picture of the result; `colors`
  and `detail` set how close it comes.
- **`draw` strokes `{"trace": name, "step": n, "fill": w}`** paint one step
  of a trace, fitted into the canvas with its proportions kept.
- **`fill: w` on any closed `draw` stroke** paints it solid with a brush
  `w` wide instead of its outline. The brush stays inside the edge, and
  shapes painted back to front cover each other. No bucket fill, so
  overlapping shapes come out right even in apps without layers. The
  result warns when shapes are narrower than the brush.
- **`screenshot` `compare: name`** (with `canvas`) compares the document
  with a trace: how many cells look alike, and the most different places
  with the colour each should be.

### Changed

- **Fill points are checked on the real pixels.** After drawing (or on the
  preview), `draw` floods the picture like a bucket fill does. It gives
  one click per piece when other lines cut a shape into pieces, and says
  where an outline has a gap a fill would leak through. It used to give
  one point from the shapes alone.
- **The design skill** paints overlapping shapes solid (Paint's playbook
  explains why outlines and bucket fills go wrong there), copies photos
  with `trace_image` (a new recipe), and checks with `compare`.
- **The security skill**: `trace_image` reads only a picture the user gave
  or pointed to.

## v2.5.0

Compared with v2.0.0
([all commits](https://github.com/mhrsdev/zero-use-computer/compare/v2.0.0...v2.5.0)).

### New

- **`draw`**: draws with the mouse button held down.
  - Shapes: rectangles (rounded corners too), ellipses, arcs, regular
    polygons, stars, Bézier curves, straight or smooth freehand lines.
  - Parametric curves `x(t)`, `y(t)` and function plots `y = f(x)` with
    ticked axes.
  - Any stroke can be rotated and repeated (rows, radial patterns).
  - Coordinates in screenshot pixels, an element's box, a document's own
    units, or a math range with y up (`canvas`).
  - `preview: true` shows the strokes over a screenshot without drawing.
  - The result names a point inside each closed shape to click for a
    bucket fill (between the circles for a ring).
  - Paced by `speed`; the stop key ends it midway and releases the button.
- **`screenshot`**:
  - `grid`: a labelled grid in the coordinates `click` and `draw` use;
  - `palette`: the main colours;
  - `pick`: the exact colour at points;
  - `canvas`: grid and `pick` in a document's units or a plot's range.
- **`press_key` / `type_text` with `x`/`y`**: the mouse points there
  first, for apps that send keys to what is under the pointer (Blender,
  CAD). Keypad keys: `Numpad0`…`Numpad9`, `NumpadDecimal` and the keypad
  operators.
- **Design skill** (`skills/computer-use-design`) for Photoshop, Paint,
  GIMP, Krita, Illustrator, Inkscape, Figma, Blender, Revit, AutoCAD and
  SketchUp, from a text brief or a reference image:
  - a spec of exact numbers first;
  - the most exact method each app has;
  - checks after every pass;
  - an app playbook for each app, and recipes for common jobs.
  - Offered over MCP like the other skills, as the `computer-use-design`
    prompt and its files as resources.

### Fixed

- Skill files served over MCP had Windows line endings (CRLF) on Windows.
  They now have LF on every OS.

### Changed

- **No approvals in the server.** Per-app approvals, the sensitive-app
  categories, the on-screen approval window (and the indicator's "waiting
  for approval" state) and `change_setting` are gone.
  - Which apps and actions need the user's OK is set by the
    `computer-use-security` skill. Every skill prompt brings it along.
  - The server still keeps input on the app it is meant for. Passwords
    and card numbers are masked. The emergency stop key works.
- **`launch_app` takes no command-line arguments** (`args` is gone). It
  starts an app by its name, bundle id or executable, and never runs a
  command line.
- **Settings**: `computer-use-mcp config` on the command line.
- **Removed tools**: `create_folder`, `list_folder`, `read_file` and
  `skill`.
  - For files, use the MCP client's own file tools.
  - Skills still come as MCP prompts and resources, as in v2.0.0.
