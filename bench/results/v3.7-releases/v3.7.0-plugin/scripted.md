## v3.7.0-plugin — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 888 (888–888) | 36755 (36755–36755) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.9 (1.8–1.9) |
| table | 3/3 | 1587 (1587–1587) | 28529 (28529–28529) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (1.0–1.5) |
| board | 3/3 | 1979 (1099–1979) | 23197 (15816–23197) | 3 (2–3) | 2 (1–2) | 1544 (772–1544) | 1.9 (1.9–2.1) |
| shapes | 3/3 | 1225 (1225–1225) | 27366 (27366–27366) | 4 (4–4) | 1 (1–1) | 772 (772–772) | 1.5 (1.4–1.5) |
| orders | 3/3 | 590 (590–590) | 24862 (24862–24862) | 4 (4–4) | 1 (1–1) | 195 (195–195) | 1.4 (1.2–1.4) |
| long | 3/3 | 3124 (3124–3124) | 144815 (144815–144815) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.4 (6.4–6.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4545 tokens (est.): framing 58, instructions 133, skills 2186, tools 2167.
