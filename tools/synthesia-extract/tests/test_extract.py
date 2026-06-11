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
# Animated-background noise rejection (issue #151)
# --------------------------------------------------------------------------- #
def test_animated_background_noise_rejected():
    """A fully animated music-video backdrop — bright, lane-narrow shapes
    falling at near-scroll speed — defeats every per-pixel gate and must be
    rejected by the note-level colour-mode filter instead."""
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_overlay_frames(notes, cfg, animated_blobs=8)
    chart = extract_chart(frames, cfg.fps, title="animated-overlay")

    tol = _frame_tol_us(cfg, frames=2)
    got_pitches = sorted(n.pitch for n in chart.notes)
    want_pitches = sorted(n.pitch for n in notes)
    assert got_pitches == want_pitches

    for gt in notes:
        m = _match(chart.notes, gt.pitch, gt.start_us)
        assert m is not None, f"missing note pitch={gt.pitch}"
        assert abs(m.start_us - gt.start_us) <= tol, f"onset off for {gt}"
        assert abs(m.dur_us - gt.dur_us) <= tol, f"dur off for {gt}"
    # The diagnostic records that the filter actually engaged.
    assert "dropped" in chart.source.noise_filter


def test_bar_glow_ghosts_suppressed():
    """Bar bloom bleeding into neighbouring lanes must not become ghost notes
    one or two semitones away from every real note."""
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_overlay_frames(notes, cfg, glow=0.35)
    chart = extract_chart(frames, cfg.fps)
    assert sorted(n.pitch for n in chart.notes) == sorted(n.pitch for n in notes)


def test_animated_background_with_glow():
    """Both failure classes at once: the full animated-music-video source."""
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_overlay_frames(notes, cfg, animated_blobs=8, glow=0.35)
    chart = extract_chart(frames, cfg.fps)
    assert sorted(n.pitch for n in chart.notes) == sorted(n.pitch for n in notes)


# --------------------------------------------------------------------------- #
# Noise-filter internals: satellite modes and hysteresis gap bridging
# --------------------------------------------------------------------------- #
def _raw(pitch, start_us, dur_us, color, coverage=0.9):
    from synthesia_extract.pipeline import _RawNote

    return _RawNote(
        pitch=pitch, start_us=start_us, dur_us=dur_us,
        coverage=coverage, color=np.array(color, dtype=np.float64),
    )


def test_color_modes_keep_satellites_drop_far_modes():
    """A translucent ink tinted by a rare scene forms a small tight mode *near*
    the main one — real notes, must survive.  A small tight mode far from any
    bar ink (e.g. dark shadow shapes) is noise, dropped despite its tightness."""
    from synthesia_extract.pipeline import filter_color_modes

    rng = np.random.default_rng(0)
    raws = []
    # Primary: 30 whitish notes.
    for i in range(30):
        raws.append(_raw(60 + i % 12, i * 100_000, 90_000,
                         np.array([215, 215, 210]) + rng.normal(0, 4, 3)))
    # Satellite: 4 pink-tinted notes (a rare backdrop), ~0.13 away in colour.
    for i in range(4):
        raws.append(_raw(64 + i, (30 + i) * 100_000, 90_000,
                         np.array([210, 185, 195]) + rng.normal(0, 3, 3)))
    # Tight-but-far noise: 4 dark notes (shadow shapes).
    for i in range(4):
        raws.append(_raw(40 + i, (34 + i) * 100_000, 90_000,
                         np.array([55, 65, 60]) + rng.normal(0, 3, 3)))
    # Diffuse noise: 8 saturated random colours.
    for i in range(8):
        raws.append(_raw(50 + i, (38 + i) * 100_000, 90_000,
                         rng.uniform(40, 255, 3)))

    survivors, diag = filter_color_modes(raws)
    pitches = sorted(r.pitch for r in survivors)
    assert pitches == sorted(r.pitch for r in raws[:34]), diag
    assert "satellite" in diag


def test_extract_notes_hysteresis_bridges_short_dropout():
    """A brief sub-threshold dip mid-bar (mask dropout) must not split the note,
    but a long low-coverage stretch must not weld two separate notes."""
    from synthesia_extract.pipeline import extract_notes

    n_bins = 120
    votes = np.zeros((1, n_bins), dtype=np.float32)
    totals = np.full((1, n_bins), 10.0, dtype=np.float32)
    color_sum = np.zeros((1, n_bins, 3), dtype=np.float64)
    # One bar split by a 4-bin dropout at coverage 0.5; core colour white.
    votes[0, 10:30] = 10.0
    votes[0, 30:34] = 5.0
    votes[0, 34:50] = 10.0
    # A second, separate note after a 20-bin half-coverage stretch.
    votes[0, 50:70] = 5.0
    votes[0, 70:90] = 10.0
    for b in range(n_bins):
        color_sum[0, b] = votes[0, b] * np.array([250.0, 250.0, 250.0])
    # Poison the dropout bins' colour: it must not leak into the note colour.
    color_sum[0, 30:34] = 5.0 * np.array([0.0, 255.0, 0.0])

    notes = extract_notes(
        votes, totals, color_sum, [60], 1000.0,
        threshold=0.6, hysteresis_low=0.42, min_run_px=3,
    )
    assert [(n.start_us, n.dur_us) for n in notes] == [(10_000, 40_000), (70_000, 20_000)]
    assert np.allclose(notes[0].color, [250.0, 250.0, 250.0])


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
