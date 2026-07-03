"""Tests for the Synthesia visual extractor.

All fixtures are *synthetic* (generated in-memory by ``synthesia_extract.synth``)
— never a real or copyrighted video.  We assert the extractor recovers the known
ground truth: pitches exactly, onsets/durations within frame tolerance, and the
two hands mapped to the correct colours.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

import cv2
import numpy as np
import pytest

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from synthesia_extract import schema  # noqa: E402
from synthesia_extract.pipeline import (  # noqa: E402
    calibrate_keyboard,
    detect_hit_line,
    estimate_scroll,
    extract_chart,
)
from synthesia_extract.synth import (  # noqa: E402
    SynthConfig,
    SynthNote,
    c_major_demo,
    render_frames,
    render_overlay_frames,
)


def _frame_tol_us(cfg: SynthConfig, frames: int = 2) -> int:
    return int(frames * 1e6 / cfg.fps)


def _match(notes, pitch, start_us):
    """Find the extracted note nearest ``start_us`` for ``pitch``."""
    cands = [n for n in notes if n.pitch == pitch]
    if not cands:
        return None
    return min(cands, key=lambda n: abs(n.start_us - start_us))


# --------------------------------------------------------------------------- #
# Calibration / scroll building blocks
# --------------------------------------------------------------------------- #
def test_detect_hit_line():
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _kb = render_frames(notes, cfg)
    hit = detect_hit_line(frames[len(frames) // 2])
    assert abs(hit - cfg.hit_line) <= 2


def test_calibration_recovers_pitches_and_middle_c():
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, kb_truth = render_frames(notes, cfg)
    ref = frames[len(frames) // 2]
    hit = detect_hit_line(ref)
    kb = calibrate_keyboard(ref, hit)
    # Same number of white keys, same pitches, and middle C present.
    assert kb.white_pitches == kb_truth.white_pitches
    assert 60 in kb.white_pitches
    # Each ground-truth note's column maps to its pitch.
    for n in notes:
        span = kb_truth.pitch_x_range(n.pitch)
        cx = (span[0] + span[1]) // 2
        assert kb.x_to_pitch[cx] == n.pitch


def test_estimate_scroll_speed():
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)
    v = estimate_scroll(frames, cfg.hit_line, cfg.fps)
    assert v is not None
    assert abs(v - cfg.scroll_px_per_s) / cfg.scroll_px_per_s < 0.1


# --------------------------------------------------------------------------- #
# End-to-end extraction
# --------------------------------------------------------------------------- #
def test_full_clip_matches_ground_truth():
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)
    chart = extract_chart(frames, cfg.fps, title="synthetic")

    tol = _frame_tol_us(cfg, frames=2)

    # Pitches recovered exactly (as a multiset).
    got_pitches = sorted(n.pitch for n in chart.notes)
    want_pitches = sorted(n.pitch for n in notes)
    assert got_pitches == want_pitches

    # Onsets/durations within frame tolerance, hands mapped correctly.
    for gt in notes:
        m = _match(chart.notes, gt.pitch, gt.start_us)
        assert m is not None, f"missing note pitch={gt.pitch}"
        assert abs(m.start_us - gt.start_us) <= tol, f"onset off for {gt}"
        assert abs(m.dur_us - gt.dur_us) <= tol, f"dur off for {gt}"
        assert m.hand.value == gt.hand, f"hand off for {gt}: {m.hand}"


def test_source_meta_populated():
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)
    chart = extract_chart(frames, cfg.fps)
    assert chart.source.fps == pytest.approx(cfg.fps)
    assert chart.source.scroll_px_per_s is not None
    assert abs(chart.source.scroll_px_per_s - cfg.scroll_px_per_s) / cfg.scroll_px_per_s < 0.1
    assert chart.source.extractor_version == schema.EXTRACTOR_VERSION


def test_single_note():
    cfg = SynthConfig()
    notes = [SynthNote(60, 300_000, 400_000, "Right")]
    frames, _ = render_frames(notes, cfg)
    chart = extract_chart(frames, cfg.fps)
    assert len(chart.notes) == 1
    n = chart.notes[0]
    assert n.pitch == 60
    tol = _frame_tol_us(cfg, frames=2)
    assert abs(n.start_us - 300_000) <= tol
    assert abs(n.dur_us - 400_000) <= tol


def test_empty_clip_does_not_crash():
    cfg = SynthConfig()
    frames, _ = render_frames([], cfg)
    chart = extract_chart(frames, cfg.fps)
    assert chart.notes == []
    # Still a valid chart with provenance.
    assert chart.source.extractor_version == schema.EXTRACTOR_VERSION


def test_chords_recovered():
    cfg = SynthConfig()
    notes = [
        SynthNote(60, 0, 500_000, "Right"),
        SynthNote(64, 0, 500_000, "Right"),
        SynthNote(67, 0, 500_000, "Right"),
    ]
    frames, _ = render_frames(notes, cfg)
    chart = extract_chart(frames, cfg.fps)
    assert sorted(n.pitch for n in chart.notes) == [60, 64, 67]


def test_suppress_semitone_ghost_with_staggered_onset():
    """A thick bar's white/black twin lags the struck core by a frame or two, so
    its onset is staggered.  Single-linkage onset grouping must still pair the
    two adjacent semitones (an earlier note must not start a group that splits
    the twin off past the tolerance window) so the ghost is dropped."""
    from synthesia_extract.pipeline import _RawNote, suppress_semitone_ghosts

    col = np.array([200.0, 100.0, 50.0])
    raws = [
        # An earlier note whose onset would anchor a group-from-first window.
        _RawNote(pitch=71, start_us=0, dur_us=100_000, coverage=0.9, color=col),
        _RawNote(pitch=47, start_us=10_000, dur_us=100_000, coverage=0.89, color=col),
        # B2's black-key ghost, 11 ms later — 21 ms past pitch 71's onset.
        _RawNote(pitch=46, start_us=21_000, dur_us=100_000, coverage=0.74, color=col),
    ]
    out = suppress_semitone_ghosts(raws, onset_tol_us=15_000)
    pitches = sorted(r.pitch for r in out)
    assert 46 not in pitches, "staggered white/black ghost must be dropped"
    assert pitches == [47, 71]


# --------------------------------------------------------------------------- #
# Glissando / fast-run recovery (sub-threshold diagonal pulses)
# --------------------------------------------------------------------------- #
_GLISS_US_PER_PX = 5000.0


def _gliss_roll(n_pitches=24, n_bins=160, base=48):
    """Empty roll: global 1-D totals=10, votes/colour zeroed.  Coverage of a bin
    is votes/totals, so votes=cov*10 sets a known coverage."""
    pitches = list(range(base, base + n_pitches))
    votes = np.zeros((n_pitches, n_bins), dtype=np.float64)
    totals = np.full(n_bins, 10.0)
    color = np.zeros((n_pitches, n_bins, 3), dtype=np.float64)
    return pitches, votes, totals, color


def _stamp(votes, color, pidx, a, b, cov):
    votes[pidx, a:b] = cov * 10.0
    color[pidx, a:b] = (0, 200, 0)


def _stamp_sweep(votes, color, pitches, base, count, step, *, cov=0.4, run=3):
    """Stamp a diagonal of ``count`` sub-threshold taps, one semitone apart,
    onset advancing ``step`` bins each — a synthetic glissando."""
    for k in range(count):
        a = 10 + k * step
        _stamp(votes, color, pitches.index(base + k), a, a + run, cov)


def test_recover_glissando_recovers_long_sweep():
    from synthesia_extract.pipeline import recover_glissando_runs

    pitches, votes, totals, color = _gliss_roll()
    _stamp_sweep(votes, color, pitches, 48, 12, 3)  # wide, straight, > min_dur
    out = recover_glissando_runs(votes, totals, color, pitches, _GLISS_US_PER_PX, [])
    assert sorted(r.pitch for r in out) == list(range(48, 60))


def test_recover_glissando_ignores_short_run():
    """A 5-tap diagonal is below ``min_span`` — the guard that stopped the old
    pass from inventing phantom diagonals out of tight ghost clusters."""
    from synthesia_extract.pipeline import recover_glissando_runs

    pitches, votes, totals, color = _gliss_roll()
    _stamp_sweep(votes, color, pitches, 48, 5, 3)
    assert recover_glissando_runs(votes, totals, color, pitches, _GLISS_US_PER_PX, []) == []


def test_recover_glissando_ignores_scatter():
    from synthesia_extract.pipeline import recover_glissando_runs

    pitches, votes, totals, color = _gliss_roll()
    # Weak blobs that never form a stepping, time-advancing diagonal.
    for pitch, a in [(60, 10), (67, 12), (71, 40), (63, 80), (58, 15), (69, 55), (50, 30)]:
        _stamp(votes, color, pitches.index(pitch), a, a + 3, 0.4)
    assert recover_glissando_runs(votes, totals, color, pitches, _GLISS_US_PER_PX, []) == []


def test_recover_glissando_skips_what_chart_already_has():
    """A long diagonal already covered by kept notes is not a *missing* sweep;
    the uncovered guard stops recovery re-tracing the strict pass's own notes."""
    from synthesia_extract.pipeline import _RawNote, recover_glissando_runs

    pitches, votes, totals, color = _gliss_roll()
    _stamp_sweep(votes, color, pitches, 48, 12, 3)
    existing = [
        _RawNote(pitch=48 + k, start_us=int((10 + k * 3) * _GLISS_US_PER_PX),
                 dur_us=int(3 * _GLISS_US_PER_PX), coverage=0.9,
                 color=np.array([0.0, 200.0, 0.0]))
        for k in range(12)
    ]
    assert recover_glissando_runs(votes, totals, color, pitches, _GLISS_US_PER_PX, existing) == []


