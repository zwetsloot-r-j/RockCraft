"""M6-H: anchor a Rocksmith+ chart to its own audio.

A Rocksmith+ recording's audio is the game playing the chart (MIDI playback,
independent of the player: a missed note still sounds), so it says exactly when
each note starts.  The visual chart (M6-G) is right about *which* notes there
are but makes three systematic errors in fast passages:

* the white "MISS" text a missed note leaves on the hit line (~0.4 s, tinted in
  the lane colour) is read in place of the short block it follows — the onset
  comes out ~0.37 s late — or as a second note after it;
* a held bar is split in two;
* a short glow at the highway edge reads as a note that isn't there.

:func:`fuse_rsplus_chart` finds every *attack* in the audio (which pitches
start at each onset), then moves each visual note onto the same-pitch attack it
belongs to, drops MISS fragments and notes with no attack behind them, and
merges splits.  When the audio is not the chart (a full mix, or the player
playing along), it falls back to the generic :func:`audio.fuse_chart`.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

import numpy as np

from . import audio
from .schema import ExtractedChart, ExtractedNote

# A visual note is anchored to a same-pitch attack this close to its onset.
ANCHOR_TOL_US = 100_000
# How much later than its attack the MISS text is read as the note.
MISS_LAG_US = (200_000, 500_000)
# A fragment this soon after a short note on the same pitch is its MISS text.
MISS_AFTER_US = (200_000, 600_000)
# A note no longer than this read off the video is a short block (2-4 frames).
SHORT_VISUAL_US = 150_000
# A visual gap this short with no attack in it is one held note split in two.
MERGE_GAP_US = 350_000
# A short note's length: a sixteenth at the editor's default 120 bpm.  The
# audio's releases are too soft to measure under the next notes.
SHORT_NOTE_US = 125_000
# Below this fraction of anchored notes the audio is not the chart.
MIN_ANCHORED_FRACTION = 0.5

# A pitch is attacked when its NNLS coefficient is this share of the onset's
# largest.  Low enough for a note inside a big chord (real piano overtones
# blur it); the extra pitches it admits only matter where the video shows them.
_ATTACK_REL = 0.2
_HARMONICS = 11
_MAX_FREQ_HZ = 5000.0
_ANALYSIS_RATE = 22050
# Where in the flux window an attack is when the flux peaks: this far before
# its end.  Calibrated on synthetic tones in the tests.
_ONSET_TAIL_S = 0.008


@dataclass
class Attack:
    """One pitch starting at one audio onset."""

    time_us: int
    pitch: int
    strength: float


@dataclass
class _Kept:
    note: ExtractedNote
    short: bool
    anchored: bool  # sits on an attack (anchored or re-anchored)
    attack: Optional[Attack] = None


def _mono_at_analysis_rate(samples: np.ndarray, sample_rate: int) -> tuple[np.ndarray, float]:
    x = np.asarray(samples, dtype=np.float32)
    if x.ndim > 1:
        x = x.mean(axis=1)
    factor = max(1, int(round(sample_rate / _ANALYSIS_RATE)))
    if factor > 1:
        n = len(x) // factor * factor
        x = x[:n].reshape(-1, factor).mean(axis=1)
    return x, sample_rate / factor


def _pow2(seconds: float, rate: float) -> int:
    return int(2 ** round(np.log2(max(16.0, seconds * rate))))


def detect_onsets(samples: np.ndarray, sample_rate: int) -> np.ndarray:
    """Onset times (µs): peaks of the broadband spectral flux, ≥ 40 ms apart."""
    x, rate = _mono_at_analysis_rate(samples, sample_rate)
    n = _pow2(0.046, rate)
    hop = max(1, int(round(0.005 * rate)))
    if len(x) < n:
        return np.zeros(0, np.int64)
    nf = 1 + (len(x) - n) // hop
    win = np.hanning(n).astype(np.float32)
    env = np.zeros(nf, np.float32)
    prev = None
    for s in range(0, nf, 4000):
        idx = np.arange(s, min(nf, s + 4000))[:, None] * hop + np.arange(n)[None, :]
        mag = np.log1p(1000.0 * np.abs(np.fft.rfft(x[idx] * win, axis=1)))
        cat = mag if prev is None else np.vstack([prev, mag])
        d = np.maximum(0.0, np.diff(cat, axis=0)).sum(axis=1)
        env[s : s + len(mag)] = d if prev is not None else np.concatenate([[0.0], d])
        prev = mag[-1:]
    k = max(1, int(round(0.3 * rate / hop)))  # 0.3 s detrend
    e = env - np.convolve(env, np.ones(k) / k, mode="same")
    if not np.any(e > 0):
        return np.zeros(0, np.int64)
    thr = 0.08 * float(np.percentile(e, 99.5))
    cand = np.nonzero((e[1:-1] > thr) & (e[1:-1] >= e[:-2]) & (e[1:-1] > e[2:]))[0] + 1
    gap = int(round(0.04 * rate / hop))
    peaks: list[int] = []
    for i in cand:
        if peaks and i - peaks[-1] < gap:
            if e[i] > e[peaks[-1]]:
                peaks[-1] = int(i)
            continue
        peaks.append(int(i))
    # A frame is labelled by its window start.  The log-compressed flux jumps
    # as soon as an attack enters the window, so the attack sits near its end.
    lead = n / rate - _ONSET_TAIL_S
    return np.round((np.asarray(peaks) * hop / rate + lead) * 1e6).astype(np.int64)


def detect_attacks(
    samples: np.ndarray, sample_rate: int, onsets_us: np.ndarray, pitches: range
) -> list[Attack]:
    """Which of ``pitches`` start at each onset.

    The square-root magnitude spectrum just after the onset minus the one just
    before it holds only what started there; NNLS on per-pitch harmonic
    templates splits it into pitches.
    """
    x, rate = _mono_at_analysis_rate(samples, sample_rate)
    n = _pow2(0.093, rate)
    nb = int(_MAX_FREQ_HZ * n / rate)
    win = np.hanning(n)
    pad = int(round(0.005 * rate))

    def spec(start: int) -> np.ndarray:
        seg = x[max(0, start) : max(0, start) + n]
        if len(seg) < n:
            seg = np.pad(seg, (0, n - len(seg)))
        return np.sqrt(np.abs(np.fft.rfft(seg * win))[:nb])

    cols = []
    for p in pitches:
        f0 = _pitch_freq(p)
        t = np.zeros(nb)
        for h in range(1, _HARMONICS + 1):
            k = f0 * h * n / rate
            if k < 1.5 or k >= nb - 2:
                continue
            i = int(round(k))
            t[i - 1 : i + 2] += np.array([0.5, 1.0, 0.5]) / h**0.7
        norm = np.linalg.norm(t)
        cols.append(t / norm if norm > 0 else t)
    w = np.array(cols).T
    gram = w.T @ w
    lip = float(np.linalg.norm(gram, 2)) or 1.0

    out: list[Attack] = []
    for t_us in onsets_us:
        i = int(round(t_us / 1e6 * rate))
        if i - n - pad < 0 or i + pad + n > len(x):
            continue
        b = np.maximum(0.0, spec(i + pad) - spec(i - n - pad))
        wtb = w.T @ b
        c = np.zeros(w.shape[1])
        for _ in range(300):  # projected gradient NNLS
            c = np.maximum(0.0, c - (gram @ c - wtb) / lip)
        top = float(c.max())
        if top <= 0:
            continue
        # Strength is the attack's projection on the pitch's own template: the
        # NNLS shares a low note's energy with its sub-octave.
        for j in np.nonzero(c >= _ATTACK_REL * top)[0]:
            out.append(Attack(int(t_us), pitches[j], float(wtb[j])))
    return out


def _pitch_freq(pitch: int) -> float:
    return 440.0 * 2.0 ** ((pitch - 69) / 12.0)


def _velocity(strength: float, median: float) -> int:
    if strength <= 0 or median <= 0:
        return audio.DEFAULT_VELOCITY
    return int(np.clip(round(80 + 40 * np.log2(strength / median)), 20, 127))


def anchor_notes(
    notes: list[ExtractedNote], attacks: list[Attack]
) -> tuple[list[ExtractedNote], dict[str, int]]:
    """Move visual notes onto their attacks; drop, merge and size them (see module)."""
    by_pitch: dict[int, list[Attack]] = {}
    for a in attacks:
        by_pitch.setdefault(a.pitch, []).append(a)
    claimed: set[int] = set()  # id() of claimed attacks
    stats = dict(anchored=0, miss=0, merged=0, miss_text=0, no_attack=0)
    kept: list[_Kept] = []

    for n in sorted(notes, key=lambda n: (n.pitch, n.start_us)):
        s, e = n.start_us, n.start_us + n.dur_us
        free = [a for a in by_pitch.get(n.pitch, []) if id(a) not in claimed]
        prev = kept[-1] if kept and kept[-1].note.pitch == n.pitch else None

        near = [a for a in free if abs(a.time_us - s) <= ANCHOR_TOL_US]
        if near:
            a = min(near, key=lambda a: abs(a.time_us - s))
            short = n.dur_us <= SHORT_VISUAL_US
            end = a.time_us + SHORT_NOTE_US if short else max(e, a.time_us + 1)
            kept.append(_Kept(_moved(n, a.time_us, end), short, True, a))
            claimed.add(id(a))
            stats["anchored"] += 1
            continue

        early = [a for a in free if MISS_LAG_US[0] <= s - a.time_us <= MISS_LAG_US[1]]
        if early:
            a = early[-1]
            kept.append(_Kept(_moved(n, a.time_us, a.time_us + SHORT_NOTE_US), True, True, a))
            claimed.add(id(a))
            stats["miss"] += 1
            continue

        if (
            prev is not None
            and prev.anchored
            and prev.short
            and MISS_AFTER_US[0] <= s - prev.note.start_us <= MISS_AFTER_US[1]
        ):
            stats["miss_text"] += 1
            continue

        if prev is not None:
            pe = prev.note.start_us + prev.note.dur_us
            between = [
                a
                for a in by_pitch.get(n.pitch, [])
                if prev.note.start_us + ANCHOR_TOL_US < a.time_us < s - ANCHOR_TOL_US
            ]
            if s - pe <= MERGE_GAP_US and not between:
                prev.note.dur_us = max(pe, e) - prev.note.start_us
                prev.short = False
                stats["merged"] += 1
                continue

        stats["no_attack"] += 1

    strengths = [k.attack.strength for k in kept if k.attack is not None]
    median = float(np.median(strengths)) if strengths else 0.0
    for k in kept:
        if k.attack is not None:
            k.note.velocity = _velocity(k.attack.strength, median)

    out = [k.note for k in kept]
    # No note runs into the next one on its pitch.
    for a, b in zip(out, out[1:]):
        if a.pitch == b.pitch and a.start_us + a.dur_us > b.start_us:
            a.dur_us = max(1, b.start_us - a.start_us)
    out.sort(key=lambda n: (n.start_us, n.pitch))
    return out, stats


def refine_clock_offset(notes: list[ExtractedNote], attacks: list[Attack], coarse_us: int) -> int:
    """``coarse_us`` corrected to the median lag of visual onsets to their attacks.

    The onset-train correlation is pulled around by the late (MISS) onsets and
    by chords; the median over notes with a same-pitch attack near them is not.
    """
    by_pitch: dict[int, list[int]] = {}
    for a in attacks:
        by_pitch.setdefault(a.pitch, []).append(a.time_us)
    lags = []
    for n in notes:
        ts = np.asarray(by_pitch.get(n.pitch, []))
        if len(ts):
            d = ts - (n.start_us + coarse_us)
            j = int(np.argmin(np.abs(d)))
            if abs(d[j]) <= ANCHOR_TOL_US:
                lags.append(d[j])
    if len(lags) < 8:
        return coarse_us
    return int(coarse_us + np.median(lags))


def _moved(n: ExtractedNote, start_us: int, end_us: int) -> ExtractedNote:
    return ExtractedNote(
        pitch=n.pitch,
        start_us=int(start_us),
        dur_us=int(max(1, end_us - start_us)),
        hand=n.hand,
        velocity=n.velocity,
        confidence=n.confidence,
    )


def fuse_rsplus_chart(
    chart: ExtractedChart, samples: np.ndarray, sample_rate: int
) -> ExtractedChart:
    """Anchor a Rocksmith+ chart to its audio, or fall back to ``fuse_chart``."""
    suitable, reason = audio.assess_suitability(samples, sample_rate)
    if not suitable or not chart.notes:
        return audio.fuse_chart(chart, samples, sample_rate)

    pitches = range(
        max(0, min(n.pitch for n in chart.notes) - 12),
        min(127, max(n.pitch for n in chart.notes) + 12) + 1,
    )
    attacks = detect_attacks(samples, sample_rate, detect_onsets(samples, sample_rate), pitches)
    coarse = audio.estimate_clock_offset(chart.notes, samples, sample_rate) or 0
    offset = refine_clock_offset(chart.notes, attacks, coarse)
    notes = audio.shift_notes(chart.notes, offset) if offset else list(chart.notes)
    anchored, stats = anchor_notes(notes, attacks)

    on_attack = stats["anchored"] + stats["miss"]
    if on_attack < MIN_ANCHORED_FRACTION * len(notes):
        fused = audio.fuse_chart(chart, samples, sample_rate)
        fused.source.audio_fusion = (
            f"rsplus anchoring skipped: only {on_attack}/{len(notes)} notes have an "
            f"attack in the audio; {fused.source.audio_fusion}"
        )
        return fused

    if offset:
        chart.source.clock_offset_us = int(offset)
    clock = f"clock {offset / 1000:+.0f} ms" if offset else "clock unchanged"
    chart.source.audio_fusion = (
        f"applied (rsplus anchoring): {reason}; {clock}; {stats['anchored']} anchored, "
        f"{stats['miss']} moved back from MISS text, {stats['merged']} merged, "
        f"{stats['miss_text']} MISS fragments and {stats['no_attack']} notes with no "
        f"attack dropped -> {len(anchored)} notes"
    )
    return ExtractedChart(notes=anchored, source=chart.source)
