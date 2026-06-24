# M12-B — Tauri play screen: pause/resume control + mock-input entry point

> Milestone: M12 — Mock-mode playback testing · Issue: #232 · Suggested tier: sonnet
> Branch: `claude/m12-tauri-mock-pause-play`
> Depends on: M12-A (#231 — `HostCommand::PlayTogglePause` and its backend pause)
> Related: M7-tauri-K (#187, demock entry points), M7-tauri-H (play live)

## Goal

Surface **pause/resume** on the Tauri play screen, and make it testable
**without a piano**: a player can start a play session in a mock-input mode,
pause and resume from the UI, and watch the highway freeze and continue. M12-A
adds the `HostCommand::PlayTogglePause` and the backend pause; this task is the
front-end control plus the mock entry point.

## Context

- Play screen: `tauri-app/src/screens/highway/HighwayScreen.tsx` drives
  `HighwayCanvas` off the backend `play_state` event (real clock + scoring, not
  the render loop). It already has a `keydown` handler (~96) and an `ipc/bridge`
  to invoke host commands. There is currently **no** pause affordance.
- Backend pause is M12-A: invoking `HostCommand::PlayTogglePause` freezes/thaws
  the backend play clock + scoring + backing, so `play_state` stops/advances
  accordingly. This task only sends that command and reflects state.
- Mock input: the TUI exposes `--mock` (`MockKeyboard`). The Tauri app needs an
  equivalent way to drive a play session from the computer keyboard when no MIDI
  piano is present (M7-tauri-K context: live wiring exists; a no-hardware path is
  needed to *exercise* play interactively). Reuse `rockcraft_midi::MockKeyboard`
  via the same input-source selection used for live capture
  (`tauri-app/src-tauri/src/midi.rs`); do not invent a parallel mock.

## What to do

1. **Pause/resume control** (`HighwayScreen.tsx`):
   - Add a visible Pause/Resume button **and** a `Space` key binding (in the
     existing `onKeydown`) that invoke `PlayTogglePause` via the bridge.
   - Reflect paused state in the UI (button label/icon + a "Paused" indicator),
     driven by the backend play state — not a local-only guess — so it stays
     correct if paused over the control socket too.
   - While paused, the highway/playhead visibly freeze (a consequence of the
     backend `play_state` not advancing); resuming continues from the same point.

2. **Mock-input entry for play** (`src-tauri/src/midi.rs` + the play start path,
   and a menu/launch affordance in the front end):
   - Provide a way to start a play session backed by `MockKeyboard` when no piano
     is connected (mirror the TUI's `--mock` fallback: if no MIDI input port
     matches, use the mock so play is always exercisable). Surface it as an
     explicit "mock input" option or an automatic no-hardware fallback —
     document which.
   - With mock input, a play session runs end-to-end (notes from the computer
     keyboard, scoring, `play_state` updates) so pause/resume can be verified
     with no hardware.

## Tests

- Front-end (repo's existing Tauri front-end test style): the Pause control and
  `Space` invoke `PlayTogglePause` exactly once per toggle; the button/indicator
  reflects the backend `play_state` paused flag (not a local toggle).
- Backend (`src-tauri`): the mock-input selection picks `MockKeyboard` when no
  piano port is available (a pure selection helper, unit-tested like
  M7-tauri-K's `newest`).
- Backend pause behaviour itself is covered by M12-A; do not re-test it here.

## Scope boundaries (do NOT)

- Do **not** add or change the `HostCommand` (that is M12-A) — only call it.
- Do **not** add a bespoke IPC command for pause; route through
  `PlayTogglePause`.
- Do **not** change highway visuals (M11) or the import/record paths.
- Do **not** alter the TUI (`crates/tui`) — its interactive path is M12-A.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; front-end checks (lint/test) green
- [ ] On the Tauri play screen, a session started with mock input can be paused
      and resumed from the UI (button + `Space`); the highway freezes while paused
      and continues from the same position on resume — verified with no piano
- [ ] PR opened against `main` from the branch above, `Closes #232`
