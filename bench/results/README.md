# Benchmark results

Summaries kept to compare against; each folder holds the run's Markdown
summary and its JSON lines (every run, every call). How they are made:
[../README.md](../README.md).

**Scripted runs only so far.** Their token figures are estimates (text ≈ 4
characters a token, images width × height / 750; `input` assumes no prompt
cache and one call per turn), from a fixed way through each task. They
show what a change did to what the tools return, not what a model would
spend: the real-model runs (`ANTHROPIC_API_KEY=… bench/run.sh --runs 5`)
decide whether a change is turned on by default.

| folder | code | settings |
|---|---|---|
| `v3.2/` | v3.2.0, before any v3.5 change | defaults |
| `v3.5-default/` | v3.5 work in progress | defaults |
| `v3.5-blind/` | v3.5 work in progress | `--config ../configs/blind-regions.toml` (`ocr.blind_regions = true`) |

The scripted ways through were refined after the v3.2 run (they look for
painted text in the first look before asking for OCR); on v3.2 the first
look never has it, so v3.2 makes the same calls either way.

## v3.2 → v3.5 (scripted, 3 runs each, every run alike)

| scenario | | v3.2 | v3.5 defaults | v3.5 + blind areas |
|---|---|---|---|---|
| form | result tokens · calls · images | 858 · 6 · 1 | 858 · 6 · 1 | 858 · 6 · 1 |
| table | | 2485 · 4 · 1 | 2485 · 4 · 1 | 2485 · 4 · 1 |
| board | | 2012 · 4 · 2 | 2014 · 4 · 2 | **1069 · 2 · 1** (−47%) |
| shapes | | 1780 · 3 · 2 | 1780 · 3 · 2 | 1806 · 3 · 2 (+1.5%) |
| orders | | 575 · 5 · 1 | 577 · 5 · 1 | **515 · 4 · 1** (−10%) |
| long | | 4187 · 20 · 1 | 4187 · 20 · 1 | 4187 · 20 · 1 |

Estimated input over the task (no cache): board 40,767 → 22,995 (−44%),
orders 44,032 → 36,621 (−17%) with blind areas on.

Costs, published with the gains:

- **Time**: with blind areas on, board takes 1.8 s instead of 1.2 s, orders
  1.6 s instead of 1.0 s, shapes 1.6 s instead of 1.2 s: the areas are read
  twice by Tesseract (as they are and enlarged).
- **Shapes** (no text at all): +26 tokens, the note that part of the
  window has no accessibility information.
- **Every request**: the tool definitions grew by about 17 tokens (the
  `screenshot` tool says what it does with a window now): ~6,941 → ~6,958
  for the fixed prefix.
- Screenshot numbers ("Screenshot #3") add a token or two where a picture
  is sent.

Not yet measured: whether a real model finishes these tasks as often, and
in how many turns, with each setting. That decides `ocr.blind_regions`'s
default.
