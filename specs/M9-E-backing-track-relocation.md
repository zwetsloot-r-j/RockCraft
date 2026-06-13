# M9-E — Move "Choose backing track" into record/edit and persist it to the piece

> Milestone: M9 — Tauri UX consolidation · Issue: #204 · Suggested tier: sonnet
> Branch: `claude/m9-backing-relocation`

## Goal

Backing-track selection is currently a **top-level main-menu** action ("Choose
backing track"), detached from any piece. It should be an action available
**while recording/editing a specific piece**, and the chosen backing should be
saved into that piece's metadata so it travels with the bundle.

## Context

- Tauri menu: `MenuScreen.tsx::STATIC_ITEMS` includes
  `{ label: "Choose backing track", key: "backing-picker" }`, which navigates to
  the `backing-picker` screen. The picker and its actions already exist:
  `attach_backing` / `detach_backing` (`tauri-app/src-tauri/src/audio.rs`),
  `Screen` variant `backing-picker` (`shell/screens.ts`).
- TUI: `app.rs::Screen::BackingPicker(BackingPicker)` + `backing.rs`, reached from
  the menu equivalently.
- Persistence already exists in the model: `RecordingMeta.backing:
  Option<BackingTrack>` (`crates/core/src/song.rs`) is written/read with the
  bundle. The gap is that menu-level backing selection isn't tied to the loaded
  piece's meta.

## What to do

1. **Remove the main-menu entry.** Drop "Choose backing track" from
   `MenuScreen.tsx::STATIC_ITEMS` (Tauri) and the equivalent main-menu item in the
   TUI.
2. **Add the entry point inside the unified capture/edit screen.** Provide a
   "backing track" affordance (key + on-screen control) on the record/edit screen
   that opens the existing `backing-picker` for the **currently loaded** piece, and
   a way to detach. Reuse `attach_backing`/`detach_backing`; do not duplicate the
   picker.
3. **Persist to the piece.** When a backing is attached while editing, store it in
   the loaded bundle's `RecordingMeta.backing` so saving the piece persists it and
   reopening restores it (and the backing-alignment offset already handled by
   `nudge_backing_offset`). Verify the save/load round-trip.
4. Apply to **both** Tauri and TUI so the menus and edit screens stay in parity.

## Tests

- Save/load round-trip: attach a backing while editing, save the bundle, reload —
  `RecordingMeta.backing` is present and the backing plays/aligns as before.
- A test asserting the main menu no longer lists a standalone backing item and the
  edit screen exposes the backing entry point.

## Scope boundaries (do NOT)

- Do **not** change the `BackingTrack` schema or audio playback path — this is
  relocation + persistence wiring around existing pieces.
- Do **not** change backing-alignment (`nudge_backing_offset`) semantics.
- Background **video** backdrop is a separate spec (M9-G); this is audio backing
  only.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Backing selection is reached from record/edit (not the main menu), attaches
      to the loaded piece, and persists in its `meta.json`, in both Tauri and TUI
- [ ] PR opened against `main` from the branch above, `Closes #204`
