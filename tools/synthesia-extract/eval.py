#!/usr/bin/env python3
"""Score an extracted chart against a reference note list (RockCraft M6).

The reference is any independent transcription of the *same* source — e.g. a
learned audio transcriber's output, or a hand-checked note list.  Neither side
is treated as perfect: the report buckets disagreements so the reader can tell
extraction bugs (notes the video shows but the chart lacks) from reference
noise (octave/harmonic artifacts of audio transcribers).

Usage:
    python eval.py --chart chart.json --ref ref.json [--tol-ms 150]

``ref.json`` is either an ``ExtractedChart``-shaped object (``{"notes": [...]}``)
or a bare list of ``{"pitch", "start_us", "dur_us", ...}`` objects.  Only ever
point this at *local* files; reference transcriptions of copyrighted media must
never be committed (see docs/IMPORT.md).
"""

from __future__ import annotations

import argparse
import collections
import json
import sys
from typing import Any, Optional

import numpy as np


def _load_notes(path: str) -> list[dict[str, Any]]:
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    notes = data["notes"] if isinstance(data, dict) else data
    return sorted(notes, key=lambda n: (n["start_us"], n["pitch"]))


def _index(notes: list[dict[str, Any]]) -> dict[int, list[int]]:
    by_pitch: dict[int, list[int]] = collections.defaultdict(list)
    for n in notes:
        by_pitch[n["pitch"]].append(n["start_us"])
    for starts in by_pitch.values():
        starts.sort()
    return by_pitch


def _nearest(by_pitch: dict[int, list[int]], pitch: int, t: int) -> Optional[int]:
    """Signed distance to the nearest onset of ``pitch``, or None."""
    starts = by_pitch.get(pitch)
    if not starts:
        return None
    i = int(np.searchsorted(starts, t))
    cands = [starts[j] - t for j in (i - 1, i) if 0 <= j < len(starts)]
    return min(cands, key=abs) if cands else None


def estimate_offset(
    chart: list[dict[str, Any]],
    ref: list[dict[str, Any]],
    *,
    search_us: int = 250_000,
    min_matches: int = 30,
) -> int:
    """Median ref-minus-chart onset offset over same-pitch nearest matches."""
    by_pitch = _index(chart)
    deltas = []
    for r in ref:
        d = _nearest(by_pitch, r["pitch"], r["start_us"])
        if d is not None and abs(-d) <= search_us:
            deltas.append(-d)  # chart onset relative to ref
    if len(deltas) < min_matches:
        return 0
    return int(np.median(deltas))


def evaluate(
    chart: list[dict[str, Any]],
    ref: list[dict[str, Any]],
    *,
    tol_us: int = 150_000,
) -> dict[str, Any]:
    """Bucketized comparison; offsets are compensated before matching."""
    offset_us = estimate_offset(chart, ref)
    chart_idx = _index(chart)
    ref_idx = _index(ref)

    buckets: collections.Counter = collections.Counter()
    dts = []
    for r in ref:
        t = r["start_us"] + offset_us
        d = _nearest(chart_idx, r["pitch"], t)
        if d is not None and abs(d) <= tol_us:
            buckets["exact"] += 1
            dts.append(d)
            continue
        if any(
            (d2 := _nearest(chart_idx, r["pitch"] + dp, t)) is not None
            and abs(d2) <= tol_us
            for dp in (-12, 12)
        ):
            buckets["octave"] += 1
        elif any(
            (d2 := _nearest(chart_idx, r["pitch"] + dp, t)) is not None
            and abs(d2) <= tol_us
            for dp in (-1, 1)
        ):
            buckets["semitone"] += 1
        else:
            buckets["missing"] += 1

    # Reference onsets buried inside a longer chart note's body: merged repeats.
    chart_spans: dict[int, list[tuple[int, int]]] = collections.defaultdict(list)
    for n in chart:
        chart_spans[n["pitch"]].append((n["start_us"], n["start_us"] + n["dur_us"]))
    buried = 0
    for r in ref:
        t = r["start_us"] + offset_us
        if any(s + tol_us < t < e - 20_000 for s, e in chart_spans.get(r["pitch"], ())):
            buried += 1

    spurious = sum(
        1
        for n in chart
        if not any(
            (d := _nearest(ref_idx, n["pitch"] + dp, n["start_us"] - offset_us))
            is not None
            and abs(d) <= tol_us
            for dp in (0, -12, 12, -1, 1)
        )
    )

    n_ref = max(1, len(ref))
    return {
        "chart_notes": len(chart),
        "ref_notes": len(ref),
        "offset_ms": round(offset_us / 1000),
        "exact": buckets["exact"],
        "exact_pct": round(100.0 * buckets["exact"] / n_ref, 1),
        "octave": buckets["octave"],
        "semitone": buckets["semitone"],
        "missing": buckets["missing"],
        "buried_onsets": buried,
        "chart_only": spurious,
        "exact_dt_ms_median": round(float(np.median(dts)) / 1000) if dts else None,
    }


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chart", required=True, help="extracted chart JSON")
    parser.add_argument("--ref", required=True, help="reference notes JSON")
    parser.add_argument("--tol-ms", type=int, default=150, help="match tolerance")
    args = parser.parse_args(argv)

    report = evaluate(
        _load_notes(args.chart), _load_notes(args.ref), tol_us=args.tol_ms * 1000
    )
    for k, v in report.items():
        print(f"{k}: {v}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
