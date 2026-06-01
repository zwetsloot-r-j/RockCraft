# M1 — note-by-note "wait mode" core (core)

> Milestone: M1 — Note Highway · Issue: #TBD · Suggested tier: sonnet
> Branch: `task/wait-mode` (SEEDED — check it out, do not create a new branch)

## Goal

The **pure logic** for note-by-note practice: playback pauses at each step until
the player holds the right key(s), then advances. The TUI will drive this (feed
it held notes each frame, ask "may I advance?"), but the state machine itself is
pure and headless — no device, no timing, just "are the required keys down?".

## Context

- New module `crates/core/src/wait.rs`, wired into `lib.rs`.
- Builds on `MidiNote` / `NoteEvent` (`crates/core/src/events.rs`).
- A "step" is the set of notes the song wants struck at one instant (a single
  note, or all notes of a chord). The song is an ordered list of steps.
- Read `CLAUDE.md` for the pure-`core` invariants.

## What to do

Implement a `WaitTracker` that holds the ordered steps and a current position,
and advances when the current step's required notes are all currently held.
**The seeded tests in `crates/core/src/wait.rs` are the contract** — implement
to satisfy them exactly; do not modify them. They pin:

- Building steps from expected (pitch, time) notes: notes at the same time
  group into one step; steps are time-ordered.
- `is_satisfied(held)`: the current step is satisfied when every required pitch
  is present in the held set (extra held notes are allowed/ignored).
- `update(held)` advances past every consecutive satisfied step and returns
  whether the position moved; `current()` is the active step (or `None` at end);
  `is_complete()` when all steps are done.
- A chord step requires **all** its notes held simultaneously.
- Re-pressing / extra notes don't skip steps; you can't advance without
  satisfying the current step.

Exact type/field/method names are fixed by the seeded tests.

## Scope boundaries (do NOT)

- Only `crates/core`. No third-party dependencies. No I/O, no timing/clock.
- Do not change existing public signatures in `events.rs`.
- Do not modify the seeded test module.

## Acceptance

- [ ] `cargo fmt --all --check`, `clippy --workspace --all-targets` (warnings =
      errors), `cargo test --workspace` all green
- [ ] The seeded `wait` tests pass unmodified
- [ ] PR against `main` from `task/wait-mode`, `Closes #<n>`
