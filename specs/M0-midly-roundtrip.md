# M0 — .mid write/read round-trip via midly

> Milestone: M0 — Echo · Issue: #6 · Suggested tier: cheap
> Branch: `vibe/midly-roundtrip`

## Goal

Serialize a sequence of `core::NoteEvent`s to a Standard MIDI File (`.mid`) and
parse one back into `NoteEvent`s, proven by a round-trip test. This is what lets
a recorded session be saved and replayed.

## Context

- Crate: `crates/midi`. Read `AGENTS.md` for architecture invariants.
- Add the `midly` crate (this is the first third-party dep in the project —
  add it under `[dependencies]` in `crates/midi/Cargo.toml`).
- Source types: `rockcraft_core::{NoteEvent, NoteEventKind, MidiNote, Velocity}`
  (re-exported as `rockcraft_midi::core`). `NoteEvent` has `note`, `kind`
  (`On { velocity }` / `Off`), and `timestamp_us`.

## What to do

Two functions, e.g. in a new module `crates/midi/src/file.rs`:

```rust
pub fn events_to_smf_bytes(events: &[NoteEvent]) -> Vec<u8>;
pub fn smf_bytes_to_events(bytes: &[u8]) -> Result<Vec<NoteEvent>, SomeError>;
```

- Map each `NoteEvent` to a midly note-on / note-off message on a single track,
  channel 0. Convert `timestamp_us` to MIDI ticks using a fixed, documented
  resolution (e.g. a constant ticks-per-quarter + tempo) — round-trip fidelity
  matters more than musical accuracy here; just be consistent both directions.
- Choose a sensible error type (a small enum, or reuse midly's error). File
  paths aren't required — work in bytes so tests need no filesystem; a thin
  path-based wrapper is optional.

## Tests

- Round-trip: build a handful of `NoteEvent`s (a couple of overlapping notes,
  varying velocities), `events_to_smf_bytes` → `smf_bytes_to_events`, and assert
  you recover the same notes, kinds, and ordering. Timestamps may differ by a
  small tick-quantization tolerance — assert within that tolerance, or pick a
  resolution where the chosen test timestamps are exact.

## Scope boundaries (do NOT)

- Only `crates/midi`. Add `midly` only — do not add `midir` or touch any live
  input path (that's a separate, local-only task).
- Do not change `crates/core`.

## Acceptance

- [ ] `cargo fmt --all --check`, `clippy --workspace --all-targets` (warnings =
      errors), `cargo test --workspace` all green
- [ ] PR against `main` from `vibe/midly-roundtrip`, `Closes #6`
