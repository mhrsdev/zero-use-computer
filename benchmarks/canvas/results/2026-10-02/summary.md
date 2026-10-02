## Results by arm

| | baseline (screenshot every call) | current (zero-use-computer defaults) |
|---|---|---|
| runs | 10 | 10 |
| **task success** (verified from the saved file) | **10/10** | **10/10** |
| claimed DONE but failed | 0 | 0 |
| runs with a wrong box deleted | 0 | 0 |
| clicks on the target box (total) | 10 | 10 |
| clicks on another box (total) | 0 | 0 |
| clicks on empty canvas (total) | 1 | 0 |
| clicks outside the canvas (total) | 0 | 0 |
| model input tokens per run, EXACT | 38,976 (median 38,960, 38,948–39,137) | 41,189 (median 39,444, 39,426–56,897) |
| model output tokens per run, EXACT | 413 (median 409, 392–505) | 418 (median 403, 392–553) |
| model input tokens per *successful* run, EXACT | 38,976 (median 38,960, 38,948–39,137) | 41,189 (median 39,444, 39,426–56,897) |
| first request (system prompt + tool definitions + task), EXACT | 9,954 (median 9,954, 9,949–9,955) | 9,953 (median 9,952, 9,950–9,956) |
| model requests per run | 3.0 (median 3.0, 3.0–3.0) | 3.1 (median 3.0, 3.0–4.0) |
| tool-result tokens per run, ESTIMATED (chars/4 + w·h/750) | 4,050 (median 4,047, 4,047–4,079) | 4,458 (median 4,312, 4,312–5,776) |
|   of which images, ESTIMATED | 2,880 (median 2,880, 2,880–2,880) | 3,024 (median 2,880, 2,880–4,320) |
| images sent to the model per run | 2.0 (median 2.0, 2.0–2.0) | 2.1 (median 2.0, 2.0–3.0) |
| tool calls per run | 5.1 (median 5.0, 5.0–6.0) | 5.1 (median 5.0, 5.0–6.0) |
| task time per run, s (wall clock, agent start to exit) | 10.5 (median 9.9, 8.6–14.7) | 10.7 (median 10.3, 8.8–15.4) |
|   of which model API time, s | 6.4 (median 5.7, 4.8–11.2) | 6.5 (median 6.7, 5.0–8.8) |
| model cost per run, USD (list price, depends on prompt-cache hits) | 0.038 | 0.041 |

## When the savings features fired (number of tool results showing each)

| feature | baseline | current |
|---|---|---|
| ocr_auto | 0 results in 0 runs | 0 results in 0 runs |
| ocr_asked | 0 results in 0 runs | 0 results in 0 runs |
| ocr_unavailable | 0 results in 0 runs | 0 results in 0 runs |
| shot_deduped | 0 results in 0 runs | 0 results in 0 runs |
| shot_changed_part | 0 results in 0 runs | 0 results in 0 runs |
| shot_not_attached | 0 results in 0 runs | 0 results in 0 runs |
| shot_overview | 0 results in 0 runs | 0 results in 0 runs |
| screen_seen_before | 0 results in 0 runs | 0 results in 0 runs |
| change_report | 0 results in 0 runs | 30 results in 10 runs |

## Tool calls (totals over all runs)

| tool | baseline | current |
|---|---|---|
| click | 11 | 10 |
| get_app_state | 10 | 10 |
| press_key | 20 | 20 |
| screenshot | 10 | 11 |

## Every run

| run | success | model in (exact) | model out (exact) | requests | tool calls | images | est. tool-result tokens | wall s | clicks: target / other box / empty canvas / outside |
|---|---|---|---|---|---|---|---|---|---|
| baseline-1 | yes | 38,964 | 411 | 3 | 5 | 2 | 4,047 | 9.83 | 1 / 0 / 0 / 0 |
| baseline-2 | yes | 38,951 | 392 | 3 | 5 | 2 | 4,047 | 14.74 | 1 / 0 / 0 / 0 |
| baseline-3 | yes | 38,965 | 409 | 3 | 5 | 2 | 4,047 | 9.91 | 1 / 0 / 0 / 0 |
| baseline-4 | yes | 38,951 | 392 | 3 | 5 | 2 | 4,047 | 8.61 | 1 / 0 / 0 / 0 |
| baseline-5 | yes | 38,968 | 409 | 3 | 5 | 2 | 4,047 | 8.88 | 1 / 0 / 0 / 0 |
| baseline-6 | yes | 38,948 | 392 | 3 | 5 | 2 | 4,047 | 11.42 | 1 / 0 / 0 / 0 |
| baseline-7 | yes | 38,951 | 392 | 3 | 5 | 2 | 4,047 | 9.06 | 1 / 0 / 0 / 0 |
| baseline-8 | yes | 38,965 | 415 | 3 | 5 | 2 | 4,047 | 10.2 | 1 / 0 / 0 / 0 |
| baseline-9 | yes | 38,957 | 416 | 3 | 5 | 2 | 4,047 | 9.72 | 1 / 0 / 0 / 0 |
| baseline-10 | yes | 39,137 | 505 | 3 | 6 | 2 | 4,079 | 12.25 | 1 / 0 / 1 / 0 |
| current-1 | yes | 39,429 | 392 | 3 | 5 | 2 | 4,312 | 10.12 | 1 / 0 / 0 / 0 |
| current-2 | yes | 56,897 | 553 | 4 | 6 | 3 | 5,776 | 15.45 | 1 / 0 / 0 / 0 |
| current-3 | yes | 39,426 | 392 | 3 | 5 | 2 | 4,312 | 11.03 | 1 / 0 / 0 / 0 |
| current-4 | yes | 39,432 | 392 | 3 | 5 | 2 | 4,312 | 10.98 | 1 / 0 / 0 / 0 |
| current-5 | yes | 39,438 | 392 | 3 | 5 | 2 | 4,312 | 9.19 | 1 / 0 / 0 / 0 |
| current-6 | yes | 39,449 | 415 | 3 | 5 | 2 | 4,312 | 10.29 | 1 / 0 / 0 / 0 |
| current-7 | yes | 39,460 | 414 | 3 | 5 | 2 | 4,312 | 10.26 | 1 / 0 / 0 / 0 |
| current-8 | yes | 39,456 | 419 | 3 | 5 | 2 | 4,312 | 12.16 | 1 / 0 / 0 / 0 |
| current-9 | yes | 39,438 | 392 | 3 | 5 | 2 | 4,312 | 8.9 | 1 / 0 / 0 / 0 |
| current-10 | yes | 39,463 | 414 | 3 | 5 | 2 | 4,312 | 8.82 | 1 / 0 / 0 / 0 |

## Consistency checks

- initial screenshots identical across all runs: yes
- Dia's drawing area had 0 accessible children in every run: yes
- no box label in any accessible name/description/text (besides the unrelated 'Database' menu item): yes
- raw AT-SPI nodes for Dia: [433]
- per-request input sums equal the CLI's modelUsage totals: yes
