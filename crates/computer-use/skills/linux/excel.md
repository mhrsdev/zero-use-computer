# Spreadsheets on Linux (LibreOffice Calc; Excel on the web)

Microsoft Excel has no Linux desktop app. Use **LibreOffice Calc**
(`launch_app("localc")`; `launch_app("libreoffice")` opens the Start Center) or Excel on the web in a
browser.

- Jump: click the Name Box (left of the formula bar), `type_text` `B2` or
  `A1:D20`, `Return`. `ctrl+Home` A1; `ctrl+arrow` data edge; `ctrl+shift+arrow`
  selects to it; `ctrl+a` selects everything.
- Enter data: type, then `Tab` (right) or `Return` (down); `F2` edit in place;
  `Escape` cancels; `ctrl+Return` new line inside a cell. `set_value` on a cell
  sets its content exactly.
- Formulas start with `=`; separators depend on locale (`;` or `,`); `alt+=`
  AutoSum; `F4` toggles absolute references; fill down `ctrl+d`. **Read the
  result back.**
- Format cells `ctrl+1`; bold `ctrl+b`; date `ctrl+;`.
- Rows/columns: select `shift+Space` / `ctrl+Space`; insert `ctrl++`; delete `ctrl+-`
  (destructive).
- Sheets: next/previous `ctrl+PageDown/PageUp`; new sheet `shift+F11`.
- Data: Autofilter `ctrl+shift+l`; Sort: Data ▸ Sort; Remove Duplicates:
  Data ▸ More Filters ▸ Standard Filter (destructive).
- Paste many values: `set_clipboard` with tab-separated columns and newline-
  separated rows, select the top-left cell, `ctrl+v` (a Text Import dialog may
  appear — confirm the separator).
- Save `ctrl+s` (Keep current format for .xlsx); Save As `ctrl+shift+s`; CSV loses
  formulas — confirm.
- Keep a copy before destructive edits and tell the user what changed.
