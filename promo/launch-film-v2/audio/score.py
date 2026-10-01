"""Film 2's soundtrack: an edit of a real track plus recorded sound effects.

Music: "Vibe Ace" by Kevin MacLeod (incompetech.com), CC BY 4.0, from
assets/audio/music/vibe-ace.ogg. It runs on a steady 130 BPM grid (measured:
129.998 BPM, first beat at 28 ms, 8 ms jitter), and cues.json uses the same
grid, so cutting the track on its own beats keeps it in time with the
picture. The track itself is never time-stretched or re-pitched, except
the tape-stops.

The edit (film beats -> track beats):
  0 .. 7.5     track 24 ..   the end of the track's build, behind a low-pass
                             that opens up ("seeing isn't using")
  7.5 .. 8     silence       one breath before the drop
  8 .. 40.5    track 32 ..   the main groove, where Zero starts working
  40.5         tape-stop     the user moves the mouse: PAUSED
  44 .. 45.5   track 64 ..   resumes at the bar it stopped in
  45.5         tape-stop     Ctrl+Alt+Esc: STOPPED
  47 .. 55.5   track 67 ..   carries on as if it had kept running
  55.5 .. 56   silence       the screen collapses into the mark
  56 .. end    track 120 ..  the track's last two bars and its own ending

Sound effects: recorded, from the HyperFrames library (Pixabay Content
License), in assets/audio/sfx. Nothing here is synthesized.

Writes audio/mix-raw.wav; audio/master.sh sets the loudness.
"""
import json
import os
import subprocess

import numpy as np
from scipy.io import wavfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
C = json.load(open(os.path.join(ROOT, "cues.json")))
SR = 48000
BEAT = 60.0 / C["bpm"]
TRACK_T0 = 0.0282  # time of the track's beat 0, from the grid fit
PRE = 0.008  # cuts land just before a beat so a kick's attack is kept


def T(b):
    return b * BEAT


def load(path):
    raw = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path, "-f", "f32le", "-ac", "2", "-ar", str(SR), "-"],
        capture_output=True,
        check=True,
    ).stdout
    return np.frombuffer(raw, dtype=np.float32).reshape(-1, 2).astype(np.float64)


def ramp(n, up=True):
    r = 0.5 - 0.5 * np.cos(np.linspace(0, np.pi, max(n, 1)))
    return r if up else r[::-1]


# ------------------------------------------------------------------ music
track = load(os.path.join(ROOT, "assets", "audio", "music", "vibe-ace.ogg"))
total = T(C["duration_beats"])
N = int(round(total * SR))
music = np.zeros((N, 2))


def place(film_a, film_b, track_beat_at_a, fade_in=0.003, fade_out=0.003):
    """Copy the track into film beats [film_a, film_b) so that film beat film_a
    lines up with track beat track_beat_at_a. Each cut lands PRE early."""
    a = max(int(round((T(film_a) - PRE) * SR)), 0)
    b = min(int(round((T(film_b) - PRE) * SR)), N) if film_b is not None else N
    # track time = film time + shift
    shift = TRACK_T0 + track_beat_at_a * BEAT - T(film_a)
    src = int(round(a + shift * SR))
    seg = track[src : src + (b - a)].copy()
    fi, fo = int(fade_in * SR), int(fade_out * SR)
    seg[:fi] *= ramp(fi)[:, None]
    seg[len(seg) - fo :] *= ramp(fo, up=False)[:, None]
    music[a : a + len(seg)] += seg


def tape_stop(film_at, track_beat_at_film_at, length=0.32):
    """From film beat film_at, play the track slowing to a halt over `length` s."""
    a = int(round(T(film_at) * SR))
    src0 = (TRACK_T0 + track_beat_at_film_at * BEAT) * SR
    n = int(length * SR)
    i = np.arange(n)
    rate = (1 - i / n) ** 1.3
    pos = src0 + np.cumsum(rate)
    out = np.stack([np.interp(pos, np.arange(len(track)), track[:, ch]) for ch in (0, 1)], 1)
    env = np.ones(n)
    f = int(0.05 * SR)
    env[n - f :] = ramp(f, up=False)
    music[a : a + n] = out * env[:, None]


