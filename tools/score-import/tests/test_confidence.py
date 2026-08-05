"""Tests for the M13-B confidence heuristics.

The point of this file is that **no OMR engine runs here**. Confidence is a pure
function over a parsed score, so every rule is exercised against a hand-built
``music21`` score — which is what lets the highest-risk part of the OMR path be
covered in CI on a machine with no engine, no model weights and no scans.

Scores are built in code rather than loaded from fixtures because the interesting
inputs are *malformed* — a bar holding three beats of a four-beat metre — and a
malformed MusicXML fixture is harder to read than the four lines that build one.
"""

from __future__ import annotations

import os
import sys

from music21 import chord, meter, note, stream

HERE = os.path.dirname(__file__)
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from score_import import confidence as conf  # noqa: E402
from score_import.convert import convert_score  # noqa: E402
from score_import.schema import Hand  # noqa: E402


# ── Score builders ───────────────────────────────────────────────────────────


def measure(number: int, pitches_and_lengths, *, time_sig: str | None = None) -> stream.Measure:
    """A measure holding `pitches_and_lengths` laid end to end from offset 0."""
    m = stream.Measure(number=number)
    if time_sig:
        m.insert(0, meter.TimeSignature(time_sig))
    offset = 0.0
    for pitch, length in pitches_and_lengths:
        element = note.Rest(quarterLength=length) if pitch is None else note.Note(pitch, quarterLength=length)
        m.insert(offset, element)
        offset += length
    return m


def score_of(*measures: stream.Measure) -> stream.Score:
    part = stream.Part()
    for m in measures:
        part.append(m)
    s = stream.Score()
    s.insert(0, part)
    return s


def confidences(score: stream.Score) -> list[float]:
    """Confidence per emitted note, in the order the audit's policy scores them."""
    audit = conf.audit(score)
    out = []
    for element in score.flatten().notes:
        pitches = element.pitches if isinstance(element, chord.Chord) else (element.pitch,)
        out.extend(audit.policy.confidence_for(element, int(p.midi)) for p in pitches)
    return out


# ── Measure-fill check: the highest-yield OMR error detector ──────────────────


def test_a_four_four_measure_holding_three_beats_flags_all_its_notes():
    score = score_of(measure(1, [("C4", 1.0), ("D4", 1.0), ("E4", 1.0)], time_sig="4/4"))
    assert confidences(score) == [conf.SUSPECT_MEASURE] * 3
    assert conf.audit(score).reasons == {1: conf.REASON_SHORT}


def test_a_clean_score_flags_nothing_and_never_claims_certainty():
    score = score_of(
        measure(1, [("C4", 1.0), ("D4", 1.0), ("E4", 1.0), ("F4", 1.0)], time_sig="4/4"),
        measure(2, [("G4", 4.0)]),
    )
    assert confidences(score) == [conf.CLEAN] * 5
    assert conf.audit(score).reasons == {}
    assert conf.CLEAN < 1.0, "1.0 belongs to M13-A's exact transform, not to OMR"


def test_only_the_doubtful_measure_is_flagged():
    """A short bar must not drag its neighbours down with it."""
    score = score_of(
        measure(1, [("C4", 1.0), ("D4", 1.0), ("E4", 1.0), ("F4", 1.0)], time_sig="4/4"),
        measure(2, [("G4", 1.0), ("A4", 1.0)]),
        measure(3, [("B4", 4.0)]),
    )
    assert confidences(score) == [conf.CLEAN] * 4 + [conf.SUSPECT_MEASURE] * 2 + [conf.CLEAN]
    assert conf.audit(score).suspect_measures == [2]


def test_an_overfull_measure_is_flagged_too():
    """Five quarters in 4/4 — an engine that doubled a note or lost a barline."""
    score = score_of(
        measure(1, [("C4", 1.0)] * 5, time_sig="4/4"),
    )
    assert conf.audit(score).reasons == {1: conf.REASON_OVERFULL}
    assert confidences(score) == [conf.SUSPECT_MEASURE] * 5


def test_rests_fill_a_measure():
    """A bar is judged on whether its *contents* reach the bar line, not on how
    many notes it holds — two beats of notes plus two of rest is a full bar."""
    score = score_of(measure(1, [("C4", 1.0), ("D4", 1.0), (None, 2.0)], time_sig="4/4"))
    assert conf.audit(score).reasons == {}


def test_a_gap_inside_a_measure_is_flagged_even_though_it_reaches_the_bar_line():
    """A dropped note leaves a hole. The last note still ends on the bar line, so
    only a coverage check — not a highest-time check — catches this."""
    m = stream.Measure(number=1)
    m.insert(0, meter.TimeSignature("4/4"))
    m.insert(0.0, note.Note("C4", quarterLength=1.0))
    m.insert(3.0, note.Note("D4", quarterLength=1.0))
    assert conf.audit(score_of(m)).reasons == {1: conf.REASON_SHORT}


