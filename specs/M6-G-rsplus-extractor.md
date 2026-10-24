# M6-G — sidecar: Rocksmith+ piano-highway extractor

> Milestone: M6 — Video Import · Issue: #364 · Suggested tier: opus
> Branch: `claude/m6-rsplus-extract`

## Goal

Import a screen capture of a song played in **Rocksmith+ (piano path)** as a
chart: pitch, onset, duration and hand for every note, so the user can open it
in the editor over the original video and audio and edit it from there. The
M6-C Synthesia extractor recovers **zero notes** from such a capture, because
Rocksmith+ draws a 3D perspective highway rather than a flat falling roll.

## Context

- Extends the M6-C (#115) sidecar in `tools/synthesia-extract/`; emits the same
  M6-A (#113) `ExtractedChart` JSON, so the Rust pipeline (`crates/import`) is
  unchanged. The sidecar decides the layout itself, the same way the score
  sidecar decides whether an input needs OMR.
- What a Rocksmith+ capture looks like (measured on two real recordings, which
  are **not** committed — see `docs/IMPORT.md`):
  - a cyan hit line on top of a 3D keyboard; slanted cyan highway edges; lanes
    and key lines share one vanishing point;
  - the keyboard does **not** pan or zoom within a song (only its range
    differs between songs);
  - keys light up well *before* their note arrives, so key colour is a preview,
    not an onset; the bar crossing the hit line is the note;
  - white-key lanes are centred on the part of the key visible *between* the
    black keys (C/F lanes sit left of the key centre, E/B right, by ~0.13 key);
  - bars are glassy tubes: bright edges, translucent body. Black-key bars are
    often hollow (dark body). A missed short note passes as a pale block with no
    bright edges. A played bar is washed out by a hit effect for a few frames,
    and a missed note gets white "MISS" text right on the hit line;
  - teal-to-blue fog glows drift over the floor, as bright as a pale bar;
  - hand zones are tinted on the floor and scroll with the notes: left hand a
    dim greyish purple-magenta, right hand a saturated teal;
  - middle C's key label is a solid dark box; other C labels are thin letters.

## What to do

New module `synthesia_extract/rsplus.py`:

- `is_rsplus(frames) -> bool` / `detect_video(path) -> bool`: the cyan hit line
  plus side edges converging upward, on at least half the sampled frames.
- `calibrate(frames, octave_shift=0) -> Geometry`, once per video from ~40
  frames spread over the clip: hit line rows; vanishing point (from the edge
  lines); keyboard bands; white-key separators (thin dark lines in the
  white-only band); black keys (centre and width); the black-key pattern's
  phase (C position); the octave anchor (the middle-C box, else the C nearest
  the frame centre, with a warning); lane centres projected through the
  vanishing point to a 3-row sample band just above the hit line. Lanes closer
  than 0.3 key to a highway edge are dropped (that is the edge's own glow).
- Per frame, read only the sample band (a per-column median HSV row):
  - bar pixels: S >= 45 and V >= 100; pixels in the fog's hue range (85-114)
    with V < 150 must also stand out from the local mean and form a run with a
    sharp edge;
  - lane state `STRONG` (a bar run spans the lane centre, or a hollow bar's dark
    body — no wider than 0.36 key for a black lane, 0.45 for a white one —
    is centred on it between same-hue edges with a matching body), `WEAK`
    (matching edges but a washed-out body, or white overlay text on the lane),
    or `OFF`;
  - the floor's hand zone at each uncovered lane centre.
- `refine_lanes`: snap lanes to where bar-sized runs actually cross the sample
  band (a weighted linear fit of offset against x); keep the projection when
  evidence is thin or the shift is implausible.
- `series_to_notes`: a note is a run of lit frames holding at least 2 `STRONG`
  frames, trimmed to its first/last strong frame; weak frames bridge but never
  start a note. Hand = majority floor zone just before/after the note on its
  lane, weighting the side where the phrase continues. `confidence` = strong
  fraction.
- `extract_video(path)`: calibrate on a sparse sample, then stream every frame.
  `SourceMeta`: `extractor_version = "rsplus-extract 0.1.0"`, fps, frame height,
  hit line; **no** `scroll_px_per_s` (a perspective highway has no single
  scroll speed, so the importer must not derive a linear overlay alignment).

`extract.py`: `--layout auto|synthesia|rsplus` (default `auto`, logged on
stderr) and `--octave-shift N` (Rocksmith+ only). The Synthesia path is
untouched.

`synthesia_extract/rsplus_synth.py`: a synthetic renderer reproducing each cue
above, for tests.

## Tests (synthetic only)

`tests/test_rsplus.py`: layout detection both ways (an M6-C synthetic clip is
not Rocksmith+); calibrated lanes within 2.5 px of truth with the right pitches
and middle C anchored; white lanes offset between black keys; mixed notes
(tube, hollow, pale) recovered with pitch exact, onset within 1 frame, duration
within 2; neighbouring white bars do not light the black lane between them,
while a black bar beside a white one lights both; repeated pale notes stay
separate; MISS text does not split a note; fog makes no notes; hands from zone
tints, unknown without zones; weak frames bridge but never start a note; source
metadata; CLI auto-detects a Rocksmith+ frames directory. The existing M6-C
suite still passes (auto-detection keeps it on the Synthesia path).

## Scope boundaries (do NOT)

- Do not commit any real capture, frame, extracted chart or audio.
- Do not change the Rust pipeline or the `ExtractedChart` schema.
- Do not change the Synthesia (M6-C) extraction itself.
- No new Python dependencies.

## Known limitations

- Timing resolution is one video frame (33 ms at 30 fps); no sub-frame
  interpolation.
- Notes the player actually hits are consumed by Rocksmith+ and read as very
  short; record without playing (every note then reads at full length).
- Lane refinement is linear in x; a pale bar inside a fog glow can be missed.
- The middle-C fallback guesses when the label is out of view (`--octave-shift`).

## Acceptance

- [ ] `pytest` in `tools/synthesia-extract` green
- [ ] `cargo fmt --all --check` / `cargo clippy --workspace --all-targets` /
      `cargo test --workspace` clean (no Rust changes expected)
- [ ] PR opened against `main` from the branch above, `Closes #364`
