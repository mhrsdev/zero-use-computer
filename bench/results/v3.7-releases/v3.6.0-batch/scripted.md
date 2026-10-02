## v3.6.0-batch — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 816 (816–816) | 24830 (24830–24830) | 2 (2–2) | 1 (1–1) | 346 (346–346) | 1.7 (1.6–1.7) |
| table | 3/3 | 1537 (1537–1537) | 35330 (35330–35330) | 3 (3–3) | 1 (1–1) | 720 (720–720) | 1.5 (1.0–1.5) |
| board | 3/3 | 2032 (2022–2033) | 36084 (36064–36086) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.4 (1.3–1.4) |
| shapes | 3/3 | 1822 (1822–1822) | 35758 (35758–35758) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 670 (655–745) | 32753 (32723–32903) | 3 (3–3) | 2 (2–2) | 265 (249–390) | 1.4 (1.4–1.5) |
| long | 3/3 | 3138 (3138–3138) | 160584 (160584–160584) | 15 (15–15) | 1 (1–1) | 720 (720–720) | 6.4 (6.4–6.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~7783 tokens (est.): framing 58, instructions 474, skills 2261, tools 4990.
