"""score-import — digital score file → M6-A ``ExtractedChart`` JSON (M13-A).

A deterministic transform: pitch, duration, tempo, metre, key and staff→hand are
all explicit in the source, so nothing here infers, guesses or learns.
"""

from .convert import ConversionReport, convert_score

__all__ = ["ConversionReport", "convert_score"]
