# M17-B — TUI practice loop

> Milestone: M17 · Issue: #294 · Suggested tier: opus
> Branch: `claude/tui-practice-loop`

## Goal

Bring M17-A's practice loop to the TUI play screen: mark bars, then loop
**count-in → demo → count-in → your turn → …** until stopped, with the same keys
and control-socket commands as the desktop app.

## Context

- M17-A (`specs/M17-A-play-practice-loop.md`) built the loop for Tauri and left
  the TUI returning `Unsupported`. This spec lifts that boundary.
- Reuse from `core`: `PracticeLoop` / `LoopPhase` / `LoopStep` / `Pass`
  (`crates/core/src/practice_loop.rs`) and `BarMap` (`crates/core/src/bars.rs`).
- Reference behaviour: `tauri-app/src-tauri/src/play.rs` (`seek_to`, `step_bar`,
  `set_loop`, `clear_loop`, `enter_phase`, `tick_loop`, `loop_clicks`,
  `autoplays`). The TUI screen is `crates/tui/src/play.rs` (`PlayScreen`); keys
  and host commands are in `crates/tui/src/app.rs`.
- **The TUI play screen has no scoring, no practice speed and no practice-hand
  mode.** So in the TUI there is no per-pass score, and the demo plays both
  hands. Those features are separate work.

## Behaviour

Keys (play screen), as in M17-A:

| Key | Effect |
|-----|--------|
| `←` / `→` | Pause, and jump to the start of the previous / next bar. Outside a loop only. |
| `[` | Loop start = the bar under the playhead (doesn't pause). |
| `]` | Loop end = the bar under the playhead (inclusive). |
| `l` | Start the loop over the marked bars / stop it. Nothing marked → the bar under the playhead; one mark → that bar. |

- Marks are bar indices from the screen's `BarMap` (the one the highway's bar
  lines use), swapped if reversed. Changing a mark while looping restarts the
  loop on the new range.
- **Count-in:** one bar (the loop's first bar length), clock from
  `start_us - count_in_us` (clamped at 0). A click per beat on the song bus
  (MIDI 76, accent on beat 1). No song notes, wait gate disarmed.
- **Demo:** the app plays the loop's notes on the song bus, whatever "hear the
  song" (`m`) is set to. Wait gate disarmed.
- **Your turn:** "hear the song" applies as usual; the wait gate is built from
  the loop's notes only and armed if wait mode is on.
- At `end_us`: jump to the count-in, the next pass is a demo, `pass += 1`. The
  wrap is checked before the wait-gate poll, so a note at exactly `end_us` never
  freezes the loop.
- Every jump: seek the clock, cut the song notes still sounding (and the click),
  reset the song triggers (pre-filling spans that end at or before the target),
  re-seek the whole-song gate outside a loop, and restart the backing at the new
  position.
- `is_finished` never fires while looping.
- **Stop (`l` while looping)**: leave the loop paused at `start_us`, with normal
  play restored; the marks stay. `play_clear_loop` with no loop running clears
  the marks.
- **Display:** the status line shows `Loop 5–8` when marked, and
  `Loop 5–8 · COUNT-IN / DEMO / YOUR TURN · pass 3` while running (1-based bars).
  The highway tints the rows inside the marked range, and dims notes outside it
  while the loop runs.

**Control socket.** `play_seek_bar`, `play_mark_loop`, `play_set_loop` and
`play_clear_loop` work in the TUI (an error off the play screen, like
`play_toggle_pause`). Each returns `{ "paused", "bar", "practice_loop" }`, where
`practice_loop` is `null` or `{ first_bar, last_bar, start_us, end_us, running,
phase, pass }` (Tauri's `LoopView` minus `last_pass`, which needs scoring).
`play_set_practice` stays `Unsupported`.

## Tests

Headless `PlayScreen` tests driven through `advance`:
- `set_loop` jumps to the count-in a bar before the loop, running.
- A full cycle: count-in → demo → count-in → your turn → wrap to the count-in
  with pass 2, at exact µs boundaries.
- The demo sounds loop notes with "hear the song" off; the count-in sounds none;
  your turn sounds none with it off. Song triggers re-fire on every pass.
- Wait mode freezes in your turn and never in the demo or count-in; a note at
  exactly `end_us` doesn't freeze the loop.
- `clear_loop` pauses at `start_us` with the whole-song gate back.
- `step_bar` pauses and lands on bar starts; refused while looping.
- `is_finished` stays false while looping past the song end.
- App: `[`, `]`, `l`, `←`/`→` reach the screen; the four host commands return
  the payload above on the play screen and fail off it.

## Scope boundaries (do NOT)

- No TUI scoring, practice speed or practice-hand mode.
- No changes to `core` or the Tauri app beyond what sharing requires.
- No new dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #294`
