# CLAIMS — film 2, every claim on screen mapped to repository evidence

Film 2 makes no claim that film 1 did not already make; it says the same
things faster. The audit behind every row is
`../launch-film/VERIFIED_FEATURES.md` (IMPLEMENTED rows only) and
`../launch-film/docs/feature-audit.md`.

Paths: `cu/` = `crates/computer-use/src/`, `mcp/` = `crates/computer-use-mcp/src/`.
Times are seconds in the final render (beat × 60 / 130, the music's tempo). "Framing" rows are
wording or scenario, not product claims; the mail app and its content are a
staged scenario.

| t (s) | On screen | Claim it makes | Evidence |
|---|---|---|---|
| 0.5–3.4 | "AI CAN SEE YOUR SCREEN." / "SEEING ISN'T USING." | Framing. | — |
| 3.1 | A purple cursor with a "Zero" tag cuts "USING." in half | The agent has its own cursor (shape, gradient and tag rebuilt from the drawing code) | `cu/overlay/draw.rs:107-259`; `config_template.toml` `cursor_color = "#9C27B0"`, `cursor_tag = "Zero"` |
| 3.69 | Pull-out from Zero's cursor tip: screen-edge glow, label "Zero is using the computer" | On-screen indicator and its label | `cu/overlay/draw.rs:368-413`; `cu/overlay/helper.rs:700-705`; `config_template.toml:134` |
| 4.6 | `Delete the three old digests.` typed big, then becomes the user chip | Framing: the user asked for it | — |
| 5.5, 10.6 | Gold label "Zero is thinking…", gold wash | Thinking state, label and colour `#D4A017` | `config_template.toml:135, 141`; `cu/overlay/helper.rs:541-564` |
| 6.0, 17.5 | `get_app_state {"app":"Mail"}` | Real tool name and argument | `cu/tools.rs:1132-1158` |
| 7.4–7.6 | Capture flash; the card number is filled grey | Private data is masked in screenshots before they are sent | `cu/imaging.rs:372-440`; test `private_data_never_reaches_the_model` |
| 7.9–9.0 | Numbered element boxes; "SEE." | Visualisation of the numbered accessibility tree | `cu/tree.rs:224, 434` |
| 9.3–10.6 | The tree's lines whip past and land on `4 button "Delete"`; "UNDERSTAND." | Pruned, numbered tree in the engine's line format (the card line in the data, blurred past, is masked to `•••• •••• •••• 1111`) | `cu/tree.rs:154-219`; `cu/privacy.rs` test `finds_card_numbers_only` |
| 11.08–11.5 | `click {"app":"Mail","element_index":4}`, cursor darts and ripples; "ACT." | Click by element index; overlay glide and ripple (timing shortened to the beat) | `cu/engine.rs:2021-2066`; `cu/overlay/helper.rs:496-524`; test `cursor_glides_then_ripples` |
| 12.0 | `computer-use-mcp` — MCP · JSON-RPC 2.0 · stdio | The server and its transport | `mcp/server.rs:56-247` |
| 12.5 | `engine` — element 4 → its handle | Index resolved to the element's handle | `cu/engine.rs:2021-2030` |
| 12.9 | macOS AXPress / Windows UIA Invoke / Linux AT-SPI DoAction | Native press per OS | `cu/backend.rs:158`; `cu/macos/mod.rs:486`; `cu/windows/mod.rs:282`; `cu/linux/atspi.rs:228-229` |
| 13.4 | Giant "Delete" pressed — "accessibility press, not a mouse click" | A click by element uses the accessibility action first (x/y input is only the fallback) | `cu/engine.rs:2021-2090`; test `click_uses_accessibility_then_falls_back_to_coords` |
| 13.9–14.6 | settle · verify · diff · `State after the action:`; "VERIFY." | Adaptive settle, verification of each action, diff against the model's view, change report | `cu/engine.rs:1389-1458, 1085-1114, 4782-4836`; tests `waits_until_the_ui_stops_changing`, `a_press_that_changes_nothing_is_reported_not_repeated`, `change_report_appended_after_action` |
| 14.8 | The app's Confirm dialog; `State after the action: now on screen #2 (new)` | New windows are followed and reported. The dialog is the mail app's own, not a confirmation step of the server (there is none) | `cu/engine.rs:4807-4816`; test `dialog_is_followed_and_main_window_recognised_after` |
| 15.5 | `click {"app":"Mail","element_index":25}` on OK | Dialog indices don't collide with screen #1's | same test |
| 15.7–16.6 | Three rows collapse and `- 14`, `- 15`, `- 16` fly out; cards screen #1 / screen #2 rewind to #1; old index tags return; `back on screen #1 (seen before)`; "REMEMBER." | Screen memory: a seen screen is recognised, its indices come back, only what changed is reported. Not replay or undo: the app returned to a screen the model had seen | `cu/screens.rs:76-147`; `cu/engine.rs:990-1114`; `cu/tree.rs:704-727`; tests `returning_to_a_seen_screen_skips_tree_and_screenshot`, `returning_screen_reports_only_what_changed_since` |
| 17.7–18.0 | The packet swerves around a dashed "another screenshot?"; `Screenshot: not attached (pass screenshot=true for one).` | An unchanged screen isn't sent again. The dashed box is the joke, not a claim about any other product | `cu/engine.rs:1930`; test `unchanged_screenshots_are_not_resent` |
| 18.6–20.3 | The user's own mouse moves; grey wash, "PAUSED.", "Paused while you use the computer", idle bar fills for exactly 1.5 s, the music tape-stops and stays silent until it resumes | Pause on user input; resumes after 1500 ms idle | `cu/engine.rs:501-545`; `config_template.toml:138, 145, 211-212`; test `actions_wait_while_the_user_uses_the_computer` |
| 20.8–21.7 | Ctrl + Alt + Esc → orange wash, "STOPPED.", "Zero stopped. Press Ctrl+Alt+Esc to let it continue"; pressed again → blue | Emergency stop key: every call refused until the user presses it again | `cu/engine.rs:1466-1475`; `cu/error.rs:51-54`; `config_template.toml:139, 146, 207`; test `stop_refuses_every_call_until_the_user_lets_it_continue` |
| 22.0 | `click {"app":"Mail","element_index":5}` on Search | Framing: the work carries on | — |
| 22.2 | The built-in tool names | 25 built-in tools (24 exposed by default) | `cu/tools.rs:1132-1158`; test `all_tools_have_object_schemas` |
| 23.0 | macOS AX API / Windows UI Automation / Linux AT-SPI2 | One accessibility backend per OS | `cu/macos/mod.rs:385`; `cu/windows/mod.rs:700`; `cu/linux/mod.rs:359` |
| 23.5 | Claude Code, Codex, Cursor, VS Code, Claude Desktop → `computer-use-mcp` | Documented clients | `docs/CONNECT.md`; `examples/` |
| 24.0–25.6 | The screen far away cycling through the state colours | The overlay's real state colours | `config_template.toml:141-147` |
| 25.85 | Mark: ring with Zero's cursor; "Zero Use Computer" | New mark built from the overlay's cursor and click ring (the repo has no logo); the name is the repository's | `cu/overlay/draw.rs:91, 112-259`; `Cargo.toml` |
| 26.8 | "OPEN SOURCE." | Licensed Apache-2.0 | `LICENSE`, `NOTICE`, `Cargo.toml` `license = "Apache-2.0"` |
| 27.3–28.0 | "Read it. Run it. Change it. Ship it." | Apache-2.0 §2 grants reproduction, derivative works and distribution (commercially too) under §4's conditions. Nothing beyond that is claimed | `LICENSE` §2, §4 |
| 28.4 | `Apache-2.0 · github.com/mhrsdev/zero-use-computer` | Exact repository | `Cargo.toml` `repository` |
| 28.6 | Music: "Vibe Ace" by Kevin MacLeod (incompetech.com) · CC BY 4.0 · edited | Attribution the music's license requires (not a product claim) | `assets/audio/CREDITS.md` |

## Deliberately left out

Same list as film 1: no server-side confirmation for sensitive actions, no
replay/undo, no page map, no browser automation, no auth gateway, no
benchmark number, and no comparison with Codex beyond the dashed
"another screenshot?" box.
