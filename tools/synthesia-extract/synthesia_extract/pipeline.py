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
5. :func:`extract_notes` / :func:`assign_hands` — threshold coverage into notes,
   map dominant colours to Left/Right.
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
    grey background, which are all near-zero saturation."""
    hsv = cv2.cvtColor(frame, cv2.COLOR_BGR2HSV)
    sat = hsv[:, :, 1]
    val = hsv[:, :, 2]
    return (sat > 60) & (val > 60)


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
    """Derive the per-column pitch ruler from the drawn keyboard."""
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
def estimate_scroll(
    frames: Sequence[np.ndarray], hit_line: int, fps: float, gap: int = 4
) -> Optional[float]:
    """Estimate scroll speed (px/s) by cross-correlating the falling-region bar
    mask across frame pairs ``gap`` apart and reading off the vertical shift."""
    if len(frames) <= gap or hit_line <= 1:
        return None
    max_shift = max(1, hit_line // 2)
    shifts: list[float] = []
    n = len(frames)
    # Sample several pairs spread through the clip.
    for i in range(0, n - gap, max(1, (n - gap) // 12 + 1)):
        m0 = colored_mask(frames[i])[:hit_line].astype(np.float32)
        m1 = colored_mask(frames[i + gap])[:hit_line].astype(np.float32)
        if m0.sum() < 20 or m1.sum() < 20:
            continue
        best_d, best_score = 0, -1.0
        for d in range(1, max_shift + 1):
            score = float((m0[: hit_line - d] * m1[d:]).sum())
            if score > best_score:
                best_score, best_d = score, d
        if best_d > 0 and best_score > 0:
            shifts.append(best_d / (gap / fps))
    if not shifts:
        return None
    return float(np.median(shifts))


# --------------------------------------------------------------------------- #
# 4. Roll stitching
# --------------------------------------------------------------------------- #
def build_roll(
    frames: Sequence[np.ndarray], fps: float, kb: Keyboard, scroll: float
) -> tuple[np.ndarray, np.ndarray, np.ndarray, list[int], float]:
    """Accumulate, per pitch, song-time coverage and mean colour.

    Returns ``(votes, totals, color_sum, pitches, us_per_px)`` where ``votes`` and
    ``color_sum`` are indexed ``[pitch_index, bin]`` and ``totals[bin]`` is the
    geometric number of frames that could observe each song-time bin.
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
    totals = np.zeros(max_bin, dtype=np.float32)
    color_sum = np.zeros((len(pitches), max_bin, 3), dtype=np.float64)

    rows = np.arange(hit)  # falling-region rows
    for f in range(n):
        o_f = f / fps * scroll
        row_bins = np.round(o_f + hit - rows).astype(np.int64)  # one bin per row
        valid_rows = (row_bins >= 0) & (row_bins < max_bin)
        # Geometric coverage: every falling row is observed once this frame.
        np.add.at(totals, row_bins[valid_rows], 1.0)

        frame = frames[f]
        mask = colored_mask(frame)[:hit]  # hit x width
        for pidx, x0, x1, cx in lanes:
            strip = mask[:, x0:x1].any(axis=1)  # colored at this lane, per row
            on = strip & valid_rows
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
    min_run_px: int = 3,
) -> list[_RawNote]:
    """Threshold per-pitch coverage into note runs."""
    safe_tot = np.maximum(totals, 1e-6)
    raws: list[_RawNote] = []
    for pi, pitch in enumerate(pitches):
        coverage = votes[pi] / safe_tot
        on = (coverage > threshold) & (totals > 0)
        for a, b in _find_runs(on):
            if b - a < min_run_px:
                continue
            seg_votes = votes[pi, a:b]
            total_votes = float(seg_votes.sum())
            if total_votes <= 0:
                continue
            color = color_sum[pi, a:b].sum(axis=0) / total_votes
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


def _kmeans2(points: np.ndarray, iters: int = 20):
    """Tiny 2-means. Returns (centers[2x3], labels, center_spread)."""
    if len(points) == 1:
        return None, np.zeros(1, dtype=int), 0.0
    # Seed with the two most distant points.
    d = np.linalg.norm(points[:, None, :] - points[None, :, :], axis=2)
    i, j = np.unravel_index(int(np.argmax(d)), d.shape)
    centers = np.array([points[i], points[j]], dtype=np.float64)
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

    # Calibrate on a frame that shows the keyboard (any frame; it is static).
    ref = frames[len(frames) // 2]
    hit_line = detect_hit_line(ref)
    if hit_line <= 1 or hit_line >= ref.shape[0]:
        return ExtractedChart(notes=[], source=source)
    kb = calibrate_keyboard(ref, hit_line, anchor_c4_x=anchor_c4_x)

    scroll = scroll_override or estimate_scroll(frames, hit_line, fps)
    if not scroll or scroll <= 0:
        # No motion detected (e.g. empty clip) -> no notes, but valid chart.
        source.scroll_px_per_s = None
        return ExtractedChart(notes=[], source=source)
    source.scroll_px_per_s = float(scroll)

    votes, totals, color_sum, pitches, us_per_px = build_roll(frames, fps, kb, scroll)
    raws = extract_notes(votes, totals, color_sum, pitches, us_per_px)
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
