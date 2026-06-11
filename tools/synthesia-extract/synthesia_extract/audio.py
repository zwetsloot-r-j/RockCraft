"""M6-F audio-fusion: enrich a visual chart with velocity + tighter timing.

The visual extractor (M6-C) recovers pitch / timing / hand but **no dynamics**.
When the video's audio is *clean piano* (a MIDI render or solo-piano take), an
audio pass recovers what the picture cannot:

* **velocity** — from each note's loudness, and
* **sub-frame onset/offset** — audio is sample-precise, where the visual roll is
  quantised to the video frame grid.

Pipeline (all optional, behind ``--audio-fusion``; M6-C alone is unaffected):

1. :func:`assess_suitability` — is this isolated piano, or a full mix? A full
   band mix has a broadband, *flat* spectrum that cannot be cleanly transcribed
   to the piano part, so we **skip** rather than corrupt the visual notes.
2. :func:`transcribe` — a self-contained classical-DSP transcriber: per visual
   pitch, FFT-bandpass the fundamental, take the amplitude envelope, and read
   off onsets/offsets/velocity.  No ML weights, so it stays hermetic in CI.  A
   heavier model (Onsets-and-Frames, ``basic-pitch``) can be dropped in behind
   the same :class:`TranscribedNote` interface — see the README.
3. :func:`fuse` — match each visual note to a transcribed event (same pitch,
   onset within the visual's frame uncertainty), copy ``velocity``, and nudge
   ``start_us`` / ``dur_us`` toward the audio estimate **without** moving past
   that uncertainty.  Visual **pitch stays ground truth**; audio never invents
   notes the picture didn't show.

:func:`fuse_chart` ties the three together and stamps ``source.audio_fusion``.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Sequence

import numpy as np

from .schema import ExtractedChart, ExtractedNote

# Velocity below which a detected run is treated as silence/noise, and the
# default velocity stamped on visual notes that fusion could not match.
DEFAULT_VELOCITY = 64
_MIN_VELOCITY = 1
_MAX_VELOCITY = 127

# Loudness->velocity calibration: the peak amplitude an audio source uses for a
# velocity-127 note.  Below 1.0 so several simultaneous notes (a chord) sum
# without clipping the [-1, 1] PCM range.  The synthetic renderer scales tones
# to this reference, and the transcriber inverts it; a real model (basic-pitch /
# Onsets-and-Frames) emits absolute velocity directly and ignores this.
REFERENCE_PEAK_AMPLITUDE = 0.25

# Spectral flatness above this -> the audio is broadband (full mix), not clean
# piano.  Clean tonal audio sits near zero; white-ish noise approaches one.
SUITABILITY_FLATNESS_MAX = 0.10

# The sliding-RMS envelope peak overshoots a tone's true amplitude by a small,
# bin-alignment-dependent factor (window edge + spectral leakage).  Dividing the
# measured peak by this calibration centres the velocity estimate.  A real
# transcriber (basic-pitch / Onsets-and-Frames) reports absolute velocity and
# would set this to 1.0.
_ENVELOPE_PEAK_CAL = 1.03


# Transcription works on fundamentals (piano tops out ~4.2 kHz), so anything
# above this rate just inflates the per-pitch FFT cost ~linearly.
_TRANSCRIBE_MAX_SAMPLE_RATE = 16_000


def _decimate_for_transcription(
    samples: np.ndarray, sample_rate: int
) -> tuple[np.ndarray, int]:
    """Integer-factor downsample to <= 16 kHz with a windowed-sinc low-pass."""
    factor = int(np.ceil(sample_rate / _TRANSCRIBE_MAX_SAMPLE_RATE))
    if factor <= 1:
        return samples, sample_rate
    t = np.arange(-15, 16, dtype=np.float64)
    cutoff = 0.5 / factor  # new Nyquist, in cycles per (old) sample
    taps = 2.0 * cutoff * np.sinc(2.0 * cutoff * t) * np.hamming(t.size)
    taps /= taps.sum()
    filtered = np.convolve(np.asarray(samples, dtype=np.float64), taps, mode="same")
    return filtered[::factor], sample_rate // factor


@dataclass
class TranscribedNote:
    """One note recovered from audio: pitch, sample-precise timing, velocity."""

    pitch: int
    start_us: int
    dur_us: int
    velocity: int
    confidence: float = 1.0


# --------------------------------------------------------------------------- #
# Suitability: clean piano vs. full mix
# --------------------------------------------------------------------------- #
def spectral_flatness(samples: np.ndarray) -> float:
    """Spectral flatness (geometric mean / arithmetic mean of the power spectrum).

    Near 0 for tonal, peaky signals (a few sustained piano tones); approaches 1
    for broadband noise.  Computed over the magnitude spectrum excluding DC.
    """
    x = np.asarray(samples, dtype=np.float64)
    if x.size == 0 or not np.any(x):
        return 0.0
    power = np.abs(np.fft.rfft(x)) ** 2
    power = power[1:]  # drop DC
    if power.size == 0 or power.sum() <= 0:
        return 0.0
    eps = 1e-12
    geo = np.exp(np.mean(np.log(power + eps)))
    arith = np.mean(power) + eps
    return float(geo / arith)


def assess_suitability(
    samples: np.ndarray, sample_rate: int
) -> tuple[bool, str]:
    """Decide whether ``samples`` is clean enough piano to fuse.

    Returns ``(is_suitable, reason)``.  The reason is recorded in
    ``SourceMeta.audio_fusion`` either way so a reviewer can see what happened.
    """
    x = np.asarray(samples, dtype=np.float64)
    if x.size == 0 or not np.any(x):
        return False, "no audio signal"
    flatness = spectral_flatness(x)
    if flatness > SUITABILITY_FLATNESS_MAX:
        return (
            False,
            f"audio looks like a full mix (spectral flatness "
            f"{flatness:.3f} > {SUITABILITY_FLATNESS_MAX:.2f})",
        )
    return True, f"clean piano (spectral flatness {flatness:.3f})"


# --------------------------------------------------------------------------- #
# Transcription: per-pitch FFT-bandpass envelope -> onsets/velocity
# --------------------------------------------------------------------------- #
def _bandpass_envelope(
    spectrum: np.ndarray,
    freqs: np.ndarray,
    n: int,
    sample_rate: int,
    f0: float,
) -> np.ndarray:
    """Amplitude envelope of the signal isolated to a semitone-wide band at ``f0``.

    Keeps only the fundamental's bins (a half-semitone either side), inverts back
    to the time domain, and converts to an amplitude envelope via a short sliding
    RMS (``amplitude = sqrt(2) * rms`` for a sinusoid).  For a tone rendered by
    :func:`synthesia_extract.synth.render_audio`, the envelope peak equals the
    note's velocity gain.
    """
    lo = f0 * 2.0 ** (-0.5 / 12.0)
    hi = f0 * 2.0 ** (0.5 / 12.0)
    mask = (freqs >= lo) & (freqs <= hi)
    if not mask.any():
        return np.zeros(n, dtype=np.float64)
    band = np.where(mask, spectrum, 0.0)
    y = np.fft.irfft(band, n=n)
    # Sliding RMS over ~3 fundamental periods (>= a small floor) -> envelope.
    win = max(8, int(round(3.0 * sample_rate / max(f0, 1.0))))
    kernel = np.ones(win, dtype=np.float64) / win
    mean_sq = np.convolve(y * y, kernel, mode="same")
    return np.sqrt(2.0 * np.maximum(mean_sq, 0.0))


def _runs_above(env: np.ndarray, on: float, off: float) -> list[tuple[int, int]]:
    """Hysteresis threshold: runs where ``env`` rises past ``on`` until it falls
    back below ``off``.  Returns (start, end_exclusive) sample indices."""
    runs: list[tuple[int, int]] = []
    active = False
    start = 0
    for i, v in enumerate(env):
        if not active and v >= on:
            active = True
            start = i
        elif active and v < off:
            active = False
            runs.append((start, i))
    if active:
        runs.append((start, len(env)))
    return runs


def _strike_onsets(
    seg: np.ndarray,
    sample_rate: int,
    *,
    min_gap_s: float = 0.08,
    rise_ratio: float = 1.3,
    min_rise: float = 0.02,
) -> list[int]:
    """Indices (into ``seg``) where a key strike begins inside one envelope run.

    A struck piano note decays monotonically, so a significant *rise* off the
    local decay floor mid-run — at least ``rise_ratio`` times the running
    minimum, and ``min_rise`` in absolute terms — is a re-strike of the same
    pitch, even when pedal sustain keeps the envelope from ever dipping below
    the hysteresis floor between strikes (which would otherwise merge repeats
    into one long run).  The run's own attack is always the first onset; the
    relative threshold keeps string-beating wobble (a few percent) from
    triggering.
    """
    min_gap = max(1, int(round(min_gap_s * sample_rate)))
    onsets = [0]
    last_onset = 0
    # Two-state walk: ride each strike's attack up to its peak, then track the
    # decay's running minimum; a qualifying rise off that minimum is the next
    # strike. (Tracking minima from the run's start would anchor at the attack
    # base and never see the inter-strike floor.)
    in_attack = True
    cur_peak = float(seg[0])
    local_min = float("inf")
    local_min_i = 0
    for i in range(1, seg.size):
        v = float(seg[i])
        if in_attack:
            cur_peak = max(cur_peak, v)
            if v <= 0.85 * cur_peak:  # decay has begun
                in_attack = False
                local_min = v
                local_min_i = i
        elif v < local_min:
            local_min = v
            local_min_i = i
        elif (
            v >= rise_ratio * local_min
            and v - local_min >= min_rise
            and local_min_i >= last_onset + min_gap
        ):
            onsets.append(local_min_i)  # the strike begins where the rise does
            last_onset = local_min_i
            in_attack = True
            cur_peak = v
    return onsets


def transcribe(
    samples: np.ndarray,
    sample_rate: int,
    candidate_pitches: Sequence[int],
    *,
    on_threshold: float = 0.02,
    off_threshold: float = 0.012,
    onset_fraction: float = 0.5,
    min_dur_us: int = 50_000,
) -> list[TranscribedNote]:
    """Recover notes for ``candidate_pitches`` from a clean-piano ``samples`` track.

    Restricting transcription to the pitches the visual already found keeps it
    focused and robust (we are *enriching* a known chart, not blind-transcribing)
    and sidesteps octave/harmonic confusion.  Returns sample-precise notes with
    velocity inferred from the envelope peak.

    A note's coarse extent is the run where the envelope is above the hysteresis
    band, further segmented at mid-run re-strikes (:func:`_strike_onsets`); each
    segment's **onset/offset are then snapped to the sharp edges** where the
    envelope crosses ``onset_fraction`` of the segment's peak.  This rejects the
    zero-phase band-pass's low-amplitude pre-ringing, which would otherwise pull
    onsets tens of milliseconds early.
    """
    x = np.asarray(samples, dtype=np.float64)
    n = x.size
    if n == 0:
        return []
    spectrum = np.fft.rfft(x)
    freqs = np.fft.rfftfreq(n, d=1.0 / sample_rate)

    out: list[TranscribedNote] = []
    for pitch in sorted(set(candidate_pitches)):
        f0 = 440.0 * 2.0 ** ((pitch - 69) / 12.0)
        env = _bandpass_envelope(spectrum, freqs, n, sample_rate, f0)
        for a, b in _runs_above(env, on_threshold, off_threshold):
            seg = env[a:b]
            bounds = _strike_onsets(seg, sample_rate)
            bounds.append(seg.size)
            for s0, s1 in zip(bounds, bounds[1:]):
                sub = seg[s0:s1]
                if sub.size == 0:
                    continue
                peak = float(sub.max())
                above = np.nonzero(sub >= onset_fraction * peak)[0]
                if above.size == 0:
                    continue
                on_i = a + s0 + int(above[0])
                off_i = a + s0 + int(above[-1]) + 1
                start_us = int(round(on_i / sample_rate * 1e6))
                dur_us = int(round((off_i - on_i) / sample_rate * 1e6))
                if dur_us < min_dur_us:
                    continue
                amplitude = peak / _ENVELOPE_PEAK_CAL / REFERENCE_PEAK_AMPLITUDE
                velocity = int(
                    np.clip(round(amplitude * 127.0), _MIN_VELOCITY, _MAX_VELOCITY)
                )
                out.append(
                    TranscribedNote(
                        pitch=pitch,
                        start_us=start_us,
                        dur_us=max(1, dur_us),
                        velocity=velocity,
                    )
                )
    out.sort(key=lambda t: (t.start_us, t.pitch))
    return out


# --------------------------------------------------------------------------- #
# Optional learned transcriber (basic-pitch) behind the same interface
# --------------------------------------------------------------------------- #
def transcribe_learned(audio_path: str) -> Optional[list[TranscribedNote]]:
    """Transcribe with basic-pitch when it is installed; ``None`` otherwise.

    A learned model hears through pedal sustain and dense polyphony far better
    than the classical band-pass transcriber, which directly improves the
    fusion pass's merged-note splitting and velocities.  It is a heavy
    *optional* dependency (not in ``requirements.txt`` — see the README's
    transcription-dependency note); without it, callers fall back to
    :func:`transcribe` and everything still works.
    """
    try:
        from basic_pitch import ICASSP_2022_MODEL_PATH
        from basic_pitch.inference import predict
    except Exception:
        return None
    try:
        _output, _midi, note_events = predict(str(audio_path), ICASSP_2022_MODEL_PATH)
    except Exception:
        return None
    out = [
        TranscribedNote(
            pitch=int(p),
            start_us=int(round(s * 1e6)),
            dur_us=max(1, int(round((e - s) * 1e6))),
            velocity=int(np.clip(round(a * 127.0), _MIN_VELOCITY, _MAX_VELOCITY)),
        )
        for (s, e, p, a, _bends) in note_events
    ]
    out.sort(key=lambda t: (t.start_us, t.pitch))
    return out


# --------------------------------------------------------------------------- #
# Clock alignment + merged-note splitting
# --------------------------------------------------------------------------- #
def estimate_global_offset(
    notes: list[ExtractedNote],
    transcribed: list[TranscribedNote],
    *,
    search_us: int = 250_000,
    min_matches: int = 30,
) -> int:
    """Median audio-minus-visual onset offset over same-pitch nearest matches.

    The visual clock systematically leads or lags the audio clock by a small,
    per-video constant (hit-line placement vs. the actual key strike, encoder
    delay).  The median over many matches is robust to the transcriber's
    occasional false events; with too few matches we return 0 rather than
    trust a noisy estimate.
    """
    by_pitch: dict[int, list[int]] = {}
    for t in transcribed:
        by_pitch.setdefault(t.pitch, []).append(t.start_us)
    deltas: list[int] = []
    for n in notes:
        starts = by_pitch.get(n.pitch)
        if not starts:
            continue
        d = min((s - n.start_us for s in starts), key=abs)
        if abs(d) <= search_us:
            deltas.append(d)
    if len(deltas) < min_matches:
        return 0
    return int(np.median(deltas))


# A lower note's harmonics land exactly in a higher pitch's band (2nd harmonic
# = +12 semitones, 3rd = +19, 4th = +24), so an onset there may be bleed, not a
# re-strike. Bleed is much weaker than a struck fundamental; the velocity ratio
# separates the two.
_HARMONIC_INTERVALS = (12, 19, 24)
_HARMONIC_VETO_RATIO = 0.6
_HARMONIC_VETO_WINDOW_US = 50_000


def split_merged_notes(
    notes: list[ExtractedNote],
    transcribed: list[TranscribedNote],
    *,
    min_segment_us: int = 80_000,
) -> tuple[list[ExtractedNote], int]:
    """Split visual notes that hold through an audible re-strike of their pitch.

    Back-to-back bars touch on screen (the gap between repeated notes blurs
    away at decimated frame rates), so the roll reads them as one long note.
    The audio still carries each strike: a transcribed onset of the *same
    pitch* well inside a visual note's span marks where to cut.  Audio only
    segments here — pitch and hand stay visual ground truth, and no notes are
    invented where the picture shows nothing.

    Onsets within ``min_segment_us`` of a segment boundary are ignored (they
    are timing jitter, not a distinct note), as are onsets explainable as a
    lower note's harmonic bleeding into this pitch's band.
    """
    by_pitch: dict[int, list[TranscribedNote]] = {}
    for t in transcribed:
        by_pitch.setdefault(t.pitch, []).append(t)

    def is_harmonic_bleed(t: TranscribedNote) -> bool:
        for interval in _HARMONIC_INTERVALS:
            for lower in by_pitch.get(t.pitch - interval, ()):
                if (
                    abs(lower.start_us - t.start_us) <= _HARMONIC_VETO_WINDOW_US
                    and t.velocity < _HARMONIC_VETO_RATIO * lower.velocity
                ):
                    return True
        return False

    out: list[ExtractedNote] = []
    n_split = 0
    for note in notes:
        end_us = note.start_us + note.dur_us
        cuts: list[int] = []
        last_bound = note.start_us
        for t in sorted(by_pitch.get(note.pitch, ()), key=lambda t: t.start_us):
            if (
                t.start_us >= last_bound + min_segment_us
                and t.start_us <= end_us - min_segment_us
                and not is_harmonic_bleed(t)
            ):
                cuts.append(t.start_us)
                last_bound = t.start_us
        if not cuts:
            out.append(note)
            continue
        n_split += 1
        bounds = [note.start_us, *cuts, end_us]
        for a, b in zip(bounds, bounds[1:]):
            out.append(
                ExtractedNote(
                    pitch=note.pitch,
                    start_us=a,
                    dur_us=b - a,
                    hand=note.hand,
                    velocity=note.velocity,
                    confidence=note.confidence,
                )
            )
    out.sort(key=lambda n: (n.start_us, n.pitch))
    return out, n_split


# --------------------------------------------------------------------------- #
# Fusion
# --------------------------------------------------------------------------- #
def _clamp_toward(value: int, target: int, max_shift: int) -> int:
    """Move ``value`` toward ``target`` by at most ``max_shift`` (never past it)."""
    delta = target - value
    if delta > max_shift:
        delta = max_shift
    elif delta < -max_shift:
        delta = -max_shift
    return value + delta


def fuse(
    notes: list[ExtractedNote],
    transcribed: list[TranscribedNote],
    *,
    frame_uncertainty_us: int,
    match_tol_us: int,
) -> list[ExtractedNote]:
    """Fuse audio detections into visual notes.

    For each visual note, find the nearest transcribed event of the **same
    pitch** whose onset is within ``match_tol_us``.  On a match: copy velocity
    and nudge ``start_us`` / end toward the audio estimate, clamped to
    ``frame_uncertainty_us`` so we never move beyond the visual's frame
    ambiguity.  Unmatched visual notes keep their timing and take
    :data:`DEFAULT_VELOCITY`.  Pitch and hand are never touched.
    """
    fused, _matched = _fuse_with_stats(
        notes,
        transcribed,
        frame_uncertainty_us=frame_uncertainty_us,
        match_tol_us=match_tol_us,
    )
    return fused


def _fuse_with_stats(
    notes: list[ExtractedNote],
    transcribed: list[TranscribedNote],
    *,
    frame_uncertainty_us: int,
    match_tol_us: int,
) -> tuple[list[ExtractedNote], int]:
    """:func:`fuse`, also returning how many visual notes matched an audio event."""
    by_pitch: dict[int, list[TranscribedNote]] = {}
    for t in transcribed:
        by_pitch.setdefault(t.pitch, []).append(t)

    used: set[int] = set()  # id() of consumed transcription events
    matched = 0
    fused: list[ExtractedNote] = []
    for note in notes:
        cands = [
            t
            for t in by_pitch.get(note.pitch, [])
            if id(t) not in used and abs(t.start_us - note.start_us) <= match_tol_us
        ]
        if not cands:
            fused.append(
                ExtractedNote(
                    pitch=note.pitch,
                    start_us=note.start_us,
                    dur_us=note.dur_us,
                    hand=note.hand,
                    velocity=DEFAULT_VELOCITY,
                    confidence=note.confidence,
                )
            )
            continue

        best = min(cands, key=lambda t: abs(t.start_us - note.start_us))
        used.add(id(best))
        matched += 1
        new_start = _clamp_toward(note.start_us, best.start_us, frame_uncertainty_us)
        visual_end = note.start_us + note.dur_us
        audio_end = best.start_us + best.dur_us
        new_end = _clamp_toward(visual_end, audio_end, frame_uncertainty_us)
        new_dur = max(1, new_end - new_start)
        # Audio match reinforces the detection: lift confidence toward 1.
        base_conf = note.confidence if note.confidence is not None else 0.5
        fused.append(
            ExtractedNote(
                pitch=note.pitch,
                start_us=new_start,
                dur_us=new_dur,
                hand=note.hand,
                velocity=best.velocity,
                confidence=round(min(1.0, base_conf + (1.0 - base_conf) * 0.5), 3),
            )
        )

    fused.sort(key=lambda n: (n.start_us, n.pitch))
    return fused, matched


def fuse_chart(
    chart: ExtractedChart,
    samples: np.ndarray,
    sample_rate: int,
    *,
    audio_path: Optional[str] = None,
    frame_uncertainty_us: Optional[int] = None,
    match_tol_us: Optional[int] = None,
) -> ExtractedChart:
    """Orchestrate suitability -> transcribe -> fuse, stamping the diagnostic.

    On unsuitable audio (a full mix) the visual ``chart`` is returned **unchanged**
    except for ``source.audio_fusion`` explaining the skip.  Defaults for the
    timing windows derive from the chart's fps (one frame of uncertainty, a
    two-frame match window) so callers usually pass only the audio.

    When ``audio_path`` is given *and* basic-pitch is installed, transcription
    uses the learned model (:func:`transcribe_learned`); otherwise the bundled
    DSP transcriber.  The suitability gate applies either way — fusion never
    runs on full-mix audio.
    """
    samples, sample_rate = _decimate_for_transcription(samples, sample_rate)
    suitable, reason = assess_suitability(samples, sample_rate)
    if not suitable:
        chart.source.audio_fusion = f"skipped: {reason}"
        return chart

    fps = chart.source.fps or 30.0
    frame_us = int(round(1e6 / fps))
    if frame_uncertainty_us is None:
        frame_uncertainty_us = frame_us
    if match_tol_us is None:
        match_tol_us = 2 * frame_us

    backend = "dsp"
    transcribed = transcribe_learned(audio_path) if audio_path is not None else None
    if transcribed is not None:
        backend = "learned"
    else:
        # Transcribe the chart's pitches plus the lower pitches whose harmonics
        # land in them — the split pass needs those to recognise harmonic bleed.
        pitches = {n.pitch for n in chart.notes}
        pitches |= {
            p - i for p in pitches for i in _HARMONIC_INTERVALS if p - i >= 21
        }
        transcribed = transcribe(samples, sample_rate, sorted(pitches))

    # Align the visual clock to the audio clock before any per-note matching:
    # the constant lead/lag would otherwise eat into every match tolerance.
    offset_us = estimate_global_offset(chart.notes, transcribed)
    notes = chart.notes
    if offset_us:
        notes = [
            ExtractedNote(
                pitch=n.pitch,
                start_us=max(0, n.start_us + offset_us),
                dur_us=n.dur_us,
                hand=n.hand,
                velocity=n.velocity,
                confidence=n.confidence,
            )
            for n in notes
        ]

    notes, n_split = split_merged_notes(notes, transcribed)

    fused_notes, matched = _fuse_with_stats(
        notes,
        transcribed,
        frame_uncertainty_us=frame_uncertainty_us,
        match_tol_us=match_tol_us,
    )
    chart.source.audio_fusion = (
        f"applied ({backend}): {reason}; offset {offset_us / 1000:+.0f} ms; "
        f"split {n_split} merged notes; {matched}/{len(fused_notes)} notes matched"
    )
    return ExtractedChart(notes=fused_notes, source=chart.source)
