"""Music and sound design for the Zero Use Computer launch film (film 2).

A 128 BPM track in D minor plus the film's sound effects, all generated
here from code (no samples, no third-party audio). Cue times come from
../cues.json in beats, the same grid the picture uses.

    python3 audio/synth.py            # writes audio/mix-raw.wav
"""
import json
import os
import sys

import numpy as np
from scipy import signal

SR = 48000
HERE = os.path.dirname(os.path.abspath(__file__))
rng = np.random.default_rng(7)  # seeded: the mix is reproducible


# ---------------------------------------------------------------- helpers
def t_axis(dur):
    return np.arange(int(dur * SR)) / SR


def env_ad(n, attack, decay_tau):
    """Attack (s) then exponential decay with time constant decay_tau (s)."""
    t = np.arange(n) / SR
    a = np.clip(t / max(attack, 1e-5), 0, 1)
    d = np.exp(-np.maximum(t - attack, 0) / decay_tau)
    return a * d


def bandpass(x, lo, hi, order=2):
    sos = signal.butter(order, [lo, hi], btype="band", fs=SR, output="sos")
    return signal.sosfilt(sos, x)


def highpass(x, f, order=2):
    sos = signal.butter(order, f, btype="high", fs=SR, output="sos")
    return signal.sosfilt(sos, x)


def lowpass(x, f, order=2):
    sos = signal.butter(order, f, btype="low", fs=SR, output="sos")
    return signal.sosfilt(sos, x)


def noise(n):
    return rng.standard_normal(n)


def norm(x, peak=1.0):
    m = np.max(np.abs(x)) or 1.0
    return x / m * peak


def pan(x, p):
    """Equal-power pan, p in [-1, 1]. Returns (n, 2)."""
    a = (p + 1) * np.pi / 4
    return np.stack([x * np.cos(a), x * np.sin(a)], axis=1)


def reverb_ir(dur=2.2, tau=0.55, bright=6000, seed=3):
    r = np.random.default_rng(seed)
    n = int(dur * SR)
    t = np.arange(n) / SR
    left = r.standard_normal(n) * np.exp(-t / tau)
    right = r.standard_normal(n) * np.exp(-t / tau)
    left = lowpass(left, bright)
    right = lowpass(right, bright)
    ir = np.stack([left, right], axis=1)
    ir[: int(0.012 * SR)] *= np.linspace(0, 1, int(0.012 * SR))[:, None]
    return ir / np.sqrt(np.sum(ir ** 2, axis=0))


def convolve_stereo(x, ir):
    out = np.zeros((x.shape[0] + ir.shape[0] - 1, 2))
    for c in range(2):
        out[:, c] = signal.fftconvolve(x[:, c], ir[:, c])
    return out


# ------------------------------------------------------------ instruments
def click(strength=1.0, tone=2600):
    """Mechanical switch: a tiny noise transient plus a damped resonance."""
    n = int(0.09 * SR)
    burst = highpass(noise(n), 1800) * env_ad(n, 0.0004, 0.004)
    t = np.arange(n) / SR
    ping = np.sin(2 * np.pi * tone * t) * env_ad(n, 0.0002, 0.012)
    body = np.sin(2 * np.pi * 180 * t) * env_ad(n, 0.0005, 0.018) * 0.5
    return (burst * 0.8 + ping * 0.35 + body) * strength


def shutter():
    """Two-stage capture: open and close, ~70 ms apart."""
    a = click(0.9, 3400)
    b = click(0.7, 2900)
    out = np.zeros(int(0.2 * SR))
    out[: a.size] += a
    off = int(0.07 * SR)
    out[off: off + b.size] += b
    return out


def blip(freq, dur=0.07, amp=0.25):
    n = int(dur * SR)
    t = np.arange(n) / SR
    s = np.sin(2 * np.pi * freq * t) + 0.25 * np.sin(2 * np.pi * freq * 2.01 * t)
    return s * env_ad(n, 0.002, dur / 4) * amp


