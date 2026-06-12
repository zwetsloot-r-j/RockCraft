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

from .audio import REFERENCE_PEAK_AMPLITUDE

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
    """A ground-truth note for synthesis and comparison.

    ``velocity`` is the ground-truth MIDI velocity (1..127) used only by the
    audio renderer (:func:`render_audio`) and the M6-F fusion tests; the visual
    renderer ignores it (the falling roll carries no dynamics).
    """

    pitch: int
    start_us: int
    dur_us: int
    hand: str  # "Left" / "Right" / "Unknown"
    velocity: int = 64


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


def render_overlay_frames(
    notes: list[SynthNote],
    cfg: SynthConfig,
    *,
    alpha: float = 0.7,
    seed: int = 7,
    lyrics: bool = True,
    animated_blobs: int = 0,
    glow: float = 0.0,
) -> tuple[list[np.ndarray], SynthKeyboard]:
    """Render an *overlay-style* clip: translucent white bars over a busy,
    colorful static background, above a filmed-piano-like keyboard.

    Differences from :func:`render_frames` that the extractor must cope with:

    * the falling region shows saturated artwork, not a dark background;
    * bars are alpha-blended white (no hand colours, near-zero saturation);
    * the keyboard has *faint* grey separators (a photo, not a rendering), so
      white-run calibration fails and the black-key-pattern fallback must run;
    * the bottom rows fade to dark like a filmed piano's front edge;
    * optionally a static "lyrics" text overlay sits mid-highway and changes
      halfway through (it must not drag the scroll estimate to zero).

    ``animated_blobs`` adds the issue-#151 failure class: an *animated music
    video* behind the roll.  Each blob is a tall, bright, lane-narrow shape
    (a character's highlight) drifting smoothly around the falling region with
    a slowly cycling hue — it defeats every cheap per-pixel gate (brighter than
    the plate, narrower than the run cap, taller than the per-frame scroll
    shift so it self-overlaps through the coherence gate, never still long
    enough for the dead-zone map) and must be rejected downstream by the
    note-level colour statistics instead.

    ``glow`` (0..1) adds a lateral halo around every bar, like the bloom real
    videos exhibit: a weaker white blend extending past the bar's own lane into
    its neighbours' centre strips.  This is the *ghost note* failure class —
    the halo rides the hit line in perfect sync with its parent bar, so only
    the neighbour-brightness relation can reject it.
    """
    import cv2

    kb = SynthKeyboard(cfg)
    width, height, hit = kb.width, cfg.height, cfg.hit_line
    v = cfg.scroll_px_per_s

    rng = np.random.default_rng(seed)
    art = rng.integers(0, 256, size=(height // 8 + 1, width // 8 + 1, 3), dtype=np.uint8)
    art = cv2.resize(art, (width, height), interpolation=cv2.INTER_CUBIC)
    art = cv2.GaussianBlur(art, (0, 0), 5)

    static = art.copy()
    # Filmed-piano keyboard: off-white keys, faint separators, dark fade at the
    # bottom edge.  (Faint = far above the rendered-path's <80 dark threshold.)
    kb_x0 = kb.white_spans[0][0]
    kb_x1 = kb.white_spans[-1][1]
    static[hit:height, kb_x0:kb_x1] = (215, 215, 215)
    for (x0, _x1) in kb.white_spans[1:]:
        static[hit:height, x0 : x0 + 1] = (150, 150, 150)
    for (bx0, bx1, _bp) in kb.black:
        static[hit : hit + cfg.black_h, bx0:bx1] = (25, 25, 25)
    static[height - 8 : height, :] = (10, 10, 10)

    last_off = max((n.start_us + n.dur_us for n in notes), default=0) / 1e6
    total_s = last_off + cfg.falling_h / v + 0.2
    n_frames = max(1, int(round(total_s * cfg.fps)))

    frames: list[np.ndarray] = []
    for f in range(n_frames):
        t = f / cfg.fps
        frame = static.copy()
        # Animated-background blobs sit *behind* the bars (drawn first).  Each
        # is a bright, lane-narrow shape falling at *near* (not exactly) the
        # scroll speed — sparkles/petals in the artwork — the worst case for
        # the per-pixel gates: the small velocity mismatch stays inside the
        # coherence gate's tolerance while the steady descent racks up
        # coverage.  Colour varies per blob and per pass (diverse palette).
        for b in range(animated_blobs):
            u = (0.72 + 0.06 * (b % 3)) * v  # fall speed, near scroll
            blob_h = 90 + 12 * (b % 4)
            half_w = 7 + (b % 3)
            travel = hit + 2 * blob_h
            pos = (u * t + (b * 977) % travel) % travel
            pass_idx = int((u * t + (b * 977) % travel) // travel)
            by = pos - blob_h  # blob centre row, entering from above
            home = 30 + ((b * 131 + pass_idx * 61) % 89) / 89.0 * (width - 60)
            bx = home + 14.0 * np.sin(2.0 * np.pi * t / (3.0 + 0.5 * b))
            hue = (b * 53 + pass_idx * 101 + 17) % 180
            sat = 170 + (b * 29 + pass_idx * 43) % 86
            color = cv2.cvtColor(
                np.uint8([[[hue, sat, 255]]]), cv2.COLOR_HSV2BGR
            )[0, 0]
            cv2.ellipse(
                frame,
                (int(bx), int(by)),
                (half_w, blob_h // 2),
                0, 0, 360,
                tuple(int(c) for c in color),
                -1,
            )
        if animated_blobs:
            frame[hit:] = static[hit:]  # animation never covers the keyboard
        for note in notes:
            span = kb.lane_x_range(note.pitch)
            if span is None:
                continue
            x0, x1 = span
            start = note.start_us / 1e6
            dur = note.dur_us / 1e6
            y0 = max(0, int(round(hit + (t - start - dur) * v)))
            y1 = min(hit, int(round(hit + (t - start) * v)))
            if y1 <= y0:
                continue
            if glow > 0.0:
                # Lateral halo: a weaker blend reaching into neighbouring lanes.
                gw = cfg.white_w // 2
                gx0, gx1 = max(0, x0 - gw), min(width, x1 + gw)
                ga = glow * alpha
                halo = frame[y0:y1, gx0:gx1].astype(np.float32)
                frame[y0:y1, gx0:gx1] = (ga * 255.0 + (1.0 - ga) * halo).astype(np.uint8)
            region = frame[y0:y1, x0:x1].astype(np.float32)
            frame[y0:y1, x0:x1] = (alpha * 255.0 + (1.0 - alpha) * region).astype(np.uint8)
        if lyrics:
            text = "LA LA LA" if t < total_s / 2 else "NA NA NA"
            cv2.putText(
                frame, text, (width // 4, hit // 2),
                cv2.FONT_HERSHEY_SIMPLEX, 1.0, (255, 255, 255), 2,
            )
        frames.append(frame)
    return frames, kb


# --------------------------------------------------------------------------- #
# Synthetic audio (M6-F): a clean tone-per-note render, and a full-mix decoy.
# --------------------------------------------------------------------------- #
# A modest sample rate keeps the FFTs in the tests fast while leaving plenty of
# frequency resolution for the piano range (lowest fixture pitch ~130 Hz).
DEFAULT_SAMPLE_RATE = 16_000


def pitch_to_freq(pitch: int) -> float:
    """MIDI note number -> fundamental frequency in Hz (A4 = 440, MIDI 69)."""
    return 440.0 * 2.0 ** ((pitch - 69) / 12.0)


def _note_envelope(n_samples: int, sample_rate: int) -> np.ndarray:
    """A simple percussive envelope: fast attack, gentle decay, short release.

    The envelope peaks at ~1.0 just after the (very short) attack, so the
    rendered note's peak amplitude equals its velocity gain — which is exactly
    what :func:`synthesia_extract.audio.transcribe` reads back as velocity.
    """
    env = np.ones(n_samples, dtype=np.float64)
    attack = min(n_samples, max(1, int(0.003 * sample_rate)))  # ~3 ms
    release = min(n_samples, max(1, int(0.02 * sample_rate)))  # ~20 ms
    env[:attack] = np.linspace(0.0, 1.0, attack)
    env *= np.linspace(1.0, 0.75, n_samples)  # gentle body decay
    env[n_samples - release :] *= np.linspace(1.0, 0.0, release)
    return env


def render_audio(
    notes: list[SynthNote],
    *,
    sample_rate: int = DEFAULT_SAMPLE_RATE,
    total_s: Optional[float] = None,
    harmonics: tuple[float, ...] = (1.0,),
) -> np.ndarray:
    """Render ``notes`` to a clean mono float32 track aligned to the video clock.

    Each note is a tone at its pitch's fundamental (plus optional ``harmonics``),
    started at ``start_us`` and shaped by :func:`_note_envelope`.  The peak
    amplitude of a note equals ``velocity / 127`` (for the default single-harmonic
    tone), which is the velocity contract the transcriber inverts.  Overlapping
    notes simply sum — different pitches occupy different frequency bins, so the
    per-pitch transcriber separates them without interference.
    """
    last_end_us = max((n.start_us + n.dur_us for n in notes), default=0)
    if total_s is None:
        total_s = last_end_us / 1e6 + 0.1
    n_total = max(1, int(round(total_s * sample_rate)))
    out = np.zeros(n_total, dtype=np.float64)

    for note in notes:
        n0 = int(round(note.start_us / 1e6 * sample_rate))
        dur_n = max(1, int(round(note.dur_us / 1e6 * sample_rate)))
        if n0 >= n_total:
            continue
        dur_n = min(dur_n, n_total - n0)
        t = np.arange(dur_n) / sample_rate
        f0 = pitch_to_freq(note.pitch)
        sig = np.zeros(dur_n, dtype=np.float64)
        for h_idx, h_amp in enumerate(harmonics, start=1):
            sig += h_amp * np.sin(2.0 * np.pi * f0 * h_idx * t)
        gain = float(np.clip(note.velocity / 127.0, 0.0, 1.0)) * REFERENCE_PEAK_AMPLITUDE
        out[n0 : n0 + dur_n] += gain * _note_envelope(dur_n, sample_rate) * sig

    return out.astype(np.float32)


def render_full_mix_audio(
    notes: list[SynthNote],
    *,
    sample_rate: int = DEFAULT_SAMPLE_RATE,
    total_s: Optional[float] = None,
    noise_level: float = 0.35,
    seed: int = 0,
) -> np.ndarray:
    """A *decoy*: the piano tones drowned in broadband noise (a stand-in for a
    full band mix with drums/vocals).  Its flat, broadband spectrum is what the
    suitability check rejects so fusion safely no-ops on real-song audio."""
    base = render_audio(
        notes, sample_rate=sample_rate, total_s=total_s, harmonics=(1.0, 0.4, 0.2)
    )
    rng = np.random.default_rng(seed)
    noise = rng.standard_normal(base.shape[0]).astype(np.float32) * float(noise_level)
    return (base + noise).astype(np.float32)


def c_major_demo(cfg: Optional[SynthConfig] = None) -> tuple[list[SynthNote], SynthConfig]:
    """A small two-hand fixture: right-hand C-major scale + a left-hand line and
    a closing chord. Returns the ground-truth notes and the config used."""
    cfg = cfg or SynthConfig()
    notes: list[SynthNote] = []
    # Right hand: C4..C5 scale, one note every 400ms, 300ms long, with a gentle
    # crescendo so the audio pass has distinct velocities to recover.
    scale = [60, 62, 64, 65, 67, 69, 71, 72]
    step = 400_000
    dur = 300_000
    for i, p in enumerate(scale):
        notes.append(SynthNote(p, i * step, dur, "Right", velocity=55 + i * 6))
    # Left hand: sustained lower notes under the scale, played mezzo-forte.
    notes.append(SynthNote(48, 0, 700_000, "Left", velocity=80))  # C3
    notes.append(SynthNote(55, 800_000, 700_000, "Left", velocity=72))  # G3
    notes.append(SynthNote(48, 1_600_000, 700_000, "Left", velocity=84))  # C3
    # Closing chord (right hand): C-E-G together, fortissimo.
    chord_start = len(scale) * step
    for p in (60, 64, 67):
        notes.append(SynthNote(p, chord_start, 500_000, "Right", velocity=112))
    return notes, cfg
