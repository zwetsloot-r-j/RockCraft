# DX — scripted input source + headless TUI tests

> Milestone: DX / dev-tooling · Issue: #40 · Suggested tier: sonnet
> Branch: `claude/mock-input-tests`

## Goal

Make whole Record/Play sessions verifiable in CI without a terminal or piano:
a `ScriptedSource` that replays a fixed `NoteEvent` timeline against a fake
clock, driven against ratatui's `TestBackend`. This turns "does the UI react
correctly to input?" into a deterministic `cargo test`.

## Context

- Depends on **#39**: the `NoteSource` trait (`crates/midi`) and
  `Box<dyn NoteSource>` in `Shell` must already exist. Read
  `specs/DX-mock-input-A-source.md` first.
- ratatui ships `ratatui::backend::TestBackend`, which renders into an
  inspectable `Buffer` with no real terminal — use it; no new deps.
- Read `CLAUDE.md`: scoring/timing run off `NoteEvent::timestamp_us`, never off
  frame rate — so a scripted timeline with explicit timestamps reproduces real
  judgments.

## What to do

**1. `ScriptedSource`** (`crates/midi/src/source.rs` or `mock.rs`):

```rust
pub struct ScriptedSource { /* sorted events + a fake clock cursor */ }

impl ScriptedSource {
    pub fn new(events: Vec<NoteEvent>) -> Self;
    /// Advance the fake clock by `dt_us`; subsequent `events()` returns events
    /// whose timestamp_us is now due.
    pub fn advance(&mut self, dt_us: u64);
}

impl NoteSource for ScriptedSource {
    fn events(&mut self) -> Vec<NoteEvent>;  // drains events up to the cursor
    fn port_name(&self) -> &str;             // e.g. "scripted"
}
```

Events are released in timestamp order as the cursor advances; nothing is
emitted ahead of its time. This keeps the source's contract identical to live
input while being fully controllable.

**2. Headless TUI integration tests** (`crates/tui/tests/` or a `#[cfg(test)]`
module): factor the run loop just enough that a test can step it — feed a
`ScriptedSource`, advance the clock, pump frames into a `TestBackend`, and
assert on resulting state. Keep refactors minimal and behaviour-preserving.

Cover at least:
- **Play/scoring**: a scripted timeline that hits the expected notes on time
  produces the expected `ScoreReport` roll-ups; an off-time/missing timeline
  produces the expected misses. (Asserts the input→scoring path, not pixels.)
- **Record**: scripted note-ons land in the recording timeline / event log in
  order with their timestamps.
- A `TestBackend` smoke render of Menu and one screen produces a non-empty,
  non-panicking buffer.

## Tests

The integration tests above ARE the deliverable. Also unit-test
`ScriptedSource`: events emit only after `advance` passes their timestamp, in
order, and the queue empties exactly once.

## Scope boundaries (do NOT)

- Depends on #39; do not re-define the trait.
- `crates/midi` (`ScriptedSource`) and `crates/tui` (tests + minimal test
  seams) only. No `core` changes.
- No new third-party deps (`TestBackend` is part of ratatui).
- Do not weaken or delete existing tests.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/mock-input-tests`, `Closes #40`
