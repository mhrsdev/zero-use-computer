"""Procedural sound design for the Z-MCP launch film.

Every sound in the film is generated here from code (no samples, no
third-party audio), so the mix is fully licensable with the project.
Cue times come from ../cues.json, the same file the visuals read.

    python3 audio/synth.py            # writes audio/mix.wav
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


# ------------------------------------------------------------------ score
def render():
    cues = json.load(open(os.path.join(HERE, "..", "cues.json")))
    C = {k: v for k, v in cues.items() if not k.startswith("_")}
    total = C["duration"] + 0.5
    N = int(total * SR)
    dry = np.zeros((N, 2))   # foley and hits
    bed = np.zeros((N, 2))   # music bed
    wet_send = np.zeros((N, 2))

    def add(buf, x, t, gain=1.0, p=0.0, send=0.0):
        if x.ndim == 1:
            x = pan(x, p)
        i = int(round(t * SR))
        if i >= N:
            return
        j = min(N, i + x.shape[0])
        buf[i:j] += x[: j - i] * gain
        if send:
            wet_send[i:j] += x[: j - i] * gain * send

    # ---- bed: drone, pad and pulse, gated by the story ----
    t = np.arange(N) / SR
    def gate(segments, attack=0.25, release=0.3, curve=None):
        g = np.zeros(N)
        for a, b, lvl in segments:
            ia, ib = int(a * SR), int(b * SR)
            ra = int(attack * SR)
            rr = int(release * SR)
            g[ia:ib] = np.maximum(g[ia:ib], lvl)
            ramp = np.linspace(0, lvl, ra)
            g[ia:ia + ra] = np.minimum(g[ia:ia + ra], ramp[: len(g[ia:ia + ra])]) if ra else g[ia:ia + ra]
            if rr:
                tail = np.linspace(lvl, 0, rr)
                seg = g[ib:ib + rr]
                g[ib:ib + rr] = np.maximum(seg, tail[: seg.size])
        return lowpass(g, 30, 1)

    live = [(C["hit1"], C["pause_on"], 1.0), (C["resume"], C["stop_key"], 1.0), (C["stop_release"], C["done"] + 0.6, 1.0)]

    # drone: D1 + D2, slow breathing
    drone = (0.3 * np.sin(2 * np.pi * 36.71 * t) + 0.45 * np.sin(2 * np.pi * 73.42 * t + 0.4)
             + 0.16 * np.sin(2 * np.pi * 110.0 * t) + 0.05 * np.sin(2 * np.pi * 146.83 * t + 1.1))
    drone *= 0.75 + 0.25 * np.sin(2 * np.pi * 0.11 * t)
    drone_gate = gate([(C["capture1"], C["pause_on"], 0.5), (C["resume"], C["stop_key"], 0.5),
                       (C["stop_release"], C["pull"] + 0.4, 0.5)], attack=1.4, release=0.5)
    # a held low tone while paused / stopped (the grey and the orange)
    hold = np.sin(2 * np.pi * 55 * t) * 0.5 + 0.2 * np.sin(2 * np.pi * 82.4 * t)
    hold_gate = gate([(C["pause_on"] + 0.25, C["resume"], 0.35), (C["stop_key"] + 0.15, C["stop_release"], 0.45)],
                     attack=0.6, release=0.2)
    bed[:, 0] += drone * drone_gate * 0.09 + hold * hold_gate * 0.08
    bed[:, 1] += drone * drone_gate * 0.09 + hold * hold_gate * 0.08

    # pad: Dm9 (D3 A3 C4 E4 F4), resolves to D add9 at "done"
    pad_a = [146.83, 220.0, 261.63, 329.63, 349.23]
    pad_b = [146.83, 220.0, 293.66, 329.63, 369.99]
    def pad(freqs, phase=0.0):
        s = np.zeros(N)
        for i, f in enumerate(freqs):
            for d in (-0.6, 0.6):
                s += np.sin(2 * np.pi * (f + d * 0.35) * t + i * 1.7 + phase)
        return lowpass(s / (2 * len(freqs)), 1800)
    pa = pad(pad_a)
    pb = pad(pad_b, 0.3)
    ga = gate([(C["hit1"], C["pause_on"], 1.0), (C["resume"], C["stop_key"], 0.8),
               (C["stop_release"], C["done"], 1.0)], attack=1.8, release=0.25)
    gb = gate([(C["done"], C["glow_out"] + 1.2, 1.0)], attack=0.5, release=1.4)
    bed[:, 0] += (pa * ga + pb * gb) * 0.1
    bed[:, 1] += (np.roll(pa, 240) * ga + np.roll(pb, 240) * gb) * 0.1

    # pulse: 120 BPM grid locked to the first hit; stops dead on pause / stop
    beat = 0.5
    k = 0
    while True:
        tb = C["hit1"] + k * beat / 2  # eighths
        if tb > C["done"] + 0.5:
            break
        on = any(a <= tb < b for a, b, _ in live)
        if on:
            if k % 2 == 0:
                lvl = 0.11 if (k // 2) % 4 == 0 else 0.065
                add(bed, highpass(sub_hit(64, 0.3, lvl, 0.6)[: int(0.28 * SR)], 40), tb)
            else:
                add(bed, tick(0.035, 6500), tb, p=0.25)
            # sixteenths while the call travels the path
            if C["pipe_build"] <= tb < C["iris_close"]:
                add(bed, tick(0.02, 8000), tb + beat / 4, p=-0.25)
        k += 1

    # ---- foley and hits ----
    # S1 — the capture and the arrival
    add(dry, shutter(), C["capture1"], 0.55, send=0.15)
    add(dry, glide(0.55, 500, 4000, 0.22), C["zero_enter"], 1.0, p=0.4)
    add(dry, sub_hit(44, 1.6, 0.7, 0.9), C["hit1"], 1.0, send=0.25)
    add(dry, whoosh(0.4, 2000, 9000, 0.12), C["hit1"] - 0.02, 1.0, send=0.3)
    add(dry, chord_swell([146.83, 220.0, 329.63], 2.4, 0.12, 0.02, 0.8), C["hit1"], 1.0, send=0.5)
    add(dry, whoosh(0.9, 150, 900, 0.08), C["cam_mid"], 1.0)

    # S2 — see / understand
    add(dry, tick(0.06, 3200), C["task_chip"], 1.0, p=0.6)
    add(dry, blip(523.25, 0.12, 0.05), C["think1"], 1.0, send=0.4)
    add(dry, tick(0.07, 3600), C["call_state"], 1.0, p=0.6)
    add(dry, data_train(0.3, 40, 0.05, 3500), C["call_state"] + 0.05, 1.0, p=0.3)
    add(dry, shutter(), C["capture2"], 0.5, send=0.15)
    add(dry, thock(0.22, 140), C["mask"], 1.0)
    for i in range(21):
        f = 880 * 2 ** (i / 30)
        add(dry, blip(f, 0.05, 0.035), C["marks"] + i * 0.045, 1.0, p=-0.4 + i * 0.03, send=0.2)
        add(dry, pencil(0.25, 0.018), C["marks"] + i * 0.045, 1.0, p=-0.3)
    add(dry, whoosh(0.95, 180, 1200, 0.07), C["cam_left"], 1.0)
    add(dry, thock(0.42), C["w_see"], 1.0, send=0.15)
    for i in range(21):
        add(dry, tick(0.03, 4200 + 40 * i), C["tree_lines"] + i * 0.062, 1.0, p=0.5)
        add(dry, pencil(0.4, 0.012, 0.8), C["tree_lines"] + i * 0.062 - 0.05, 1.0, p=0.35)
    add(dry, thock(0.42), C["w_und"], 1.0, send=0.15)
    add(dry, pencil(0.5, 0.04, 1.2), C["target_hot"], 1.0, p=0.2)
    add(dry, blip(659.25, 0.18, 0.06), C["target_hot"] + 0.05, 1.0, send=0.4)
    add(dry, blip(523.25, 0.12, 0.05), C["think2"], 1.0, send=0.4)

    # S3 — act: the click becomes the call path
    add(dry, tick(0.07, 3600), C["call_click"], 1.0, p=0.6)
    add(dry, whoosh(0.8, 200, 1400, 0.07), C["cam_mid2"], 1.0)
    add(dry, glide(0.62, 600, 4500, 0.2), C["zero_glide1"], 1.0, p=-0.2)
    add(dry, click(1.0, 2600), C["click1"], 0.9, send=0.2)
    add(dry, riser(0.85, 200, 5000, 0.22), C["iris_open"], 1.0, send=0.35)
    add(dry, sub_hit(50, 0.9, 0.45, 0.6), C["iris_open"] + 0.85, 1.0, send=0.3)
    add(dry, thock(0.4), C["w_act"], 1.0, send=0.15)
    add(dry, pencil(0.9, 0.035), C["pipe_build"], 1.0, p=-0.3)
    pings = [C["pipe_build"] + 0.1, C["pkt_engine"] - 0.05, C["pkt_backend"] - 0.05, C["lanes"] - 0.15, C["app_press"] - 0.05]
    for i, pt in enumerate(pings):
        add(dry, blip(784 * 2 ** (i / 12), 0.09, 0.05), pt, 1.0, p=-0.6 + i * 0.3, send=0.3)
    for i, at in enumerate([C["pkt_mcp"], C["pkt_engine"], C["pkt_backend"], C["pkt_app"]]):
        add(dry, data_train(0.42, 34, 0.045, 3000 + 300 * i), at, 1.0, p=-0.6 + i * 0.35)
    add(dry, click(0.8, 2200), C["app_press"], 1.0, p=0.7, send=0.2)
    for i, at in enumerate([C["ret_settle"], C["ret_verify"], C["ret_diff"], C["ret_agent"]]):
        add(dry, data_train(0.35, 30, 0.04, 2600 - 150 * i), at, 1.0, p=0.6 - i * 0.35)
        add(dry, blip(587.33 * 2 ** (-i / 12), 0.09, 0.045), at + 0.32, 1.0, p=0.6 - i * 0.35, send=0.3)
    # settle: two reads that agree
    add(dry, tick(0.06, 5000), C["ret_settle"] + 0.42, 1.0, p=0.3)
    add(dry, tick(0.06, 5000), C["ret_settle"] + 0.52, 1.0, p=0.3)
    add(dry, pencil(0.7, 0.03), C["ret_settle"], 1.0, p=0.3)
    add(dry, thock(0.4), C["w_ver"], 1.0, send=0.15)
    add(dry, reverse_sweep(0.6, 0.18), C["iris_close"], 1.0, send=0.3)
    add(dry, chord_swell([293.66, 440.0], 1.4, 0.07, 0.03, 0.4), C["dialog_in"], 1.0, send=0.5)
    for i in range(5):
        add(dry, tick(0.035, 4400), C["rep2_in"] + 0.12 + i * 0.09, 1.0, p=0.5)

    # S4 — control
    add(dry, blip(523.25, 0.12, 0.05), C["think3"], 1.0, send=0.4)
    add(dry, tick(0.07, 3600), C["call_ok"], 1.0, p=0.6)
    add(dry, glide(0.47, 600, 3000, 0.16), C["zero_go1"], 1.0, p=0.1)
    add(dry, tape_stop(0.38, 120, 0.22), C["pause_on"], 1.0)
    add(dry, thock(0.3, 70), C["pause_on"], 1.0)
    add(dry, thock(0.4), C["w_ctl"], 1.0, send=0.15)
    # idle time filling: a faint rising tone, 1.5 s
    n = int((C["resume"] - C["user_stop"]) * SR)
    tt = np.arange(n) / SR
    rise = np.sin(2 * np.pi * np.cumsum(220 + 220 * tt / tt[-1]) / SR) * np.linspace(0, 1, n) ** 1.5 * 0.03
    add(dry, rise, C["user_stop"], 1.0)
    add(dry, blip(880, 0.12, 0.06), C["resume"], 1.0, send=0.4)
    add(dry, glide(0.4, 600, 3000, 0.14), C["zero_go2"], 1.0, p=0.15)
    for rep in (C["stop_key"] - 0.1, C["stop_release"] - 0.1):
        for i in range(3):
            add(dry, key_thock(0.4), rep + i * 0.05, 1.0, p=-0.2 + 0.2 * i)
    add(dry, stop_tone(), C["stop_key"], 0.8, send=0.25)
    add(dry, blip(880, 0.12, 0.06), C["stop_release"] + 0.05, 1.0, send=0.4)
    add(dry, glide(0.45, 600, 4000, 0.16), C["zero_go3"], 1.0, p=0.2)
    add(dry, click(1.0, 2700), C["click2"], 0.9, send=0.2)

    # S5 — remember / what changed
    add(dry, whoosh(0.3, 3000, 600, 0.07), C["dialog_out"], 1.0)
    for i in range(3):
        add(dry, tick(0.05, 2400 - 200 * i), C["rows_gone"] + 0.25 + i * 0.05, 1.0, p=-0.3)
    add(dry, whoosh(0.85, 200, 1200, 0.07), C["cam_left3"], 1.0)
    add(dry, reverse_sweep(0.55, 0.2), C["rewind"], 1.0, p=0.4, send=0.35)
    add(dry, thock(0.4), C["w_rem"], 1.0, send=0.15)
    for i in range(11):
        add(dry, blip(988 * 2 ** (-i / 24), 0.04, 0.025), C["rewind"] + 0.45 + i * 0.03, 1.0, p=-0.4, send=0.2)
    for i in range(5):
        add(dry, tick(0.04, 4000), C["rep3_in"] + 0.35 + i * 0.1, 1.0, p=0.5)
    add(dry, pencil(0.3, 0.04), C["delta"], 1.0, p=-0.2)
    add(dry, whoosh(0.6, 250, 1600, 0.08), C["dd_cam"], 1.0)
    add(dry, tick(0.07, 3600), C["call_state2"], 1.0, p=0.6)
    add(dry, data_train(0.25, 40, 0.045, 3400), C["dd_pkt"], 1.0, p=0.2)
    add(dry, blip(1174.66, 0.06, 0.05), C["dd_swerve"], 1.0, p=0.3)   # the swerve
    add(dry, data_train(0.4, 40, 0.04, 3000), C["dd_swerve"], 1.0, p=0.5)
    add(dry, blip(783.99, 0.14, 0.05), C["dd_out"], 1.0, send=0.4)
    add(dry, whoosh(0.55, 250, 1400, 0.07), C["dd_back"], 1.0)
    add(dry, chord_swell([293.66, 369.99, 440.0, 587.33], 2.4, 0.07, 0.04, 0.9), C["done"], 1.0, send=0.6)

    # S6 / S7 — pull back, collapse, the mark
    add(dry, riser(2.2, 80, 1800, 0.16), C["pull"], 1.0, send=0.3)
    add(dry, riser(0.7, 300, 8000, 0.25), C["collapse"], 1.0, send=0.3)
    add(dry, sub_hit(41, 3.2, 1.0, 1.1), C["hit_final"], 1.0, send=0.35)
    add(dry, whoosh(0.5, 1500, 12000, 0.16), C["hit_final"] - 0.03, 1.0, send=0.5)
    add(dry, chord_swell([73.42, 146.83, 220.0, 329.63, 440.0, 659.25], 6.5, 0.2, 0.02, 2.4), C["hit_final"], 1.0, send=0.7)
    add(dry, click(0.6, 3000), C["hit_final"], 1.0, send=0.4)
    add(dry, thock(0.28, 120), C["lock_slide"] + 0.12, 1.0, send=0.2)
    add(dry, sub_hit(55, 1.6, 0.45, 0.5), C["os"], 1.0, send=0.3)
    add(dry, chord_swell([146.83, 220.0, 293.66, 440.0], 4.5, 0.1, 0.02, 1.8), C["os"], 1.0, send=0.6)
    add(dry, tick(0.06, 3000), C["freedoms"], 1.0)
    add(dry, tick(0.06, 3400), C["repo"], 1.0)

    # ---- mix ----
    ir = reverb_ir(2.4, 0.6)
    wet = convolve_stereo(wet_send, ir)[:N] * 0.35
    bed_ir = reverb_ir(1.6, 0.4, seed=5)
    bed_wet = convolve_stereo(bed, bed_ir)[:N] * 0.25
    mix = dry + wet + bed + bed_wet
    # room tone so silence is never digital
    room = lowpass(noise(N), 900) * 0.0025
    mix[:, 0] += room
    mix[:, 1] += np.roll(room, 777)
    # gentle bus glue: soft clip
    mix = np.tanh(mix * 1.1) / 1.1
    # fade the very end
    fe = int((C["duration"] - 0.6) * SR)
    mix[fe:] *= np.linspace(1, 0, N - fe)[:, None] ** 2
    mix = mix[: int(C["duration"] * SR)]
    return mix


def write_wav(path, x):
    x = np.clip(x, -1, 1)
    pcm = (x * 32767).astype("<i2")
    import wave
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
