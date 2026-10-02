## v3.7.0-batch — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 852 (852–852) | 16814 (16814–16814) | 2 (2–2) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.7) |
| table | 3/3 | 1537 (1537–1537) | 24594 (24594–24594) | 3 (3–3) | 1 (1–1) | 720 (720–720) | 1.5 (1.0–1.5) |
| board | 3/3 | 2033 (2022–2433) | 25350 (25328–39070) | 3 (3–5) | 2 (2–3) | 1544 (1544–1598) | 1.2 (1.1–1.3) |
| shapes | 3/3 | 1977 (1977–1977) | 31594 (31594–31594) | 4 (4–4) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.2–1.8) |
| orders | 3/3 | 750 (660–750) | 22172 (21992–22172) | 3 (3–3) | 2 (2–2) | 390 (249–390) | 1.2 (1.1–1.3) |
| long | 3/3 | 3158 (3158–3158) | 117960 (117960–117960) | 15 (15–15) | 1 (1–1) | 720 (720–720) | 6.4 (6.3–6.6) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~5099 tokens (est.): framing 58, instructions 527, skills 2186, tools 2328.
