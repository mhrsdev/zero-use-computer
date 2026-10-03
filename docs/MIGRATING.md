# Upgrading to v3.8.2

Nothing to change for most setups. What behaves differently:

| What | Before | Now | The old way |
|---|---|---|---|
| A settings file with an error | the server didn't start | it serves with the defaults and says what is wrong | none (`computer-use-mcp config check` shows the error) |
| `$RUST_LOG` | set the log level | ignored: `$COMPUTER_USE_LOG` or `server.log` | `COMPUTER_USE_LOG=…` |
| `timing.*` over a minute, `overlay.move_ms` over 5 s, a misspelt `server.log` | accepted | refused, with the limit | none |
| A relative `script.dir` / `audit.path` | in the folder the server was started in | in the server's folder (`~/.computer-use`) | an absolute path |
| Matching text with accents | `é` matched `é` only | `é`, `e` + accent and `e` match each other | none |
| The client closing the connection | the running call went on | it stops, and waiting calls don't run | none |
| Windows: `press_key("A")` and other plain characters | the key's position, US-style | the character on the user's layout (Shift added for capitals) | `type_text` for text; shortcuts are unchanged |
| Windows: an app run as administrator | input silently dropped | an error saying so | run the server as administrator |
| Windows: a window change that didn't happen | reported as done | an error | none |
| macOS: keys and shortcuts | US key codes | the current layout's keys | none |
| macOS: Electron apps | the window frame only | their tree (`AXManualAccessibility` is set) | none |
| Linux: accessibility switched off | left off (Qt, Firefox, Chromium had no tree) | switched on, and remembered by the desktop | switch it off again in the desktop's settings after use |
| Linux: no accessibility bus | the server didn't start | it starts without the tree | none |
| Linux: an empty clipboard | an error | `""` | none |

# Upgrading to v3.8.1

The design board changes what it sends, not what it takes:

| What | Before | Now | The old way |
|---|---|---|---|
| The picture after a change | the whole page | the part that changed, and where it is, when it is under half the page | `screenshot.scope = "full"` |
| `export` alone | the listing and the picture again | the file, and what changed (nothing) | none (a call with only `name` shows everything) |
| Look-alike layers in a row | a line each | one record (`3 × ellipse …: d1 x … · d2 x …`) | none |
| Paint steps of a see-through layer | its colour | the colour it shows over the page | none |
| Bold text with no font | measured and painted regular | bold | none |
| An action, with the cursor shown | at once, the cursor arriving after | when the cursor is there (up to `overlay.move_ms`, 220 ms) | `overlay.move_ms` lower, or `overlay.show_cursor = false` |

# Upgrading to v3.8

Nothing changes without a decision model. With one:

| What | Before | Now | The old way |
|---|---|---|---|
| An `expect`ed text shown in other words | not seen | confirmed by the model ("in other words, as the decision model reads it: 0.93") | `decision.auto = false` |
| `get_app_state(about=…)` | one request per part, always the model's judgment | one request for all the parts; the words matched where the model doesn't answer | `decision.auto = false` (words only) |
| `find_tools(query=…)` naming no tool | the words in descriptions | the model's choice, marked "(Chosen by the decision model.)" | `decision.auto = false` |
| The same question about the same state | asked again | answered from memory for 5 minutes | `decision.cache_seconds = 0` |
| A failing model | each question waits for it | the server's own questions stop for a minute after three failures | none: the agent's own questions are always asked |

# Upgrading to v3.7.5

A debugging release: no setting or tool changes. A few things that were
unsafe now refuse:

| What | Before | Now |
|---|---|---|
| `decide setup={…}` from the chat | took `api_key_env`; a new `base_url` kept the saved key | no `api_key_env` (set it in the file); a new address or provider needs its key in the same call |
| Script `import` | any path | the name of a saved script |
| Script files on Windows | `\x` and `C:x` were joined to the scripts' folder | they follow the rules for absolute paths |
| X11 screenshot of a minimized window | the pixels in its place | an error: bring it forward (`window action=focus`) |
| `tools.manager = "list_changed"` over HTTP | as set | works as `"dispatch"` |
| A request with `"id": null` | dropped | an `Invalid Request` error |
| `computer-use-mcp tools` | every tool | the tools the model is served |

# Upgrading to v3.7

v3.7 turns on every token-saving setting and changes the wording of some
results. Nothing is removed: every call that worked in v3.6 still works,
and every row below has a setting that brings the old way back.

| What changed | Before | Now | The old way |
|---|---|---|---|
| The tool list | every tool | the base tools, `find_tools` and `use_tool`; the others found and run through `use_tool` (a direct call still works where a client allows it) | `tools.manager = "off"` |
| Tool schemas | compact | lean: nested objects as their keys, no `window` (still accepted), no defaults | `tools.descriptions = "compact"` |
| `decide` | always listed | listed once a decision model is set up (it can always be called) | `tools.descriptions = "full"` |
| An action's report | every change | the changes around what it acted on, new windows, added elements, and how many others | `tree.report = "full"` |
| Elements that keep changing (clocks, spinners) | listed each time | summed up in a line after a few reports | `tree.quiet_volatile = false` |
| Automatic pictures | as before | held back for a well-described window while the model hasn't used pixels there | `screenshot.adaptive = false` |
| `locate` | with a picture of the places | the places only (`picture: true` asks) | `screenshot.locate_picture = true` |
| Canvases and painted text next to a full toolbar | only through `ocr=true` | read and watched on every look (`ocr text` elements) | `ocr.blind_regions = false` |
| `design` | the paint steps with every answer | how many; `show.steps` lists them | `tools.design_steps = "always"` |
| Tool results' `_meta` | none | which earlier results each one repeats | `server.result_meta = false` |
| A diff's intro, the third time on | `Changes (+ added, ~ changed, - removed):` | `Changes:` | `tree.brief_repeats = false` |
| Report footers, after the first | `…; get_app_state shows them` | the count only | `tree.brief_repeats = false` |
| draw preview, loupe, zoomed screenshot notes, after the first | in full | short | `tree.brief_repeats = false` |
| Tesseract | its own thread count | `OMP_THREAD_LIMIT=1` unless the environment sets one | set `OMP_THREAD_LIMIT` |

