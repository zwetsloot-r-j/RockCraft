# M27-B — edit: enter and show finger numbers (desktop + TUI)

> Milestone: M27 — Finger numbers · Issue: #371 · Suggested tier: sonnet
> Branch: `claude/m27-finger-edit` · Depends on: M27-A (#370)

## Goal

In edit mode, let the author type a finger number onto the note under the
cursor (or the selection), and draw that number on every note that has one.
Notes without a finger look exactly as they do today. Both frontends.

## Context

- M27-A added `Note.finger`, `NoteView.finger` and the actions
  `set_note_finger { finger }` / `clear_note_finger`. This spec only adds keys
  and drawing.
- **Digits are not free.** On the desktop, `1`–`9` start the M25-A count prefix
  (`EditScreen.tsx`, `count.ts`), and with a mock MIDI source the router also
  forwards digits to the mock keyboard. In the TUI, digits are free in normal
  mode but go to the mock keyboard while record is armed (`app.rs`, the
  `forward_key` arm). So finger entry uses a **prefix key**.
- `f` is free in normal mode in both frontends. (The desktop binds `f` only in
  background mode, and `F` to something else — leave both alone.)
- Desktop notes are drawn in `EditCanvas.ts` `drawNotes`; a hand override
  already draws a white outline plus a white dot at the onset. The TUI edit
  grid (`crates/tui/src/edit.rs` `draw_highway`) draws notes from
  `build_spans(&self.timeline().to_events())`, which drops per-note metadata,
  and draws no hand marking at all.

## What to do

### B1 — keys (both frontends, same behaviour)

In normal mode:

| keys | effect |
| --- | --- |
| `f` then `1`…`5` | `set_note_finger { finger: n }` |
| `f` then `0`, `x`, Backspace or Delete | `clear_note_finger` |
| `f` then Esc, or any other key | cancel; the other key is **not** acted on |

- After `f`, the status line shows a hint such as `finger: 1-5, 0 clears`
  until the next key.
- The pending state is a view concern; keep it in the frontend, like the count
  prefix. It takes priority over the count prefix and over mock-keyboard
  forwarding: the digit after `f` must reach neither. On the desktop that means
  `stopPropagation` as well as `preventDefault` for that key.
- `f` is inert in chord mode, background mode, split mode and while an overlay
  or prompt is open — same places the other normal-mode edit keys are inert.
- A count typed before `f` is discarded.

### B2 — desktop drawing (`tauri-app/src/screens/edit/`)

- Draw the digit on each note with `finger != null`, centred horizontally in
  the note, sitting at the onset end, in a dark colour that reads on every hand
  tint and at the lowest velocity alpha.
- Put the "where does the digit go" decision in a pure helper (new
  `finger.ts`) so vitest can cover it:
  - note tall and wide enough for the glyph → inside the note at the onset;
  - otherwise → just outside the note beyond its tail, in the note's tint,
    so a short note still shows its number;
  - lane narrower than a readable glyph (about 7 px) → no digit.
- When a note has both a hand override and a finger, the digit replaces the
  override dot; the white outline still marks the override.
- The status bar shows the finger of the note under the cursor when it has one.
- Help overlay: add the rows to the "Hands" section (rename it "Hands &
  fingers").

### B3 — TUI drawing (`crates/tui/src/edit.rs`)

- Draw notes from `timeline().notes()` rather than `to_events()` so the finger
  is available; the grid must otherwise look the same.
- For a note with a finger, replace the glyph in the note's **onset row** with
  the digit, in the centre column of the cell (the only column at Compact scale
  and on black keys), dark foreground on the note's colour as background.
  Selected notes keep the selection colour as that background.
- Status bar: show the cursor note's finger when set; add `[f] finger` to the
  hint string. Add an entry to the `?` help overlay (and one for `n` hand,
  which is missing there today).

### B4 — docs

`docs/TAURI-CONTROLS.md` and the keys section of any TUI help text that lists
edit keys.

## Tests

- Desktop (vitest): the placement helper for a tall note, a short note and a
  too-narrow lane; the key handler — `f` `3` dispatches
  `set_note_finger {finger: 3}`, `f` `0` dispatches `clear_note_finger`, `f`
  Esc dispatches nothing, `f` `7` dispatches nothing and does not start a
  count, and a digit after `f` does not reach the count prefix.
- TUI: `f` `3` sets the finger on the cursor note; `f` `0` clears; `f` then `j`
  cancels without moving the cursor; with record armed and the mock source, the
  digit after `f` is not forwarded as a note. A `render_to_string` test: a
  fingered note shows its digit in the onset row, an unfingered note shows
  none, at Roomy and at Compact scale.
- The existing grid render tests still pass unchanged for pieces with no
  fingers.

## Scope boundaries (do NOT)

- No new `Action`s — use the two from M27-A.
- No auto-advance to the next note after setting a finger, and no automatic
  fingering.
- Do not change the count prefix or any existing binding.
- No hand tint in the TUI grid (still out of scope, as in M14-E).
- No third-party dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `npm run check` (tsc + vitest) green in `tauri-app/`
- [ ] PR opened against `main` from the branch above, `Closes #371`
