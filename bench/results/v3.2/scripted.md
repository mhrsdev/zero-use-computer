## v3.2 — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 858 (858–858) | 53290 (53290–53290) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.7) |
| table | 3/3 | 2485 (2485–2485) | 43981 (43981–43981) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (0.9–1.5) |
| board | 3/3 | 2012 (2012–2012) | 40767 (40767–40767) | 4 (4–4) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.3) |
| shapes | 3/3 | 1780 (1780–1780) | 32264 (32264–32264) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 575 (575–575) | 44032 (44032–44032) | 5 (5–5) | 1 (1–1) | 195 (195–195) | 1.0 (1.0–1.1) |
| long | 3/3 | 4187 (4187–4187) | 213921 (213921–213921) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.4 (6.3–6.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~6941 tokens (est.).
