"""Rocksmith+ piano-highway extractor (M6-G).

Rocksmith+ draws its piano chart as a *perspective* highway: lanes converge on
a vanishing point, bars grow as they approach, and a cyan hit line sits on top
of a 3D keyboard.  The Synthesia pipeline's rigid-falling-rectangle model does
not hold there, so this module reads the chart a different way:

* **Read at the hit line, not along the roll.**  A bar is sounding exactly
  while it crosses the hit line, and at that row every lane sits directly over
  its key.  Sampling one thin band just above the cyan line per frame turns the
  video into a per-pitch on/off series — no scroll speed, no perspective unwarp
  of the falling region.
* **Calibrate once per video.**  The keyboard does not move within a song, so
  geometry is fitted from a sparse sample of frames: the cyan hit line, the
  vanishing point (from the highway's slanted side edges), every white-key
  separator (thin dark lines in the white-only band of the keyboard), the black
  keys (dark runs just below the hit line) and the octave anchor (Rocksmith+
  boxes the "C" label of middle C).
* **Project, don't guess.**  Keys and lanes share one vanishing point, so a key
  position measured anywhere on the keyboard maps to the sample row by a single
  perspective scale about that point.

The keys themselves are *not* used as the note signal: Rocksmith+ lights a key
well before its note arrives, so key colour is a preview, not an onset.

Output is the same M6-A :class:`ExtractedChart` the Synthesia path emits.
"""

from __future__ import annotations

import sys
from dataclasses import dataclass, field
from typing import Iterable, Optional, Sequence

import cv2
import numpy as np

from .io import sparse_sample, stream_frames, video_info
from .schema import ExtractedChart, ExtractedNote, Hand, SourceMeta

RSPLUS_EXTRACTOR_VERSION = "rsplus-extract 0.1.0"

# --- colour thresholds (OpenCV HSV: H 0..179, S/V 0..255) -------------------

# The hit line and highway edges: a saturated, bright cyan.
_CYAN_H = (80, 100)
_CYAN_S_MIN = 120
_CYAN_V_MIN = 150
# A note bar crossing the hit line: coloured and brighter than the floor.  The
# hand-zone tints the highway paints behind the bars are saturated but dim
# (V <= ~85), and the white "MISS"/finger-number text is bright but
# unsaturated.  Bars vary a lot: a held bar's edges reach V 250, a missed short
# note passes as a pale translucent block (V ~105-150), and the hit effect
# washes a played bar out to S ~50.
_BAR_S_MIN = 45
_BAR_V_MIN = 100
# The highway also drifts soft teal-to-blue "fog" glows (H ~90-111, V ~100-130)
# that are as bright as a pale bar.  Pixels in that hue range only count as bar
# when clearly bright, or when they stand out from the local average *and* the
# run they form has a sharp edge — a glow is a smooth swell, a bar a ridge with
# a hard boundary (V jumps ~60 within 2 px even on a pale missed note).
_GLOW_H = (85, 114)
_GLOW_V_SURE = 150
_GLOW_CONTRAST = 20
_GLOW_EDGE_JUMP = 35
# Dark pixels: black keys, key separators, the middle-C label box.
_DARK_V = 80
_SEP_V = 95

# Hand-zone tints on the highway floor (OpenCV hue range, saturation range).
# Rocksmith+ shades the left hand's zone a dim, greyish purple-magenta (H
# ~110-165 — glows push it toward blue — S ~25-70) and the right hand's a
# clearly saturated teal (H ~85-100, S ~90+).  Saturation separates them more
# reliably than hue.  Both are dark (V < ~100).
_LEFT_TINT = ((105, 172), (25, 80))
_RIGHT_TINT = ((82, 100), (80, 256))
_TINT_V = (35, 100)

# Pitch classes of the white keys, in order, and which white-key boundaries
# carry a black key: boundary k sits between white key k and k+1 of an octave
# starting at C (C|D, D|E, F|G, G|A, A|B have one; E|F and B|C do not).
_WHITE_PCS = (0, 2, 4, 5, 7, 9, 11)
_BLACK_AFTER = (True, True, False, True, True, True, False)

MIDDLE_C = 60


@dataclass
class Lane:
    """One key's lane at the sample row."""

    pitch: int
    x: float  # lane centre (px) at the sample row
    black: bool


@dataclass
class Geometry:
    """Per-video calibration."""

    hit_top: int
    hit_bottom: int
    sample_rows: list[int]
    vanishing_point: tuple[float, float]
    lanes: list[Lane]
    key_width: float  # median white-key width at the sample row (px)
    anchored: bool  # True when middle C was found from its boxed label
    width: int = 0
    height: int = 0
    notes: list[str] = field(default_factory=list)  # human-readable diagnostics


# --- small helpers ----------------------------------------------------------


def _runs(mask: np.ndarray) -> list[tuple[int, int]]:
    """``[start, end)`` runs of True in a 1-D boolean array."""
    m = np.concatenate(([False], mask.astype(bool), [False]))
    d = np.diff(m.astype(np.int8))
    starts = np.flatnonzero(d == 1)
    ends = np.flatnonzero(d == -1)
    return list(zip(starts.tolist(), ends.tolist()))


