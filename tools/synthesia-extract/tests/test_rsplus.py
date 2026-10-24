"""Tests for the Rocksmith+ piano-highway extractor (M6-G).

All fixtures are *synthetic*, rendered in memory by
``synthesia_extract.rsplus_synth`` — never real or copyrighted footage.  Each
case reproduces one behaviour seen on real Rocksmith+ recordings (hollow
black-key bars, pale missed notes, "MISS" text on the hit line, fog glows,
hand-zone tints) and asserts the extractor recovers the known notes.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

import cv2
import numpy as np

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from synthesia_extract import rsplus  # noqa: E402
from synthesia_extract.rsplus_synth import (  # noqa: E402
    HOLLOW,
    PALE,
    Glow,
    RsConfig,
    RsNote,
    RsSynth,
    Zone,
)
from synthesia_extract.schema import Hand  # noqa: E402
from synthesia_extract.synth import SynthConfig, c_major_demo, render_frames  # noqa: E402

FPS = 30.0
FRAME_US = int(1e6 / FPS)


def _f(n: int) -> int:
    """``n`` frames in µs (note times on the frame grid keep onsets exact)."""
    return int(round(n * 1e6 / FPS))


def _extract(syn: RsSynth, **kw):
    return rsplus.extract_frames(syn.frames(), syn.cfg.fps, syn.sample(12), log=lambda _m: None, **kw)


def _assert_matches(got, want, *, onset_frames: int = 1, dur_frames: int = 2):
    """Every wanted note found once (pitch exact, timing within tolerance), nothing extra."""
    remaining = list(got)
    for w in want:
        cands = [n for n in remaining if n.pitch == w.pitch]
        assert cands, f"missing pitch {w.pitch} @ {w.start_us}"
        best = min(cands, key=lambda n: abs(n.start_us - w.start_us))
        assert abs(best.start_us - w.start_us) <= onset_frames * FRAME_US, (w, best)
        assert abs(best.dur_us - w.dur_us) <= dur_frames * FRAME_US, (w, best)
        remaining.remove(best)
    assert not remaining, f"unexpected notes: {remaining}"


# --------------------------------------------------------------------------- #
# Layout detection and calibration
# --------------------------------------------------------------------------- #
def test_detects_rsplus_layout():
    syn = RsSynth([RsNote(60, _f(5), _f(10))])
    assert rsplus.is_rsplus(syn.sample(6))


def test_synthesia_clip_is_not_rsplus():
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)
    assert not rsplus.is_rsplus(frames[:: max(1, len(frames) // 6)])


def test_calibration_lanes_match_ground_truth():
    syn = RsSynth([])
    geo = rsplus.calibrate(syn.sample(6))
    assert geo.anchored, geo.notes
    assert [ln.pitch for ln in geo.lanes] == syn.highway_pitches
    for ln in geo.lanes:
        assert abs(ln.x - syn.lane_x(ln.pitch)) <= 2.5, (ln.pitch, ln.x, syn.lane_x(ln.pitch))
        assert ln.black == syn.is_black(ln.pitch)


def test_white_lanes_sit_between_black_keys():
    # C/F lanes lean left of their key's centre, E/B lanes right, D/G/A stay put.
    syn = RsSynth([])
    geo = rsplus.calibrate(syn.sample(6))
    x = {ln.pitch: ln.x for ln in geo.lanes}
    kw = geo.key_width
    key_centre = {p: (syn.boundary(i) + syn.boundary(i + 1)) / 2 for i, p in enumerate(
        syn._white_pitch(i) for i in range(syn.cfg.n_keys))}
    assert x[60] < key_centre[60] - 0.08 * kw  # C
    assert x[64] > key_centre[64] + 0.08 * kw  # E
    assert abs(x[62] - key_centre[62]) < 0.05 * kw  # D


def test_middle_c_guess_and_octave_shift_without_label():
    # No boxed label: the C nearest the frame centre is taken as middle C, and
    # --octave-shift corrects a wrong guess.
    cfg = RsConfig(middle_c_box=False, first_white_pitch=36)  # C2..: centre C is C3
    syn = RsSynth([RsNote(52, _f(4), _f(8))], cfg=cfg)
    geo = rsplus.calibrate(syn.sample(6))
    assert not geo.anchored
    guessed = _extract(syn)
    assert [n.pitch for n in guessed.notes] == [52 + 12]
    fixed = _extract(syn, octave_shift=-1)
    assert [n.pitch for n in fixed.notes] == [52]


# --------------------------------------------------------------------------- #
# Reading bars at the hit line
# --------------------------------------------------------------------------- #
def test_recovers_mixed_notes():
    want = [
        RsNote(60, _f(6), _f(15), hue=150),  # C4, tube
        RsNote(66, _f(9), _f(18), hue=15, style=HOLLOW),  # F#4, hollow black-key bar
        RsNote(53, _f(12), _f(6), hue=60, style=PALE),  # F3, pale missed note
        RsNote(72, _f(30), _f(24), hue=130),  # C5
        RsNote(49, _f(36), _f(9), hue=0, style=HOLLOW),  # C#3
        RsNote(77, _f(40), _f(12), hue=25),  # F5
    ]
    syn = RsSynth(want, cfg=RsConfig(duration_s=3.0))
    chart = _extract(syn)
    _assert_matches(chart.notes, want)


def test_chord_of_neighbours_does_not_light_black_lane_between():
    # D4 and E4 together leave D#4 dark; D#4 with E4 lights both.
    want = [
        RsNote(62, _f(6), _f(12), hue=150),
        RsNote(64, _f(6), _f(12), hue=130),
        RsNote(63, _f(30), _f(12), hue=15, style=HOLLOW),
        RsNote(64, _f(30), _f(12), hue=130),
    ]
    syn = RsSynth(want, cfg=RsConfig(duration_s=2.0))
    _assert_matches(_extract(syn).notes, want)


def test_repeated_pale_notes_stay_separate():
    # A run of short missed notes on one key, four frames apart.
    want = [RsNote(55, _f(6 + 10 * k), _f(6), hue=10, style=PALE) for k in range(5)]
    syn = RsSynth(want, cfg=RsConfig(duration_s=2.2))
    _assert_matches(_extract(syn).notes, want)


def test_miss_text_does_not_split_a_note():
    want = [RsNote(64, _f(6), _f(30), hue=130, miss_from_us=_f(16), miss_to_us=_f(22))]
    syn = RsSynth(want, cfg=RsConfig(duration_s=1.6))
    _assert_matches(_extract(syn).notes, want)


def test_fog_glow_makes_no_notes():
    syn = RsSynth(
        [RsNote(71, _f(4), _f(8))],
        glows=[Glow(250, 0, _f(45)), Glow(560, _f(10), _f(40), sigma=70)],
        cfg=RsConfig(duration_s=1.5),
    )
    assert [n.pitch for n in _extract(syn).notes] == [71]


# --------------------------------------------------------------------------- #
# Hands
# --------------------------------------------------------------------------- #
def test_hands_from_zone_tints():
    notes = [
        RsNote(50, _f(8), _f(8)),
        RsNote(55, _f(20), _f(8), hue=60),
        RsNote(64, _f(8), _f(8), hue=130),
        RsNote(69, _f(20), _f(8), hue=25),
        RsNote(57, _f(40), _f(8), hue=150),  # inside the left zone's upper edge
    ]
    zones = [Zone(48, 59, 0, _f(60), "Left"), Zone(60, 79, 0, _f(60), "Right")]
    syn = RsSynth(notes, zones=zones, cfg=RsConfig(duration_s=2.0))
    hands = {n.pitch: n.hand for n in _extract(syn).notes}
    assert hands == {50: Hand.LEFT, 55: Hand.LEFT, 57: Hand.LEFT, 64: Hand.RIGHT, 69: Hand.RIGHT}


def test_no_zones_means_unknown_hand():
    syn = RsSynth([RsNote(60, _f(6), _f(9))], cfg=RsConfig(duration_s=1.0))
    assert [n.hand for n in _extract(syn).notes] == [Hand.UNKNOWN]


# --------------------------------------------------------------------------- #
# Series assembly
# --------------------------------------------------------------------------- #
def test_weak_frames_bridge_but_never_start_a_note():
    off, weak, strong = rsplus.OFF, rsplus.WEAK, rsplus.STRONG
    series = [off, weak, weak, strong, strong, weak, weak, strong, strong, weak, off, weak, weak, off]
    state = np.array(series, np.int8)[:, None]
    zones = np.full(state.shape, -1.0)
    lane = rsplus.Lane(pitch=60, x=0.0, black=False)
    notes = rsplus.series_to_notes(state, zones, [lane], FPS)
    # One note from the first to the last strong frame; the trailing weak-only
    # run (frames 11-12) is not a note.
    assert [(n.start_us, n.dur_us) for n in notes] == [(_f(3), _f(6))]


def test_single_strong_frame_is_noise():
    state = np.array([[0], [2], [0], [0]], np.int8)
    lane = rsplus.Lane(pitch=60, x=0.0, black=False)
    assert rsplus.series_to_notes(state, np.full(state.shape, -1.0), [lane], FPS) == []


# --------------------------------------------------------------------------- #
# Output / CLI
# --------------------------------------------------------------------------- #
def test_source_meta():
    syn = RsSynth([RsNote(60, _f(4), _f(6))], cfg=RsConfig(duration_s=0.8))
    src = _extract(syn).source
    assert src.extractor_version.startswith("rsplus-extract")
    assert src.fps == FPS
    assert src.frame_height_px == syn.cfg.height
    assert src.hit_line_px == syn.cfg.hit
    # A perspective highway has no single scroll speed; leaving it out keeps
    # the importer from deriving a (wrong) linear overlay alignment.
    assert src.scroll_px_per_s is None


def test_cli_auto_detects_rsplus_frames_dir(tmp_path):
    want = [RsNote(60, _f(6), _f(9)), RsNote(67, _f(12), _f(9), hue=25)]
    syn = RsSynth(want, cfg=RsConfig(duration_s=1.2))
    frames_dir = tmp_path / "frames"
    frames_dir.mkdir()
    for i, f in enumerate(syn.frames()):
        cv2.imwrite(str(frames_dir / f"frame_{i:05d}.png"), f)
    out = tmp_path / "chart.json"
    result = subprocess.run(
        [sys.executable, os.path.join(ROOT, "extract.py"), "--in", str(frames_dir),
         "--fps", str(FPS), "--out", str(out)],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )
    assert result.returncode == 0, result.stderr
    assert "layout: rsplus" in result.stderr
    data = json.loads(out.read_text())
    assert data["source"]["extractor_version"].startswith("rsplus-extract")
    assert sorted(n["pitch"] for n in data["notes"]) == [60, 67]
