## v3.6.0 — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 878 (878–878) | 59305 (59305–59305) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.7) |
| table | 3/3 | 1587 (1587–1587) | 44719 (44719–44719) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (1.0–1.5) |
| board | 3/3 | 2033 (1579–2945) | 36086 (35178–47639) | 3 (3–4) | 2 (2–3) | 1544 (1051–2316) | 1.5 (1.4–1.5) |
| shapes | 3/3 | 1822 (1822–1822) | 35758 (35758–35758) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 799 (709–799) | 50049 (49689–50049) | 5 (5–5) | 2 (2–2) | 390 (249–390) | 1.5 (1.5–1.5) |
| long | 3/3 | 3148 (3148–3148) | 212933 (212933–212933) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.4 (6.2–6.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~7783 tokens (est.): framing 58, instructions 474, skills 2261, tools 4990.
