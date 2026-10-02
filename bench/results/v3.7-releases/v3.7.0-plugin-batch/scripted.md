## v3.7.0-plugin-batch — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 931 (878–936) | 15296 (15204–15306) | 2 (2–2) | 1 (1–1) | 346 (346–346) | 2.1 (2.0–2.1) |
| table | 3/3 | 1537 (1537–1537) | 22378 (22378–22378) | 3 (3–3) | 1 (1–1) | 720 (720–720) | 1.5 (1.4–1.6) |
| board | 3/3 | 1979 (1099–1979) | 23197 (15816–23197) | 3 (2–3) | 2 (1–2) | 1544 (772–1544) | 1.9 (1.8–2.1) |
| shapes | 3/3 | 1225 (1225–1225) | 27366 (27366–27366) | 4 (4–4) | 1 (1–1) | 772 (772–772) | 1.5 (1.5–1.5) |
| orders | 3/3 | 543 (543–543) | 14632 (14632–14632) | 2 (2–2) | 1 (1–1) | 195 (195–195) | 1.4 (1.3–1.4) |
| long | 3/3 | 3158 (3158–3158) | 109096 (109096–109096) | 15 (15–15) | 1 (1–1) | 720 (720–720) | 7.2 (7.1–7.3) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4545 tokens (est.): framing 58, instructions 133, skills 2186, tools 2167.
