# When something doesn't work (Windows)

- **`get_app_state` shows almost nothing**: the app draws its own UI. Take a
  `screenshot`, try `ocr: true`, then click by `x`/`y` from the screenshot.
- **Element indices "unknown"**: they expire at the next `get_app_state` — read
  the state again and use the new numbers.
- **Nothing happened after a click**: `wait_for` the expected element, then read
  the state; the app may be slow or a dialog opened in a separate window
  (check `list_apps` / `window list`).
- **App is blocked**: terminals, password managers and Windows Security are
  blocked on purpose. Tell the user; don't try to go around it.
- **Not responding ("(Not Responding)" in the title)**: wait a few seconds; don't
  kill it without the user's OK.
- **Elevated (Run as administrator) apps** can't be controlled from a normal
  process — ask the user to restart the agent elevated, or use the app normally.
- **Input seems ignored**: the user may be using the mouse/keyboard (the agent
  pauses); or the stop key was pressed — wait for the user.
- **Wrong window**: use the `window` tool to list and focus the right one.
