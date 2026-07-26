"""score-import — score file or scan → M6-A ``ExtractedChart`` JSON.

Two tiers, one output format:

- **M13-A**, a deterministic transform: pitch, duration, tempo, metre, key and
  staff→hand are all explicit in a score file, so nothing here infers, guesses or
  learns, and every note comes out at ``confidence = 1.0``.
- **M13-B**, optical music recognition: a scan or PDF is transcribed to MusicXML
  by an external engine and then run through that same transform. That tier *is*
  lossy, so :mod:`score_import.confidence` derives a per-note confidence and the
  result is meant to be reviewed rather than trusted.
"""

from .confidence import ConfidenceSummary
from .convert import ConversionReport, convert_input, convert_score
from .omr import Engine, EngineMissing, OmrError

__all__ = [
    "ConfidenceSummary",
    "ConversionReport",
    "Engine",
    "EngineMissing",
    "OmrError",
    "convert_input",
    "convert_score",
]
