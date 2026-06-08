"""M6-A ``ExtractedChart`` schema, mirrored in Python.

The JSON these emit is the wire format read by ``crates/import`` (see
``crates/import/src/schema.rs``).  Field names, the ``Hand`` string variants
(``"Left"`` / ``"Right"`` / ``"Unknown"``), and the ``skip_serializing_if =
Option::is_none`` behaviour for ``velocity`` / ``confidence`` / ``title`` /
``fps`` / ``scroll_px_per_s`` are all reproduced so the Rust ``serde``
deserializer accepts the output verbatim.
"""

from __future__ import annotations

import enum
import json
from dataclasses import dataclass, field
from typing import Any, Optional

# Bump when the extraction algorithm changes in a way reviewers should notice.
EXTRACTOR_VERSION = "synthesia-extract 0.1.0"


class Hand(enum.Enum):
    """Which hand played a note. Serializes to the Rust enum's variant name."""

    LEFT = "Left"
    RIGHT = "Right"
    UNKNOWN = "Unknown"

    def __str__(self) -> str:  # pragma: no cover - convenience only
        return self.value


@dataclass
class ExtractedNote:
    """One detected note. Mirrors ``rockcraft_import::schema::ExtractedNote``."""

    pitch: int  # MIDI note number 0..=127
    start_us: int  # onset in microseconds from the start of the video
    dur_us: int  # duration in microseconds (>= 1 after conversion)
    hand: Hand = Hand.UNKNOWN
    velocity: Optional[int] = None  # None until M6-F audio-fusion fills it
    confidence: Optional[float] = None  # 0.0..=1.0, for the M6-E review step

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
    """Provenance/diagnostics. Mirrors ``rockcraft_import::schema::SourceMeta``."""

    extractor_version: str = EXTRACTOR_VERSION
    title: Optional[str] = None
    fps: Optional[float] = None
    scroll_px_per_s: Optional[float] = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {"extractor_version": self.extractor_version}
        if self.title is not None:
            out["title"] = self.title
        if self.fps is not None:
            out["fps"] = float(self.fps)
        if self.scroll_px_per_s is not None:
            out["scroll_px_per_s"] = float(self.scroll_px_per_s)
        return out


@dataclass
class ExtractedChart:
    """Top-level extractor output. Mirrors ``ExtractedChart``."""

    notes: list[ExtractedNote] = field(default_factory=list)
    source: SourceMeta = field(default_factory=SourceMeta)

    def to_dict(self) -> dict[str, Any]:
        return {
            "notes": [n.to_dict() for n in self.notes],
            "source": self.source.to_dict(),
        }

    def to_json(self, *, indent: Optional[int] = None) -> str:
        return json.dumps(self.to_dict(), indent=indent)
