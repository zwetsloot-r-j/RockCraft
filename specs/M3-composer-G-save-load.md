# M3-G — tui: save composition + load-for-edit + new empty composition

> Milestone: M3 — Composer · Issue: #55 · Suggested tier: sonnet
> Branch: `claude/m3-save-load`

## Goal

Make the editor a round trip: start a blank composition, or open the latest
recorded/saved take into the editor, edit it, and save it back as a bundle —
reusing the exact `recordings/take-*/` format the Record screen already writes.

## Context

- Crate: `crates/tui`. Uses `Timeline` (#49), `EditScreen` (#52),
  `events_to_smf_bytes`/`smf_bytes_to_events` (`midi/file.rs`), and
  `RecordingMeta` + the bundle layout in `record.rs::save`.
- `app.rs` menu (`MENU_ITEMS`) and `latest_recording()` already exist — extend,
  don't duplicate. `main.rs` parses flags (e.g. `--mock`); add an opt-in entry.

## What to do

- **Save:** `EditScreen::save()` mirrors `RecordScreen::save()` — write
  `events_to_smf_bytes(self.timeline.to_events())` to `recordings/take-<stamp>/
  song.mid` and a `meta.json` (`RecordingMeta`). Return the bundle path; show it
  in the status line. (Grid/key persistence is #56 — leave a hook.)
- **Load for edit:** add a menu entry "Edit last recording" that loads
  `latest_recording()` bytes → `smf_bytes_to_events` → `Timeline::from_events` →
  `EditScreen::from_timeline(..)`. Reuse `latest_recording()`.
- **New composition:** add a menu entry "Compose (new)" → `EditScreen::new()`
  (empty timeline). Wire both into the `MENU_ITEMS`/`menu_activate` match.
- **Flag:** `main.rs` may accept `--edit` to boot straight into a new editor.

Bind `s` (save) in the Edit screen's `on_key`, matching the Record screen's
convention, and release synth notes on leave (as Record/Play do).

## Tests

- `EditScreen::save()` on a known timeline writes a bundle whose `song.mid`
  reloads (`smf_bytes_to_events` → `Timeline::from_events`) to an equal timeline
  (round-trip; use a temp dir).
- Menu navigation to "Compose (new)" enters `Screen::Edit` with an empty timeline
  (assert via `screen_name()` and `note_count()`), driven by `ScriptedKeys`.
- "Edit last recording" with a seeded bundle enters the editor pre-populated.

## Scope boundaries (do NOT)

- Do not change the bundle format or `RecordingMeta` fields here (that is #56).
- Do not change `events_to_smf_bytes` timing semantics.
- No interactive file browser here (that is #60); reuse `latest_recording()`.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-save-load`, `Closes #55`
