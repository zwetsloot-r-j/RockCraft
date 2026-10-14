# M25-A — Edit QoL: tempo-aware paste that replaces overlaps, snap selection, count prefix

> Milestone: M25 — Editing quality of life · Issue: #343 · Suggested tier: sonnet
> Branch: `claude/m25-edit-qol`

## Goal

Make bulk editing of imported pieces faster: paste over old material without
cleaning up first, re-use a passage across tempo changes, snap a whole
selection to the grid in one key, and move the cursor many steps or bars at
once.

## Context

- Selection and clipboard live in `Composer` (`crates/core/src/composer.rs`:
  `selection_bounds`, `selection_ids`, `yank_selection`, `paste_clipboard`,
  `delete_selection`). `D` (`delete_selection`) already deletes every note
  whose onset is inside the selection; this spec leaves it as it is.
- Tempo-map helpers: `bar_start_us`, `bar_dur_us`, `bar_at_us`, `pos_snap`,
  `pos_us_of_step`, `pos_step_index`. They fall back to the uniform grid when
  the map is empty.
- `quantize_region` already snaps by an explicit µs range and step. The new
  action works on the selection at the live subdivision instead.
- Keymaps: `crates/tui/src/edit.rs` (`on_key`) and
  `tauri-app/src/screens/edit/keymap.ts`. `q` is free in both edit screens.
  Digits are free in the desktop normal keymap except `0` (lowest pitch).

## What to do

### 1. Tempo-aware clipboard (core)

- Store clipboard notes as `ClipNote { pitch (relative), start, end, velocity,
  hand }`, where `start`/`end` are **bar positions** relative to the
  selection's start: `bar_index × CLIP_TICKS_PER_BAR + fraction of that bar`.
  `CLIP_TICKS_PER_BAR = 2^40`, far finer than a µs, so positions round-trip
  exactly. Times before the first downbeat extend bar 0 backwards (negative
  positions).
- Paste converts each note back through the bar it lands in. At the same
  tempo this is an exact copy. At a different tempo, onsets and lengths follow
  the new bar lines (a quarter stays a quarter).

### 2. Paste replaces overlapped notes (core)

Before inserting, remove every existing note of the **same pitch** whose
interval overlaps a pasted note's (touching end-to-start doesn't count). The
removal and the paste are one undo step. Other pitches are never touched.

### 3. `Action::SnapNotes` / `snap_notes` (core)

The targets are the notes in the selection, or the note under the cursor when
there is no selection. Snap each onset and end to the nearest grid line at the
live subdivision (`pos_snap`, tempo-map aware), with:

- a minimum length of one step;
- each end clamped to the next same-pitch onset, so snapping never creates a
  same-key overlap.

One undo checkpoint, and none when nothing moves. The selection stays active.
Add it to the action catalog and the parity lists.

### 4. Keys

- `q` → `snap_notes` in both frontends, with a help row and the TUI help line.
- **Count prefix (desktop):** `1`–`9` start a count and further digits extend
  it, up to 999. `0` extends a count but keeps its own binding when none is
  pending. The next key consumes the count:
  - the cursor moves (`h/j/k/l` / arrows, `H/L`, `w/b/J/K`) run that many
    times;
  - any other key runs once and drops the count;
  - Esc cancels the count;
  - a bare modifier keydown (Shift for `H`/`L`) keeps it.

  The status bar shows the pending count. The pure logic is in
  `screens/edit/count.ts`.

## Tests

- Core:
  - same-tempo paste is µs-exact (odd times);
  - a quarter + eighth copied from a 2 s bar into a 1 s bar paste as 250 ms /
    125 ms;
  - paste removes an overlapping same-pitch note, keeps a touching one and
    another pitch, and one undo restores it;
  - `snap_notes` on a selection (onset/end/min length, untouched outside,
    selection kept, one undo);
  - `snap_notes` without a selection clamps at the next same-pitch onset.
- Frontend: `count.test.ts` (start/extend/cap/bare-0/non-digits,
  `repeatsFor`, `keepsCount`).

## Scope boundaries (do NOT)

- No count prefix in the TUI (its number row is the mock piano while
  recording).
- Don't change `delete_selection` or `quantize_region`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; frontend `tsc`/vitest/build green
- [ ] PR opened against `main` from the branch above, `Closes #343`
