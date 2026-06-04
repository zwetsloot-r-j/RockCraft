# M3-A — core: editable note model + ops

> Milestone: M3 — Composer · Issue: #49 · Suggested tier: opus
> Branch: `claude/m3-timeline`

## Goal

Give `core` a **pure, editable** representation of a song that the composer can
mutate note-by-note: add, remove, move, resize, transpose. This is the model the
TUI edit screen (#52) drives; everything else in M3 builds on it. It converts
losslessly to/from `Vec<NoteEvent>` so the existing MIDI writer, highway, and
scoring keep working unchanged.

## Context

- Crate: `crates/core` (new module `timeline.rs`, re-export from `lib.rs`).
- `NoteEvent`/`MidiNote`/`Velocity`/`NoteEventKind` already exist
  (`core/events.rs`). A note in an editor is a *span* — the same pairing
  `highway::build_spans` already does (`tui/highway.rs`). Mirror that pairing in
  `from_events`.
- `core` stays pure (see `CLAUDE.md`): no I/O, no device, no terminal. `serde`
  is already a workspace dep and may be derived if useful.

## What to do

```rust
// crates/core/src/timeline.rs

/// Opaque, stable handle to a note inside a `Timeline`. Survives edits to other
/// notes; invalidated only when its own note is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteId(u32);

/// One editable note: a pitch sounding for `dur_us` from `start_us`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub pitch: MidiNote,
    pub start_us: u64,
    pub dur_us: u64,
    pub velocity: Velocity,
}

#[derive(Debug, Clone, Default)]
pub struct Timeline { /* stable id -> Note + monotonic next_id */ }

impl Timeline {
    pub fn new() -> Self;
    pub fn insert(&mut self, note: Note) -> NoteId;
    pub fn remove(&mut self, id: NoteId) -> Option<Note>;
    pub fn get(&self, id: NoteId) -> Option<&Note>;

    /// Reposition: set start and/or pitch. Returns false if `id` is unknown.
    pub fn set_start(&mut self, id: NoteId, start_us: u64) -> bool;
    /// Transpose by semitones, clamped to 0..=127. No-op (returns false) if the
    /// result would leave the MIDI range or `id` is unknown.
    pub fn transpose(&mut self, id: NoteId, semitones: i8) -> bool;
    /// Set duration (>= 1 µs; 0 is clamped to 1). Returns false if unknown.
    pub fn resize(&mut self, id: NoteId, dur_us: u64) -> bool;

    /// The note whose span covers `[start, start+dur)` at `pitch` and `us`,
    /// if any — what the cursor edits. Highest-id wins on overlap.
    pub fn find_at(&self, pitch: u8, us: u64) -> Option<NoteId>;

    /// Notes in stable id order (insertion order).
    pub fn notes(&self) -> impl Iterator<Item = (NoteId, &Note)> + '_;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;

    /// Emit on+off events, sorted by timestamp (note-off before note-on at an
    /// equal timestamp). Feeds `events_to_smf_bytes` and `build_spans`.
    pub fn to_events(&self) -> Vec<NoteEvent>;
    /// Rebuild a timeline from an event stream, pairing each on with the next
    /// off of the same pitch (same rule as `build_spans`; dangling ons closed
    /// at the last timestamp seen, dur >= 1).
    pub fn from_events(events: &[NoteEvent]) -> Self;
}
```

## Tests

- `insert` then `get` returns the note; `remove` returns it and `get` is `None`.
- `set_start` / `resize` mutate only the targeted note; ids are stable across
  other inserts/removes.
- `transpose` clamps: +12 on C8(108)→fails near top; -1 on A0(21) succeeds, on
  C-1(0) fails; returns false leaves the note unchanged.
- `find_at` returns the covering note and `None` when the cursor is on an empty
  slot; highest-id wins on overlap.
- `to_events`/`from_events` round-trip a multi-note, overlapping-chord fixture
  (pitch, start, dur, velocity preserved).

## Scope boundaries (do NOT)

- No changes to `NoteEvent`/`Velocity`/`MidiNote` or scoring.
- No I/O, no new third-party deps (serde derive only if needed; it's a workspace dep).
- No grid/snap logic here — that is #50. No undo/redo here — that is #61.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-timeline`, `Closes #49`