def test_recover_glissando_ignores_near_instant_cluster():
    """Twelve lanes firing almost together (< min_dur) is a false onset burst,
    not a real slide that crosses several video frames."""
    from synthesia_extract.pipeline import recover_glissando_runs

    pitches, votes, totals, color = _gliss_roll()
    _stamp_sweep(votes, color, pitches, 48, 12, 1, run=2)  # onset step 1 bin -> ~55 ms
    assert recover_glissando_runs(votes, totals, color, pitches, _GLISS_US_PER_PX, []) == []


def test_roll_to_notes_recovers_glissando_by_default():
    """Recovery is on by default now; a clear sweep is laid, and it can still
    be switched off."""
    from synthesia_extract.pipeline import _roll_to_notes

    pitches, votes, totals, color = _gliss_roll()
    _stamp_sweep(votes, color, pitches, 48, 12, 3)
    scroll = 1e6 / _GLISS_US_PER_PX  # keep us_per_px == _GLISS_US_PER_PX

    on = _roll_to_notes(votes, totals, color, pitches, _GLISS_US_PER_PX, scroll)
    assert sorted(n.pitch for n in on) == list(range(48, 60))

    off = _roll_to_notes(
        votes, totals, color, pitches, _GLISS_US_PER_PX, scroll, recover_glissandi=False
    )
    assert off == []


