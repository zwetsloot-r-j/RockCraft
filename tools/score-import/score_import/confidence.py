"""Structural per-note confidence for OMR-derived scores (M13-B).

M13-A's transform is exact: the file *states* every pitch and duration, so every
note it emits carries ``confidence = 1.0``. Optical music recognition is the
opposite — it *infers* the notes from pixels, and on a handwritten or dense piano
score it gets some of them wrong. This module is the part that says so.

OMR engines generally report no per-note confidence at all, so confidence here is
derived **structurally** — from what the transcription says about itself:

1. **Measure fill** — a measure whose notes and rests don't cover its time
   signature has lost or gained something. This is the single highest-yield OMR
   error detector, and every note in such a measure drops to
   :data:`SUSPECT_MEASURE`.
2. **Piano range** — a pitch outside A0..C8 is a misread ledger line, clef or
   octave mark, and drops to :data:`OUT_OF_RANGE`.
3. **Engine-reported confidence**, when an engine supplies one, is taken as a
   further minimum.
4. Everything else is :data:`CLEAN` — deliberately *not* ``1.0``, so a chart read
   off a scan is never indistinguishable from one converted from a real score.

Every function here is **pure over a parsed ``music21`` score**: no engine, no
subprocess, no file. That is what lets the whole detector be tested in CI on
hand-built scores while OMR itself never runs there.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Iterable, Iterator, Optional

from music21 import chord, note as m21note, stream

#: Confidence for a note nothing is wrong with. Never ``1.0``: that value is
#: M13-A's claim that the source *stated* the note, and the difference between
#: "the file said so" and "an engine read it off a scan" must stay visible all
#: the way into the bundle.
CLEAN = 0.9

#: A note in a measure whose contents don't cover its time signature.
SUSPECT_MEASURE = 0.5

#: A pitch outside the range of a piano.
OUT_OF_RANGE = 0.25

#: Piano range, inclusive: A0 (MIDI 21) to C8 (MIDI 108).
PITCH_FLOOR = 21
PITCH_CEILING = 108

#: Quarter-length slack when comparing a measure's contents to its bar duration.
#: Far finer than any notatable duration, so it absorbs float error and nothing
#: else.
DURATION_TOLERANCE = 1e-6

#: Reasons :func:`measure_reason` gives for doubting a measure.
REASON_SHORT = "short"
REASON_OVERFULL = "overfull"


# ── Measure fill ─────────────────────────────────────────────────────────────


def expected_fill(measure: stream.Measure) -> float:
    """Quarter lengths of content `measure` should hold.

    Its time signature's bar duration, less any padding. Padding is how a score
    *declares* a deliberately partial bar (a pickup, or a truncated final bar),
    so honouring it keeps a declared anacrusis off the suspect list. An
    **undeclared** short bar — which is what an OMR engine emits for a pickup it
    did not recognise as one — is still flagged, which is the right direction to
    err on a path whose whole point is "review this".
    """
    expected = float(measure.barDuration.quarterLength)
    expected -= float(measure.paddingLeft or 0.0) + float(measure.paddingRight or 0.0)
    return max(0.0, expected)


def _content(measure: stream.Measure) -> Iterator[tuple[Any, float, float]]:
    """``(element, start_ql, end_ql)`` for every note and rest in `measure`.

    Offsets are measure-relative and recurse into voices, so a polyphonic bar is
    judged by what its voices *cover* rather than by a sum that would double-count
    them.
    """
    for element in measure.recurse().notesAndRests:
        start = float(element.getOffsetInHierarchy(measure))
        yield element, start, start + float(element.duration.quarterLength)


def _covered(intervals: Iterable[tuple[float, float]], limit: float) -> float:
    """Total length of `intervals` unioned and clipped to ``[0, limit)``.

    A union rather than a sum for two reasons: chords and voices overlap (a sum
    would report a bar as over-full), and a *gap* in the middle of a bar — a note
    the engine dropped — has to count as missing coverage even though the bar's
    last note still reaches the bar line.
    """
    clipped = sorted(
        (max(0.0, start), min(limit, end))
        for start, end in intervals
        if min(limit, end) > max(0.0, start)
    )
    total = 0.0
    reach = 0.0
    for start, end in clipped:
        if end <= reach:
            continue
        total += end - max(start, reach)
        reach = end
    return total


def measure_reason(
    measure: stream.Measure,
    *,
    spill_ql: float = 0.0,
) -> Optional[str]:
    """Why `measure` is doubtful, or ``None`` when it looks sound.

    `spill_ql` is how far the *previous* measure's content ran past its own bar
    line — a note tied over the barline, which M13-A's tie merge turns into one
    long note sitting in the earlier bar. Without it every measure after a tie
    would look short.

    A measure holding no notes is never flagged: there would be nothing to lower
    the confidence of, and reporting a measure number with zero flagged notes
    would only make the summary counts look inconsistent.
    """
    expected = expected_fill(measure)
    if expected <= 0.0:
        return None
    content = list(_content(measure))
    if not any(_is_pitched(element) for element, _, _ in content):
        return None
    # Content starting at or past the bar line means the bar holds more than its
    # metre allows — a doubled note, or a barline the engine missed entirely.
    if any(start >= expected - DURATION_TOLERANCE for _, start, _ in content):
        return REASON_OVERFULL
    intervals = [(start, end) for _, start, end in content]
    if spill_ql > 0.0:
        intervals.append((0.0, min(spill_ql, expected)))
    if _covered(intervals, expected) < expected - DURATION_TOLERANCE:
        return REASON_SHORT
    return None


def measure_spill(measure: stream.Measure) -> float:
    """Quarter lengths by which `measure`'s content overruns its bar line."""
    expected = expected_fill(measure)
    if expected <= 0.0:
        return 0.0
    return max((end - expected for _, _, end in _content(measure)), default=0.0)


