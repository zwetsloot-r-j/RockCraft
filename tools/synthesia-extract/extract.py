#!/usr/bin/env python3
"""Synthesia visual note extractor — CLI (RockCraft M6-C).

Turns a local Synthesia-style tutorial video into the M6-A ``ExtractedChart``
JSON.  No downloading, no audio, no network — a local file path in, JSON out.

Usage:
    python extract.py --in <video|frames_dir> --out <chart.json> [--debug]
                      [--fps F] [--anchor-c4-x X] [--scroll PX_PER_S]
                      [--audio-fusion --audio <track.wav>]

When ``--in`` is a directory of numbered frames, ``--fps`` is required.
``--debug`` writes annotated frames under ``./debug-out/`` (gitignored).

``--audio-fusion`` runs the optional M6-F pass: when ``--audio`` points at a
clean-piano WAV, it fills in velocity and tightens onsets; on a full mix it
safely no-ops, leaving the visual notes untouched (see ``synthesia_extract.audio``).
"""

from __future__ import annotations

import argparse
import sys

from synthesia_extract.audio import fuse_chart
from synthesia_extract.io import load_frames, read_wav
from synthesia_extract.pipeline import extract_chart

DEBUG_DIR = "debug-out"  # gitignored; see .gitignore


def _limit_memory() -> None:
    """Cap this process's address space below what the system can spare.

    A runaway allocation then raises ``MemoryError`` (a clean traceback and a
    non-zero exit the import pipeline reports as ``SidecarFailed``) instead of
    invoking the kernel OOM killer — which on WSL can take the whole VM down
    with it.  Best-effort: silently does nothing where unsupported.
    """
    try:
        import resource

        available_kb = None
        with open("/proc/meminfo", encoding="ascii") as fh:
            for line in fh:
                if line.startswith("MemAvailable:"):
                    available_kb = int(line.split()[1])
                    break
        if available_kb is None:
            return
        # Leave headroom for the rest of the system; never go below 2 GiB
        # (the loader's own decimation budget; see io.load_frames).
        cap = max(2 * 1024**3, available_kb * 1024 - 1200 * 2**20)
        resource.setrlimit(resource.RLIMIT_AS, (cap, cap))
    except (ImportError, OSError, ValueError):
        return


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--in", dest="inp", required=True, help="video file or frames directory")
    parser.add_argument("--out", dest="out", default="-", help="output JSON path ('-' = stdout)")
    parser.add_argument("--fps", type=float, default=None, help="fps (required for a frames dir)")
    parser.add_argument("--title", default=None, help="optional source title")
    parser.add_argument(
        "--anchor-c4-x", type=int, default=None,
        help="x pixel of middle C (overrides the heuristic octave anchor)",
    )
    parser.add_argument(
        "--scroll", type=float, default=None,
        help="scroll speed px/s (overrides estimation)",
    )
    parser.add_argument("--debug", action="store_true", help=f"write annotated frames to ./{DEBUG_DIR}/")
    parser.add_argument(
        "--audio-fusion", dest="audio_fusion", action="store_true",
        help="run the optional M6-F audio pass (velocity + onset refinement)",
    )
    parser.add_argument(
        "--audio", dest="audio", default=None,
        help="clean-piano WAV track for --audio-fusion (16-bit PCM)",
    )
    args = parser.parse_args(argv)

    _limit_memory()

    try:
        frames, fps = load_frames(args.inp, fps_override=args.fps)
    except (FileNotFoundError, ValueError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except MemoryError:
        print(
            "error: out of memory while loading frames — close other "
            "applications or lower the source resolution",
            file=sys.stderr,
        )
        return 3

    try:
        chart = extract_chart(
            frames,
            fps,
            title=args.title,
            anchor_c4_x=args.anchor_c4_x,
            scroll_override=args.scroll,
            debug_dir=DEBUG_DIR if args.debug else None,
        )
    except MemoryError:
        print(
            "error: out of memory during extraction — this is a bug guardrail, "
            "not a crash; please report the video's resolution and length",
            file=sys.stderr,
        )
        return 3

    if args.audio_fusion:
        if not args.audio:
            print(
                "error: --audio-fusion requires --audio <track.wav>",
                file=sys.stderr,
            )
            return 2
        try:
            samples, sample_rate = read_wav(args.audio)
        except (FileNotFoundError, ValueError, OSError) as e:
            print(f"error: could not read audio: {e}", file=sys.stderr)
            return 2
        chart = fuse_chart(chart, samples, sample_rate)
        print(f"audio-fusion: {chart.source.audio_fusion}", file=sys.stderr)

    json_str = chart.to_json(indent=2)
    if args.out == "-":
        print(json_str)
    else:
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write(json_str)
        print(
            f"wrote {len(chart.notes)} notes -> {args.out}"
            f" (scroll={chart.source.scroll_px_per_s} px/s, fps={chart.source.fps})",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
