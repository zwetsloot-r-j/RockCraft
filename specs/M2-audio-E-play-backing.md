# M2-E — backing track on playback: load bundle, sync to highway

> Milestone: M2 — Audio · Issue: #31 · Suggested tier: sonnet
> Branch: `claude/play-backing`

## Goal

Play a recording bundle's backing track in sync with the falling-note highway:
load the bundle, start the audio at the right moment so it lines up with the
notes reaching the keyboard line, and mix it with the play-along synth.

## Context

- Depends on **Task C (#29)** (`RecordingMeta`, `backing_position_us`,
  `song_shift_us`), **Task A (#27)** (audio out), and **Task D (#30)** (bundles
  exist on disk).
- Crate: `crates/tui` (`app.rs` discovery + `play.rs`) and `crates/audio`
  (file playback / seek).
- `PlayScreen` shifts spans by `offset = (PRE_ROLL_US + LEAD_US) − first_us` and
  runs a playback clock. The backing track must follow `backing_position_us`
  from Task C — i.e. it begins when the clock reaches `offset` (audio not heard
  during the lead-in), at file position `audio_start_us`.

## What to do

1. **Discovery (`app.rs`):** teach `latest_recording()` (and the menu's load
   path) to find **bundle directories** `recordings/take-*/` and load via their
   `meta.json`. Keep loading legacy loose `take-*.mid` working if present.
2. **Load:** parse `meta.json` (Task C) → read `song.mid` (existing
   `smf_bytes_to_events`) → if `backing` is set, resolve the audio path inside
   the bundle dir.
3. **Sync playback (`play.rs` + audio):**
   - The Play clock and the existing `offset` are unchanged.
   - Begin backing playback when the clock first reaches `offset`, seeking the
     file to `audio_start_us` (use `backing_position_us(clock, offset,
     audio_start_us)`; `None` ⇒ not yet). For the MVP, starting the `Sink` at
     `clock == offset` is sufficient; expose a seek for `restart`/resync.
   - On `restart()`, stop and re-arm the backing track so it re-syncs from the
     top.
   - Mix backing audio with the play-along synth (Task A) on the same output.
   - A bundle with no backing track behaves exactly like today.
4. Surface in the status line that a backing track is playing.

## Tests

Audible/sync behaviour is local/manual. Cover the pure decision logic in CI:
- The "should the backing track be playing yet, and at what file position"
  decision is `backing_position_us` from Task C — add a tui-level test (or
  reuse C's) over a few clock values incl. before `offset` (None) and after.
- Bundle discovery picks the newest `take-*/` dir; falls back to loose `.mid`.

## Scope boundaries (do NOT)

- Do not modify `crates/core` (consume Task C).
- Do not re-derive the sync formula in tui — call `backing_position_us` /
  `song_shift_us` so Play visuals and audio share one source of truth.
- Do not block the audio thread.
- No scrubbing/seek UI beyond what `restart` needs.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` green
- [ ] Manual (local): playing a bundle recorded in Task D plays the backing
      track in time with the highway; `restart` re-syncs; midi-only bundles are
      silent backing-wise as before
- [ ] PR against `main` from `claude/play-backing`, `Closes #31`
