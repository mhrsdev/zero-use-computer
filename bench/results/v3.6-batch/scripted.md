## v3.6-batch — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 747 (747–747) | 23270 (23270–23270) | 2 (2–2) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.6) |
| table | 3/3 | 1468 (1468–1468) | 33227 (33227–33227) | 3 (3–3) | 1 (1–1) | 720 (720–720) | 1.5 (0.9–1.5) |
| board | 3/3 | 1196 (1196–1196) | 32446 (32446–32446) | 3 (3–3) | 1 (1–1) | 772 (772–772) | 1.4 (1.4–1.6) |
| shapes | 3/3 | 1753 (1753–1753) | 33655 (33655–33655) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 496 (496–496) | 30439 (30439–30439) | 3 (3–3) | 1 (1–1) | 195 (195–195) | 1.4 (1.4–1.4) |
| long | 3/3 | 3069 (3069–3069) | 151965 (151965–151965) | 15 (15–15) | 1 (1–1) | 720 (720–720) | 6.3 (6.2–6.4) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~7309 tokens (est.).
