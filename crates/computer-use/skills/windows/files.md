# Files and folders (Windows, File Explorer)

Prefer the tools first: `list_folder` / `read_file` to look, `create_folder` to
make a folder. Use File Explorer only for what they can't do (move, copy,
rename, delete, open with an app).

- Open Explorer: `launch_app("explorer")` or `press_key win+e`.
- Go to a path: focus the address bar with `ctrl+l` (or `alt+d`), `type_text`
  the full path, `Return`.
- New folder: `ctrl+shift+n`, type the name, `Return`.
- Rename: select the item, `F2`, type the new name, `Return`.
- Copy / cut / paste: `ctrl+c` / `ctrl+x` / `ctrl+v`; move by cut + paste in the
  target folder. Duplicate by copy + paste in the same folder.
- Delete: `Delete` sends to the Recycle Bin (recoverable); `shift+Delete` is
  permanent — never use it unless the user asked. Deleting is a sensitive
  action: expect a confirmation.
- Select many: `ctrl+a` (all), or click the first then `shift+click` the last.
- Search: click the search box (`ctrl+e`), type, `Return`; results stream in, so
  `wait_for` the list to settle.
- Properties / size: `alt+Return` on the selected item.
- Show file extensions if names look wrong: View ▸ Show ▸ File name extensions.
- Zip: right-click (or `perform_secondary_action show_menu`) ▸ Compress to ZIP file.
