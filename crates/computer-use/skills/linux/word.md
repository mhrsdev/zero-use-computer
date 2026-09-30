# Word processing on Linux (LibreOffice Writer; Word on the web)

Microsoft Word has no Linux desktop app. Use **LibreOffice Writer**
(`launch_app("libreoffice --writer")` or `lowriter`) or Word on the web
(office.com in a browser — see the `browser` skills; the UI is the same as Word).

- New `ctrl+n`; open `ctrl+o`; save `ctrl+s` (choose "Keep current format" for
  .docx); Save As `ctrl+shift+s`; export PDF: File ▸ Export as ▸ Export as PDF.
- Format: `ctrl+b/i/u`; headings `ctrl+1/2/3`, body text `ctrl+0`; align
  `ctrl+l/e/r/j`; link `ctrl+k`; page break `ctrl+Return`.
- Editing: find & replace `ctrl+h`; find `ctrl+f`; select all `ctrl+a`; undo
  `ctrl+z`; redo `ctrl+y` (or `ctrl+shift+z`); spelling `F7`.
- Track Changes: Edit ▸ Track Changes ▸ Record (`ctrl+shift+c`). Edits show as
  markup — say so; never accept all unless asked.
- Type body text with `type_text` after clicking into the page; newline is a new
  paragraph, `shift+Return` a line break.
- Use paragraph styles (sidebar `F11`) instead of manual formatting.
- Table: Table ▸ Insert Table (`ctrl+F12`); `Tab` moves between cells.
- Formatting of .docx may shift slightly; tell the user if fidelity matters.
- Don't overwrite an existing file silently; read the "Replace?" prompt.
