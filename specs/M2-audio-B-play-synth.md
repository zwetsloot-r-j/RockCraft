# M2-B — synth in Play mode: play-along + hear-the-song

> Milestone: M2 — Audio · Issue: #28 · Suggested tier: sonnet
> Branch: `claude/audio-play-synth`

## Goal

Bring the Task A synth into Play mode. Your live keys sound as you play along
with the falling-note highway, and — via a toggle — the song's own recorded
notes are synthesized as they reach the keyboard line, so you can *hear* the
piece you're learning.

## Context

- Depends on **Task A (#27)**: `AudioOut` / `SynthHandle` exist in
  `crates/audio`. Reuse them; do not build a second synth.
- Crate: `crates/tui` — `play.rs` (`PlayScreen`) and `app.rs` (routing).
- `PlayScreen` already loads a `.mid` into `NoteSpan`s shifted by
  `PRE_ROLL_US + LEAD_US` and runs a playback clock (`started: Instant`). It
  knows the current clock position and projects spans onto the highway.
- `PlayScreen::ingest(ev)` already receives the player's live `NoteEvent`s.

## What to do

1. Give `PlayScreen` access to a `SynthHandle` (pass it in via the constructor /
   from the `Shell`, like Record in Task A).
2. **Play-along:** in `PlayScreen::ingest`, forward the live `NoteEvent` to the
   synth (`handle.apply(&ev)`) so the player hears themselves.
3. **Hear-the-song toggle:** add a boolean (default OFF) toggled by a key
   (suggest `m` for "music"; surface it in the status line). When ON, as the
   playback clock crosses each recorded note's keyboard-line moment, emit
   `note_on` (at the note's start) and `note_off` (at its end) to the synth.
   - Drive this off the **playback clock**, not frame rate (CLAUDE.md: decouple
     audio timing from rendering). Track which spans have already been
     started/stopped so each fires exactly once per pass.
   - On `restart()` and on toggling OFF, send `all_off()` and reset the
     fired-state bookkeeping so a restart re-triggers cleanly.
4. Update the Play status line to show the toggle state and its key.

## Tests

Audible behaviour is local/manual. Add pure unit tests for the trigger
bookkeeping where practical (no device):
- Given a small set of spans and a sequence of clock positions, the code emits
  each note's on exactly once when the clock passes its start and off once when
  it passes its end, and `restart` clears the state so they fire again.
(Factor the "which spans fire at clock C" decision into a pure helper so it is
testable without rendering or hardware.)

## Scope boundaries (do NOT)

- Do not modify `crates/core`, `crates/midi`, or `crates/audio` (consume Task A's
  public API as-is; if A's API needs a small addition, note it but prefer not).
- Do not add backing-track audio here — Tasks D/E.
- Hear-the-song defaults OFF so play-along feel is unchanged unless asked for.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` green
- [ ] Manual (local): live keys sound during Play; toggling music plays the
      recorded notes in time with the highway; restart re-triggers correctly
- [ ] PR against `main` from `claude/audio-play-synth`, `Closes #28`
