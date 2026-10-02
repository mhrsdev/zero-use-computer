## v3.6-manager — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 747 (747–747) | 14534 (14534–14534) | 2 (2–2) | 1 (1–1) | 346 (346–346) | 1.7 (1.7–1.7) |
| table | 3/3 | 1468 (1468–1468) | 21579 (21579–21579) | 3 (3–3) | 1 (1–1) | 720 (720–720) | 1.5 (1.4–1.5) |
| board | 3/3 | 1030 (1030–1030) | 15234 (15234–15234) | 2 (2–2) | 1 (1–1) | 772 (772–772) | 1.6 (1.6–1.6) |
| shapes | 3/3 | 1007 (1007–1007) | 20541 (20541–20541) | 3 (3–3) | 1 (1–1) | 772 (772–772) | 1.7 (1.7–1.7) |
| orders | 3/3 | 422 (422–422) | 13983 (13983–13983) | 2 (2–2) | 1 (1–1) | 195 (195–195) | 1.7 (1.6–1.7) |
| long | 3/3 | 3069 (3069–3069) | 105373 (105373–105373) | 15 (15–15) | 1 (1–1) | 720 (720–720) | 8.7 (8.5–8.7) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4397 tokens (est.).
