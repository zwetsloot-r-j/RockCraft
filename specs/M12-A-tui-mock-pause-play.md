# M12-A — Pause/resume a play session (PlayTogglePause), interactive in `--mock`

> Milestone: M12 — Mock-mode playback testing · Issue: #231 · Suggested tier: sonnet
> Branch: `claude/m12-tui-mock-pause-play`
> Related: M12-B (#232, Tauri play-screen pause control + mock entry)

## Goal

Let a player **pause and resume** an in-progress play-along session, and make
that exercisable **without a piano** via `--mock`. Today the play screen can
start/restart/finish but has no pause: `crates/tui/src/app.rs` (~331–336) binds
only `Esc` (leave), `r` (restart), `m` (hear-song), `w` (wait-mode). You cannot
freeze the highway mid-song to study a passage.

The pause/resume **clock + backing freeze machinery already exists** — wait-mode
uses it (`crates/tui/src/play.rs` ~220–233: `self.clock.pause()` + the backing
handle `h.pause()`, and the resume path). This spec exposes it as a first-class
transport control and routes it through a `HostCommand` so it is uniform across
frontends and driveable over the control socket.

## Context

- **Why a `HostCommand`, not a `core::Action`:** pausing the live play session
  touches the audio **backing handle** (I/O) and the real wall-clock
  `PlayClock` — it is not pure, so per `CLAUDE.md`'s control-surface rule it is a
  `control::HostCommand`, not an `Action`. (The pure composer transport
  `Action`s — `Play`/`Stop`/`TogglePlayCursor` — are a different, edit-screen
  surface; do not conflate.)
- **The compiler seam:** `control::HostServices::dispatch` is matched
  **exhaustively** in *both* frontends — `crates/tui/src/app.rs` and
  `tauri-app/src-tauri/src/control.rs`. Adding a variant forces both to handle it
  or the workspace won't compile. This task therefore must wire **both** arms
  (the rich TUI behaviour + a working Tauri-side pause); the Tauri *front-end UI*
  is deferred to M12-B.
- Existing sibling host commands to mirror for style/registration:
  `HostCommand::PlaySetWait { on }`, `PlayToggleHearSong`, `PlayFinish`
  (`crates/control/src/host.rs`). The `host.rs` parity tests require the new
  variant to appear in the catalog (`host_command_names` / help) and the match.
- Mock input: `crates/tui/src/main.rs` (`--mock` → `MockKeyboard`,
  `rockcraft_midi::MockKeyboard`). Pause must work the same whether input is a
  live piano or the mock.

## What to do

1. **Add the host command** (`crates/control/src/host.rs`):

   ```rust
   /// Toggle pause on the active play session (freeze/thaw clock + backing).
   PlayTogglePause,
   ```

   Register it in the name/help catalog exactly like `PlayToggleHearSong`, and
   update the `host.rs` parity tests/fixtures so the catalog stays in lockstep.

2. **TUI behaviour** (`crates/tui/src/play.rs` + `app.rs`):
   - Add `PlayScreen::toggle_pause(&mut self)` that freezes (clock + backing
     paused) when playing and thaws (clock + backing resumed at the current
     position) when paused — reuse the wait-mode freeze/thaw code paths; do not
     duplicate clock math. Track a `paused: bool` and surface it for the view.
   - Bind a key on the play screen (`Space`) in `app.rs` to `toggle_pause()`, and
     route `HostCommand::PlayTogglePause` to the same method.
   - While paused, the highway, playhead, and scoring clock do **not** advance;
     resuming continues from the same position (no jump, no missed-note storm).
   - Show a clear paused indicator in the play HUD.

3. **Tauri dispatch arm** (`tauri-app/src-tauri/src/control.rs` + `play.rs`/
   `state.rs`): implement `PlayTogglePause` to pause/resume the backend play
   state (the clock + scoring that drives the `play_state` event, and the backing
   audio). This is required for the workspace to compile and gives the Tauri side
   a working pause over the socket. The on-screen control/keys are M12-B.

4. Keep `--mock` launches fully functional: `cargo run -p rockcraft-tui --
   --mock`, enter a play session, `Space` pauses/resumes with no piano attached.

## Tests

- `crates/control`: `host.rs` parity — `PlayTogglePause` is in the catalog,
  round-trips by name, and the exhaustive-match tests pass.
- `crates/tui` (headless, using the existing `ScriptedSource`/fake-clock style in
  `crates/tui/tests/`): advancing a play session, calling `toggle_pause()`, then
  advancing wall-time, leaves the play position **unchanged** while paused;
  toggling again resumes advancement from that position.
- A no-op guard: `toggle_pause()` outside an active play session does nothing and
  does not panic.

## Scope boundaries (do NOT)

- Do **not** add a `core::Action` for this or change the composer/edit transport.
- Do **not** add a bespoke one-off IPC command — it goes through
  `HostCommand::PlayTogglePause` in both frontends.
- Do **not** build the Tauri play-screen button/keys or mock entry point — that
  is M12-B (this task only makes the backend Tauri arm pause correctly).
- Do **not** change the highway visuals (that is M11).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] In `cargo run -p rockcraft-tui -- --mock`, a play session can be paused and
      resumed with `Space`; the highway/playhead freeze while paused and continue
      from the same position on resume
- [ ] `PlayTogglePause` is callable over the control socket (`query help` lists it)
- [ ] PR opened against `main` from the branch above, `Closes #231`
