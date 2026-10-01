# Zero Use Computer — open-source launch, film 2

A 31-second, 1920×1080, 60 fps cut of the launch film, made after film 1
(`../launch-film/`) read as too slow. Same story and same claims
(SEE → UNDERSTAND → ACT → VERIFY → REMEMBER → CONTROL → OPEN SOURCE),
retold on a 128 BPM grid: every cut, word and sound lands on a beat or
an eighth. Built with [HyperFrames](https://github.com/heygen-com/hyperframes)
(HTML + GSAP, rendered headless); the music and sound are synthesized
from code. Nothing in the product was changed; everything lives in this
folder.

**Final master:** `renders/zero-use-computer-film2-1080p60.mp4`
(H.264, 60 fps, AAC 48 kHz, -14.3 LUFS integrated, -1.4 dBTP true peak).

## What changed from film 1

| | Film 1 | Film 2 |
|---|---|---|
| Length | 44 s | 31 s |
| Pace | One continuous camera, scenes 3–6 s | A cut or a hit on every beat (0.47 s) |
| Type | Restrained labels | Full-frame kinetic words (Archivo, stretch 62–125 %) |
| Camera | Slow pushes | Macro pull-out from Zero's cursor tip, whip pans, iris through the click ripple |
| Call path | One diagram | One page per beat: server → engine → OS triptych → giant Delete press → settle/verify/diff |
| States | The label changes colour | Full-frame washes in the overlay's real state colours; the music stops dead on PAUSED and STOPPED and drops back in on resume |
| Sound | Ambient score | Four-on-the-floor groove, side-chained bass, arps, tape-stops |

## Beat sheet (beats at 128 BPM; seconds = beat × 0.469)

| Beats | Section | What happens | What causes the next cut |
|---|---|---|---|
| 0–8 | Intro | AI CAN / SEE / YOUR SCREEN. — a screenshot freezes the desktop — SEEING / ISN'T / USING. | Zero's cursor slices "USING." in two; half a beat of silence |
| 8–16 | Drop | Macro pull-out from the cursor tip to the whole screen, glow and label on; the request types itself big, then shrinks into a chip; thinking (gold); `get_app_state` | The call fires the capture |
| 16–24 | See, understand | Capture flash, card number filled grey, element boxes on 32nds, SEE.; whip to the tree ticker landing on `4 button "Delete"`; UNDERSTAND. | `click … element_index 4` |
| 24–32 | Act, verify | Dart, click, ACT.; the ripple irises into the call path, one stop per beat; VERIFY. | `State after the action:` snaps back to the screen |
| 32–40 | Remember | The app's Confirm dialog = screen #2 (new); OK; rows collapse with `- 14/15/16`; cards rewind to screen #1 (seen before); old tags return; REMEMBER.; the packet swerves around "another screenshot?" | `Screenshot: not attached` |
| 40–48 | Control | The user's mouse moves: grey, PAUSED., music cut, idle bar 1.5 s, resume; Ctrl+Alt+Esc: orange, STOPPED., music cut; pressed again: go | The next click |
| 48–56 | Build | Tool names, three OS backends, the clients wiring into `computer-use-mcp`; the screen far away cycling state colours | Everything collapses into a ring |
| 56–66 | Reveal | Hit: mark + "Zero Use Computer"; OPEN SOURCE.; Read it. Run it. Change it. Ship it.; `Apache-2.0 · github.com/mhrsdev/zero-use-computer` | Fade |

## What is in here

| File | What it is |
|---|---|
| `CLAIMS.md` | Every claim on screen, timestamped, mapped to code and tests. The feature audit itself is `../launch-film/VERIFIED_FEATURES.md`. |
| `index.html`, `film.css` | The composition: film 1's screen and overlay, plus the kinetic layers (words, washes, ticker, path, cards, montage, lockup). |
| `src/timeline.js` | One seekable GSAP timeline built from measured layout, timed in beats. |
| `src/scene-data.js` | Every string the system "says", in the engine's formats (shared with film 1). |
| `cues.json` → `src/cues.js` | The single beat grid for picture **and** sound (`python3 tools/sync-cues.py`). |
| `audio/synth.py`, `audio/master.sh` | Music and sound effects generated from `cues.json`, mastered to about -14 LUFS. |
| `tools/mux.sh` | Puts the mastered WAV on the rendered picture (the renderer's own audio path is quieter). |
| `tools/review.sh` | Contact sheet, key frames, transition strips and an audio picture in `review/`. |
| `assets/` | Fonts (Archivo, JetBrains Mono — OFL), GSAP, grain, the two model-view thumbnails, the mastered score. |

## Rebuild

Needs Node 22+, FFmpeg, Python 3 with numpy and scipy, and a Chrome headless shell.

```bash
cd promo/launch-film-v2
export HYPERFRAMES_BROWSER_PATH=/path/to/chrome-headless-shell   # or: npx hyperframes browser ensure

python3 tools/sync-cues.py            # cues.json -> src/cues.js
./audio/master.sh                     # cues.json -> assets/audio/score.wav
npx hyperframes@0.8.105 check .       # lint, runtime, layout, motion, contrast
npx hyperframes@0.8.105 render -f 60 -q high -o renders/render-video.mp4
./tools/mux.sh renders/render-video.mp4 renders/zero-use-computer-film2-1080p60.mp4
./tools/review.sh
```

Retiming: change a beat in `cues.json`, run `sync-cues.py` and
`audio/master.sh`, and picture and sound move together.

`hyperframes check` passes. What it still reports, all reviewed: one lint
warning that the film is a single composition (deliberate: the camera,
cursor and overlay persist across every section), and contrast notes on
the dimmed desktop in the intro and on the cursor's tag where it passes
over a timestamp.
