"""Tests for the M13-A score-file converter.

Every fixture under ``fixtures/score/`` is *synthetic* — a hand-written scale, a
fabricated two-hand chord — never a real or copyrighted piece.

Unlike the M6-C visual extractor, this transform is **deterministic**: the source
states pitch, duration, tempo, metre and key outright. So the assertions here are
exact to the microsecond, with no tolerance windows anywhere.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

import pytest

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(ROOT))
FIXTURES = os.path.join(REPO, "fixtures", "score")
sys.path.insert(0, ROOT)

from score_import.convert import DEFAULT_BPM, convert_score, parse_hand_map  # noqa: E402
from score_import.schema import Hand  # noqa: E402


def fixture(name: str) -> str:
    return os.path.join(FIXTURES, f"{name}.musicxml")


def convert(name: str, **kwargs):
    return convert_score(fixture(name), **kwargs)


def notes_of(name: str, **kwargs):
    chart, _ = convert(name, **kwargs)
    return chart.notes


def warnings_of(report) -> str:
    return " | ".join(report.warnings)


# ── Pitch and timing: exact, to the microsecond ──────────────────────────────


def test_scale_pitches_onsets_and_durations_are_exact():
    """A C-major scale at 120 BPM: 8 quarter notes, each exactly 500 000 µs."""
    notes = notes_of("single_staff_scale")
    assert [n.pitch for n in notes] == [60, 62, 64, 65, 67, 69, 71, 72]
    assert [n.start_us for n in notes] == [i * 500_000 for i in range(8)]
    assert {n.dur_us for n in notes} == {500_000}


def test_every_note_is_fully_confident_and_velocity_free():
    """The transform is exact, and the source states no velocity."""
    for note in notes_of("single_staff_scale"):
        assert note.confidence == 1.0
        assert note.velocity is None


def test_single_note_score_converts():
    notes = notes_of("single_note")
    assert len(notes) == 1
    assert (notes[0].pitch, notes[0].start_us, notes[0].dur_us) == (60, 0, 2_000_000)


def test_empty_score_does_not_crash():
    chart, _ = convert("empty")
    assert chart.notes == []
    assert chart.notation is not None


# ── Hands ────────────────────────────────────────────────────────────────────


def test_two_staff_score_maps_upper_right_and_lower_left():
    notes = notes_of("two_staff_chords")
    right = sorted(n.pitch for n in notes if n.hand is Hand.RIGHT)
    left = sorted(n.pitch for n in notes if n.hand is Hand.LEFT)
    assert right == [67, 69, 71, 74], "upper staff is the right hand"
    assert left == [43, 50], "lower staff is the left hand"


def test_single_staff_score_leaves_hands_unknown():
    """One staff carries no hand information — it must not be guessed."""
    assert {n.hand for n in notes_of("single_staff_scale")} == {Hand.UNKNOWN}


def test_hand_map_overrides_the_staff_heuristic():
    notes = notes_of("two_staff_chords", hand_map={0: Hand.LEFT, 1: Hand.RIGHT})
    assert sorted(n.pitch for n in notes if n.hand is Hand.LEFT) == [67, 69, 71, 74]
    assert sorted(n.pitch for n in notes if n.hand is Hand.RIGHT) == [43, 50]


def test_hand_map_parsing_rejects_nonsense():
    assert parse_hand_map("0=right,1=left") == {0: Hand.RIGHT, 1: Hand.LEFT}
    assert parse_hand_map(None) == {}
    for bad in ("0", "0=middle", "x=left"):
        with pytest.raises(ValueError):
            parse_hand_map(bad)


# ── Chords, ties, repeats, grace notes ───────────────────────────────────────


def test_chord_becomes_one_note_per_pitch_sharing_timing():
    notes = [n for n in notes_of("two_staff_chords") if n.start_us == 0 and n.hand is Hand.RIGHT]
    assert sorted(n.pitch for n in notes) == [67, 71, 74], "G major triad, one note per pitch"
    assert len({(n.start_us, n.dur_us) for n in notes}) == 1, "chord tones share start and duration"


def test_tied_pair_becomes_one_note_of_the_summed_duration():
    """Two tied half notes are one 4-quarter note — not two, and not merged with
    the untied same-pitch note that precedes them."""
    notes = notes_of("tied_note")
    c4 = [n for n in notes if n.pitch == 60]
    assert len(c4) == 2, "the untied half stays separate from the tied pair"
    assert c4[0].dur_us == 1_000_000, "untied half note keeps its own length"
    assert c4[1].dur_us == 2_000_000, "the tied pair sums to four quarters"


def test_repeated_bar_unrolls_to_double_the_notes():
    notes = notes_of("repeated_bar")
    assert [n.pitch for n in notes] == [60, 64, 60, 64]
    assert [n.start_us for n in notes] == [0, 1_000_000, 2_000_000, 3_000_000]


def test_grace_note_is_dropped_and_counted():
    chart, report = convert("grace_note")
    assert [n.pitch for n in chart.notes] == [60], "only the principal note survives"
    assert report.dropped == {"grace note(s)": 1}
    assert any("grace note(s)" in line for line in report.lines())


# ── Notated context ──────────────────────────────────────────────────────────


def test_notation_matches_the_fixture_metre_tempo_and_key():
    chart, _ = convert("two_staff_chords")
    assert chart.notation.bpm == 90
    assert chart.notation.time_sig.beats_per_bar == 3
    assert chart.notation.time_sig.beat_unit == 4
    assert chart.notation.key.root_pc == 7, "G"
    assert chart.notation.key.scale.value == "Major"


def test_score_without_a_tempo_mark_assumes_120_and_warns():
    chart, report = convert("no_tempo")
    assert chart.notation.bpm == DEFAULT_BPM
    assert "no tempo mark" in warnings_of(report)
    assert chart.notes, "the score still converts"


def test_tempo_change_moves_later_notes_and_warns_loudly():
    """Note times honour both tempos; the single-BPM grid limitation is warned."""
    chart, report = convert("two_tempos")
    starts = [n.start_us for n in chart.notes]
    durations = [n.dur_us for n in chart.notes]
    assert starts == [0, 4_000_000], "the second note starts after 4 s at 60 BPM"
    assert durations == [4_000_000, 2_000_000], "the second whole note is 120 BPM long"
    assert "more than one tempo" in warnings_of(report)
    assert chart.notation.bpm == 60, "the grid takes the tempo at the start of the piece"


def test_key_signature_without_a_mode_is_omitted_not_guessed():
    chart, report = convert("key_without_mode")
    assert chart.notation.key is None
    assert "no mode" in warnings_of(report)


def test_source_meta_names_the_score_converter():
    chart, _ = convert("single_staff_scale")
    assert chart.source.extractor_version == "score-import-0.1"
    assert "fixture" in (chart.source.title or "")


# ── Emitted JSON: the M6-A wire contract ─────────────────────────────────────

#: The exact key sets `crates/import/src/schema.rs` deserializes. Asserting the
#: sets (rather than restating the schema) keeps this honest without maintaining
#: a second copy of it.
CHART_KEYS = {"notes", "source", "notation"}
NOTE_KEYS = {"pitch", "start_us", "dur_us", "hand", "velocity", "confidence"}
NOTE_REQUIRED = {"pitch", "start_us", "dur_us", "hand"}
SOURCE_KEYS = {"extractor_version", "title"}
NOTATION_KEYS = {"bpm", "time_sig", "key"}


def test_emitted_json_matches_the_m6a_contract():
    chart, _ = convert("two_staff_chords")
    payload = json.loads(chart.to_json())

    assert set(payload) <= CHART_KEYS
    assert {"notes", "source"} <= set(payload), "notes and source are mandatory"
    assert set(payload["source"]) <= SOURCE_KEYS
    assert "extractor_version" in payload["source"]
    assert set(payload["notation"]) <= NOTATION_KEYS
    assert set(payload["notation"]["time_sig"]) == {"beats_per_bar", "beat_unit"}
    assert set(payload["notation"]["key"]) == {"root_pc", "scale"}

    for note in payload["notes"]:
        assert set(note) <= NOTE_KEYS
        assert NOTE_REQUIRED <= set(note)
        assert note["hand"] in ("Left", "Right", "Unknown")
        assert 0 <= note["pitch"] <= 127
        assert note["dur_us"] >= 1


def test_omitted_optionals_are_absent_rather_than_null():
    """Serde's `skip_serializing_if = Option::is_none` mirrored on this side."""
    payload = json.loads(convert("single_staff_scale")[0].to_json())
    assert "velocity" not in payload["notes"][0]
    assert "key" not in json.loads(convert("key_without_mode")[0].to_json())["notation"]


