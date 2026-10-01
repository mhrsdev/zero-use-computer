# VERIFIED_FEATURES — Z-MCP / Zero Use Computer (repo `mhrsdev/zero-use-computer`, v2.6.0)

Audited at commit `37278f3`. A feature is **IMPLEMENTED** only where the code
that does it was read (and, where one exists, the test that exercises it).
README text was treated as a claim to check, not as evidence. Paths:
`cu/` = `crates/computer-use/src/`, `mcp/` = `crates/computer-use-mcp/src/`,
`engine.rs` = `cu/engine.rs`. `cargo test --workspace` passes here (203 + 19
+ 1 doctest; the 9 live Linux tests need a desktop and are no-ops in this
container).

Only rows marked IMPLEMENTED may appear in the film as existing behaviour.

## IMPLEMENTED

| Capability | Evidence (code · test) |
|---|---|
| MCP server over stdio (JSON-RPC 2.0): `initialize`, `ping`, `tools/list`, `tools/call`, `prompts/*`, `resources/*`, `notifications/tools/list_changed` (stdio) | `mcp/server.rs:56-247` · `initialize_list_and_call`, `settings_change_is_hot_reloaded_and_announced` |
| HTTP transport with required bearer token and local-Origin check (build feature `http`; release zips include it) | `mcp/http.rs:25-134` · `token_is_required_and_exact`, `only_local_pages_may_call` |
| Host → overlay status notification `computer_use/status` | `mcp/server.rs:145-179` |
| 25 built-in tools (24 exposed by default; `get_notifications` is off by default) + saved scripts as extra tools | `cu/tools.rs:1132-1158` · `all_tools_have_object_schemas` (asserts 25) |
| Accessibility tree per OS: macOS AX, Windows UI Automation, Linux AT-SPI2 | `cu/macos/mod.rs:385`, `cu/windows/mod.rs:700`, `cu/linux/mod.rs:359` · live `atspi_tree_actions_and_screenshot` (Linux) |
| Pruned, numbered tree; line format `<index> <role> "<name>" value="…" (flags) actions=[…]` | `cu/tree.rs:154-219, 224` · `prunes_wrappers_duplicates_and_hidden`, `stable_indices_and_diff` |
| Indices never reused; stale index refused ("unknown element_index …") | `cu/tree.rs:434`, `cu/error.rs:33-36` · `unknown_index_is_a_tool_error` |
| Window screenshot with the tree (`get_app_state`) | `engine.rs:1769-1797` · `screenshot_window_annotated` |
| Unchanged screenshots not re-sent (`Screenshot: not attached.` / `…unchanged…, not re-sent`) | `cu/screens.rs:310-410`, `engine.rs:1836-1851, 1930` · `unchanged_screenshots_are_not_resent` |
| Only the part of a screenshot that changed is sent | `engine.rs:1945-1966` · `a_follow_up_screenshot_sends_only_the_part_that_changed` |
| Diffs between reads (`+` added, `~` changed `(was: …)`, `-` removed) | `cu/tree.rs:654-727` · `removed_elements_listed` |
| Change report after every action (`State after the action: …`) | `engine.rs:4782-4836` · `change_report_appended_after_action` |
| Adaptive settle: re-read until two reads agree (max 2 s) | `engine.rs:1389-1433` · `waits_until_the_ui_stops_changing` |
| Screen memory: Jaccard ≥ 0.8 recognition, old indices restored, only changes since then sent | `cu/screens.rs:76-147`, `engine.rs:990-1114` · `returning_to_a_seen_screen_skips_tree_and_screenshot`, `returning_screen_reports_only_what_changed_since` |
| New windows followed (`now on screen #2 (new), window "Confirm"`), main window recognised after (`back on screen #1 (seen before)`), dialog indices don't collide | `engine.rs:745-822, 4807-4816` · `dialog_is_followed_and_main_window_recognised_after` |
| Click by element through the native accessibility action (works on background windows); x/y fallback | `engine.rs:2021-2090` · `click_uses_accessibility_then_falls_back_to_coords` |
| Native press per OS: macOS `AXPress`, Windows UIA Invoke, Linux AT-SPI `DoAction` | `cu/macos/mod.rs:486`, `cu/windows/mod.rs:282`, `cu/linux/atspi.rs:228-229` |
| Keyboard (`press_key`, `type_text`), `set_value`, `select_text`, secondary actions | `engine.rs:2093-2193, 3800-3889` · `press_key_sequences`, `set_value_and_type_and_select` |
| Input only reaches the target app (brought to front and checked, else not sent) | `engine.rs:1265-1316` · `no_input_when_the_app_cannot_come_to_the_front` |
| Verification of each action; "Nothing on screen changed after it; check (get_app_state, screenshot=true) before repeating it."; retry another way; typed text never retyped | `engine.rs:1431-1458, 2059-2160, 3849-3856, 4873-4876` · `a_press_that_changes_nothing_is_reported_not_repeated`, `typing_that_does_not_show_is_never_typed_twice` |
| OCR fallback (`ocr text` elements): Vision (macOS), Windows.Media.Ocr, Tesseract | `cu/ocr.rs`, `engine.rs:829-906` · `custom_drawn_apps_get_their_text_read_and_clickable` |
| On-screen indicator in a separate helper process: own cursor with "Zero" tag, screen-edge glow, top-centre label, click ripple, state colours, fades; click-through; excluded from captures | `cu/overlay/draw.rs:112-413`, `cu/overlay/helper.rs:541-812` · `cursor_glides_then_ripples`, `colours_blend_between_states`, live `overlay_is_left_out_of_screenshots_and_the_mouse_stays_put` |
| The real mouse is never moved (macOS posts to pid; Linux/Windows restore the pointer) | `cu/macos/cg.rs:26-29`, `cu/linux/x11.rs:164-178`, `cu/windows/input.rs:66-69` · live test above |
| Pause while the user uses mouse/keyboard (idle time only; resumes after 1500 ms idle; gives up after 120 s) | `engine.rs:501-545` · `actions_wait_while_the_user_uses_the_computer`, `own_input_is_not_taken_for_the_user` |
| Emergency stop key Ctrl+Alt+Esc (Ctrl+Option+Esc on a Mac): every call refused until pressed again | `engine.rs:1466-1475`, `cu/error.rs:51-54` · `stop_refuses_every_call_until_the_user_lets_it_continue` |
| Privacy: password fields removed (not even length), Luhn card numbers masked to last four (`Card •••• •••• •••• 1111`), sensitive labels, screenshot areas filled grey `#808080` | `cu/privacy.rs:12-202`, `cu/imaging.rs:372-440` · `finds_card_numbers_only`, `private_data_never_reaches_the_model` |
| Window management tool (`window`), `follow_new_windows` | `engine.rs:4469`, `cu/tools.rs:1774-1795` · `windows_can_be_arranged` |
| Sandboxed Rhai `script` tool; saved scripts become tools | `cu/script/`, `cu/engine/scripting.rs:28-51` · `saved_scripts_become_tools` |
| Design tools: `draw`, `trace_image`, `design`, `scene`, `locate` | `cu/draw.rs`, `cu/paint.rs`, `cu/design.rs`, `cu/scene.rs`, `cu/target.rs` · see audit |
| Audit log (JSONL metadata per call, off by default) | `engine.rs:1607-1642` |
| Clients documented: Claude Code, Claude Desktop, Codex, Cursor, VS Code, HTTP | `docs/CONNECT.md`, `examples/*.json`, `examples/codex.config.toml` |
| License Apache-2.0 with NOTICE ("Copyright 2026 mhrsdev") | `LICENSE`, `NOTICE`, `Cargo.toml` |

