"""Score file → :class:`ExtractedChart` conversion (M13-A), and the scan
routing that feeds it (M13-B).

The whole module is a **deterministic transform**. Everything it emits — pitch,
onset, duration, tempo, metre, key, staff→hand — is stated explicitly by the
source score. Nothing is inferred, and nothing that the source does not state is
invented: an absent tempo mark becomes a documented 120 BPM assumption with a
warning, not a guess derived from the notes.

A **scan or PDF** (M13-B) is not stated by anything: it goes through
:mod:`score_import.omr` first, which drives an external OMR engine to produce
MusicXML, and then through this same transform — with the one difference that
:mod:`score_import.confidence` replaces rule 9's exact ``1.0`` with a structural
per-note confidence. :func:`convert_input` is the entry point that decides which
of the two an input is.

Conversion rules (each one has a test in ``tests/test_convert.py``):

1. Parse with :func:`music21.converter.parse`.
2. Expand repeats (repeats, voltas, D.C., D.S., coda) into a linear score;
   fall back to the unexpanded score with a warning when expansion fails.
3. Merge ties, so a tied pair is one note of the summed duration.
4. Chords become one note per pitch, sharing start and duration.
5. Times come from a tempo map over the score's metronome marks, so a tempo
   change mid-piece moves every later note correctly.
6. Hand comes from the staff/part: upper → Right, lower → Left; a single staff
   or 3+ parts → Unknown, overridable with an explicit hand map.
7. Velocity comes from the notated dynamics, resolved per staff by
   :mod:`score_import.dynamics` — levels, hairpin ramps, accent and marcato. A
   velocity the source states outright wins over the derived one, and a passage
   with no dynamic in effect stays ``None`` rather than being invented.
8. Grace notes, ornament realizations, unpitched/percussion notes, pedal marks,
   duration-changing articulations and lyrics are dropped — counted, never
   silently swallowed.
9. Every note gets ``confidence = 1.0``: this transform is exact. The one
   exception is the OMR path, which opts into ``confidence_check=True`` because
   an engine reading pixels is not exact — see :mod:`score_import.confidence`.
"""

from __future__ import annotations

import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Optional

from music21 import bar, chord, converter, key as m21key, note as m21note, repeat, stream, tempo
from music21 import meter as m21meter

from . import confidence as conf
from . import dynamics as dyn
from . import omr
from .schema import (
    EXTRACTOR_VERSION,
    ExtractedChart,
    ExtractedNote,
    Hand,
    Key,
    NotationMeta,
    Scale,
    SourceMeta,
    TimeSig,
)

#: Tempo assumed when the score carries no metronome mark at all. Reported as a
#: warning and written into ``notation.bpm`` so the editor's bar lines still line
#: up with *something* deliberate rather than an accident of the default grid.
DEFAULT_BPM = 120

#: MusicXML modes we can map onto ``core::Scale``. Any other mode (dorian,
#: mixolydian, …) omits the key rather than flattening it onto a wrong scale.
_MODE_TO_SCALE = {
    "major": Scale.MAJOR,
    "minor": Scale.NATURAL_MINOR,
}

#: Part index → hand for the common two-staff piano layout.
_DEFAULT_HANDS = {0: Hand.RIGHT, 1: Hand.LEFT}

_HAND_BY_NAME = {
    "left": Hand.LEFT,
    "right": Hand.RIGHT,
    "unknown": Hand.UNKNOWN,
}


