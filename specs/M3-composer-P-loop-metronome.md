# M3-P — tui: loop region + metronome / count-in

> Milestone: M3 — Composer · Issue: #64 · Suggested tier: sonnet
> Branch: `claude/m3-loop-metronome`

## Goal

Make practising and recording a part comfortable: loop a region during playback,
hear a metronome on the beat, and get a count-in before live recording so the
first notes land in time.

## Context

- Crate: `crates/tui`, extends the transport (#59) and live record (#57);
  uses `core::Grid` (#50) for beat positions and the `SynthHandle` for clicks.
- Loop + click timing is clock-driven (off the playhead µs), not frame-rate —
  keep the `advance(dt_us)` seam from #59 so it stays headless-testable.

## What to do

- **Loop:** loop points `loop_start_us`/`loop_end_us` (default = current
  selection from #63, else the bar under the cursor). A key toggles looping;
  when the playhead reaches `loop_end_us` it wraps to `loop_start_us`.
- **Metronome:** when armed, fire a synth click at each beat boundary
  (`grid.quarter_us()` cadence within the bar; optionally an accent on beat 1).
  A key toggles the metronome.
- **Count-in:** before LiveRecord (#57), play N bars (default 1) of clicks
  with no recording, then start recording at bar 0 of the part.
- Expose test seams: `is_looping()`, `loop_bounds()`, and a way to assert clicks
  fired at the expected beats given `advance()` steps.

## Tests (headless, via the playhead `advance` seam)

- With a loop set, advancing past `loop_end_us` wraps the playhead to
  `loop_start_us` (no overshoot beyond one step).
- Metronome fires exactly once per beat over a known span at 120 BPM 4/4
  (count the click triggers).
- Count-in delays the first recorded note by N bars.

## Scope boundaries (do NOT)

- No audio-file metronome samples; reuse the synth for clicks.
- No tempo automation / ramps; constant BPM from the grid.
- Depends on #59 (transport) and #57 (record); stub their seams if landing
  first, but do not duplicate their logic.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-loop-metronome`, `Closes #64`
