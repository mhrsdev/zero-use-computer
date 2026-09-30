# Microsoft Excel (Windows)

Cells are a grid: the tree exposes the cells near the window; the **Name Box**
(left of the formula bar) jumps anywhere. `set_value` on a cell sets its content
exactly; `type_text` types into the active cell.

- Jump: click the Name Box, `type_text` `B2` or `A1:D20`, `Return` (selects the
  range). `ctrl+g` / `F5` opens Go To. `ctrl+Home` A1; `ctrl+arrow` jumps to the
  data edge; `ctrl+shift+arrow` selects to it; `ctrl+a` selects the region.
- Enter data: type, then `Tab` (right) or `Return` (down); `F2` edits the cell in
  place; `Escape` cancels; `alt+Return` a new line inside a cell.
- Formulas start with `=` (`=SUM(B2:B10)`); `alt+=` AutoSum; `F4` toggles
  absolute references (`$A$1`); `ctrl+shift+Return` array formula. Fill down
  `ctrl+d`, fill right `ctrl+r`. **Read the result back** (formula bar and cell
  value) instead of trusting the screenshot.
- Format: cells `ctrl+1`; number formats `ctrl+shift+4` (currency), `ctrl+shift+5`
  (percent); bold `ctrl+b`; date `ctrl+;`.
- Rows/columns: select with `shift+Space` / `ctrl+Space`; insert `ctrl+shift++`;
  delete `ctrl+-` (destructive; it shifts data).
- Sheets: next/previous `ctrl+PageDown/PageUp`; rename: double-click the tab;
  new sheet `shift+F11`.
- Data: filters `ctrl+shift+l`; table `ctrl+t`; sort: Data ▸ Sort; freeze panes:
  View ▸ Freeze Panes; remove duplicates: Data ▸ Remove Duplicates (destructive).
- Pasting many values: `set_clipboard` with tab-separated columns and newline-
  separated rows, select the top-left cell, `ctrl+v`.
- Charts: select the data, Insert ▸ Recommended Charts.
- Save `ctrl+s`; Save As `F12`; .csv loses formatting/formulas — confirm first.
- Before large edits (delete rows, overwrite ranges, sort) keep a copy
  (Save a Copy) and tell the user what you changed.
