# When something doesn't work (Linux)

- **No accessibility info / `list_apps` empty**: AT-SPI must be running and the
  session accessibility bus enabled (GNOME: Settings ▸ Accessibility; or
  `gsettings set org.gnome.desktop.interface toolkit-accessibility true`).
  Ask the user; some apps need a restart afterwards.
- **`get_app_state` shows almost nothing**: the app draws its own UI. Take a
  `screenshot`, try `get_app_state(ocr: true)`, then click by `x`/`y` from the screenshot.
- **Element indices "unknown"**: they expire at the next `get_app_state` — read
  the state again.
- **Nothing happened after a click**: `wait_for` the expected element; a dialog
  may have opened in a separate window (`list_apps` / `window list`).
- **App is blocked**: terminals, password managers and polkit/credential
  prompts are blocked on purpose. Tell the user.
- **Wayland limits**: screenshots, global keys and window management may need
  portal permission or be unsupported; say so and use the app's own UI.
- **Input seems ignored**: the user may be using the mouse/keyboard (the agent
  pauses), or the stop key was pressed — wait for the user.
