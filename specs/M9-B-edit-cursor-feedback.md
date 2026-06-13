# M9-B — Visible cursor / selected-timeslot feedback in the edit grid

> Milestone: M9 — Tauri UX consolidation · Issue: #201 · Suggested tier: sonnet
> Branch: `claude/m9-edit-cursor-feedback`

## Goal

In edit mode there is currently **no clear on-grid indication of which timeslot
is selected** — the user can't tell where a note will be added, deleted, or
resized. Render an unmistakable cursor cell (pitch × step) on the grid so every
edit has an obvious target.

## Context

- The `ComposerSnapshot` already carries the cursor: `cursor.pitch` (MIDI note)
  and `cursor.step` (timeline step). `StatusBar.tsx` shows the derived `bar:beat`
  and note name, but the **grid itself** has no strong cursor marker.
- **Tauri:** the grid is drawn in `tauri-app/src/screens/edit/EditCanvas.ts`,
  with axis math in `screens/edit/viewport.ts` (`yOf(us)`, `stepUs`, pitch→x).
  Orientation after M7-tauri-M (#198): time → y (earlier at bottom), pitch → x.
- **TUI:** the equivalent grid render is in `crates/tui/src/edit.rs` (`draw_grid`
  / cursor cell). Keep the two frontends visually consistent.

## What to do

- Draw a clearly visible **cursor cell** at `(cursor.pitch, cursor.step)`: a
  filled/outlined highlight on the exact grid cell, distinct from notes and from
  the playhead. Add light **crosshair guides** (the cursor's pitch column and
  step row tinted) so the selected timeslot reads at a glance even on a sparse
  grid.
- Ensure the cursor stays visible when it moves off-screen by keeping it within
  the scrolled viewport (the viewport already scrolls; confirm the cursor is
  always in view after `cursor_*` actions).
- When a **selection** (`snapshot.selection`) is active, render its region as a
  band and keep the cursor highlight on top, so "what's selected" and "where the
  cursor is" are both legible.
- Mirror the change in **both** Tauri (`EditCanvas.ts`) and TUI (`edit.rs`).
- Keep the status-bar `bar`/`♪` fields; the grid marker complements them.

## Tests

- Tauri: a render/unit test (or canvas-draw assertion at the `viewport.ts` level)
  that the cursor cell maps to the expected x/y for a given `(pitch, step)` and
  that moving the cursor moves the marker.
- TUI: a `ratatui` buffer test asserting the cursor cell is styled distinctly at
  the snapshot's `(pitch, step)`.

## Scope boundaries (do NOT)

- Do **not** change `core`, the `Action` set, or the snapshot shape — this is a
  render-only change consuming the existing `cursor` fields.
- Do **not** change grid orientation or scrolling behaviour beyond keeping the
  cursor in view.
- No new keybindings here (transport/loop discoverability is M9-C).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] The selected timeslot is obvious on the grid in both Tauri and TUI;
      moving the cursor visibly moves the marker
- [ ] PR opened against `main` from the branch above, `Closes #201`
