# Web browsing (Linux: Firefox, Chromium, Chrome)

- Open a page: `ctrl+l`, `type_text` the URL, `Return`. New tab `ctrl+t`, close
  `ctrl+w`, reopen `ctrl+shift+t`, next/previous tab `ctrl+Tab` /
  `ctrl+shift+Tab`, back `alt+Left`.
- Web content is exposed to AT-SPI only when accessibility is on. If the tree is
  nearly empty, the browser may need accessibility enabled (Firefox:
  `GNOME_ACCESSIBILITY=1` / `accessibility.force_disabled`; Chromium: start with
  `--force-renderer-accessibility`) — ask the user. Meanwhile use `screenshot`,
  `find_element` and `ocr: true`.
- Find on page: `ctrl+f`. Zoom: `ctrl+plus` / `ctrl+minus` / `ctrl+0`.
- Forms: `set_value` on each field; `Tab` between fields. Don't submit
  payments or anything irreversible without the user's OK.
- Sign-in / password-manager prompts are blocked on purpose — ask the user.
- Downloads go to `~/Downloads`; check with `list_folder`.
- Use `wait_for` instead of fixed sleeps while pages load.
