## v0.1.0 — scripted runs

Scripted calls, no model: token figures are estimates (text ≈ 4 characters a token, images width × height / 750). `input` assumes no prompt cache and one call per turn.

| scenario | success | results tok (est.) | input tok (est.) | calls | images | image tok (est.) | seconds |
|---|---|---|---|---|---|---|---|
| form | 3/3 | 921 (921–921) | 33303 (33303–33303) | 6 (6–6) | 1 (1–1) | 346 (346–346) | 1.0 (0.9–1.0) |
| table | 3/3 | 2471 (2471–2471) | 29584 (29584–29584) | 4 (4–4) | 1 (1–1) | 720 (720–720) | 1.8 (1.6–1.8) |
| board | 0/3 | 1089 (1080–1098) | 19382 (19364–19400) | 3 (3–3) | 1 (1–1) | 772 (772–772) | 1.1 (1.0–1.1) |
| shapes | 0/3 | 948 (948–948) | 14087 (14087–14087) | 2 (2–2) | 1 (1–1) | 772 (772–772) | 0.3 (0.3–0.3) |
| orders | 3/3 | 549 (549–549) | 26660 (26660–26660) | 5 (5–5) | 1 (1–1) | 195 (195–195) | 1.1 (1.1–1.1) |
| long | 3/3 | 4861 (4861–4861) | 158681 (158681–158681) | 20 (20–20) | 1 (1–1) | 720 (720–720) | 8.4 (8.4–8.5) |

- `form`: a form: named fields, radio buttons, a check box
- `table`: a 300-row table with a filter
- `board`: a canvas of labelled boxes under a full toolbar (text the tree doesn't have)
- `shapes`: a canvas of shapes without any text
- `orders`: mixed: painted text, a field, a confirmation dialog
- `long`: a longer session: the same table, five items in turn

Fixed prefix (system prompt + tool definitions): ~4063 tokens (est.): framing 58, instructions 273, skills 1899, tools 1834.