@dataclass
class ConversionReport:
    """Everything the conversion wants to *say* rather than encode in the chart.

    The caller decides where this goes; the CLI prints it to stderr, keeping
    stdout pure JSON for ``run_sidecar``.
    """

    warnings: list[str] = field(default_factory=list)
    #: Category → count of elements dropped by design (grace notes, unpitched …).
    dropped: dict[str, int] = field(default_factory=dict)
    #: Distinct quarter-note BPMs found in the score, in order of appearance.
    tempos: list[float] = field(default_factory=list)
    #: What the confidence pass found — ``None`` unless it ran (the OMR path).
    confidence: Optional[conf.ConfidenceSummary] = None
    #: Explicit-vs-default velocity counts — ``None`` unless the dynamics pass
    #: ran, which is every conversion except one asking for ``--no-dynamics``.
    velocities: Optional[dyn.VelocitySummary] = None

    def warn(self, message: str) -> None:
        self.warnings.append(message)

    def drop(self, category: str, count: int = 1) -> None:
        if count:
            self.dropped[category] = self.dropped.get(category, 0) + count

    def lines(self) -> list[str]:
        """Human-readable report lines, warnings first."""
        out = [f"warning: {w}" for w in self.warnings]
        for category in sorted(self.dropped):
            out.append(f"dropped {self.dropped[category]} {category} (by design)")
        if self.velocities is not None:
            out.extend(self.velocities.lines())
        if self.confidence is not None:
            out.extend(self.confidence.detail_lines())
        return out


def parse_hand_map(spec: Optional[str]) -> dict[int, Hand]:
    """Parse a ``--hand-map`` value like ``"0=right,1=left"``.

    Part indices are 0-based, matching the order the parts appear in the score.
    Raises :class:`ValueError` on anything malformed — a typo here would silently
    mis-assign every note, so it is worth failing loudly.
    """
    if not spec:
        return {}
    mapping: dict[int, Hand] = {}
    for entry in spec.split(","):
        entry = entry.strip()
        if not entry:
            continue
        if "=" not in entry:
            raise ValueError(f"bad --hand-map entry {entry!r}; expected <part-index>=<hand>")
        raw_index, raw_hand = entry.split("=", 1)
        try:
            index = int(raw_index.strip())
        except ValueError as exc:
            raise ValueError(f"bad part index in --hand-map entry {entry!r}") from exc
        hand = _HAND_BY_NAME.get(raw_hand.strip().lower())
        if hand is None:
            raise ValueError(
                f"bad hand in --hand-map entry {entry!r}; expected left, right or unknown"
            )
        mapping[index] = hand
    return mapping


def convert_input(
    path: str,
    *,
    hand_map: Optional[dict[int, Hand]] = None,
    title: Optional[str] = None,
    read_dynamics: bool = True,
    log: Callable[[str], None] = omr._log,
) -> tuple[ExtractedChart, ConversionReport]:
    """Convert any supported input — a score file, or a scan via OMR (M13-B).

    This is the one entry point the CLI uses, which is what keeps the Rust side
    on a single sidecar contract: same ``--in``/``--out``, same JSON on stdout,
    whether the input was MusicXML or a photograph of a page.

    A score file goes straight to :func:`convert_score`. A scan or PDF is
    transcribed to MusicXML in a temporary directory first — the intermediate is
    never left behind next to the user's scan — and then converted with the
    confidence pass enabled and the engine recorded in ``extractor_version``.
    """
    if not omr.is_scan(path):
        return convert_score(path, hand_map=hand_map, title=title, read_dynamics=read_dynamics)

    with tempfile.TemporaryDirectory(prefix="rockcraft-omr-") as work:
        musicxml = Path(work) / "transcribed.musicxml"
        engine = omr.transcribe(path, musicxml, log=log)
        return convert_score(
            str(musicxml),
            hand_map=hand_map,
            title=title,
            read_dynamics=read_dynamics,
            confidence_check=True,
            extractor_version=engine.label(),
        )


