# Helper tools

- `find_element(app, role, name, text, editable)`: just the matching elements
  and their indices. Matching ignores case, spacing, and look-alike letter
  forms. It searches every element, including folded and cut ones.
- `wait_for(app, role/name/text, state, timeout_ms)`: after something slow
  (loading, a dialog opening), wait for an element (`present`, `visible`,
  `enabled`, `focused`, `gone`…) instead of polling. At most two minutes.
- `batch(app, steps=[{tool, arguments}, …])`: several steps in one call,
  stopping at the first error. The report has one line per step, so call
  `get_app_state` afterwards to see the result.
- `screenshot`:
  - `mode: "full"` the whole screen, `"auto"` only what changed since the
    last full one, `"region"` with `x`, `y`, `width`, `height` (screen
    coordinates);
  - `app` (+ `window`) one window; `annotate: true` draws each element's
    index on it; `element_index` zooms into one element at full resolution.
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
- `trace_image(path | app + box, colors, detail, name)`: a reference
  picture as flat colour steps to paint with `draw` (see
  [drawing.md](drawing.md)).
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
  `press_key "cmd+v"`).
- `get_notifications(app?, limit?)`: recent desktop notifications, when the
  user turned it on. Codes in them are masked on purpose.
