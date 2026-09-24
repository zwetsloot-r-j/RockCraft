# M16-A — Tempo map from audio (adaptive BPM)

> Milestone: M16 — Adaptive tempo · Issue: #273 · Suggested tier: opus
> Branch: `claude/m16-tempo-map-from-audio`

## Goal

Let a piece whose tempo breathes (a hand-played cover, rubato) get bar lines that
follow the performance, **detected from its backing audio** instead of built bar
by bar. #271 added the per-bar tempo map (`RecordingMeta.bar_starts`) and the
per-bar nudge tools; this task adds the missing pieces: a way to install a whole
map, a beat/downbeat detector that produces one, and making the remaining
uniform-tempo consumers (quantize, metronome, play highway) follow the map.

Motivating piece: an imported ~4:50 piano cover whose beat tracker pulse sits at
~172/min (eighth notes; ~86 BPM quarters) with per-bar drift of roughly ±5–10%.

## Context

- Tempo map: `Composer.bar_starts` (`crates/core/src/composer.rs`) — song-time µs
  of each bar's downbeat, ascending; empty = uniform `Grid`. Helpers
  `bar_start_us` / `bar_dur_us` / `bar_at_us` / `pos_us_of_step` /
  `pos_step_index` / `pos_snap`; `set_bar_starts` (no action wraps it yet).
  Steps are spaced evenly **within** each bar (per-bar granularity).
- Persistence already round-trips `bar_starts` (`RecordingMeta`, tauri `state.rs`).
- Still uniform: `quantize_region` (`grid.snap_to_step`), `tick_metronome_click`
  (`grid.quarter_us()`), the play highway (`liveSong.ts` `BEAT`/`BAR` from `bpm`).
- Sidecar pattern: `crates/import/src/pipeline.rs` (`run_sidecar`,
  `workspace_root`, `env_ffmpeg_cmd`) — `python3 <script> --in X --out -`,
  stdout JSON, stderr diagnostics.
- Two-tier agent surface (CLAUDE.md): pure → `core::Action`; I/O → `HostCommand`.

## What to do

### 1. core — install a whole map (pure)

```rust
// crates/core/src/action.rs
SetBarStarts { bars_us: Vec<u64> },   // name "set_bar_starts"
```

- `bars_us` empty → clear the map (uniform grid).
- Otherwise it must have ≥ 2 entries and be **strictly ascending**; anything else
  is a no-op (map unchanged).
- On success also set `grid.origin_us = bars_us[0]` and `grid.bpm` to the tempo
  of the **median** bar duration (`60e6 * beats_per_bar / median_bar_us`, rounded,
  clamped to the existing BPM range) so the uniform fallback, status bar and
  anything still reading `grid.bpm` agree with the map. **No note moves.**
- Cursor re-snaps via the existing `set_bar_starts` logic.
- Help entry + parity tests, as every action.

### 2. core — make quantize and metronome follow the map

- `quantize_region` with a non-empty map: resolution `n = max(1,
  round(grid.bar_us() / step_us))` divisions per bar; snap an onset to the nearest
  `bar_start + k * bar_dur / n` of **the bar containing it** (k = n rolls to the
  next downbeat); snap the end the same way in the bar containing the end;
  minimum length = one division of the bar the snapped onset lands in. Uniform behaviour unchanged.
- `tick_metronome_click` with a non-empty map and `now >= bars[0]`: beat index =
  `bar * bpb + floor((now - bar_start) * bpb / bar_dur)`; accent when the in-bar
  beat is 0. Uniform behaviour unchanged.

### 3. sidecar — `tools/tempo-map/` (numpy + stdlib only)

`python3 tools/tempo-map/tempo_map.py --in <audio.wav> --out - [--beats-per-bar N]
[--anchor-us US] [--tempo-hint BPM] [--min-bpm 40] [--max-bpm 220]`

Output JSON:

```json
{"version":1, "beats_us":[...], "bars_us":[...], "bpm":86.1,
 "pulse_bpm":172.3, "anchor_us":3070000}
```

Algorithm (Ellis 2007 dynamic-programming beat tracker, as librosa):
1. Read WAV via stdlib `wave` (PCM 8/16/24/32-bit), mix to mono, decimate to ~11 kHz.
2. Onset envelope: log-magnitude STFT spectral flux (half-wave rectified, summed
   over bins), hop ≈ 5.8 ms, detrended and normalised.
3. Pulse tempo: autocorrelation of the envelope weighted by a log-Gaussian prior
   centred at 120/min; pick the peak in `[min, max]`.
