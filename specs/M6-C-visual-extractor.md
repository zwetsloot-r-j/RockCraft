# M6-C — sidecar: visual note extractor (Synthesia roll → notes JSON)

> Milestone: M6 — Video Import · Issue: #115 · Suggested tier: opus
> Branch: `claude/m6-visual-extractor`

## Goal

A Python + OpenCV sidecar that turns a **local** Synthesia-style tutorial video
into the M6-A notes JSON. Vision-first; phase 1 recovers **pitch + onset +
duration + hand** (velocity comes later in M6-F).

## Context

- Lives in `tools/synthesia-extract/` as a standalone Python project (its own
  `requirements.txt` / venv) — **not** a workspace crate, invoked as a
  subprocess by M6-D (#116). Keeps OpenCV/heavy ML out of the Rust build.
- Emits the schema defined in M6-A (#113): `ExtractedChart` JSON on stdout (or a
  file path argument). Confidence per note feeds the review step (M6-E).
- Input is a local file path only — **no downloading here** (that's M6-D's
  pluggable hook). No audio in this task (M6-F).
- The falling-bar strip is a piano roll: bar x → pitch (calibrated against the
  on-screen keyboard), bar length + scroll speed → duration, color → hand. Every
  note is drawn in full before it is played, so frames can be stitched into the
  whole roll rather than raced in real time.

## What to do

A CLI: `python extract.py --in <video> --out <chart.json> [--debug]`.

Pipeline:
1. **Keyboard calibration.** Locate the keyboard region; derive per-key x
   boundaries from the white/black layout (the keyboard is the pitch ruler).
   Establish the octave anchoring (where C4/middle C sits) heuristically and
   allow an override.
2. **Hit-line + scroll speed.** Find the line where bars meet the keyboard;
   estimate scroll speed (px/s) by tracking a bar's motion across frames.
3. **Bar tracking.** Segment colored bars in the falling region; group by key
   column; recover each bar's top/bottom over time. Convert pixel-y → song time
   with the scroll speed, using **sub-frame interpolation** (a bar's offset from
   the hit-line at the crossing frame) so onset precision beats the frame grid.
4. **Hand mapping.** Map the (typically two) dominant bar colors → Left/Right;
   fall back to `Unknown`.
5. **Emit** M6-A `ExtractedChart` with `SourceMeta` (fps, scroll_px_per_s,
   extractor_version) and per-note `confidence`. Flag low-confidence notes
   (ambiguous color, overlapping bars, calibration uncertainty).
6. `--debug` writes annotated frames to a **gitignored** dir for tuning.

## Tests (synthetic only — never a real video)

- A test **generates a synthetic Synthesia-like clip**: render colored bars
  scrolling at a known speed over a drawn keyboard for a known note sequence
  (e.g. a C-major scale + a couple of chords, two hands/colors). Run the
  extractor and assert:
  - pitches recovered **exactly**;
  - onsets/durations within frame-time tolerance;
  - hand colors mapped correctly.
- A degenerate case (single note, empty video) does not crash and yields the
  expected (possibly empty) chart.
- Keep synthetic-asset generation in the test (or commit only small synthetic
  PNGs/clips) — **no copyrighted material in the repo** (M6-B).

## Scope boundaries (do NOT)

- No downloading / network; no audio transcription (M6-F).
- Do not commit real videos, frames, or extracted songs — synthetic only.
- Do not reimplement the bundle writer — just emit M6-A JSON; M6-D writes the
  bundle.

## Acceptance

- [ ] Synthetic-clip extraction matches ground truth within tolerance
- [ ] Emits schema-valid M6-A JSON; `requirements.txt` + a short README for setup
- [ ] `--debug` output lands only under a gitignored path
- [ ] PR against `main` from `claude/m6-visual-extractor`, `Closes #115`
