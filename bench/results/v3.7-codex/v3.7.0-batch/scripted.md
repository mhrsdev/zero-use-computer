## v3.7.0-batch — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 783 (783–783) | 14612 (14612–14612) | 2 (2–2) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.7) |
| table | 3/3 | 1468 (1468–1468) | 21635 (21635–21635) | 3 (3–3) | 1 (1–1) | 720 (720–720) | 1.4 (1.3–1.5) |
| board | 3/3 | 1030 (1030–1030) | 15276 (15276–15276) | 2 (2–2) | 1 (1–1) | 772 (772–772) | 1.4 (1.3–1.5) |
| shapes | 3/3 | 1156 (1156–1156) | 26420 (26420–26420) | 4 (4–4) | 1 (1–1) | 772 (772–772) | 1.4 (1.4–1.6) |
| orders | 3/3 | 458 (458–458) | 14061 (14061–14061) | 2 (2–2) | 1 (1–1) | 195 (195–195) | 1.0 (0.9–1.1) |
| long | 3/3 | 3089 (3089–3089) | 105917 (105917–105917) | 15 (15–15) | 1 (1–1) | 720 (720–720) | 6.9 (6.9–7.0) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4411 tokens (est.): framing 58, instructions 0, skills 2186, tools 2167.