# ── CLI ──────────────────────────────────────────────────────────────────────


def run_cli(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, os.path.join(ROOT, "convert.py"), *args],
        capture_output=True,
        text=True,
        check=False,
    )


def test_cli_writes_only_json_to_stdout():
    """`run_sidecar` parses stdout, so every warning must go to stderr."""
    result = run_cli("--in", fixture("two_tempos"), "--out", "-")
    assert result.returncode == 0
    payload = json.loads(result.stdout)  # would raise if a warning leaked here
    assert len(payload["notes"]) == 2
    assert "more than one tempo" in result.stderr


def test_cli_writes_to_a_file(tmp_path):
    out = tmp_path / "chart.json"
    result = run_cli("--in", fixture("single_note"), "--out", str(out))
    assert result.returncode == 0
    assert json.loads(out.read_text())["notes"][0]["pitch"] == 60


def test_cli_reports_an_unreadable_score_without_writing_json():
    result = run_cli("--in", os.path.join(FIXTURES, "does-not-exist.musicxml"), "--out", "-")
    assert result.returncode == 2
    assert result.stdout.strip() == ""
    assert "error" in result.stderr.lower()


def test_cli_rejects_a_malformed_hand_map():
    result = run_cli("--in", fixture("single_note"), "--out", "-", "--hand-map", "0=middle")
    assert result.returncode == 2
    assert "hand" in result.stderr.lower()
