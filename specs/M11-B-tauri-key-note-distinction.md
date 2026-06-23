# M11-B — Tauri highway: make white-key vs black-key note blocks distinct

> Milestone: M11 — Highway readability · Issue: #230 · Suggested tier: sonnet
> Branch: `claude/m11-tauri-key-note-distinction`
> Related: M11-A (#229, same goal for the TUI highway)

## Goal

On the Tauri canvas note highway, make a **black-key** note block (accidental:
C#/D#/F#/G#/A#) immediately recognizable as different from a **white-key**
(natural) note block, at a glance. Today the only differences are a slightly
smaller gap and a marginally lower alpha for black keys — too subtle to read in
motion.

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

1. **Define a single pure helper** for the black/white note treatment, e.g. in
   `utils.ts`:

   ```ts
   // Visual treatment for a note block, independent of color mode.
   export interface KeyNoteStyle { inset: number; stroke: string | null; shadeMul: number }
   export function keyNoteStyle(note: number): KeyNoteStyle
   ```

   so the rule lives in one place and is unit-testable.

2. **Make black-key blocks distinct via redundant, mode-independent cues**, on
   top of whatever fill `noteColor()` produces. Use at least **two** of:
   - a clear **width inset** (narrower than the lane — stronger than today's 0.06
     gap), giving accidentals a visibly slimmer pill;
   - an **outline/stroke** (e.g. a darker or contrasting border) that naturals
     don't get, so the boundary reads even when fills are similar;
   - a consistent **shade offset** (e.g. darken the fill by a fixed multiplier)
     applied the same way in spectrum/accent/hands modes.

   Pick concrete numbers and document them in the file. The result must look
   intentional and consistent across the three color modes (verify each).

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
