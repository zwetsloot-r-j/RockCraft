# M7-tauri-M-edit-vertical-orientation — Reorient the edit grid to vertical (highway-aligned)

> Milestone: M7 · Issue: #189 · Suggested tier: opus
> Branch: `claude/tauri-edit-vertical`
> Depends on: #164 (edit grid), #165 (edit overlays)

## Goal

Flip the composer edit grid from a horizontal piano-roll to a **vertical** one
that matches the Record and Play screens: **time runs bottom → top** (the start
of the song is at the bottom, later time is higher), and **pitch runs left →
right, lowest key → highest key**. Advancing in time moves the cursor **up**
(`k` / `ArrowUp`). This is almost entirely a **rendering** change — the
`core::Action` bindings already express this mental model.

## Context

- `screens/edit/viewport.ts` — currently maps **time → x** (`xOf`, `pxPerUs`,
  `originUs`) and **pitch → y** (`yOf`, `laneH`, `pitchLo/Hi`). This is the axis
  swap's core.
- `screens/edit/EditCanvas.ts` — `draw()` calls `drawLanes` (horizontal pitch
  lanes), `drawGridlines` (vertical per-step lines), `drawNotes`,
  `drawSelection`, `drawChordPreview`, `drawCursor`, `drawPlayhead`,
  `drawLaneLabels`. All consume the viewport mapping.
- `screens/edit/keymap.ts` — **already correct** for the target orientation:
  `k`/`ArrowUp` → `cursor_right` (step **+1**, later in time); `j`/`ArrowDown`
  → `cursor_left` (earlier); `h`/`ArrowLeft` → `cursor_down` (pitch −1);
  `l`/`ArrowRight` → `cursor_up` (pitch +1). The action names stay identical;
  only the **comments** describing the visual layout need updating to "vertical
  time, horizontal pitch".
- Visual language must stay consistent with `screens/highway/` (Spectrum
  palette, `#0f1016`, IBM Plex Mono) — the highway is the reference for "time is
  vertical".

## What to do

### 1. `viewport.ts` — swap the axes

Rework `Viewport` so:
- **Pitch → x:** `xOf(pitch)`, lane **width** `laneW`, with `pitchLo` at the
  **left** and `pitchHi` at the **right** (lowest MIDI left, highest right).
  Horizontal window is still the visible pitch span (full 88-key range fits, or
  a window that scrolls with the cursor pitch — pick fit-88 if it reads well,
  else keep the ±range and scroll horizontally).
- **Time → y:** `yOf(us)` with **earlier time at the bottom** (`y = height −
  (us − originUs)·pxPerUs`) and later time toward the top. `pxPerUs` now sizes
  the vertical axis; `spanUs` is the vertical zoom. Keep the "anchor a third of
  the way in" behaviour, but as a **vertical** offset from the bottom so there
  is history below and lookahead above the cursor/playhead.
- Keep the module pure geometry; update the doc comment to describe the new
  orientation. Rename fields where x/y meaning changed (`laneH`→`laneW`, etc.)
  and update all call sites.

### 2. `EditCanvas.ts` — redraw in the new axes

- **Lanes** become **vertical** columns (one per pitch), black-key columns
  tinted darker; C columns labelled (`C3`…) along the **bottom** gutter instead
  of the left.
- **Gridlines** become **horizontal** (per subdivision step / beat / bar),
  heaviest per bar — drawn across the width at each step's `yOf`.
- **Notes:** rounded rects, width = one pitch lane, **height ∝ duration**,
  spectrum fill by pitch class, velocity → alpha. A note starting later sits
  higher.
- **Cursor / selection / chord preview / playhead:** re-expressed in the new
  axes — cursor is the cell at `(xOf(pitch), yOf(cursorStep))`; playhead is a
  **horizontal** line; loop region a horizontal band between
  `loop_start_us`/`loop_end_us`.
- Cursor-follow scrolling now tracks the cursor **vertically** (and pitch
  horizontally if you kept a scrolling pitch window).

### 3. `keymap.ts` — comments only

Update the navigation-section comments to describe vertical-time / horizontal-
pitch. **Do not change any action name or param** — verify by diffing that only
comments moved.

## Tests

- `npx tsc --noEmit` passes after the field renames + call-site updates.
- If practical, a tiny pure-geometry sanity check kept inline (e.g. assert in a
  comment / dev-only): `yOf(0) ≈ height` (song start at bottom),
  `yOf(later) < yOf(earlier)`, `xOf(pitchLo) < xOf(pitchHi)`.

## Scope boundaries (do NOT)

- Do not change `core`, the snapshot shape, or any `Action`.
- Do not change the keymap's action bindings (comments only).
- Do not add the video backdrop (that is M7-tauri-N) — but leave the canvas
  layering such that N can draw a frame **behind** the grid (e.g. keep the grid
  background fill optional/translucent-ready).
- Do not add graphics libraries — raw 2D context, like the highway.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] `npm run dev` → Compose (new): empty grid reads vertically — earliest
      step at the bottom, pitch low→high left→right; `k`/`ArrowUp` moves the
      cursor up (later in time), `h`/`l` move pitch left/right
- [ ] Notes, selection, chord preview, cursor, playhead and loop band all
      render correctly in the new orientation; Space plays with the playhead
      sweeping upward
- [ ] PR opened against `main` from `claude/tauri-edit-vertical`, `Closes #189`
