# synthesia-extract — visual note extractor (M6-C)

A Python + OpenCV **sidecar** that turns a *local* Synthesia-style tutorial
video (a falling piano-roll) into the M6-A `ExtractedChart` JSON consumed by the
Rust `rockcraft-import` crate. Classical computer vision — no ML, no network, no
audio.

This is **not** a workspace crate. It runs as a standalone process, invoked by
the M6-D pipeline as a subprocess. Keeping OpenCV out of the Rust build is
deliberate.

## What it recovers (phase 1)

For each note: **pitch + onset + duration + hand** (Left/Right/Unknown) plus a
per-note **confidence**. Velocity is left `null` — it is filled later by M6-F
audio-fusion.

## Setup

```bash
cd tools/synthesia-extract
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
```

`opencv-python-headless` is used so this works in CI / cloud sandboxes with no
GUI libraries.

## Usage

```bash
# From a video file (anything OpenCV can decode):
python extract.py --in tutorial.mp4 --out chart.json

# From a directory of numbered frames (fps must be given explicitly):
python extract.py --in frames_dir/ --fps 30 --out chart.json

# To stdout:
python extract.py --in tutorial.mp4

# Tuning aids:
python extract.py --in tutorial.mp4 --out chart.json --debug        # annotated frames -> ./debug-out/ (gitignored)
python extract.py --in tutorial.mp4 --anchor-c4-x 512               # override octave anchoring
python extract.py --in tutorial.mp4 --scroll 180                    # override scroll-speed estimation
```

**Input is a local path only — no downloading.** Fetching source media is the
pluggable hook in M6-D; audio fusion is M6-F. Do not point this at, or commit,
any copyrighted video, frames, or extracted song.

## How it works

A Synthesia bar is a *rigid* falling rectangle, so a colored pixel at row `y` in
a frame at time `t` represents song-time `t + (hit_line - y) / scroll`. The
pipeline:

1. **Hit-line** — find the keyboard's top edge (bottom block of white rows).
2. **Keyboard calibration** — white/black key x-boundaries become a pitch ruler;
   the black-key 2/3 grouping anchors octaves (middle C → MIDI 60, overridable
   with `--anchor-c4-x`).
3. **Scroll speed** — vertical cross-correlation of the falling-region bar mask
   across frames (px/s).
4. **Roll stitching** — every colored pixel of every frame votes into a per-pitch
   song-time occupancy map; overlapping frames reinforce true bars, sub-frame
   interpolation beats the frame grid.
5. **Notes + hands** — threshold coverage into note runs; cluster the (typically
   two) dominant bar colors into Left/Right (lower average pitch → Left), else
   `Unknown`. Coverage and color-purity feed `confidence` for the M6-E review.

## Output

`ExtractedChart` JSON matching `crates/import/src/schema.rs` exactly — field
names, `hand` strings (`"Left"`/`"Right"`/`"Unknown"`), and the omit-when-`None`
behavior for `velocity`/`confidence`. Example:

```json
{
  "notes": [
    {"pitch": 60, "start_us": 0, "dur_us": 300000, "hand": "Right", "confidence": 1.0}
  ],
  "source": {
    "extractor_version": "synthesia-extract 0.1.0",
    "fps": 30.0,
    "scroll_px_per_s": 180.0
  }
}
```

## Tests

```bash
pip install -r requirements.txt
pytest
```

Tests are **fully synthetic**: `synthesia_extract/synth.py` renders a fabricated
falling-bar clip (a two-hand C-major fixture) in memory, and the tests assert the
extractor recovers the ground truth — pitches exactly, onsets/durations within
frame tolerance, hands mapped correctly. No real or copyrighted media is ever
read or committed.
