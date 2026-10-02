# Helper tools

- `find_element(app, role, name, text, editable)`: just the matching elements
  and their indices. Matching ignores case, spacing, and look-alike letter
  forms. It searches every element, including folded and cut ones.
- `wait_for(app, role/name/text, state, timeout_ms)`: after something slow
  (loading, a dialog opening), wait for an element (`present`, `visible`,
  `enabled`, `focused`, `gone`…) instead of polling. At most two minutes.
  With a decision model, `until: "Have the results loaded?"` waits for a
  yes about the whole window instead (see [decisions.md](decisions.md)).
- `decide(question, …)`: typed answers from the decision model about text,
  many items at once, or an app's window; `pick` finds an element by
  description. See [decisions.md](decisions.md).
- `batch(app, steps=[{tool, arguments}, …])`: several steps in one call,
  stopping at the first error. The report has one line per step, so call
  `get_app_state` afterwards to see the result.
- `screenshot`:
  - `mode: "full"` the whole screen, `"auto"` only what changed since the
    last full one; `x`, `y`, `width`, `height` (screen coordinates) a
    region of the screen;
  - `app` (+ `window`) one window: nothing if it looks as in your last
    picture of it ("unchanged … not re-sent": that picture is current),
    else only the part that changed when that is small, else all of it;
    `mode: "window"` always all of it. `annotate: true` draws each
    element's index on it; `element_index` zooms into one element at full
    resolution.
  - Screenshots are numbered (`Screenshot #7`); a changed part says which
    one it patches, and `x`/`y` still refer to that whole one.
  - `grid: 100` draws a labelled grid every 100 px, in the x/y that
    `click`, `drag` and `draw` use for that window (screen coordinates for
    full/region shots): read positions off it instead of guessing.
    `grid: true` picks a round step.
  - `palette: true` lists the main colours (hex and share);
    `pick: [[x, y], ...]` gives the exact colour at each point (same
    coordinates as the grid).
  - `canvas` (window shots, the same object `draw` takes): the grid covers
    only that box, labelled in the document's units or the plot's range,
    and `pick` points are in those units too.
  - `compare: "name"` (with `canvas`): how the document differs from a
    picture traced with `trace_image`.
  - `cells: true` (with `canvas`): graph paper over the document, columns
    A, B… and rows 1, 2… (about 8 across; `cell_size` sets their size in
    the document's units). `cell: "C4"` shows that cell magnified with a
    fine grid in the document's units and its colours.
  - `zoom: [x, y]` (window shots): a magnified view around a point, each
    screen pixel a square, with a crosshair and a grid in click
    coordinates. Use it to aim at something small.
- `locate(app, ...)`: exact places in a window, in click coordinates:
  - `color: "#hex"`: every area of that colour, with its centre and box;
  - `like: [l, t, r, b]`: every other place that looks like that part of
    the window (the same icon, handle or marker);
  - `near: [x, y]` with `feature: "corner"`, `"edge"` or `"center"`: the
    exact point next to a rough one.
- `click` and `drag` take `snap: "corner"` (or `"edge"`, `"center"`,
  `"#hex"`): the x/y moves onto the nearest one within `snap_radius`
  before acting. The result says how far it moved.
- `trace_image(path | app + box, colors, detail, name)`: a reference
  picture as flat colour steps to paint with `draw` (see
  [drawing.md](drawing.md)).
- `design` (a 2D picture from layers) and `scene` (a 3D model from
  solids): plan and see a drawing before building it in an app. See
  [drawing.md](drawing.md) and the computer-use-design skill.
- `script(code, args, data)`: a small program run in the server, for
  loops over tool calls, maths, data and graph-paper pages; `save` keeps
  one as a tool of its own. See [scripts.md](scripts.md).
- `window(app, action)`: `list`, `focus`, `move` (x, y, optional width and
  height), `resize`, `maximize`, `minimize`, `restore`, `fullscreen`,
  `exit_fullscreen`, `close`, `tile_left` / `tile_right` / `tile_top` /
  `tile_bottom`, `center`, `move_to_display`, `move_to_desktop`.
  `window(action="displays")` lists the screens. Positions are screen
  coordinates, not screenshot pixels.
- `press_key` / `type_text` with `x`/`y`: point the mouse there first, for
  apps that send keys to what is under the pointer (Blender). Keypad keys:
  `Numpad0`…`Numpad9`, `NumpadDecimal`, `NumpadAdd`, `NumpadSubtract`.
- `get_clipboard` / `set_clipboard`: move text between apps (set it, then
  `press_key "cmd+v"`). Long text goes in faster this way than typed.
- Limits per call: `type_text` up to 100,000 characters (typed in pieces;
  the stop key works between them), `press_key` up to 500 presses,
  `scroll` up to 50 pages; `get_clipboard` returns the first 30,000
  characters of a longer clipboard.
- `get_notifications(app?, limit?)`: recent desktop notifications, when the
  user turned it on. Codes in them are masked on purpose.
