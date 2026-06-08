# M5-E — tui (Edit): backing alignment (nudge offset, persist)

> Milestone: M5 — Play-along & Backing Sync · Issue: #110 · Suggested tier: sonnet
> Branch: `claude/m5-edit-backing-align`

## Goal

Let the editor **align an arbitrary music track to song time**: nudge the
backing earlier/later while it plays, and **persist the offset** as
`audio_start_us` in `meta.json` so the alignment survives save/load and feeds
the Play screen.

## Context

- Crate: `crates/tui`, file `edit.rs` (+ a control action in `core::action.rs`).
- Builds directly on **M5-D (#109)**, which attaches a backing track to the
  editor and seeks it via `BackingHandle::seek` (M5-B) using
  `core::backing_position_us(playhead, shift, audio_start_us)`.
- The bundle model already has `BackingTrack.audio_start_us`
  (`crates/core/src/song.rs`); this task just makes it editable and live.
- A picked track rarely starts exactly at song time 0, so to place notes on the
  right beats the audio must slide under the highway, with the offset retained.

## What to do

1. **Nudge actions.** Add to the editor keymap and the action model:
   - fine nudge ±, e.g. ±10 ms; coarse nudge ±, e.g. ±250 ms (pick keys that
     don’t collide with the existing editor map — document them in the help
     overlay).
   - Core action variants (mirror M4-A): `Action::NudgeBackingOffset {
     delta_us: i64 }` (`name` = `"nudge_backing_offset"`), with parity tests.
     `audio_start_us` is clamped at 0 (cannot go negative).
2. **Live apply.** Adjusting the offset updates the in-memory `audio_start_us`
   and, while playing, immediately re-`seek()`s the `BackingHandle` to the new
   `backing_position_us(playhead, shift, audio_start_us)` so the shift is
   audible at once. Reflect the current offset in the snapshot/status line.
3. **Persist + load.** `save_bundle` writes the current `audio_start_us` into
   `RecordingMeta.backing`. The "Edit last recording" path loads it back so
   reopening a bundle restores the alignment; the Play screen already consumes
   `audio_start_us`, so it inherits the alignment for free.

## Tests

- **Headless:** `nudge_backing_offset` adjusts `audio_start_us` by the delta and
  clamps at 0; the resulting backing seek target equals
  `backing_position_us(playhead, shift, audio_start_us)` for the new offset;
  action name/param round-trips through `action_from_name`. Save→load preserves
  `audio_start_us` through `meta.json`.
- **Host (`loc:local`, note in PR):** nudging audibly slides the music vs the
  highway; the value persists across save/reopen and matches in Play.

## Scope boundaries (do NOT)

- Do not change the sync formula (`backing_position_us`) or the transport.
- Do not add tempo/stretch or per-section offsets — a single whole-track
  `audio_start_us`, as the model already defines.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Host verification of audible alignment + persistence noted in the PR
- [ ] PR against `main` from `claude/m5-edit-backing-align`, `Closes #110`