# --------------------------------------------------------------------------- #
# Overlay style: translucent white bars over busy artwork, filmed-piano keys
# --------------------------------------------------------------------------- #
def test_overlay_full_clip_recovers_notes():
    """The style that broke the saturation-based extractor (issue #148)."""
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_overlay_frames(notes, cfg)
    chart = extract_chart(frames, cfg.fps, title="overlay-synthetic")

    tol = _frame_tol_us(cfg, frames=2)
    got_pitches = sorted(n.pitch for n in chart.notes)
    want_pitches = sorted(n.pitch for n in notes)
    assert got_pitches == want_pitches

    for gt in notes:
        m = _match(chart.notes, gt.pitch, gt.start_us)
        assert m is not None, f"missing note pitch={gt.pitch}"
        assert abs(m.start_us - gt.start_us) <= tol, f"onset off for {gt}"
        assert abs(m.dur_us - gt.dur_us) <= tol, f"dur off for {gt}"
    # All bars are the same white -> hands are unknowable from colour.
    assert all(n.hand == schema.Hand.UNKNOWN for n in chart.notes)


def test_overlay_scroll_unaffected_by_static_lyrics():
    """Static text overlays must not drag the scroll estimate toward zero."""
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_overlay_frames(notes, cfg, lyrics=True)
    chart = extract_chart(frames, cfg.fps)
    assert chart.source.scroll_px_per_s is not None
    assert abs(chart.source.scroll_px_per_s - cfg.scroll_px_per_s) / cfg.scroll_px_per_s < 0.1


