"""Synthetic Synthesia-style clip generator.

This renders a *fabricated* falling piano-roll over a drawn keyboard for a known
note sequence, so tests can assert the extractor recovers the ground truth.  It
is the inverse of the extractor and exists only to produce non-copyrighted test
fixtures in memory — **never** a real video.

Geometry / conventions (the extractor calibrates these from pixels; here we
synthesize them):

* The keyboard occupies the bottom ``keyboard_h`` rows; everything above the
  hit-line (the keyboard's top edge) is the falling region.
* A note's bar bottom edge reaches the hit-line exactly at its onset; its top
  edge reaches the hit-line at its note-off.  Bar height = ``dur * scroll``.
* Bars are saturated colours on a dark background; white/black keys are drawn so
  the keyboard doubles as the pitch ruler.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

import numpy as np

# White-key semitone offsets within an octave (C=0 .. B=11).
_WHITE_SEMITONES = [0, 2, 4, 5, 7, 9, 11]
# For a white key, is there a black key immediately to its right (within octave)?
# C C# D D# E (no) F F# G G# A A# B (no) -> pattern keyed by white index 0..6.
_HAS_BLACK_RIGHT = [True, True, False, True, True, True, False]

# BGR colours (OpenCV order).
_BG = (28, 28, 28)
_WHITE = (255, 255, 255)
_BLACK = (0, 0, 0)
LEFT_COLOR = (40, 140, 255)  # orange-ish
RIGHT_COLOR = (60, 200, 60)  # green-ish


@dataclass(frozen=True)
class SynthNote:
    """A ground-truth note for synthesis and comparison."""

    pitch: int
    start_us: int
    dur_us: int
    hand: str  # "Left" / "Right" / "Unknown"


@dataclass
class SynthConfig:
    """Layout/timing knobs for the synthetic renderer."""

    low_pitch: int = 48  # C3
    high_pitch: int = 83  # B5
    white_w: int = 34
    black_w: int = 20
    keyboard_h: int = 90
    black_h: int = 55
    margin_x: int = 10
    falling_h: int = 270
    fps: float = 30.0
    scroll_px_per_s: float = 180.0

    @property
    def height(self) -> int:
        return self.falling_h + self.keyboard_h

    @property
    def hit_line(self) -> int:
        return self.falling_h


def _white_pitches(low: int, high: int) -> list[int]:
    """White-key MIDI pitches in [low, high], inclusive."""
    return [p for p in range(low, high + 1) if (p % 12) in _WHITE_SEMITONES]


class SynthKeyboard:
    """Pixel layout of the drawn keyboard; also the ground-truth pitch ruler."""

    def __init__(self, cfg: SynthConfig):
        self.cfg = cfg
        self.white_pitches = _white_pitches(cfg.low_pitch, cfg.high_pitch)
        # x span of each white key (left inclusive, right exclusive).
        self.white_spans: list[tuple[int, int]] = []
        x = cfg.margin_x
        for _ in self.white_pitches:
            self.white_spans.append((x, x + cfg.white_w))
            x += cfg.white_w
        self.width = x + cfg.margin_x
        # Black keys: centred on the boundary between consecutive white keys
        # whose left key has a black neighbour to the right.
        self.black: list[tuple[int, int, int]] = []  # (x0, x1, pitch)
        for i, p in enumerate(self.white_pitches[:-1]):
            if _HAS_BLACK_RIGHT[_white_index(p)]:
                boundary = self.white_spans[i][1]
                bx0 = boundary - cfg.black_w // 2
                bx1 = bx0 + cfg.black_w
                self.black.append((bx0, bx1, p + 1))

    def pitch_x_range(self, pitch: int) -> Optional[tuple[int, int]]:
        """Return the full (x0, x1) key span for ``pitch`` (used for calibration
        ground truth, not for drawing bars)."""
        for i, p in enumerate(self.white_pitches):
            if p == pitch:
                return self.white_spans[i]
        for (bx0, bx1, bp) in self.black:
            if bp == pitch:
                return (bx0, bx1)
        return None

    def lane_x_range(self, pitch: int) -> Optional[tuple[int, int]]:
        """The (x0, x1) pixel span where a falling *bar* for ``pitch`` is drawn.

        Like real Synthesia, white and black lanes do **not** overlap: a white
        bar occupies only the visible white strip between its neighbouring black
        keys, so it never bleeds into a black lane (and vice versa).
        """
        if (pitch % 12) in (1, 3, 6, 8, 10):  # black key
            for (bx0, bx1, bp) in self.black:
                if bp == pitch:
                    return (bx0, bx1)
            return None
        for i, p in enumerate(self.white_pitches):
            if p == pitch:
                x0, x1 = self.white_spans[i]
                center = (x0 + x1) // 2
                half = max(3, (self.cfg.white_w - self.cfg.black_w) // 2)
                return (center - half, center + half)
        return None


def _white_index(pitch: int) -> int:
    """Index 0..6 of a white pitch within its octave (C=0 .. B=6)."""
    return _WHITE_SEMITONES.index(pitch % 12)


def render_frames(notes: list[SynthNote], cfg: SynthConfig) -> tuple[list[np.ndarray], SynthKeyboard]:
    """Render the full clip as a list of BGR frames plus the keyboard layout."""
    kb = SynthKeyboard(cfg)
    width = kb.width
    height = cfg.height
    hit = cfg.hit_line
    v = cfg.scroll_px_per_s

    # Total duration: last note must fully exit the falling region.
    last_off = max((n.start_us + n.dur_us for n in notes), default=0) / 1e6
    lookahead = cfg.falling_h / v
    total_s = last_off + lookahead + 0.2
    n_frames = max(1, int(round(total_s * cfg.fps)))

    # Static keyboard layer drawn once, copied per frame.
    keyboard = np.zeros((height, width, 3), dtype=np.uint8)
    keyboard[:] = _BG
    kb_x0 = kb.white_spans[0][0]
    kb_x1 = kb.white_spans[-1][1]
    keyboard[hit:height, kb_x0:kb_x1] = _WHITE  # only the key area, not the margins
    for (x0, _x1) in kb.white_spans[1:]:  # 1px separators between keys
        keyboard[hit:height, x0 : x0 + 1] = _BLACK
    for (bx0, bx1, _bp) in kb.black:
        keyboard[hit : hit + cfg.black_h, bx0:bx1] = _BLACK

    frames: list[np.ndarray] = []
    for f in range(n_frames):
        t = f / cfg.fps
        frame = keyboard.copy()
        for note in notes:
            span = kb.lane_x_range(note.pitch)
            if span is None:
                continue
            x0, x1 = span
            start = note.start_us / 1e6
            dur = note.dur_us / 1e6
            # Bottom edge reaches hit-line at onset; top at note-off.
            bottom_y = hit + (t - start) * v
            top_y = hit + (t - start - dur) * v
            y0 = int(round(top_y))
            y1 = int(round(bottom_y))
            # Clip to the falling region (above the hit-line).
            y0 = max(0, y0)
            y1 = min(hit, y1)
            if y1 <= y0:
                continue
            color = LEFT_COLOR if note.hand == "Left" else RIGHT_COLOR
            # Black-key bars are drawn narrower (centred), like Synthesia.
            frame[y0:y1, x0:x1] = color
        frames.append(frame)
    return frames, kb


def c_major_demo(cfg: Optional[SynthConfig] = None) -> tuple[list[SynthNote], SynthConfig]:
    """A small two-hand fixture: right-hand C-major scale + a left-hand line and
    a closing chord. Returns the ground-truth notes and the config used."""
    cfg = cfg or SynthConfig()
    notes: list[SynthNote] = []
    # Right hand: C4..C5 scale, one note every 400ms, 300ms long.
    scale = [60, 62, 64, 65, 67, 69, 71, 72]
    step = 400_000
    dur = 300_000
    for i, p in enumerate(scale):
        notes.append(SynthNote(p, i * step, dur, "Right"))
    # Left hand: sustained lower notes under the scale.
    notes.append(SynthNote(48, 0, 700_000, "Left"))  # C3
    notes.append(SynthNote(55, 800_000, 700_000, "Left"))  # G3
    notes.append(SynthNote(48, 1_600_000, 700_000, "Left"))  # C3
    # Closing chord (right hand): C-E-G together.
    chord_start = len(scale) * step
    for p in (60, 64, 67):
        notes.append(SynthNote(p, chord_start, 500_000, "Right"))
    return notes, cfg
