# M21-A — Backing-track waveform data (envelope + onsets)

> Milestone: M21 — Audio-guided editing · Issue: TBD · Suggested tier: sonnet
> Branch: `claude/m21-waveform-data`

## Goal

Let the editor show the backing music's waveform behind the edit grid, so
spikes in the real track guide the timing of tricky rhythms. This spec
builds only the **data**: two per-time-slice curves computed from the
piece's backing audio, served through one new `HostCommand`. M21-B draws
them.

- **Envelope**: loudness per slice. Easy to read, but dominated by kick and bass.
- **Onsets**: how sharply the sound changes per slice (energy flux). It peaks
  where notes and hits begin, which is what helps with rhythm.

## Context

- **The audio already exists for imported videos.** The import pipeline
  extracts the source video's audio into `backing.wav` and records it as
  `meta.backing` (`crates/import/src/pipeline.rs`, `extract_backing`). So for
  an imported piece, the waveform source is the backing track. No new
  extraction step is needed.
- Backing tracks are decoded fully into memory by
  `rockcraft_audio::DecodedTrack` (`crates/audio/src/lib.rs`:
  `channels`, `sample_rate`, interleaved `samples: Arc<[i16]>`;
  `load_in_background` → `TrackLoader`).
- **Time mapping.** The backing file position for song time `t` is
  `t + backing_offset_us` (`tauri-app/src-tauri/src/audio.rs`, `backing_pos`;
  the offset is `Composer::backing_offset_us`, nudged in the editor). The data
  is therefore indexed by **file position**, so nudging the offset never
  requires a recompute.
- `core` is dependency-light on purpose (`crates/core/Cargo.toml`: serde only).
  Do **not** add an FFT crate. The onset curve below needs only simple filters.
- Host commands: `crates/control/src/host.rs` (enum, name table, parity tests),
  dispatched by the exhaustive `match` in `tauri-app/src-tauri/src/control.rs`
  and `crates/tui/src/app.rs`. Precedent for a read-only query that the TUI
  doesn't serve: `QueryVideo` (TUI answers `Unsupported`). Precedent for the
  paired Tauri IPC: `edit_query_video` ↔ `HostCommand::QueryVideo`.

## What to do

### 1. Pure analysis (core)

New module `crates/core/src/waveform.rs`:

```rust
/// Width of one slice. 10 ms: blocky at deep zoom, which is acceptable for v1.
pub const WAVEFORM_BUCKET_US: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waveform {
    pub bucket_us: u64,
    /// Bucket `i` covers file position `[i·bucket_us, (i+1)·bucket_us)`.
    pub envelope: Vec<u8>, // 0..=255
    pub onsets: Vec<u8>,   // 0..=255, same length as `envelope`
}

/// `samples` are interleaved i16 frames (as `DecodedTrack` holds them).
pub fn analyze(samples: &[i16], channels: u16, sample_rate: u32, bucket_us: u64) -> Waveform;
```

- **Mono mix:** average the channels per frame.
- **Envelope:** the peak `|x|` per bucket, converted to dB relative to the
  track's loudest bucket and mapped linearly over a **48 dB** range:
  `-48 dB → 0`, `0 dB → 255`. Values quieter than that, including silence,
  become 0. The dB scale keeps quiet passages visible.
- **Onsets (energy flux, no FFT):** split the mono signal into three bands
  with simple IIR filters (one-pole or biquad):
  low `< 200 Hz`, mid `200 Hz–2 kHz`, high `> 2 kHz`. Per bucket and band,
  take the compressed level `ln(1 + 100·rms)` (rms on a 0..1 scale). This is
  log-like for loud material but near-linear for quiet noise, so hiss and
  reverb tails don't read as onsets (a plain `ln(ε + Σx²)` would amplify
  them). The bucket's flux is the sum over
  bands of `max(0, E_b[i] − E_b[i−1])` (bucket 0 = 0). Normalise so the
  **99th percentile** of non-zero flux maps to 255 (clamp above). An all-silent
  track gives all zeros, never NaN.
- The last bucket may be partial; include it. Empty input gives empty vectors.
- Export from `crates/core/src/lib.rs`.

### 2. Sample access (audio)

`DecodedTrack` gets read accessors `channels()`, `sample_rate()`,
`samples() -> &[i16]`. No other change to `audio`.

### 3. Host command (control)

```rust
/// The attached backing track's waveform (M21-A), indexed by backing-file
/// position. Map song time t to bucket (t + backing_offset_us) / bucket_us.
BackingWaveform, // wire name "backing_waveform", no params
```

Reply payload, tagged by `status`:

```json
{ "status": "none" }                      // no backing attached
{ "status": "pending" }                   // decode/analysis still running
{ "status": "failed", "detail": "..." }  // the backing could not be decoded
{ "status": "ready", "file": "backing.wav",
  "bucket_us": 10000, "envelope": [..], "onsets": [..] }
```

Add it to the name table, `query help`, and the `host.rs` parity tests the
same way existing variants are.

### 4. Desktop backend (`tauri-app/src-tauri`)

- The first query for a backing path (the editor asks right after a bundle
  load and after `B` attaches one) starts a background thread:
  `DecodedTrack::load` → `core::waveform::analyze`. The result is cached in
  app state, keyed by the backing path (`waveform.rs`, `WaveformCache`). Never
  run this on the audio thread, and never block a command on it (`pending`
  instead).
- A query with no backing, or with a different path, drops the cache. A
  finished analysis for a path that is no longer the current one is discarded.
- Add `HostCommand::BackingWaveform` to the exhaustive match in `control.rs`
  and a paired IPC command `edit_backing_waveform` that calls the same
  function (precedent: `edit_query_video`). Add `editBackingWaveform()` and
  its TS type to `tauri-app/src/ipc/bridge.ts`.

### 5. TUI

Answer `HostCommand::BackingWaveform` with `Unsupported` (as `QueryVideo`).
Drawing in the TUI is out of scope.

## Tests

- `core::waveform` (unit, synthetic signals, no files):
  - Silence → all-zero envelope and onsets; length = ⌈duration / bucket⌉.
  - A 440 Hz tone at full scale → envelope 255 throughout; a −24 dB half →
    about 127 (±3).
  - Clicks (short bursts) every 500 ms on silence → onset values ≥ 200 in the
    click buckets and ≤ 20 elsewhere; peak bucket indices land within ±1 of
    `click_time / bucket_us`.
  - Stereo with one silent channel equals mono at half amplitude (≈ −6 dB).
  - Partial final bucket is included; empty input → empty vectors.
- `control`: parity tests cover `backing_waveform`; a help-catalog test lists it.
- Desktop: a unit test (no GUI) that the cache returns `none` → `pending` →
  `ready` for a short committed or generated WAV, and `none` after detach.

## Out of scope

- Drawing (M21-B).
- A video attached by hand in the editor (`V`) with no backing track gets no
  waveform. Imported videos already carry `backing.wav`. If this matters
  later, extract its audio with ffmpeg on attach as a follow-up.
- Finer resolution / zoom-dependent levels (blocky is accepted for v1).
- Persisting the analysis in the bundle (it is recomputed on load; ~a second
  for a full song).