def convert_score(
    path: str,
    *,
    hand_map: Optional[dict[int, Hand]] = None,
    title: Optional[str] = None,
    read_dynamics: bool = True,
    confidence_check: bool = False,
    extractor_version: str = EXTRACTOR_VERSION,
) -> tuple[ExtractedChart, ConversionReport]:
    """Convert the score at `path` into an :class:`ExtractedChart`.

    Returns the chart and a :class:`ConversionReport` of warnings and
    by-design drops. Raises whatever ``music21`` raises for an unreadable or
    unsupported file — the CLI turns that into a non-zero exit.

    `read_dynamics` is rule 7's velocity pass (M13-D). Turning it off restores
    M13-A's behaviour wholesale — velocity only where the source states one
    outright — which is what ``--no-dynamics`` is for: an A/B against the flat
    default, and an escape hatch for a score whose markings resolve badly.

    `confidence_check` swaps rule 9's exact ``confidence = 1.0`` for
    :mod:`score_import.confidence`'s structural per-note score, and fills in
    ``report.confidence``. It is off by default: only the OMR path, whose input
    was *inferred* rather than stated, turns it on.
    """
    report = ConversionReport()
    score = converter.parse(path)
    score = _expand_repeats(score, report)
    score = _strip_ties(score, report)

    tempo_map = _tempo_map(score, report)
    parts = _parts(score)
    hands = _resolve_hands(parts, hand_map or {}, report)

    # Audited *after* the repeat/tie rewrites, because both copy their elements
    # and the policy is keyed on the identity of the notes actually emitted.
    audit = conf.audit(score) if confidence_check else None
    note_confidence = audit.policy.confidence_for if audit else _exact_confidence

    notes: list[ExtractedNote] = []
    for index, part in enumerate(parts):
        # Dynamics resolve per staff, so each part gets its own reader: a left
        # hand marked `p` under a right hand marked `f` is ordinary piano writing.
        staff_dynamics = (
            dyn.read(part, warn=report.warn, drop=report.drop) if read_dynamics else dyn.SILENT
        )
        notes.extend(
            _part_notes(part, hands[index], tempo_map, report, note_confidence, staff_dynamics)
        )
    notes.sort(key=lambda n: (n.start_us, n.pitch))

    if read_dynamics:
        report.velocities = dyn.summarize(notes)
    if audit is not None:
        report.confidence = conf.summarize(notes, audit)

    return (
        ExtractedChart(
            notes=notes,
            source=SourceMeta(
                extractor_version=extractor_version,
                title=title if title is not None else _title(score),
            ),
            notation=_notation(score, tempo_map, report),
        ),
        report,
    )


# ── Score preparation ────────────────────────────────────────────────────────


def _expand_repeats(score: stream.Score, report: ConversionReport) -> stream.Score:
    """Unroll repeats, voltas and D.C./D.S./coda jumps into a linear score.

    A score with no repeat structure is returned untouched — running the expander
    over it would be pure risk for no gain. Malformed repeat structures are
    common in real files, so an expansion failure degrades to the unexpanded
    score with a warning rather than failing the import.
    """
    if not _has_repeats(score):
        return score
    try:
        return score.expandRepeats()
    except Exception as exc:  # music21 raises several unrelated exception types
        report.warn(
            f"could not expand repeats ({type(exc).__name__}: {exc}); "
            "importing the score as written, so repeated sections appear once"
        )
        return score


def _has_repeats(score: stream.Score) -> bool:
    elements = score.recurse()
    return bool(
        elements.getElementsByClass(bar.Repeat)
        or elements.getElementsByClass(repeat.RepeatExpression)
    )


def _strip_ties(score: stream.Score, report: ConversionReport) -> stream.Score:
    """Merge tied notes so a tied pair becomes one note of the summed duration."""
    try:
        return score.stripTies(inPlace=False, matchByPitch=True)
    except Exception as exc:  # pragma: no cover - defensive; stripTies is total
        report.warn(
            f"could not merge ties ({type(exc).__name__}: {exc}); "
            "tied notes will be imported as separate notes"
        )
        return score


def _parts(score: stream.Score) -> list[stream.Part]:
    """The score's parts (a multi-staff piano part is two ``PartStaff``s).

    A score with no parts at all — a bare stream of notes — is treated as one
    part so a minimal file still converts.
    """
    parts = list(score.parts) if hasattr(score, "parts") else []
    return parts if parts else [score]


# ── Timing ───────────────────────────────────────────────────────────────────


