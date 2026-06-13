# M9-C — Edit-mode transport & loop-region: discoverability and control

> Milestone: M9 — Tauri UX consolidation · Issue: #202 · Suggested tier: sonnet
> Branch: `claude/m9-transport-loop-control`

## Goal

Two related usability gaps in edit mode: (1) it isn't obvious how to **stop**
playback once `Space` starts it, and (2) there's no clear way to **set or move
the loop region** that `o` toggles. Make playback state and loop bounds visible
and directly controllable.

## Context

- `Space` is bound to `toggle_play_cursor`, `P` to `play_from_start`, and `o` to
  `toggle_loop` (`tauri-app/src/screens/edit/keymap.ts`; TUI
  `crates/tui/src/edit.rs::key_to_action`). `Space` already *toggles*, so pressing
  it again stops — but nothing on screen tells the user that, so it reads as "I
  can't cancel play".
- The loop is reflected by `snapshot.looping` (shown as a `LOOP` badge in
  `StatusBar.tsx`), but **how the loop's start/end are chosen is undiscoverable**
  and there is no visible loop region on the grid. Inspect `core`'s loop model
  (`toggle_loop` and any loop-bounds state on the composer / snapshot) to confirm
  whether the loop derives from the current selection, the cursor, or a fixed
  region, and design around what exists.
- A help overlay already exists (`?` in the edit screen, `HelpOverlay`), and the
  `StatusBar` carries a one-line hint strip.

## What to do

1. **Playback state is obvious and cancellable.** While the cursor is playing,
   show an unmistakable **PLAYING** indicator and make the stop affordance explicit
   in the status hint (e.g. "Space play/stop"). Confirm `Space` reliably stops and
   that `Esc` does not silently leave playback running.
2. **Loop region is visible and movable.** Render the loop's start/end on the grid
   when `looping` is on. Provide a clear way to **define and move** the loop region:
   - If `core` already ties the loop to the visual **selection** (`v`/`y` region),
     document that and make it discoverable (hint + on-grid band labelled "LOOP").
   - If there is no way to move the loop bounds, add explicit **loop-in / loop-out**
     controls. Prefer a pure `core::Action` (e.g. `set_loop_start` /
     `set_loop_end` at the cursor, or `nudge_loop_bounds`) so it is auto-wired to
     all frontends and the agent-control socket — follow the `action.rs` pattern
     (variant + `ActionInfo`/help + parity tests). Only add what's missing.
3. Update the status hint strip and the `?` help overlay in **both** frontends to
   document Space (play/stop), `P` (play from start), `o` (loop toggle), and the
   loop-region controls.
4. Apply to **both** Tauri (`keymap.ts`, `StatusBar.tsx`, `EditCanvas.ts`,
   `HelpOverlay`) and TUI (`edit.rs`).

## Tests

- If a new loop-bounds `Action` is added: `core` unit tests for its effect on the
  loop region, plus the `action.rs` parity battery (name/tag, help coverage,
  dispatch-from-help-params, uniqueness).
- Frontend test: with `looping` on, the loop band renders at the expected region;
  the play indicator appears while playing and clears on stop.

## Scope boundaries (do NOT)

- Do **not** change unrelated transport semantics (count-in, metronome, backing
  alignment).
- If a new loop `Action` is needed it lives in `core` (pure) — do **not** put loop
  logic in a frontend.
- Do **not** repurpose existing bindings (`Space`/`P`/`o`) to different actions.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Playback shows a PLAYING state and is clearly stoppable; the loop region is
      visible and its bounds can be set/moved, documented in the help overlay, in
      both Tauri and TUI
- [ ] PR opened against `main` from the branch above, `Closes #202`