def test_a_chord_does_not_read_as_an_overfull_measure():
    """Four voices' durations sum to 16 quarters in a 4/4 bar; the union is 4."""
    m = stream.Measure(number=1)
    m.insert(0, meter.TimeSignature("4/4"))
    m.insert(0.0, chord.Chord(["C4", "E4", "G4"], quarterLength=4.0))
    assert conf.audit(score_of(m)).reasons == {}
    assert confidences(score_of(m)) == [conf.CLEAN] * 3


def test_a_note_tied_over_the_barline_does_not_flag_either_measure():
    """M13-A merges ties, so the merged note sits in the first bar and overruns
    it. Both bars must still read as full — otherwise every tie is a false
    positive, and ties are everywhere."""
    first = measure(1, [("C4", 2.0), ("D4", 2.0)], time_sig="4/4")
    second = measure(2, [("E4", 2.0), ("F4", 2.0)])
    # Simulate the post-`stripTies` shape: D4 is merged across the barline, so it
    # runs 4 quarters from offset 2 and measure 2 starts 2 quarters in.
    first.getElementsByClass(note.Note)[1].quarterLength = 4.0
    second.remove(second.getElementsByClass(note.Note)[0])
    assert conf.audit(score_of(first, second)).reasons == {}


def test_a_declared_pickup_bar_is_not_flagged():
    """Padding is a score *declaring* a partial bar. An OMR engine that does not
    declare one gets its short first bar flagged, which is the safe direction."""
    pickup = measure(1, [("C4", 1.0)], time_sig="4/4")
    pickup.paddingLeft = 3.0
    assert conf.audit(score_of(pickup)).reasons == {}

    undeclared = measure(1, [("C4", 1.0)], time_sig="4/4")
    assert conf.audit(score_of(undeclared)).reasons == {1: conf.REASON_SHORT}


def test_an_empty_measure_reports_no_flagged_notes():
    """Nothing to lower the confidence of, so nothing is reported — the summary's
    measure list must never name a bar that contributed zero flagged notes."""
    empty = stream.Measure(number=2)
    empty.insert(0, meter.TimeSignature("4/4"))
    score = score_of(measure(1, [("C4", 4.0)], time_sig="4/4"), empty)
    assert conf.audit(score).reasons == {}


def test_each_staff_is_judged_on_its_own():
    """A doubtful left-hand bar must not flag the right hand's notes."""
    right = stream.Part()
    right.append(measure(1, [("C5", 4.0)], time_sig="4/4"))
    left = stream.Part()
    left.append(measure(1, [("C3", 1.0)], time_sig="4/4"))
    score = stream.Score()
    score.insert(0, right)
    score.insert(0, left)

    audit = conf.audit(score)
    right_note = right.flatten().notes[0]
    left_note = left.flatten().notes[0]
    assert audit.policy.confidence_for(right_note, 72) == conf.CLEAN
    assert audit.policy.confidence_for(left_note, 48) == conf.SUSPECT_MEASURE


# ── Range check ──────────────────────────────────────────────────────────────


def test_a_pitch_above_c8_is_flagged_as_out_of_range():
    score = score_of(measure(1, [("C4", 4.0)], time_sig="4/4"))
    element = score.flatten().notes[0]
    policy = conf.audit(score).policy
    assert policy.confidence_for(element, 109) == conf.OUT_OF_RANGE
    assert policy.confidence_for(element, conf.PITCH_CEILING) == conf.CLEAN


def test_a_pitch_below_a0_is_flagged_as_out_of_range():
    score = score_of(measure(1, [("C4", 4.0)], time_sig="4/4"))
    element = score.flatten().notes[0]
    policy = conf.audit(score).policy
    assert policy.confidence_for(element, 20) == conf.OUT_OF_RANGE
    assert policy.confidence_for(element, conf.PITCH_FLOOR) == conf.CLEAN


def test_the_strictest_rule_wins():
    """An out-of-range note in a short bar takes the lower of the two."""
    score = score_of(measure(1, [("C4", 1.0)], time_sig="4/4"))
    element = score.flatten().notes[0]
    assert conf.audit(score).policy.confidence_for(element, 200) == conf.OUT_OF_RANGE


# ── Engine-reported confidence ───────────────────────────────────────────────


def test_engine_confidence_beats_the_clean_default():
    score = score_of(measure(1, [("C4", 4.0)], time_sig="4/4"))
    element = score.flatten().notes[0]
    element.editorial.confidence = 0.4
    assert conf.audit(score).policy.confidence_for(element, 60) == 0.4


