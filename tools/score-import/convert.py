#!/usr/bin/env python3
"""Score file or scan → chart JSON — CLI (RockCraft M13-A / M13-B).

Turns a local score file (MusicXML and the other formats ``music21`` reads) into
the M6-A ``ExtractedChart`` JSON the Rust import pipeline consumes. A local file
path in, JSON out: no downloading, no network.

A **scan or PDF** (`.pdf`, `.png`, `.jpg`, `.jpeg`, `.tif`, `.tiff`, `.bmp`) is
routed through the optical music recognition stage (``omr.py``) first and then
through the same conversion, so the Rust side sees exactly one sidecar contract
regardless of which kind of input it was given. OMR output carries a derived
per-note ``confidence`` and is meant to be reviewed — see the summary line on
stderr.

Usage:
    python convert.py --in <score-file|scan> --out <chart.json>|- [--title T]
                      [--hand-map 0=right,1=left]

**stdout carries only the JSON.** Warnings, dropped-element counts, engine
output and the summary all go to stderr, because ``crates/import``'s
``run_sidecar`` parses stdout.
"""

from __future__ import annotations

import argparse
import sys

from score_import.convert import convert_input, parse_hand_map
from score_import.omr import SCAN_EXTENSIONS, OmrError


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--in", dest="inp", required=True,
        help="score file (.musicxml, .xml, .mxl, .abc, .krn) or scan ("
        + ", ".join(sorted(SCAN_EXTENSIONS))
        + ")",
    )
    parser.add_argument("--out", dest="out", default="-", help="output JSON path ('-' = stdout)")
    parser.add_argument("--title", default=None, help="override the score's own title")
    parser.add_argument(
        "--hand-map", dest="hand_map", default=None,
        help="explicit part-index→hand assignment, e.g. '0=right,1=left'; "
        "overrides the staff heuristic for scores it cannot split",
    )
    args = parser.parse_args(argv)

    try:
        hand_map = parse_hand_map(args.hand_map)
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    try:
        chart, report = convert_input(args.inp, hand_map=hand_map, title=args.title)
    except FileNotFoundError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except OmrError as e:
        # A missing engine or an engine failure: already actionable, and printed
        # as-is because the message names every way to fix it.
        print(f"error: {e}", file=sys.stderr)
        return 2
    except Exception as e:  # music21 raises a wide family for unreadable scores
        print(f"error: could not convert {args.inp}: {type(e).__name__}: {e}", file=sys.stderr)
        return 2

    for line in report.lines():
        print(line, file=sys.stderr)

    json_str = chart.to_json(indent=2)
    if args.out == "-":
        print(json_str)
    else:
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write(json_str)
    print(
        f"converted {len(chart.notes)} notes -> {args.out}"
        f" (bpm={chart.notation.bpm if chart.notation else None})",
        file=sys.stderr,
    )
    # Last line, so it is the freshest thing in the pipeline's log stream: the
    # frontends lift this one out by its `omr: ` prefix and put it on the import
    # status line. Only the OMR path has a confidence story to tell.
    if report.confidence is not None:
        print(report.confidence.summary_line(), file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
