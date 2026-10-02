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
| `v3.6-default/` | v3.6.0 | defaults, `--plan step` |
| `v3.6-lean/` | v3.6.0 | `--config ../configs/lean.toml`, `--plan step` |
| `v3.6-batch/` | v3.6.0 | defaults, `--plan batch` |
| `v3.6-lean-batch/` | v3.6.0 | `lean.toml`, `--plan batch` |
| `v3.6-manager/` | v3.6.0 | `--config ../configs/manager.toml` (lean + the tool manager), `--plan batch` |

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

## v3.2 → v3.6 (scripted, 3 runs each, every run alike, every run succeeded)

Result tokens · calls · images (estimates). `step` is one action a call,
the way an agent on v3.2 works; `batch` uses v3.6's batch lines, clicks
by name and `expect` (`--plan batch`).

| scenario | v3.2 | v3.6 defaults, step | v3.6 lean, step | v3.6 defaults, batch | v3.6 lean, batch |
|---|---|---|---|---|---|
| form | 858 · 6 · 1 | 809 · 6 · 1 | 809 · 6 · 1 | 747 · **2** · 1 | 747 · **2** · 1 |
| table | 2485 · 4 · 1 | **1518** · 4 · 1 (−39%) | 1518 · 4 · 1 | 1468 · 3 · 1 | 1468 · 3 · 1 |
| board | 2012 · 4 · 2 | **1196 · 3 · 1** (−41%) | 1030 · 2 · 1 (−49%) | 1196 · 3 · 1 | 1030 · 2 · 1 |
| shapes | 1780 · 3 · 2 | 1753 · 3 · 2 | **1007** · 3 · **1** (−43%) | 1753 · 3 · 2 | 1007 · 3 · 1 |
| orders | 575 · 5 · 1 | 550 · 5 · 1 | 516 · 4 · 1 | 496 · 3 · 1 | **422 · 2** · 1 (−27%) |
| long | 4187 · 20 · 1 | **3079** · 20 · 1 (−26%) | 3079 · 20 · 1 | 3069 · **15** · 1 | 3069 · 15 · 1 |
| **all six** | 11,897 · 42 | 8,905 · 41 (−25%) | 7,959 · 39 (−33%) | 8,729 · 29 (−27%) | 7,743 · 27 (−35%) |

Estimated input over all six tasks (the prefix and everything before,
sent again with every call; **no prompt cache**): v3.2 428,255; v3.6
defaults 411,485 (−4%); lean 336,676 (−21%); batch 305,002 (−29%); lean +
batch 245,463 (−43%); lean + batch + the tool manager 191,244 (−55%).
With a prompt cache, as real clients have, the fixed prefix costs about a
tenth after the first request, so the real difference made by lighter
definitions and the manager is much smaller than these figures; the
real-model runs will say how much.

What did what:

- **Tables and the long session** (−39%, −26%): records and a row a line
  (`tree.compact`, on by default).
- **Board** (4 calls and 2 pictures → 3 and 1 with the defaults): OCR now
  reads every label when asked; with blind areas on (lean) the first look
  already has them, 2 calls.
- **Shapes** (2 pictures → 1, lean): `locate` without its picture.
- **Form, orders, long** (6 → 2, 5 → 2, 20 → 15 calls): batch lines with
  clicks by name; the click that opens the order dialog says `expect
  dialog`, so the batch goes on to Confirm.

Costs, published with the gains:

- **Every request** (defaults): the fixed prefix grew from ~6,941 to
  ~7,309 tokens (+5%): the tools gained `expect`, `name`, batch lines,
  `about` and design lines, and the skill says how to use them. That is
  why form, shapes and orders take 4–5% more input with the defaults when
  they are done one action a call. Lean definitions bring the prefix to
  ~6,040, the tool manager to ~4,397.
- **Time**: blind areas still cost about half a second per look at a
  canvas window (board 1.3 → 1.8 s, shapes 1.2 → 1.7 s); with lean
  settings and batches the long session takes 8.8 s instead of 6.3 s
  (each alone takes 6.3 s; why together they are slower is not yet looked
  into).
- `expect` waits up to two seconds for what doesn't come (only on calls
  that use it).

Not measured yet: whether a real model finishes as often, and with how
many turns, output tokens and retries, with each setting: wrong calls
caused by lighter schemas, steps a model would put in a batch, how often
it uses `expect`. That decides the defaults ([roadmap](../../docs/ROADMAP.md)).
