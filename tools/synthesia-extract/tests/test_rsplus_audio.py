"""Tests for anchoring a Rocksmith+ chart to its audio (M6-H).

Synthetic only: the audio is rendered tones (``synth.render_audio``) and the
visual charts are written by hand, each reproducing one error the M6-G
extractor makes on real recordings (MISS text read late, split held notes,
ghost notes).
"""

from __future__ import annotations

import os
import subprocess
import sys

import cv2
import numpy as np

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from synthesia_extract import rsplus_audio  # noqa: E402
from synthesia_extract.io import write_wav  # noqa: E402
from synthesia_extract.rsplus_audio import (  # noqa: E402
    SHORT_NOTE_US,
    Attack,
    anchor_notes,
    fuse_rsplus_chart,
)
from synthesia_extract.rsplus_synth import RsConfig, RsNote, RsSynth  # noqa: E402
from synthesia_extract.schema import ExtractedChart, ExtractedNote, Hand, SourceMeta  # noqa: E402
from synthesia_extract.synth import SynthNote  # noqa: E402

SR = 22050
PIANO = (1.0, 0.5, 0.3, 0.2)


def _song(n: int = 40, seed: int = 3) -> list[SynthNote]:
    rng = np.random.default_rng(seed)
    t, notes = 400_000, []
    for _ in range(n):
        dur = int(rng.integers(250_000, 600_000))
        notes.append(SynthNote(int(rng.integers(48, 80)), t, dur, "Right", velocity=90))
        t += int(rng.integers(300_000, 500_000))
    return notes


def _visual(notes, lag_us=0, jitter_us=0, seed=0, hand=Hand.RIGHT) -> list[ExtractedNote]:
    rng = np.random.default_rng(seed)
    return [
        ExtractedNote(
            pitch=n.pitch,
            start_us=n.start_us + lag_us + int(rng.integers(-jitter_us, jitter_us + 1)),
            dur_us=n.dur_us,
            hand=hand,
        )
        for n in notes
    ]


def _piano(notes, total_s=None) -> np.ndarray:
    """Decaying harmonic tones with a soft release, like the game's MIDI piano.

    (``synth.render_audio``'s flat body and 20 ms release cut make a click at
    every note end, which reads as an attack.)
    """
    end = max(n.start_us + n.dur_us for n in notes) / 1e6 + 0.3
    out = np.zeros(int(round((total_s or end) * SR)))
    for n in notes:
        i0 = int(round(n.start_us / 1e6 * SR))
        k = min(len(out) - i0, int(round((n.dur_us / 1e6 + 0.08) * SR)))
        t = np.arange(k) / SR
        env = np.exp(-t / 0.6) * np.minimum(1.0, t / 0.003)
        rel = t > n.dur_us / 1e6
        env[rel] *= 0.5 + 0.5 * np.cos(np.pi * np.minimum(1.0, (t[rel] - n.dur_us / 1e6) / 0.08))
        f0 = 440.0 * 2 ** ((n.pitch - 69) / 12)
        tone = sum(a * np.sin(2 * np.pi * f0 * h * t) for h, a in enumerate(PIANO, start=1))
        out[i0 : i0 + k] += 0.15 * n.velocity / 127 * env * tone
    return out.astype(np.float32)


def _fuse(visual, notes):
    clean = _piano(notes)
    chart = ExtractedChart(notes=visual, source=SourceMeta(fps=30.0))
    return fuse_rsplus_chart(chart, clean, SR)


# --------------------------------------------------------------------------- #
# Against rendered audio
# --------------------------------------------------------------------------- #
def test_onsets_snap_to_the_audio():
    truth = _song()
    fused = _fuse(_visual(truth, lag_us=60_000, jitter_us=30_000), truth)
    assert "rsplus anchoring" in fused.source.audio_fusion
    assert abs(fused.source.clock_offset_us + 60_000) <= 15_000
    assert len(fused.notes) == len(truth)
    for n, t in zip(fused.notes, sorted(truth, key=lambda n: (n.start_us, n.pitch))):
        assert n.pitch == t.pitch
        assert abs(n.start_us - t.start_us) <= 12_000, (n, t)


