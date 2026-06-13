# M9-A — Unify Record and Compose/Edit into one capture+edit screen

> Milestone: M9 — Tauri UX consolidation · Issue: #200 · Suggested tier: opus
> Branch: `claude/m9-unified-capture-edit`

## Goal

Collapse the three separate entry points — **Record**, **Compose (new)**, and
**Edit last recording** — into a single screen that is both a recorder and an
editor, with a live toggle between *recording* and *editing* the same timeline.
A user opens one piece, and can step/live-record into it, hand-edit it, and play
it back without ever leaving the screen or losing context.

## Context

The pieces already exist; this task is consolidation, not new capability.

- **Tauri.** `tauri-app/src/shell/screens.ts` has separate `record` and `edit`
  `Screen` variants; `MenuScreen.tsx` exposes "Record", "Compose (new)", and
  "Edit last recording" as distinct items. The edit screen
  (`screens/edit/EditScreen.tsx`) **already** supports input modes —
  `core::InputMode` is `DirectEdit | StepRecord | LiveRecord`, toggled by the
  `toggle_record_arm` (`R`) and `toggle_record_flavour` (`t`) actions in
  `screens/edit/keymap.ts`, and `shell/Router.tsx::screenWantsInstrumentInput`
  already routes piano/mock-key input to the edit screen. The dedicated
  `RecordScreen.tsx` (toolbar, level meter, takes, dirty-exit) is the live-capture
  UX that must be folded in or reachable as the recording mode of the unified
  screen.
- **TUI.** `crates/tui/src/app.rs::Screen` has parallel `Record(RecordScreen)` and
  `Edit(Box<EditScreen>)` variants (`record.rs`, `edit.rs`). The same `InputMode`
  + `toggle_record_arm`/`toggle_record_flavour` actions drive capture inside
  `edit.rs`.
- Piano-key note entry at the cursor is the existing **StepRecord** behaviour in
  `core` (a key strike inserts a note at the cursor timeslot and advances);
  LiveRecord places notes by `NoteEvent::timestamp_us`. Verify and reuse — do not
  reimplement note placement in a frontend.

## What to do

1. **One screen, two modes.** Make the editor the single capture+edit surface.
   The mode toggle is the *input mode* already modelled in `core`: editing =
   `DirectEdit`, recording = `StepRecord`/`LiveRecord`. Surface a clear,
   discoverable **Record ⇄ Edit toggle** (reuse `toggle_record_arm`; do not add a
   new `core::Action` unless a genuine gap is found) with an unmistakable on-screen
   state (e.g. the existing `STEP-REC`/`LIVE-REC`/`EDIT` mode badge in
   `StatusBar.tsx`, plus a record indicator).
2. **Piano-key entry at the cursor.** Confirm that, in `StepRecord`, a piano /
   mock-key strike adds a note at the **cursor's current timeslot** (pitch from
   the key, step from the cursor) and that this works on the unified screen for
   both live MIDI and the keyboard mock. Fix the wiring if a strike is dropped or
   misrouted; the placement logic stays in `core`.
3. **Fold in the live-capture UX.** The recording mode must keep what
   `RecordScreen` provides and the editor lacks (input level / activity feedback,
   take handling, count-in, the record dirty-exit prompt). Reuse the existing
   `RecordToolbar`/meter components rather than rebuilding them.
4. **Menu + routing.** Replace "Record" / "Compose (new)" / "Edit last recording"
   with a smaller surface: **New piece** (opens the unified screen empty, in record
   mode) and **Continue last** (opens the latest bundle). Opening a bundle from the
   Library (`e`) lands on the same unified screen. Retire the standalone `record`
   screen variant (Tauri `screens.ts`; TUI `app.rs::Screen`) once nothing routes to
   it, or keep it only as a thin alias — justify the choice in the PR.
5. Apply the same consolidation to **both** the Tauri and TUI frontends so they
   stay in parity (`CLAUDE.md` — frontends are swappable; keep view parity).

## Tests

- Tauri component/integration test: opening the unified screen, toggling to record,
  striking a mock key adds a note at the cursor step/pitch; toggling back to edit
  lets `hjkl`/`a`/`x` operate on it.
- TUI test mirroring the above against `EditScreen` with a mock key source.
- A test that "New piece" and "Continue last" both land on the unified screen and
  that the old `record` route no longer exists (or aliases the unified one).

## Scope boundaries (do NOT)

- Do **not** add note-placement, timing, or capture logic to a frontend or change
  `core`'s `InputMode` semantics — reuse the existing actions and `core` behaviour.
- Do **not** change the recording file format / bundle layout.
- Background video selection and BPM editing are **separate** specs (M9-G, M9-D);
  do not pull them in.
- Keep the agent-control action surface unchanged (no renamed/removed actions).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] One screen records (step + live, piano keys land at the cursor) and edits
      the same timeline with a clear Record⇄Edit toggle, in both Tauri and TUI
- [ ] Menu no longer has three overlapping Record/Compose/Edit entries
- [ ] PR opened against `main` from the branch above, `Closes #200`
