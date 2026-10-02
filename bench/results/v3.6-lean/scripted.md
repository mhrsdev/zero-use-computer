## v3.6-lean — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 809 (809–809) | 46690 (46690–46690) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.6 (1.6–1.6) |
| table | 3/3 | 1518 (1518–1518) | 35728 (35728–35728) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (0.9–1.5) |
| board | 3/3 | 1030 (1030–1030) | 20163 (20163–20163) | 2 (2–2) | 1 (1–1) | 772 (772–772) | 1.8 (1.7–1.9) |
| shapes | 3/3 | 1007 (1007–1007) | 27113 (27113–27113) | 3 (3–3) | 1 (1–1) | 772 (772–772) | 1.7 (1.6–1.7) |
| orders | 3/3 | 516 (516–516) | 32032 (32032–32032) | 4 (4–4) | 1 (1–1) | 195 (195–195) | 1.6 (1.6–1.7) |
| long | 3/3 | 3079 (3079–3079) | 174950 (174950–174950) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.3 (6.3–6.4) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~6040 tokens (est.).