def test_fast_arpeggio_read_off_miss_text():
    # Bar 55 of the real recording: F3 G3 F2 C3 on a 183 ms pulse; the video
    # mostly caught each note's MISS text, ~0.37 s late and ~0.4 s long.
    pattern = [53, 55, 41, 48]
    truth = [
        SynthNote(pattern[i % 4], 500_000 + i * 183_000, 150_000, "Right", velocity=90)
        for i in range(16)
    ]
    visual = []
    for i, n in enumerate(truth):
        if i % 3:
            visual.append(ExtractedNote(n.pitch, n.start_us + 370_000, 400_000, Hand.LEFT))
        else:  # the block itself, then its MISS text
            visual.append(ExtractedNote(n.pitch, n.start_us + 10_000, 67_000, Hand.LEFT))
            visual.append(ExtractedNote(n.pitch, n.start_us + 380_000, 400_000, Hand.LEFT))
    fused = _fuse(visual, truth)
    got = [(n.pitch, n.start_us, n.dur_us) for n in fused.notes]
    assert len(got) == len(truth), got
    for (p, s, d), t in zip(got, truth):
        assert p == t.pitch
        assert abs(s - t.start_us) <= 12_000, (p, s, t)
        assert d == SHORT_NOTE_US
    assert all(n.hand is Hand.LEFT for n in fused.notes)


def test_velocity_follows_loudness():
    truth = [
        SynthNote(n.pitch, n.start_us, n.dur_us, "Right", velocity=40 if i % 2 else 120)
        for i, n in enumerate(_song(n=24))
    ]
    fused = _fuse(_visual(truth), truth)
    soft = [n.velocity for n, t in zip(fused.notes, truth) if t.velocity == 40]
    loud = [n.velocity for n, t in zip(fused.notes, truth) if t.velocity == 120]
    assert max(soft) < min(loud)


def test_unrelated_chart_falls_back_to_generic_fusion():
    truth = _song()
    rng = np.random.default_rng(9)
    unrelated = [
        ExtractedNote(int(rng.integers(48, 80)), int(t), 300_000, Hand.RIGHT)
        for t in rng.uniform(0, truth[-1].start_us, len(truth))
    ]
    fused = _fuse(unrelated, truth)
    assert fused.source.audio_fusion.startswith("rsplus anchoring skipped")
    assert len(fused.notes) == len(unrelated)


def test_full_mix_falls_back_to_generic_fusion():
    truth = _song(n=12)
    rng = np.random.default_rng(0)
    noisy = _piano(truth)
    noisy = noisy + rng.standard_normal(len(noisy)).astype(np.float32) * 0.35
    chart = ExtractedChart(notes=_visual(truth), source=SourceMeta(fps=30.0))
    fused = fuse_rsplus_chart(chart, noisy, SR)
    assert fused.source.audio_fusion.startswith("skipped")
    assert len(fused.notes) == len(truth)


# --------------------------------------------------------------------------- #
# The anchoring rules, on hand-written attacks
# --------------------------------------------------------------------------- #
def _note(pitch, start_ms, dur_ms, hand=Hand.RIGHT):
    return ExtractedNote(pitch, start_ms * 1000, dur_ms * 1000, hand)


def _att(pitch, ms, strength=1.0):
    return Attack(ms * 1000, pitch, strength)


