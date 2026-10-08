# Office apps: documents, spreadsheets, slides

Microsoft Office (Windows, macOS), LibreOffice (all; on Linux `launch_app`
`"LibreOffice Writer"` / `"LibreOffice Calc"` / `"LibreOffice Impress"`),
Apple Pages / Numbers / Keynote, or the web versions in a browser. `cmd+`
below is Cmd on a Mac, Ctrl elsewhere. The ribbon and menus are in the tree:
`find_element(name="Page Break")` is cheaper than reading it all.

## Before you change a file

Keep a copy (Save As a new name) before big or destructive edits, and read
every "Replace?" / "Don't Save" prompt (security rule 3). Saving as `.csv`
loses formulas and formatting. Protected View / "Enable Editing": only if
the user trusts the file. With Track Changes on, say so; never "Accept All"
unless asked.

## Documents (Word, Writer, Pages)

- Click into the page, then `type_text`: `\n` is a new paragraph,
  `shift+Return` a line break inside one.
- Use styles, not manual formatting: Word headings `cmd+alt+1/2/3`, Writer
  `cmd+1/2/3` (body `cmd+0`). Bold/italic/underline `cmd+b/i/u`, link
  `cmd+k`, page break `cmd+Return`, find / replace `cmd+f` / `cmd+h`
  (Word on a Mac: `cmd+shift+h`).
- PDF: File ▸ Export (Writer: Export as PDF).

## Spreadsheets (Excel, Calc, Numbers)

- Jump: click the Name Box left of the formula bar, `type_text` `B2` or
  `A1:D20`, `Return`. `ctrl+Home` goes to A1, `cmd+arrow` to the data's edge
  (add `shift` to select).
- `set_value` on a cell sets it exactly. Typing: `Tab` moves right, `Return`
  down, `F2` edits in place (Excel on a Mac: `ctrl+u`), `Escape` cancels.
- Formulas start with `=`; the argument separator follows the locale (`,` or
  `;`). Fill down `cmd+d`. Read the result back from the cell, not the
  picture.
- Many values at once: `set_clipboard` with tab-separated columns and one row
  per line, select the top-left cell, `cmd+v` (Calc may ask about the
  separator).
- Sheets: `ctrl+PageDown` / `ctrl+PageUp`. Deleting rows or columns, sorting
  and removing duplicates move data: confirm first.

## Slides (PowerPoint, Impress, Keynote)

- New slide: PowerPoint `ctrl+m` on Windows, `cmd+shift+n` on a Mac (also
  Keynote); Impress: Slide ▸ New Slide. Click a placeholder, then
  `type_text`; in a bullet list `Tab` demotes, `shift+Tab` promotes.
- Many slides: the Outline view (each line a title, `Tab` makes a bullet).
- Change fonts and colours in the design / master, not slide by slide.
- Present: `F5` on Windows and in Impress, the Slide Show / Play menu on a
  Mac; stop with `Escape`. Presenting doesn't change the file.
