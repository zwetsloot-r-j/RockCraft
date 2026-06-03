# M1 — scoring engine (core)

> Milestone: M1 — Note Highway · Issue: #TBD · Suggested tier: opus→sonnet
> Branch: `task/scoring-engine` (SEEDED — check it out, do not create a new branch)

## Goal

A **pure** scoring engine in `crates/core`: given the song's expected notes and
the player's actual note-ons, decide for each expected note whether it was hit,
and how well-timed. No I/O, no device, no rendering — a function over data,
fully unit-tested. This is the keystone the practice/feedback features build on.

## Context

- New module `crates/core/src/scoring.rs`, wired into `lib.rs`.
- Builds on existing types: `MidiNote`, `NoteEvent`, `NoteEventKind`
  (`crates/core/src/events.rs`). Reuse the buffer/timeline if helpful but it's
  not required.
- Read `CLAUDE.md` for the invariants — `core` stays pure and headless.

## What to do

Model the inputs as:
- **Expected notes**: a slice of `ExpectedNote { note: MidiNote, time_us: u64 }`
  — the moments the song wants each note struck (note-ons only; sustain/offs
  are out of scope for scoring).
- **Played notes**: the player's note-on events (a `&[NoteEvent]`; ignore
  note-offs and note-on velocity 0 for hit detection).

Produce a judgment per expected note. **The seeded acceptance tests in
`crates/core/src/scoring.rs` are the contract** — implement to satisfy them
exactly; do not weaken or delete them. They pin these behaviours:

- A `Timing` window classification relative to a configurable tolerance:
  `Perfect` (within ±perfect_us), `Early`/`Late` (within ±good_us but outside
  perfect), and a `Miss` when no matching played note falls within ±good_us.
- Matching is **by pitch within the timing window**, each played note consumed
  by at most one expected note (no double-counting), nearest-in-time preferred.
- `score(expected, played, config) -> ScoreReport` returning the per-note
  `NoteJudgment`s in expected order, plus simple roll-ups the tests check.
- An extra played note with no matching expected note is counted as an
  **extra/spurious** hit in the report (does not crash, does not match).

Exact type/field/method names are fixed by the seeded tests — follow them.

## Scope boundaries (do NOT)

- Only `crates/core`. No third-party dependencies. No I/O.
- Do not change existing public signatures in `events.rs`.
- Do not modify the seeded test module.

## Acceptance

- [ ] `cargo fmt --all --check`, `clippy --workspace --all-targets` (warnings =
      errors), `cargo test --workspace` all green
- [ ] The seeded `scoring` tests pass unmodified
- [ ] PR against `main` from `task/scoring-engine`, `Closes #<n>`
