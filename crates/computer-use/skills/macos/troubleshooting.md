# When something doesn't work (macOS)

- **Permission errors**: the agent needs Accessibility and Screen Recording
  (System Settings ▸ Privacy & Security). Ask the user to enable them for the
  host app, then retry.
- **`get_app_state` shows almost nothing**: the app draws its own UI. Take a
  `screenshot`, try `get_app_state(ocr: true)`, then click by `x`/`y` from the screenshot.
- **Element indices "unknown"**: they expire at the next `get_app_state` — read
  the state again.
- **Nothing happened after a click**: `wait_for` the expected element; the app
  may be slow, or an alert sheet/dialog appeared (check the state and
  `window list`).
- **App is blocked**: Terminal, password managers and security prompts are
  blocked on purpose. Tell the user.
- **Spinning beach ball**: wait; don't force-quit without the user's OK.
- **Input seems ignored**: the user may be using the mouse/keyboard (the agent
  pauses), or the stop key was pressed — wait for the user.
- **Wrong window/Space**: use the `window` tool to list and focus the right one.
