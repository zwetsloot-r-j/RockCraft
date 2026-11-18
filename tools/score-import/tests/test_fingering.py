"""Tests for reading finger numbers from MusicXML fingering marks (M27-D).

The fixture, ``fixtures/score/fingering.musicxml``, is synthetic: a handful of
made-up notes and chords carrying each kind of fingering mark. A note gets a
finger only where the score states exactly one for it, 1 to 5; anything unclear
gets none.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(ROOT))
FIXTURES = os.path.join(REPO, "fixtures", "score")
sys.path.insert(0, ROOT)

from score_import.convert import convert_score  # noqa: E402
from score_import.schema import ExtractedNote  # noqa: E402

FIXTURE = os.path.join(FIXTURES, "fingering.musicxml")
Q = 500_000  # one quarter note at the default 120 BPM, in µs


def fingers(**kwargs) -> dict[tuple[int, int], int | None]:
    """``(pitch, start_us) -> finger`` for the fingering fixture."""
    chart, _ = convert_score(FIXTURE, **kwargs)
    return {(n.pitch, n.start_us): n.finger for n in chart.notes}


def test_single_notes_take_their_fingering():
    got = fingers()
    assert got[(60, 0)] == 1
    assert got[(62, Q)] == 2


def test_a_fully_fingered_chord_is_assigned_note_by_note():
    got = fingers()
    assert got[(64, 2 * Q)] == 1, "E4, the chord's first written note"
    assert got[(67, 2 * Q)] == 3, "G4, its second"


def test_a_partly_fingered_chord_gets_none():
    """One fingering on a two-note chord: no telling which note it is for."""
    got = fingers()
    assert got[(65, 3 * Q)] is None
    assert got[(69, 3 * Q)] is None


def test_a_tied_note_keeps_the_fingering_of_its_first_segment():
    chart, _ = convert_score(FIXTURE)
    c5 = [n for n in chart.notes if n.pitch == 72]
    assert len(c5) == 1, "the tied pair is one note"
    assert c5[0].dur_us == 4 * Q
    assert c5[0].finger == 5


def test_substitution_alternate_and_out_of_range_fingerings_get_none():
    got = fingers()
    assert got[(64, 6 * Q)] is None, "2-1 substitution: two fingers, not one"
    assert got[(67, 10 * Q)] is None, "an alternate fingering is not the fingering"
    assert got[(71, 11 * Q)] is None, "7 is not a finger"


def test_an_unfingered_note_gets_none():
    assert fingers()[(65, 7 * Q)] is None


def test_no_fingering_reads_none_and_changes_nothing_else():
    with_f, _ = convert_score(FIXTURE)
    without, _ = convert_score(FIXTURE, read_fingering=False)
    assert all(n.finger is None for n in without.notes)
    strip = lambda notes: [(n.pitch, n.start_us, n.dur_us, n.hand, n.velocity) for n in notes]
    assert strip(with_f.notes) == strip(without.notes)


def test_scores_without_fingering_marks_yield_no_fingers():
    for name in ("single_staff_scale", "two_staff_chords", "tied_note"):
        chart, _ = convert_score(os.path.join(FIXTURES, f"{name}.musicxml"))
        assert all(n.finger is None for n in chart.notes), name
        assert all("finger" not in n.to_dict() for n in chart.notes), name


def test_finger_is_written_only_when_set():
    assert "finger" not in ExtractedNote(pitch=60, start_us=0, dur_us=1).to_dict()
    assert ExtractedNote(pitch=60, start_us=0, dur_us=1, finger=4).to_dict()["finger"] == 4


def run_cli(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, os.path.join(ROOT, "convert.py"), *args],
        capture_output=True,
        text=True,
    )


def test_cli_writes_fingers_and_no_fingering_omits_them():
    result = run_cli("--in", FIXTURE, "--out", "-")
    assert result.returncode == 0, result.stderr
    notes = json.loads(result.stdout)["notes"]
    assert [n["finger"] for n in notes if "finger" in n] == [1, 2, 1, 3, 5]

    result = run_cli("--in", FIXTURE, "--out", "-", "--no-fingering")
    assert result.returncode == 0, result.stderr
    assert all("finger" not in n for n in json.loads(result.stdout)["notes"])
