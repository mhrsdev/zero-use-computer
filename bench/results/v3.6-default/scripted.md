## v3.6-default — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 809 (809–809) | 55573 (55573–55573) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.6) |
| table | 3/3 | 1518 (1518–1518) | 42073 (42073–42073) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (0.9–1.5) |
| board | 3/3 | 1196 (1196–1196) | 32446 (32446–32446) | 3 (3–3) | 1 (1–1) | 772 (772–772) | 1.3 (1.3–1.4) |
| shapes | 3/3 | 1753 (1753–1753) | 33655 (33655–33655) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 550 (550–550) | 46139 (46139–46139) | 5 (5–5) | 1 (1–1) | 195 (195–195) | 1.4 (1.3–1.4) |
| long | 3/3 | 3079 (3079–3079) | 201599 (201599–201599) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.3 (6.2–6.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~7309 tokens (est.).