New: `--instructions full|short|off` on the command line (the Claude Code
plugin uses `short`, as it brings the skills).

# Upgrading to v3.6

v3.6 changes what some tool results look like, and how a few tools
behave. Nothing is removed: every call that worked in v3.2 still works.
This page lists what a prompt, a script or an embedder that reads results
might notice, and the setting that brings the old behaviour back where
there is one.

## Tool results

| What changed | Before | Now | The old way |
|---|---|---|---|
| Look-alike siblings | one line each: `7 button "Select"`, `8 button "Pen"` | a record line: `2 × button:` then `7 "Select" · 8 "Pen"` | `tree.compact = false` |
| Table rows | one line per cell | the column names once, then a row a line | `tree.compact = false` |
| Flags the role implies | `text field "Name" (editable)` | `text field "Name"` (a field says `read-only` when it isn't editable) | `tree.compact = false` |
| A look that changed nothing | the header and "No changes" | one line: `…: nothing changed since your last look` | `tree.compact = false` |
| `set_value` on a field that took it | `Set text field "Name" to "Ada".` | `Set text field "Name"; it shows the new value.` | `tree.compact = false` |
| Many removed elements | one line each | ranges of indices | `tree.compact = false` |
| `launch_app` | "Launched X. Call get_app_state to see it." | the same line, then the app's first state (as `get_app_state` gives it) | `tools.launch_look = false` |
| `batch` | one line per step | one line per step, then one report of what the steps changed; lines as steps | — |
| `design`, `scene` (after the first answer) | everything, every time | what changed since the last answer; "as before"; no picture when its pixels are the same; a call that changes nothing still gets everything | `tree.compact = false` |
| Forms on Linux | `text field` with no name, next to its label | `text field "Full name"` (the label it is labelled by) | — |
| OCR lines | every line Tesseract returned | rulers and shapes left out; low-confidence lines say `unsure reading` | — |

Code that reads trees line by line can expand records back to one element
a line with `computer_use::tree::expand(text)`, and find an element's
index with `computer_use::tree::index_of(text, needle)`.

## Behaviour

- **`batch` stops on a window it didn't expect.** When a step brings up
  another window (a dialog, an error) and later steps remain, the batch
  stops there and says so, because those steps were meant for the window
  before. Give the step that opens a window `expect: "dialog"` (or end its
  line with `expect dialog`), or pass `through_windows: true` for the old
  behaviour. A step whose `expect` isn't confirmed stops the batch too.
- **A table cell whose press changed nothing is clicked with the mouse**,
  once. Selecting a row is safe to repeat; GTK's accessibility press on a
  cell doesn't select its row. Other elements keep the old rule (no second
  try unless `verify.retry_on_no_change`).
- **`click` takes `name` (and `role`)** when no `element_index` or point
  is given. One element must have that name; when several do, nothing is
  clicked and they are listed.

## New settings (all optional)

Defaults keep v3.2's behaviour except where the table above says
otherwise. The ones that change what the model reads are off until the
real-model benchmark has measured them ([roadmap](ROADMAP.md)).

| Setting | Default | What it does |
|---|---|---|
| `tree.compact` | `true` | each thing said once (above) |
| `tree.report` | `"full"` | `"relevant"`: changes around what the action acted on; `"brief"`: how many |
| `tree.quiet_volatile` | `false` | elements that keep changing on their own summed up in a line |
| `screenshot.adaptive` | `false` | fewer automatic pictures while the model doesn't use pixels |
| `screenshot.locate_picture` | `true` | `false`: `locate` answers without its picture |
| `screenshot.icon_sprite` | `false` | a strip of the unnamed buttons' icons (experimental) |
| `tools.descriptions` | `"compact"` | `"lean"`: lighter schemas |
| `tools.manager` | `"off"` (v3.7: `"dispatch"`) | `"dispatch"` or `"list_changed"`: base tools plus `find_tools` |
| `tools.preset` | `"full"` | `"small"`: ten tools for smaller models |
| `tools.default_app` | `false` | a call without `app` acts on the last app |
| `tools.launch_look` | `true` | `launch_app` returns the first state |
| `tools.design_steps` | `"always"` | `"asked"`: paint steps only with `show.steps` |
| `cache.rebase_after_tokens` | `0` | send a whole tree again after this many tokens |
| `timing.expect_wait_ms` | `2000` | how long an action waits for what it `expect`s |
| `server.instructions` | `"full"` | `"short"` or `"off"` for clients that load the skills |
| `server.result_meta` | `false` | `_meta` on results: which earlier ones they repeat |
| `ocr.blind_regions` | `false` | read and watch what the tree says nothing about (v3.5) |

`bench/configs/lean.toml` turns on every one that saves tokens, for an
A/B run.
