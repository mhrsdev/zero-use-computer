# Every minor version, as it ships: `--plan batch` (v3.6 on)

The newest patch of each minor version, downloaded from its GitHub
release (Linux x64), run over MCP with its own skills and instructions
(`--server`, `--skills`; v0.1.5 and v2.0.0 with `--server-args "--approval
allow-all --headless-approve allow"` and `[guard] mode = "allow"`).
**Every task in a desktop of its own** (`--runs 1 --scenarios <one>`),
three rounds, the server's helper processes ended between runs; medians.
Scripted estimates, no prompt cache: see [the limits](../../../README.md#version-history-and-token-use).

## The fixed prefix: sent with every request

| | v3.6.0-batch | v3.7.5-batch | v3.8.3-batch | v3.9.0-batch |
|---|---|---|---|---|
| framing | 58 | 58 | 58 | 58 |
| instructions | 474 | 527 | 527 | 527 |
| skills | 2261 | 2186 | 2186 | 2222 |
| tools | 4990 | 2167 | 2167 | 2165 |
| **total** | **7783** | **4938** | **4938** | **4972** |
| tools listed | 25 | 17 | 17 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, board, shapes, orders, long), summed

| | v3.6.0-batch | v3.7.5-batch | v3.8.3-batch | v3.9.0-batch |
|---|---|---|---|---|
| framing (x turns) | 2030 | 1972 | 1972 | 1972 |
| instructions (x turns) | 16590 | 17918 | 17918 | 17918 |
| skills (x turns) | 79135 | 74324 | 74324 | 75548 |
| tools (x turns) | 174650 | 73678 | 73678 | 73610 |
| **prefix, all turns** | 272405 | 167892 | 167892 | 169048 |
| results: text | 4858 | 4915 | 4915 | 4458 |
| results: pictures | 4445 | 3525 | 3525 | 3525 |
| arguments the model wrote | 354 | 352 | 352 | 352 |
| conversation read again (history) | 51577 | 49962 | 49962 | 47943 |
| **input, no cache** | 323982 | 217854 | 217854 | 216991 |
| tool calls | 29 | 28 | 28 | 28 |
| pictures | 8 | 6 | 6 | 6 |
| seconds (tools only) | 14.6 | 18.1 | 19.8 | 19.1 |

Against v3.9.0-batch:

| | v3.6.0-batch | v3.7.5-batch | v3.8.3-batch |
|---|---|---|---|
| prefix per request | -36% | +1% | +1% |
| input, no cache | -33% | -0% | -0% |
| tool results | -15% | -5% | -5% |
| tool calls | -3% | +0% | +0% |
| seconds | +31% | +6% | -4% |

(The change from each release to v3.9.0-batch: negative is less.)

## Each scenario

| scenario | v3.6.0-batch | v3.7.5-batch | v3.8.3-batch | v3.9.0-batch |
|---|---|---|---|---|
| form | 24830 in · 816 res · 2 calls · 1.8 s | 16383 in · 878 res · 2 calls · 2.4 s | 16383 in · 878 res · 2 calls · 2.7 s | 16295 in · 783 res · 2 calls · 2.4 s |
| table | 35330 in · 1537 res · 3 calls · 1.1 s | 23950 in · 1537 res · 3 calls · 1.6 s | 23950 in · 1537 res · 3 calls · 1.9 s | 23879 in · 1468 res · 3 calls · 1.9 s |
| board | 34935 in · 1458 res · 3 calls · 1.7 s | 16995 in · 1099 res · 2 calls · 2.4 s | 16995 in · 1099 res · 2 calls · 2.0 s | 16959 in · 1030 res · 2 calls · 2.0 s |
| shapes | 35758 in · 1822 res · 3 calls · 1.1 s | 29331 in · 1225 res · 4 calls · 1.7 s | 29331 in · 1225 res · 4 calls · 1.6 s | 29221 in · 1155 res · 4 calls · 1.6 s |
| orders | 32545 in · 566 res · 3 calls · 1.8 s | 15811 in · 543 res · 2 calls · 1.5 s | 15811 in · 543 res · 2 calls · 2.0 s | 15744 in · 458 res · 2 calls · 1.7 s |
| long | 160584 in · 3138 res · 15 calls · 7.2 s | 115384 in · 3158 res · 15 calls · 8.4 s | 115384 in · 3158 res · 15 calls · 9.6 s | 114893 in · 3089 res · 15 calls · 9.5 s |

