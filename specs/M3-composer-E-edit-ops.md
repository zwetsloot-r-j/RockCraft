# M3-E — tui: note edit operations on keys

> Milestone: M3 — Composer · Issue: #53 · Suggested tier: sonnet
> Branch: `claude/m3-edit-ops`

## Goal

Bind the cursor in the Edit screen to real mutation: add, delete, lengthen,
shorten, and move notes, plus velocity adjust — turning the navigable grid
(#52) into an actual note editor. Each add/move auditions through the synth.

## Context

- Crate: `crates/tui`, extends `edit.rs` (#52) and the keymap table there.
- Mutates via the pure `Timeline` ops from #49
  (`insert/remove/set_start/resize/transpose/find_at`). Cursor time =
  `grid.us_of_step(step)`; default new-note length = one `grid.step_us()`.
- Audition: `EditScreen` should accept an optional `SynthHandle` (as Record/Play
  do) and play a brief note-on/off when a note is added/moved/grabbed. The synth
  is already owned by `Shell`; thread it in via the constructor.

## What to do

Extend the keymap (append to the table in #52's spec/comment):

| Key        | Action                                                        |
|------------|---------------------------------------------------------------|
| `a` / `i`  | add note at cursor (pitch, start=cursor, dur=1 step, vel=80)  |
| `x` / `d`  | delete note under cursor (`Timeline::find_at`)                |
| `]`        | lengthen note under cursor by one step                        |
| `[`        | shorten note under cursor by one step (min 1 step)            |
| `+` / `=`  | velocity +8 (clamp 127) on note under cursor                  |
| `-`        | velocity −8 (clamp 1) on note under cursor                    |
| `m`        | toggle "grab": subsequent `h/j/k/l` move the grabbed note     |
|            | (set_start / transpose) instead of the cursor; `m` again drops |

- Adding onto an occupied cell replaces/re-triggers per `find_at` semantics
  (document the chosen behaviour).
- In grab mode, moving the note also moves the cursor with it so they track.
- Provide accessors for tests: `note_count()`, `note_under_cursor()`.

## Tests (headless, via `ScriptedKeys` + `TestBackend` or direct `on_key`)

- `a` adds exactly one note at the cursor's pitch/step with one-step duration;
  `a` again on the same cell follows the documented replace/re-trigger rule.
- `x` on a cell with a note removes it; `x` on empty cell is a no-op.
- `]`/`[` change duration by one step; `[` never goes below one step.
- `+`/`-` clamp velocity at 127/1.
- Grab (`m`) + `l`/`k` moves the note (start/pitch change) and the cursor tracks it.

## Scope boundaries (do NOT)

- No chords here (that is #54); single notes only.
- No transport/playback (#59), no undo (#61) — though keep ops going
  through `Timeline` so #61 can wrap them later.
- Do not change `core` signatures; consume #49 as-is.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-edit-ops`, `Closes #53`
