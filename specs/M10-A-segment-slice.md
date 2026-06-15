# M10-A — Segment/slice math: sub-timeline + derived backing/video offsets

> Milestone: M10 — Split & Trim into Pieces · Issue: #217 · Suggested tier: opus
> Branch: `claude/m10-segment-slice`
> Related: M9-E (#204, backing persistence), M9-G (#206, video persistence)

## Goal

Add the pure-`core` foundation for cutting a piece into parts: given a source
`Timeline` and the piece's optional `BackingTrack` / `BackgroundVideo`, produce
a new sub-timeline for a half-open song-time range `[start_us, end_us)`, shifted
so the range start becomes time 0, together with the **derived** media
references for the new bundle. No I/O — this is data + math only, headless-
testable against fixtures.

## Context

- `crates/core/src/timeline.rs::Timeline` already has the building blocks:
  `notes_in_region(pitch_lo, pitch_hi, us_lo, us_hi)` filters by a half-open
  time window (`start_us >= us_lo && start_us < us_hi`), and `insert_shifted`
  shows the shift-by-`d_us` clone pattern. `Note { pitch, start_us, dur_us, .. }`.
- `crates/core/src/song.rs` defines `BackingTrack { file, audio_start_us: u64 }`
  and `BackgroundVideo { file, offset_us: i64 }`. The sync semantics we mirror:
  - backing playback position is `clock - shift + audio_start_us`
    (`backing_position_us`), so a part starting at `S` in the original plays the
    same audio by setting `audio_start_us' = audio_start_us + S`.
  - video alignment is `videoTime = songTime + offset_us` (M9-G), so a part
    starting at `S` shows the same frame at part-time 0 by setting
    `offset_us' = offset_us + S`.
- **Media strategy is reference + offset (no re-encode):** the part bundle keeps
  a *full, unchanged copy* of the media file (`file` stays the same name); only
  the offsets change. The actual file copy is M10-B's job — `core` only computes
  the references.

## What to do

Create `crates/core/src/segment.rs` (and `pub mod segment;` + re-exports in
`lib.rs`, matching how `song`/`timeline` are exposed).

```rust
// in crates/core/src/segment.rs
use crate::{BackgroundVideo, BackingTrack, Timeline};

/// A consecutive slice of a piece's timeline, in song-time microseconds.
/// Half-open: `start_us` inclusive, `end_us` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start_us: u64,
    pub end_us: u64,
}

/// The sliced piece for one part: a sub-timeline shifted to t=0 plus the media
/// references derived for the new bundle (file names unchanged; offsets shifted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceResult {
    pub timeline: Timeline,
    pub backing: Option<BackingTrack>,
    pub video: Option<BackgroundVideo>,
}

/// Slice `src` to `seg`, returning a sub-timeline shifted so `seg.start_us`
/// maps to 0, with derived `backing`/`video` references.
///
/// - Notes are included iff `start_us` is in `[seg.start_us, seg.end_us)`
///   (same rule as `notes_in_region`); each kept note's `start_us` is reduced
///   by `seg.start_us`.
/// - Durations are **clipped** so a note never extends past the segment length
///   (`dur_us = min(dur_us, (seg.end_us - new_start_us))`), clamped to >= 1 µs.
/// - `backing` => `audio_start_us += seg.start_us` (file unchanged); `None`
///   stays `None`.
/// - `video`   => `offset_us += seg.start_us as i64` (file unchanged); `None`
///   stays `None`.
pub fn slice_segment(
    src: &Timeline,
    seg: Segment,
    backing: Option<&BackingTrack>,
    video: Option<&BackgroundVideo>,
) -> SliceResult;

/// Turn sorted-or-unsorted split points into consecutive segments covering
/// `[0, total_us)`. Points are clamped to `[0, total_us]`, deduplicated and
/// sorted; empty/zero-width gaps are dropped. No splits => one segment
/// `[0, total_us)`. `total_us == 0` => empty vec.
pub fn segments_from_splits(splits: &[u64], total_us: u64) -> Vec<Segment>;
```

Keep `slice_segment` independent of how `Segment`s were chosen — M10-B/C/D
decide keep/discard and naming; this module only slices.

## Tests

In `segment.rs` (`#[cfg(test)]`), all headless:

- **Note windowing + shift:** build a timeline with notes at 0, 1 s, 2 s, 3 s;
  slice `[1_000_000, 3_000_000)` → exactly the 1 s and 2 s notes remain, now at
  0 and 1 s. Note at 3 s excluded (exclusive end); note at 0 excluded.
- **Duration clip:** a note at 2.5 s with 1 s duration, sliced to
  `[0, 3_000_000)` → its duration clips to 0.5 s; never below 1 µs.
- **Backing offset:** `BackingTrack { audio_start_us: 250_000 }`, slice starting
  at `S = 1_000_000` → derived `audio_start_us == 1_250_000`, `file` unchanged.
  `None` backing → `None`.
- **Video offset:** `BackgroundVideo { offset_us: -200_000 }`, `S = 1_000_000`
  → derived `offset_us == 800_000`, `file` unchanged. `None` video → `None`.
- **`segments_from_splits`:** `([1s, 2s], 3s)` → `[0,1s),[1s,2s),[2s,3s)`;
  `([], 3s)` → `[0,3s)`; out-of-range / duplicate / unsorted points are
  clamped, deduped, sorted; `total_us == 0` → `[]`.
- A full round-trip sanity check: concatenating the slices of an un-gapped split
  plan covers every note exactly once.

## Scope boundaries (do NOT)

- Do **not** add any I/O, file copy, ffmpeg, or media decoding — `core` stays
  pure. This module computes references and timelines only.
- Do **not** trim/re-encode media or add an `end`/length field to
  `BackingTrack`/`BackgroundVideo` — reference + offset is the chosen model.
- Do **not** change existing `Timeline`/`song.rs` public signatures; add the new
  module alongside them.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green (`cargo test -p rockcraft-core` suffices to
      iterate — no system deps needed)
- [ ] `slice_segment` / `segments_from_splits` land in `core` with the tests above
- [ ] PR opened against `main` from the branch above, `Closes #217`
