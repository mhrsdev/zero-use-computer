# Typing, editing and file dialogs (Windows)

- Prefer `set_value` for fields; use `type_text` for focused text areas and
  editors. Newlines in `type_text` press Return.
- Clipboard route for long or special text: `set_clipboard`, then `ctrl+v`.
- Select all `ctrl+a`; copy `ctrl+c`; paste `ctrl+v`; undo `ctrl+z`; redo
  `ctrl+y`; find `ctrl+f`; replace `ctrl+h`; save `ctrl+s`; save as `ctrl+shift+s`.
- Move by word `ctrl+Left/Right`; line start/end `Home`/`End`; document start/end
  `ctrl+Home`/`ctrl+End`; select while moving: hold `shift`.
- Open/Save dialogs: the "File name" box accepts a full path — `type_text` the
  path, `Return`. To pick a folder, type it in the File name box and press
  `Return` to navigate there first.
- Confirmation dialogs: read the buttons before pressing; "Don't save",
  "Delete", "Replace" are destructive. Prefer Cancel when unsure and ask.
- Emoji / symbols: `win+.`.
