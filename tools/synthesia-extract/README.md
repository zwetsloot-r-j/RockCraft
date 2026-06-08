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
per-note **confidence**. Velocity is left `null` — it is filled by the optional
M6-F audio-fusion pass (below).

## Audio fusion (phase 2, M6-F) — optional

The picture has no dynamics. When the video's audio is **clean piano** (a MIDI
render or a solo-piano take), the optional `--audio-fusion` pass enriches the
chart with what the picture can't give:

* **velocity** — from each note's loudness, and
* **sub-frame onset/offset** — audio is sample-precise where the visual roll is
  quantised to the video frame grid.

It is strictly additive: M6-C alone still produces a full chart, and on
**full-band** audio (vocals/drums/broadband energy) the pass detects the
unsuitable input and **no-ops**, leaving the visual notes authoritative rather
than corrupting them.

```bash
# Visual + audio fusion (audio is a 16-bit PCM WAV alongside the video/frames):
python extract.py --in frames_dir/ --fps 30 --audio clip.wav --audio-fusion --out chart.json
```

The diagnostic `source.audio_fusion` records what happened (`"applied: ..."` /
`"skipped: ..."`). Fusion never overrides visual **pitch** or invents notes the
picture didn't show: it only matches a transcribed event to an existing visual
note (same pitch, onset within the visual's frame uncertainty), copies velocity,
and nudges timing **within** that uncertainty. Unmatched visual notes keep their
timing and take a default velocity.

### Transcription dependency

The bundled transcriber (`synthesia_extract.audio`) is **self-contained
classical DSP** (per-pitch FFT band-pass → amplitude envelope → onsets +
velocity), so it needs no ML weights and stays hermetic in CI. For real-world
accuracy you can drop in a learned piano-transcription model — e.g.
[`basic-pitch`](https://github.com/spotify/basic-pitch) or an
Onsets-and-Frames implementation — behind the same `TranscribedNote` interface
(`pitch`, `start_us`, `dur_us`, `velocity`); a learned model reports absolute
MIDI velocity and would set `REFERENCE_PEAK_AMPLITUDE`/`_ENVELOPE_PEAK_CAL` to
`1.0`. Such a model is an **optional** extra (heavy TF/Torch deps) and is *not*
in `requirements.txt`; the fusion path works without it.

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
