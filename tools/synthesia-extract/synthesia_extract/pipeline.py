"""The extraction pipeline: frames -> ``ExtractedChart``.

Classical CV, no ML.  The algorithm exploits the fact that a Synthesia bar is a
*rigid* falling rectangle: a colored pixel at row ``y`` in a frame at time ``t``
will reach the hit-line after ``(hit_line - y) / v`` seconds, so it represents
song-time ``t + (hit_line - y) / v``.  Mapping every colored pixel of every
frame into this song-time coordinate stitches the whole roll back together
(frames overlap and reinforce), turning per-key occupancy into notes.

Stages:

1. :func:`detect_hit_line` — find the keyboard's top edge.
2. :func:`calibrate_keyboard` — white/black key x-boundaries -> a pitch ruler,
   anchored so middle C lands on MIDI 60 (overridable).
3. :func:`estimate_scroll` — vertical cross-correlation of the falling region
   across frames -> scroll speed (px/s).
4. :func:`build_roll` — accumulate per-pitch song-time coverage + colour.
5. :func:`extract_notes` — threshold coverage into notes; then
   :func:`suppress_neighbor_ghosts` and :func:`filter_color_modes` reject
   what isn't a falling bar (halo bleed, animated-artwork noise).
6. :func:`assign_hands` — map dominant colours to Left/Right.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Sequence

import cv2
import numpy as np

from .schema import EXTRACTOR_VERSION, ExtractedChart, ExtractedNote, Hand, SourceMeta

# White-key semitone offsets within an octave (C=0 .. B=11).
_WHITE_SEMITONES = [0, 2, 4, 5, 7, 9, 11]


@dataclass
class Keyboard:
    """Calibration result: a per-column pitch ruler plus per-key centers.

    Note detection samples a thin strip at each key *centre* (``centers``): a
    white key's centre is never covered by a neighbouring black-key bar, and a
    black key's centre sits on the white-key boundary, so the two never collide —
    this is what keeps overlapping lanes from leaking pitches.
    """

    hit_line: int
    x_to_pitch: np.ndarray  # int array of length width; -1 where no key
    white_centers: list[int]
    white_pitches: list[int]
    centers: list[tuple[int, int]]  # (x_centre, pitch) for every key (white+black)
    strip_half: int  # half-width of the centre sampling strip, in pixels
    anchor_c4_x: int  # x of the column treated as middle C


@dataclass
class _RawNote:
    pitch: int
    start_us: int
    dur_us: int
    coverage: float
    color: np.ndarray  # mean BGR


# --------------------------------------------------------------------------- #
# Colour segmentation
# --------------------------------------------------------------------------- #
def colored_mask(frame: np.ndarray) -> np.ndarray:
    """Boolean mask of saturated (bar) pixels — excludes white/black keys and the
    grey background, which are all near-zero saturation.

    Only valid for *classic* clips (vivid bars on a desaturated background);
    :func:`bar_mask` with a background plate supersedes it whenever enough
    frames exist to build one.
    """
    hsv = cv2.cvtColor(frame, cv2.COLOR_BGR2HSV)
    sat = hsv[:, :, 1]
    val = hsv[:, :, 2]
    return (sat > 60) & (val > 60)


def background_plate(frames: Sequence[np.ndarray], max_samples: int = 64) -> Optional[np.ndarray]:
    """Per-pixel low percentile over sampled frames = the static scene.

    Falling bars (and, in filmed-piano overlays, the hands) are transient and
    only ever *brighten* a pixel, so a low percentile recovers the background
    even in busy lanes.  This single plate makes detection style-agnostic:
    classic clips have a static dark background, overlay clips a static busy
    one — both diff cleanly against their plate.  Returns ``None`` when the
    clip is too short to average anything out.
    """
    n = len(frames)
    if n < 8:
        return None
    idx = np.linspace(0, n - 1, min(n, max_samples)).astype(int)
    stack = np.stack([frames[i] for i in idx])
    return np.percentile(stack, 30, axis=0).astype(np.uint8)


@dataclass
class BackgroundModel:
    """Per-window background plates, for videos whose artwork changes scene."""

    plates: list[np.ndarray]
    window: int  # frames per plate

    def plate_for(self, f: int) -> np.ndarray:
        return self.plates[min(f // self.window, len(self.plates) - 1)]


def background_model(
    frames: Sequence[np.ndarray],
    fps: float,
    window_s: float = 20.0,
    samples_per_window: int = 24,
) -> Optional[BackgroundModel]:
    """A :func:`background_plate` per ~``window_s`` slice of the clip.

    Music-video sources change their backdrop between scenes; one global plate
    then mismatches whole sections, flooding the mask.  Each window samples a
    half-window beyond its own bounds so bars still average out near cuts.
    """
    n = len(frames)
    if n < 8:
        return None
    win = max(1, int(round(window_s * fps)))
    plates: list[np.ndarray] = []
    for w0 in range(0, n, win):
        lo = max(0, w0 - win // 2)
        hi = min(n, w0 + win + win // 2)
        idx = np.linspace(lo, hi - 1, min(hi - lo, samples_per_window)).astype(int)
        stack = np.stack([frames[i] for i in idx])
        plates.append(np.percentile(stack, 30, axis=0).astype(np.uint8))
    return BackgroundModel(plates=plates, window=win)


def _plate_at(bg, f: int) -> Optional[np.ndarray]:
    """Resolve a background argument (None | plate | model) for frame ``f``."""
    if bg is None or isinstance(bg, np.ndarray):
        return bg
    return bg.plate_for(f)


def bar_mask(
    frame: np.ndarray,
    plate: Optional[np.ndarray],
    max_run_w: Optional[int] = None,
) -> np.ndarray:
    """Boolean mask of falling-bar pixels: brighter than the background plate.

    Works for vivid bars on dark backgrounds (classic) and translucent white
    bars over busy artwork (overlay) alike.  Falls back to the saturation
    heuristic when no plate is available.

    ``max_run_w`` (pixels) drops horizontal mask runs wider than a bar can be:
    a falling bar is at most a lane or two wide, while bright *animated*
    background regions (characters, flashes) span tens of lanes.  A horizontal
    morphological opening isolates those wide runs so they can be removed.
    """
    if plate is None:
        return colored_mask(frame)
    pos = frame.astype(np.int16) - plate.astype(np.int16)
    np.clip(pos, 0, None, out=pos)
    mask = (pos.max(axis=2) > 30) & (pos.sum(axis=2) > 60)
    if max_run_w is not None and max_run_w > 0:
        kernel = np.ones((1, max_run_w), dtype=np.uint8)
        wide = cv2.morphologyEx(mask.astype(np.uint8), cv2.MORPH_OPEN, kernel)
        mask &= ~wide.astype(bool)
    return mask


# --------------------------------------------------------------------------- #
# 1. Hit-line
# --------------------------------------------------------------------------- #
def detect_hit_line(frame: np.ndarray) -> int:
    """Row index of the keyboard's top edge (the hit-line).

    The keyboard rows are mostly white (white keys dominate even across the
    black-key band); the falling region above is dark.  The hit-line is the top
    of the contiguous bottom block of high-white-fraction rows.
    """
    white = np.all(frame > 200, axis=2)  # HxW
    white_frac = white.mean(axis=1)  # per-row
    h = frame.shape[0]
    keyboard_row = white_frac > 0.3
    # Walk up from the bottom while we stay in the keyboard block.
    top = h - 1
    if not keyboard_row[top]:
        # Bottom row isn't bright (unexpected) — fall back to the lowest bright row.
        bright = np.where(keyboard_row)[0]
        if bright.size == 0:
            return h  # no keyboard found; whole frame is falling region
        top = int(bright[-1])
    while top > 0 and keyboard_row[top - 1]:
        top -= 1
    return top


# --------------------------------------------------------------------------- #
# 2. Keyboard calibration
# --------------------------------------------------------------------------- #
def _find_runs(mask: np.ndarray) -> list[tuple[int, int]]:
    """Contiguous True runs in a 1-D boolean array as (start, end_exclusive)."""
    runs: list[tuple[int, int]] = []
    in_run = False
    start = 0
    for i, v in enumerate(mask):
        if v and not in_run:
            in_run = True
            start = i
        elif not v and in_run:
            in_run = False
            runs.append((start, i))
    if in_run:
        runs.append((start, len(mask)))
    return runs


def calibrate_keyboard(
    frame: np.ndarray, hit_line: int, anchor_c4_x: Optional[int] = None
) -> Keyboard:
    """Derive the per-column pitch ruler from the keyboard image.

    Tries the rendered-keyboard path first (crisp dark 1-px separators between
    white keys).  When that recovers implausibly few keys — typical for *filmed*
    pianos, where separators are faint — it falls back to reconstructing the
    key grid from the black keys alone (:func:`_calibrate_black_pattern`),
    which are high-contrast in any keyboard image.
    """
    kb = _calibrate_white_runs(frame, hit_line, anchor_c4_x)
    if len(kb.white_centers) >= 10:
        return kb
    fallback = _calibrate_black_pattern(frame, hit_line, anchor_c4_x)
    if fallback is not None and len(fallback.white_centers) > len(kb.white_centers):
        return fallback
    return kb


def _calibrate_white_runs(
    frame: np.ndarray, hit_line: int, anchor_c4_x: Optional[int] = None
) -> Keyboard:
    """Rendered-keyboard calibration: white-key runs split by dark separators."""
    h, w = frame.shape[:2]
    dark = np.all(frame < 80, axis=2)  # HxW

    # White keys: a row near the very bottom is white-keys-only; dark runs are
    # the 1px separators, white runs are the keys.
    wk_row = min(h - 2, hit_line + (h - hit_line) - 2)
    sep = dark[wk_row]  # True at separators / outside the keyboard
    white_runs = [r for r in _find_runs(~sep) if (r[1] - r[0]) >= 4]
    white_centers = [int((a + b) / 2) for (a, b) in white_runs]

    # Black keys: a row just below the hit-line, inside the black-key band. Dark
    # runs wider than a separator are black keys.
    bk_row = min(h - 1, hit_line + max(2, (h - hit_line) // 8))
    bk_dark = dark[bk_row]
    black_runs = [r for r in _find_runs(bk_dark) if (r[1] - r[0]) >= 6]
    black_centers = [int((a + b) / 2) for (a, b) in black_runs]

    n_white = len(white_centers)
    # Which white keys have a black key immediately to their right?
    has_black_right = [False] * n_white
    for bc in black_centers:
        for i in range(n_white - 1):
            if white_centers[i] < bc < white_centers[i + 1]:
                has_black_right[i] = True
                break

    # Anchor letters: a run of exactly two consecutive has_black_right whites is
    # [C, D]; the first is C. (A run of three is [F, G, A].)
    c_indices = _c_white_indices(has_black_right)

    # Choose the C nearest the anchor x (default: keyboard centre) as middle C.
    if anchor_c4_x is None and white_centers:
        anchor_c4_x = (white_centers[0] + white_centers[-1]) // 2
    c4_white_idx = _nearest_index(white_centers, c_indices, anchor_c4_x, n_white)

    # Assign pitches to white keys relative to C4 (= MIDI 60).
    white_pitches: list[int] = []
    for j in range(n_white):
        n = j - c4_white_idx
        octave = n // 7
        within = n % 7
        white_pitches.append(60 + 12 * octave + _WHITE_SEMITONES[within])

    # Build the x -> pitch ruler: white spans first, black spans override.
    x_to_pitch = np.full(w, -1, dtype=np.int32)
    for (a, b), pitch in zip(white_runs, white_pitches):
        x_to_pitch[a:b] = pitch
    pitch_by_white_center = dict(zip(white_centers, white_pitches))
    centers: list[tuple[int, int]] = [
        (cx, p) for cx, p in zip(white_centers, white_pitches)
    ]
    for (a, b) in black_runs:
        bc = int((a + b) / 2)
        # Black pitch = (white to its left) + 1.
        left_pitch = None
        for i in range(n_white - 1):
            if white_centers[i] < bc < white_centers[i + 1]:
                left_pitch = pitch_by_white_center[white_centers[i]]
                break
        if left_pitch is None:
            continue
        x_to_pitch[a:b] = left_pitch + 1
        centers.append((bc, left_pitch + 1))

    # Centre strip half-width: a fraction of the white-key width, but narrow
    # enough that a white centre never falls under an adjacent black key.
    if len(white_centers) >= 2:
        spacing = int(np.median(np.diff(white_centers)))
    else:
        spacing = 8
    strip_half = max(1, spacing // 6)

    return Keyboard(
        hit_line=hit_line,
        x_to_pitch=x_to_pitch,
        white_centers=white_centers,
        white_pitches=white_pitches,
        centers=centers,
        strip_half=strip_half,
        anchor_c4_x=anchor_c4_x if anchor_c4_x is not None else 0,
    )


# Within-octave unit positions of the black keys, C# = 1 (units = white-key
# widths; a black key sits on the boundary between two whites).
_BLACK_UNIT_SEMITONE = {0: 1, 1: 3, 3: 6, 4: 8, 5: 10}  # (unit - anchor) % 7 -> semitone


def _calibrate_black_pattern(
    frame: np.ndarray, hit_line: int, anchor_c4_x: Optional[int] = None
) -> Optional[Keyboard]:
    """Reconstruct the key grid from black keys only (filmed keyboards).

    Black keys are the one reliably high-contrast feature of any keyboard
    image.  Their octave grouping (2 + 3) identifies C#, and the gap sequence
    (1-unit within a group, 2-units across) assigns every black key a position
    on a white-key-unit lattice; the white keys are then generated between
    them.  Returns ``None`` when no plausible black-key row exists.
    """
    h, w = frame.shape[:2]
    gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)

    # Pick the row in the black-key band with the most plausible dark runs.
    best: Optional[tuple[int, list[tuple[int, int]]]] = None
    band_lo = hit_line + 2
    band_hi = min(h - 1, hit_line + max(4, (h - hit_line) // 2))
    for r in range(band_lo, band_hi):
        row = gray[r].astype(np.float64)
        bright = float(np.percentile(row, 90))
        if bright < 60:  # row is mostly dark (shadow/fade) — no key contrast
            continue
        dark = row < 0.45 * bright
        runs = [run for run in _find_runs(dark) if 2 <= run[1] - run[0] <= max(6, w // 20)]
        if best is None or len(runs) > len(best[1]):
            best = (r, runs)
    if best is None or len(best[1]) < 5:
        return None
    runs = best[1]

    # Drop width outliers (shadows, margins).
    widths = [b - a for a, b in runs]
    med_w = float(np.median(widths))
    runs = [(a, b) for (a, b) in runs if 0.4 * med_w <= (b - a) <= 2.5 * med_w]
    centers = [int((a + b) / 2) for a, b in runs]
    if len(centers) < 5:
        return None

    # Snap each gap to integer white-key units (1 within a group, 2 across,
    # 3+ when a black key went undetected).  The 25th percentile is a robust
    # one-unit estimate: small gaps outnumber large ones 3:2 per octave.
    gaps = np.diff(centers).astype(np.float64)
    unit_w0 = float(np.percentile(gaps, 25))
    if unit_w0 <= 0:
        return None
    unit_gaps = [int(np.clip(round(g / unit_w0), 1, 7)) for g in gaps]
    if 2 not in unit_gaps:
        return None  # no across-group gap anywhere -> not a piano pattern
    unit_w = float(np.median([g / u for g, u in zip(gaps, unit_gaps)]))

    # Cumulative unit coordinate per black key (drift-free: integers via gaps).
    units = [0]
    for u in unit_gaps:
        units.append(units[-1] + u)
    deltas = unit_gaps

    # Anchor: C# starts a 2-group -> delta pattern (1, 2, 1, 1); F# starts a
    # 3-group -> (1, 1, 2).
    anchor_unit: Optional[int] = None
    for i in range(len(deltas) - 3):
        if deltas[i : i + 4] == [1, 2, 1, 1]:
            anchor_unit = units[i]  # C#
            break
    if anchor_unit is None:
        for i in range(len(deltas) - 2):
            if deltas[i : i + 3] == [1, 1, 2]:
                anchor_unit = units[i] - 3  # F# is 3 units after C#
                break
    if anchor_unit is None:
        return None

    # Least-squares x(u) so mild perspective doesn't accumulate.
    u_arr = np.array(units, dtype=np.float64)
    x_arr = np.array(centers, dtype=np.float64)
    slope, intercept = np.polyfit(u_arr, x_arr, 1)

    def x_of(u: float) -> float:
        return slope * u + intercept

    # Black pitches relative to the anchor octave's C (=0).
    black_keys: list[tuple[int, int, int]] = []  # (x0, x1, rel_pitch)
    for (a, b), u in zip(runs, units):
        offset = u - anchor_unit  # units from C# of anchor octave
        uu = offset % 7
        if uu not in _BLACK_UNIT_SEMITONE:
            continue  # a dark run not on a black-key slot — noise
        octave = offset // 7
        black_keys.append((a, b, 12 * octave + _BLACK_UNIT_SEMITONE[uu]))
    if len(black_keys) < 5:
        return None

    # White keys: centres at half-units between blacks, spanning the keyboard.
    # White n (n=0 at the anchor octave's C) sits at unit n*(7/7)... in white
    # units: C=0.5, D=1.5, ... B=6.5 relative to that C, whose C# is at unit 1.
    u_c0 = anchor_unit - 1  # unit coordinate of the anchor octave's C|B boundary
    u_min, u_max = min(units), max(units)
    white_centers: list[int] = []
    white_rel: list[int] = []
    n_lo = int(np.floor((u_min - 1.5 - u_c0)))
    n_hi = int(np.ceil((u_max + 1.5 - u_c0)))
    for n in range(n_lo, n_hi + 1):
        cx = x_of(u_c0 + n + 0.5)
        if cx < 0 or cx >= w:
            continue
        octave = n // 7
        within = n % 7
        white_centers.append(int(round(cx)))
        white_rel.append(12 * octave + _WHITE_SEMITONES[within])

    if len(white_centers) < 8:
        return None

    # Absolute octave: the C nearest the anchor x becomes middle C (MIDI 60).
    if anchor_c4_x is None:
        anchor_c4_x = (white_centers[0] + white_centers[-1]) // 2
    c_candidates = [i for i, rp in enumerate(white_rel) if rp % 12 == 0]
    c4_idx = _nearest_index(white_centers, c_candidates, anchor_c4_x, len(white_centers))
    base = 60 - white_rel[c4_idx]
    white_pitches = [base + rp for rp in white_rel]

    # Build the ruler: white spans first, black runs override.
    x_to_pitch = np.full(w, -1, dtype=np.int32)
    half = unit_w / 2.0
    for cx, pitch in zip(white_centers, white_pitches):
        a = max(0, int(round(cx - half)))
        b = min(w, int(round(cx + half)))
        x_to_pitch[a:b] = pitch
    centers_out: list[tuple[int, int]] = list(zip(white_centers, white_pitches))
    for (a, b, rel_pitch) in black_keys:
        x_to_pitch[a:b] = base + rel_pitch
        centers_out.append((int((a + b) / 2), base + rel_pitch))

    return Keyboard(
        hit_line=hit_line,
        x_to_pitch=x_to_pitch,
        white_centers=white_centers,
        white_pitches=white_pitches,
        centers=centers_out,
        strip_half=max(1, int(unit_w) // 6),
        anchor_c4_x=anchor_c4_x,
    )


def _c_white_indices(has_black_right: list[bool]) -> list[int]:
    """White-key indices that are a 'C', inferred from the black-key pattern.

    Within an octave the whites with a black to the right are C, D (a run of 2)
    and F, G, A (a run of 3).  The first element of a 2-run is C; we then step in
    sevens to enumerate every C across the keyboard.
    """
    n = len(has_black_right)
    runs = _find_runs(np.array(has_black_right, dtype=bool))
    anchor = None
    for (a, b) in runs:
        if b - a == 2:  # [C, D]
            anchor = a
            break
    if anchor is None:
        for (a, b) in runs:
            if b - a == 3:  # [F, G, A] -> C is two whites earlier
                anchor = a - 3
                break
    if anchor is None:
        return []
    return [i for i in range(n) if (i - anchor) % 7 == 0]


def _nearest_index(centers: list[int], candidate_idxs: list[int], x: int, n: int) -> int:
    """Pick the candidate white-key index whose centre is nearest ``x``.

    Falls back gracefully when no C was identified so calibration never crashes.
    """
    if not centers:
        return 0
    if not candidate_idxs:
        # No 'C' found: assume the leftmost white is C (best effort).
        return 0
    best = candidate_idxs[0]
    best_d = abs(centers[best] - x)
    for idx in candidate_idxs[1:]:
        d = abs(centers[idx] - x)
        if d < best_d:
            best, best_d = idx, d
    return best


# --------------------------------------------------------------------------- #
# 3. Scroll-speed estimation
# --------------------------------------------------------------------------- #
def _bar_edges(mask: np.ndarray) -> np.ndarray:
    """Horizontal (top/bottom) boundaries of mask regions, one row thick."""
    edges = mask.copy()
    edges[1:] ^= mask[:-1]
    edges[0] = False
    return edges


def estimate_scroll(
    frames: Sequence[np.ndarray],
    hit_line: int,
    fps: float,
    gap: Optional[int] = None,
    plate: Optional[np.ndarray] = None,
    max_run_w: Optional[int] = None,
) -> Optional[float]:
    """Estimate scroll speed (px/s) by cross-correlating the falling-region bar
    mask across frame pairs ``gap`` apart and reading off the vertical shift.

    Correlation runs on bar *edges* with static pixels suppressed (an edge
    present in both frames of a pair hasn't moved), so stationary overlays —
    lyrics text, watermarks — cannot drag the estimate toward zero.
    """
    if gap is None:
        # ~0.13 s between the pair: enough motion to measure, little enough
        # that bars stay in frame.  Adapts to decimated (low-fps) clips.
        gap = max(1, int(round(0.13 * fps)))
    if len(frames) <= gap or hit_line <= 1:
        return None
    max_shift = max(1, hit_line // 2)
    n = len(frames)
    # Pool the full correlation curve over many pairs instead of voting per
    # pair: bars share one coherent velocity for the whole song and pile up at
    # its shift, while *animated* backgrounds (dancing characters, particles)
    # move incoherently and spread their correlation energy across shifts.
    acc = np.zeros(max_shift + 1, dtype=np.float64)
    counted = 0
    # Suppress *slow movers*, not just static pixels: an edge still within
    # ±slop px in the other frame is background jitter (animated characters),
    # not a falling bar — bars travel far more than slop per gap.
    slop = 3
    kernel = np.ones((2 * slop + 1, 1), dtype=np.uint8)
    for i in range(0, n - gap, max(1, (n - gap) // 24 + 1)):
        e0 = _bar_edges(bar_mask(frames[i], _plate_at(plate, i), max_run_w)[:hit_line])
        e1 = _bar_edges(
            bar_mask(frames[i + gap], _plate_at(plate, i + gap), max_run_w)[:hit_line]
        )
        d0 = cv2.dilate(e0.astype(np.uint8), kernel).astype(bool)
        d1 = cv2.dilate(e1.astype(np.uint8), kernel).astype(bool)
        m0 = (e0 & ~d1).astype(np.float32)
        m1 = (e1 & ~d0).astype(np.float32)
        s0, s1 = float(m0.sum()), float(m1.sum())
        if s0 < 10 or s1 < 10:
            continue
        # Per-pair normalisation so busy frames don't dominate the pool.
        norm = float(np.sqrt(s0 * s1))
        for d in range(slop + 1, max_shift + 1):
            acc[d] += float((m0[: hit_line - d] * m1[d:]).sum()) / norm
        counted += 1
    if counted == 0 or acc.max() <= 0.0:
        return None
    return float(np.argmax(acc)) * fps / gap


# --------------------------------------------------------------------------- #
# 4. Roll stitching
# --------------------------------------------------------------------------- #
def build_roll(
    frames: Sequence[np.ndarray],
    fps: float,
    kb: Keyboard,
    scroll: float,
    plate: Optional[np.ndarray] = None,
    max_run_w: Optional[int] = None,
) -> tuple[np.ndarray, np.ndarray, np.ndarray, list[int], float]:
    """Accumulate, per pitch, song-time coverage and mean colour.

    Returns ``(votes, totals, color_sum, pitches, us_per_px)`` where ``votes``,
    ``totals`` and ``color_sum`` are indexed ``[pitch_index, bin]``;
    ``totals[p, bin]`` is the geometric number of frames in which lane ``p``
    could observe song-time ``bin`` (dead-zone rows excluded).
    """
    hit = kb.hit_line
    us_per_px = 1e6 / scroll
    n = len(frames)
    width = kb.x_to_pitch.shape[0]
    sh = kb.strip_half

    pitches = sorted({p for (_cx, p) in kb.centers})
    pidx_of = {p: i for i, p in enumerate(pitches)}
    # Per key: its pitch index and the centre-strip column slice.
    lanes = [
        (pidx_of[p], max(0, cx - sh), min(width, cx + sh + 1), cx)
        for (cx, p) in kb.centers
    ]

    last_off = (n - 1) / fps * scroll
    max_bin = int(round(last_off + hit)) + 2

    votes = np.zeros((len(pitches), max_bin), dtype=np.float32)
    totals = np.zeros((len(pitches), max_bin), dtype=np.float32)
    color_sum = np.zeros((len(pitches), max_bin, 3), dtype=np.float64)

    # Dead-zone map: pixels mask-active for a large fraction of the whole clip
    # are persistent overlays — karaoke lyrics, watermarks, logos — not bars
    # (even a busy lane is bar-covered well under a quarter of the time).
    # Excluded pixels also leave the per-lane totals so coverage stays fair.
    n_dead_samples = min(n, 64)
    dead_idx = np.linspace(0, n - 1, n_dead_samples).astype(int)
    freq = np.zeros((hit, width), dtype=np.float32)
    for f in dead_idx:
        freq += bar_mask(frames[f], _plate_at(plate, f), max_run_w)[:hit]
    dead = (freq / n_dead_samples) > 0.35

    # Scroll-coherence gate: a true bar pixel at (frame f, row y) must appear
    # again at (f+k, y + k*scroll/fps) — bars translate rigidly at the known
    # speed, while bright *animated* artwork only matches that by accident.
    # k is chosen so the shift is at least a few pixels (robust at high fps).
    delta = scroll / fps  # px of travel between consecutive frames
    coh_k = max(1, int(np.ceil(4.0 / max(delta, 1e-6))))
    coh_px = int(round(coh_k * delta))
    coh_kernel = np.ones((5, 1), dtype=np.uint8)  # +/-2 px tolerance

    raw_cache: dict[int, np.ndarray] = {}

    def raw_mask(f: int) -> np.ndarray:
        m = raw_cache.get(f)
        if m is None:
            m = bar_mask(frames[f], _plate_at(plate, f), max_run_w)[:hit]
            raw_cache[f] = m
        return m

    def coherent_mask(f: int) -> np.ndarray:
        now = raw_mask(f)
        if coh_px <= 0 or coh_px >= hit or n <= coh_k:
            return now
        aligned = np.zeros_like(now)
        if f + coh_k < n:  # partner is later: bars sit coh_px lower there
            aligned[: hit - coh_px] = raw_mask(f + coh_k)[coh_px:]
        else:  # tail frames: partner is earlier, bars sat coh_px higher
            aligned[coh_px:] = raw_mask(f - coh_k)[: hit - coh_px]
        aligned = cv2.dilate(aligned.astype(np.uint8), coh_kernel).astype(bool)
        return now & aligned

    # Rows a lane can actually observe: at least one strip column not dead.
    lane_clean = [~dead[:, x0:x1].all(axis=1) for (_pidx, x0, x1, _cx) in lanes]

    rows = np.arange(hit)  # falling-region rows
    for f in range(n):
        o_f = f / fps * scroll
        row_bins = np.round(o_f + hit - rows).astype(np.int64)  # one bin per row
        valid_rows = (row_bins >= 0) & (row_bins < max_bin)

        frame = frames[f]
        mask = coherent_mask(f) & ~dead
        raw_cache.pop(f - coh_k, None)  # keep the cache a small rolling window
        for li, (pidx, x0, x1, cx) in enumerate(lanes):
            observable = valid_rows & lane_clean[li]
            # Geometric coverage: every observable row, once per frame per lane.
            np.add.at(totals[pidx], row_bins[observable], 1.0)
            strip = mask[:, x0:x1].any(axis=1)  # colored at this lane, per row
            on = strip & observable
            if not on.any():
                continue
            rb = row_bins[on]
            # At most one vote per (lane, bin) per frame (rows -> distinct bins).
            np.add.at(votes[pidx], rb, 1.0)
            bgr = frame[np.where(on)[0], cx].astype(np.float64)
            np.add.at(color_sum[pidx], (rb, 0), bgr[:, 0])
            np.add.at(color_sum[pidx], (rb, 1), bgr[:, 1])
            np.add.at(color_sum[pidx], (rb, 2), bgr[:, 2])

    return votes, totals, color_sum, pitches, us_per_px


# --------------------------------------------------------------------------- #
# 5. Notes + hands
# --------------------------------------------------------------------------- #
def extract_notes(
    votes: np.ndarray,
    totals: np.ndarray,
    color_sum: np.ndarray,
    pitches: list[int],
    us_per_px: float,
    *,
    threshold: float = 0.5,
    hysteresis_low: Optional[float] = None,
    min_run_px: int = 3,
) -> list[_RawNote]:
    """Threshold per-pitch coverage into note runs.

    ``totals`` may be global (1-D, one value per bin) or per-lane (2-D,
    ``[pitch_index, bin]``, from the dead-zone-aware roll).

    ``hysteresis_low`` enables dual-threshold *gap bridging*: two solid seeds
    above ``threshold`` (each >= ``min_run_px`` long) merge into one run when
    the gap between them is short (<= ``2 * min_run_px``) and stays above the
    low threshold throughout.  Brief mask dropouts — washed-out scenes,
    dead-zone holes — dip a bar's coverage below the high threshold mid-note
    and would otherwise split one bar into fragments; they rarely dip below
    the low one, while true gaps between repeated notes carry no votes at
    all.  Only interior gaps are bridged — runs never extend past their
    outermost seed bins, so a note's onset/offset always rests on
    high-coverage evidence.
    """
    raws: list[_RawNote] = []
    for pi, pitch in enumerate(pitches):
        tot = totals[pi] if totals.ndim == 2 else totals
        coverage = votes[pi] / np.maximum(tot, 1e-6)
        on = (coverage > threshold) & (tot > 0)
        if hysteresis_low is not None:
            low_on = (coverage > hysteresis_low) & (tot > 0)
            max_gap = 2 * min_run_px
            merged: list[tuple[int, int]] = []
            for a, b in _find_runs(on):
                if b - a < min_run_px:
                    continue
                if merged and a - merged[-1][1] <= max_gap and low_on[merged[-1][1] : a].all():
                    merged[-1] = (merged[-1][0], b)
                else:
                    merged.append((a, b))
            runs = merged
        else:
            runs = _find_runs(on)
        for a, b in runs:
            if b - a < min_run_px:
                continue
            # Colour comes from the high-coverage core bins only: extension
            # bins are weakly attested (their pixels are part background) and
            # would tint the note's colour away from its ink.
            core = on[a:b] if hysteresis_low is not None else np.ones(b - a, dtype=bool)
            core_votes = float(votes[pi, a:b][core].sum())
            total_votes = float(votes[pi, a:b].sum())
            if total_votes <= 0 or core_votes <= 0:
                continue
            color = color_sum[pi, a:b][core].sum(axis=0) / core_votes
            cov = float(np.clip(coverage[a:b].mean(), 0.0, 1.0))
            raws.append(
                _RawNote(
                    pitch=pitch,
                    start_us=int(round(a * us_per_px)),
                    dur_us=int(round((b - a) * us_per_px)),
                    coverage=cov,
                    color=color,
                )
            )
    raws.sort(key=lambda r: (r.start_us, r.pitch))
    return raws


def suppress_neighbor_ghosts(
    raws: list[_RawNote],
    *,
    brightness_ratio: float = 1.2,
    chroma_tol: float = 0.10,
    min_overlap: float = 0.5,
) -> tuple[list[_RawNote], int]:
    """Drop *ghost* notes: a bar's glow bleeding into the adjacent lane.

    Rendered bars (and compressed video) bloom a halo past their own lane, so a
    strong note often drags a faint copy of itself 1–2 semitones away.  A ghost
    is recognisable by all three of: it overlaps a neighbouring-lane note in
    time, that neighbour is much *brighter* (the halo is an attenuated fringe),
    and the two share a chroma direction (a halo has its parent's colour — a
    genuinely different-coloured neighbouring note, e.g. the other hand in a
    close interval, is never suppressed).  Returns ``(survivors, n_dropped)``.
    """
    n = len(raws)
    if n < 2:
        return raws, 0
    colors = np.array([r.color for r in raws], dtype=np.float64)
    brightness = colors.mean(axis=1)
    chroma = colors / np.maximum(np.linalg.norm(colors, axis=1, keepdims=True), 1e-6)
    by_pitch: dict[int, list[int]] = {}
    for i, r in enumerate(raws):
        by_pitch.setdefault(r.pitch, []).append(i)

    keep = np.ones(n, dtype=bool)
    for i, r in enumerate(raws):
        s, e = r.start_us, r.start_us + r.dur_us
        for dp in (-2, -1, 1, 2):
            for j in by_pitch.get(r.pitch + dp, ()):
                other = raws[j]
                overlap = min(e, other.start_us + other.dur_us) - max(s, other.start_us)
                if (
                    overlap > min_overlap * r.dur_us
                    and brightness[j] > brightness_ratio * brightness[i]
                    and float(np.linalg.norm(chroma[i] - chroma[j])) <= chroma_tol
                ):
                    keep[i] = False
                    break
            if not keep[i]:
                break
    survivors = [r for r, k in zip(raws, keep) if k]
    return survivors, n - len(survivors)


def filter_color_modes(
    raws: list[_RawNote],
    *,
    radius: float = 0.12,
    min_mode_frac: float = 0.10,
    satellite_frac: float = 0.02,
    satellite_dist: float = 0.35,
    max_modes: int = 8,
    min_notes: int = 24,
) -> tuple[list[_RawNote], str]:
    """Keep only notes whose colour belongs to a tight, coverage-strong mode;
    return ``(survivors, diagnostic)``.

    Real falling bars are rendered in a handful of solid inks (per hand), so
    their *full* colours — brightness included, since a translucent bar over
    any backdrop stays bright — pile into modes that are simultaneously *tight*
    (every note near one colour) and *strong* (a large share of the total vote
    mass).  Bright animated artwork — characters, sparkles, flashes — is
    colour-diverse: its false notes scatter across colour space and never form
    such a mode.  Dropping the diffuse residue removes them while leaving any
    solid-colour bar population (however styled) untouched.

    A translucent ink additionally spawns *satellite* modes: the same ink over
    each recurring scene backdrop picks up that scene's tint, so rarer scenes
    contribute small-but-tight modes *near* the main one in colour space.
    Those are real notes — a weight floor alone would throw away every note
    played during a rare backdrop.  A small tight mode is therefore also kept
    when it sits within ``satellite_dist`` of an already-accepted mode; noise
    modes (dark shadow shapes, saturated artwork colours) lie far from any bar
    ink and still die regardless of their tightness.

    Fail-safes: clips with too few notes to support the statistics pass through
    unfiltered, as does a population with no qualifying mode at all (we cannot
    tell signal from noise — never destroy the chart on a hunch).
    """
    if len(raws) < min_notes:
        return raws, f"skipped: only {len(raws)} notes"
    colors = np.array([r.color for r in raws], dtype=np.float64) / 255.0
    # Vote mass: how much evidence the roll accumulated for this note.
    weight = np.array([r.coverage * r.dur_us for r in raws], dtype=np.float64)
    total = float(weight.sum())
    if total <= 0:
        return raws, "skipped: zero weight"

    remaining = np.ones(len(raws), dtype=bool)
    keep = np.zeros(len(raws), dtype=bool)
    rng = np.random.default_rng(0)
    accepted_centers: list[np.ndarray] = []
    primaries = satellites = 0
    for _ in range(max_modes):
        idx = np.where(remaining)[0]
        if idx.size == 0:
            break
        # Densest ball: try a bounded sample of the remaining notes as centres
        # and take the one capturing the most weight (O(sample * n), no O(n^2)).
        cand = idx if idx.size <= 256 else rng.choice(idx, 256, replace=False)
        d = np.linalg.norm(colors[idx][None, :] - colors[cand][:, None], axis=2)
        captured = ((d <= radius) * weight[idx][None, :]).sum(axis=1)
        center = colors[cand[int(captured.argmax())]]
        # A few mean-shift steps to settle on the mode's true centre.
        for _ in range(3):
            member = np.linalg.norm(colors[idx] - center, axis=1) <= radius
            if not member.any():
                break
            center = np.average(colors[idx][member], axis=0, weights=weight[idx][member])
        member = np.linalg.norm(colors[idx] - center, axis=1) <= radius
        frac = float(weight[idx][member].sum()) / total
        if frac < satellite_frac:
            break  # strongest remaining mode is a speck -> the rest is residue
        if frac >= min_mode_frac:
            primaries += 1
        elif accepted_centers and min(
            float(np.linalg.norm(center - c)) for c in accepted_centers
        ) <= satellite_dist:
            satellites += 1
        else:
            # Tight but weak and far from every bar ink: noise. Carve it out
            # of `remaining` so the next-densest mode can surface.
            remaining[idx[member]] = False
            continue
        accepted_centers.append(center)
        keep[idx[member]] = True
        remaining[idx[member]] = False

    if primaries == 0:
        return raws, "skipped: no qualifying colour mode"
    survivors = [r for r, k in zip(raws, keep) if k]
    dropped = len(raws) - len(survivors)
    return survivors, (
        f"kept {primaries} mode(s) + {satellites} satellite(s), "
        f"dropped {dropped}/{len(raws)} notes"
    )


def assign_hands(raws: list[_RawNote]) -> list[ExtractedNote]:
    """Cluster bar colours into up to two hands and map them to Left/Right.

    The lower-average-pitch cluster is Left; the other is Right.  A single colour
    cluster (or no clear split) falls back to ``Unknown`` with reduced confidence.
    """
    if not raws:
        return []
    colors = np.array([r.color for r in raws], dtype=np.float64)
    # Chromaticity (direction) to be robust to brightness differences.
    norm = np.linalg.norm(colors, axis=1, keepdims=True)
    chroma = colors / np.maximum(norm, 1e-6)

    centers, labels, spread = _kmeans2(chroma)
    hand_for_label: dict[int, Hand] = {}
    ambiguous = spread < 0.15  # two centres too close -> single colour
    if not ambiguous and centers is not None:
        # Average pitch per cluster -> lower pitch is the left hand.
        pitch0 = np.mean([raws[i].pitch for i in range(len(raws)) if labels[i] == 0] or [0])
        pitch1 = np.mean([raws[i].pitch for i in range(len(raws)) if labels[i] == 1] or [0])
        if pitch0 <= pitch1:
            hand_for_label = {0: Hand.LEFT, 1: Hand.RIGHT}
        else:
            hand_for_label = {0: Hand.RIGHT, 1: Hand.LEFT}

    out: list[ExtractedNote] = []
    for i, r in enumerate(raws):
        if ambiguous:
            hand = Hand.UNKNOWN
            conf = r.coverage * 0.6
        else:
            hand = hand_for_label[int(labels[i])]
            # Confidence drops near the colour boundary between clusters.
            d_own = np.linalg.norm(chroma[i] - centers[int(labels[i])])
            d_other = np.linalg.norm(chroma[i] - centers[1 - int(labels[i])])
            purity = d_other / max(d_own + d_other, 1e-6)
            conf = r.coverage * float(np.clip(purity, 0.0, 1.0))
        out.append(
            ExtractedNote(
                pitch=r.pitch,
                start_us=r.start_us,
                dur_us=max(1, r.dur_us),
                hand=hand,
                velocity=None,
                confidence=round(float(np.clip(conf, 0.0, 1.0)), 3),
            )
        )
    out.sort(key=lambda n: (n.start_us, n.pitch))
    return out


def _kmeans2(points: np.ndarray, iters: int = 20, max_seed_sample: int = 512):
    """Tiny 2-means. Returns (centers[2x3], labels, center_spread)."""
    if len(points) == 1:
        return None, np.zeros(1, dtype=int), 0.0
    # Seed with the two most distant points of a bounded sample: the pairwise
    # tensor is O(n^2 * 3 * 8) bytes, which on a real video (tens of thousands
    # of raw segments) would be gigabytes — a sample finds the same two colour
    # poles at negligible cost.
    if len(points) > max_seed_sample:
        rng = np.random.default_rng(0)
        sample = points[rng.choice(len(points), max_seed_sample, replace=False)]
    else:
        sample = points
    d = np.linalg.norm(sample[:, None, :] - sample[None, :, :], axis=2)
    i, j = np.unravel_index(int(np.argmax(d)), d.shape)
    centers = np.array([sample[i], sample[j]], dtype=np.float64)
    labels = np.zeros(len(points), dtype=int)
    for _ in range(iters):
        dist = np.linalg.norm(points[:, None, :] - centers[None, :, :], axis=2)
        new_labels = dist.argmin(axis=1)
        if np.array_equal(new_labels, labels) and _ > 0:
            labels = new_labels
            break
        labels = new_labels
        for c in (0, 1):
            if np.any(labels == c):
                centers[c] = points[labels == c].mean(axis=0)
    spread = float(np.linalg.norm(centers[0] - centers[1]))
    return centers, labels, spread


# --------------------------------------------------------------------------- #
# Orchestration
# --------------------------------------------------------------------------- #
def extract_chart(
    frames: Sequence[np.ndarray],
    fps: float,
    *,
    title: Optional[str] = None,
    anchor_c4_x: Optional[int] = None,
    scroll_override: Optional[float] = None,
    debug_dir: Optional[str] = None,
) -> ExtractedChart:
    """Run the full pipeline over ``frames`` and return an ``ExtractedChart``."""
    frames = list(frames)
    source = SourceMeta(extractor_version=EXTRACTOR_VERSION, title=title, fps=float(fps))
    if not frames:
        return ExtractedChart(notes=[], source=source)

    # The global background plate is the static scene: bars (and hands, in
    # filmed overlays) average out.  Calibrate on it — it is a cleaner keyboard
    # image than any single frame.  Masking, however, uses *windowed* plates
    # (a BackgroundModel) so scene changes in the artwork don't flood the mask.
    plate = background_plate(frames)
    ref = plate if plate is not None else frames[len(frames) // 2]
    hit_line = detect_hit_line(ref)
    if hit_line <= 1 or hit_line >= ref.shape[0]:
        return ExtractedChart(notes=[], source=source)
    kb = calibrate_keyboard(ref, hit_line, anchor_c4_x=anchor_c4_x)

    bg = background_model(frames, fps) or plate

    # A falling bar is at most a couple of lanes wide; anything wider in the
    # mask is bright animated artwork.  3 white-key widths is a safe cap.
    max_run_w: Optional[int] = None
    if len(kb.white_centers) >= 2:
        spacing = float(np.median(np.diff(sorted(kb.white_centers))))
        max_run_w = max(8, int(round(3 * spacing)))

    scroll = scroll_override or estimate_scroll(
        frames, hit_line, fps, plate=bg, max_run_w=max_run_w
    )
    if not scroll or scroll <= 0:
        # No motion detected (e.g. empty clip) -> no notes, but valid chart.
        source.scroll_px_per_s = None
        return ExtractedChart(notes=[], source=source)
    source.scroll_px_per_s = float(scroll)

    votes, totals, color_sum, pitches, us_per_px = build_roll(
        frames, fps, kb, scroll, plate=bg, max_run_w=max_run_w
    )
    # 40 ms is shorter than any playable tutorial note; in pixels it scales
    # with the scroll speed.
    raws = extract_notes(
        votes,
        totals,
        color_sum,
        pitches,
        us_per_px,
        threshold=0.6,
        hysteresis_low=0.42,
        min_run_px=max(3, int(round(0.04 * scroll))),
    )
    n_extracted = len(raws)
    raws, n_ghosts = suppress_neighbor_ghosts(raws)
    raws, color_diag = filter_color_modes(raws)
    source.noise_filter = (
        f"extracted {n_extracted}; ghosts dropped {n_ghosts}; colour modes: {color_diag}"
    )
    notes = assign_hands(raws)

    if debug_dir:
        _write_debug(debug_dir, ref, kb)

    return ExtractedChart(notes=notes, source=source)


def _write_debug(debug_dir: str, frame: np.ndarray, kb: Keyboard) -> None:
    """Annotate the keyboard calibration onto a frame for tuning. Gitignored."""
    import os

    os.makedirs(debug_dir, exist_ok=True)
    annotated = frame.copy()
    cv2.line(annotated, (0, kb.hit_line), (frame.shape[1], kb.hit_line), (0, 0, 255), 1)
    for cx, p in zip(kb.white_centers, kb.white_pitches):
        cv2.line(annotated, (cx, kb.hit_line), (cx, frame.shape[0]), (0, 255, 255), 1)
        cv2.putText(
            annotated, str(p), (cx - 8, frame.shape[0] - 4),
            cv2.FONT_HERSHEY_PLAIN, 0.6, (255, 0, 0), 1,
        )
    cv2.imwrite(os.path.join(debug_dir, "calibration.png"), annotated)
