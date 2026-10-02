## The fixed prefix: sent with every request

| | v3.6.0-batch | v3.7.0-batch | v3.7.0-plugin-batch |
|---|---|---|---|
| framing | 58 | 58 | 58 |
| instructions | 474 | 527 | 133 |
| skills | 2261 | 2186 | 2186 |
| tools | 4990 | 2167 | 2167 |
| **total** | **7783** | **4938** | **4545** |
| tools listed | 25 | 17 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, board, shapes, orders, long), summed

| | v3.6.0-batch | v3.7.0-batch | v3.7.0-plugin-batch |
|---|---|---|---|
| framing (x turns) | 2030 | 2030 | 2030 |
| instructions (x turns) | 16590 | 18445 | 4655 |
| skills (x turns) | 79135 | 76510 | 76510 |
| tools (x turns) | 174650 | 75845 | 75845 |
| **prefix, all turns** | 272405 | 172830 | 159075 |
| results: text | 4876 | 5023 | 5076 |
| results: pictures | 5139 | 4297 | 4297 |
| arguments the model wrote | 354 | 359 | 359 |
| conversation read again (history) | 52934 | 52798 | 52890 |
| **input, no cache** | 325339 | 225628 | 211965 |
| tool calls | 29 | 29 | 29 |
| pictures | 9 | 7 | 7 |
| seconds (tools only) | 13.7 | 15.7 | 15.4 |

Against v3.7.0-batch:

| | v3.6.0-batch | v3.7.0-plugin-batch |
|---|---|---|
| prefix per request | -37% | +9% |
| input, no cache | -31% | +6% |
| tool results | -7% | -1% |
| tool calls | +0% | +0% |
| seconds | +15% | +2% |

(The change from each release to v3.7.0-batch: negative is less.)

## Each scenario

| scenario | v3.6.0-batch | v3.7.0-batch | v3.7.0-plugin-batch |
|---|---|---|---|
| form | 24830 in · 816 res · 2 calls · 1.6 s | 16383 in · 878 res · 2 calls · 2.1 s | 15296 in · 931 res · 2 calls · 2.1 s |
| table | 35330 in · 1537 res · 3 calls · 1.5 s | 23950 in · 1537 res · 3 calls · 1.5 s | 22378 in · 1537 res · 3 calls · 1.5 s |
| board | 36084 in · 2032 res · 3 calls · 1.4 s | 24769 in · 1979 res · 3 calls · 1.8 s | 23197 in · 1979 res · 3 calls · 1.9 s |
| shapes | 35758 in · 1822 res · 3 calls · 1.2 s | 29331 in · 1225 res · 4 calls · 1.5 s | 27366 in · 1225 res · 4 calls · 1.5 s |
| orders | 32753 in · 670 res · 3 calls · 1.4 s | 15811 in · 543 res · 2 calls · 1.3 s | 14632 in · 543 res · 2 calls · 1.4 s |
| long | 160584 in · 3138 res · 15 calls · 6.4 s | 115384 in · 3158 res · 15 calls · 7.5 s | 109096 in · 3158 res · 15 calls · 7.2 s |

