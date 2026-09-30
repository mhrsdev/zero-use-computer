# Office documents and spreadsheets (Linux: LibreOffice, OnlyOffice)

- LibreOffice exposes menus and toolbars to AT-SPI; use `find_element` with the
  command name ("Bold", "Insert").
- Keyboard first: Writer `ctrl+b/i/u`, `ctrl+Return` page break; Calc `F2` edit
  cell, `ctrl+Home` go to A1, `ctrl+arrow` jump to data edge.
- Calc: click the Name Box (left of the formula bar) and type a cell (`B2`) or
  range (`A1:C10`); `Tab` moves right, `Return` moves down. `set_value` on a
  cell sets its content exactly.
- Formulas start with `=`; function arguments are separated by `;` or `,`
  depending on locale — read the result back to check.
- Save early with `ctrl+s` (Keep the current format? — choose as the user
  wants); export PDF: File ▸ Export as ▸ Export as PDF.
- Don't overwrite an existing file silently: read the "Replace?" prompt.
