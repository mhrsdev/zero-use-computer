## v3.7.0-batch — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 878 (878–936) | 16383 (16383–16485) | 2 (2–2) | 1 (1–1) | 346 (346–346) | 2.1 (2.0–2.1) |
| table | 3/3 | 1537 (1537–1537) | 23950 (23950–23950) | 3 (3–3) | 1 (1–1) | 720 (720–720) | 1.5 (1.5–1.7) |
| board | 3/3 | 1979 (1099–1979) | 24769 (16995–24769) | 3 (2–3) | 2 (1–2) | 1544 (772–1544) | 1.8 (1.8–2.0) |
| shapes | 3/3 | 1225 (1225–1225) | 29331 (29331–29331) | 4 (4–4) | 1 (1–1) | 772 (772–772) | 1.5 (1.4–1.5) |
| orders | 3/3 | 543 (543–543) | 15811 (15811–15811) | 2 (2–2) | 1 (1–1) | 195 (195–195) | 1.3 (1.3–1.5) |
| long | 3/3 | 3158 (3158–3158) | 115384 (115384–115384) | 15 (15–15) | 1 (1–1) | 720 (720–720) | 7.5 (7.2–7.6) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4938 tokens (est.): framing 58, instructions 527, skills 2186, tools 2167.
