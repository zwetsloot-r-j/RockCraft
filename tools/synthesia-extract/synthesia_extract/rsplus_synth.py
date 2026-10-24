"""Synthetic Rocksmith+-style piano-highway clip generator (M6-G tests).

Renders a *fabricated* perspective note highway over a drawn 3D-ish keyboard for
a known note list, so tests can assert :mod:`synthesia_extract.rsplus` recovers
the ground truth.  It reproduces only the structural cues the extractor reads —
never real footage, artwork or a song:

* a cyan hit line with slanted cyan highway edges converging on a vanishing
  point;
* a keyboard below the hit line whose key lines share that vanishing point: a
  black-key band, a white-only band with thin separators, a solid dark box on
  the middle-C label and thin "C" letters on the other Cs;
* white-key lanes centred on the part of the key visible between black keys;
* bars drawn as glassy tubes (bright edges, dimmer body), hollow black-key
  bars, pale "missed" bars, white "MISS" text on the hit line, a drifting teal
  fog glow, and magenta/teal hand-zone tints on the floor.

Frames are produced lazily (:meth:`RsSynth.frames`) so a test clip never has to
sit in memory whole.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Iterator, Optional

import cv2
import numpy as np

_WHITE_PCS = (0, 2, 4, 5, 7, 9, 11)
_BLACK_AFTER = (True, True, False, True, True, True, False)


def _hsv(h: int, s: int, v: int) -> tuple[int, int, int]:
    """OpenCV HSV -> BGR tuple."""
    px = cv2.cvtColor(np.uint8([[[h, s, v]]]), cv2.COLOR_HSV2BGR)[0, 0]
    return int(px[0]), int(px[1]), int(px[2])


CYAN = _hsv(90, 220, 240)
FLOOR = (30, 30, 30)
WHITE_KEY = (150, 150, 150)
BLACK_KEY = (20, 20, 20)
SEPARATOR = (40, 40, 40)
LEFT_TINT = _hsv(140, 50, 70)
RIGHT_TINT = _hsv(92, 140, 70)
GLOW_HUE = 98

# Bar styles.
TUBE = "tube"  # bright edges, mid-bright body (white-key bars)
HOLLOW = "hollow"  # bright edges, dark translucent body (black-key bars)
PALE = "pale"  # no bright edges, pale body (a missed short note)


@dataclass(frozen=True)
class RsNote:
    pitch: int
    start_us: int
    dur_us: int
    hue: int = 150  # bar colour (OpenCV hue); stay clear of the teal 80-114 band
    style: str = TUBE
    miss_from_us: Optional[int] = None  # white "MISS" text on the hit line...
    miss_to_us: Optional[int] = None  # ...over this note during [from, to)


@dataclass(frozen=True)
class Zone:
    low: int  # lowest pitch covered
    high: int  # highest pitch covered
    start_us: int
    end_us: int
    hand: str  # "Left" / "Right"


@dataclass(frozen=True)
class Glow:
    x: float  # centre x at the hit line
    start_us: int
    end_us: int
    sigma: float = 40.0


@dataclass
class RsConfig:
    width: int = 800
    height: int = 480
    fps: float = 30.0
    hit: int = 380  # top row of the cyan hit line (4 rows thick)
    top: int = 120  # far end of the highway
    vp: tuple[float, float] = (400.0, -200.0)
    key_w: float = 34.0  # white-key width at the hit line
    n_keys: int = 19  # white keys across the highway
    first_white_pitch: int = 48  # C3: the leftmost white key in the highway
    extra_keys: int = 6  # keyboard keys drawn beyond each highway edge
    scroll: float = 150.0  # px/s near the hit line
    middle_c_box: bool = True
    duration_s: float = 4.0


@dataclass
class RsSynth:
    notes: list[RsNote]
    cfg: RsConfig = field(default_factory=RsConfig)
    zones: list[Zone] = field(default_factory=list)
    glows: list[Glow] = field(default_factory=list)

    # --- geometry ------------------------------------------------------------

    def _scale(self, y: float) -> float:
        vy = self.cfg.vp[1]
        return (y - vy) / (self.cfg.hit - vy)

    def _x(self, x_hit: float, y: float) -> float:
        """A point on a vanishing line, given its x at the hit line."""
        vx = self.cfg.vp[0]
        return vx + (x_hit - vx) * self._scale(y)

    def _x_left(self) -> float:
        return self.cfg.vp[0] - self.cfg.n_keys * self.cfg.key_w / 2

    def boundary(self, i: int) -> float:
        """x at the hit line of the boundary left of highway white key i."""
        return self._x_left() + i * self.cfg.key_w

    def _white_pitch(self, i: int) -> int:
        base = self.cfg.first_white_pitch
        start = _WHITE_PCS.index(base % 12)
        octave, step = divmod(start + i, 7)
        return base - base % 12 + 12 * octave + _WHITE_PCS[step]

    def _black_after(self, i: int) -> bool:
        """Is there a black key on the boundary right of white key i?"""
        start = _WHITE_PCS.index(self.cfg.first_white_pitch % 12)
        return _BLACK_AFTER[(start + i) % 7]

    @property
    def black_w(self) -> float:
        return 0.6 * self.cfg.key_w

    def lane_x(self, pitch: int) -> float:
        """Lane centre at the hit line (the extractor's ground truth)."""
        for i in range(-self.cfg.extra_keys, self.cfg.n_keys + self.cfg.extra_keys):
            p = self._white_pitch(i)
            if p == pitch:
                lo = self.boundary(i)
                hi = self.boundary(i + 1)
                if self._black_after(i - 1):
                    lo += self.black_w / 2
                if self._black_after(i):
                    hi -= self.black_w / 2
                return (lo + hi) / 2
            if p + 1 == pitch and self._black_after(i):
                return self.boundary(i + 1)
        raise ValueError(f"pitch {pitch} not on the keyboard")

    def is_black(self, pitch: int) -> bool:
        return pitch % 12 not in _WHITE_PCS

    @property
    def highway_pitches(self) -> list[int]:
        out = []
        for i in range(self.cfg.n_keys):
            p = self._white_pitch(i)
            out.append(p)
            if self._black_after(i) and i + 1 < self.cfg.n_keys:
                out.append(p + 1)
        return out

    def _y(self, t_us: float, now_us: float) -> float:
        """Row at which song-time ``t_us`` sits at playback time ``now_us``."""
        return self.cfg.hit - (t_us - now_us) / 1e6 * self.cfg.scroll

    # --- static layers ---------------------------------------------------------

    def _keyboard(self) -> np.ndarray:
        cfg = self.cfg
        img = np.zeros((cfg.height, cfg.width, 3), np.uint8)
        kb_top = cfg.hit + 5
        black_end = kb_top + 40
        kb_end = cfg.height - 12
        lo_i, hi_i = -cfg.extra_keys, cfg.n_keys + cfg.extra_keys
        for y in range(kb_top, kb_end):
            row = img[y]
            xs = [self._x(self.boundary(i), y) for i in range(lo_i, hi_i + 1)]
            x0, x1 = max(16, int(xs[0])), min(cfg.width - 16, int(xs[-1]))
            row[x0:x1] = WHITE_KEY
            for i, x in zip(range(lo_i, hi_i + 1), xs):
                xi = int(round(x))
                if x0 <= xi < x1:
                    row[max(0, xi - 1) : xi + 1] = SEPARATOR
                if y < black_end and self._black_after(i - 1):
                    half = self.black_w * self._scale(y) / 2
                    a, b = int(round(x - half)), int(round(x + half))
                    a, b = max(x0, a), min(x1, b)
                    if a < b:
                        row[a:b] = BLACK_KEY
        # Labels: a solid box on middle C, thin letters on the other Cs.
        ly = (black_end + kb_end) // 2 + 6
        for i in range(lo_i, hi_i):
            if self._white_pitch(i) % 12 != 0:
                continue
            cx = self._x((self.boundary(i) + self.boundary(i + 1)) / 2, ly)
            if self._white_pitch(i) == 60 and cfg.middle_c_box:
                hw = 0.35 * cfg.key_w * self._scale(ly)
                cv2.rectangle(img, (int(cx - hw), ly - 10), (int(cx + hw), ly + 8), (25, 25, 25), -1)
            else:
                cv2.putText(img, "C", (int(cx - 5), ly + 5), cv2.FONT_HERSHEY_SIMPLEX, 0.4, (40, 40, 40), 1)
        return img

    def _highway(self) -> np.ndarray:
        cfg = self.cfg
        img = np.zeros((cfg.height, cfg.width, 3), np.uint8)
        xl, xr = self._x_left(), self._x_left() + cfg.n_keys * cfg.key_w
        poly = np.array(
            [
                (self._x(xl, cfg.top), cfg.top),
                (self._x(xr, cfg.top), cfg.top),
                (xr, cfg.hit),
                (xl, cfg.hit),
            ],
            np.int32,
        )
        cv2.fillConvexPoly(img, poly, FLOOR)
        return img

    def _edges(self, img: np.ndarray) -> None:
        cfg = self.cfg
        xl, xr = self._x_left(), self._x_left() + cfg.n_keys * cfg.key_w
        for x in (xl, xr):
            cv2.line(img, (int(self._x(x, cfg.top)), cfg.top), (int(x), cfg.hit), CYAN, 2)
        cv2.rectangle(img, (int(xl) - 2, cfg.hit), (int(xr) + 2, cfg.hit + 3), CYAN, -1)

    # --- per-frame -------------------------------------------------------------

    def _lane_poly(self, x_hit: float, half: float, y0: float, y1: float) -> np.ndarray:
        return np.array(
            [
                (self._x(x_hit - half, y0), y0),
                (self._x(x_hit + half, y0), y0),
                (self._x(x_hit + half, y1), y1),
                (self._x(x_hit - half, y1), y1),
            ],
            np.int32,
        )

    def render(self, now_us: float) -> np.ndarray:
        cfg = self.cfg
        if not hasattr(self, "_static"):
            self._static = self._keyboard()
            self._floor = self._highway()
        img = self._static.copy()
        floor = self._floor.copy()
        mask = floor.any(axis=2)

        # Hand zones: floor tint over a lane range, scrolling with song-time.
        for z in self.zones:
            y0 = max(cfg.top, self._y(z.end_us, now_us))
            y1 = min(cfg.hit - 1, self._y(z.start_us, now_us))
            if y0 >= y1:
                continue
            lo = self.lane_x(z.low) - (self.black_w / 2 if self.is_black(z.low) else 0.5 * cfg.key_w)
            hi = self.lane_x(z.high) + (self.black_w / 2 if self.is_black(z.high) else 0.5 * cfg.key_w)
            colour = LEFT_TINT if z.hand == "Left" else RIGHT_TINT
            poly = self._lane_poly((lo + hi) / 2, (hi - lo) / 2, y0, y1)
            cv2.fillConvexPoly(floor, poly, colour)

        # Teal fog glows over the floor near the hit line.
        for g in self.glows:
            if not g.start_us <= now_us < g.end_us:
                continue
            xs = np.arange(cfg.width, dtype=np.float32)
            v = (125 * np.exp(-((xs - g.x) ** 2) / (2 * g.sigma**2))).astype(np.uint8)
            band = np.zeros((1, cfg.width, 3), np.uint8)
            band[0, :, 0] = GLOW_HUE
            band[0, :, 1] = 150
            band[0, :, 2] = v
            glow = cv2.cvtColor(band, cv2.COLOR_HSV2BGR)
            rows = slice(cfg.hit - 30, cfg.hit)
            floor[rows] = np.where(
                (glow.sum(axis=2) > floor[rows].sum(axis=2))[..., None] & mask[rows][..., None],
                glow,
                floor[rows],
            )

        img[mask] = floor[mask]

        # Bars (only above the hit line; Rocksmith+ bars end on it).
        for n in self.notes:
            end = n.start_us + n.dur_us
            if end <= now_us:
                continue
            y_bottom = min(cfg.hit - 1, self._y(max(n.start_us, now_us), now_us))
            y_top = max(cfg.top, self._y(end, now_us))
            if y_top >= y_bottom:
                continue
            x = self.lane_x(n.pitch)
            black = self.is_black(n.pitch)
            half = (0.22 if black else 0.36) * cfg.key_w
            if n.style == PALE:
                body = _hsv(n.hue, 110, 135)
                edge = None
            elif n.style == HOLLOW:
                body = _hsv(n.hue, 100, 85)
                edge = _hsv(n.hue, 200, 250)
            else:
                body = _hsv(n.hue, 150, 175)
                edge = _hsv(n.hue, 200, 250)
            cv2.fillConvexPoly(img, self._lane_poly(x, half, y_top, y_bottom), body)
            if edge is not None:
                for side in (-1, 1):
                    xe = x + side * (half - 1.5)
                    cv2.line(
                        img,
                        (int(round(self._x(xe, y_top))), int(y_top)),
                        (int(round(self._x(xe, y_bottom))), int(y_bottom)),
                        edge,
                        3,
                    )
            if n.miss_from_us is not None and n.miss_from_us <= now_us < (n.miss_to_us or 0):
                cv2.rectangle(
                    img,
                    (int(x - 0.4 * cfg.key_w), cfg.hit - 9),
                    (int(x + 0.4 * cfg.key_w), cfg.hit - 1),
                    (245, 245, 245),
                    -1,
                )

        self._edges(img)
        return img

    def n_frames(self) -> int:
        return int(round(self.cfg.duration_s * self.cfg.fps))

    def frames(self) -> Iterator[np.ndarray]:
        for i in range(self.n_frames()):
            yield self.render(i * 1e6 / self.cfg.fps)

    def sample(self, count: int) -> list[np.ndarray]:
        """``count`` frames spread over the clip (for calibration)."""
        n = self.n_frames()
        idx = np.linspace(0, n - 1, num=min(count, n)).round().astype(int)
        return [self.render(i * 1e6 / self.cfg.fps) for i in idx]
