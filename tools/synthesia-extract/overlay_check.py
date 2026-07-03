#!/usr/bin/env python3
"""Overlay an extracted chart back onto its source video — a visual sanity check.

Draws every note in an ``ExtractedChart`` JSON (the output of :mod:`extract.py`)
as a rectangle on the real video frames, positioned by pitch (keyboard
calibration) and time (the chart's own ``scroll_px_per_s``).  A correct note
hugs its falling bar; a *missing* note shows as a bar with no rectangle, a
*wrong* one as a rectangle over empty lane, and a *too-short* one as a rectangle
stubbier than its bar.  This is how you eyeball extractor accuracy without
replaying the whole clip.

No network, no download — a local video + local chart JSON in, annotated PNGs
out.  Nothing here is committed with the repo except this script; the video and
chart stay local (see ``.gitignore``).

Usage:
    # a few individual frames (seconds):
    python overlay_check.py --video src.mp4 --chart chart.json --times 12 90 200
    # a whole-song contact sheet (every --step seconds, tiled into sheets):
    python overlay_check.py --video src.mp4 --chart chart.json --contact --step 4

Outputs land in ``--out-dir`` (default ``overlay-out/``, gitignored).
"""

from __future__ import annotations

import argparse
import json
import os

import cv2
import numpy as np

from synthesia_extract import io as sio
from synthesia_extract.pipeline import (
    background_plate,
    calibrate_keyboard,
    detect_hit_line,
)


def _load_chart(path):
    """Return (notes, scroll_px_per_s, hit_line_px|None) from an ExtractedChart JSON."""
    data = json.loads(open(path).read())
    notes = [(int(n["pitch"]), int(n["start_us"]), int(max(1, n["dur_us"]))) for n in data["notes"]]
    src = data.get("source", {})
    return notes, src.get("scroll_px_per_s"), src.get("hit_line_px")


def _calibrate(video, hit_hint):
    sample, _ = sio.sparse_sample(video, 200)
    if not sample:
        raise SystemExit("could not read frames from video")
    ref = background_plate(sample)
    ref = ref if ref is not None else sample[len(sample) // 2]
    hit = int(hit_hint) if hit_hint else detect_hit_line(ref)
    kb = calibrate_keyboard(ref, hit)
    centers = {p: x for (x, p) in kb.centers}
    white_w = int(np.median(np.diff(sorted(kb.white_centers)))) if len(kb.white_centers) > 1 else 8
    return hit, centers, white_w


def _annotate(video, t_s, notes, hit, centers, white_w, scroll, fps):
    fr = sio.read_burst(video, int(round(t_s * fps)), 1)
    if not fr:
        return None
    img = cv2.convertScaleAbs(fr[0], alpha=1.25, beta=13)
    h, w = img.shape[:2]
    t_us = t_s * 1e6
    k = scroll / 1e6  # px per us
    cv2.line(img, (0, hit), (w, hit), (0, 0, 255), 1)
    bar_w = max(2, white_w // 2 - 1)
    for pitch, start, dur in notes:
        x = centers.get(pitch)
        if x is None:
            continue
        y_bot = int(hit - (start - t_us) * k)
        y_top = int(hit - (start + dur - t_us) * k)
        if y_bot < -15 or y_top > h + 15:
            continue
        colour = (40, 255, 255) if pitch >= 72 else (255, 180, 40)  # RH yellow, LH blue
        cv2.rectangle(img, (x - bar_w, y_top), (x + bar_w, y_bot), colour, 2)
    label = "%.0fs" % t_s
    cv2.putText(img, label, (6, 20), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (0, 0, 0), 4)
    cv2.putText(img, label, (6, 20), cv2.FONT_HERSHEY_SIMPLEX, 0.6, (255, 255, 255), 1)
    return img


def _contact_sheet(tiles, out_dir, cols=4, rows=6, tile=(480, 270)):
    tw, th = tile
    tiles = [cv2.resize(t, (tw, th)) for t in tiles if t is not None]
    per = cols * rows
    for si in range((len(tiles) + per - 1) // per):
        chunk = tiles[si * per : (si + 1) * per]
        nrows = (len(chunk) + cols - 1) // cols
        sheet = np.full((nrows * th, cols * tw, 3), 30, np.uint8)
        for i, t in enumerate(chunk):
            r, c = divmod(i, cols)
            sheet[r * th : (r + 1) * th, c * tw : (c + 1) * tw] = t
        path = os.path.join(out_dir, "sheet_%d.png" % si)
        cv2.imwrite(path, sheet)
        print("wrote", path)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--video", required=True, help="source video (mp4/...)")
    ap.add_argument("--chart", required=True, help="ExtractedChart JSON from extract.py")
    ap.add_argument("--times", type=float, nargs="*", help="seconds to render as full frames")
    ap.add_argument("--contact", action="store_true", help="render a whole-song contact sheet")
    ap.add_argument("--step", type=float, default=4.0, help="contact-sheet interval (s)")
    ap.add_argument("--fps", type=float, default=30.0, help="video fps for frame indexing")
    ap.add_argument("--scroll", type=float, help="override scroll px/s (else from chart JSON)")
    ap.add_argument("--out-dir", default="overlay-out")
    args = ap.parse_args()

    notes, scroll_meta, hit_hint = _load_chart(args.chart)
    scroll = args.scroll or scroll_meta
    if not scroll:
        raise SystemExit("chart JSON has no scroll_px_per_s; pass --scroll")
    hit, centers, white_w = _calibrate(args.video, hit_hint)
    os.makedirs(args.out_dir, exist_ok=True)
    print("notes %d  hit_line %d  scroll %.1f px/s" % (len(notes), hit, scroll))

    if args.contact:
        info = sio.video_info(args.video, args.fps)
        dur_s = (info[1] / info[0]) if info[1] and info[0] else 0
        times = list(np.arange(6, max(8, dur_s - 2), args.step))
        tiles = [_annotate(args.video, t, notes, hit, centers, white_w, scroll, args.fps) for t in times]
        _contact_sheet(tiles, args.out_dir)
    for t in args.times or []:
        img = _annotate(args.video, t, notes, hit, centers, white_w, scroll, args.fps)
        if img is not None:
            img = cv2.resize(img, (img.shape[1] * 2, img.shape[0] * 2), interpolation=cv2.INTER_NEAREST)
            path = os.path.join(args.out_dir, "frame_%06.1f.png" % t)
            cv2.imwrite(path, img)
            print("wrote", path)


if __name__ == "__main__":
    main()
