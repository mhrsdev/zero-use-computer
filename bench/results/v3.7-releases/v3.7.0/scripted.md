## v3.7.0 — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 1192 (888–1192) | 40312 (39506–40312) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.9 (1.8–1.9) |
| table | 3/3 | 1587 (1587–1587) | 30494 (30494–30494) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (1.0–1.6) |
| board | 3/3 | 1979 (1099–1979) | 24769 (16995–24769) | 3 (2–3) | 2 (1–2) | 1544 (772–1544) | 1.9 (1.8–2.1) |
| shapes | 3/3 | 1225 (1225–1225) | 29331 (29331–29331) | 4 (4–4) | 1 (1–1) | 772 (772–772) | 1.5 (1.4–1.5) |
| orders | 3/3 | 590 (590–590) | 26827 (26827–26827) | 4 (4–4) | 1 (1–1) | 195 (195–195) | 1.3 (1.3–1.3) |
| long | 3/3 | 3124 (3124–3124) | 153068 (153068–153068) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.4 (6.3–6.4) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4938 tokens (est.): framing 58, instructions 527, skills 2186, tools 2167.
