# M7-tauri-D-edit-grid — Composer edit screen: piano-roll grid, cursor, note editing

> Milestone: M7 · Issue: #164 · Suggested tier: opus
> Branch: `claude/tauri-edit-grid`
> Depends on: M7-tauri-A-ipc-bridge (#161), M7-tauri-B-shell-menu (#162)

## Goal

The composer edit screen as a canvas piano-roll, rendered **entirely from
`ComposerSnapshot`** and driven **entirely by `run_action`**. The frontend
holds zero edit logic — it is a renderer plus a keymap, exactly like the TUI
after the M4 refactor.

## Context

- TUI reference: `crates/tui/src/edit.rs` — especially `key_to_action()`
  (normal-mode keymap) and the render functions; `crates/tui/src/highway.rs`
  and `keyboard.rs` for the lane/keyboard geometry ideas.
- State source: `ComposerSnapshot` via the #161 bridge (`snapshot` events +
  `queryState()` on mount).
- Visual language: reuse the Spectrum palette and helpers from
  `screens/highway/utils.ts` (#23) if present — pitch-class spectrum colors,
  dark `#0f1016` background, `IBM Plex Mono` for numbers. The edit grid is a
  *horizontal* piano-roll (time → right, pitch → up), unlike the falling
  highway: it matches the mental model of `edit.rs` (steps × pitches).

## What to do

### `src/screens/edit/`

```
edit/
  EditScreen.tsx    # layout: status bar (top) + grid canvas (rest)
  EditCanvas.ts     # canvas renderer class: draw(snapshot, viewport)
  keymap.ts         # KeyboardEvent → { name, params } | frontend-intent
  viewport.ts       # step/pitch <-> pixel mapping, cursor-follow scrolling
  StatusBar.tsx     # bpm, time-sig, subdivision, input mode, bar:beat,
                    # loop on/off, metronome on/off, clipboard count
```

### `EditCanvas.ts` draws, per frame, from the latest snapshot

- Horizontal pitch lanes (visible window ±2 octaves around the cursor,
  scrolling to keep the cursor inside); black-key lanes tinted darker; C
  lanes labelled (`C3`…).
- Vertical gridlines per subdivision step, heavier per beat, heaviest per
  bar (derive from `bpm`, `time_sig`, `subdivision` — steps are
  `snapshot`-relative, same maths as `core::grid`).
- Notes: rounded rects, spectrum pitch-class fill, width ∝ duration,
  velocity → alpha.
- Cursor: outlined cell at `(cursor.step, cursor.pitch)`.
- Selection: translucent rect over `snapshot.selection` bounds.
- Chord preview: ghost (50% alpha, dashed outline) notes at
  `snapshot.chord_preview` pitches on the cursor step.
- Playhead: vertical line at `playhead_us`; loop region: tinted band between
  `loop_start_us`/`loop_end_us` when `looping`.

### `keymap.ts` — copy `key_to_action()` exactly

Every binding from `crates/tui/src/edit.rs::key_to_action()` (lines ~129–205)
maps to the same action name and params: `h/l/j/k` + arrows (cursor), `H/L`
(bar), `w/b/J` (octave), `g/G/0/$` (jumps), `</>` (subdivision), `a/i`
(add_note), `x/d` (delete_note), `[/]` (resize_note ∓1), `+/-`
(adjust_velocity ±8), `m` (toggle_grab), `c` (enter_chord_mode), `R/t`
(record arm/flavour), Space (toggle_play_cursor), `P` (play_from_start),
`,/./;/'` (nudge_backing_offset −10 ms/+10 ms/−250 ms/+250 ms), `o`
(toggle_loop), `M` (toggle_metronome), `C` (start_count_in_record),
`v/y/p/D` (selection/clipboard), Esc (clear_selection), `u/U` (undo/redo).

Chord-mode keys (`1`–`7`, `[`/`]`, `s`, Enter, Esc) are **#165** — when
`snapshot.chord_preview` is non-null, swallow keys and do nothing except Esc
→ `cancel_chord`, so the screen isn't wedged before #165 lands.

Frontend-only intents (`?` help, `s/S` save, Tab/menu exit) are also #165 —
this issue exits with the router's global Esc only when no selection/chord is
active.

### Rendering loop

Redraw on every `snapshot` event; while `snapshot.playing`, also run RAF for
smooth playhead interpolation between events (interpolate from the last
snapshot's `playhead_us` + wall-clock delta; snap on each real event).

## Tests

- `keymap.ts` unit-tested with vitest? **No** — do not add a JS test
  framework. Instead: a `keymap.test-table.ts` exporting the binding table,
  and a Rust test is *not* required either. Verification is the acceptance
  list + `tsc`. Keep `keymap.ts` declarative (a table, not a switch) so
  review against `edit.rs` is line-by-line.

## Scope boundaries (do NOT)

- Do not implement edit logic in TS (no note math beyond drawing).
- Do not implement overlays, save/load, prompts (#165) or sound (#166).
- Do not modify `crates/` (the snapshot already carries everything needed).
- Do not add canvas/graphics libraries — raw 2D context, like the highway.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] `npm run dev` → Compose (new): empty grid with cursor; `a` places a
      note, `x` removes it, `]`/`[` resize, `m`+`h/l` drags, `v`+moves+`y`
      then `p` pastes, `u`/`U` undo/redo — all rendered correctly
- [ ] Space plays: playhead sweeps, loop band shows when `o` toggled
- [ ] Status bar mirrors snapshot fields live
- [ ] PR opened against `main` from `claude/tauri-edit-grid`, `Closes #164`
