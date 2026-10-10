# M20-A — View-only vertical zoom in edit mode + snap note end

> Milestone: M20 — Precision editing · Issue: #319 · Suggested tier: sonnet
> Branch: `claude/m20-edit-zoom-snap-end`

## Goal

Two small precision-editing tools for cleaning up artifacts (tiny notes,
near-duplicate onsets, ragged note ends):

1. A **view-only vertical zoom** in the edit screen of both frontends, so notes
   draw longer and short artifacts become visible. It changes **only the view**:
   no note, bar line, tempo or grid value changes.
2. A **`snap_note_end`** action that snaps a note's end to the grid without
   touching its onset, bound to a key in both frontends.

## Context

- **Why this exists:** in the desktop editor, `W`/`Q` are `nudge_bar_length`
  (`keymap.ts:96-97`). Users looking for zoom press them and move bar lines.
  The only real zoom today is `W`/`S` inside the `` ` `` backdrop-calibration
  overlay (`EditScreen.tsx` `zoomGrid`). That overlay opens only when a video
  is attached, takes over the keyboard, and saves the zoom as **video
  alignment** (`gridSpanUs` → `saveVideoCal` / `alignment.json`). It is not a
  general view zoom, and this spec leaves it unchanged.
- **Desktop rendering:** `EditCanvas.ts` draws with `gridCal.spanUs` (µs across
  the canvas height) through `Viewport` (`viewport.ts`, `pxPerUs`).
  `onKeydown` (`EditScreen.tsx`) currently returns early on `ctrlKey`/`metaKey`
  and on every Alt combo except `ALT_TEMPO_CODES`. `+`/`=`/`-` are
  `adjust_velocity`; the status hint shows `+/- vel`.
- **TUI rendering:** `crates/tui/src/edit.rs` shows a fixed window,
  `lead_us() = bar_us × LEAD_BARS` (`LEAD_BARS = 4`), with the cursor anchored
  ¼ up. `edit.rs` has no modifier-key handling yet.
- **Precedents for snapping:** `quantize_region` in `crates/core/src/composer.rs`
  snaps onset **and** end, using `grid.snap_to_step` on a uniform grid and
  `map_snap` when a tempo map (`bar_starts`) exists. `resize_note` (`[`/`]`)
  adds or removes whole steps but keeps an off-grid end off-grid.
  `SetNoteHand` is the precedent for "target = selection if active, else the
  note under the cursor".

## What to do

### 1. View zoom: desktop app (`tauri-app/src/screens/edit/`)

- Add an editor-local **view zoom factor** `viewZoom` (default `1`, range
  `0.25`–`16`, multiplicative step `×1.25`). The canvas draws with an
  **effective span** of `gridCal.spanUs / viewZoom`. The calibration
  `spanUs` itself is never modified.
- `viewZoom` must **not** be written to `saveVideoCal`, `alignment.json`, the
  bundle, or any `core` state. It is session/view state only.
- Zoom keeps the **cursor row** fixed on screen (zoom about the cursor, or the
  playhead while playing), so the note being fixed stays in view.
- Inputs:
  - **`Ctrl` + mouse wheel** over the edit canvas: wheel up zooms in, wheel down
    zooms out. Register the listener with `{ passive: false }` and call
    `preventDefault()` so the webview's own page zoom never fires. Plain wheel
    keeps its current behaviour.
  - **`Alt` + `=`** zoom in, **`Alt` + `-`** zoom out, **`Alt` + `0`** reset to 1×.
    Match on `e.code` (`Equal`, `Minus`, `Digit0`) and add them to the Alt
    allow-list next to `ALT_TEMPO_CODES`. Do not use `Ctrl`+`=`/`-`; those are
    the webview's page-zoom keys.
  - The zoom keys also work while the calibration overlay is closed and **no
    video is attached**.
- Show the factor in the status bar when it is not 1× (e.g. `zoom 2.4×`).
  With a backdrop attached and zoom ≠ 1×, the video no longer lines up with
  the grid. That is expected; the indicator makes it obvious, and `Alt`+`0`
  restores alignment.
- Add the keys to the `?` help overlay.

### 2. View zoom: TUI (`crates/tui/src/edit.rs`)

- Add a `view_zoom` to the edit screen, with the same range, step and default
  as the desktop app (store it as a rational or permille to keep it integer).
  `lead_us()` becomes `bar_us × LEAD_BARS / view_zoom`, never below 1 µs.
  Every place that uses `lead` (gridlines, loop band, split markers, playhead,
  notes, cursor) already goes through `lead_us()`, so check that nothing
  bypasses it.
- Keys: **`Alt` + `=`**, **`Alt` + `-`**, **`Alt` + `0`**. Match
  `KeyModifiers::ALT` with `KeyCode::Char('=')` / `'-'` / `'0'`. Plain `=`,
  `-` and `0` keep their current bindings.
- Status bar: show `z2.4×` only when ≠ 1× (stay within the 80-column budget).
  Update the help overlay.
- Mouse-wheel zoom in the TUI is **out of scope**: mouse capture is not
  enabled.

### 3. `snap_note_end` action (core)

```rust
/// Snap the END of the target notes to the nearest grid line at the live
/// subdivision, leaving each onset untouched. Target = the selection when one
/// is active, else the note under the cursor; with neither it is a no-op.
SnapNoteEnd,
```

- Grid lines: reuse `quantize_region`'s computation (uniform grid via
  `grid.snap_to_step` phased from the origin; `map_snap` when `bar_starts`
  exists). Factor out a shared helper rather than duplicating it.
- New end = the nearest grid line to `start + dur`. If that is ≤ the onset, use
  the **first grid line strictly after the onset**, so the note keeps a
  positive length.
- One `checkpoint()` for the whole action. Do **not** checkpoint when there is
  no target or no note would change. Keep note ids stable (resize in place, as
  `resize_note` does).
- Add the catalog/help entry and extend the `action.rs` parity tests.
- Bind **`|`** in both frontends (it is free in both: `keymap.ts` and
  `edit.rs::key_to_action`). Add it to the status hints/help next to `[`/`]`.

### 4. Docs

`docs/AGENT-CONTROL.md` action vocabulary: add `snap_note_end`.

## Tests

- **core:** with the cursor on a note whose end is 30% past a grid line, the end
  snaps back to that line; at 70% it snaps forward; the onset is unchanged.
- **core:** a very short note whose nearest line is ≤ its onset gets extended to
  the first line after the onset.
- **core:** with a selection over 3 notes, all 3 ends snap and one `undo`
  restores all 3. An already-snapped note and an empty cursor cell cause no new
  undo step.
- **core:** with a tempo map (`set_bar_starts` with uneven bars), the ends snap
  to the map's grid lines.
- **core:** the `action.rs` parity battery passes.
- **TUI:** `Alt`+`=` divides the visible span by 1.25 per press up to the clamp,
  `Alt`+`-` reverses it, and `Alt`+`0` resets. Plain `=`/`-` still adjust
  velocity. `|` dispatches `SnapNoteEnd`. The note under the cursor is still
  visible after zooming in. Zoom changes no note, bar or BPM in the snapshot.
- **desktop (vitest, where the harness allows):** the effective span is
  `spanUs / viewZoom` and the clamp holds. `saveVideoCal` is never called with
  a changed span because of view zoom.

## Scope boundaries (do NOT)

- Do not change the backdrop-calibration overlay, its keys, or its persisted
  values.
- Do not rebind any existing key (`Q`/`W`, `+`/`-`, `{`/`}` and so on keep
  their meaning).
- No horizontal (pitch-axis) view zoom; no TUI mouse support.
- Do not persist the view zoom.
- Do not add third-party dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #319`
