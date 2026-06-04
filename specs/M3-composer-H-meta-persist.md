# M3-H — core+tui: persist grid & key in meta.json

> Milestone: M3 — Composer · Issue: #56 · Suggested tier: sonnet
> Branch: `claude/m3-meta-persist`

## Goal

So a saved composition reopens with the tempo, time signature, snap, and key it
was authored in — store `Grid` and `Key` in the bundle's `meta.json`, optionally
and backward-compatibly (old bundles without them still load).

## Context

- Crates: `crates/core` (`song.rs::RecordingMeta`) and `crates/tui`
  (`edit.rs` save/load — #55).
- `Grid` (#50) and `Key` (#51) already derive `Serialize/Deserialize`.
- `RecordingMeta` already uses `#[serde(default)]` for `backing`/`version`; follow
  that pattern so the additions are non-breaking (see existing
  `minimal_legacy_json_deserializes` test).

## What to do

In `core/song.rs`, extend `RecordingMeta`:

```rust
pub struct RecordingMeta {
    pub midi_file: String,
    #[serde(default)] pub backing: Option<BackingTrack>,
    #[serde(default)] pub grid: Option<Grid>, // None for legacy / piano recordings
    #[serde(default)] pub key: Option<Key>,
    #[serde(default = "default_version")] pub version: u32,
}
```

- In `EditScreen::save()` (#55) write `Some(grid)`/`Some(key)`.
- In "Edit last recording" load, read `meta.json`; if `grid`/`key` present, seed
  the editor with them, else fall back to `Grid::default_120()` / C major.

## Tests

- `RecordingMeta` round-trips with `grid`+`key` populated.
- A legacy `meta.json` (only `midi_file`, or `midi_file`+`backing`) still
  deserializes with `grid`/`key` == `None` (extend the existing legacy test).
- tui: a saved-then-loaded composition restores its grid/key (assert the editor's
  grid bpm/subdivision and key match what was saved).

## Scope boundaries (do NOT)

- Keep the fields optional; do not bump or repurpose `version` semantics.
- Do not move `Grid`/`Key` out of their modules; just reference them in `song.rs`.
- No new third-party deps beyond the existing workspace `serde`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-meta-persist`, `Closes #56`