def _close_runs(runs: list[tuple[int, int]], max_gap: int) -> list[tuple[int, int]]:
    out: list[tuple[int, int]] = []
    for a, b in runs:
        if out and a - out[-1][1] <= max_gap:
            out[-1] = (out[-1][0], b)
        else:
            out.append((a, b))
    return out


def _cyan(hsv: np.ndarray) -> np.ndarray:
    h, s, v = hsv[..., 0], hsv[..., 1], hsv[..., 2]
    return (h >= _CYAN_H[0]) & (h <= _CYAN_H[1]) & (s >= _CYAN_S_MIN) & (v >= _CYAN_V_MIN)


def _peaks(values: Iterable[float], n_frames: int, tol: float, min_support: float) -> list[float]:
    """Cluster 1-D detections pooled over ``n_frames`` frames.

    Returns the median of each cluster seen in at least ``min_support`` of the
    frames.  Robust to a key being lit (and so missed) in some frames.
    """
    xs = sorted(values)
    clusters: list[list[float]] = []
    for x in xs:
        if clusters and x - clusters[-1][-1] <= tol:
            clusters[-1].append(x)
        else:
            clusters.append([x])
    need = max(1, int(np.ceil(min_support * n_frames)))
    return [float(np.median(c)) for c in clusters if len(c) >= need]


# --- detection --------------------------------------------------------------


def find_hit_line(frame: np.ndarray) -> Optional[tuple[int, int]]:
    """Rows ``(top, bottom)`` of the cyan hit line, or ``None``.

    The hit line is the lowest long horizontal cyan band in the lower 60% of
    the frame (it spans the highway, well over a quarter of the width).
    """
    hsv = cv2.cvtColor(frame, cv2.COLOR_BGR2HSV)
    h, w = frame.shape[:2]
    counts = _cyan(hsv).sum(axis=1)
    rows = [y for y in range(int(h * 0.4), h) if counts[y] >= 0.25 * w]
    if not rows:
        return None
    # The lowest contiguous block.
    bottom = rows[-1]
    top = bottom
    while top - 1 in rows:
        top -= 1
    return top, bottom


