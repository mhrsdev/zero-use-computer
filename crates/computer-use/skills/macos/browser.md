# Web browsing (macOS: Safari, Chrome, Firefox)

- Open a page: `cmd+l`, `type_text` the URL, `Return`. New tab `cmd+t`, close
  tab `cmd+w`, reopen `cmd+shift+t`, next/previous tab `ctrl+Tab` /
  `ctrl+shift+Tab` (or `cmd+shift+]` / `cmd+shift+[`), back `cmd+[`.
- Safari exposes web content well; Chromium browsers may need a second read
  after a moment. If `get_app_state` shows few elements, use `find_element` by
  text, a `screenshot`, and `ocr: true`.
- Find on page: `cmd+f`. Zoom: `cmd+plus` / `cmd+minus` / `cmd+0`.
- Forms: `set_value` on each field; `Tab` between fields. Don't submit payments
  or anything irreversible without the user's OK.
- Passwords / Keychain / sign-in prompts are blocked on purpose — ask the user
  to do that step.
- Downloads go to `~/Downloads`; check with `list_folder`.
- Use `wait_for` instead of fixed sleeps while pages load.
