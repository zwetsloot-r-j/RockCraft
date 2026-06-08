# M5-D — tui (Edit): audible backing track in the composer

> Milestone: M5 — Play-along & Backing Sync · Issue: #109 · Suggested tier: sonnet
> Branch: `claude/m5-edit-backing`

## Goal

Make the **actual music audible while editing** so notes can be placed at the
right time. Attach a backing track to the `EditScreen` and play / seek / pause
it **in lockstep with the composer transport** (`is_playing()`, `playhead_us()`,
stop, loop).

## Context

- Crate: `crates/tui`, files `edit.rs` and `app.rs` (the wiring), plus the run
  loop in `app.rs`.
- Today `EditScreen` hardcodes `backing: None` in `save_bundle`’s meta and never
  receives a backing path; only `RecordScreen::with_backing` gets one. The
  `Shell` already holds `backing_path: Option<PathBuf>` and a chosen track from
  the "Choose backing track" picker.
- The composer transport is pure and time-injected (`advance(dt_us)`,
  `is_playing()`, `playhead_us()`, `start_play`, `stop_play`, loop from
  `M3-P`). The Play screen’s `backing_target_us` + `tick_backing` (using
  `core::backing_position_us`) is the reference pattern — reuse the approach,
  don’t fork the math.
- Depends on **M5-B (#107)** for `BackingHandle::pause/resume/seek`.
- `loc:local`: audible sync verified on the host; the position math stays in the
  existing core seam.

## What to do

1. **Plumb the path.** Pass `Shell.backing_path` into the `EditScreen` on both
   menu paths in `app.rs::menu_activate` — "Compose (new)" (`activate_edit`) and
   "Edit last recording". Add an `EditScreen::with_backing(path,
   audio_start_us)` (default `audio_start_us = 0`; M5-E makes it adjustable).
   When loading an existing bundle whose `meta.json` declares a backing, prefer
   that (path + `audio_start_us`) over the session default.

2. **Sync to the transport.** Hold an `Option<BackingHandle>`. Tick it from the
   run loop right after `edit.tick_audition()`:
   - On transport start (`PlayFromStart` / `TogglePlayCursor` → playing): start
     the backing seeked to `backing_position_us(playhead_us, shift, audio_start)`
     (compute `shift` consistently with how Edit positions song time; if Edit
     has no pre-roll, `shift = 0` and the file position is
     `playhead_us + audio_start_us`).
   - While playing: it free-runs with the sink; on a **seek** (`SetPlayhead`,
     cursor jumps that move the playhead) or **loop wrap**, re-`seek()` the
     handle to match.
   - On **stop**: `pause()` (keep the stream; cheap to resume).
   - On leaving the editor: stop/drop the handle (mirror `RecordScreen`).

3. **Persist.** When `save_bundle` writes meta, carry the backing through:
   `RecordingMeta.backing = Some(BackingTrack { file, audio_start_us })` when a
   track is attached (copy/reference the file into the bundle the same way
   Record bundles do, or store the relative filename if it already lives under
   `backing/`). MIDI-only saves keep `backing: None`.

## Tests

- **Headless:** with a backing attached, driving the transport via the
  `advance`/playhead seam produces backing **target positions** equal to
  `backing_position_us(playhead, shift, audio_start)` at sampled playhead values
  (assert the computed seek targets, not real audio). A seek/loop-wrap yields the
  re-seek target. Stop marks the handle paused. Save round-trips the backing into
  `meta.json` and `from_json` reads it back.
- **Host (`loc:local`, note in PR):** choose a track, open the editor, press
  play → music + note auditions sound together and stay aligned; stop pauses the
  music; replay re-syncs from the playhead.

## Scope boundaries (do NOT)

- Do not add wait-mode to the editor (Play-only; M5-C).
- Do not add the offset-nudge UI here — that is M5-E (#110); this task only
  plumbs and persists a fixed `audio_start_us` (default 0 / from meta).
- Do not change `backing_position_us`, the composer transport, or the Play
  screen.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Host verification of audible, aligned editing playback noted in the PR
- [ ] PR against `main` from `claude/m5-edit-backing`, `Closes #109`