def sweep_lowpass(x, f_from, f_to, q=0.9):
    """Time-varying 2-pole low-pass (TPT state-variable filter), cutoff moving
    exponentially from f_from to f_to across x."""
    n = len(x)
    fc = f_from * (f_to / f_from) ** (np.arange(n) / max(n - 1, 1))
    g = np.tan(np.pi * fc / SR)
    k = 1.0 / q
    a1 = 1.0 / (1.0 + g * (g + k))
    out = np.zeros_like(x)
    for ch in range(2):
        ic1 = ic2 = 0.0
        col = x[:, ch]
        res = out[:, ch]
        for i in range(n):
            v3 = col[i] - ic2
            v1 = a1[i] * ic1 + g[i] * a1[i] * v3
            v2 = ic2 + g[i] * v1
            ic1 = 2 * v1 - ic1
            ic2 = 2 * v2 - ic2
            res[i] = v2
    return out


# intro: the end of the build, muffled, opening up; then a breath
place(0, C["gap"], 24, fade_in=0.05, fade_out=0.01)
ia, ib = 0, int(round((T(C["gap"]) - PRE) * SR))
music[ia:ib] = sweep_lowpass(music[ia:ib], 280.0, 16000.0)
# the drop: the main groove, mapped film b -> track b + 24
place(C["drop"], C["pause"], C["drop"] + 24)
tape_stop(C["pause"], C["pause"] + 24)
# resumes at the bar it stopped in (film 44 -> track 64), and keeps that mapping
place(C["resume"], C["stop"], 64)
tape_stop(C["stop"], C["stop"] + 20, length=0.24)
place(C["go"], C["collapse"], C["go"] + 20, fade_out=0.006)
# the reveal: the track's last two bars and its own ending
place(C["hit"], None, 120)
fo = int(0.45 * SR)
music[N - fo :] *= ramp(fo, up=False)[:, None]

# ------------------------------------------------------------------ sound effects
SFX_DIR = os.path.join(ROOT, "assets", "audio", "sfx")
_cache = {}


def S(name):
    if name not in _cache:
        _cache[name] = load(os.path.join(SFX_DIR, name + ".mp3"))
    return _cache[name]


# where each recording's event is (seconds into the file), so it lands on the cue
ANCHOR = {
    "click": 0.046,
    "key-press": 0.062,
    "pop": 0.108,
    "ping": 0.318,
    "whoosh": 0.170,  # the swell's peak
    "typing": 0.44,
    "impact-bass-1": 0.05,
    "impact-bass-2": 0.02,
    "sparkle": 0.02,
    "chime": 0.41,
}
sfx = np.zeros((N, 2))


def fx(name, at, gain, pan=0.0, length=None, fade=0.05, start=None, reverse=False, anchor=None):
    """Place recording `name` so its anchor lands on film beat `at`.
    start: skip the first `start` s of the file and put that point on the cue.
    anchor: override where the event is in the file (seconds)."""
    x = S(name)
    if reverse:
        x = x[::-1]
    if start is not None:
        x, anc = x[int(start * SR) :], 0.0
    elif anchor is not None:
        anc = anchor
    else:
        anc = ANCHOR.get(name, 0.0)
        if reverse:
            anc = len(x) / SR - anc
    x = x.copy()
    if length is not None:
        x = x[: int(length * SR)]
        f = int(fade * SR)
        x[len(x) - f :] *= ramp(f, up=False)[:, None]
    if pan:
        x = x * np.array([np.sqrt(1 - pan), np.sqrt(1 + pan)])
    i = int(round((T(at) - anc) * SR))
    j0 = max(0, -i)
    i = max(i, 0)
    n = min(len(x) - j0, N - i)
    if n > 0:
        sfx[i : i + n] += gain * x[j0 : j0 + n]


