# M6-A — import: data contract + chart bundle writer

> Milestone: M6 — Video Import · Issue: #113 · Suggested tier: sonnet
> Branch: `claude/m6-import-contract`

## Goal

Stand up the new `rockcraft-import` crate and the typed seam the whole import
pipeline plugs into: the **extractor JSON schema** a sidecar emits, a **parser**
into `rockcraft-core` types, and a **bundle writer** that produces a `.mid` +
`meta.json` chart (reusing the existing song-bundle model) in the gitignored
import output dir.

## Context

- New crate `crates/import` (`rockcraft-import`), added to the workspace
  `members` in the root `Cargo.toml`. Depends **inward on `rockcraft-core`
  only** (architecture invariant) — no midi/audio/tui, no device, no network.
- Reuses `core`'s bundle model: `RecordingMeta` / `BackingTrack`
  (`crates/core/src/song.rs`) and the MIDI write path used by the editor
  (`events_to_smf_bytes` via the timeline → events). Mirror how
  `EditScreen::save_bundle` writes `song.mid` + `meta.json`.
- Output location is defined by M6-B (#114): a gitignored import dir. This task
  provides a path helper; M6-B owns the gitignore/CI guard.
- Cloud-testable: the only fixtures are **synthetic** (fabricated notes); no
  real video/song/`.mid` is committed (see M6-B).

## What to do

### 1. Schema — `crates/import/src/schema.rs`

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ExtractedChart {
    pub notes: Vec<ExtractedNote>,
    pub source: SourceMeta,   // provenance/diagnostics; never committed
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ExtractedNote {
    pub pitch: u8,            // MIDI note number
    pub start_us: u64,
    pub dur_us: u64,
    pub hand: Hand,           // L / R / Unknown
    pub velocity: Option<u8>, // None until M6-F audio-fusion fills it
    pub confidence: Option<f32>, // 0..=1, for the review step (M6-E)
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum Hand { Left, Right, Unknown }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SourceMeta {
    pub title: Option<String>,
    pub fps: Option<f32>,
    pub scroll_px_per_s: Option<f32>,
    pub extractor_version: String,
}
```

### 2. Parser — `ExtractedChart` → core

```rust
/// Convert to core note events (pairing into a Timeline). Invalid pitches
/// (>127) are an error; missing velocity defaults to a documented constant
/// (e.g. 80) so MIDI is always valid; Unknown hand maps to a single track.
pub fn chart_to_timeline(chart: &ExtractedChart) -> Result<rockcraft_core::Timeline, ImportError>;
```

### 3. Bundle writer

```rust
/// Write `<dir>/song.mid` + `<dir>/meta.json` (RecordingMeta, backing: None,
/// version 1). `dir` must live under the gitignored import output root
/// (`import_output_dir()`); refuse to write under a tracked path.
pub fn write_chart_bundle(chart: &ExtractedChart, dir: &std::path::Path)
    -> Result<std::path::PathBuf, ImportError>;

/// The canonical gitignored output root (defined with M6-B), e.g.
/// `<workspace>/import-out`. A helper so callers never hard-code it.
pub fn import_output_dir() -> std::path::PathBuf;
```

`from_json(&str) -> Result<ExtractedChart, ImportError>` and `to_json` round-trip
the schema.

## Tests (headless, synthetic fixtures only)

- Round-trip: a hand-built `ExtractedChart` → `to_json` → `from_json` is equal.
- `chart_to_timeline`: pitches/timings land on the right notes; missing velocity
  → default; `pitch > 127` → `ImportError`.
- `write_chart_bundle` into a `tempfile` dir produces a parseable `song.mid`
  (reload via the same path the editor uses) and a `meta.json` that
  `RecordingMeta::from_json` accepts.
- Committed fixture is `tests/fixtures/synthetic_chart.json` — obviously
  fabricated (e.g. a C-major scale), never a real song.

## Scope boundaries (do NOT)

- No network, no subprocess, no CV/audio — those are M6-C/M6-D/M6-F.
- Do not commit any real media or extracted `.mid` (M6-B enforces this).
- Depend only on `rockcraft-core`; do not pull in midi/audio/tui.
- New third-party deps limited to `serde`/`serde_json` (already in the
  workspace) and the MIDI writer already used by core/the editor — no others.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m6-import-contract`, `Closes #113`