def glide(dur=0.5, lo=400, hi=3500, amp=0.18):
    """Soft air movement for the cursor: a filtered noise swell."""
    n = int(dur * SR)
    x = noise(n)
    out = np.zeros(n)
    hop = 512
    for i in range(0, n, hop):
        f = lo + (hi - lo) * (i / n)
        seg = x[i: i + hop]
        out[i: i + hop] = seg
    out = bandpass(out, lo, hi)
    e = np.sin(np.pi * np.linspace(0, 1, n)) ** 2
    return out * e * amp


def whoosh(dur=0.8, f0=200, f1=4000, amp=0.3, reverse=False):
    """Band-limited noise sweep via short-time filtering."""
    n = int(dur * SR)
    x = noise(n)
    out = np.zeros(n)
    hop = 1024
    for i in range(0, n, hop):
        frac = i / n
        fc = f0 * (f1 / f0) ** frac
        lo, hi = max(40, fc * 0.6), min(SR / 2 - 100, fc * 1.6)
        seg = x[max(0, i - 2048): i + hop]
        y = bandpass(seg, lo, hi)
        out[i: i + hop] = y[-min(hop, n - i):]
    e = np.sin(np.pi * np.linspace(0, 1, n) ** 0.7) ** 2
    out = out * e * amp
    return out[::-1] if reverse else out


def sub_hit(freq=48, dur=1.6, amp=0.9, drop=0.5):
    """Sine kick with a pitch drop: the heavy structural hit."""
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = freq * (1 + drop * np.exp(-t / 0.05))
    ph = 2 * np.pi * np.cumsum(f) / SR
    body = np.sin(ph) * env_ad(n, 0.002, dur / 3.2)
    trans = highpass(noise(n), 2500) * env_ad(n, 0.0005, 0.01) * 0.4
    return (body + trans) * amp


def tick(amp=0.12, tone=5200):
    n = int(0.03 * SR)
    t = np.arange(n) / SR
    return (np.sin(2 * np.pi * tone * t) * env_ad(n, 0.0002, 0.003)
            + highpass(noise(n), 6000) * env_ad(n, 0.0002, 0.002) * 0.5) * amp


def chord_swell(freqs, dur=2.5, amp=0.12, attack=0.6, tau=0.9):
    n = int(dur * SR)
    t = np.arange(n) / SR
    s = np.zeros(n)
    for i, f in enumerate(freqs):
        det = 1 + 0.0015 * (i - len(freqs) / 2)
        s += np.sin(2 * np.pi * f * det * t + i)
        s += 0.3 * np.sin(2 * np.pi * f * 2 * det * t)
    s = lowpass(s, 3500)
    return s / len(freqs) * env_ad(n, attack, tau) * amp


def stop_tone():
    """Halting: a short pitch-down of a mid tone plus a low thud."""
    n = int(0.5 * SR)
    t = np.arange(n) / SR
    f = 660 * np.exp(-t / 0.12) + 110
    ph = 2 * np.pi * np.cumsum(f) / SR
    tone = np.sin(ph) * env_ad(n, 0.001, 0.12) * 0.35
    thud = sub_hit(70, 0.5, 0.6, 0.3)[:n]
    return tone + thud


def data_train(dur, rate=28, amp=0.06, tone=3000):
    """A stream of tiny ticks: data travelling along a path."""
    n = int(dur * SR)
    out = np.zeros(n)
    k = int(dur * rate)
    for i in range(k):
        pos = int((i / rate + rng.uniform(-0.004, 0.004)) * SR)
        tk = tick(amp * rng.uniform(0.6, 1.0), tone * rng.uniform(0.85, 1.15))
        if 0 <= pos < n - tk.size:
            out[pos: pos + tk.size] += tk
    fade = np.sin(np.pi * np.linspace(0, 1, n)) ** 0.5
    return out * fade


def pencil(dur=0.4, amp=0.06, bright=1.0):
    """A pencil line on paper: grainy high noise with a scratchy envelope."""
    n = int(dur * SR)
    x = bandpass(noise(n), 1800 * bright, min(9000 * bright, 20000))
    grain = np.abs(lowpass(noise(n), 60)) + 0.3
    e = np.sin(np.pi * np.linspace(0, 1, n)) ** 0.6
    return x * grain * e * amp