def test_engine_confidence_never_raises_a_structural_verdict():
    """An engine claiming 0.99 for a note in a broken bar does not get to
    overrule the bar: the minimum is taken, not the engine's word."""
    score = score_of(measure(1, [("C4", 1.0)], time_sig="4/4"))
    element = score.flatten().notes[0]
    element.editorial.confidence = 0.99
    assert conf.audit(score).policy.confidence_for(element, 60) == conf.SUSPECT_MEASURE


def test_absent_or_unusable_engine_confidence_is_ignored():
    score = score_of(measure(1, [("C4", 4.0)], time_sig="4/4"))
    element = score.flatten().notes[0]
    assert conf.engine_confidence(element) is None
    element.editorial.confidence = "not a number"
    assert conf.engine_confidence(element) is None
    assert conf.audit(score).policy.confidence_for(element, 60) == conf.CLEAN


# ── Summary ──────────────────────────────────────────────────────────────────


def test_summary_counts_match_the_flagged_notes():
    score = score_of(
        measure(1, [("C4", 1.0), ("D4", 1.0), ("E4", 1.0), ("F4", 1.0)], time_sig="4/4"),
        measure(2, [("G4", 1.0), ("A4", 1.0)]),
    )
    audit = conf.audit(score)
    notes = [
        _StubNote(c) for c in confidences(score)
    ]
    summary = conf.summarize(notes, audit)
    assert summary.total_notes == 6
    assert summary.flagged_notes == 2
    assert summary.suspect_measures == (2,)
    assert summary.summary_line() == (
        f"{conf.SUMMARY_PREFIX}imported 6 notes, 2 flagged — review in the editor"
    )
    assert summary.detail_lines() == ["suspect measures (1): 2"]


def test_a_clean_summary_does_not_ask_for_a_review():
    summary = conf.ConfidenceSummary(total_notes=8, flagged_notes=0, suspect_measures=())
    assert summary.summary_line() == f"{conf.SUMMARY_PREFIX}imported 8 notes, 0 flagged"
    assert summary.detail_lines() == []


def test_a_long_measure_list_is_truncated_rather_than_flooding_the_log():
    measures = tuple(range(1, conf.MAX_LISTED_MEASURES + 6))
    summary = conf.ConfidenceSummary(
        total_notes=500, flagged_notes=200, suspect_measures=measures
    )
    line = summary.detail_lines()[0]
    assert f"({len(measures)})" in line
    assert "… (5 more)" in line


class _StubNote:
    """Just the ``confidence`` attribute :func:`conf.summarize` counts."""

    def __init__(self, confidence: float) -> None:
        self.confidence = confidence


# ── End to end through the converter, still with no engine ────────────────────


def test_convert_score_leaves_confidence_exact_unless_asked():
    """M13-A's contract is untouched by M13-B existing."""
    fixture = os.path.join(
        os.path.dirname(os.path.dirname(ROOT)), "fixtures", "score", "single_staff_scale.musicxml"
    )
    chart, report = convert_score(fixture)
    assert {n.confidence for n in chart.notes} == {1.0}
    assert report.confidence is None


def test_confidence_check_rewrites_confidence_and_fills_the_summary():
    """The same clean fixture, converted as if it had come off a scan: every note
    drops to CLEAN, nothing is flagged, and the summary is populated."""
    fixture = os.path.join(
        os.path.dirname(os.path.dirname(ROOT)), "fixtures", "score", "single_staff_scale.musicxml"
    )
    chart, report = convert_score(fixture, confidence_check=True)
    assert {n.confidence for n in chart.notes} == {conf.CLEAN}
    assert report.confidence is not None
    assert report.confidence.total_notes == len(chart.notes)
    assert report.confidence.flagged_notes == 0
    assert {n.hand for n in chart.notes} == {Hand.UNKNOWN}, "M13-A's rules still apply"


def test_confidence_check_reports_a_scores_own_broken_bars():
    """`grace_note.musicxml` drops its grace note, which leaves that bar short of
    the metre — exactly the shape a real OMR misread has."""
    fixture = os.path.join(
        os.path.dirname(os.path.dirname(ROOT)), "fixtures", "score", "single_note.musicxml"
    )
    chart, report = convert_score(fixture, confidence_check=True)
    assert report.confidence is not None
    assert report.confidence.total_notes == len(chart.notes)
    # Whatever the verdict, the counts must agree with the notes themselves.
    flagged = sum(1 for n in chart.notes if n.confidence < conf.CLEAN)
    assert report.confidence.flagged_notes == flagged
