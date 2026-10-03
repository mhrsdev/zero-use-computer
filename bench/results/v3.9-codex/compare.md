# Codex-style behaviour against v3.9 (simulated, in process)

`--preset codex` (a screenshot with every look, no screen memory, no
picture dedupe, no change report, every tool listed, no blind areas or
adaptive pictures) against v3.9.0 defaults and `--plan batch`. A
simulation of how Codex's computer use behaves, on this server: not a run
of Codex or of a GPT model. Every task in a desktop of its own
(`--runs 1 --scenarios <one>`), three rounds, medians. Scripted
estimates, no prompt cache. orders is left out: its scripted way reads
the dialog's Confirm button from the change report, which the
Codex-style run doesn't have.

## The fixed prefix: sent with every request

| | codex-sim | v3.9.0 | v3.9.0-batch |
|---|---|---|---|
| framing | 58 | 58 | 58 |
| instructions | 0 | 0 | 0 |
| skills | 2222 | 2222 | 2222 |
| tools | 3827 | 2165 | 2165 |
| **total** | **6107** | **4445** | **4445** |
| tools listed | 25 | 17 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, board, shapes, long), summed

| | codex-sim | v3.9.0 | v3.9.0-batch |
|---|---|---|---|
| framing (x turns) | 2668 | 2378 | 1798 |
| instructions (x turns) | 0 | 0 | 0 |
| skills (x turns) | 102212 | 91102 | 68882 |
| tools (x turns) | 176042 | 88765 | 67115 |
| **prefix, all turns** | 280922 | 182245 | 137795 |
| results: text | 2699 | 4221 | 4195 |
| results: pictures | 7702 | 3330 | 3330 |
| arguments the model wrote | 419 | 399 | 323 |
| conversation read again (history) | 90654 | 64292 | 47115 |
| **input, no cache** | 371576 | 246537 | 184910 |
| tool calls | 41 | 36 | 26 |
| pictures | 11 | 5 | 5 |
| seconds (tools only) | 9.3 | 10.1 | 11.3 |

Against v3.9.0:

| | codex-sim | v3.9.0-batch |
|---|---|---|
| prefix per request | -27% | +0% |
| input, no cache | -34% | +33% |
| tool results | -27% | +0% |
| tool calls | -12% | +38% |
| seconds | +9% | -10% |

(The change from each release to v3.9.0: negative is less.)

## Each scenario

| scenario | codex-sim | v3.9.0 | v3.9.0-batch |
|---|---|---|---|
| form | 46449 in · 621 res · 6 calls · 1.2 s | 35485 in · 793 res · 6 calls · 1.2 s | 14714 in · 783 res · 2 calls · 1.3 s |
| table | 45222 in · 2043 res · 5 calls · 0.9 s | 27753 in · 1518 res · 4 calls · 0.9 s | 21771 in · 1468 res · 3 calls · 1.3 s |
| board | 29038 in · 1840 res · 3 calls · 1.1 s | 15378 in · 1030 res · 2 calls · 1.5 s | 15378 in · 1030 res · 2 calls · 1.5 s |
| shapes | 27303 in · 981 res · 3 calls · 1.0 s | 26586 in · 1155 res · 4 calls · 1.5 s | 26586 in · 1155 res · 4 calls · 1.4 s |
| long | 223564 in · 4916 res · 24 calls · 5.1 s | 141335 in · 3055 res · 20 calls · 5.1 s | 106461 in · 3089 res · 15 calls · 5.8 s |