def thock(amp=0.5, f=95):
    """A typographic stamp: a short, dry, low body hit."""
    n = int(0.35 * SR)
    t = np.arange(n) / SR
    fr = f * (1 + 0.8 * np.exp(-t / 0.02))
    body = np.sin(2 * np.pi * np.cumsum(fr) / SR) * env_ad(n, 0.001, 0.07)
    snap = bandpass(noise(n), 900, 4000) * env_ad(n, 0.0005, 0.012) * 0.35
    return (body + snap) * amp


def key_thock(amp=0.35):
    """A mechanical keycap bottoming out."""
    n = int(0.12 * SR)
    t = np.arange(n) / SR
    c = bandpass(noise(n), 1200, 5000) * env_ad(n, 0.0004, 0.006)
    b = np.sin(2 * np.pi * 240 * t) * env_ad(n, 0.0008, 0.02) * 0.6
    return (c + b) * amp


def riser(dur, f0=120, f1=2400, amp=0.2):
    """Filtered noise that rises and swells into a hit."""
    out = whoosh(dur, f0, f1, amp)
    n = out.size
    shape = np.linspace(0, 1, n) ** 2.2
    return out / (np.max(np.abs(out)) + 1e-9) * shape * amp


def tape_stop(dur=0.35, f=110, amp=0.3):
    n = int(dur * SR)
    t = np.arange(n) / SR
    fr = f * (1 - t / dur) ** 1.5 + 8
    s = np.sin(2 * np.pi * np.cumsum(fr) / SR) + 0.4 * np.sin(4 * np.pi * np.cumsum(fr) / SR)
    return s * (1 - t / dur) * amp


def reverse_sweep(dur=0.55, amp=0.18):
    x = whoosh(dur, 300, 6000, amp)
    return x[::-1] * np.linspace(0.3, 1, x.size)




# ------------------------------------------------------------ drum machine
def kick(amp=1.0):
    n = int(0.42 * SR)
    t = np.arange(n) / SR
    f = 46 + 110 * np.exp(-t / 0.035)
    body = np.sin(2 * np.pi * np.cumsum(f) / SR) * env_ad(n, 0.001, 0.16)
    click_ = highpass(noise(n), 3000) * env_ad(n, 0.0003, 0.004) * 0.35
    return np.tanh((body + click_) * 1.6) / np.tanh(1.6) * amp


def clap(amp=0.5):
    n = int(0.4 * SR)
    out = np.zeros(n)
    for k, d in enumerate((0.0, 0.011, 0.022)):
        i = int(d * SR)
        m = n - i
        out[i:] += bandpass(noise(m), 900, 5200) * env_ad(m, 0.0005, 0.012 if k < 2 else 0.11)
    return out * amp


def hat(amp=0.12, open_=False):
    n = int((0.22 if open_ else 0.05) * SR)
    x = highpass(noise(n), 7500)
    return x * env_ad(n, 0.0004, 0.09 if open_ else 0.014) * amp


def saw(freq, dur, harmonics=24):
    t = t_axis(dur)
    s = np.zeros_like(t)
    for h in range(1, harmonics + 1):
        if freq * h > SR / 2.2:
            break
        s += np.sin(2 * np.pi * freq * h * t) / h
    return s * 0.6


def bass_note(freq, dur, amp=0.4, cutoff=900):
    x = saw(freq, dur, 18) + 0.5 * np.sin(2 * np.pi * freq * t_axis(dur))
    n = x.size
    x = lowpass(x, cutoff, 2)
    return x * env_ad(n, 0.003, dur * 0.55) * amp


def pluck(freq, dur=0.22, amp=0.12, bright=3000):
    x = saw(freq, dur, 16)
    x = lowpass(x, bright, 2)
    return x * env_ad(x.size, 0.002, dur / 4) * amp


