## The fixed prefix: sent with every request

| | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| framing | 58 | 58 | 58 | 58 |
| instructions | 273 | 474 | 474 | 527 |
| skills | 1899 | 1823 | 2261 | 2186 |
| tools | 1834 | 4363 | 4990 | 2328 |
| **total** | **4063** | **6718** | **7783** | **5099** |
| tools listed | 22 | 24 | 25 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, orders, long), summed

| | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| framing (x turns) | 2262 | 2262 | 2262 | 2262 |
| instructions (x turns) | 10647 | 18486 | 18486 | 20553 |
| skills (x turns) | 74061 | 71097 | 88179 | 85254 |
| tools (x turns) | 71526 | 170157 | 194610 | 90792 |
| **prefix, all turns** | 158457 | 262002 | 303537 | 198861 |
| results: text | 6821 | 6406 | 4236 | 4196 |
| results: pictures | 1981 | 2176 | 2176 | 2176 |
| arguments the model wrote | 395 | 395 | 395 | 395 |
| conversation read again (history) | 89771 | 87871 | 63469 | 63309 |
| **input, no cache** | 248228 | 349873 | 367006 | 262170 |
| tool calls | 35 | 35 | 35 | 35 |
| pictures | 4 | 5 | 5 | 5 |
| seconds (tools only) | 12.2 | 10.6 | 11.0 | 10.6 |

Against v3.7.0:

| | v0.1.0 | v3.0.0 | v3.6.0 |
|---|---|---|---|
| prefix per request | +25% | -24% | -34% |
| input, no cache | +6% | -25% | -29% |
| tool results | -28% | -26% | -1% |
| tool calls | +0% | +0% | +0% |
| seconds | -13% | -0% | -4% |

(The change from each release to v3.7.0: negative is less.)

## Each scenario

| scenario | v0.1.0 | v3.0.0 | v3.6.0 | v3.7.0 |
|---|---|---|---|---|
| form | 33303 in · 921 res · 6 calls · 0.9 s | 52143 in · 927 res · 6 calls · 1.6 s | 59305 in · 878 res · 6 calls · 1.6 s | 40477 in · 862 res · 6 calls · 1.6 s |
| table | 29584 in · 2471 res · 4 calls · 1.8 s | 43154 in · 2559 res · 4 calls · 1.5 s | 44719 in · 1587 res · 4 calls · 1.5 s | 31299 in · 1587 res · 4 calls · 1.5 s |
| board | fails (0/3) | 40274 in · 2204 res · 4 calls · 1.2 s | 36086 in · 2033 res · 3 calls · 1.5 s | 38714 in · 2344 res · 5 calls · 1.2 s |
| shapes | fails (0/3) | 31579 in · 1849 res · 3 calls · 1.2 s | 35758 in · 1822 res · 3 calls · 1.2 s | 31594 in · 1977 res · 4 calls · 1.2 s |
| orders | 26660 in · 549 res · 5 calls · 1.1 s | 43760 in · 824 res · 5 calls · 1.2 s | 50049 in · 799 res · 5 calls · 1.5 s | 33945 in · 799 res · 5 calls · 1.2 s |
| long | 158681 in · 4861 res · 20 calls · 8.4 s | 210816 in · 4272 res · 20 calls · 6.4 s | 212933 in · 3148 res · 20 calls · 6.4 s | 156449 in · 3124 res · 20 calls · 6.4 s |

