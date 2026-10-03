# Every minor version, as it ships: one action a call (`--plan step`)

The newest patch of each minor version, downloaded from its GitHub
release (Linux x64), run over MCP with its own skills and instructions
(`--server`, `--skills`; v0.1.5 and v2.0.0 with `--server-args "--approval
allow-all --headless-approve allow"` and `[guard] mode = "allow"`).
**Every task in a desktop of its own** (`--runs 1 --scenarios <one>`),
three rounds, the server's helper processes ended between runs; medians.
Scripted estimates, no prompt cache: see [the limits](../../../README.md#version-history-and-token-use).

## The fixed prefix: sent with every request

| | v0.1.5 | v2.0.0 | v2.5.7 | v2.6.0 | v3.0.0 | v3.1.0 | v3.2.0 | v3.6.0 | v3.7.5 | v3.8.3 | v3.9.0 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| framing | 58 | 58 | 58 | 58 | 58 | 58 | 58 | 58 | 58 | 58 | 58 |
| instructions | 299 | 513 | 432 | 474 | 474 | 474 | 474 | 474 | 527 | 527 | 527 |
| skills | 1951 | 1417 | 1636 | 1823 | 1823 | 1823 | 2276 | 2261 | 2186 | 2186 | 2222 |
| tools | 1929 | 1950 | 4511 | 4809 | 4363 | 4363 | 4607 | 4990 | 2167 | 2167 | 2165 |
| **total** | **4236** | **3938** | **6637** | **7164** | **6718** | **6718** | **7415** | **7783** | **4938** | **4938** | **4972** |
| tools listed | 23 | 23 | 23 | 24 | 24 | 24 | 25 | 25 | 17 | 17 | 17 |

## A task's tokens by part: every scenario all of them finish (form, table, orders, long), summed

| | v0.1.5 | v2.0.0 | v2.5.7 | v2.6.0 | v3.0.0 | v3.1.0 | v3.2.0 | v3.6.0 | v3.7.5 | v3.8.3 | v3.9.0 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| framing (x turns) | 2262 | 2262 | 2262 | 2262 | 2262 | 2262 | 2262 | 2262 | 2204 | 2204 | 2204 |
| instructions (x turns) | 11661 | 20007 | 16848 | 18486 | 18486 | 18486 | 18486 | 18486 | 20026 | 20026 | 20026 |
| skills (x turns) | 76089 | 55263 | 63804 | 71097 | 71097 | 71097 | 88764 | 88179 | 83068 | 83068 | 84436 |
| tools (x turns) | 75231 | 76050 | 175929 | 187551 | 170157 | 170157 | 179673 | 194610 | 82346 | 82346 | 82270 |
| **prefix, all turns** | 165204 | 153582 | 258843 | 279396 | 262002 | 262002 | 289185 | 303537 | 187644 | 187644 | 188936 |
| results: text | 6821 | 6979 | 6963 | 6963 | 6406 | 6406 | 6385 | 4236 | 4208 | 4208 | 3901 |
| results: pictures | 1981 | 1981 | 2176 | 2176 | 2176 | 2176 | 2176 | 2176 | 1981 | 1981 | 1981 |
| arguments the model wrote | 395 | 395 | 395 | 395 | 395 | 395 | 395 | 395 | 388 | 388 | 388 |
| conversation read again (history) | 89771 | 91553 | 92269 | 92269 | 87866 | 87866 | 87656 | 63469 | 62251 | 62251 | 59720 |
| **input, no cache** | 254975 | 245135 | 351112 | 371665 | 349868 | 349868 | 376841 | 367006 | 249895 | 249895 | 248656 |
| tool calls | 35 | 35 | 35 | 35 | 35 | 35 | 35 | 35 | 34 | 34 | 34 |
| pictures | 4 | 4 | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 4 | 4 |
| seconds (tools only) | 15.2 | 17.9 | 17.9 | 18.0 | 11.2 | 11.2 | 11.2 | 11.9 | 11.8 | 14.5 | 14.0 |

Against v3.9.0:

| | v0.1.5 | v2.0.0 | v2.5.7 | v2.6.0 | v3.0.0 | v3.1.0 | v3.2.0 | v3.6.0 | v3.7.5 | v3.8.3 |
|---|---|---|---|---|---|---|---|---|---|---|
| prefix per request | +17% | +26% | -25% | -31% | -26% | -26% | -33% | -36% | +1% | +1% |
| input, no cache | -2% | +1% | -29% | -33% | -29% | -29% | -34% | -32% | -0% | -0% |
| tool results | -33% | -34% | -36% | -36% | -31% | -31% | -31% | -8% | -5% | -5% |
| tool calls | -3% | -3% | -3% | -3% | -3% | -3% | -3% | -3% | +0% | +0% |
| seconds | -8% | -22% | -22% | -23% | +25% | +25% | +25% | +17% | +18% | -3% |

(The change from each release to v3.9.0: negative is less.)

## Each scenario

| scenario | v0.1.5 | v2.0.0 | v2.5.7 | v2.6.0 | v3.0.0 | v3.1.0 | v3.2.0 | v3.6.0 | v3.7.5 | v3.8.3 | v3.9.0 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| form | 34514 in · 921 res · 6 calls · 1.1 s | 32680 in · 936 res · 6 calls · 1.7 s | 51573 in · 936 res · 6 calls · 1.7 s | 55262 in · 936 res · 6 calls · 1.7 s | 52143 in · 927 res · 6 calls · 1.8 s | 52143 in · 927 res · 6 calls · 1.7 s | 57022 in · 927 res · 6 calls · 1.7 s | 59305 in · 878 res · 6 calls · 1.8 s | 39506 in · 888 res · 6 calls · 2.0 s | 39506 in · 888 res · 6 calls · 2.5 s | 39174 in · 793 res · 6 calls · 2.2 s |
| table | 30449 in · 2471 res · 4 calls · 2.0 s | 29235 in · 2540 res · 4 calls · 1.9 s | 42730 in · 2540 res · 4 calls · 2.0 s | 45365 in · 2540 res · 4 calls · 1.9 s | 43154 in · 2559 res · 4 calls · 1.1 s | 43154 in · 2559 res · 4 calls · 1.1 s | 46627 in · 2554 res · 4 calls · 1.1 s | 44719 in · 1587 res · 4 calls · 1.1 s | 30494 in · 1587 res · 4 calls · 1.1 s | 30494 in · 1587 res · 4 calls · 1.6 s | 30388 in · 1518 res · 4 calls · 1.5 s |
| board | fails (0/3) | fails (0/3) | 40406 in · 2375 res · 4 calls · 1.4 s | 44372 in · 2819 res · 4 calls · 1.4 s | 42203 in · 2839 res · 4 calls · 1.5 s | 42203 in · 2839 res · 4 calls · 1.4 s | 44349 in · 2393 res · 4 calls · 1.4 s | 36084 in · 2032 res · 3 calls · 1.7 s | 16995 in · 1099 res · 2 calls · 2.3 s | 16995 in · 1099 res · 2 calls · 2.0 s | 16959 in · 1030 res · 2 calls · 2.1 s |
| shapes | fails (0/3) | fails (0/3) | 31255 in · 1849 res · 3 calls · 1.1 s | 33363 in · 1849 res · 3 calls · 1.1 s | 31579 in · 1849 res · 3 calls · 1.2 s | 31579 in · 1849 res · 3 calls · 1.1 s | 34367 in · 1849 res · 3 calls · 1.1 s | 35758 in · 1822 res · 3 calls · 1.1 s | 29331 in · 1225 res · 4 calls · 1.7 s | 29331 in · 1225 res · 4 calls · 1.5 s | 29225 in · 1156 res · 4 calls · 1.6 s |
| orders | 27698 in · 549 res · 5 calls · 1.3 s | 26288 in · 626 res · 5 calls · 1.3 s | 43198 in · 805 res · 5 calls · 1.3 s | 46360 in · 805 res · 5 calls · 1.3 s | 43755 in · 823 res · 5 calls · 1.2 s | 43755 in · 823 res · 5 calls · 1.3 s | 47937 in · 823 res · 5 calls · 1.3 s | 50049 in · 799 res · 5 calls · 1.7 s | 26827 in · 590 res · 4 calls · 1.5 s | 26827 in · 590 res · 4 calls · 1.9 s | 26692 in · 516 res · 4 calls · 1.7 s |
| long | 162314 in · 4861 res · 20 calls · 10.8 s | 156932 in · 4858 res · 20 calls · 13.0 s | 213611 in · 4858 res · 20 calls · 12.9 s | 224678 in · 4858 res · 20 calls · 13.2 s | 210816 in · 4272 res · 20 calls · 7.1 s | 210816 in · 4272 res · 20 calls · 7.1 s | 225255 in · 4256 res · 20 calls · 7.2 s | 212933 in · 3148 res · 20 calls · 7.4 s | 153068 in · 3124 res · 20 calls · 7.2 s | 153068 in · 3124 res · 20 calls · 8.4 s | 152402 in · 3055 res · 20 calls · 8.5 s |

