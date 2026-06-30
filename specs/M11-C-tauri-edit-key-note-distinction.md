# M11-C — Tauri edit view: make white-key vs black-key note blocks distinct

> Milestone: M11 — Highway readability · Issue: #234 · Suggested tier: sonnet
> Branch: `claude/m11-tauri-edit-key-note-distinction`
> Related: M11-B (#230, same goal for the Tauri highway — **defines the shared
> `keyNoteStyle` helper this spec consumes**) · M11-A (#229, TUI highway)

## Goal

In the Tauri **edit** view (the piano-roll composer), make a **black-key** note
block (accidental: C#/D#/F#/G#/A#) immediately recognizable as different from a
**white-key** (natural) note block, at a glance — matching the distinction the
highway gets in M11-B. Today edit-view note blocks carry **no** black/white
distinction: only the lane *background* is tinted darker for accidentals.

## Dependency

This builds on **M11-B (#230)**, which introduces the pure helper
`keyNoteStyle(note)` in `tauri-app/src/screens/highway/utils.ts`. Land M11-B
first (or rebase onto it); this task **reuses that exact helper** — do not define
a second one. `EditCanvas.ts` already imports from `../highway/utils`.

## Context

- Front end only: `tauri-app/src/screens/edit/`. The backend (`core`) carries no
  per-note style — keep it that way.
- Drawing: `EditCanvas.ts`
  - `drawNotes()` (~line 255): every note is filled `oklch(0.72 0.16 <hue>)` with
    velocity→alpha and a bright bottom (onset) edge — identical for black and
    white keys. This is where the distinction is missing.
  - `drawLanes()` (~line 192): `ctx.fillStyle = isBlack(p) ? "rgba(0,0,0,0.28)"
    : "rgba(255,255,255,0.012)"` — the lane *background* already distinguishes
    black keys; do **not** rely on it for the note block, and do not change it.
- The edit canvas is **spectrum-only** (single `oklch` hue per pitch) — there are
  no color modes here, so the treatment is simpler than the highway's.

## What to do

1. **Consume the shared helper** `keyNoteStyle(note)` from `../highway/utils` in
   `drawNotes()`. Apply its `inset` (slimmer pill), `shadeMul` (darker fill), and
   `stroke` (outline) to black-key note blocks so they read as distinct from
   naturals.
2. Keep the edit view's static look: apply the slim + darker + outline cues, but
   **not** the highway's diagonal rear cutoff (that is a scroll-motion cue and is
   not part of `keyNoteStyle`).
3. Keep the existing onset (bottom-edge) highlight and velocity→alpha mapping
   intact — layer the key treatment on top, one source of truth via the helper.

## Tests

- The helper's unit test is owned by M11-B. Add an edit-view-level test only if
  the repo's front-end test style makes the application (inset/shade applied to a
  black-key note) cheaply assertable; otherwise rely on the shared helper test
  plus manual verification.
- Keep the existing edit-canvas/viewport tests green.

## Scope boundaries (do NOT)

- Do **not** change `core` or any Rust crate — `tauri-app/` front end only.
- Do **not** redefine `keyNoteStyle`; import the one from M11-B.
- Do **not** change the lane backgrounds, grid, selection, viewport/scroll
  geometry, the keyboard component, or velocity/onset rendering — only the
  per-note key treatment in `drawNotes()`.
- Do **not** add a bespoke IPC command or backend field; this is pure rendering.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; front-end checks (lint/test) green
- [ ] Black-key note blocks are clearly distinguishable from white-key blocks in
      the edit view, consistent with the highway (M11-B), using the shared helper
- [ ] PR opened against `main` from the branch above, `Closes #234`
