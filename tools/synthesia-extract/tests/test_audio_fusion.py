"""Tests for the M6-F audio-fusion pass.

All fixtures are *synthetic*: a clean tone-per-note track (and a noisy full-mix
decoy) generated in memory by ``synthesia_extract.synth`` — never real or
copyrighted audio.  The tests assert that fusing clean-piano audio into a visual
chart recovers the ground-truth velocities and produces onsets at least as tight
as visual-only, and that a full-mix decoy safely no-ops.
"""

from __future__ import annotations

import copy
import json
import os
import subprocess
import sys

import numpy as np

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from synthesia_extract import audio  # noqa: E402
from synthesia_extract.io import read_wav, write_wav  # noqa: E402
from synthesia_extract.pipeline import extract_chart  # noqa: E402
from synthesia_extract.schema import ExtractedChart, ExtractedNote, Hand, SourceMeta  # noqa: E402
from synthesia_extract.synth import (  # noqa: E402
    DEFAULT_SAMPLE_RATE,
    SynthConfig,
    SynthNote,
    c_major_demo,
    render_audio,
    render_frames,
    render_full_mix_audio,
)

# Velocity recovery is amplitude-based, so allow a small band; ordering and
# closeness are what "correct velocities" means in practice.
VEL_TOL = 6


def _nearest(notes, pitch, start_us):
    cands = [n for n in notes if n.pitch == pitch]
    return min(cands, key=lambda n: abs(n.start_us - start_us)) if cands else None


# --------------------------------------------------------------------------- #
# Suitability: clean piano accepted, full mix rejected
# --------------------------------------------------------------------------- #
def test_clean_audio_is_suitable():
    notes, _ = c_major_demo()
    clean = render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    ok, reason = audio.assess_suitability(clean, DEFAULT_SAMPLE_RATE)
    assert ok, reason


def test_full_mix_is_unsuitable():
    notes, _ = c_major_demo()
    mix = render_full_mix_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    ok, reason = audio.assess_suitability(mix, DEFAULT_SAMPLE_RATE)
    assert not ok
    assert "full mix" in reason


def test_silence_is_unsuitable():
    silence = np.zeros(DEFAULT_SAMPLE_RATE, dtype=np.float32)
    ok, reason = audio.assess_suitability(silence, DEFAULT_SAMPLE_RATE)
    assert not ok


def test_flatness_ordering():
    notes, _ = c_major_demo()
    clean = render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    mix = render_full_mix_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    assert audio.spectral_flatness(clean) < audio.spectral_flatness(mix)


# --------------------------------------------------------------------------- #
# Transcription: recovers a clean track's notes + velocities
# --------------------------------------------------------------------------- #
def test_transcribe_recovers_each_note():
    notes, _ = c_major_demo()
    sr = DEFAULT_SAMPLE_RATE
    clean = render_audio(notes, sample_rate=sr)
    pitches = [n.pitch for n in notes]
    transcribed = audio.transcribe(clean, sr, pitches)

    for gt in notes:
        m = _nearest(transcribed, gt.pitch, gt.start_us)
        assert m is not None, f"missing transcription for {gt}"
        # Same-pitch repeats are far apart; the nearest must be the right onset.
        assert abs(m.start_us - gt.start_us) <= 40_000, f"onset off for {gt}"
        assert abs(m.velocity - gt.velocity) <= VEL_TOL, (
            f"velocity off for {gt}: got {m.velocity}"
        )


def test_transcribe_velocity_is_monotonic_in_loudness():
    sr = DEFAULT_SAMPLE_RATE
    notes = [
        SynthNote(60, 0, 300_000, "Right", velocity=40),
        SynthNote(60, 500_000, 300_000, "Right", velocity=80),
        SynthNote(60, 1_000_000, 300_000, "Right", velocity=120),
    ]
    clean = render_audio(notes, sample_rate=sr)
    transcribed = sorted(audio.transcribe(clean, sr, [60]), key=lambda t: t.start_us)
    vels = [t.velocity for t in transcribed]
    assert len(vels) == 3, vels
    assert vels[0] < vels[1] < vels[2], vels


def test_transcribe_empty_audio():
    assert audio.transcribe(np.zeros(0, dtype=np.float32), DEFAULT_SAMPLE_RATE, [60]) == []