def _is_pitched(element: Any) -> bool:
    return isinstance(element, (m21note.Note, chord.Chord))


# ── Engine-reported confidence ───────────────────────────────────────────────


def engine_confidence(element: Any) -> Optional[float]:
    """Per-note confidence an engine attached to `element`, or ``None``.

    Read from ``music21``'s per-element ``editorial`` dict, which is the only
    place a score-shaped object can carry an annotation the notation itself has
    no field for. **No engine we wrap populates it**: MusicXML has no confidence
    element, so neither Audiveris nor ``oemer`` can express one and every note
    they produce lands on the structural rules above. The hook exists so that an
    engine (or a ``$ROCKCRAFT_OMR_CMD`` wrapper) that *does* know how sure it is
    has somewhere to say it, instead of having the information thrown away.
    """
    editorial = getattr(element, "editorial", None)
    if editorial is None:
        return None
    value = getattr(editorial, "confidence", None)
    if value is None:
        return None
    try:
        return min(1.0, max(0.0, float(value)))
    except (TypeError, ValueError):
        return None


# ── The policy applied per note ──────────────────────────────────────────────


@dataclass(frozen=True)
class ConfidencePolicy:
    """Per-note confidence for one audited score.

    Notes are identified by object identity rather than by position or time:
    ``music21``'s ``flatten()`` re-hosts the *same* element objects, so the
    measure verdicts computed here stay attached to exactly the notes M13-A's
    converter goes on to emit — no offset arithmetic, no boundary ambiguity, and
    a suspect bar in the left staff flags only the left staff.
    """

    #: ``id()`` of every pitched element inside a doubtful measure.
    suspect_ids: frozenset[int]

    def confidence_for(self, element: Any, pitch: int) -> float:
        """Confidence for one emitted note: the strictest applicable rule."""
        candidates = [CLEAN]
        if id(element) in self.suspect_ids:
            candidates.append(SUSPECT_MEASURE)
        if not PITCH_FLOOR <= pitch <= PITCH_CEILING:
            candidates.append(OUT_OF_RANGE)
        reported = engine_confidence(element)
        if reported is not None:
            candidates.append(reported)
        return min(candidates)


@dataclass(frozen=True)
class ScoreAudit:
    """A whole score's verdict: the policy plus what to tell the operator."""

    policy: ConfidencePolicy
    #: Measure number → why it is doubtful, in measure order.
    reasons: dict[int, str]

    @property
    def suspect_measures(self) -> list[int]:
        return sorted(self.reasons)


def audit(score: stream.Score) -> ScoreAudit:
    """Audit `score`'s measures and return the policy to score its notes with.

    Pure: takes a parsed score, touches no engine and no disk. Must be handed the
    **same** score object the converter reads its notes from — ``expandRepeats``
    and ``stripTies`` both copy, and the policy is keyed on element identity.
    """
    suspect_ids: set[int] = set()
    reasons: dict[int, str] = {}
    for part in _parts(score):
        spill = 0.0
        for measure in part.getElementsByClass(stream.Measure):
            reason = measure_reason(measure, spill_ql=spill)
            spill = measure_spill(measure)
            if reason is None:
                continue
            reasons.setdefault(int(measure.number or 0), reason)
            suspect_ids.update(
                id(element) for element, _, _ in _content(measure) if _is_pitched(element)
            )
    return ScoreAudit(ConfidencePolicy(frozenset(suspect_ids)), reasons)


def _parts(score: stream.Score) -> list[stream.Stream]:
    """The score's parts, or the score itself when it has none.

    Mirrors ``convert._parts`` deliberately rather than importing it: the
    converter imports *this* module, and this one must stay a leaf.
    """
    parts = list(score.parts) if hasattr(score, "parts") else []
    return parts if parts else [score]


# ── Summary ──────────────────────────────────────────────────────────────────

#: Marker prefix on the one summary line the frontends lift out of the sidecar's
#: stderr and put on their import status line. Mirrored in Rust as
#: ``rockcraft_import::OMR_SUMMARY_PREFIX`` — change both or neither.
SUMMARY_PREFIX = "omr: "

#: Most suspect measure numbers to name before truncating the detail line.
MAX_LISTED_MEASURES = 40


@dataclass(frozen=True)
class ConfidenceSummary:
    """What the confidence pass found, for stderr and the import status line."""

    total_notes: int
    flagged_notes: int
    suspect_measures: tuple[int, ...]

    def summary_line(self) -> str:
        """The prefixed one-liner the frontends show as the import status."""
        line = (
            f"{SUMMARY_PREFIX}imported {self.total_notes} notes, "
            f"{self.flagged_notes} flagged"
        )
        if self.flagged_notes:
            line += " — review in the editor"
        return line

    def detail_lines(self) -> list[str]:
        """Which measures were doubted — the log-pane half of the summary."""
        if not self.suspect_measures:
            return []
        listed = self.suspect_measures[:MAX_LISTED_MEASURES]
        numbers = ", ".join(str(n) for n in listed)
        if len(listed) < len(self.suspect_measures):
            numbers += f", … ({len(self.suspect_measures) - len(listed)} more)"
        return [f"suspect measures ({len(self.suspect_measures)}): {numbers}"]


def summarize(notes: Iterable[Any], audit_result: ScoreAudit) -> ConfidenceSummary:
    """Count what the pass flagged. `notes` are the emitted ``ExtractedNote``s."""
    notes = list(notes)
    flagged = sum(
        1 for n in notes if n.confidence is not None and n.confidence < CLEAN
    )
    return ConfidenceSummary(
        total_notes=len(notes),
        flagged_notes=flagged,
        suspect_measures=tuple(audit_result.suspect_measures),
    )
