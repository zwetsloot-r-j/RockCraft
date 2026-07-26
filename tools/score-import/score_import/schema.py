"""M6-A ``ExtractedChart`` schema, mirrored in Python (M13-A extension).

The JSON these emit is the wire format read by ``crates/import`` (see
``crates/import/src/schema.rs``).  Field names, the ``Hand`` string variants
(``"Left"`` / ``"Right"`` / ``"Unknown"``) and the ``skip_serializing_if =
Option::is_none`` behaviour are reproduced so the Rust ``serde`` deserializer
accepts the output verbatim.

This mirrors ``tools/synthesia-extract/synthesia_extract/schema.py`` rather than
importing it: each sidecar is a standalone project with its own dependencies,
and the video extractor must not become a dependency of the score converter.
The score path additionally emits ``notation`` — the notated tempo/metre/key the
video path has no way to know.
"""

from __future__ import annotations

import enum
import json
from dataclasses import dataclass, field
from typing import Any, Optional

# Bump when the conversion changes in a way reviewers should notice.
EXTRACTOR_VERSION = "score-import-0.1"


class Hand(enum.Enum):
    """Which hand plays a note. Serializes to the Rust enum's variant name."""

    LEFT = "Left"
    RIGHT = "Right"
    UNKNOWN = "Unknown"

    def __str__(self) -> str:  # pragma: no cover - convenience only
        return self.value


class Scale(enum.Enum):
    """Mirrors ``rockcraft_core::Scale``. Only the two scales core models."""

    MAJOR = "Major"
    NATURAL_MINOR = "NaturalMinor"


@dataclass
class ExtractedNote:
    """One note. Mirrors ``rockcraft_import::schema::ExtractedNote``."""

    pitch: int  # MIDI note number 0..=127
    start_us: int  # onset in microseconds from the start of the piece
    dur_us: int  # duration in microseconds (>= 1 after conversion)
    hand: Hand = Hand.UNKNOWN
    velocity: Optional[int] = None  # only when the source states one
    confidence: Optional[float] = None  # 1.0 for a score: the transform is exact

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "pitch": int(self.pitch),
            "start_us": int(self.start_us),
            "dur_us": int(max(1, self.dur_us)),
            "hand": self.hand.value,
        }
        # Match serde's skip_serializing_if = Option::is_none.
        if self.velocity is not None:
            out["velocity"] = int(self.velocity)
        if self.confidence is not None:
            out["confidence"] = float(self.confidence)
        return out


@dataclass
class SourceMeta:
    """Provenance. Mirrors ``rockcraft_import::schema::SourceMeta``.

    Every video-geometry field stays ``None``: a score has no frames, no scroll
    speed and no hit line, so the writer emits no overlay calibration for it.
    """

    extractor_version: str = EXTRACTOR_VERSION
    title: Optional[str] = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {"extractor_version": self.extractor_version}
        if self.title is not None:
            out["title"] = self.title
        return out


@dataclass
class TimeSig:
    """Mirrors ``rockcraft_core::TimeSig``."""

    beats_per_bar: int
    beat_unit: int

    def to_dict(self) -> dict[str, Any]:
        return {
            "beats_per_bar": int(self.beats_per_bar),
            "beat_unit": int(self.beat_unit),
        }


@dataclass
class Key:
    """Mirrors ``rockcraft_core::Key``: tonic pitch-class + scale."""

    root_pc: int  # 0..=11, 0 == C
    scale: Scale

    def to_dict(self) -> dict[str, Any]:
        return {"root_pc": int(self.root_pc), "scale": self.scale.value}


@dataclass
class NotationMeta:
    """Notated tempo / metre / key. Mirrors ``NotationMeta``.

    A field left ``None`` means *the score did not say* — never "use the
    default". The Rust writer decides what a missing field falls back to.
    """

    bpm: Optional[int] = None
    time_sig: Optional[TimeSig] = None
    key: Optional[Key] = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {}
        if self.bpm is not None:
            out["bpm"] = int(self.bpm)
        if self.time_sig is not None:
            out["time_sig"] = self.time_sig.to_dict()
        if self.key is not None:
            out["key"] = self.key.to_dict()
        return out


@dataclass
class ExtractedChart:
    """Top-level converter output. Mirrors ``ExtractedChart``."""

    notes: list[ExtractedNote] = field(default_factory=list)
    source: SourceMeta = field(default_factory=SourceMeta)
    notation: Optional[NotationMeta] = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "notes": [n.to_dict() for n in self.notes],
            "source": self.source.to_dict(),
        }
        if self.notation is not None:
            out["notation"] = self.notation.to_dict()
        return out

    def to_json(self, *, indent: Optional[int] = None) -> str:
        return json.dumps(self.to_dict(), indent=indent)
