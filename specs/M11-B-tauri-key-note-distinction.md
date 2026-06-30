# M11-B — Tauri highway: make white-key vs black-key note blocks distinct

> Milestone: M11 — Highway readability · Issue: #230 · Suggested tier: sonnet
> Branch: `claude/m11-tauri-key-note-distinction`
> Related: M11-A (#229, same goal for the TUI highway) ·
> M11-C (#234, edit-view sibling, shares the `keyNoteStyle` helper this spec defines)

## Goal

On the Tauri canvas note highway, make a **black-key** note block (accidental:
C#/D#/F#/G#/A#) immediately recognizable as different from a **white-key**
(natural) note block, at a glance. Today the only differences are a slightly
smaller gap and a marginally lower alpha for black keys — too subtle to read in
motion.

## Design reference

The target look is the updated Claude Design prototype in
`design/note_highway/rockcraft-proto/` (serve the folder over HTTP — e.g.
`python3 -m http.server` — then open `Note Highway Prototypes.html`; opening it
as a `file://` URL renders blank because Babel fetches the `.jsx`/`.js` siblings
over XHR). The black-key treatment lives in `highway.js` (`drawNote`, the
`distinguishBlack` branch) and is labelled "BLACK ♯" in the legend
(`prototypes.jsx`). In the prototype a black-key note is:

- **slim** — noticeably narrower than the lane (the prototype uses a ~0.36 gap
  vs. a white key's small gap), so accidentals read as a thinner pill;
- **darker** — tinted from the *adjacent lower white key's* colour, then darkened
  (`shade(..., -0.18)`), so the hue still tracks pitch but the block reads dimmer;
- shaped with a **diagonal cutoff on the rear (top) edge** when perspective is
  off — a highway-motion cue.

Match this look. The diagonal rear cutoff is highway-specific; the edit-view
sibling (M11-C) reuses the slim+darker treatment but **not** the cutoff.

## Context

- Front end only: `tauri-app/src/screens/highway/`. The backend (`core`) carries
  no per-note style — keep it that way.
- Drawing: `HighwayCanvas.ts`
  - `drawNote()` (~line 364): `const gap = lane.black ? 0.06 : c.noteGap;` and
    `ctx.globalAlpha = lane.black ? 0.96 : 1;` — the current (weak) distinction.
    Lane geometry comes from `keyLayout.byNote[note]` which carries `black: boolean`.
  - `noteColor()` (~line 260): three `colorMode`s — `spectrum` (hue from pitch
    class), `accent` (single color), `hands` (`handColors[hand]`). The black-key
    distinction must read clearly in **all three** modes.
- Helpers: `utils.ts::isBlack(n)` (`[1,3,6,8,10].includes(n % 12)`) and
  `spectrumHue(n)`. Reuse `isBlack`; do not duplicate.

## What to do

1. **Define a single pure helper** for the black/white note treatment in
   `utils.ts` (it lives here so the edit-view sibling M11-C can import the same
   one — see `EditCanvas.ts`, which already imports from `../highway/utils`):

   ```ts
   // Visual treatment for a note block, independent of color mode.
   export interface KeyNoteStyle { inset: number; stroke: string | null; shadeMul: number }
   export function keyNoteStyle(note: number): KeyNoteStyle
   ```

   so the rule lives in one place and is unit-testable.

2. **Make black-key blocks distinct, matching the prototype** (see *Design
   reference*), via redundant, mode-independent cues on top of whatever fill
   `noteColor()` produces. Reproduce both of the prototype's mode-independent
   cues, plus the outline so the boundary reads even when fills are similar:
   - a clear **width inset** (the slim pill — stronger than today's 0.06 gap);
   - a consistent **shade offset** that darkens the fill the same way in
     spectrum/accent/hands modes (the prototype's `shade(..., -0.18)`);
   - an **outline/stroke** that naturals don't get.

   The diagonal rear cutoff from the prototype is highway-specific and may be
   kept here, but it is **not** part of the shared `keyNoteStyle` (the edit view
   does not use it). Pick concrete numbers, document them in the file, and verify
   the result reads as intentional across all three color modes.

3. Apply the helper in `drawNote()` and remove/replace the ad-hoc
   `lane.black ? 0.06 : …` / alpha tweak so there is one source of truth.

## Tests

- Unit test (`utils` test in the repo's existing front-end test style):
  `keyNoteStyle` returns a distinct treatment for a black-key pitch (e.g. 61) vs
  a white-key pitch (e.g. 60) — e.g. larger `inset` and/or a non-null `stroke`
  for the black key — and agrees with `isBlack` across a full octave.
- Keep the existing geometry/layout tests green.

## Scope boundaries (do NOT)

- Do **not** change `core` or any Rust crate — `tauri-app/` front end only.
- Do **not** change the highway scroll geometry, the keyboard component, the
  color-mode definitions, or hand assignment — only the per-note key treatment.
- Do **not** add a bespoke IPC command or backend field; this is pure rendering.
- TUI parity is M11-A; do not edit `crates/`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; front-end checks (lint/test) green
- [ ] Black-key note blocks are clearly distinguishable from white-key blocks in
      all three color modes (spectrum/accent/hands), in motion
- [ ] PR opened against `main` from the branch above, `Closes #230`
