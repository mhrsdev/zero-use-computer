## v3.5-blind — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 858 (858–858) | 53409 (53409–53409) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.6) |
| table | 3/3 | 2485 (2485–2485) | 44066 (44066–44066) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (0.9–1.5) |
| board | 3/3 | 1069 (1069–1069) | 22995 (22995–22995) | 2 (2–2) | 1 (1–1) | 772 (772–772) | 1.8 (1.7–1.8) |
| shapes | 3/3 | 1806 (1806–1806) | 32410 (32410–32410) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.6 (1.6–1.6) |
| orders | 3/3 | 515 (515–515) | 36621 (36621–36621) | 4 (4–4) | 1 (1–1) | 195 (195–195) | 1.6 (1.5–1.6) |
| long | 3/3 | 4187 (4187–4187) | 214278 (214278–214278) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.3 (6.3–6.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~6958 tokens (est.).