def test_overlay_black_pattern_calibration():
    """Faint separators defeat white-run calibration; the black-key-pattern
    fallback must still recover every key's lane."""
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, kb_truth = render_overlay_frames(notes, cfg)
    from synthesia_extract.pipeline import background_plate

    plate = background_plate(frames)
    assert plate is not None
    hit = detect_hit_line(plate)
    assert abs(hit - cfg.hit_line) <= 2
    kb = calibrate_keyboard(plate, hit)
    assert len(kb.white_centers) >= len(kb_truth.white_pitches) - 2
    for n in notes:
        span = kb_truth.pitch_x_range(n.pitch)
        cx = (span[0] + span[1]) // 2
        assert kb.x_to_pitch[cx] == n.pitch, f"lane miscalibrated for pitch {n.pitch}"


# --------------------------------------------------------------------------- #
# JSON wire format (must match crates/import schema)
# --------------------------------------------------------------------------- #
def test_json_schema_shape():
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)
    chart = extract_chart(frames, cfg.fps, title="synthetic")
    data = json.loads(chart.to_json())

    assert set(data.keys()) == {"notes", "source"}
    assert data["source"]["extractor_version"] == schema.EXTRACTOR_VERSION
    assert data["source"]["fps"] == pytest.approx(cfg.fps)

    for n in data["notes"]:
        assert set(n.keys()) <= {"pitch", "start_us", "dur_us", "hand", "velocity", "confidence"}
        assert {"pitch", "start_us", "dur_us", "hand"} <= set(n.keys())
        assert n["hand"] in {"Left", "Right", "Unknown"}
        assert "velocity" not in n  # None -> omitted, like serde skip_serializing_if
        assert 0 <= pitch_ok(n["pitch"]) <= 127


def pitch_ok(p):
    assert isinstance(p, int)
    return p


def test_hand_serialization():
    n = schema.ExtractedNote(pitch=60, start_us=0, dur_us=1, hand=schema.Hand.LEFT)
    assert n.to_dict()["hand"] == "Left"
    n.hand = schema.Hand.RIGHT
    assert n.to_dict()["hand"] == "Right"
    n.hand = schema.Hand.UNKNOWN
    assert n.to_dict()["hand"] == "Unknown"


# --------------------------------------------------------------------------- #
# CLI smoke test (frames directory -> JSON), hermetic (no video codec)
# --------------------------------------------------------------------------- #
def test_cli_frames_dir(tmp_path):
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)

    frames_dir = tmp_path / "frames"
    frames_dir.mkdir()
    for i, f in enumerate(frames):
        cv2.imwrite(str(frames_dir / f"frame_{i:05d}.png"), f)

    out = tmp_path / "chart.json"
    result = subprocess.run(
        [
            sys.executable,
            os.path.join(ROOT, "extract.py"),
            "--in", str(frames_dir),
            "--fps", str(cfg.fps),
            "--out", str(out),
        ],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )
    assert result.returncode == 0, result.stderr
    data = json.loads(out.read_text())
    assert sorted(n["pitch"] for n in data["notes"]) == sorted(n.pitch for n in notes)


def _note(pitch: int, start_us: int, dur_us: int = 120_000) -> "schema.ExtractedNote":
    return schema.ExtractedNote(pitch=pitch, start_us=start_us, dur_us=dur_us)


def _dense(start_us: int, count: int, step_us: int = 200_000) -> list:
    """A continuous run of ``count`` onsets — stands in for real playing."""
    return [_note(60 + (i % 12), start_us + i * step_us) for i in range(count)]


