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
- `window(app, action)`: `list`, `focus`, `move` (x, y, optional width and
  height), `resize`, `maximize`, `minimize`, `restore`, `fullscreen`,
  `exit_fullscreen`, `close`, `tile_left` / `tile_right` / `tile_top` /
  `tile_bottom`, `center`, `move_to_display`, `move_to_desktop`.
  `window(action="displays")` lists the screens. Positions are screen
  coordinates, not screenshot pixels.
- `get_clipboard` / `set_clipboard`: move text between apps (set it, then
  `press_key "cmd+v"`).
- `get_notifications(app?, limit?)`: recent desktop notifications, when the
  user turned it on. Codes in them are masked on purpose.
- `launch_app(app, args?)`: `app` is one name (never a command line); put
  program arguments in `args`. Starting a program with arguments asks first.
- `list_folder(path)` / `read_file(path)`: read-only file access; no
  desktop clicks needed for reading. `create_folder(path)` makes a folder.
- `skill(name?)`: built-in notes for this OS and for common apps (browser,
  VS Code, Office, mail, chat…). Call it with no name to list them.
- `change_setting(key, value)`: change a config setting when the user asks;
  the server opens an Allow / Deny window on screen first, and restrictions
  can only be tightened this way, never loosened.