def _run(notes, attacks):
    out, stats = anchor_notes(notes, attacks)
    return [(n.pitch, n.start_us // 1000, n.dur_us // 1000) for n in out], stats


def test_long_note_keeps_its_visual_end():
    got, _ = _run([_note(60, 1030, 970)], [_att(60, 1000)])
    assert got == [(60, 1000, 1000)]


def test_short_note_is_a_sixteenth():
    got, _ = _run([_note(60, 1030, 70)], [_att(60, 1000)])
    assert got == [(60, 1000, SHORT_NOTE_US // 1000)]


def test_short_notes_never_overlap_the_next_on_their_pitch():
    got, _ = _run([_note(60, 1000, 60), _note(60, 1100, 60)], [_att(60, 1000), _att(60, 1100)])
    assert got == [(60, 1000, 100), (60, 1100, SHORT_NOTE_US // 1000)]


def test_miss_late_note_moves_back_to_its_attack():
    got, stats = _run([_note(60, 1370, 400)], [_att(60, 1000)])
    assert got == [(60, 1000, SHORT_NOTE_US // 1000)]
    assert stats["miss"] == 1


def test_miss_text_after_a_found_note_is_dropped():
    got, stats = _run([_note(60, 1000, 67), _note(60, 1370, 400)], [_att(60, 1000)])
    assert got == [(60, 1000, SHORT_NOTE_US // 1000)]
    assert stats["miss_text"] == 1


def test_split_held_note_is_merged():
    got, stats = _run([_note(60, 1000, 900), _note(60, 2100, 900)], [_att(60, 1000)])
    assert got == [(60, 1000, 2000)]
    assert stats["merged"] == 1


def test_restruck_note_is_not_merged():
    got, _ = _run(
        [_note(60, 1000, 900), _note(60, 2100, 900)], [_att(60, 1000), _att(60, 2100)]
    )
    assert got == [(60, 1000, 900), (60, 2100, 900)]


def test_note_with_no_attack_is_dropped():
    got, stats = _run([_note(60, 1000, 500), _note(72, 3000, 1500)], [_att(60, 1000)])
    assert got == [(60, 1000, 500)]
    assert stats["no_attack"] == 1


def test_an_attack_is_claimed_once():
    # Two overlapping visual notes near one attack: the nearer anchors, the
    # other has no attack of its own and merges into it.
    got, _ = _run([_note(60, 990, 500), _note(60, 1060, 500)], [_att(60, 1000)])
    assert got == [(60, 1000, 560)]


def test_hands_are_kept():
    notes = [_note(48, 1000, 300, Hand.LEFT), _note(67, 1000, 300, Hand.RIGHT)]
    out, _ = anchor_notes(notes, [_att(48, 1000), _att(67, 1000)])
    assert {n.pitch: n.hand for n in out} == {48: Hand.LEFT, 67: Hand.RIGHT}


def test_detect_attacks_names_the_pitches_of_a_chord():
    truth = [
        SynthNote(p, 300_000, 500_000, "Right", velocity=90) for p in (48, 55, 64)
    ] + [SynthNote(60, 900_000, 400_000, "Right", velocity=90)]
    clean = _piano(truth)
    onsets = rsplus_audio.detect_onsets(clean, SR)
    attacks = rsplus_audio.detect_attacks(clean, SR, onsets, range(36, 90))
    found = {(a.pitch, round(a.time_us / 50_000) * 50) for a in attacks}
    # Overtones can add a sub-octave (a chord's common root), which anchoring
    # ignores: only same-pitch attacks are used.
    assert {(48, 300), (55, 300), (64, 300), (60, 900)} <= found, found
    assert {round(a.time_us / 50_000) * 50 for a in attacks} == {300, 900}


# --------------------------------------------------------------------------- #
# CLI
# --------------------------------------------------------------------------- #
def test_cli_uses_rsplus_fusion_on_a_rsplus_video(tmp_path):
    fps = 30.0
    f = lambda n: int(round(n * 1e6 / fps))  # noqa: E731
    want = [
        RsNote(60, f(6), f(12)),
        RsNote(67, f(21), f(12), hue=25),
        RsNote(64, f(36), f(12), hue=130),
    ]
    syn = RsSynth(want, cfg=RsConfig(duration_s=2.0))
    frames_dir = tmp_path / "frames"
    frames_dir.mkdir()
    for i, frame in enumerate(syn.frames()):
        cv2.imwrite(str(frames_dir / f"frame_{i:05d}.png"), frame)
    wav = tmp_path / "audio.wav"
    tones = [SynthNote(n.pitch, n.start_us, n.dur_us, "Right", velocity=90) for n in want]
    write_wav(str(wav), _piano(tones, total_s=2.0), SR)
    out = tmp_path / "chart.json"
    result = subprocess.run(
        [sys.executable, os.path.join(ROOT, "extract.py"), "--in", str(frames_dir),
         "--fps", str(fps), "--out", str(out), "--audio-fusion", "--audio", str(wav)],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )
    assert result.returncode == 0, result.stderr
    assert "rsplus anchoring" in result.stderr, result.stderr
