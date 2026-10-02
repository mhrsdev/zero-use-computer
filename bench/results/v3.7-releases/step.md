## The fixed prefix: sent with every request

| | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| framing | 58 | 58 | 58 | 58 |
| instructions | 273 | 474 | 474 | 527 |
| skills | 1899 | 1823 | 2261 | 2186 |
| tools | 1834 | 4363 | 4990 | 2167 |
| **total** | **4063** | **6718** | **7783** | **4938** |
| tools listed | 22 | 24 | 25 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, orders, long), summed

| | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| framing (x turns) | 2262 | 2262 | 2262 | 2204 |
| instructions (x turns) | 10647 | 18486 | 18486 | 20026 |
| skills (x turns) | 74061 | 71097 | 88179 | 83068 |
| tools (x turns) | 71526 | 170157 | 194610 | 82346 |
| **prefix, all turns** | 158457 | 262002 | 303537 | 187644 |
| results: text | 6821 | 6406 | 4236 | 4512 |
| results: pictures | 1981 | 2176 | 2176 | 1981 |
| arguments the model wrote | 395 | 395 | 395 | 388 |
| conversation read again (history) | 89771 | 87871 | 63469 | 63057 |
| **input, no cache** | 248228 | 349873 | 367006 | 250701 |
| tool calls | 35 | 35 | 35 | 34 |
| pictures | 4 | 5 | 5 | 4 |
| seconds (tools only) | 12.2 | 10.6 | 11.0 | 11.1 |

Against v3.7.0:

| | v0.1.0 | v3.0.0 | v3.6.0 |
|---|---|---|---|
| prefix per request | +22% | -26% | -37% |
| input, no cache | +1% | -28% | -32% |
| tool results | -26% | -24% | +1% |
| tool calls | -3% | -3% | -3% |
| seconds | -10% | +4% | +0% |

(The change from each release to v3.7.0: negative is less.)

## Each scenario

| scenario | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| form | 33303 in · 921 res · 6 calls · 0.9 s | 52143 in · 927 res · 6 calls · 1.6 s | 59305 in · 878 res · 6 calls · 1.6 s | 40312 in · 1192 res · 6 calls · 1.9 s |
| table | 29584 in · 2471 res · 4 calls · 1.8 s | 43154 in · 2559 res · 4 calls · 1.5 s | 44719 in · 1587 res · 4 calls · 1.5 s | 30494 in · 1587 res · 4 calls · 1.5 s |
| board | fails (0/3) | 40274 in · 2204 res · 4 calls · 1.2 s | 36086 in · 2033 res · 3 calls · 1.5 s | 24769 in · 1979 res · 3 calls · 1.9 s |
| shapes | fails (0/3) | 31579 in · 1849 res · 3 calls · 1.2 s | 35758 in · 1822 res · 3 calls · 1.2 s | 29331 in · 1225 res · 4 calls · 1.5 s |
| orders | 26660 in · 549 res · 5 calls · 1.1 s | 43760 in · 824 res · 5 calls · 1.2 s | 50049 in · 799 res · 5 calls · 1.5 s | 26827 in · 590 res · 4 calls · 1.3 s |
| long | 158681 in · 4861 res · 20 calls · 8.4 s | 210816 in · 4272 res · 20 calls · 6.4 s | 212933 in · 3148 res · 20 calls · 6.4 s | 153068 in · 3124 res · 20 calls · 6.4 s |

