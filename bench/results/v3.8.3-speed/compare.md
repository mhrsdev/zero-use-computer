## The fixed prefix: sent with every request

| | v3.8.2 | v3.8.3 |
|---|---|---|
| framing | 58 | 58 |
| instructions | 0 | 0 |
| skills | 2186 | 2186 |
| tools | 2167 | 2167 |
| **total** | **4411** | **4411** |
| tools listed | 17 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, board, shapes, orders, long), summed

| | v3.8.2 | v3.8.3 |
|---|---|---|
| framing (x turns) | 2668 | 2668 |
| instructions (x turns) | 0 | 0 |
| skills (x turns) | 100556 | 100556 |
| tools (x turns) | 99682 | 99682 |
| **prefix, all turns** | 202906 | 202906 |
| results: text | 4543 | 4542 |
| results: pictures | 3525 | 3525 |
| arguments the model wrote | 435 | 435 |
| conversation read again (history) | 66128 | 66124 |
| **input, no cache** | 269034 | 269030 |
| tool calls | 40 | 40 |
| pictures | 6 | 6 |
| seconds (tools only) | 13.9 | 11.9 |

Against v3.8.3:

| | v3.8.2 |
|---|---|
| prefix per request | +0% |
| input, no cache | -0% |
| tool results | -0% |
| tool calls | +0% |
| seconds | -15% |

(The change from each release to v3.8.3: negative is less.)

## Each scenario

| scenario | v3.8.2 | v3.8.3 |
|---|---|---|
| form | 35247 in · 793 res · 6 calls · 1.6 s | 35247 in · 793 res · 6 calls · 1.2 s |
| table | 27583 in · 1518 res · 4 calls · 1.5 s | 27583 in · 1518 res · 4 calls · 1.5 s |
| board | 15276 in · 1030 res · 2 calls · 1.6 s | 15276 in · 1030 res · 2 calls · 1.5 s |
| shapes | 26420 in · 1156 res · 4 calls · 1.5 s | 26416 in · 1155 res · 4 calls · 1.4 s |
| orders | 23887 in · 516 res · 4 calls · 1.1 s | 23887 in · 516 res · 4 calls · 1.0 s |
| long | 140621 in · 3055 res · 20 calls · 6.6 s | 140621 in · 3055 res · 20 calls · 5.3 s |


## Where the time went (v3.8.2 → v3.8.3, median of 3 scripted runs)

| | v3.8.2 | v3.8.3 |
|---|---:|---:|
| form: a click that changed nothing (after 4 that did) | 736 ms | 363 ms |
| board: a look read by OCR | 600 ms | 458 ms |
| shapes: a look read by OCR | 533 ms | 427 ms |
| long: a click (mean) | 342 ms | 203 ms |
| table: the first look at 270 elements | 149 ms | 144 ms |
| long: set_value (mean) | 306 ms | 315 ms |

The backend alone (`examples/bench.rs`, the Inventory window, 270
elements, 20 reads): 2148 → 1336 accessibility calls a read, 117 → 78–87
ms. The tool results are the same text, picture for picture (shapes'
one token is its process id, one digit shorter).
