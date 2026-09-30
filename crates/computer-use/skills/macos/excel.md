# Microsoft Excel (macOS); Numbers notes at the end

The grid's cells near the window are in the tree; the **Name Box** (left of the
formula bar) jumps anywhere. `set_value` on a cell sets its content exactly.

- Jump: click the Name Box, `type_text` `B2` or `A1:D20`, `Return`. Go To
  `ctrl+g`. `cmd+Up/Down/Left/Right` jump to the data edge; add `shift` to select;
  `cmd+a` selects the region.
- Enter data: type, then `Tab` (right) or `Return` (down); edit in place with
  `ctrl+u` (or `F2` if enabled); `Escape` cancels; `ctrl+alt+Return` new line in
  a cell.
- Formulas start with `=`; AutoSum `cmd+shift+t`; toggle absolute reference
  `cmd+t`; fill down `cmd+d`, fill right `cmd+r`. **Read the result back**.
- Format: cells `cmd+1`; bold `cmd+b`; date `ctrl+;`.
- Rows/columns: select with `shift+Space` / `ctrl+Space`; insert `ctrl+i`; delete
  `cmd+-` (destructive; shifts data).
- Sheets: next/previous `ctrl+PageDown/PageUp`; new sheet `shift+F11`.
- Data: filters `cmd+shift+f`; table: Insert ▸ Table; sort: Data ▸ Sort.
- Paste many values: `set_clipboard` with tab-separated columns and newline-
  separated rows, select the top-left cell, `cmd+v`.
- Save `cmd+s`; Save As `cmd+shift+s`; .csv loses formulas/formatting — confirm.
- If a shortcut doesn't behave, use the menu bar or `find_element` for the
  command instead of guessing.
- **Numbers** (Apple): click a cell and type; formulas start with `=`; export
  via File ▸ Export To ▸ Excel/CSV/PDF.
- Keep a copy before destructive edits and tell the user what changed.
