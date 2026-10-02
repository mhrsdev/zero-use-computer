## The fixed prefix: sent with every request

| | v3.6.0-batch | v3.7.0-batch | v3.7.0-plugin-batch |
|---|---|---|---|
| framing | 58 | 58 | 58 |
| instructions | 474 | 527 | 133 |
| skills | 2261 | 2186 | 2186 |
| tools | 4990 | 2328 | 2328 |
| **total** | **7783** | **5099** | **4706** |
| tools listed | 25 | 17 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, board, shapes, orders, long), summed

| | v3.6.0-batch | v3.7.0-batch | v3.7.0-plugin-batch |
|---|---|---|---|
| framing (x turns) | 2030 | 2088 | 2204 |
| instructions (x turns) | 16590 | 18972 | 5054 |
| skills (x turns) | 79135 | 78696 | 83068 |
| tools (x turns) | 174650 | 83808 | 88464 |
| **prefix, all turns** | 272405 | 183564 | 178828 |
| results: text | 4876 | 5043 | 5328 |
| results: pictures | 5139 | 5264 | 5318 |
| arguments the model wrote | 354 | 366 | 387 |
| conversation read again (history) | 52934 | 54920 | 58402 |
| **input, no cache** | 325339 | 238484 | 237230 |
| tool calls | 29 | 30 | 32 |
| pictures | 9 | 9 | 10 |
| seconds (tools only) | 13.7 | 13.2 | 13 |

Against v3.7.0-batch:

| | v3.6.0-batch | v3.7.0-plugin-batch |
|---|---|---|
| prefix per request | -34% | +8% |
| input, no cache | -27% | +1% |
| tool results | +3% | -4% |
| tool calls | +3% | -6% |
| seconds | -3% | +2% |

(The change from each release to v3.7.0-batch: negative is less.)

## Each scenario

| scenario | v3.6.0-batch | v3.7.0-batch | v3.7.0-plugin-batch |
|---|---|---|---|
| form | 24830 in · 816 res · 2 calls · 1.6 s | 16814 in · 852 res · 2 calls · 1.6 s | 15635 in · 852 res · 2 calls · 1.6 s |
| table | 35330 in · 1537 res · 3 calls · 1.5 s | 24594 in · 1537 res · 3 calls · 1.5 s | 23022 in · 1537 res · 3 calls · 1.5 s |
| board | 36084 in · 2032 res · 3 calls · 1.4 s | 25350 in · 2033 res · 3 calls · 1.2 s | 36672 in · 2423 res · 5 calls · 1.2 s |
| shapes | 35758 in · 1822 res · 3 calls · 1.2 s | 31594 in · 1977 res · 4 calls · 1.2 s | 29629 in · 1977 res · 4 calls · 1.2 s |
| orders | 32753 in · 670 res · 3 calls · 1.4 s | 22172 in · 750 res · 3 calls · 1.2 s | 20600 in · 750 res · 3 calls · 1.1 s |
| long | 160584 in · 3138 res · 15 calls · 6.4 s | 117960 in · 3158 res · 15 calls · 6.4 s | 111672 in · 3158 res · 15 calls · 6.3 s |