def note(name):
    # D minor palette, A4 = 440
    table = {"D1": 36.71, "F1": 43.65, "G1": 49.0, "A1": 55.0, "Bb1": 58.27, "C2": 65.41, "D2": 73.42,
             "F2": 87.31, "A2": 110.0, "Bb2": 116.54, "C3": 130.81, "D3": 146.83, "E3": 164.81,
             "F3": 174.61, "G3": 196.0, "A3": 220.0, "Bb3": 233.08, "C4": 261.63, "D4": 293.66,
             "E4": 329.63, "F4": 349.23, "G4": 392.0, "A4": 440.0, "C5": 523.25, "D5": 587.33,
             "E5": 659.25, "F5": 698.46, "A5": 880.0, "D6": 1174.66}
    return table[name]


def tape_stop_region(x, t0, length):
    """Slow the bus down to a halt over `length` seconds starting at t0."""
    i0 = int(t0 * SR)
    n = int(length * SR)
    speed = np.linspace(1.0, 0.0, n) ** 1.3
    pos = i0 + np.cumsum(speed)
    for c in range(x.shape[1]):
        src = x[:, c]
        x[i0:i0 + n, c] = np.interp(pos, np.arange(src.size), src) * np.linspace(1, 0.2, n)
    x[i0 + n:, :] = 0  # caller re-enables later regions
    return x


