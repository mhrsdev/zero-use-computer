## The fixed prefix: sent with every request

| | codex-sim | v3.7.0 | v3.7.0-batch |
|---|---|---|---|
| framing | 58 | 58 | 58 |
| instructions | 0 | 0 | 0 |
| skills | 2186 | 2186 | 2186 |
| tools | 3706 | 2167 | 2167 |
| **total** | **5950** | **4411** | **4411** |
| tools listed | 24 | 17 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, board, shapes, long), summed

| | codex-sim | v3.7.0 | v3.7.0-batch |
|---|---|---|---|
| framing (x turns) | 2668 | 2378 | 1798 |
| instructions (x turns) | 0 | 0 | 0 |
| skills (x turns) | 100556 | 89626 | 67766 |
| tools (x turns) | 170476 | 88847 | 67177 |
| **prefix, all turns** | 273700 | 180851 | 136741 |
| results: text | 2699 | 4222 | 4196 |
| results: pictures | 7702 | 3330 | 3330 |
| arguments the model wrote | 419 | 399 | 323 |
| conversation read again (history) | 90654 | 64296 | 47119 |
| **input, no cache** | 364354 | 245147 | 183860 |
| tool calls | 41 | 36 | 26 |
| pictures | 11 | 5 | 5 |
| seconds (tools only) | 11.7 | 12.0 | 12.7 |

Against v3.7.0:

| | codex-sim | v3.7.0-batch |
|---|---|---|
| prefix per request | -26% | +0% |
| input, no cache | -33% | +33% |
| tool results | -27% | +0% |
| tool calls | -12% | +38% |
| seconds | +2% | -6% |

(The change from each release to v3.7.0: negative is less.)

## Each scenario

| scenario | codex-sim | v3.7.0 | v3.7.0-batch |
|---|---|---|---|
| form | 45350 in · 621 res · 6 calls · 1.6 s | 35247 in · 793 res · 6 calls · 1.5 s | 14612 in · 783 res · 2 calls · 1.6 s |
| table | 44280 in · 2043 res · 5 calls · 1.5 s | 27583 in · 1518 res · 4 calls · 1.5 s | 21635 in · 1468 res · 3 calls · 1.4 s |
| board | 28410 in · 1840 res · 3 calls · 1.2 s | 15276 in · 1030 res · 2 calls · 1.3 s | 15276 in · 1030 res · 2 calls · 1.4 s |
| shapes | 26675 in · 981 res · 3 calls · 1.2 s | 26420 in · 1156 res · 4 calls · 1.4 s | 26420 in · 1156 res · 4 calls · 1.4 s |
| orders | fails (0/3) | 23887 in · 516 res · 4 calls · 1.0 s | 14061 in · 458 res · 2 calls · 1.0 s |
| long | 219639 in · 4916 res · 24 calls · 6.2 s | 140621 in · 3055 res · 20 calls · 6.2 s | 105917 in · 3089 res · 15 calls · 6.9 s |

