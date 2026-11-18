# M27-E — Rocksmith+ import: read the finger numbers off the video

> Milestone: M27 — Finger numbers · Issue: #374 · Suggested tier: opus
> Branch: `claude/m27-finger-rsplus` · Depends on: M27-D (#373)
> Labels: `needs-review` — and the first step needs a real capture, which never
> leaves the author's machine.

## Goal

Rocksmith+ piano captures draw a finger number on notes. When the Rocksmith+
extractor can read that number **clearly**, the imported note carries it;
when it cannot, the note gets none. A wrong number is worse than a missing
one, so the bar for "clearly" is high.

## Context

- Extractor: `tools/synthesia-extract/synthesia_extract/rsplus.py` (M6-G), then
  audio anchoring in `rsplus_audio.py` (M6-H). M6-H listed finger numbers as a
  possible follow-up; this is it.
- Today the extractor only knows the white overlay text as an **occlusion**:
  `_lane_state` marks a lane `WEAK` when unsaturated bright pixels
  (`_TEXT_S_MAX`, `_TEXT_V_MIN`) cover it near the hit line, so a "MISS" label
  cannot split a note. Nothing tells digits from "MISS".
- `extract_frames` keeps only a 1-px band per frame (`sample_band`, three rows
  just above `hit_top`). Reading a digit needs a taller patch.
- Dependencies are `numpy` and `opencv-python-headless` only. There is no OCR
  library, and none is to be added.
- M27-D added `ExtractedNote.finger` and carries it through `_moved` and the
  Rust writer.
- Tests are synthetic only: `rsplus_synth.py` renders fabricated frames
  (`cv2.putText` is already used for key labels).

## What to do

### E0 — look first (local, not committed)

How the game draws the number is not written down anywhere in this repo. Before
coding, inspect a real capture and record, in the "Known limitations / observed
layout" section of this spec (words and measurements only — no frames):

- where the digit sits (on the note bar, on the hit line, beside the key?),
  and whether it scrolls with the note or stays put;
- when it appears and for how long, relative to the note's onset;
- its size relative to the lane width, its colour, and whether it is tinted;
- whether every note gets one or only some (e.g. only the first of a run);
- how it interacts with the "MISS" label.

If the numbers turn out not to be readable at capture resolution, stop: write
that finding into this spec, comment on the issue, and close it without code.

### E1 — capture the digit patches (`rsplus.py`)

- During `extract_frames`, besides the 1-px band, keep a small grey patch per
  lane **only for frames where that lane shows overlay text** (the existing
  white-text test), sized from the geometry found in E0. Memory must stay
  bounded: patches are tiny and only kept while text is present.
- Do not change how notes, hands or `WEAK`/`STRONG` states are detected.

### E2 — classify

- A pure function `read_digit(patch) -> tuple[int | None, float]` returning a
  finger 1–5 (or `None`) and a score, using OpenCV only — normalise the patch
  (threshold, crop to the glyph's bounding box, scale to a fixed size) and
  compare against five templates.
- Templates are generated in code at import time from an OpenCV built-in font,
  or, if E0 shows the game's glyphs differ too much for that, from small
  hand-drawn binary arrays defined in the source. **No image files, and nothing
  derived from a real frame, is committed.**
- "MISS" and any other non-digit text must classify as `None`: reject patches
  whose glyph bounding box is much wider than one digit.

### E3 — assign to notes

- For each note, gather the patches of its lane within the window E0
  established around its onset, classify each, and vote.
- Assign a finger only when **all** hold; otherwise `None`:
  - at least `FINGER_MIN_FRAMES` (start at 3) patches classified as a digit;
  - at least `FINGER_MIN_AGREE` (start at 0.8) of them agree;
  - their mean score clears `FINGER_MIN_SCORE`.
- `series_to_notes` sets `ExtractedNote.finger`; the M6-H anchoring keeps it.
  A fragment that anchoring drops takes its finger with it; do not move a
  finger to a neighbouring note.
- CLI flag `--no-fingers` on `extract.py` (and env `ROCKCRAFT_NO_FINGERS=1`,
  following `ROCKCRAFT_LINEAR_KEYBOARD`) skips E1–E3 entirely, giving today's
  output byte for byte. The default is to read them.
- Bump `RSPLUS_EXTRACTOR_VERSION`.

### E4 — synthetic renderer (`rsplus_synth.py`)

`RsNote` gains an optional `finger`; the renderer draws the digit the way E0
describes (position, timing, size). Draw it with a font and size that are
**not** the classifier's template source, so the test is not a tautology.

## Tests (synthetic only)

- Each digit 1–5 rendered on a note is read back on that note.
- A note with no digit gets `None`; with no digits anywhere, the chart equals
  the `--no-fingers` chart.
- "MISS" over a note gives `None`, and the existing
  `test_miss_text_does_not_split_a_note` still passes.
- A digit visible for fewer than `FINGER_MIN_FRAMES` frames gives `None`.
- A digit that changes halfway (frames disagree) gives `None`.
- A blurred / low-contrast digit below the score threshold gives `None`.
- Digits on adjacent lanes at the same time are assigned to the right notes.
- Black-key (narrow) lane: read correctly or `None`, never a wrong digit.
- Hands and note timings are identical with and without finger reading.
- The finger survives `rsplus_audio.anchor_notes`.

## Scope boundaries (do NOT)

- Do not commit any real capture, frame, crop, template taken from a frame,
  extracted chart or audio.
- No new Python dependencies (no pytesseract/easyocr/torch).
- Do not change note, hand or timing detection, or the Synthesia path.
- Do not change the `ExtractedChart` schema beyond what M27-D added.
- No finger reading for Synthesia-style videos.
- Never guess: below the thresholds the answer is `None`.

## Known limitations / observed layout

_To be filled in by E0._

## Acceptance

- [ ] E0 findings written into this spec
- [ ] `pytest` green in `tools/synthesia-extract`
- [ ] `scripts/check-no-media.sh` passes
- [ ] Run locally on a real capture (not committed): spot-check 20 fingered
      notes — report how many were read, and that none was read wrong
- [ ] `cargo fmt` / `clippy` / `test` gate green (no Rust change expected)
- [ ] PR opened against `main` from the branch above, `Closes #374`; **not**
      self-merged (`needs-review`)