# ------------------------------------------------------------------ score
def render():
    cues = json.load(open(os.path.join(HERE, "..", "cues.json")))
    C = {k: v for k, v in cues.items() if not k.startswith("_")}
    BEAT = 60.0 / C["bpm"]
    T = lambda b: b * BEAT  # beats -> seconds
    total = T(C["duration_beats"])
    N = int((total + 1.0) * SR)
    music = np.zeros((N, 2))   # the track (gets tape-stopped and muted on pause / stop)
    sfx = np.zeros((N, 2))     # sound effects (never muted)
    send = np.zeros((N, 2))

    def add(buf, x, b, gain=1.0, p=0.0, rv=0.0):
        if x.ndim == 1:
            x = pan(x, p)
        i = int(round(T(b) * SR))
        if i >= N or i < 0:
            return
        j = min(N, i + x.shape[0])
        buf[i:j] += x[: j - i] * gain
        if rv:
            send[i:j] += x[: j - i] * gain * rv

    # ---------------- intro (B0-B8): ticks, word hits, the build, the gap
    for b in range(0, 4):
        add(sfx, tick(0.12, 5200), b, p=0.0)
    for b, g in ((C["w_aican"], 0.55), (C["w_see0"], 0.6), (C["w_yourscreen"], 0.6)):
        add(sfx, thock(g, 90), b, rv=0.2)
        add(sfx, sub_hit(52, 0.6, 0.35, 0.6), b)
    add(sfx, shutter(), C["shot0"], 0.6, rv=0.15)
    for b in (C["w_seeing"], C["w_isnt"], C["w_using"]):
        add(music, kick(0.9), b)
        add(sfx, thock(0.5, 80), b, rv=0.2)
    for i in range(16):  # snare build: 8ths then 16ths
        b = 4 + i * 0.25 if i >= 8 else 4 + i * 0.25
        if b < 6 and i % 2:
            continue
        add(music, clap(0.12 + 0.02 * i), b, p=0.1 * ((i % 2) * 2 - 1))
    add(music, riser(T(3.5), 120, 6000, 0.3), 4, rv=0.3)
    add(sfx, whoosh(0.28, 600, 9000, 0.35), C["slice"], p=-0.3, rv=0.2)
    add(sfx, glide(0.2, 900, 7000, 0.25), C["slice"], p=0.3)

    # ---------------- the groove (B8 onwards), gated later by pause / stop
    groove_end = C["collapse"]
    prog = ["D2", "Bb1", "F2", "C2"]  # per bar: Dm, Bb, F, C
    bass_line = [0, 0, 12, 0, 0, 7, 0, 12, 0, 0, 12, 0, 3, 0, 7, 12]  # semitone offsets per 16th
    b = C["drop"]
    while b < groove_end:
        bar = int((b - C["drop"]) // 4)
        pos = (b - C["drop"]) % 4
        root = note(prog[bar % 4])
        add(music, kick(1.0), b)
        if pos in (1, 3):
            add(music, clap(0.42), b, rv=0.25)
        add(music, hat(0.06), b + 0.25, p=0.3)
        add(music, hat(0.09, True), b + 0.5, p=-0.2)
        add(music, hat(0.05), b + 0.75, p=0.3)
        for s in range(4):  # 16th bass, ducked away from the kick
            k = int(pos * 4 + s) % 16
            f = root * 2 ** (bass_line[k] / 12)
            g = 0.18 if s == 0 else 0.34
            add(music, bass_note(f, T(0.24), g, 700 + 500 * (s % 2)), b + s * 0.25)
        b += 1
    # arp in the act and the build
    arp = ["D4", "F4", "A4", "C5", "E5", "C5", "A4", "F4"]
    for start, end, amp in ((C["call_click"], C["snap_back"], 0.09), (C["tools"], groove_end, 0.11)):
        b = start
        i = 0
        while b < end:
            add(music, pluck(note(arp[i % 8]), 0.2, amp, 2600), b, p=0.35 * ((i % 2) * 2 - 1), rv=0.3)
            b += 0.25
            i += 1
    # pad under everything from the drop, chords per bar
    chords = [["D3", "A3", "C4", "F4"], ["Bb2", "F3", "A3", "D4"], ["F3", "A3", "C4", "E4"], ["C3", "G3", "Bb3", "E4"]]
    b = C["drop"]
    while b < groove_end:
        bar = int((b - C["drop"]) // 4)
        ch = [note(n) for n in chords[bar % 4]]
        add(music, chord_swell(ch, T(4.2), 0.06, 0.08, 1.4), b, rv=0.4)
        b += 4
    # build before the collapse
    for i in range(16):
        add(music, clap(0.1 + 0.02 * i), C["build"] + i * 0.25, p=0.2 * ((i % 2) * 2 - 1))
    add(music, riser(T(3.5), 150, 8000, 0.34), C["build"], rv=0.35)

    # ---------------- sound effects on the picture's events
    add(sfx, sub_hit(40, 2.2, 1.0, 1.0), C["drop"], rv=0.35)
    add(sfx, whoosh(0.6, 8000, 300, 0.25), C["drop"], rv=0.3)
    add(sfx, chord_swell([note("D3"), note("A3"), note("E4")], 2.0, 0.12, 0.01, 0.6), C["drop"], rv=0.5)
    for i in range(29):  # the task being typed
        add(sfx, tick(0.035, 3800 + 37 * (i % 5)), C["task"] + i * (1.25 / 29), p=0.4)
    add(sfx, blip(note("D5"), 0.12, 0.07), C["think1"], rv=0.4)
    add(sfx, tick(0.09, 3600), C["call_state"], p=0.5)
    add(sfx, shutter(), C["capture"], 0.8, rv=0.2)
    add(sfx, sub_hit(48, 0.9, 0.55, 0.7), C["capture"], rv=0.3)
    add(sfx, thock(0.3, 150), C["mask"])
    dm9 = ["D4", "F4", "A4", "C5", "E5", "F5", "A5", "D6"]
    for i in range(21):  # the boxes play an arpeggio
        add(sfx, pluck(note(dm9[i % 8]) * (2 if i >= 16 else 1), 0.12, 0.07, 5000), C["boxes"] + i * 0.12, p=-0.5 + i * 0.05, rv=0.25)
    add(sfx, whoosh(T(0.5), 300, 7000, 0.32), C["whip_tree"], p=0.5, rv=0.2)
    add(sfx, whoosh(T(2), 5000, 400, 0.12), C["ticker"])
    add(sfx, sub_hit(50, 0.8, 0.5, 0.6), C["w_und"], rv=0.3)
    add(sfx, blip(note("D5"), 0.12, 0.07), C["think2"], rv=0.4)
    add(sfx, whoosh(T(0.5), 7000, 300, 0.3), C["call_click"], p=-0.5)
    add(sfx, glide(T(0.45), 700, 5000, 0.2), C["dart"], p=-0.2)
    add(sfx, click(1.1, 2700), C["click1"], rv=0.2)
    add(sfx, sub_hit(46, 1.0, 0.6, 0.8), C["click1"], rv=0.3)
    add(sfx, riser(T(0.75), 300, 9000, 0.25), C["iris"], rv=0.3)
    for i, at in enumerate((C["n_engine"], C["n_os"], C["n_press"], C["n_settle"])):
        add(sfx, whoosh(T(0.42), 400, 8000, 0.3), at - 0.21, p=-0.6 + 0.4 * i)
        add(sfx, thock(0.35, 110 + 10 * i), at, rv=0.2)
    for i in range(3):
        add(sfx, thock(0.3, 140 + 20 * i), C["n_os"] + i * 0.33, p=-0.5 + 0.5 * i)
    add(sfx, click(1.0, 2300), C["n_press"] + 0.25, rv=0.3)
    for i, at in enumerate((C["n_verify"], C["n_verify"] + 0.375, C["n_diff"] + 0.25, C["n_result"] + 0.125)):
        add(sfx, blip(note(["A4", "C5", "D5", "F5"][i]), 0.1, 0.06), at, p=0.3, rv=0.3)
    add(sfx, reverse_sweep(T(0.4), 0.22), C["snap_back"] - 0.4, rv=0.3)
    add(sfx, sub_hit(50, 0.8, 0.5, 0.6), C["snap_back"], rv=0.3)
    add(sfx, glide(T(0.45), 700, 5000, 0.18), C["dart2"])
    add(sfx, click(1.0, 2800), C["click2"], rv=0.2)
    for i in range(3):
        add(sfx, tick(0.08, 2600 - 200 * i), C["rows"] + i * 0.5, p=-0.3)
    add(sfx, reverse_sweep(T(0.6), 0.3), C["rewind"], p=0.5, rv=0.4)
    add(sfx, whoosh(T(0.5), 9000, 200, 0.2), C["rewind"], p=-0.5)
    add(sfx, sub_hit(50, 0.8, 0.5, 0.6), C["seen"], rv=0.3)
    for i in range(11):
        add(sfx, blip(988 * 2 ** (-i / 24), 0.04, 0.035), C["seen"] + 0.75 + i * 0.07, p=-0.3, rv=0.2)
    add(sfx, thock(0.45, 90), C["w_rem"], rv=0.2)
    add(sfx, tick(0.09, 3600), C["call_state2"], p=0.5)
    add(sfx, data_train(T(1), 34, 0.05, 3200), C["call_state2"], p=0.2)
    add(sfx, blip(note("D6"), 0.07, 0.07), C["swerve"], p=0.3)
    add(sfx, blip(note("A5"), 0.16, 0.07), C["notattached"], rv=0.4)
    # control: the track stops, the sound effects carry the silence
    add(sfx, tick(0.09, 3600), C["call_search"], p=0.5)
    add(sfx, thock(0.4, 70), C["pause"])
    n = int(T(C["resume"] - C["user_stop"]) * SR)
    tt = np.arange(n) / SR
    rise_ = np.sin(2 * np.pi * np.cumsum(220 + 220 * tt / tt[-1]) / SR) * np.linspace(0, 1, n) ** 1.6 * 0.05
    add(sfx, rise_, C["user_stop"])
    add(sfx, sub_hit(44, 1.2, 0.7, 0.8), C["resume"], rv=0.3)
    for rep in (C["keys"], C["keys2"] - 0.5):
        for i in range(3):
            add(sfx, key_thock(0.55), rep + i * 0.25, p=-0.3 + 0.3 * i)
    add(sfx, stop_tone(), C["stop"], 0.9, rv=0.3)
    add(sfx, sub_hit(44, 1.2, 0.7, 0.8), C["go"], rv=0.3)
    add(sfx, click(1.0, 2600), C["click3"], rv=0.2)
    for i in range(25):
        add(sfx, blip(1318 * 2 ** ((i % 6) / 12), 0.035, 0.03), C["tools"] + i * 0.075, p=-0.6 + 0.05 * i)
    for i in range(3):
        add(sfx, sub_hit(60 + 6 * i, 0.5, 0.4, 0.5), C["os3"] + i * 0.33, rv=0.2)
    for i in range(5):
        add(sfx, tick(0.07, 3000), C["clients"] + i * 0.125, p=-0.4)
    add(sfx, riser(T(0.5), 400, 12000, 0.35), C["collapse"], rv=0.3)
    # the mark: the strongest hit of the film
    add(sfx, sub_hit(38, 3.5, 1.15, 1.2), C["hit"], rv=0.35)
    add(sfx, kick(1.0), C["hit"])
    add(sfx, whoosh(0.5, 1500, 14000, 0.25), C["hit"] - 0.05, rv=0.5)
    add(sfx, chord_swell([note("D2"), note("D3"), note("A3"), note("E4"), note("A4")], 6.0, 0.22, 0.02, 2.6), C["hit"], rv=0.7)
    add(sfx, sub_hit(50, 1.6, 0.55, 0.6), C["opensource"], rv=0.3)
    add(sfx, chord_swell([note("F3"), note("A3"), note("C4"), note("E4")], 4.0, 0.1, 0.02, 1.6), C["opensource"], rv=0.6)
    for i, b in enumerate((C["read"], C["run"], C["change"], C["ship"])):
        add(sfx, pluck(note(["D5", "F5", "A5", "D6"][i]), 0.5, 0.11, 4500), b, p=-0.3 + 0.2 * i, rv=0.5)
    add(sfx, tick(0.08, 3400), C["repo"])

    # ---------------- the pause and the stop: tape-stop the track, then silence
    def gate(buf, a, b_):
        i, j = int(T(a) * SR), int(T(b_) * SR)
        buf[i:j] = 0
    m2 = music.copy()
    tape_stop_region(m2, T(C["pause"]), 0.32)
    tail_keep = m2.copy()
    music_out = music.copy()
    i_p, i_r = int(T(C["pause"]) * SR), int(T(C["resume"]) * SR)
    music_out[i_p:i_r] = tail_keep[i_p:i_r]
    music_out[i_p + int(0.32 * SR):i_r] = 0
    m3 = music.copy()
    tape_stop_region(m3, T(C["stop"]), 0.28)
    i_s, i_g = int(T(C["stop"]) * SR), int(T(C["go"]) * SR)
    music_out[i_s:i_g] = m3[i_s:i_g]
    music_out[i_s + int(0.28 * SR):i_g] = 0
    # the gap before the drop
    gate(music_out, C["gap"], C["drop"])
    # fade the groove into the collapse
    i_c = int(T(C["collapse"]) * SR)
    music_out[i_c:] = 0

    # ---------------- mix
    # sidechain: duck the music under each kick
    duck = np.ones(N)
    b = C["drop"]
    while b < C["collapse"]:
        i = int(T(b) * SR)
        L = int(0.18 * SR)
        duck[i:i + L] = np.minimum(duck[i:i + L], 0.55 + 0.45 * np.linspace(0, 1, L) ** 0.6)
        b += 1
    kick_free = music_out * duck[:, None]
    ir = reverb_ir(2.2, 0.55)
    wet = convolve_stereo(send, ir)[:N] * 0.3
    mix = kick_free * 0.72 + sfx + wet
    room = lowpass(noise(N), 900) * 0.002
    mix[:, 0] += room
    mix[:, 1] += np.roll(room, 777)
    mix = np.tanh(mix * 1.15) / 1.15
    end = int(total * SR)
    fe = int(T(C["end_fade"]) * SR)
    mix[fe:end] *= np.linspace(1, 0, end - fe)[:, None] ** 2
    return mix[:end]


def write_wav(path, x):
    import wave
    x = np.clip(x, -1, 1)
    pcm = (x * 32767).astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())


if __name__ == "__main__":
    out = os.path.join(HERE, "mix-raw.wav")
    m = render()
    m = m / (np.max(np.abs(m)) + 1e-9) * 0.89
    write_wav(out, m)
    print("wrote", out, m.shape[0] / SR, "s")
