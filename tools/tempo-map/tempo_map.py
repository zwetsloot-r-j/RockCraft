#!/usr/bin/env python3
"""Tempo-map detector — CLI (RockCraft M16-A).

Turns a piece's backing audio into a per-bar tempo map: the file-time of every
bar's downbeat, following a performance whose tempo breathes (rubato, a
hand-played cover). numpy + stdlib only; a local WAV in, JSON out.

Usage:
    python tempo_map.py --in <audio.wav> --out <map.json|-> [--beats-per-bar N]
                        [--anchor-us US] [--tempo-hint BPM]
                        [--min-bpm 40] [--max-bpm 220]

Output (times are microseconds into the audio FILE):
    {"version": 1, "beats_us": [...], "bars_us": [...], "bpm": 86.1,
     "pulse_bpm": 172.3, "anchor_us": 3070000}

Method: a spectral-flux onset envelope, an autocorrelation tempo estimate with a
log-Gaussian prior, and Ellis' (2007) dynamic-programming beat tracker — the
same recipe as librosa's ``beat_track``, re-implemented so the sidecar needs no
audio dependency. The tracked pulse is then folded to quarter-note beats (a
fast eighth-note pulse with alternating accents counts two pulses per beat) and
grouped into bars from an anchor downbeat.
"""

from __future__ import annotations

import argparse
import json
import sys
import wave

import numpy as np

TARGET_SR = 11025
N_FFT = 1024
HOP = 64  # ~5.8 ms at 11025 Hz
BASS_HZ = 250.0
# Spectral flux peaks as an attack enters the analysis window, ahead of the
# centred frame time. Calibrated on synthetic clicks (tests/): ~30 ms at
# N_FFT=1024 / 11025 Hz.
ONSET_LAG_S = 0.0305
TIGHTNESS = 100.0
PRIOR_BPM = 120.0
PRIOR_STD_OCT = 1.0


# ── audio I/O ────────────────────────────────────────────────────────────────


def read_wav(path: str) -> tuple[np.ndarray, int]:
    """Read a PCM WAV as mono float32 in [-1, 1]; returns (samples, rate)."""
    with wave.open(path, "rb") as w:
        ch, width, sr, n = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
        raw = w.readframes(n)
    if width == 1:
        x = (np.frombuffer(raw, np.uint8).astype(np.float32) - 128.0) / 128.0
    elif width == 2:
        x = np.frombuffer(raw, "<i2").astype(np.float32) / 32768.0
    elif width == 3:
        b = np.frombuffer(raw, np.uint8).reshape(-1, 3).astype(np.int32)
        v = b[:, 0] | (b[:, 1] << 8) | (b[:, 2] << 16)
        v = np.where(v >= 1 << 23, v - (1 << 24), v)
        x = v.astype(np.float32) / float(1 << 23)
    elif width == 4:
        x = np.frombuffer(raw, "<i4").astype(np.float32) / float(1 << 31)
    else:
        raise ValueError(f"unsupported WAV sample width: {width} bytes")
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    return x, sr


def downsample(x: np.ndarray, sr: int) -> tuple[np.ndarray, float]:
    """Boxcar-decimate towards TARGET_SR (plenty for onset detection)."""
    factor = max(1, int(round(sr / TARGET_SR)))
    if factor == 1:
        return x, float(sr)
    n = len(x) // factor * factor
    return x[:n].reshape(-1, factor).mean(axis=1), sr / factor


# ── onset envelope ───────────────────────────────────────────────────────────


