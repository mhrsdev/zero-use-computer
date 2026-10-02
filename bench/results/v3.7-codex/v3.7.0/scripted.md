## v3.7.0 — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 793 (793–819) | 35247 (35247–35403) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.5 (1.5–1.8) |
| table | 3/3 | 1518 (1518–1518) | 27583 (27583–27583) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.5 (0.9–1.5) |
| board | 3/3 | 1030 (1030–1030) | 15276 (15276–15276) | 2 (2–2) | 1 (1–1) | 772 (772–772) | 1.3 (1.3–1.7) |
| shapes | 3/3 | 1156 (1156–1156) | 26420 (26420–26420) | 4 (4–4) | 1 (1–1) | 772 (772–772) | 1.4 (1.4–1.5) |
| orders | 3/3 | 516 (516–516) | 23887 (23887–23887) | 4 (4–4) | 1 (1–1) | 195 (195–195) | 1.0 (0.9–1.1) |
| long | 3/3 | 3055 (3055–3055) | 140621 (140621–140621) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 6.2 (6.1–6.4) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4411 tokens (est.): framing 58, instructions 0, skills 2186, tools 2167.
