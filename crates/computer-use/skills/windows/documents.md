# Office documents and spreadsheets (Windows)

- Word / Excel / PowerPoint and LibreOffice expose menus and ribbons in the
  tree; use `find_element` with the command name (for example "Bold", "Insert").
- Keyboard first: Word `ctrl+b/i/u`, `ctrl+Enter` page break; Excel `F2` edit
  cell, `ctrl+Home` go to A1, `ctrl+arrow` jump to data edge, `ctrl+;` today's date.
- Excel: click the Name Box (left of the formula bar) and type a cell (`B2`) or
  range (`A1:C10`) to jump/select; then type values, `Tab` moves right, `Return`
  moves down. `set_value` on a cell sets its content exactly.
- Formulas start with `=`. Verify results by reading the cell back, not just
  the screenshot.
- Save early with `ctrl+s`; export PDF: File ▸ Export ▸ Create PDF/XPS (Word) or
  File ▸ Save As ▸ PDF.
- Don't overwrite an existing file silently: read the "Replace?" prompt.
