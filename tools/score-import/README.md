# score-import — digital score file → chart bundle (M13-A)

A Python **sidecar** that turns a *local* digital score file (MusicXML and the
other formats [`music21`](https://www.music21.org/) reads) into the M6-A
`ExtractedChart` JSON consumed by the Rust `rockcraft-import` crate.

Unlike the M6-C video extractor, this is **not** an inference problem. Pitch,
duration, tempo, metre, key and staff→hand are all stated explicitly by the
source, so the conversion is a **deterministic transform**: no computer vision,
no ML, no LLM, no heuristics beyond the documented staff→hand rule. Its tests
assert onsets and durations exact to the microsecond.

This is **not** a workspace crate. It runs as a standalone process, invoked by
the import pipeline as a subprocess (`crates/import/src/pipeline.rs`).

## Setup

```bash
cd tools/score-import
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
```

`music21` is the only runtime dependency. It is an **optional** system
dependency of RockCraft as a whole: the Rust build never links it, and every
other import path works without it. Without the venv, a score import fails with
an actionable `SidecarMissing` error naming this file.

## Usage

```bash
python convert.py --in <score-file> --out <chart.json>|-   # '-' = stdout
                  [--title "Name"] [--hand-map 0=right,1=left]
```

**stdout carries only the JSON.** Warnings, dropped-element counts and the
summary line all go to stderr, because the pipeline's `run_sidecar` parses
stdout.

From the app: the TUI's *"Import score file…"* menu entry, or the
`import_score` host command over the agent-control socket
(`{"command":"import_score","path":"…"}`).

## Supported formats

The documented and tested set is **`.musicxml`, `.xml`, `.mxl`, `.abc`,
`.krn`**. `music21` reads more than that, and `convert.py` will happily try
anything it accepts — but only these are covered by the pipeline's file filters
and this project's tests.

Scanned PDFs and images are **not** handled here; optical music recognition is a
separate follow-up (issue #246).

## Conversion rules

Each rule has a test in `tests/test_convert.py`.

1. **Parse** with `music21.converter.parse`.
2. **Repeats are expanded** into a linear score — repeats, voltas, D.C., D.S.
   and coda jumps all unroll, so a repeated bar yields twice the notes.
   Malformed repeat structures are common in real files: if expansion fails, the
   score is imported *as written* (repeated sections appear once) with a warning
   on stderr, rather than failing the import.
3. **Ties are merged**: a tied pair becomes one note of the summed duration, not
   two. Adjacent *untied* notes of the same pitch stay separate.
4. **Chords** become one note per pitch, sharing start and duration.
5. **Timing** is integrated over a tempo map built from the score's metronome
   marks, so a tempo change moves every later note correctly:
   `start_us = round(seconds × 1e6)`, `dur_us = max(1, …)`.
6. **Hand** comes from the staff/part: upper staff (or first part) → `Right`,
   lower staff (or second part) → `Left`. A single staff, or 3+ parts, →
   `Unknown` — one staff simply carries no hand information, and guessing would
   be worse than saying so. `--hand-map` overrides both.
7. **Velocity** is `null` unless the source states one explicitly, letting the
   Rust parser's `DEFAULT_VELOCITY` apply. Dynamics markings are *not* turned
   into velocities: a synthesized number would be indistinguishable from a real
   one downstream.
8. **Dropped by design** — counted on stderr, never silently swallowed: grace
   notes, ornament realizations (the principal note is kept), unpitched and
   percussion notes, pedal marks, articulations, lyrics.
9. **`confidence: 1.0`** on every note. This transform is exact.
10. **`notation`** carries the tempo, time signature and key as *notated*, which
    is what gives an imported piece a grid its bars actually snap to.

## Known limitations

- **One tempo per piece.** Note times are absolute microseconds and stay correct
  across a tempo change, but `core::Grid` holds a single BPM, so the editor's bar
  lines drift after the first change. A score with more than one tempo is
  imported with a loud warning; adding a tempo map to `Grid` is a separate task.
- **No tempo mark** → 120 BPM is assumed, warned about, and written into
  `notation.bpm` so the grid at least matches the note times.
- **A key signature with no mode is omitted, not guessed.** The same accidentals
  fit a major key and its relative minor. Modes beyond major and natural minor
  are omitted for the same reason — `core::Key` models only those two.
- **Hand is dropped downstream.** The chart carries it, but
  `chart_to_timeline` does not yet thread it into `core`'s `Timeline`.

## Test fixtures

The fixtures live in `fixtures/score/` at the repository root, reusing the
existing `fixtures/` carve-out in the content-policy guard
(`scripts/check-no-media.sh`). They are hand-written and obviously synthetic — a
C-major scale, a fabricated two-hand chord — never a real piece.

**Never commit a real score.** `.pdf`, `.mxl`, `.mscz`, `.sib` and Guitar Pro
files are rejected anywhere in the tree by the guard; `.mxl` in particular is a
zip container and therefore unreviewable in a diff, so plain `.musicxml` is the
only committable score format. See `docs/IMPORT.md`.

```bash
python3 -m pytest        # from tools/score-import/
```