def test_trim_edge_phantoms_drops_intro_card_and_beyond_video():
    from synthesia_extract.pipeline import trim_edge_phantoms

    duration_us = 60_000_000  # 60 s clip
    intro = [_note(77, 160_000), _note(74, 190_000), _note(64, 540_000)]  # title card
    body = _dense(5_000_000, 40)  # real performance, continuous
    beyond = [_note(55, 61_000_000), _note(57, 61_000_000)]  # past final frame
    notes = intro + body + beyond

    kept = trim_edge_phantoms(notes, duration_us)
    kept_keys = {(n.pitch, n.start_us) for n in kept}

    # Both edge clusters gone, the whole body retained.
    assert all((n.pitch, n.start_us) not in kept_keys for n in intro + beyond)
    assert all((n.pitch, n.start_us) in kept_keys for n in body)
    assert len(kept) == len(body)


def test_trim_edge_phantoms_keeps_continuous_intro():
    """An intro that flows straight into the body (no long silence) is music."""
    from synthesia_extract.pipeline import trim_edge_phantoms

    # Three onsets then immediately the body, no gap > gap_us anywhere.
    notes = [_note(60, 0), _note(62, 200_000), _note(64, 400_000)] + _dense(600_000, 40)
    kept = trim_edge_phantoms(notes, 60_000_000)
    assert len(kept) == len(notes)


def test_trim_edge_phantoms_keeps_substantial_leading_phrase():
    """A real intro phrase (>= min_cluster) behind a gap is not a card blip."""
    from synthesia_extract.pipeline import trim_edge_phantoms

    phrase = _dense(100_000, 8)  # 8 notes — a genuine pickup phrase
    body = _dense(10_000_000, 40)  # separated by a long rest
    kept = trim_edge_phantoms(phrase + body, 60_000_000)
    assert len(kept) == len(phrase) + len(body)


def test_trim_edge_phantoms_never_deletes_the_only_segment():
    from synthesia_extract.pipeline import trim_edge_phantoms

    # A short lonely clip (< min_cluster, single segment) must survive intact —
    # there is no "body" to prefer it against.
    notes = [_note(60, 0), _note(62, 200_000)]
    kept = trim_edge_phantoms(notes, 60_000_000)
    assert len(kept) == 2


def test_extract_notes_hysteresis_keeps_pale_note_whole():
    """A pale bar dips below the strict cut mid-note; hysteresis holds it as one
    note instead of fragmenting it into two (which min_run then culls)."""
    from synthesia_extract.pipeline import extract_notes

    pitches, votes, totals, color = _gliss_roll()
    pi = pitches.index(60)
    _stamp(votes, color, pi, 10, 14, 0.7)   # seed above threshold
    _stamp(votes, color, pi, 14, 30, 0.45)  # pale middle: < 0.6 but > 0.4
    _stamp(votes, color, pi, 30, 34, 0.7)   # seed above threshold

    strict = extract_notes(
        votes, totals, color, pitches, _GLISS_US_PER_PX,
        threshold=0.6, min_run_px=2, bridge_px=0,
    )
    assert len([r for r in strict if r.pitch == 60]) == 2

    hyst = extract_notes(
        votes, totals, color, pitches, _GLISS_US_PER_PX,
        threshold=0.6, sustain_threshold=0.4, min_run_px=2, bridge_px=0,
    )
    kept = [r for r in hyst if r.pitch == 60]
    assert len(kept) == 1
    assert kept[0].start_us == 10 * int(_GLISS_US_PER_PX)
    assert kept[0].dur_us == 24 * int(_GLISS_US_PER_PX)  # spans bins 10..34


def test_extract_notes_hysteresis_invents_no_onset():
    """Sub-threshold shimmer never seeded above ``threshold`` yields no note —
    hysteresis only *extends* real onsets, it does not create them."""
    from synthesia_extract.pipeline import extract_notes

    pitches, votes, totals, color = _gliss_roll()
    _stamp(votes, color, pitches.index(60), 10, 40, 0.45)  # wholly < 0.6

    got = extract_notes(
        votes, totals, color, pitches, _GLISS_US_PER_PX,
        threshold=0.6, sustain_threshold=0.4, min_run_px=2, bridge_px=0,
    )
    assert all(r.pitch != 60 for r in got)
