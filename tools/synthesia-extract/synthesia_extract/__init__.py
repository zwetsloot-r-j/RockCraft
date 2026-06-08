"""Synthesia visual note extractor (RockCraft M6-C).

A classical-CV sidecar that turns a Synthesia-style tutorial video (a falling
piano-roll) into the M6-A ``ExtractedChart`` JSON consumed by the Rust import
pipeline.  No network, no audio, no LLM — just OpenCV + numpy.

Public surface:

* :data:`EXTRACTOR_VERSION` — stamped into ``SourceMeta``.
* :mod:`schema` — dataclasses mirroring the Rust ``rockcraft-import`` schema.
* :func:`pipeline.extract_chart` — frames + fps -> ``ExtractedChart``.
* :mod:`synth` — synthetic clip generator used by the tests (never a real video).
"""

from .schema import EXTRACTOR_VERSION, ExtractedChart, ExtractedNote, Hand, SourceMeta

__all__ = [
    "EXTRACTOR_VERSION",
    "ExtractedChart",
    "ExtractedNote",
    "Hand",
    "SourceMeta",
]
