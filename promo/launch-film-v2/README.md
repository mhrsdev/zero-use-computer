# Zero Use Computer — open-source launch, film 2

A 31-second, 1920×1080, 60 fps cut of the launch film, made after film 1
(`../launch-film/`) read as too slow. Same story and same claims
(SEE → UNDERSTAND → ACT → VERIFY → REMEMBER → CONTROL → OPEN SOURCE),
cut to a real track: **"Vibe Ace" by Kevin MacLeod** (CC BY 4.0). The
picture runs on the track's own 130 BPM grid, so every cut, word and sound
lands on one of its beats. The sound effects are recordings too; nothing
in the soundtrack is synthesized. Built with
[HyperFrames](https://github.com/heygen-com/hyperframes) (HTML + GSAP,
rendered headless). Nothing in the product was changed; everything lives
in this folder.

**Final master:** `renders/zero-use-computer-film2-1080p60.mp4`
(H.264, 60 fps, AAC 48 kHz, 30.55 s, -16.0 LUFS integrated, -2.0 dBTP true peak).

Music: "Vibe Ace" by Kevin MacLeod (incompetech.com), licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/), edited. Sound
effects: Pixabay Content License. Details in `assets/audio/CREDITS.md`.

## What changed from film 1

| | Film 1 | Film 2 |
|---|---|---|
| Length | 44 s | 31 s |
| Pace | One continuous camera, scenes 3–6 s | A cut or a hit on every beat (0.46 s) |
| Type | Restrained labels | Full-frame kinetic words (Archivo, stretch 62–125 %) |
| Camera | Slow pushes | Macro pull-out from Zero's cursor tip, whip pans, iris through the click ripple |
| Call path | One diagram | One page per beat: server → engine → OS triptych → giant Delete press → settle/verify/diff |
| States | The label changes colour | Full-frame washes in the overlay's real state colours; the music tape-stops on PAUSED and STOPPED and drops back in on resume |
| Sound | Score synthesized from code | A real track (Kevin MacLeod, "Vibe Ace") edited on its beats, plus recorded sound effects |

## Beat sheet (beats at 130 BPM; seconds = beat × 0.4615)

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
| `audio/score.py`, `audio/master.sh` | The music edit and the sound effects, placed from `cues.json`; mastered to -16 LUFS. |
| `tools/mux.sh` | Puts the mastered WAV on the rendered picture (the renderer's own audio path is quieter). |
| `tools/review.sh` | Contact sheet, key frames, transition strips and an audio picture in `review/`. |
| `assets/` | Fonts (Archivo, JetBrains Mono — OFL), GSAP, grain, the two model-view thumbnails, the track (`audio/music/`), the sound effects (`audio/sfx/`), their credits (`audio/CREDITS.md`) and the mastered score. |

## Rebuild

Needs Node 22+, FFmpeg, Python 3 with numpy and scipy, and a Chrome headless shell.

```bash
cd promo/launch-film-v2
export HYPERFRAMES_BROWSER_PATH=/path/to/chrome-headless-shell   # or: npx hyperframes browser ensure

python3 tools/sync-cues.py            # cues.json -> src/cues.js
./audio/master.sh                     # cues.json + the track + sfx -> assets/audio/score.wav
npx hyperframes@0.8.105 check .       # lint, runtime, layout, motion, contrast
npx hyperframes@0.8.105 render -f 60 -q high -o renders/render-video.mp4
./tools/mux.sh renders/render-video.mp4 renders/zero-use-computer-film2-1080p60.mp4
./tools/review.sh
```

Retiming: change a beat in `cues.json`, run `sync-cues.py` and
`audio/master.sh`, and picture and sound move together. `bpm` must stay
130: it is the track's measured tempo (129.998 BPM, first beat at 28 ms,
8 ms jitter; measured with librosa), and the edit only cuts the track on
its own beats.

## The music edit

| Film beats | Track beats | What it does |
|---|---|---|
| 0–7.5 | 24–31.5 | The end of the track's build, behind a low-pass filter that opens up: seeing isn't using |
| 7.5–8 | — | Silence, one breath |
| 8–40.5 | 32–64.5 | The main groove drops in as Zero starts working |
| 40.5 | tape-stop | The user's mouse moves: PAUSED; silence through the 1.5 s idle wait |
| 44–45.5 | 64–65.5 | Resumes at the bar it stopped in |
| 45.5 | tape-stop | Ctrl+Alt+Esc: STOPPED; silence |
| 47–55.5 | 67–75.5 | Carries on as if it had kept running |
| 55.5–56 | — | Silence under a riser as the screen collapses |
| 56–end | 120–130 | The track's last two bars and its own ending |

The film's beats and the track's line up to within a few milliseconds
(strong onsets: median 2 ms from the picture's beats, 10 ms spread).
The sound effects sit about 7 LU under the music.

`hyperframes check` passes. What it still reports, all reviewed: one lint
warning that the film is a single composition (deliberate: the camera,
cursor and overlay persist across every section), and contrast notes on
the dimmed desktop in the intro and on the cursor's tag where it passes
over a timestamp.
