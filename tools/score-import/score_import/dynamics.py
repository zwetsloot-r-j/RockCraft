"""Velocity from notated dynamics (M13-D).

M13-A emits ``velocity: None`` for every note, so the Rust parser's
``DEFAULT_VELOCITY`` (80) applies uniformly and an imported score auditions at one
flat dynamic from start to finish. That matters more for a score than for a
video: the video path fills velocity from the source audio and ships a real
``backing.wav``, while a score import has neither — the synth is the *only*
sound, so flat velocity is the whole listening experience.

The source already carries the answer. This module reads it:

1. **Level** — the dynamic in effect at the note's offset, from
   :data:`DYNAMIC_VELOCITY`. ``mf`` maps to **80, the same value as
   ``DEFAULT_VELOCITY``**, so a score whose only marking is ``mf`` sounds exactly
   as it did before this pass existed.
2. **Hairpins** — a ``cresc.``/``dim.`` wedge ramps linearly from the dynamic in
   effect where it starts to the one that resolves it, by the note's offset
   within the wedge. A wedge nothing resolves ramps one rung of the ladder and
   says so.
3. **Articulations** — accent ``+15``, marcato ``+25``, stacked on the level and
   clamped to 127.
4. **Nothing in effect** — before the first marking, or in a score with no
   markings at all — stays ``None``, *not* 80. The null is the honest signal
   "the source didn't say"; ``DEFAULT_VELOCITY`` still applies downstream, and a
   score with zero dynamics markings comes out byte-identical to M13-A's output.

Resolution is **per staff**, which is why :class:`StaffDynamics` is built from
one part at a time: a left hand marked ``p`` under a right hand marked ``f`` is
ordinary piano writing and must not be flattened onto one curve.

Everything here is **pure over a parsed ``music21`` part** — no engine, no
subprocess, no file — so the whole mapping is testable in CI against hand-written
synthetic MusicXML.

Explicitly *not* here: the sustain pedal. MusicXML states it, but ``SynthCommand``
is ``NoteOn``/``NoteOff``/``AllOff``, ``NoteEvent`` has no control-change concept
and the MIDI writer emits note on/off — CC64 needs a control-event model in
``core`` before there is anywhere to put it. Duration-changing marks (staccato,
tenuto, fermata) are out for a different reason: they would move the highway
blocks and the scoring windows, not just the volume. Velocity only.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Iterable, Optional

from music21 import articulations, dynamics as m21dynamics, stream

#: The dynamic ladder, softest first. Order matters: a wedge with no resolving
#: dynamic steps exactly one rung along it.
DYNAMIC_LADDER = ("ppp", "pp", "p", "mp", "mf", "f", "ff", "fff")

#: Notated dynamic → MIDI velocity.
#:
#: ``mf`` is 80 on purpose — it is ``parser::DEFAULT_VELOCITY``, so the marking a
#: score most often carries produces exactly the sound M13-A produced. The rest
#: fan out either side of it with enough spread to be audible on a synth without
#: reaching the extremes, where a piano sample tends to lose its tone.
DYNAMIC_VELOCITY = {
    "ppp": 16,
    "pp": 33,
    "p": 49,
    "mp": 64,
    "mf": 80,
    "f": 96,
    "ff": 112,
    "fff": 126,
}

#: Added on top of the level in effect. An accent is a note pushed harder than
#: its surroundings, not a new dynamic — which is why these stack rather than
#: replace, and why they survive a hairpin ramp underneath them.
ACCENT_BOOST = 15
MARCATO_BOOST = 25

#: Velocity 0 is a note-off in MIDI, so the floor is 1.
MIN_VELOCITY = 1
MAX_VELOCITY = 127


def _clamp(velocity: int) -> int:
    return max(MIN_VELOCITY, min(MAX_VELOCITY, velocity))


@dataclass(frozen=True)
class Hairpin:
    """One resolved wedge: where it ramps from, to, and over which offsets.

    `start_ql`/`end_ql` are the offsets of the first and last note the wedge
    spans — not the wedge's own graphical extent — so the last note under a
    ``p`` → ``f`` cresc. lands exactly on ``f`` rather than somewhere short of it.
    """

    start_ql: float
    end_ql: float
    from_velocity: Optional[int]
    to_velocity: Optional[int]

    def contains(self, offset_ql: float) -> bool:
        return self.start_ql <= offset_ql <= self.end_ql

    def velocity_at(self, offset_ql: float) -> Optional[int]:
        """Linear interpolation across the wedge, or ``None`` if it has no base.

        A wedge over a single note has nowhere to ramp, so it simply arrives:
        that note gets the target.
        """
        if self.from_velocity is None or self.to_velocity is None:
            return None
        if self.end_ql <= self.start_ql:
            return self.to_velocity
        fraction = (offset_ql - self.start_ql) / (self.end_ql - self.start_ql)
        fraction = max(0.0, min(1.0, fraction))
        span = self.to_velocity - self.from_velocity
        return int(round(self.from_velocity + span * fraction))


@dataclass(frozen=True)
class StaffDynamics:
    """The dynamics of one staff, resolved to a velocity per note offset.

    Built by :func:`read`. Marks are ``(offset_in_quarters, velocity)`` sorted by
    offset; hairpins are the wedges that were resolvable against them.
    """

    marks: tuple[tuple[float, int], ...] = ()
    hairpins: tuple[Hairpin, ...] = ()

    def velocity_for(self, element: Any, offset_ql: float) -> Optional[int]:
        """The velocity for a note at `offset_ql`, or ``None`` if nothing says.

        Articulations only ever *modify* a stated level. An accent on its own
        says "harder than the surrounding notes" and names no absolute loudness,
        so with no dynamic in effect the honest answer is still ``None``.
        """
        level = self._level_at(offset_ql)
        if level is None:
            return None
        return _clamp(level + _articulation_boost(element))

    def _level_at(self, offset_ql: float) -> Optional[int]:
        for hairpin in self.hairpins:
            if hairpin.contains(offset_ql):
                ramped = hairpin.velocity_at(offset_ql)
                if ramped is not None:
                    return ramped
        return _mark_at(self.marks, offset_ql)


def _mark_at(marks: tuple[tuple[float, int], ...], offset_ql: float) -> Optional[int]:
    """The velocity of the last mark at or before `offset_ql`, or ``None``."""
    level: Optional[int] = None
    for mark_offset, velocity in marks:
        if mark_offset > offset_ql:
            break
        level = velocity
    return level


#: A :class:`StaffDynamics` that says nothing — every note falls through to
#: ``None``. What ``--no-dynamics`` and a staff with no markings both resolve to.
SILENT = StaffDynamics()


def _articulation_boost(element: Any) -> int:
    """Accent/marcato boost for a note or chord; 0 for anything else.

    ``StrongAccent`` (MusicXML ``<strong-accent/>``, engraved as marcato) is a
    *subclass* of ``Accent`` in music21, so it has to be tested first or every
    marcato would read as a plain accent.
    """
    boost = 0
    for articulation in getattr(element, "articulations", ()) or ():
        if isinstance(articulation, articulations.StrongAccent):
            boost += MARCATO_BOOST
        elif isinstance(articulation, articulations.Accent):
            boost += ACCENT_BOOST
    return boost


def read(
    part: stream.Stream,
    warn: Callable[[str], None] = lambda _message: None,
    drop: Callable[[str, int], None] = lambda _category, _count: None,
) -> StaffDynamics:
    """Read one staff's dynamics into a :class:`StaffDynamics`.

    `warn` and `drop` are the :class:`~score_import.convert.ConversionReport`
    hooks, kept as plain callables so this module never has to import the
    converter back.
    """
    marks = _read_marks(part, drop)
    hairpins = _read_hairpins(part, marks, warn)
    return StaffDynamics(marks=marks, hairpins=hairpins)


def _read_marks(
    part: stream.Stream,
    drop: Callable[[str, int], None],
) -> tuple[tuple[float, int], ...]:
    """The staff's dynamic markings as sorted ``(offset, velocity)`` pairs.

    Marks outside the ladder — ``sf``, ``sfz``, ``fp``, ``rfz`` — are dropped
    rather than mapped. They are accents or shapes, not levels: ``sf`` says
    "this note harder", ``fp`` says "loud then immediately soft", and forcing
    either onto a rung would set the loudness of everything that follows to a
    value the score never asked for.
    """
    marks: list[tuple[float, int]] = []
    unmapped = 0
    for mark in part.recurse().getElementsByClass(m21dynamics.Dynamic):
        velocity = DYNAMIC_VELOCITY.get(str(mark.value).strip())
        if velocity is None:
            unmapped += 1
            continue
        offset = _offset_in(mark, part)
        if offset is not None:
            marks.append((offset, velocity))
    drop("dynamic marking(s) outside ppp..fff", unmapped)
    marks.sort(key=lambda item: item[0])
    return tuple(marks)


def _read_hairpins(
    part: stream.Stream,
    marks: tuple[tuple[float, int], ...],
    warn: Callable[[str], None],
) -> tuple[Hairpin, ...]:
    hairpins: list[Hairpin] = []
    for wedge in part.recurse().getElementsByClass(m21dynamics.DynamicWedge):
        hairpin = _resolve_wedge(wedge, part, marks, warn)
        if hairpin is not None:
            hairpins.append(hairpin)
    return tuple(hairpins)


def _resolve_wedge(
    wedge: m21dynamics.DynamicWedge,
    part: stream.Stream,
    marks: tuple[tuple[float, int], ...],
    warn: Callable[[str], None],
) -> Optional[Hairpin]:
    """Turn one wedge into a :class:`Hairpin`, or ``None`` if it spans nothing.

    The wedge ramps *from* the dynamic in effect where it starts *to* the first
    dynamic stated after that. A wedge nothing resolves — common in real scores,
    where the engraver leaves the arrival implied — steps one rung of the ladder
    and warns, because the alternative is either ignoring the wedge (a cresc.
    that does not crescendo) or inventing an arrival the score never stated.
    """
    offsets = [
        offset
        for offset in (_offset_in(element, part) for element in wedge.getSpannedElements())
        if offset is not None
    ]
    if not offsets:
        return None

    start_ql, end_ql = min(offsets), max(offsets)
    from_velocity = _mark_at(marks, start_ql)
    to_velocity = next((velocity for offset, velocity in marks if offset > start_ql), None)

    if to_velocity is None:
        louder = isinstance(wedge, m21dynamics.Crescendo)
        to_velocity = _one_rung(from_velocity, louder=louder)
        if from_velocity is not None:
            warn(
                f"{'crescendo' if louder else 'diminuendo'} at quarter-note offset "
                f"{start_ql:g} has no dynamic resolving it; ramping one level to "
                f"velocity {to_velocity}"
            )
    return Hairpin(
        start_ql=start_ql,
        end_ql=end_ql,
        from_velocity=from_velocity,
        to_velocity=to_velocity,
    )


def _one_rung(velocity: Optional[int], *, louder: bool) -> Optional[int]:
    """One step along :data:`DYNAMIC_LADDER` from `velocity`, saturating at the ends."""
    if velocity is None:
        return None
    rungs = [DYNAMIC_VELOCITY[name] for name in DYNAMIC_LADDER]
    if velocity not in rungs:
        # Mid-ramp value (a wedge starting inside another): step by the nearest rung.
        return _clamp(velocity + (ACCENT_BOOST if louder else -ACCENT_BOOST))
    index = rungs.index(velocity) + (1 if louder else -1)
    return rungs[max(0, min(len(rungs) - 1, index))]


def _offset_in(element: Any, part: stream.Stream) -> Optional[float]:
    """`element`'s offset in quarter notes within `part`, or ``None`` if unplaced.

    A spanner can outlive the stream its endpoints were in — after a repeat
    expansion, for instance — and music21 raises rather than returning a
    placeholder. A wedge we cannot place is a wedge we cannot ramp.
    """
    try:
        return float(element.getOffsetInHierarchy(part))
    except Exception:  # music21 raises several unrelated types for an unplaced element
        return None


@dataclass(frozen=True)
class VelocitySummary:
    """How many notes the dynamics pass could speak for, for stderr."""

    explicit: int
    default: int

    @property
    def total(self) -> int:
        return self.explicit + self.default

    def lines(self) -> list[str]:
        if not self.total:
            return []
        return [
            f"velocity from notated dynamics: {self.explicit} of {self.total} note(s) "
            f"explicit, {self.default} left to the default"
        ]


def summarize(notes: Iterable[Any]) -> VelocitySummary:
    """Count the emitted notes by whether they carry a velocity."""
    emitted = list(notes)
    explicit = sum(1 for note in emitted if note.velocity is not None)
    return VelocitySummary(explicit=explicit, default=len(emitted) - explicit)
