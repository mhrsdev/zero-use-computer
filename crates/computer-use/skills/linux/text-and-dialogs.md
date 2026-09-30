# Typing, editing and file dialogs (Linux)

- Prefer `set_value` for fields; `type_text` for focused text areas. Newlines in
  `type_text` press Return.
- Clipboard route for long or special text: `set_clipboard`, then `ctrl+v`
  (terminals use `ctrl+shift+v`, but those are blocked).
- Select all `ctrl+a`; copy `ctrl+c`; paste `ctrl+v`; undo `ctrl+z`; redo
  `ctrl+shift+z` (or `ctrl+y`); find `ctrl+f`; replace `ctrl+h`; save `ctrl+s`;
  save as `ctrl+shift+s`.
- Move by word `ctrl+Left/Right`; line start/end `Home`/`End`; document
  start/end `ctrl+Home`/`ctrl+End`; select while moving: hold `shift`.
- GTK Open/Save dialogs: press `ctrl+l` to get a location field, `type_text` a
  path, `Return`; the name field takes the file name.
- Confirmation dialogs: read the buttons — "Don't Save", "Delete", "Replace"
  are destructive. Prefer Cancel when unsure and ask.
- Non-Latin text: prefer `set_value` or the clipboard route; `type_text` goes
  through synthetic key events and may mis-type unusual characters.