# --------------------------------------------------------------------------- #
# Fusion unit: velocity copied, onset nudged within frame uncertainty
# --------------------------------------------------------------------------- #
def test_fuse_copies_velocity_and_nudges_within_uncertainty():
    visual = [
        ExtractedNote(pitch=60, start_us=100_000, dur_us=300_000, hand=Hand.RIGHT),
        ExtractedNote(pitch=64, start_us=500_000, dur_us=300_000, hand=Hand.LEFT),
    ]
    transcribed = [
        # 5 ms earlier than visual (< frame uncertainty) -> onset reaches audio.
        audio.TranscribedNote(pitch=60, start_us=95_000, dur_us=305_000, velocity=101),
        # 1 s earlier: far outside the match window -> must NOT match note 64.
        audio.TranscribedNote(pitch=64, start_us=1_500_000, dur_us=10_000, velocity=30),
    ]
    fused = audio.fuse(
        visual, transcribed, frame_uncertainty_us=33_000, match_tol_us=66_000
    )

    n60 = _nearest(fused, 60, 100_000)
    assert n60.velocity == 101  # copied from audio
    assert n60.start_us == 95_000  # nudged fully (5 ms < 33 ms uncertainty)
    assert n60.hand == Hand.RIGHT  # hand untouched

    n64 = _nearest(fused, 64, 500_000)
    assert n64.velocity == audio.DEFAULT_VELOCITY  # unmatched -> default
    assert n64.start_us == 500_000  # timing untouched
    assert n64.hand == Hand.LEFT


def test_fuse_clamps_onset_nudge_to_uncertainty():
    visual = [ExtractedNote(pitch=60, start_us=200_000, dur_us=300_000, hand=Hand.RIGHT)]
    # Audio onset 100 ms earlier, but we only allow a 33 ms nudge.
    transcribed = [audio.TranscribedNote(pitch=60, start_us=100_000, dur_us=300_000, velocity=90)]
    fused = audio.fuse(
        visual, transcribed, frame_uncertainty_us=33_000, match_tol_us=150_000
    )
    assert fused[0].start_us == 200_000 - 33_000  # clamped, not all the way to 100_000


def test_fuse_never_changes_pitch_or_invents_notes():
    visual = [ExtractedNote(pitch=60, start_us=0, dur_us=300_000, hand=Hand.RIGHT)]
    transcribed = [
        audio.TranscribedNote(pitch=62, start_us=0, dur_us=300_000, velocity=90),
        audio.TranscribedNote(pitch=64, start_us=0, dur_us=300_000, velocity=90),
    ]
    fused = audio.fuse(visual, transcribed, frame_uncertainty_us=33_000, match_tol_us=66_000)
    assert len(fused) == 1  # no notes invented
    assert fused[0].pitch == 60  # pitch never overridden
    assert fused[0].velocity == audio.DEFAULT_VELOCITY  # no same-pitch match


# --------------------------------------------------------------------------- #
# End-to-end: visual extraction + clean-piano fusion
# --------------------------------------------------------------------------- #
def test_fusion_recovers_velocities_and_tightens_onsets():
    notes, cfg = c_major_demo()
    frames, _ = render_frames(notes, cfg)
    visual = extract_chart(frames, cfg.fps, title="demo")
    clean = render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)

    fused = audio.fuse_chart(copy.deepcopy(visual), clean, DEFAULT_SAMPLE_RATE)
    assert fused.source.audio_fusion.startswith("applied")

    # Every note now carries a velocity.
    assert all(n.velocity is not None for n in fused.notes)

    vis_err, fused_err, vel_err = [], [], []
    for gt in notes:
        v = _nearest(visual.notes, gt.pitch, gt.start_us)
        f = _nearest(fused.notes, gt.pitch, gt.start_us)
        assert v is not None and f is not None
        vis_err.append(abs(v.start_us - gt.start_us))
        fused_err.append(abs(f.start_us - gt.start_us))
        vel_err.append(abs(f.velocity - gt.velocity))

    # Correct velocities.
    assert max(vel_err) <= VEL_TOL, f"max velocity error {max(vel_err)}"
    # Onsets at least as tight as visual-only (in aggregate).
    assert np.mean(fused_err) <= np.mean(vis_err)


def test_fusion_distinguishes_soft_from_loud():
    notes, cfg = c_major_demo()
    frames, _ = render_frames(notes, cfg)
    visual = extract_chart(frames, cfg.fps)
    clean = render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    fused = audio.fuse_chart(copy.deepcopy(visual), clean, DEFAULT_SAMPLE_RATE)

    # The closing fortissimo chord (vel 112) must read louder than the softest
    # scale note (vel 55).
    soft = _nearest(fused.notes, 60, 0)
    loud = _nearest(fused.notes, 60, 8 * 400_000)  # chord C, last onset
    assert loud.velocity > soft.velocity


