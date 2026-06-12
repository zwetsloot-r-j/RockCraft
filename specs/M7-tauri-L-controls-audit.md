# M7-tauri-L-controls-audit — Controls audit: remove redundant demo controls, wire/disable the rest

> Milestone: M7 · Issue: #188 · Suggested tier: sonnet
> Branch: `claude/tauri-controls-audit`
> Depends on: #169 (record live), M7-tauri-K (#187) (de-mock entry points)

## Goal

Audit every interactive control on the Tauri screens and resolve each one to a
known state: **real** (keep), **redundant demo-only** (remove or wire), or
**missing** (add the cheap, clearly-specced ones). The deliverable is a tidy
control surface where nothing is a decorative no-op, plus a short audit doc that
stays in the repo as the source of truth.

## Context

- `screens/record/RecordToolbar.tsx`, `RecordHeader.tsx`, `RecordScreen.tsx` —
  the toolbar/transport. `RecordScreen` keeps `metro`, `count`, `snap`, `clef`,
  `spelling` signals that drive only visuals; the Esc "confirm if unsaved" is
  stubbed (`// a confirm dialog is a future enhancement` — currently just stops).
- `screens/highway/HighwayHeader.tsx` — play header (`m` hear-song, `w` wait).
- `screens/menu/MenuScreen.tsx` — menu items.
- TUI reference for "what actually exists as behaviour":
  `crates/tui/src/record.rs`, `play.rs`, and the `core::Action` set. If no core
  action backs a control, it is **not** real.

## Audit (starting point — verify against the code, then act)

| Screen | Control | Status | Action in this task |
|---|---|---|---|
| Record | Trim / Quantize / Punch-in | redundant (no core action) | render **disabled** w/ tooltip "not yet wired" (per #169) |
| Record | Metronome toggle | cosmetic signal | wire to count-in/metronome if a trivial path exists; else disabled+tooltip |
| Record | Count-in toggle | cosmetic signal | same as metronome |
| Record | Clef (Grand/Treble/Bass) | cosmetic (staff render only) | keep **only if** it changes the staff render; else remove |
| Record | Spelling (♯/♭) | cosmetic (staff render only) | keep only if it changes render; else remove |
| Record | Snap (1/8…1/16) | cosmetic (no quantize) | disabled+tooltip until quantize exists |
| Record | Esc confirm-if-unsaved | **missing** (stubbed) | implement real Save / Discard / Cancel prompt |
| Record | Level meter | cosmetic ok (velocity-derived) | keep |
| Play | `m` hear-song, `w` wait | real | keep |
| Menu | "Edit last recording" | fixed in M7-tauri-K | n/a here |

Treat the table as a checklist: confirm each row against the actual code,
correct any mis-classification, and record the final verdict.

## What to do

1. **Disable, don't fake.** Any control with no backing behaviour renders in a
   visibly-disabled state with a `title` tooltip ("not yet wired"). No control
   may silently do nothing on click.
2. **Remove pure decoration** that isn't even cosmetically meaningful (a toggle
   whose value is never read). Delete the dead signal + its UI.
3. **Implement the record dirty-exit prompt** (the one clearly-missing, cheap
   control): Esc with unsaved input → overlay "Save (s) / Discard (d) /
   Cancel (Esc)", mirroring the TUI flow in `record.rs`. Save reuses
   `recordSave`; Discard stops + navigates to menu; Cancel dismisses.
4. **Write `docs/TAURI-CONTROLS.md`** — the finalized audit table (screen ×
   control × status × keybinding), so future screens can be checked against it.
   Keep it short; it is a map, not prose.

## Tests

- `npx tsc --noEmit` passes (dead signals/props removed cleanly).
- Frontend behaviour is acceptance-verified (no JS test framework — per the M7
  convention). If the dirty-exit decision logic is non-trivial, extract a pure
  `nextExitState(dirty, key)` helper and keep it obviously correct.

## Scope boundaries (do NOT)

- Do not add a quantize/trim/punch-in **implementation** (no core action — out
  of scope; those stay disabled).
- Do not add a JS test framework.
- Do not touch the edit screen (M7-tauri-M/N) or `crates/`.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] No control silently no-ops: every button is real, or disabled+tooltip
- [ ] Record Esc with unsaved input prompts Save/Discard/Cancel and behaves
- [ ] `docs/TAURI-CONTROLS.md` committed with the final audit table
- [ ] PR opened against `main` from `claude/tauri-controls-audit`, `Closes #188`
