# M3-J — tui: on-the-fly snap / grid control

> Milestone: M3 — Composer · Issue: #58 · Suggested tier: cheap
> Branch: `claude/m3-snap-control`

## Goal

Let the user change the cursor/snap resolution mid-edit — coarse 1/4-note moves
for blocking out a part, finer 1/8 / 1/16 / 1/32 (and triplets) for detail —
without leaving the editor. The cursor step size follows the active subdivision
immediately.

## Context

- Crate: `crates/tui`, extends `edit.rs` (#52) keymap. Pure subdivision logic
  lives in `core::grid::Subdivision::{finer, coarser, ALL, label}` (#50).
- The cursor already steps by `grid.step_us()`; changing `grid.subdivision`
  changes the step size for all subsequent navigation and new-note durations.

## What to do

Append to the #52 keymap:

| Key   | Action                                                         |
|-------|----------------------------------------------------------------|
| `>`   | finer snap (`Subdivision::finer`)                              |
| `<`   | coarser snap (`Subdivision::coarser`)                          |

- On change, keep the cursor on the nearest grid line of the new subdivision
  (re-snap `cursor` µs via `grid.snap` and recompute `cursor.step`), so the
  cursor doesn't jump to a musically meaningless spot.
- Show the current snap `label()` ("1/4", "1/8", …, "1/16T") in the status line.
- Expose `current_subdivision()` for tests.

## Tests (headless)

- `>` walks the subdivision finer through `Subdivision::ALL` and saturates at the
  finest; `<` walks coarser and saturates at 1/4.
- After changing snap, the cursor's µs position stays on a valid grid line of the
  new subdivision (`grid.snap(cursor_us) == cursor_us`).
- Status line contains the active snap label.

## Scope boundaries (do NOT)

- Do not add BPM/time-signature editing here (a later issue); only subdivision.
- Do not change `core::grid` signatures; consume #50.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-snap-control`, `Closes #58`
