# M1 — performance statistics summary (core)

> Milestone: M1 — Note Highway · Issue: #TBD · Suggested tier: sonnet
> Branch: `task/stats-summary` (will be SEEDED after the scoring engine lands)

## Goal

A **pure** summary over the scoring engine's output: turn a `ScoreReport` (and
its per-note `NoteJudgment`s) into player-facing statistics — accuracy, counts,
and a timing breakdown — so the UI can show "how did I do?". No I/O.

## Dependency

**Blocked on the scoring engine** (`crates/core/src/scoring.rs`, the
`ScoreReport` / `NoteJudgment` / `Timing` types). This task's seeded acceptance
tests will be added to a `task/stats-summary` branch once scoring has merged to
`main`, so the types exist to compile against. Until then this is a spec only.

## What to do (to be pinned by seeded tests)

A `Summary` built from a `&ScoreReport`, exposing at least:
- `accuracy()` — hits / (hits + misses) as an `f32` in 0.0..=1.0 (0.0 when no
  expected notes).
- counts: total expected, hits, misses, extras.
- timing breakdown: how many hits were `Perfect` / `Early` / `Late`.
- `mean_abs_error_us()` — average absolute timing error over hits (0 if none).

Exact names/signatures will be fixed by the seeded tests (added post-merge).

## Scope boundaries (do NOT)

- Only `crates/core`. No third-party dependencies. No I/O.
- Pure aggregation — do not re-implement scoring; consume its report.

## Acceptance

- [ ] `cargo fmt --all --check`, `clippy --workspace --all-targets`, `cargo test
      --workspace` all green
- [ ] The seeded `stats` tests pass unmodified
- [ ] PR against `main` from `task/stats-summary`, `Closes #<n>`
