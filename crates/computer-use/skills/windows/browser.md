# Web browsing (Windows: Edge, Chrome, Firefox)

- Open a page: focus the address bar with `ctrl+l` (or `F6`), `type_text` the
  URL, `Return`. New tab `ctrl+t`, close tab `ctrl+w`, reopen `ctrl+shift+t`,
  next/previous tab `ctrl+Tab` / `ctrl+shift+Tab`, back `alt+Left`.
- Page content is often exposed poorly by browsers: if `get_app_state` shows
  few elements, use `find_element` by text, and take a `screenshot`; OCR
  (`get_app_state(ocr: true)`) helps for canvas-heavy pages. Chromium browsers enable full
  accessibility when an assistive tool is detected — the first read may be
  sparse, read again after a second.
- Find on page: `ctrl+f`, type, `Return`. Zoom: `ctrl+plus` / `ctrl+minus` / `ctrl+0`.
- Forms: `set_value` on each field, then `Tab` between fields if needed. Do not
  submit payments, sign-ins, or anything irreversible without the user's OK.
- Never type passwords the user hasn't given you; password managers and
  sign-in prompts are blocked on purpose — ask the user to do that step.
- Downloads land in `%USERPROFILE%\Downloads`; check with `list_folder`.
- Wait for loading with `wait_for` (a known element or text), not fixed sleeps.
