#!/usr/bin/env python3
"""Scan or PDF → MusicXML — standalone OMR CLI (RockCraft M13-B).

Drives an external optical music recognition engine over a scanned or
photographed page and writes the MusicXML it produces. This is the *first* half
of a scan import, exposed on its own for debugging: `convert.py` runs the same
stage internally, so nothing needs to call this in normal use.

Reach for it when a scan import produces a wrong chart and you want to see
whether the engine or the conversion is at fault — the MusicXML this writes is
the exact input `convert.py` would have converted.

Usage:
    python omr.py --in <scan.pdf|png|jpg> --out <score.musicxml>

Engine resolution, in order: `$ROCKCRAFT_OMR_CMD`, then `audiveris` on PATH,
then `oemer`. With none installed this exits 2 with a message naming all three;
it never falls back silently and never downloads anything.

**Engine output goes to stderr**, keeping this tool's contract the same as
`convert.py`'s.
"""

from __future__ import annotations

import argparse
import sys

from score_import.omr import SCAN_EXTENSIONS, OmrError, is_scan, transcribe


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--in", dest="inp", required=True,
        help="scanned page or PDF (" + ", ".join(sorted(SCAN_EXTENSIONS)) + ")",
    )
    parser.add_argument("--out", dest="out", required=True, help="output MusicXML path")
    args = parser.parse_args(argv)

    if not is_scan(args.inp):
        print(
            f"error: {args.inp} is not a scan; OMR handles "
            + ", ".join(sorted(SCAN_EXTENSIONS))
            + ". A score file needs no OMR — convert it directly with convert.py.",
            file=sys.stderr,
        )
        return 2

    try:
        engine = transcribe(args.inp, args.out)
    except OmrError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    print(f"transcribed {args.inp} -> {args.out} via {engine.label()}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
