## codex-sim — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 621 (621–621) | 45350 (45350–45350) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.5 (1.5–1.6) |
| table | 3/3 | 2043 (2043–2043) | 44280 (44280–44280) | 5 (5–5) | 2 (2–2) | 1440 (1440–1440) | 1.5 (0.9–1.5) |
| board | 3/3 | 1840 (1840–1840) | 28410 (28410–28410) | 3 (3–3) | 2 (2–2) | 1544 (1544–1544) | 1.2 (1.1–1.2) |
| shapes | 3/3 | 981 (981–981) | 26675 (26675–26675) | 3 (3–3) | 1 (1–1) | 772 (772–772) | 1.2 (1.2–1.8) |
| orders | 0/3 | 596 (596–596) | 31845 (31845–31845) | 4 (4–4) | 2 (2–2) | 390 (390–390) | 0.7 (0.7–0.8) |
| long | 3/3 | 4916 (4916–4916) | 219639 (219639–219639) | 24 (24–24) | 5 (5–5) | 3600 (3600–3600) | 6.2 (6.2–6.3) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~5950 tokens (est.): framing 58, instructions 0, skills 2186, tools 3706.