# intro
fx("click", C["shot0"], 0.45)
fx("whoosh", C["slice"] + 0.45, 0.5, pan=-0.2)
# drop
fx("impact-bass-1", C["drop"], 0.16, length=0.9, fade=0.6)
fx("typing", C["task"], 0.55, start=0.40, length=0.65, fade=0.12, pan=0.2)
fx("pop", C["call_state"], 0.22, pan=0.3)
# see / understand
fx("click", C["capture"], 0.5)
fx("key-press", C["mask"], 0.3)
for i in range(0, 21, 3):
    fx("key-press", C["boxes"] + i * 0.12, 0.2, pan=-0.5 + i * 0.05)
fx("whoosh", C["whip_tree"] + 0.25, 0.45, pan=0.4)
fx("ping", C["land4"], 0.16)
# act / verify
fx("pop", C["call_click"], 0.2, pan=0.3)
fx("click", C["click1"], 0.6)
fx("whoosh", C["iris"] + 0.25, 0.35)
for i, b in enumerate((C["n_mcp"], C["n_engine"], C["n_os"], C["n_press"], C["n_settle"])):
    fx("whoosh", b, 0.16, pan=0.35 if i % 2 else -0.35)
for i in range(3):
    fx("key-press", C["n_os"] + i * 0.33, 0.25, pan=-0.4 + 0.4 * i)
fx("click", C["n_press"] + 0.25, 0.65)
for at in (C["n_verify"], C["n_verify"] + 0.375, C["n_diff"] + 0.25, C["n_result"] + 0.125):
    fx("key-press", at, 0.22)
# remember
fx("whoosh", C["snap_back"], 0.4, pan=-0.3)
fx("pop", C["snap_back"], 0.2)
fx("click", C["click2"], 0.6)
for i in range(3):
    fx("pop", C["rows"] + i * 0.5, 0.22, pan=-0.2)
fx("whoosh", C["rewind"] + 0.5, 0.4, reverse=True, pan=0.3)
fx("ping", C["seen"], 0.18)
fx("pop", C["call_state2"], 0.2, pan=0.3)
fx("whoosh", C["swerve"], 0.25, pan=0.2)
fx("key-press", C["notattached"], 0.25)
# control
fx("pop", C["call_search"], 0.2, pan=0.3)
fx("click", C["user_in"], 0.3, pan=0.5)
for rep in (C["keys"], C["keys2"] - 0.5):
    for i in range(3):
        fx("key-press", rep + i * 0.25, 0.5, pan=-0.2 + 0.2 * i)
fx("impact-bass-1", C["stop"], 0.12, length=0.5, fade=0.35)
fx("click", C["click3"], 0.55)
# montage
fx("whoosh", C["tools"], 0.35)
fx("typing", C["tools"], 0.3, start=0.40, length=1.0, fade=0.2, pan=-0.2)
for i in range(3):
    fx("pop", C["os3"] + i * 0.33, 0.25, pan=-0.4 + 0.4 * i)
for i in range(5):
    fx("key-press", C["clients"] + i * 0.125, 0.2, pan=-0.3)
# the riser's own cut-off (4.25 s into the file) lands on the hit
fx("riser", C["hit"], 0.22, anchor=4.25)
fx("whoosh", C["collapse"] + 0.25, 0.45)
# reveal
fx("impact-bass-1", C["hit"], 0.26, length=1.4, fade=1.0)
fx("sparkle", C["hit"], 0.25)
fx("impact-bass-2", C["opensource"], 0.12, length=0.6, fade=0.4)
for i, b in enumerate((C["read"], C["run"], C["change"], C["ship"])):
    fx("key-press", b, 0.35, pan=-0.3 + 0.2 * i)
fx("chime", C["repo"], 0.22)

# ------------------------------------------------------------------ mix
# the music leads: effects sit about 7 LU under it
SFX_BUS = 0.85
mix = music + SFX_BUS * sfx
wavfile.write(os.path.join(HERE, "mix-raw.wav"), SR, (mix / max(1.0, np.abs(mix).max()) * 0.98).astype(np.float32))
print(f"mix-raw.wav {N / SR:.3f} s; music peak {np.abs(music).max():.2f}, sfx peak {np.abs(sfx).max():.2f}")