## PARTIAL

| Capability | What is true |
|---|---|
| macOS and Windows backends | Implemented and type-checked/tested with the mock backend on CI runners; no live desktop tests and no measurements on real hardware (README says so). |
| Linux | X11 only for input and capture; Wayland sessions reach only XWayland apps. |
| `scroll`, `drag` | Code exists; no unit or live test. |
| HTTP transport | Only in builds with `--features http` (release zips have it). |
| Performance table in the README | "Before" timings come from an earlier version; the Codex comparison (`examples/compare.rs`) compares a *Codex-style configuration of this same engine*, not OpenAI Codex. Not used in the film. |

## NOT IMPLEMENTED (kept out of the film)

| Idea from the brief | Reality |
|---|---|
| A confirmation/approval step for sensitive actions | **None in code** (`mcp/server.rs:3-5`). "Confirm consequential actions" is rule 3 of `skills/computer-use-security/SKILL.md`, an instruction the agent follows. The film shows no system confirmation dialog; the "Confirm" dialog on screen is the *mail app's own* dialog. |
| Replay / undo / rewind / "step N of M" | None. The film's "return" beat is screen memory (`back on screen #1 (seen before)`), never labelled replay. |
| Page map / regions | None. What exists: the numbered tree, set-of-marks screenshots (red `#FF2828` boxes), grids, cells, `locate`. The film's hairline element boxes are a visualisation of the numbered tree, not a product feature. |
| Dedicated browser automation (CDP/WebDriver) | None; browsers are operated as ordinary apps. Not mentioned in the film. |
| Authentication gateway | None beyond the HTTP bearer token + Origin check. Not mentioned in the film. |
| A logo, "Z-MCP" or "Use Computer" in the repo | None. "Z-MCP / Zero Use Computer" is the launch name given in the brief; the film's mark is new and built from the real overlay (cursor + ring). |

## Verbatim strings used on screen

- Labels: `Zero is using the computer`, `Zero is thinking…`, `Paused while you use the computer`, `Zero stopped. Press Ctrl+Alt+Esc to let it continue`, `Zero is done` (`config_template.toml:134-139`).
- Colours: working `#1E88E5`, thinking `#D4A017`, paused `#78909C`, stopped `#FF6D00`, done `#2E7D32`, cursor `#9C27B0` (`config_template.toml:141-148`).
- `State after the action: now on screen #2 (new), window "Confirm":` / `State after the action: back on screen #1 (seen before), window "…":` (`engine.rs:4807-4816`).
- `Changes since you last saw screen #1 (+ added, ~ changed, - removed). Other elements are as they were then, with the same indices.` (`engine.rs:1108`).
- `Screenshot: not attached.` (`engine.rs:1931`), `No changes to the accessibility tree since the previous get_app_state.` (`cu/tree.rs:698`).
- `the user stopped the agent with the emergency stop key (Ctrl+Alt+Esc). Stop here: don't retry, and ask the user how to proceed. …` (`cu/error.rs:51-54`).
- Card mask `Card •••• •••• •••• 1111` (`cu/privacy.rs:249-259`).

The full audit with line-level evidence for every category is
`docs/feature-audit.md` in this folder.
