---
format: 1920x1080
fps: 60
duration: 44s
message: "An agent shouldn't just receive screenshots. Zero Use Computer sees, understands, acts, verifies and stays under your control — and it's open source."
arc: Hook → See → Understand → Act (dive) → Verify → Control → Remember (screen memory) → Pull back → Open source
audience: developers who build or use coding agents
mode: autonomous
---

# Zero Use Computer — launch film beat sheet

One continuous canvas. The frame **is** the user's screen until the final
pull-back. Every transition is caused by something the system really does.
Times are seconds; the authoritative values live in `cues.json`, which
drives both the picture (`src/cues.js`) and the sound (`audio/synth.py`).

The scenario is the round trip the engine's own test
`dialog_is_followed_and_main_window_recognised_after` exercises: press
Delete → a Confirm dialog opens (new screen) → press OK → back on the
main screen, recognised. The app is a neutral mail client; the task is
"delete the three old digests".

## Frame 1 — Hook: seeing isn't using

- duration: 4.2s (0.0–4.2)
- scene: Black. The user's ordinary arrow. A desktop fades in, dim. "AI can see your screen." A capture freezes the frame. "But seeing isn't using." Zero's own cursor glides in; HIT — the screen-edge glow ignites and the label reads "Zero is using the computer".
- transition_in: cut from black
- status: animated

Cause → effect: the capture (shutter) freezes the frame into a mere
picture; Zero's cursor arriving is what turns the picture back into a
live, operable screen (the glow, `border_target = "screen"` default).

## Frame 2 — SEE / UNDERSTAND

- duration: 6.6s (4.2–10.8)
- scene: Label goes gold ("Zero is thinking…"), then blue. `get_app_state {"app":"Mail"}` travels to the window. Capture: the card number in the Billing row is blacked out before anything leaves. Hairline boxes with index numbers grow out of the app in tree order. The camera slides the window left; the numbered tree types out on the right, each line tied to its box by a leader. "SEE." then "UNDERSTAND." Line `4 button "Delete"` lights; gold again while the model decides.
- status: animated

## Frame 3 — ACT: the click becomes the pipeline

- duration: 8.9s (10.8–19.7)
- scene: `click {"app":"Mail","element_index":4}`. Zero's cursor glides to Delete and clicks; the ripple keeps expanding and the camera dives into it. The ring becomes the drafting circle of the call path: agent → computer-use-mcp (JSON-RPC) → engine → backend (AX · UI Automation · AT-SPI2) → native press → the app; back: settle (re-read until two reads agree) → verify → diff against what the model saw → "State after the action". The diagram folds back into the ring, the ring into the button; the app now shows the Confirm dialog and the change report "now on screen #2 (new), window "Confirm"" with fresh, non-colliding indices.
- status: animated

## Frame 4 — CONTROL

- duration: 7.9s (19.7–27.6)
- scene: `click … element_index: 25` (OK). Zero's cursor sets off — the user's own mouse moves. Everything stops: grey glow, "Paused while you use the computer". The user stops; an idle hairline fills for 1.5 s (`resume_after_idle_ms = 1500`); blue again. Then Ctrl + Alt + Esc: orange, "Zero stopped. Press Ctrl+Alt+Esc to let it continue", and the tool call is refused with the real error text. Pressed again: blue. Zero clicks OK. "CONTROL."
- status: animated

## Frame 5 — REMEMBER: what changed

- duration: 5.8s (27.6–33.4)
- scene: The dialog closes and the app removes the three rows. Two cards show the screens the model has seen (captured as the model saw them: no overlay, card number masked). The selection travels back from screen #2 to screen #1 while a recognition sweep crosses the live screen, and the indices the model saw on screen #1 drop back into place. "back on screen #1 (seen before)": the change report lists only the delta (`~` the Inbox count, `-` the three digests, `~` the reading pane). The next `get_app_state` pulls the camera in: a dashed node labelled "another screenshot?" sits on the path and the packet routes around it into "Screenshot: not attached (pass screenshot=true for one)." (the dry Codex nod). Back out; the label turns green, "Zero is done", and the overlay fades out slowly. "REMEMBER."
- status: animated

## Frame 6 — Pull back: the whole system

- duration: 2.4s (33.4–35.8)
- scene: The camera pulls far back. The screen becomes one rectangle on the drafting surface, surrounded by everything that ran: MCP clients (Claude Code, Codex, Cursor, VS Code, Claude Desktop) into `computer-use-mcp`, the engine, the three OS backends, the numbered tree, the two remembered screens, the tool names. Bookend line: "Visible on your screen. / Readable in its source."
- status: animated

## Frame 7 — Open source

- duration: 8.2s (35.8–44.0)
- scene: Everything collapses into the centre: the screen glow becomes a ring, Zero's cursor docks inside it — the strongest hit of the film. "Zero Use Computer". "OPEN SOURCE." "Read it. Run it. Change it. Ship it." "Apache-2.0 · github.com/mhrsdev/zero-use-computer". Hold.
- status: animated

## Sound map

| t (s) | event | sound |
|---|---|---|
| 0.35 | user arrow appears | near silence, room tone |
| 1.55 | capture freezes frame | two-stage shutter |
| 3.05–3.55 | Zero cursor glides in | air glide |
| 3.60 | glow ignites | first structural hit (medium) + bed begins |
| 5.20 | get_app_state capture | shutter |
| 5.35 | card number masked | short low "thunk" |
| 5.6–7.0 | elements detected | ascending blips, one per box |
| 7.0–8.6 | tree lines | data ticks |
| 12.0 | click Delete | click + ripple swell |
| 12.1–12.9 | dive into ripple | whoosh |
| 13.0–16.8 | packets along the call path | tick trains, node pings |
| 17.0–17.8 | fold back | reverse whoosh |
| 18.0 | dialog appears | soft tonal swell |
| 20.8 | user input → pause | bed drops out, tape-stop |
| 24.1 | resume | bed returns |
| 24.5 | stop key | key thocks + halting tone |
| 25.9 | release | key thock, bed returns |
| 26.6 | click OK | click |
| 27.0–27.6 | screen memory rewind | reverse granular sweep |
| 28.4–29.0 | rows removed | three soft ticks |
| 29.85 | camera pulls in on the next look | soft air |
| 30.75 | routes around "another screenshot?" | tiny swerve blip |
| 31.8 | done (green) | gentle resolve chord |
| 33.4–35.6 | pull back | rising air |
| 35.8 | collapse | inward suck |
| 36.3 | mark + Zero Use Computer | strongest hit (sub + chord bloom) |
| 38.0 | OPEN SOURCE | second, lighter hit |
| 40.4 | URL | tick |
| 40.4–44.0 | hold | tail decays to silence |