4. DP beat tracking over the envelope with that period (tightness ≈ 100); trim
   weak leading/trailing beats.
5. Pulse → beat: with `--tempo-hint`, scale the pulse period by ×½/×1/×2 to the
   octave nearest the hint. Otherwise fold ×2 when the pulse rate > 140/min and
   alternate pulses differ in onset strength by ≥ 10% — judged as the median over
   16-pulse windows, since a tracker slip flips the accent parity mid-song and a
   global average cancels out. Folding **re-tracks** at the doubled period (the
   DP follows the accented stream) rather than decimating the pulse.
   Frame times are shifted by a calibrated onset lag (~30 ms) so beats land on
   attacks.
6. Bars: group beats by `beats_per_bar` (default 4). Anchor = the beat nearest
   `--anchor-us` if given, else the phase with the highest mean low-band
   (< 250 Hz) onset strength. Bars extend backwards from the anchor while a full
   bar's downbeat index is ≥ 0, and one extrapolated bar end is appended after
   the last full bar so the final bar has a length.

Tests (`tools/tempo-map/tests/`, synthetic audio only — no copyrighted media):
steady 100 BPM click track → beats within 15 ms, bpm ≈ 100; a track accelerating
90 → 110 → beats within 20 ms and bar durations shrinking; accented eighths at
172/min → bpm ≈ 86 on the accented clicks; the fold decision survives a
mid-song parity flip; `--tempo-hint` 86 vs 172 picks the octave; `--anchor-us`
selects the bar phase; a bass accent picks the downbeat; accent-less noise still
returns a valid ascending map; the CLI reads a 16-bit stereo WAV.

CI: run `pytest` for `tools/tempo-map` in the existing job (numpy only).

### 4. import crate — Rust wrapper (I/O)

```rust
// crates/import/src/tempo.rs  (re-exported from lib.rs)
pub struct TempoMapOpts { pub beats_per_bar: u8, pub anchor_file_us: Option<u64>,
                          pub tempo_hint_bpm: Option<f64> }
pub struct DetectedTempoMap { pub bars_us: Vec<u64>, pub beats_us: Vec<u64>,
                              pub bpm: f64, pub pulse_bpm: f64, pub anchor_us: u64 }
pub fn detect_tempo_map(audio: &Path, opts: &TempoMapOpts)
    -> Result<DetectedTempoMap, ImportError>;
/// File time → song time for a backing with `audio_start_us` (song = file − start);
/// None for positions before song time 0.
pub fn file_to_song_us(file_us: u64, audio_start_us: i64) -> Option<u64>;
pub fn song_to_file_us(song_us: u64, audio_start_us: i64) -> Option<u64>;
```

Non-`.wav` input is first decoded to a temp WAV with ffmpeg (`env_ffmpeg_cmd`).
Times in `DetectedTempoMap` are **file** times.

### 5. control — `HostCommand::DetectTempoMap`

```rust
DetectTempoMap { beats_per_bar: Option<u8>, anchor_us: Option<u64>,
                 tempo_hint_bpm: Option<f64> }   // name "detect_tempo_map"
```

Runs on the loaded piece's backing audio (error if none). `beats_per_bar`
defaults to the grid's; `anchor_us` is **song** time (converted with the
backing offset). Bars before song time 0 are dropped; the result is applied via
`Action::SetBarStarts`. Returns `{bars, bpm, pulse_bpm, anchor_us}` (song
time) plus the new snapshot. Help entry + parity tests. Tauri dispatches it;
the TUI returns `Unsupported`.

### 6. Tauri frontend

- Edit screen: `I` = detect the tempo map, anchored at the cursor's time; show the
  result (bars, BPM) in the status line; the grid redraws from the
  snapshot's `bar_starts`.
- Play highway: the play session carries the map shifted into play-clock time
  (`bar_starts_us`); `drawGrid` draws bar lines at those times and beat lines
  evenly inside each bar when present, the uniform grid otherwise.

## Scope boundaries (do NOT)

- No per-beat (intra-bar) tempo map — per-bar granularity only.
- Do not move notes when installing a map; re-timing notes is `nudge_bar_tempo`'s job.
- No new Python deps beyond numpy; do not touch the Synthesia extractor.
- No tempo-mapped MIDI export (`crates/midi` 1 tick = 1 µs TODO stays).
- No copyrighted audio in tests or fixtures.

## Acceptance

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`
- [ ] `tools/tempo-map` pytest green; frontend `tsc --noEmit` + `npm test` green
- [ ] Action/HostCommand parity tests updated
- [ ] PR against `main`, `Closes #273`
