"""Tempo-map detector tests — synthetic click tracks only (no real media)."""

from __future__ import annotations

import json
import os
import sys
import wave

import numpy as np
import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
import tempo_map as tm  # noqa: E402

SR = 22050


def click_track(times_s, amps=None, dur_s=None, freq=None, seed=0):
    """Decaying tone bursts at ``times_s`` plus a little noise."""
    times_s = np.asarray(times_s, float)
    amps = np.ones(len(times_s)) if amps is None else np.asarray(amps, float)
    total = dur_s if dur_s is not None else times_s[-1] + 1.5
    x = np.random.default_rng(seed).normal(0, 0.002, int(total * SR)).astype(np.float32)
    t = np.arange(int(0.12 * SR)) / SR
    for i, (ts, a) in enumerate(zip(times_s, amps)):
        f = freq[i] if freq is not None else 880.0
        burst = a * np.sin(2 * np.pi * f * t) * np.exp(-t / 0.03)
        s = int(ts * SR)
        x[s : s + len(burst)] += burst[: len(x) - s]
    return x


def steady(bpm, n, start=1.0):
    return start + np.arange(n) * 60.0 / bpm


def match_err_ms(detected_us, truth_s):
    d = np.asarray(detected_us) / 1e6
    return np.array([np.min(np.abs(d - t)) for t in truth_s]) * 1e3


def test_steady_100bpm_beats_and_tempo():
    truth = steady(100, 64)
    out = tm.detect(click_track(truth), SR)
    assert out["bpm"] == pytest.approx(100, abs=1.5)
    # Every true beat except the edges is found within 15 ms.
    assert np.median(match_err_ms(out["beats_us"], truth[2:-2])) < 15
    assert np.percentile(match_err_ms(out["beats_us"], truth[2:-2]), 90) < 20


def test_accelerating_track_follows_the_tempo():
    # 90 -> 110 BPM linearly over 80 beats.
    ibi = 60.0 / np.linspace(90, 110, 80)
    truth = 1.0 + np.concatenate([[0], np.cumsum(ibi[:-1])])
    out = tm.detect(click_track(truth), SR, beats_per_bar=4)
    assert np.percentile(match_err_ms(out["beats_us"], truth[2:-2]), 90) < 20
    bars = np.diff(out["bars_us"])
    # Bar durations shrink as the tempo rises: late bars clearly shorter.
    assert bars[-3:].mean() < 0.9 * bars[:3].mean()


def test_accented_eighths_track_quarter_beats():
    # Eighths at 172/min (86 BPM quarters); on-beat clicks louder and lower.
    eighths = steady(172, 160)
    amps = np.where(np.arange(160) % 2 == 0, 1.0, 0.45)
    freq = np.where(np.arange(160) % 2 == 0, 440.0, 1320.0)
    out = tm.detect(click_track(eighths, amps, freq=freq), SR)
    assert out["bpm"] == pytest.approx(86, abs=2)
    # The beats are the accented (even) eighths, not the off-beats.
    assert np.median(match_err_ms(out["beats_us"], eighths[4:-4:2])) < 15


def test_fold_decision_uses_local_accent_contrast():
    fr = 100.0
    pulses = np.arange(0, 6400, 35)  # ~171 pulses/min at 100 frames/s
    env = np.zeros(6500)
    strong = np.where(np.arange(len(pulses)) % 2 == 0, 1.0, 0.6)
    # The accent parity flips halfway (a tracker slip) - globally it cancels.
    half = len(pulses) // 2
    strong[half:] = np.where(np.arange(len(pulses) - half) % 2 == 0, 0.6, 1.0)
    env[pulses] = strong
    assert tm.pulses_per_beat_of(pulses, env, fr) == 2
    env[pulses] = 1.0  # no accents: keep the pulse
    assert tm.pulses_per_beat_of(pulses, env, fr) == 1
    slow = np.arange(0, 6400, 70)  # ~86/min: already a beat rate
    env2 = np.zeros(6500)
    env2[slow] = np.where(np.arange(len(slow)) % 2 == 0, 1.0, 0.5)
    assert tm.pulses_per_beat_of(slow, env2, fr) == 1


@pytest.mark.parametrize("hint", [86, 172])
def test_tempo_hint_picks_the_octave(hint):
    eighths = steady(172, 160)
    out = tm.detect(click_track(eighths), SR, tempo_hint=hint)
    assert out["bpm"] == pytest.approx(hint, rel=0.03)


def test_anchor_selects_the_bar_phase():
    truth = steady(120, 48)
    x = click_track(truth)
    for k in range(4):
        anchor = int(truth[8 + k] * 1e6)
        out = tm.detect(x, SR, beats_per_bar=4, anchor_us=anchor)
        assert abs(out["anchor_us"] - anchor) < 20_000
        # The anchor is one of the bar lines.
        assert min(abs(b - anchor) for b in out["bars_us"]) < 20_000


def test_bass_accent_picks_the_downbeat_without_anchor():
    truth = steady(100, 48)
    downbeat = np.arange(48) % 4 == 1  # bars start on the 2nd click
    freq = np.where(downbeat, 110.0, 1760.0)
    out = tm.detect(click_track(truth, freq=freq), SR, beats_per_bar=4)
    assert min(abs(out["anchor_us"] / 1e6 - t) for t in truth[downbeat]) < 0.02


def test_bars_are_ascending_and_extend_past_the_last_beat():
    truth = steady(95, 50)
    out = tm.detect(click_track(truth), SR, beats_per_bar=3)
    bars = out["bars_us"]
    assert len(bars) >= 2
    assert all(b2 > b1 for b1, b2 in zip(bars, bars[1:]))
    assert bars[-1] > out["beats_us"][-1]
    # Bar lines are every 3rd beat.
    beats = out["beats_us"]
    assert beats.index(bars[1]) - beats.index(bars[0]) == 3


def test_accentless_noise_still_returns_a_valid_map():
    x = np.random.default_rng(3).normal(0, 0.1, SR * 20).astype(np.float32)
    out = tm.detect(x, SR)
    bars = out["bars_us"]
    assert all(b2 > b1 for b1, b2 in zip(bars, bars[1:]))


def test_cli_reads_a_16bit_stereo_wav(tmp_path):
    truth = steady(110, 40)
    x = click_track(truth)
    pcm = (np.clip(x, -1, 1) * 32767).astype("<i2")
    stereo = np.repeat(pcm[:, None], 2, axis=1)
    path = tmp_path / "t.wav"
    with wave.open(str(path), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(stereo.tobytes())
    out_path = tmp_path / "map.json"
    assert tm.main(["--in", str(path), "--out", str(out_path)]) == 0
    out = json.loads(out_path.read_text())
    assert out["version"] == 1
    assert out["bpm"] == pytest.approx(110, abs=1.5)
