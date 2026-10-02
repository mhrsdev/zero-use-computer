# CLAIMS — every significant claim in the film, mapped to repository evidence

Paths: `cu/` = `crates/computer-use/src/`, `mcp/` = `crates/computer-use-mcp/src/`.
"Framing" rows are scenario or wording, not product claims. Everything the
system "says" on screen uses the engine's real output formats; the app
shown (a neutral mail client) and its content are a staged scenario.

| t (s) | On screen | Claim it makes | Evidence |
|---|---|---|---|
| 0.0–3.6 | "AI can see your screen." / "But seeing isn't using." | Framing. | — |
| 3.0 | A second pointer, purple with a "Zero" tag, beside the user's own arrow | The agent has its own cursor; the real mouse is not taken | `cu/overlay/draw.rs:107-259` (cursor, tag), `config_template.toml:120-148` (`cursor_color = "#9C27B0"`, `cursor_tag = "Zero"`); live test `overlay_is_left_out_of_screenshots_and_the_mouse_stays_put` |
| 3.6 | Blue glow along the screen edges; top-centre label "Zero is using the computer" | On-screen indicator, default `border_target = "screen"`, label text and colour | `cu/overlay/draw.rs:368-413` (edge), `cu/overlay/helper.rs:700-705` (label at top centre), `config_template.toml:129, 134, 142` |
| 4.35 | `user › Delete the three old digests.` | Framing: the user asked for the deletion (so the app's own Confirm is the only confirmation needed; the server has none — see VERIFIED_FEATURES) | — |
| 4.55, 9.6, 19.3 | Gold label "Zero is thinking…" | Thinking state and colour | `config_template.toml:135, 141`; `cu/overlay/helper.rs:541-564` |
| 5.0 | `get_app_state {"app":"Mail"}` | Real tool name and argument | `cu/tools.rs:1132-1158` |
| 5.35–5.6 | Capture; the card number is filled grey `#808080` | Private data is masked in screenshots before they are sent | `cu/imaging.rs:372-440`; test `private_data_never_reaches_the_model` |
| 5.6–6.6 | Element boxes with numbers | Visualisation of the numbered accessibility tree (set-of-marks screenshots also exist) | `cu/tree.rs:224, 434`; `cu/imaging.rs:876` |
| 6.9–10.5 | "SEE." "UNDERSTAND." and the tree `0 window "Inbox — Mail"` … `20 text "3 messages selected"` | Pruned, numbered tree in the engine's line format; card number masked to `•••• •••• •••• 1111` | `cu/tree.rs:154-219` (line format); `cu/privacy.rs` test `finds_card_numbers_only`; header shortened with "…" as the README does |
| 10.55 | `click {"app":"Mail","element_index":4}` | Click by element index | `cu/engine.rs:2021-2066` |
| 11.3–12.5 | Cursor glides and ripples where it clicks | Overlay glide and click ripple (film timing slowed from 220 / 450 ms for legibility) | `cu/overlay/helper.rs:496-524`; `draw.rs` ripple; test `cursor_glides_then_ripples` |
| 12.85–17.0 | agent → computer-use-mcp (MCP · JSON-RPC 2.0 · stdio) → engine (element 4 → its handle) → backend → Mail | The call path | `mcp/server.rs:56-247`; `Engine::call_tool` `cu/engine.rs:1555`; index → handle `cu/engine.rs:2021-2030` |
| 14.45 | macOS · AXPress / Windows · UIA Invoke / Linux · AT-SPI DoAction; `perform_action(h, native)` | The native press per OS through the Backend trait | `cu/backend.rs:158`; `cu/macos/mod.rs:486`; `cu/windows/mod.rs:282`; `cu/linux/atspi.rs:228-229` |
| 15.25 | settle — "re-read until two reads agree" | Adaptive settle | `cu/engine.rs:1389-1433`; test `waits_until_the_ui_stops_changing` |
| 15.8 | verify — "did the press take effect?" | Each action's result is checked | `cu/engine.rs:1431-1458, 4876`; test `a_press_that_changes_nothing_is_reported_not_repeated` |
| 16.3 | diff — "against what the model saw" | Diffs are against the model's view | `cu/engine.rs:1085-1114`; README "The model's view is the baseline" backed by `commit` in `cu/engine.rs:4829-4836` |
| 16.0 | "VERIFY." | Covered by the three rows above | — |
| 16.95 | `State after the action: now on screen #2 (new), window "Confirm":` | Change report after the action; new windows are followed | `cu/engine.rs:4807-4816`; test `dialog_is_followed_and_main_window_recognised_after` |
| 18.4 | `21 dialog "Confirm"` … `25 button "OK"` | New screen, new indices that don't collide with screen #1's | same test: `assert!(ok > delete, "dialog numbers don't collide")` |
| 20.6 | User's mouse moves → grey, "Paused while you use the computer" | Pause on user input | `cu/engine.rs:501-545`; `config_template.toml:138, 145, 211`; test `actions_wait_while_the_user_uses_the_computer` |
| 22.45–23.95 | Idle line fills for 1.5 s, then blue again | `resume_after_idle_ms = 1500` | `config_template.toml:212`; `cu/engine.rs:510-528` |
| 24.4 | Ctrl + Alt + Esc → orange, "Zero stopped. Press Ctrl+Alt+Esc to let it continue" | Emergency stop key, label and colour | `config_template.toml:139, 146, 207`; `cu/overlay/helper.rs:94-136` (`pretty_key`) |
| 24.5 | `the user stopped the agent with the emergency stop key (Ctrl+Alt+Esc). Stop here…` | Every call is refused while stopped (string truncated with "…") | `cu/error.rs:51-54`; `cu/engine.rs:1466-1475`; test `stop_refuses_every_call_until_the_user_lets_it_continue` |
| 25.75 | Pressed again → blue | Only the user's key lets it continue | same test |
| 21.0 | "CONTROL." | Covered by the four rows above | — |
| 26.6–27.1 | The app removes three rows; Inbox 12 → 9 | Framing: the app's response | — |
| 27.2–28.2 | Two cards: screen #1, screen #2; selection goes back to #1; old index tags return | Screen memory: seen screens are remembered and recognised; their indices come back. (Not a replay feature: the app returned to a screen the model had seen.) Card images are the screens as captured, without the overlay (excluded from captures) and with the card number masked | `cu/screens.rs:76-147`; `cu/engine.rs:990-1023`; tests `returning_to_a_seen_screen_skips_tree_and_screenshot`, `restores_indices_by_shape_when_keys_change` |
| 28.35 | `State after the action: back on screen #1 (seen before), window "Inbox — Mail":` + `Changes since you last saw screen #1 (+ added, ~ changed, - removed). Other elements are as they were then, with the same indices.` + `~ 7 …`, `- 14 …`, `- 15 …`, `- 16 …`, `~ 20 …` | Only what changed since the model last saw the screen, in the diff format | `cu/engine.rs:1108, 4815`; `cu/tree.rs:704-727`; test `returning_screen_reports_only_what_changed_since` |
| 27.95 | "REMEMBER." | Covered by the rows above | — |
| 30.25–31.4 | `get_app_state` again: a dashed node "another screenshot?" is routed around; `Screenshot: not attached (pass screenshot=true for one).` / `No changes to the accessibility tree since the previous get_app_state.` | An unchanged screen isn't sent again. The dashed node is a joke, not a claim about any other product | `cu/engine.rs:1930`; `cu/tree.rs:698`; test `unchanged_screenshots_are_not_resent`; real mock output in `docs/reference-renders/mock_session_output.txt` |
| 31.9–33.6 | Green "Zero is done", then everything fades out | Done state; slow fade-out | `config_template.toml:137, 144, 154`; test `fades_in_and_out_instead_of_popping` |
| 33.2–35.6 | Map: Claude Code, Codex, Cursor, VS Code, Claude Desktop → computer-use-mcp (stdio · HTTP · JSON-RPC 2.0) → engine; macOS · AX, Windows · UI Automation, Linux · AT-SPI2; the 25 tool names | Documented clients; transports (HTTP is the `http` build feature, included in release zips); backends; built-in tools | `docs/CONNECT.md`, `examples/`; `mcp/http.rs`; `cu/tools.rs:1132-1158` (test asserts 25; 24 exposed by default) |
| 33.95 | "Visible on your screen. / Readable in its source." | The indicator shows what the agent does; the source is open | overlay rows above; `LICENSE` |
| 36.3 | Mark: a ring with Zero's cursor at its centre | New mark, built from the overlay's own cursor geometry and click ring (the repo has no logo) | `cu/overlay/draw.rs:91, 112-259` |
| 36.75 | "Zero Use Computer" | The product name, from the repository name `zero-use-computer` (the crates are `computer-use` / `computer-use-mcp`; "Zero" is also the overlay's name tag) | `Cargo.toml`; `config_template.toml:148` |
| 38.0 | "OPEN SOURCE." | Licensed Apache-2.0 | `LICENSE`, `NOTICE`, `Cargo.toml` `license = "Apache-2.0"` |
| 39.1 | "Read it. Run it. Change it. Ship it." | Apache-2.0 §2 grants the rights to reproduce, prepare derivative works and distribute (commercially too), under §4's conditions (keep the license and the NOTICE, mark changes). The film makes no claim beyond that ("free for anything" is avoided) | `LICENSE` §2, §4; `NOTICE` |
| 40.2 | `Apache-2.0 · github.com/mhrsdev/zero-use-computer` | Exact repository | `Cargo.toml` `repository = "https://github.com/mhrsdev/zero-use-computer"` |

## Deliberately left out

Claimed by the brief's ideas but not implemented, so absent from the film:
a server-side confirmation step for sensitive actions (it is an agent-side
rule in `skills/computer-use-security/SKILL.md`), replay/undo/"step N of M",
a page map, dedicated browser automation, an auth gateway, any benchmark
number, and any comparison with Codex beyond the one dry joke node (the
README's "Codex-style" numbers compare two configurations of this same
engine and are not used).
