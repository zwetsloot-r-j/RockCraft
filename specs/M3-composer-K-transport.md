# M3-K — tui: editor transport / playback (region or whole song)

> Milestone: M3 — Composer · Issue: #59 · Suggested tier: sonnet
> Branch: `claude/m3-transport`

## Goal

Hear what you're editing: play the part under edit (from the cursor, or a
selected region) or the whole song from the start, with a moving playhead on the
highway and notes auditioned through the synth. Stop returns to editing.

## Context

- Crate: `crates/tui`, extends `edit.rs`. Audition uses the `SynthHandle` already
  owned by `Shell` (see `app.rs` run loop, `play.rs::tick_song_synth`).
- The playback math already exists: `highway::project` for the playhead/notes and
  `PlayScreen`'s clock-driven trigger approach. Reuse that pattern; do not couple
  rendering to frame rate (`CLAUDE.md`) — drive the playhead off a wall clock and
  trigger note-ons whose `start_us` the playhead has passed.

## What to do

- Add a transport to `EditScreen`: `Stopped` / `Playing { from_us, started_at }`.
  - `Space` (when not in chord/grab mode) = play/stop.
  - `p` = play whole song from 0; play/stop toggles play-from-cursor.
- While playing: advance a playhead `now_us`, fire `synth.apply(note_on)` for
  notes entering and `note_off` after their duration (a `tick_audition()` called
  each frame from the run loop, like `tick_song_synth`); render the playhead row
  via `project`. On stop / leave, `synth.all_off()`.
- Expose `is_playing()` and `playhead_us()` for tests; allow a manual
  `advance(dt_us)` seam so playback is testable without wall-clock timing
  (mirror `ScriptedSource::advance`).

## Tests (headless, via the advance seam)

- Play-from-cursor sets the playhead to the cursor µs; `advance` moves it forward.
- A note whose `start_us` the playhead passes is triggered exactly once (track
  triggered ids); its off fires after `dur_us`.
- Stop resets to `Stopped` and reports not playing; play-whole starts at 0.

## Scope boundaries (do NOT)

- No loop region or metronome (that is #64); single linear pass only.
- No backing-track playback here (audio backing is its own concern / #60).
- Do not change `core` or `audio` signatures; consume `SynthHandle`.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-transport`, `Closes #59`
