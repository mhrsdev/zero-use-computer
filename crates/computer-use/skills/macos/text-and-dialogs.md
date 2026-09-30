# Typing, editing and file dialogs (macOS)

- Prefer `set_value` for fields; `type_text` for focused text areas. Newlines in
  `type_text` press Return.
- Clipboard route for long or special text: `set_clipboard`, then `cmd+v`.
- Select all `cmd+a`; copy `cmd+c`; paste `cmd+v`; paste without style
  `cmd+alt+shift+v`; undo `cmd+z`; redo `cmd+shift+z`; find `cmd+f`; save
  `cmd+s`; save as `cmd+shift+s` (or hold `alt` on File).
- Move by word `alt+Left/Right`; line start/end `cmd+Left/Right`; document
  start/end `cmd+Up/Down`; select while moving: hold `shift`.
- Open/Save sheets: press `cmd+shift+g` inside the sheet, `type_text` a path,
  `Return` to jump to that folder; the "Save As" name field takes the file name.
- Confirmation sheets: read the buttons — "Don't Save", "Delete", "Replace" are
  destructive. Prefer Cancel when unsure and ask.
- Emoji & symbols: `ctrl+cmd+space`.
