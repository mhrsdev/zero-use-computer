## v3.7.0 — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 862 (862–862) | 40477 (40477–40477) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.8) |
| table | 3/3 | 1587 (1587–1587) | 31299 (31299–31299) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (1.0–1.5) |
| board | 3/3 | 2344 (1266–2433) | 38714 (23816–39070) | 5 (3–5) | 2 (1–3) | 1544 (772–1598) | 1.2 (1.1–1.3) |
| shapes | 3/3 | 1977 (1977–1977) | 31594 (31594–31594) | 4 (4–4) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 799 (724–799) | 33945 (33645–33945) | 5 (5–5) | 2 (2–2) | 390 (265–390) | 1.2 (1.1–1.3) |
| long | 3/3 | 3124 (3124–3124) | 156449 (156449–156449) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.4 (6.3–6.4) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~5099 tokens (est.): framing 58, instructions 527, skills 2186, tools 2328.
