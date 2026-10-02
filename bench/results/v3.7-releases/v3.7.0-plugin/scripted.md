## v3.7.0-plugin — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 862 (862–862) | 37726 (37726–37726) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.5–1.6) |
| table | 3/3 | 1587 (1587–1587) | 29334 (29334–29334) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.4 (1.0–1.5) |
| board | 3/3 | 2334 (2022–3100) | 36316 (23756–39380) | 5 (3–5) | 2 (2–3) | 1544 (1544–2316) | 1.2 (1.1–1.2) |
| shapes | 3/3 | 1977 (1977–1977) | 29629 (29629–29629) | 4 (4–4) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 709 (620–799) | 31227 (30871–31587) | 5 (5–5) | 2 (1–2) | 249 (195–390) | 1.2 (1.2–1.2) |
| long | 3/3 | 3124 (3124–3124) | 148196 (148196–148196) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.3 (6.2–6.3) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4706 tokens (est.): framing 58, instructions 133, skills 2186, tools 2328.
