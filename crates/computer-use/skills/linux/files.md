# Files and folders (Linux: GNOME Files / Nautilus, Dolphin, Thunar…)

Prefer the tools first: `list_folder` / `read_file` to look, `create_folder` to
make a folder. Use the file manager only for what they can't do (move, copy,
rename, trash, open with an app).

- Open the file manager: `launch_app("nautilus")` (GNOME "Files"), `dolphin`
  (KDE), `thunar` (Xfce), `nemo`, `pcmanfm`.
- Go to a path: `ctrl+l`, `type_text` the path (`~/Documents`), `Return`.
- New folder: `ctrl+shift+n` (Nautilus/Nemo/Thunar) or `F10` in Dolphin; type
  the name, `Return`.
- Rename: select, `F2`, type, `Return`.
- Copy / cut / paste: `ctrl+c` / `ctrl+x` / `ctrl+v`. Move by cut + paste.
- Trash: `Delete` (recoverable); `shift+Delete` deletes permanently — never use
  unless asked. Deleting is a sensitive action: expect a confirmation.
- Hidden files: `ctrl+h`. Search: `ctrl+f`.
- Properties: `alt+Return`. Open terminal here: blocked on purpose.
- Desktop environments differ: check the tree with `get_app_state` for the
  real buttons rather than assuming a layout.