def test_full_mix_decoy_is_a_safe_noop():
    notes, cfg = c_major_demo()
    frames, _ = render_frames(notes, cfg)
    visual = extract_chart(frames, cfg.fps)
    mix = render_full_mix_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)

    fused = audio.fuse_chart(copy.deepcopy(visual), mix, DEFAULT_SAMPLE_RATE)
    assert fused.source.audio_fusion.startswith("skipped")
    # Visual notes are byte-for-byte unchanged (no velocities invented).
    assert [n.to_dict() for n in fused.notes] == [n.to_dict() for n in visual.notes]


def test_fuse_chart_default_velocity_for_unmatched():
    # A chart with a pitch the audio never plays -> that note gets DEFAULT.
    notes, cfg = c_major_demo()
    clean = render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    chart = ExtractedChart(
        notes=[ExtractedNote(pitch=36, start_us=0, dur_us=300_000, hand=Hand.LEFT)],
        source=SourceMeta(fps=cfg.fps),
    )
    fused = audio.fuse_chart(chart, clean, DEFAULT_SAMPLE_RATE)
    assert fused.notes[0].velocity == audio.DEFAULT_VELOCITY


# --------------------------------------------------------------------------- #
# WAV round trip + JSON shape
# --------------------------------------------------------------------------- #
def test_wav_round_trip(tmp_path):
    notes, _ = c_major_demo()
    clean = render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    path = str(tmp_path / "clip.wav")
    write_wav(path, clean, DEFAULT_SAMPLE_RATE)
    back, sr = read_wav(path)
    assert sr == DEFAULT_SAMPLE_RATE
    assert back.shape[0] == clean.shape[0]
    assert np.max(np.abs(back - clean)) < 1e-3  # 16-bit quantisation only


def test_fused_json_shape_includes_velocity():
    notes, cfg = c_major_demo()
    frames, _ = render_frames(notes, cfg)
    visual = extract_chart(frames, cfg.fps)
    clean = render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE)
    fused = audio.fuse_chart(copy.deepcopy(visual), clean, DEFAULT_SAMPLE_RATE)

    data = json.loads(fused.to_json())
    assert "audio_fusion" in data["source"]
    for n in data["notes"]:
        assert set(n.keys()) <= {"pitch", "start_us", "dur_us", "hand", "velocity", "confidence"}
        assert "velocity" in n  # fusion populated it
        assert 0 <= n["velocity"] <= 127


# --------------------------------------------------------------------------- #
# CLI: frames dir + WAV, hermetic (no video/audio codec needed)
# --------------------------------------------------------------------------- #
def test_cli_audio_fusion(tmp_path):
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)

    frames_dir = tmp_path / "frames"
    frames_dir.mkdir()
    import cv2

    for i, f in enumerate(frames):
        cv2.imwrite(str(frames_dir / f"frame_{i:05d}.png"), f)

    wav = tmp_path / "clip.wav"
    write_wav(str(wav), render_audio(notes, sample_rate=DEFAULT_SAMPLE_RATE), DEFAULT_SAMPLE_RATE)

    out = tmp_path / "chart.json"
    result = subprocess.run(
        [
            sys.executable,
            os.path.join(ROOT, "extract.py"),
            "--in", str(frames_dir),
            "--fps", str(cfg.fps),
            "--audio-fusion",
            "--audio", str(wav),
            "--out", str(out),
        ],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )
    assert result.returncode == 0, result.stderr
    assert "audio-fusion: applied" in result.stderr
    data = json.loads(out.read_text())
    assert all("velocity" in n for n in data["notes"])


def test_cli_audio_fusion_requires_audio(tmp_path):
    cfg = SynthConfig()
    notes, _ = c_major_demo(cfg)
    frames, _ = render_frames(notes, cfg)
    frames_dir = tmp_path / "frames"
    frames_dir.mkdir()
    import cv2

    for i, f in enumerate(frames):
        cv2.imwrite(str(frames_dir / f"frame_{i:05d}.png"), f)

    result = subprocess.run(
        [
            sys.executable,
            os.path.join(ROOT, "extract.py"),
            "--in", str(frames_dir),
            "--fps", str(cfg.fps),
            "--audio-fusion",
        ],
        capture_output=True,
        text=True,
        cwd=ROOT,
    )
    assert result.returncode == 2
    assert "requires --audio" in result.stderr