def _tempo_map(score: stream.Score, report: ConversionReport) -> list[tuple[float, float]]:
    """Build ``[(offset_in_quarters, seconds_per_quarter)]``, sorted, starting at 0.

    Times are integrated across this map rather than multiplied by one BPM, so a
    tempo change moves every later note correctly. ``Grid`` still holds a single
    BPM, which is why more than one tempo earns a loud warning: the *notes* stay
    right, the editor's *bar lines* drift after the change.
    """
    marks = sorted(
        (
            (float(mark.getOffsetInHierarchy(score)), float(mark.getQuarterBPM() or DEFAULT_BPM))
            for mark in score.recurse().getElementsByClass(tempo.MetronomeMark)
            if mark.getQuarterBPM()
        ),
        key=lambda item: item[0],
    )

    # Several parts commonly repeat the same mark at the same offset; collapse
    # duplicates so a two-staff piano score doesn't look like a tempo change.
    deduped: list[tuple[float, float]] = []
    for offset, bpm in marks:
        if deduped and deduped[-1][0] == offset:
            continue
        if deduped and deduped[-1][1] == bpm:
            continue
        deduped.append((offset, bpm))

    for _, bpm in deduped:
        if bpm not in report.tempos:
            report.tempos.append(bpm)

    if not deduped:
        report.warn(
            f"no tempo mark in the score; assuming {DEFAULT_BPM} BPM "
            "(note times and the imported grid both use it)"
        )
        return [(0.0, 60.0 / DEFAULT_BPM)]

    if len(report.tempos) > 1:
        report.warn(
            "score contains more than one tempo "
            f"({', '.join(f'{t:g}' for t in report.tempos)} BPM): note times are correct, "
            "but the imported grid holds a single BPM, so the editor's bar lines "
            "will drift after the first tempo change"
        )

    if deduped[0][0] > 0.0:
        # Music before the first mark: carry the first tempo backwards rather
        # than inventing a different one for the pickup.
        deduped.insert(0, (0.0, deduped[0][1]))
    return [(offset, 60.0 / bpm) for offset, bpm in deduped]


def _seconds_at(offset_ql: float, tempo_map: list[tuple[float, float]]) -> float:
    """Absolute seconds at `offset_ql` quarter notes into the piece."""
    total = 0.0
    for index, (segment_start, seconds_per_quarter) in enumerate(tempo_map):
        if segment_start >= offset_ql:
            break
        segment_end = tempo_map[index + 1][0] if index + 1 < len(tempo_map) else offset_ql
        total += (min(segment_end, offset_ql) - segment_start) * seconds_per_quarter
    return total


def _us(seconds: float) -> int:
    return int(round(seconds * 1_000_000))


# ── Notes ────────────────────────────────────────────────────────────────────


def _exact_confidence(_element: Any, _pitch: int) -> float:
    """Rule 9: a score *states* its notes, so the transform is fully confident."""
    return 1.0


def _part_notes(
    part: stream.Part,
    hand: Hand,
    tempo_map: list[tuple[float, float]],
    report: ConversionReport,
    note_confidence: Callable[[Any, int], float] = _exact_confidence,
    staff_dynamics: dyn.StaffDynamics = dyn.SILENT,
) -> list[ExtractedNote]:
    notes: list[ExtractedNote] = []
    for element in part.flatten().notes:
        if element.duration.isGrace:
            report.drop("grace note(s)")
            continue
        pitches = _pitches(element, report)
        if not pitches:
            continue

        offset = float(element.getOffsetInHierarchy(part))
        start_us = _us(_seconds_at(offset, tempo_map))
        end_us = _us(_seconds_at(offset + float(element.duration.quarterLength), tempo_map))
        dur_us = max(1, end_us - start_us)
        velocity = _velocity(element, staff_dynamics, offset)

        for pitch in pitches:
            notes.append(
                ExtractedNote(
                    pitch=pitch,
                    start_us=start_us,
                    dur_us=dur_us,
                    hand=hand,
                    velocity=velocity,
                    confidence=note_confidence(element, pitch),
                )
            )
    return notes


def _pitches(element: Any, report: ConversionReport) -> list[int]:
    """MIDI pitches of `element`; a chord yields one per pitch, sharing timing."""
    if isinstance(element, m21note.Note):
        return [int(element.pitch.midi)]
    if isinstance(element, chord.Chord):
        return [int(p.midi) for p in element.pitches]
    # Unpitched / percussion notes have no pitch to chart against a piano.
    report.drop("unpitched note(s)")
    return []


