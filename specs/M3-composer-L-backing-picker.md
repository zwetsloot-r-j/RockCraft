# M3-L — tui: interactive backing-track picker

> Milestone: M3 — Composer · Issue: #60 · Suggested tier: sonnet
> Branch: `claude/m3-backing-picker`

## Goal

Choose (and swap) the backing audio track from inside the app instead of only
via a CLI option — a small in-app file browser that lists audio files and
attaches the chosen one to the Record/Edit session.

## Context

- Crate: `crates/tui`. Today the backing path is threaded from `main.rs` into
  `Shell { backing_path }` and on to `RecordScreen::with_backing` (`app.rs`,
  `record.rs`); playback is `rockcraft_audio::play_file`. This task replaces the
  "passed in once" model with an interactive selection.
- Keep the directory-listing/filtering logic in a small **pure helper** so it's
  unit-testable in CI (no real audio device needed to list files).

## What to do

- A pure helper (e.g. `backing::list_audio_files(dir) -> Vec<PathBuf>`) that
  returns audio files (`.mp3 .wav .ogg .flac`, case-insensitive) in a directory,
  sorted. Testable against a temp dir.
- A picker screen/overlay: a `List` of files (start in a sensible dir — cwd or a
  `backing/` folder, configurable), `j/k`/arrows to move, `Enter` to select,
  `Esc` to cancel. On select, set the session's backing path and (re)start
  playback via `play_file`; on cancel, leave the current selection unchanged.
- Reachable from the menu ("Choose backing track") and/or a key in the Record/
  Edit screen; selecting then entering Record/Edit uses the chosen track. The
  existing `--backing`/CLI path stays as an initial default.

## Tests

- `list_audio_files` returns only the audio extensions present, sorted, and skips
  non-audio files and subdirectories (temp-dir fixture).
- Picker navigation with `ScriptedKeys`: move + `Enter` reports the selected path;
  `Esc` reports cancellation and leaves the prior selection.

## Scope boundaries (do NOT)

- No recursive tree browser; single-directory listing is enough for v1 (note the
  limitation).
- Do not change `audio` crate signatures; consume `play_file`.
- No new third-party deps (use `std::fs`).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-backing-picker`, `Closes #60`
