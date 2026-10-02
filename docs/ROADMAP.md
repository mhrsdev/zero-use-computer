# Roadmap: v3.5 to v4.0

The goal: fewer tokens spent on **finishing** a task, with the same accuracy
and stability. A smaller tool result is not a win on its own; every change
is measured with the real model on [the benchmark](../bench/README.md).
v3.5, v3.6, … are steps; v4.0 is the stable release of what they proved.

## Rules for every step

- **What counts**: real tokens for a successful task — input, output, cache
  reads and cache writes apart — plus cost, time, turns, tool calls and
  pictures. Estimates are reported apart from real numbers.
- **Behind a setting first**: a change to what the tools return is added
  off, measured both ways (A/B), and turned on by default only if success
  doesn't drop.
- **Every step is compared** with the step before and with the v3.2
  baseline, and its regressions are published with its gains.
- **The decision model (Jev and the like) is optional.** Nothing depends
  on it; a feature that uses it is an extra on top of one that doesn't.
- **Safety doesn't move**: finding a tool never permits running it; the
  masking and the stop key stay as they are.
- A simulation of Codex's behaviour is called a simulation.

| step | focus |
|---|---|
| v3.5 | measuring for real; the screenshot path; areas the tree says nothing about |
| v3.6 | OCR, observation that suits the content, reports that say what matters |
| v3.7 | the tool surface: compact definitions and a tool manager |
| v3.8 | checking results, fewer round trips |
| v3.9 | optional layers, release candidate |
| v4.0 | stable |

## v3.5 — measuring, screenshots, blind areas

Done:

- `bench/`: real-model and scripted runs, six scenarios, success checked
  from the app; the v3.2 scripted baseline in `bench/results/v3.2`.
- Screenshots: region parameters are never ignored; `screenshot(app)`
  shares `get_app_state`'s dedupe and changed-part path; every screenshot
  is numbered and a changed part names the one it patches.
- Blind areas (`ocr.blind_regions`, off): found from the tree and checked
  in the pixels; read alone, at two sizes; watched on every look.

Left before v3.5.0:

- Real-model runs (5 or more per scenario) of v3.2 and v3.5 with and
  without `ocr.blind_regions`; `--calibrate` to check the token estimate
  (Persian and other non-Latin text especially), then fix the estimate if
  it is off.
- Decide `ocr.blind_regions`'s default from those runs.

Found while building it (for the steps below):

- GTK 3 table cells: the accessibility action doesn't select the row; a
  mouse click does (v3.8).
- GTK 3 keeps a filtered table's old cell text over AT-SPI while the
  picture shows the new rows, so the tree and the picture disagree (v3.6).
- Every table cell carries `actions=[expand_or_collapse, edit]` (v3.6,
  tables).
- Fields named only by a label (GTK's mnemonic label, a labelled-by
  relation) come without a name (v3.6).
- `locate` sends a picture of the whole window with every answer (v3.6).
- Whole-window OCR enlarges everything, which breaks big framed text
  (v3.6, OCR).

## v3.6 — OCR and observation that suit the content

- **OCR on Linux**: line and grid removal, binarizing, cropping to the
  areas that need it, the page-segmentation mode per area; icons and rulers
  filtered out; uncertain lines marked as such; text read off the screen
  never taken for a real button; a picture when OCR can't read it.
- **Observation by scenario**, from simple rules (no extra model): forms
  and settings from accessibility; tables and lists by search, pages and
  the part needed (`get_app_state(within=index)`, `offset` in
  `find_element`); canvases with text by OCR and the area's picture; 3D and
  graphics by a detailed picture; a more complete way when one isn't
  enough.
- **Tables**: column headers once and one line per row; one template for
  look-alike subtrees; no flags or actions the role already implies.
- **Reports in levels**: brief, relevant changes (around the target, new
  windows, errors, and how many others), full; `detail` asks for more.
  The default is relevant changes, measured against extra
  `get_app_state` calls.
- **Smaller reports**: a short header when nothing changed (about 60
  tokens down to 8); no echo of what the model just wrote; removed lines as
  ranges of indices; added lines grouped under their parent; volatile
  elements (clocks, spinners) summarised.
- **Pictures**: attach automatically by how the model works in an app (no
  x/y or screenshots asked for → fewer automatic pictures); measure the
  overview size A/B.

## v3.7 — the tool surface

- **Compact definitions**: lighter schemas for shapes, no `window` or
  `default` in schemas (still accepted), no repeats; watched by wrong calls
  and retries.
- **`instructions`** full, short or off; skills shortened and freed of
  overlap with them (today about 580 + 2,400 tokens); `decide` shown only
  when a decision model is set up.
- **Tool manager** (experimental, behind a setting): the base tools, a line
  per category and an always-present `find_tools`. Two ways, by client:
  `tools/list_changed` where clients support it, or a fixed list with the
  schema returned in the conversation and a `use_tool` dispatcher (cache
  friendly, any client). The active set stays fixed through a step;
  hidden is not forbidden (permission is still checked where it is today);
  a compatibility table for Claude Code (which has its own tool search),
  Codex, Cursor, VS Code and Claude Desktop.
- **Smaller models**: a fixed small profile with simpler descriptions and
  no dynamic discovery; measured per model.

Today the definitions are about 4,650 tokens per request, 46% of them the
five design tools.

## v3.8 — checking results, fewer round trips

- **Targeted checks**: an optional `expect` (a dialog opens, the value
  changes, an element appears); three outcomes (confirmed, not confirmed,
  uncertain); "sent" and "seen to take effect" said apart; nothing
  uncertain repeated.
- **`batch`**: stops on an unexpected change (a dialog, focus to another
  app), ends with one diff, takes a short string syntax; fewer round trips
  measured against more errors.
- **Fewer turns and output tokens**: click by name or role; `launch_app`
  returns the first state; `app` defaults to the last one.
- **References the model may have lost**: the server counts what it has
  sent; past a threshold, references count as stale and the base view is
  sent again; `rebase` asks for it.
- **`design` and `scene`**: diffs of layers and checks, a picture only when
  it changed, steps only when asked, short strings for shapes.

## v3.9 — optional layers, release candidate

- Decision model (optional): extraction by `pick` (the small model picks
  the element, the server reads its value), relevance of subtrees by
  `score`.
- For embedders: older results marked superseded, so a host can drop them
  from its context.
- Experimental: a sprite of unlabelled icons; icon labels cached per app.
- Defaults set from the A/B results; real Windows and macOS hardware
  tested; the full benchmark on every model.

## v4.0 — stable

Final defaults, deprecated behaviour removed, a migration guide, and the
full benchmark published: v3.2 against v4.0, real numbers apart from
estimates, gains and regressions alike.