def _velocity(
    element: Any,
    staff_dynamics: dyn.StaffDynamics,
    offset_ql: float,
) -> Optional[int]:
    """The velocity for `element`: stated first, then read off the dynamics.

    A velocity the file states outright is the strongest signal there is, so it
    wins over anything derived. Failing that, :mod:`score_import.dynamics`
    resolves the notated level, hairpin ramp and accent for this staff (M13-D).
    ``None`` survives both — the Rust parser's ``DEFAULT_VELOCITY`` then applies,
    and "the source didn't say" stays distinguishable from a real value.
    """
    volume = getattr(element, "volume", None)
    velocity = getattr(volume, "velocity", None)
    if velocity is not None:
        return int(velocity)
    return staff_dynamics.velocity_for(element, offset_ql)


# ── Hands ────────────────────────────────────────────────────────────────────


def _resolve_hands(
    parts: list[stream.Part],
    hand_map: dict[int, Hand],
    report: ConversionReport,
) -> list[Hand]:
    """Hand per part index: explicit override first, then the staff heuristic.

    Two parts/staves is the piano layout the heuristic is for — upper Right,
    lower Left. One staff carries no hand information, and 3+ parts is an
    ensemble score the heuristic would only mislabel, so both stay Unknown and
    say so.
    """
    if hand_map:
        unknown = [index for index in hand_map if not 0 <= index < len(parts)]
        if unknown:
            report.warn(
                f"--hand-map names part index {unknown} but the score has {len(parts)} part(s)"
            )
        return [hand_map.get(index, Hand.UNKNOWN) for index in range(len(parts))]

    if len(parts) == 2:
        return [_DEFAULT_HANDS[index] for index in range(2)]

    if len(parts) > 2:
        report.warn(
            f"score has {len(parts)} parts; hands left Unknown "
            "(pass --hand-map to assign them explicitly)"
        )
    return [Hand.UNKNOWN] * len(parts)


# ── Notated context ──────────────────────────────────────────────────────────


def _notation(
    score: stream.Score,
    tempo_map: list[tuple[float, float]],
    report: ConversionReport,
) -> NotationMeta:
    """Tempo / metre / key as notated at the start of the piece."""
    bpm = int(round(60.0 / tempo_map[0][1])) if tempo_map else DEFAULT_BPM
    return NotationMeta(bpm=bpm, time_sig=_time_sig(score), key=_key(score, report))


def _time_sig(score: stream.Score) -> Optional[TimeSig]:
    for signature in score.recurse().getElementsByClass(m21meter.TimeSignature):
        return TimeSig(
            beats_per_bar=int(signature.numerator),
            beat_unit=int(signature.denominator),
        )
    return None


def _key(score: stream.Score, report: ConversionReport) -> Optional[Key]:
    """The notated key, or ``None`` when the score does not state a usable one.

    A bare key *signature* (fifths with no mode) is genuinely ambiguous — the
    same accidentals serve a major key and its relative minor — so it is omitted
    rather than guessed. Modal keys are omitted for the same reason: core models
    only major and natural minor.
    """
    for signature in score.recurse().getElementsByClass(m21key.KeySignature):
        if not isinstance(signature, m21key.Key):
            report.warn(
                "score states a key signature but no mode; key left unset "
                "(the same accidentals fit a major key and its relative minor)"
            )
            return None
        scale = _MODE_TO_SCALE.get(str(signature.mode).lower())
        if scale is None:
            report.warn(f"unsupported key mode {signature.mode!r}; key left unset")
            return None
        return Key(root_pc=int(signature.tonic.pitchClass), scale=scale)
    return None


def _title(score: stream.Score) -> Optional[str]:
    metadata = getattr(score, "metadata", None)
    if metadata is None:
        return None
    for attribute in ("title", "movementName"):
        value = getattr(metadata, attribute, None)
        if value:
            return str(value)
    return None
