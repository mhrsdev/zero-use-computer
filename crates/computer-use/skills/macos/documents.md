# Office documents and spreadsheets (macOS)

- Pages / Numbers / Keynote, Microsoft Office and LibreOffice expose menus in
  the menu bar and toolbars in the tree; use `find_element` with the command
  name ("Bold", "Insert").
- Keyboard first: `cmd+b/i/u`; Numbers/Excel: `Return` edits/commits a cell,
  `Tab` moves right, `cmd+arrow` jumps to the data edge.
- Excel for Mac: click the Name Box and type a cell (`B2`) or range (`A1:C10`);
  Numbers: click the cell. `set_value` on a cell sets its content exactly.
- Formulas start with `=`. Verify by reading the cell back, not only the
  screenshot.
- Save early with `cmd+s`; export PDF: File ▸ Export To ▸ PDF (Pages/Numbers) or
  File ▸ Print ▸ PDF ▸ Save as PDF.
- Don't overwrite an existing file silently: read the "Replace?" sheet.