def onset_envelopes(x: np.ndarray, sr: float) -> tuple[np.ndarray, np.ndarray, float]:
    """Spectral-flux onset strength (full band and bass band) per HOP frame.

    Returns (envelope, bass_envelope, frame_rate). Log-compressed magnitudes are
    pooled into log-spaced bands so a loud bass note can't drown the treble, then
    the half-wave-rectified frame-to-frame rise is averaged over bands.
    """
    if len(x) < N_FFT:
        x = np.pad(x, (0, N_FFT - len(x)))
    x = np.pad(x, (N_FFT // 2, N_FFT // 2))
    n_frames = 1 + (len(x) - N_FFT) // HOP
    win = np.hanning(N_FFT).astype(np.float32)
    freqs = np.fft.rfftfreq(N_FFT, 1.0 / sr)
    # ~40 log-spaced bands from 30 Hz to Nyquist.
    edges = np.geomspace(30.0, sr / 2, 41)
    band = np.clip(np.searchsorted(edges, freqs) - 1, -1, 39)
    valid = band >= 0
    bass_bands = np.unique(band[valid & (freqs < BASS_HZ)])
    onehot = np.zeros((len(freqs), 40), np.float32)
    onehot[np.nonzero(valid)[0], band[valid]] = 1.0
    counts = np.maximum(onehot.sum(axis=0), 1.0)

    spec = np.empty((n_frames, 40), np.float32)
    chunk = 4096
    idx = np.arange(N_FFT)[None, :]
    for s in range(0, n_frames, chunk):
        f = np.arange(s, min(n_frames, s + chunk))[:, None] * HOP + idx
        mag = np.abs(np.fft.rfft(x[f] * win, axis=1)).astype(np.float32)
        spec[s : s + len(f)] = (mag @ onehot) / counts
    spec = np.log1p(1000.0 * spec)
    flux = np.maximum(0.0, np.diff(spec, axis=0, prepend=spec[:1]))
    env = flux.mean(axis=1)
    bass = flux[:, bass_bands].mean(axis=1) if len(bass_bands) else env.copy()
    return _normalise(env), _normalise(bass), sr / HOP


def _normalise(e: np.ndarray) -> np.ndarray:
    # Remove slow loudness drift (1 s moving average), then unit-std.
    k = 173
    if len(e) > k:
        trend = np.convolve(e, np.ones(k) / k, mode="same")
        e = np.maximum(0.0, e - trend)
    sd = e.std()
    return e / sd if sd > 0 else e


# ── tempo + beat tracking ────────────────────────────────────────────────────


def estimate_period(env: np.ndarray, fr: float, min_bpm: float, max_bpm: float) -> float:
    """Pulse period in frames: autocorrelation peak under a log-Gaussian prior."""
    n = len(env)
    lo = max(1, int(np.floor(60.0 * fr / max_bpm)))
    hi = min(n - 1, int(np.ceil(60.0 * fr / min_bpm)))
    if hi <= lo + 1:
        return 60.0 * fr / PRIOR_BPM
    size = 1 << int(np.ceil(np.log2(2 * n)))
    f = np.fft.rfft(env - env.mean(), size)
    ac = np.fft.irfft(f * np.conj(f), size)[: hi + 2]
    lags = np.arange(len(ac), dtype=float)
    lags[0] = 1.0
    bpm = 60.0 * fr / lags
    prior = np.exp(-0.5 * (np.log2(bpm / PRIOR_BPM) / PRIOR_STD_OCT) ** 2)
    score = ac * prior
    k = lo + int(np.argmax(score[lo : hi + 1]))
    if lo < k < hi:  # parabolic refinement
        a, b, c = score[k - 1], score[k], score[k + 1]
        d = a - 2 * b + c
        if d < 0:
            return k + 0.5 * (a - c) / d
    return float(k)


def track_beats(env: np.ndarray, period: float) -> np.ndarray:
    """Ellis DP beat tracker: frame indices of the pulse, following tempo drift."""
    n = len(env)
    w = np.arange(-period, period + 1)
    local = np.convolve(env, np.exp(-0.5 * (w * 32.0 / period) ** 2), mode="same")
    cum = np.zeros(n)
    back = np.full(n, -1, dtype=np.int64)
    lo_off, hi_off = int(round(2 * period)), int(round(period / 2))
    offs = np.arange(lo_off, hi_off - 1, -1)  # candidate distances to the previous beat
    penalty = -TIGHTNESS * np.log(offs / period) ** 2
    for i in range(n):
        prev = i - offs
        ok = prev >= 0
        if not ok.any():
            cum[i] = local[i]
            continue
        cand = cum[prev[ok]] + penalty[ok]
        j = int(np.argmax(cand))
        cum[i] = local[i] + cand[j]
        back[i] = prev[ok][j]
    # Last beat: the final local maximum of cum above half its median peak.
    peaks = np.nonzero((cum[1:-1] > cum[:-2]) & (cum[1:-1] >= cum[2:]))[0] + 1
    if len(peaks) == 0:
        return np.array([], dtype=np.int64)
    good = peaks[cum[peaks] >= 0.5 * np.median(cum[peaks])]
    i = int(good[-1] if len(good) else peaks[-1])
    beats = []
    while i >= 0:
        beats.append(i)
        i = int(back[i])
    beats = np.array(beats[::-1], dtype=np.int64)
    # Trim weak beats at both ends (silence before/after the music).
    strength = local[beats]
    cut = 0.5 * np.sqrt(np.mean(strength**2))
    keep = np.nonzero(strength >= cut)[0]
    if len(keep):
        beats = beats[keep[0] : keep[-1] + 1]
    return beats


def _strength_at(env: np.ndarray, frames: np.ndarray, rad: int = 3) -> np.ndarray:
    return np.array([env[max(0, f - rad) : f + rad + 1].max() for f in frames])


def pulses_per_beat_of(pulses: np.ndarray, env: np.ndarray, fr: float) -> int:
    """2 when the tracked pulse is a fast eighth-note stream with alternating
    accents, else 1.

    The accent contrast is judged in short windows: the tracker can slip a pulse
    mid-song, flipping which parity carries the accent, so a global even/odd
    average would cancel out exactly when the alternation is real.
    """
    if len(pulses) < 8:
        return 1
    rate = 60.0 * fr / float(np.median(np.diff(pulses)))
    if rate <= 140.0:
        return 1
    s = _strength_at(env, pulses)
    w = 16
    logs = [
        abs(np.log(max(1e-9, s[k : k + w : 2].mean()) / max(1e-9, s[k + 1 : k + w : 2].mean())))
        for k in range(0, len(s) - w + 1, w)
    ]
    return 2 if logs and float(np.median(logs)) >= np.log(1.10) else 1


def group_bars(
    beats_us: np.ndarray, bass_strength: np.ndarray, bpb: int, anchor_us: int | None
) -> tuple[list[int], int]:
    """Downbeat times (µs) from beats, grouped by ``bpb`` from an anchor beat.

    Returns (bars_us, anchor_us). One extrapolated bar end is appended so the
    last full bar has a duration.
    """
    n = len(beats_us)
    if n == 0:
        return [], 0
    if anchor_us is not None:
        a = int(np.argmin(np.abs(beats_us - anchor_us)))
    else:
        phase = max(range(min(bpb, n)), key=lambda p: bass_strength[p::bpb].mean())
        a = phase
    first = a % bpb
    idx = list(range(first, n, bpb))
    ibi = float(np.median(np.diff(beats_us[-min(n, bpb + 1) :]))) if n > 1 else 0.0
    bars = [int(beats_us[i]) for i in idx]
    last = idx[-1] + bpb
    if n > 1:
        bars.append(int(beats_us[-1] + (last - (n - 1)) * ibi) if last > n - 1 else int(beats_us[last]))
    return bars, int(beats_us[a])


def detect(
    x: np.ndarray,
    sr: int,
    beats_per_bar: int = 4,
    anchor_us: int | None = None,
    tempo_hint: float | None = None,
    min_bpm: float = 40.0,
    max_bpm: float = 220.0,
) -> dict:
    """Full pipeline on mono samples; returns the output JSON object."""
    y, fsr = downsample(np.asarray(x, np.float32), sr)
    env, bass, fr = onset_envelopes(y, fsr)
    period = estimate_period(env, fr, min_bpm, max_bpm)
    pulses = track_beats(env, period)
    pulse_bpm = 60.0 * fr / period
    if tempo_hint:
        # The caller knows the beat tempo: scale the pulse to the nearest octave.
        factor = min((0.5, 1.0, 2.0), key=lambda f: abs(np.log(pulse_bpm / f / tempo_hint)))
    else:
        factor = float(pulses_per_beat_of(pulses, env, fr))
    # Re-track at the beat period rather than decimating the pulse: the DP then
    # follows whichever pulse stream carries the accents, even across a slip.
    beats = pulses if factor == 1.0 else track_beats(env, period * factor)
    # Frame -> µs, compensating the flux's lead over the true attack.
    beats_us = np.round((beats / fr + ONSET_LAG_S) * 1e6).astype(np.int64)
    bars, anchor = group_bars(beats_us, _strength_at(bass, beats), beats_per_bar, anchor_us)
    bpm = 60e6 / float(np.median(np.diff(beats_us))) if len(beats_us) > 1 else 0.0
    return {
        "version": 1,
        "beats_us": [int(b) for b in beats_us],
        "bars_us": bars,
        "bpm": round(bpm, 2),
        "pulse_bpm": round(pulse_bpm, 2),
        "anchor_us": anchor,
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--in", dest="inp", required=True, help="input WAV file")
    ap.add_argument("--out", required=True, help="output JSON path, or - for stdout")
    ap.add_argument("--beats-per-bar", type=int, default=4)
    ap.add_argument("--anchor-us", type=int, default=None, help="a downbeat (file µs)")
    ap.add_argument("--tempo-hint", type=float, default=None, help="expected beat BPM")
    ap.add_argument("--min-bpm", type=float, default=40.0)
    ap.add_argument("--max-bpm", type=float, default=220.0)
    a = ap.parse_args(argv)
    if a.beats_per_bar < 1:
        ap.error("--beats-per-bar must be >= 1")
    x, sr = read_wav(a.inp)
    out = detect(x, sr, a.beats_per_bar, a.anchor_us, a.tempo_hint, a.min_bpm, a.max_bpm)
    print(
        f"tempo-map: {len(out['beats_us'])} beats, {len(out['bars_us'])} bar lines, "
        f"~{out['bpm']} BPM (tracked pulse {out['pulse_bpm']}/min)",
        file=sys.stderr,
    )
    text = json.dumps(out)
    if a.out == "-":
        sys.stdout.write(text)
    else:
        with open(a.out, "w") as f:
            f.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
