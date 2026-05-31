# M0 — core event buffer / recording timeline

> Milestone: M0 — Echo · Issue: #5 · Suggested tier: sonnet
> Branch: `claude/core-buffer`

## Goal

A pure, in-memory buffer that accumulates `NoteEvent`s in timestamp order. It's
what a live session records into, and later what we compare a performance
against. No I/O, no device — `crates/core` stays headless-testable.

## Context

- Crate: `crates/core`. Read `CLAUDE.md` for the architecture invariants
  (core stays pure; expand the typed model over stringly-typed shortcuts).
- Builds on the existing `NoteEvent` (fields: `note`, `kind`, `timestamp_us`)
  in `crates/core/src/events.rs`.
- Put this in a new module `crates/core/src/timeline.rs` and wire it into
  `lib.rs` (`pub mod timeline;` plus a re-export of the main type).

## What to do

Add a type roughly like:

```rust
pub struct EventTimeline { /* ordered events */ }

impl EventTimeline {
    pub fn new() -> Self;
    /// Append an event. Maintains ordering by `timestamp_us` (events from a
    /// live capture usually arrive in order, but don't assume it — insert so
    /// the buffer stays sorted, ties keep insertion order).
    pub fn push(&mut self, event: NoteEvent);
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    /// Events in time order.
    pub fn events(&self) -> &[NoteEvent];
    /// Total duration = last timestamp (0 if empty).
    pub fn duration_us(&self) -> u64;
}
```

Implement `Default`. Exact naming/extra conveniences are at your discretion as
long as the behaviour above holds.

## Tests

- empty timeline: `is_empty()`, `len() == 0`, `duration_us() == 0`
- pushing in order preserves order; `events()` reflects it
- pushing out of order still yields a time-sorted `events()`
- `duration_us()` returns the max timestamp

## Scope boundaries (do NOT)

- Only `crates/core`. No third-party dependencies.
- Do not change existing public signatures in `events.rs`.

## Acceptance

- [ ] `cargo fmt --all --check`, `clippy --workspace --all-targets` (warnings =
      errors), `cargo test --workspace` all green
- [ ] PR against `main` from `claude/core-buffer`, `Closes #5`
