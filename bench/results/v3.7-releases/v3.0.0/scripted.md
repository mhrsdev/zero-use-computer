## v3.0.0 — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 927 (927–927) | 52143 (52143–52143) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–2.2) |
| table | 3/3 | 2559 (2559–2559) | 43154 (43154–43154) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (1.0–1.5) |
| board | 3/3 | 2204 (2005–2442) | 40274 (39697–40982) | 4 (4–4) | 3 (2–3) | 1598 (1544–1823) | 1.2 (1.1–1.8) |
| shapes | 3/3 | 1849 (1849–1849) | 31579 (31579–31579) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 824 (824–824) | 43760 (43760–43760) | 5 (5–5) | 2 (2–2) | 390 (390–390) | 1.2 (1.2–1.2) |
| long | 3/3 | 4272 (4272–4272) | 210816 (210816–210816) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.4 (6.2–6.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~6718 tokens (est.): framing 58, instructions 474, skills 1823, tools 4363.