def _edge_points(frame: np.ndarray, hit_top: int) -> tuple[list[tuple[float, float]], list[tuple[float, float]]]:
    """Points on the highway's slanted left and right cyan edges."""
    hsv = cv2.cvtColor(frame, cv2.COLOR_BGR2HSV)
    cyan = _cyan(hsv)
    w = frame.shape[1]
    left: list[tuple[float, float]] = []
    right: list[tuple[float, float]] = []
    span = max(20, int(hit_top * 0.6))
    for y in range(hit_top - 8, hit_top - span, -6):
        if y < 0:
            break
        xs = np.flatnonzero(cyan[y])
        if xs.size == 0:
            continue
        runs = _close_runs(_runs(cyan[y]), 2)
        runs = [(a, b) for a, b in runs if b - a <= max(12, w // 60)]
        if len(runs) < 2:
            continue
        lx = (runs[0][0] + runs[0][1] - 1) / 2
        rx = (runs[-1][0] + runs[-1][1] - 1) / 2
        if rx - lx < w * 0.15:
            continue
        left.append((float(y), lx))
        right.append((float(y), rx))
    return left, right


def _fit_line(points: Sequence[tuple[float, float]]) -> Optional[tuple[float, float]]:
    """Robust ``x = a*y + b`` fit (median-of-residuals trimmed least squares)."""
    if len(points) < 3:
        return None
    p = np.asarray(points, dtype=np.float64)
    keep = np.ones(len(p), dtype=bool)
    for _ in range(3):
        a, b = np.polyfit(p[keep, 0], p[keep, 1], 1)
        res = np.abs(p[:, 1] - (a * p[:, 0] + b))
        cut = max(2.0, 2.5 * float(np.median(res[keep])))
        new = res <= cut
        if new.sum() < 3 or (new == keep).all():
            break
        keep = new
    return float(a), float(b)


def is_rsplus(frames: Sequence[np.ndarray]) -> bool:
    """Whether sampled frames look like a Rocksmith+ piano highway.

    Signature: a long cyan hit line in the lower part of the frame, plus cyan
    side edges above it that converge upward (a perspective trapezoid).  A
    Synthesia roll has neither.
    """
    hits = 0
    looked = 0
    for f in frames:
        looked += 1
        hl = find_hit_line(f)
        if hl is None:
            continue
        left, right = _edge_points(f, hl[0])
        lf, rf = _fit_line(left), _fit_line(right)
        # Converging upward: left edge moves right as y decreases (a < 0),
        # right edge moves left (a > 0).
        if lf is not None and rf is not None and lf[0] < -0.1 and rf[0] > 0.1:
            hits += 1
    return looked > 0 and hits >= max(1, looked // 2)


# --- calibration ------------------------------------------------------------


Line = tuple[float, float]  # x = a*y + b


def _vanishing_point(
    frames: Sequence[np.ndarray], hit_top: int
) -> tuple[tuple[float, float], Line, Line]:
    """The highway's vanishing point and its left/right edge lines."""
    left: list[tuple[float, float]] = []
    right: list[tuple[float, float]] = []
    for f in frames:
        lp, rp = _edge_points(f, hit_top)
        left += lp
        right += rp
    lf, rf = _fit_line(left), _fit_line(right)
    if lf is None or rf is None or abs(lf[0] - rf[0]) < 1e-3:
        raise ValueError("Rocksmith+ layout: could not find the highway edges")
    # a1*y + b1 = a2*y + b2
    y = (rf[1] - lf[1]) / (lf[0] - rf[0])
    x = lf[0] * y + lf[1]
    return (float(x), float(y)), lf, rf


def _project(x: float, y_from: float, y_to: float, vp: tuple[float, float]) -> float:
    """Move a point on a line through the vanishing point from row ``y_from`` to ``y_to``."""
    vx, vy = vp
    return vx + (x - vx) * (y_to - vy) / (y_from - vy)


def _keyboard_rows(frames: Sequence[np.ndarray], hit_bottom: int) -> tuple[int, int, int]:
    """``(black_row, white_row, kb_bottom)``.

    Below the hit line the keyboard has a black-key band (white fraction
    ~0.4: black keys interleave white key tops) and then a white-only band
    (~0.65+).  ``black_row`` sits in the upper black band, ``white_row`` just
    inside the white band, ``kb_bottom`` is the last bright row.
    """
    f = frames[len(frames) // 2]
    hsv = cv2.cvtColor(f, cv2.COLOR_BGR2HSV)
    s, v = hsv[..., 1].astype(int), hsv[..., 2].astype(int)
    whiteish = ((v > 100) & (s < 50)).mean(axis=1)
    h = f.shape[0]
    y = hit_bottom + 1
    while y < h and whiteish[y] < 0.2:
        y += 1
    black_top = y
    while y < h and whiteish[y] < 0.55:
        y += 1
    white_top = y
    while y < h and whiteish[y] >= 0.3:
        y += 1
    kb_bottom = y - 1
    if white_top >= h or kb_bottom - white_top < 6 or white_top - black_top < 6:
        raise ValueError("Rocksmith+ layout: could not find the keyboard below the hit line")
    black_row = black_top + max(3, (white_top - black_top) // 5)
    white_row = white_top + max(2, (kb_bottom - white_top) // 8)
    return black_row, white_row, kb_bottom


def _white_separators(frames: Sequence[np.ndarray], row: int) -> list[float]:
    xs: list[float] = []
    for f in frames:
        v = cv2.cvtColor(f[row : row + 1], cv2.COLOR_BGR2HSV)[0, :, 2]
        for a, b in _runs(v < _SEP_V):
            if 1 <= b - a <= 6:
                xs.append((a + b - 1) / 2)
    seps = _peaks(xs, len(frames), tol=3.0, min_support=0.3)
    if len(seps) < 8:
        raise ValueError("Rocksmith+ layout: too few white-key separators found")
    # Fill separators hidden in every sampled frame (labels, lit keys): any gap
    # of ~k key widths gets k-1 evenly spaced fillers.
    gaps = np.diff(seps)
    unit = float(np.median(gaps))
    out = [seps[0]]
    for a, b in zip(seps, seps[1:]):
        k = int(round((b - a) / unit))
        if k <= 0:  # a doubled detection; keep one
            continue
        for i in range(1, k):
            out.append(a + (b - a) * i / k)
        out.append(b)
    return out


def _black_keys(frames: Sequence[np.ndarray], row: int, unit: float) -> list[tuple[float, float]]:
    """``(centre, width)`` of each black key at ``row``, pooled over frames."""
    found: list[tuple[float, float]] = []
    lo, hi = 0.25 * unit, 0.85 * unit
    for f in frames:
        v = cv2.cvtColor(f[row : row + 1], cv2.COLOR_BGR2HSV)[0, :, 2]
        for a, b in _runs(v < _DARK_V):
            if lo <= b - a <= hi:
                found.append(((a + b - 1) / 2, float(b - a)))
    centres = _peaks((c for c, _ in found), len(frames), tol=0.15 * unit, min_support=0.3)
    out = []
    for c in centres:
        ws = [w for x, w in found if abs(x - c) <= 0.15 * unit]
        out.append((c, float(np.median(ws))))
    return out


def _middle_c_box(frames: Sequence[np.ndarray], top: int, bottom: int, unit: float) -> Optional[float]:
    """x (at the band's middle row) of the boxed "C" label marking middle C.

    The box is a solid dark block about 0.7 key wide; an ordinary "C" label is
    a thin letter outline, so a fill-ratio test tells them apart.  The dark
    margins beside the keyboard are solid too, but they span the whole band and
    touch the frame edge; a label does neither.
    """
    xs: list[float] = []
    band_h = bottom - top
    for f in frames:
        band = cv2.cvtColor(f[top:bottom], cv2.COLOR_BGR2HSV)[..., 2] < 70
        n, _lab, stats, cents = cv2.connectedComponentsWithStats(band.astype(np.uint8), connectivity=8)
        for j in range(1, n):
            x, y, w, h, area = stats[j]
            if not (0.4 * unit <= w <= 1.1 * unit and 0.25 * unit <= h <= 0.8 * band_h):
                continue
            if x == 0 or x + w >= band.shape[1]:
                continue
            if area / float(w * h) >= 0.5:
                xs.append(float(cents[j][0]))
    peaks = _peaks(xs, len(frames), tol=0.3 * unit, min_support=0.3)
    if len(peaks) != 1:
        return None
    return peaks[0]


def calibrate(frames: Sequence[np.ndarray], octave_shift: int = 0) -> Geometry:
    """Fit the per-video geometry from a sparse sample of frames."""
    if not frames:
        raise ValueError("Rocksmith+ layout: no frames")
    h, w = frames[0].shape[:2]
    hl = [find_hit_line(f) for f in frames]
    hl = [x for x in hl if x is not None]
    if not hl:
        raise ValueError("Rocksmith+ layout: no cyan hit line found")
    hit_top = int(np.median([a for a, _ in hl]))
    hit_bottom = int(np.median([b for _, b in hl]))
    vp, left_edge, right_edge = _vanishing_point(frames, hit_top)
    black_row, white_row, kb_bottom = _keyboard_rows(frames, hit_bottom)

    seps = _white_separators(frames, white_row)  # boundaries at white_row
    unit_w = float(np.median(np.diff(seps)))
    # Black keys are measured at black_row; express them at white_row.
    blacks_b = _black_keys(frames, black_row, _project(unit_w, white_row, black_row, (0.0, vp[1])))
    blacks = [_project(x, black_row, white_row, vp) for x, _ in blacks_b]
    black_w = [_project(bw, black_row, white_row, (0.0, vp[1])) for _, bw in blacks_b]
    typical_bw = float(np.median(black_w)) if black_w else 0.55 * unit_w

    # Which boundaries carry a black key?
    has_black = []
    for s in seps:
        d = min((abs(s - b) for b in blacks), default=1e9)
        has_black.append(d <= 0.4 * unit_w)
    # Phase: boundary i is boundary (i + phase) % 7 of a C-based octave.
    best_phase, best_score = 0, -1
    for phase in range(7):
        score = sum(1 for i, hb in enumerate(has_black) if hb == _BLACK_AFTER[(i + phase) % 7])
        if score > best_score:
            best_phase, best_score = phase, score
    agreement = best_score / max(1, len(has_black))
    if agreement < 0.8:
        raise ValueError(
            f"Rocksmith+ layout: black-key pattern unclear ({agreement:.0%} agreement)"
        )

    # White key j lies between boundaries j and j+1.  Its *relative* white
    # index j + 1 + phase is unbounded; modulo 7 it is the key's step in a
    # C-based octave (0 = C), and the octave anchor below fixes which C is 60.
    diag: list[str] = []
    white_cx = [(a + b) / 2 for a, b in zip(seps, seps[1:])]  # at white_row
    white_idx = [i + 1 + best_phase for i in range(len(white_cx))]

    # Octave anchor: the boxed middle-C label.
    label_top = white_row
    box_x = _middle_c_box(frames, label_top, kb_bottom, unit_w)
    anchored = False
    c4_rel: Optional[int] = None
    if box_x is not None:
        y_mid = (label_top + kb_bottom) / 2
        bx = _project(box_x, y_mid, white_row, vp)
        j = int(np.argmin([abs(c - bx) for c in white_cx]))
        if white_idx[j] % 7 == 0:
            c4_rel = white_idx[j]
            anchored = True
        else:
            diag.append("middle-C box is not on a C key; ignoring it")
    if c4_rel is None:
        # Fallback: the C nearest the frame centre is middle C.
        cs = [j for j in range(len(white_cx)) if white_idx[j] % 7 == 0]
        if not cs:
            raise ValueError("Rocksmith+ layout: no C key in view")
        j = min(cs, key=lambda k: abs(white_cx[k] - w / 2))
        c4_rel = white_idx[j]
        diag.append(
            "middle C not found from its label; assuming the C nearest the centre "
            "(override with --octave-shift)"
        )

    def white_pitch(rel: int) -> int:
        d = rel - c4_rel
        octave, step = divmod(d, 7)
        return MIDDLE_C + 12 * (octave + octave_shift) + _WHITE_PCS[step]

    # Lanes at the sample row: a thin band just above the cyan line.
    sample_rows = [max(0, hit_top - 4), max(0, hit_top - 3), max(0, hit_top - 2)]
    ys = float(sample_rows[1])
    n_sep = len(seps)

    def boundary(i: int) -> float:
        """Boundary i at white_row, extrapolated one key past either end."""
        if i < 0:
            return seps[0] + i * unit_w
        if i >= n_sep:
            return seps[-1] + (i - n_sep + 1) * unit_w
        return seps[i]

    def black_at(i: int) -> Optional[tuple[float, float]]:
        """``(centre, width)`` at white_row of the black key on boundary i, if any."""
        if not _BLACK_AFTER[(i + best_phase) % 7]:
            return None
        bx = boundary(i)
        near = [k for k, b in enumerate(blacks) if abs(b - bx) <= 0.4 * unit_w]
        if not near:
            return bx, typical_bw
        k = min(near, key=lambda k: abs(blacks[k] - bx))
        return blacks[k], black_w[k]

    kw_s = _project(unit_w, white_row, ys, (0.0, vp[1]))
    lanes: list[Lane] = []
    # The outermost keys often have no visible outer separator (the highway
    # edge hides it), so walk one key past each end; lanes outside the highway
    # are dropped below.
    #
    # A white key's lane is centred on the part of the key visible *between*
    # the black keys, not on the whole key: C and F lanes sit left of their
    # key's centre, E and B right of it, D/G/A on it (~0.13 key either way).
    for i in range(-1, n_sep):
        lb = black_at(i)
        rb = black_at(i + 1)
        lo = lb[0] + lb[1] / 2 if lb else boundary(i)
        hi = rb[0] - rb[1] / 2 if rb else boundary(i + 1)
        lanes.append(
            Lane(
                pitch=white_pitch(i + 1 + best_phase),
                x=_project((lo + hi) / 2, white_row, ys, vp),
                black=False,
            )
        )
    for i in range(-1, n_sep + 1):
        bk = black_at(i)
        if bk is None:
            continue
        lanes.append(
            Lane(
                pitch=white_pitch(i + best_phase) + 1,
                x=_project(bk[0], white_row, ys, vp),
                black=True,
            )
        )

    # Only lanes inside the highway carry notes.
    # A lane hugging an edge (< 0.3 key in) is the edge's own cyan glow, which
    # reads as a bar; the outermost real lanes sit further in.
    left_x = left_edge[0] * ys + left_edge[1] + 0.3 * kw_s
    right_x = right_edge[0] * ys + right_edge[1] - 0.3 * kw_s
    lanes = [ln for ln in lanes if left_x <= ln.x <= right_x and 0 <= ln.pitch <= 127]
    lanes.sort(key=lambda ln: ln.x)
    return Geometry(
        hit_top=hit_top,
        hit_bottom=hit_bottom,
        sample_rows=sample_rows,
        vanishing_point=vp,
        lanes=lanes,
        key_width=kw_s,
        anchored=anchored,
        width=w,
        height=h,
        notes=diag,
    )


# --- per-frame reading ------------------------------------------------------


def _hue_dist(a: np.ndarray, b: float) -> np.ndarray:
    d = np.abs(a.astype(np.float64) - b)
    return np.minimum(d, 180.0 - d)


def _mean_hue(h: np.ndarray) -> float:
    ang = h.astype(np.float64) * (np.pi / 90.0)
    return float((np.degrees(np.arctan2(np.sin(ang).mean(), np.cos(ang).mean())) / 2.0) % 180.0)


# Lane states: a bar clearly crossing, only partly readable, or nothing.
OFF, WEAK, STRONG = 0, 1, 2
# Hand-zone codes in the per-lane zone matrix (-1 = unknown).
LEFT_ZONE, RIGHT_ZONE = 0.0, 1.0

# Overlay text Rocksmith+ draws on the hit line ("MISS", finger numbers): white.
_TEXT_S_MAX = 40
_TEXT_V_MIN = 180


def _lane_state(
    ln: Lane,
    runs: Sequence[tuple[int, int]],
    hue: np.ndarray,
    sat: np.ndarray,
    val: np.ndarray,
    kw: float,
) -> int:
    """How clearly lane ``ln``'s bar crosses the sample band.

    :func:`_bar_state`, except that a lane with no readable bar but white
    overlay text over it is ``WEAK``: Rocksmith+ prints "MISS" right on the hit
    line over every note that was not played, washing the bar out for several
    frames.  Like the hit effect, occlusion can carry a note but never start one.
    """
    st = _bar_state(ln, runs, hue, sat, kw)
    if st != OFF:
        return st
    lo = max(0, int(ln.x - 0.45 * kw))
    hi = min(sat.shape[0], int(ln.x + 0.45 * kw) + 1)
    text = (sat[lo:hi] < _TEXT_S_MAX) & (val[lo:hi] >= _TEXT_V_MIN)
    return WEAK if int(text.sum()) >= 2 else OFF


# A hollow bar's dark body, between its two bright edges, is at most this wide
# (fraction of a white key).  Measured ~0.28 on a black-key bar; the floor
# between two bars on neighbouring white keys is ~0.43 wide, between two
# neighbouring black-key bars ~0.55.
_HOLLOW_BODY_MAX = {True: 0.36, False: 0.45}


def _bar_state(
    ln: Lane,
    runs: Sequence[tuple[int, int]],
    hue: np.ndarray,
    sat: np.ndarray,
    kw: float,
) -> int:
    """How clearly lane ``ln``'s bar crosses the sample band.

    Rocksmith+ bars are glassy tubes: bright, saturated edges around a
    translucent body.  A wide (white-key) bar's body is usually bright enough to
    read as one solid run; a narrow (black-key) bar's body often is not, leaving
    a dark gap between two bright edges.  ``STRONG`` when either

    * one bright run spans the lane centre (with a small margin), or
    * the lane centre sits in the middle of a dark gap no wider than a hollow
      bar's body, the bright pixels bordering it share one hue, and the body
      has that hue too.

    ``WEAK`` when the bordering edges match but the body does not — the hit
    effect Rocksmith+ draws over a bar while it is being played darkens the
    body for a few frames.  A weak frame can carry a note across that effect
    but never starts one.

    The gap-width limit is what keeps two bars on neighbouring keys from
    reading as a bar on the key between them; only the gap is measured, not
    the runs around it, because a neighbouring bar's glow can merge into a
    hollow bar's edge.
    """
    cx = ln.x
    margin = max(2.0, 0.12 * kw)  # keeps a bar's glow out of the next lane
    for a, b in runs:
        if a + margin <= cx <= b - 1 - margin:
            return STRONG
    left = [r for r in runs if r[1] - 1 < cx]
    right = [r for r in runs if r[0] > cx]
    if not left or not right:
        return OFF
    lb, ra = left[-1][1], right[0][0]  # the dark gap is [lb, ra)
    gap = ra - lb
    if gap < 1 or gap > _HOLLOW_BODY_MAX[ln.black] * kw:
        return OFF
    if abs((lb + ra) / 2 - cx) > 0.15 * kw:
        return OFF
    rim = 3
    lh = hue[max(0, lb - rim) : lb][sat[max(0, lb - rim) : lb] >= _BAR_S_MIN]
    rh = hue[ra : ra + rim][sat[ra : ra + rim] >= _BAR_S_MIN]
    if lh.size == 0 or rh.size == 0:
        return OFF
    h_l, h_r = _mean_hue(lh), _mean_hue(rh)
    if float(_hue_dist(np.array([h_l]), h_r)[0]) > 15:
        return OFF  # edges of two different bars
    h0 = _mean_hue(np.concatenate((lh, rh)))
    body_h, body_s = hue[lb:ra], sat[lb:ra]
    coloured = body_s >= 40
    if coloured.mean() >= 0.5 and float((_hue_dist(body_h[coloured], h0) <= 15).mean()) >= 0.6:
        return STRONG
    return WEAK


def sample_band(frame: np.ndarray, geo: Geometry) -> np.ndarray:
    """The sample band of one frame, collapsed to a per-column median HSV row."""
    rows = frame[geo.sample_rows[0] : geo.sample_rows[-1] + 1]
    hsv = cv2.cvtColor(rows, cv2.COLOR_BGR2HSV)
    return np.median(hsv, axis=0).astype(np.uint8)


def bar_mask(band: np.ndarray, kw: float) -> np.ndarray:
    """Columns of a sample band that belong to a bar (see the thresholds above)."""
    hue = band[:, 0].astype(np.int16)
    sat = band[:, 1].astype(np.int16)
    val = band[:, 2].astype(np.int16)
    bar = (sat >= _BAR_S_MIN) & (val >= _BAR_V_MIN)
    glowish = (hue >= _GLOW_H[0]) & (hue <= _GLOW_H[1]) & (val < _GLOW_V_SURE)
    if glowish.any():
        win = max(3, int(round(3 * kw)) | 1)
        local = cv2.blur(val.astype(np.float32).reshape(1, -1), (win, 1)).ravel()
        bar &= ~glowish | (val - local >= _GLOW_CONTRAST)
        n = val.shape[0]
        for a, b in _runs(bar):
            if glowish[a:b].mean() < 0.5:
                continue
            left = int(val[a + 1]) - int(val[max(0, a - 3)]) if b - a > 1 else 0
            right = int(val[b - 2]) - int(val[min(n - 1, b + 2)]) if b - a > 1 else 0
            if max(left, right) < _GLOW_EDGE_JUMP:
                bar[a:b] = False
    return bar


def read_band(band: np.ndarray, geo: Geometry) -> tuple[np.ndarray, np.ndarray]:
    """Per-lane ``(state, floor_zone)`` for one sample band.

    ``state[k]`` is lane k's :func:`_lane_state` (``OFF``/``WEAK``/``STRONG``).
    ``floor_zone[k]`` is the hand zone (``LEFT_ZONE``/``RIGHT_ZONE``) tinting
    lane k's floor where no bar covers it, or -1.
    """
    hue = band[:, 0].astype(np.int16)
    sat = band[:, 1].astype(np.int16)
    val = band[:, 2].astype(np.int16)
    bar = bar_mask(band, geo.key_width)
    runs = _close_runs(_runs(bar), 2)
    w = bar.shape[0]
    state = np.zeros(len(geo.lanes), dtype=np.int8)
    zone = np.full(len(geo.lanes), -1.0)
    for k, ln in enumerate(geo.lanes):
        if ln.x < 0 or ln.x >= w:
            continue
        st = _lane_state(ln, runs, hue, sat, val, geo.key_width)
        state[k] = st
        if st == OFF:
            zone[k] = _floor_zone(hue, sat, val, ln.x, geo.key_width)
    return state, zone


def read_frame(frame: np.ndarray, geo: Geometry) -> tuple[np.ndarray, np.ndarray]:
    """:func:`read_band` of one frame."""
    return read_band(sample_band(frame, geo), geo)


def refine_lanes(geo: Geometry, bands: Sequence[np.ndarray], log=None) -> Geometry:
    """Snap lane centres to where the bars actually cross the sample band.

    Lanes are projected from the keyboard, and a small vanishing-point error
    grows toward the edges of the highway (several px on a 44 px key).  The
    bars themselves are the ground truth: collect the centres of bar-sized runs
    over the whole video, measure each lane's median offset, and fit a smooth
    (linear in x) correction through those offsets — lanes no bar ever touched
    get the fitted value too.  Without enough evidence the geometry is kept.
    """
    kw = geo.key_width
    xs = np.array([ln.x for ln in geo.lanes])
    if xs.size == 0:
        return geo
    offsets: dict[int, list[float]] = {}
    step = max(1, len(bands) // 3000)  # a few thousand frames is plenty
    for band in bands[::step]:
        for a, b in _close_runs(_runs(bar_mask(band, kw)), 2):
            if not (0.3 * kw <= b - a <= 0.9 * kw):
                continue
            c = (a + b - 1) / 2
            k = int(np.argmin(np.abs(xs - c)))
            if abs(xs[k] - c) <= 0.3 * kw:
                offsets.setdefault(k, []).append(c - xs[k])
    pts = [(xs[k], float(np.median(v)), len(v)) for k, v in offsets.items() if len(v) >= 5]
    if len(pts) < 3:
        return geo
    px = np.array([p[0] for p in pts])
    po = np.array([p[1] for p in pts])
    pw = np.sqrt(np.array([p[2] for p in pts], dtype=np.float64))
    a, b = np.polyfit(px, po, 1, w=pw)
    shift = a * xs + b
    if np.max(np.abs(shift)) > 0.35 * kw:
        if log:
            log("rsplus: lane refinement implausible; keeping projected lanes")
        return geo
    for ln, d in zip(geo.lanes, shift):
        ln.x += float(d)
    if log:
        log(
            f"rsplus: lanes refined from {sum(p[2] for p in pts)} bar crossings "
            f"(shift {shift.min():+.1f}..{shift.max():+.1f} px)"
        )
    return geo


def _floor_zone(hue: np.ndarray, sat: np.ndarray, val: np.ndarray, cx: float, kw: float) -> float:
    """Hand zone tinting the floor at a lane centre: ``LEFT_ZONE``, ``RIGHT_ZONE`` or -1.

    Rocksmith+ shades each hand's zone on the highway floor as a rectangle of
    lanes x time that scrolls with the notes.  Zone boundaries follow lane
    boundaries, so the only reliable reading of a lane's zone is its own floor;
    beside a bar is already the neighbouring lane, which at a zone edge belongs
    to the other hand.
    """
    w = hue.shape[0]
    lo, hi = max(0, int(cx - 0.15 * kw)), min(w, int(cx + 0.15 * kw) + 1)
    h, s, v = hue[lo:hi], sat[lo:hi], val[lo:hi]
    floor = (v >= _TINT_V[0]) & (v < _TINT_V[1])

    def count(tint: tuple[tuple[int, int], tuple[int, int]]) -> int:
        (h0, h1), (s0, s1) = tint
        return int((floor & (h >= h0) & (h <= h1) & (s >= s0) & (s < s1)).sum())

    left, right = count(_LEFT_TINT), count(_RIGHT_TINT)
    if max(left, right) < 0.5 * (hi - lo):
        return -1.0
    return LEFT_ZONE if left > right else RIGHT_ZONE


def _note_hand(
    zones: np.ndarray, a: int, b: int, prev_end: Optional[int], next_start: Optional[int], fps: float
) -> Hand:
    """A note's hand from its lane's floor just before it starts and after it ends.

    ``zones`` is the lane's per-frame floor zone (-1 while a bar covers it).  A
    zone spans a phrase, so the floor on the side where the phrase continues —
    toward a neighbouring note on the same lane within a second and a half —
    is the trustworthy one; the other side of a phrase's first or last note
    already shows whatever comes before or after the zone.
    """
    reach = max(3, int(round(0.25 * fps)))
    near = int(round(1.5 * fps))
    w_before = 2 if prev_end is not None and a - prev_end <= near else 1
    w_after = 2 if next_start is not None and next_start - b <= near else 1
    before = zones[max(0, a - reach) : a]
    after = zones[b : b + reach]
    left = w_before * int((before == LEFT_ZONE).sum()) + w_after * int((after == LEFT_ZONE).sum())
    right = w_before * int((before == RIGHT_ZONE).sum()) + w_after * int((after == RIGHT_ZONE).sum())
    if left == right:
        return Hand.UNKNOWN
    return Hand.LEFT if left > right else Hand.RIGHT


# --- assembling notes -------------------------------------------------------


def series_to_notes(
    state: np.ndarray,
    zones: np.ndarray,
    lanes: Sequence[Lane],
    fps: float,
    min_frames: int = 2,
) -> list[ExtractedNote]:
    """Turn a ``(frames, lanes)`` matrix of lane states into notes.

    A note is a maximal run of lit (``WEAK`` or ``STRONG``) frames that holds at
    least ``min_frames`` ``STRONG`` frames; leading/trailing weak frames are
    trimmed, so a note starts and ends on a clear reading.  Rocksmith+ draws a
    visible gap between repeated notes on one key — an ``OFF`` frame — which
    splits them.  ``confidence`` is the fraction of the note's frames read as
    ``STRONG``.
    """
    notes: list[ExtractedNote] = []
    frame_us = 1e6 / fps
    for k, ln in enumerate(lanes):
        col = state[:, k]
        spans: list[tuple[int, int, float]] = []
        for a, b in _runs(col > OFF):
            strong = np.flatnonzero(col[a:b] == STRONG)
            if strong.size < min_frames:
                continue
            a, b = a + int(strong[0]), a + int(strong[-1]) + 1
            spans.append((a, b, float((col[a:b] == STRONG).mean())))
        for i, (a, b, cover) in enumerate(spans):
            prev_end = spans[i - 1][1] if i > 0 else None
            next_start = spans[i + 1][0] if i + 1 < len(spans) else None
            notes.append(
                ExtractedNote(
                    pitch=ln.pitch,
                    start_us=int(round(a * frame_us)),
                    dur_us=int(round((b - a) * frame_us)),
                    hand=_note_hand(zones[:, k], a, b, prev_end, next_start, fps),
                    confidence=round(cover, 3),
                )
            )
    notes.sort(key=lambda nt: (nt.start_us, nt.pitch))
    return notes


def extract_frames(
    frames: Iterable[np.ndarray],
    fps: float,
    calib_frames: Sequence[np.ndarray],
    *,
    title: Optional[str] = None,
    octave_shift: int = 0,
    log=None,
) -> ExtractedChart:
    """Extract a chart from a frame stream (calibrated on ``calib_frames``)."""
    log = log or (lambda msg: print(msg, file=sys.stderr))
    geo = calibrate(calib_frames, octave_shift=octave_shift)
    lo = min(ln.pitch for ln in geo.lanes) if geo.lanes else 0
    hi = max(ln.pitch for ln in geo.lanes) if geo.lanes else 0
    log(
        f"rsplus: hit line rows {geo.hit_top}-{geo.hit_bottom}, "
        f"vanishing point ({geo.vanishing_point[0]:.0f}, {geo.vanishing_point[1]:.0f}), "
        f"{len(geo.lanes)} lanes MIDI {lo}-{hi}, "
        f"middle C {'from label' if geo.anchored else 'GUESSED'}"
    )
    for d in geo.notes:
        log(f"rsplus: {d}")
    bands = [sample_band(f, geo) for f in frames]
    geo = refine_lanes(geo, bands, log=log)
    state_rows: list[np.ndarray] = []
    hue_rows: list[np.ndarray] = []
    for band in bands:
        st, hue = read_band(band, geo)
        state_rows.append(st)
        hue_rows.append(hue)
    if state_rows:
        state_m = np.stack(state_rows)
        hue_m = np.stack(hue_rows)
    else:
        state_m = np.zeros((0, len(geo.lanes)), dtype=np.int8)
        hue_m = np.zeros((0, len(geo.lanes)))
    notes = series_to_notes(state_m, hue_m, geo.lanes, fps)
    src = SourceMeta(
        extractor_version=RSPLUS_EXTRACTOR_VERSION,
        title=title,
        fps=float(fps),
        frame_height_px=int(geo.height),
        hit_line_px=int(geo.hit_top),
    )
    return ExtractedChart(notes=notes, source=src)


def detect_video(path: str, fps_override: Optional[float] = None) -> bool:
    """Whether the video at ``path`` is a Rocksmith+ piano highway (8 sampled frames)."""
    frames, _ = sparse_sample(path, 8, fps_override)
    return is_rsplus(frames)


def extract_video(
    path: str,
    *,
    fps_override: Optional[float] = None,
    title: Optional[str] = None,
    octave_shift: int = 0,
    log=None,
) -> ExtractedChart:
    """Extract a chart from a Rocksmith+ video file.

    Calibrates on 40 frames spread over the clip, then streams every frame at
    full rate, keeping only each frame's one-row sample band in memory.
    """
    calib, _ = sparse_sample(path, 40, fps_override)
    fps, _n, _w, _h = video_info(path, fps_override)
    return extract_frames(
        stream_frames(path), fps, calib, title=title, octave_shift=octave_shift, log=log
    )
