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


# --------------------------------------------------------------------------- #
# Iteration 3: clock alignment + merged-note splitting (issue #151 follow-up)
# --------------------------------------------------------------------------- #
def _visual_chart(notes, fps=30.0):
    return ExtractedChart(
        notes=[
            ExtractedNote(
                pitch=n.pitch, start_us=n.start_us, dur_us=n.dur_us,
                hand=Hand.UNKNOWN, confidence=0.9,
            )
            for n in notes
        ],
        source=SourceMeta(fps=fps),
    )


def test_global_offset_aligns_visual_clock():
    """A constant visual lead/lag must be measured and removed for every note,
    matched or not."""
    notes, _ = c_major_demo()
    lead_us = 120_000
    base_us = 300_000  # keep every shifted onset positive (no clamping)
    # estimate_global_offset needs >= 30 matches; repeat the demo notes by
    # tiling them later in time (audio re-rendered to match).
    tiled = []
    tiled_shifted = []
    span = max(n.start_us + n.dur_us for n in notes) + 500_000
    for k in (0, 1, 2):
        for n in notes:
            t = base_us + n.start_us + k * span
            tiled.append(SynthNote(n.pitch, t, n.dur_us, n.hand, n.velocity))
            tiled_shifted.append(
                SynthNote(n.pitch, t - lead_us, n.dur_us, n.hand, n.velocity)
            )
    track = render_audio(tiled, sample_rate=DEFAULT_SAMPLE_RATE)
    chart = _visual_chart(tiled_shifted)
    fused = audio.fuse_chart(chart, track, DEFAULT_SAMPLE_RATE)
    assert "offset +1" in fused.source.audio_fusion  # ~ +120 ms
    for gt in tiled:
        m = _nearest(fused.notes, gt.pitch, gt.start_us)
        assert m is not None
        assert abs(m.start_us - gt.start_us) <= 40_000, (gt.pitch, gt.start_us, m.start_us)


def test_split_merged_repeated_notes():
    """Two same-pitch strikes the roll merged into one long note are split at
    the audible second onset, each side keeping its own velocity."""
    truth = [
        SynthNote(60, 100_000, 350_000, "Right", velocity=96),
        SynthNote(60, 600_000, 350_000, "Right", velocity=52),
    ]
    track = render_audio(truth, sample_rate=DEFAULT_SAMPLE_RATE)
    merged = [SynthNote(60, 100_000, 850_000, "Right")]  # one held visual note
    fused = audio.fuse_chart(_visual_chart(merged), track, DEFAULT_SAMPLE_RATE)
    sixty = [n for n in fused.notes if n.pitch == 60]
    assert len(sixty) == 2, fused.source.audio_fusion
    assert "split 1 merged notes" in fused.source.audio_fusion
    first, second = sixty
    assert abs(first.start_us - 100_000) <= 40_000
    assert abs(second.start_us - 600_000) <= 40_000
    assert abs(first.velocity - 96) <= VEL_TOL
    assert abs(second.velocity - 52) <= VEL_TOL


def test_split_vetoes_harmonic_bleed():
    """A lower note's harmonic landing in a held note's band is bleed, not a
    re-strike: the held note must stay whole."""
    # Audio: C4 strikes briefly at t=0; C3 (one octave below, strong harmonics)
    # strikes at 600 ms while the *visual* C4 bar is still held.
    audio_truth = [
        SynthNote(60, 0, 300_000, "Right", velocity=80),
        SynthNote(48, 600_000, 300_000, "Left", velocity=100),
    ]
    track = render_audio(
        audio_truth, sample_rate=DEFAULT_SAMPLE_RATE, harmonics=(1.0, 0.5)
    )
    held = [SynthNote(60, 0, 1_100_000, "Right")]
    fused = audio.fuse_chart(_visual_chart(held), track, DEFAULT_SAMPLE_RATE)
    sixty = [n for n in fused.notes if n.pitch == 60]
    assert len(sixty) == 1, [(n.start_us, n.dur_us) for n in sixty]


def test_decimation_preserves_transcription():
    """48 kHz input is decimated for speed without losing the notes."""
    notes, _ = c_major_demo()
    hi_rate = 48_000
    track = render_audio(notes, sample_rate=hi_rate)
    chart = _visual_chart(notes)
    fused = audio.fuse_chart(chart, track, hi_rate)
    assert fused.source.audio_fusion.startswith("applied")
    for gt in notes:
        m = _nearest(fused.notes, gt.pitch, gt.start_us)
        assert m is not None
        assert abs(m.velocity - gt.velocity) <= VEL_TOL + 2


# --------------------------------------------------------------------------- #
# eval.py: the accuracy harness scores known discrepancies correctly
# --------------------------------------------------------------------------- #
def test_eval_buckets_known_discrepancies():
    sys.path.insert(0, ROOT)
    from eval import evaluate

    ref = [
        {"pitch": 60, "start_us": 1_000_000, "dur_us": 200_000},
        {"pitch": 62, "start_us": 1_500_000, "dur_us": 200_000},
        {"pitch": 64, "start_us": 2_000_000, "dur_us": 200_000},  # chart has 76
        {"pitch": 65, "start_us": 2_500_000, "dur_us": 200_000},  # chart lacks it
        {"pitch": 67, "start_us": 3_000_000, "dur_us": 200_000},  # buried repeat
        {"pitch": 67, "start_us": 3_400_000, "dur_us": 200_000},
    ]
    chart = [
        {"pitch": 60, "start_us": 1_010_000, "dur_us": 200_000},
        {"pitch": 62, "start_us": 1_490_000, "dur_us": 200_000},
        {"pitch": 76, "start_us": 2_000_000, "dur_us": 200_000},
        {"pitch": 67, "start_us": 3_000_000, "dur_us": 620_000},  # merged pair
        {"pitch": 90, "start_us": 5_000_000, "dur_us": 200_000},  # chart-only
    ]
    r = evaluate(chart, ref, tol_us=150_000)
    assert r["exact"] == 3  # 60, 62, first 67
    assert r["octave"] == 1  # 64 vs 76
    assert r["missing"] == 2  # 65, second 67
    assert r["buried_onsets"] == 1  # second 67 inside the merged note
    assert r["chart_only"] == 1  # pitch 90


def test_split_pedal_sustained_repeats():
    """Sustain (overlapping same-pitch audio) keeps the envelope from dipping
    between strikes; the mid-run flux onset must still find the re-strike and
    split the merged visual note."""
    truth = [
        SynthNote(60, 100_000, 700_000, "Right", velocity=90),  # rings past...
        SynthNote(60, 550_000, 400_000, "Right", velocity=88),  # ...this strike
    ]
    track = render_audio(truth, sample_rate=DEFAULT_SAMPLE_RATE)
    merged = [SynthNote(60, 100_000, 850_000, "Right")]
    fused = audio.fuse_chart(_visual_chart(merged), track, DEFAULT_SAMPLE_RATE)
    sixty = [n for n in fused.notes if n.pitch == 60]
    assert len(sixty) == 2, fused.source.audio_fusion
    assert abs(sixty[1].start_us - 550_000) <= 40_000
