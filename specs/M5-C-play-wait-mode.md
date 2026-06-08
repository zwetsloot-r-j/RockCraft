# M5-C — tui (Play): play-along wait-mode (freeze highway + pause music)

> Milestone: M5 — Play-along & Backing Sync · Issue: #108 · Suggested tier: opus
> Branch: `claude/m5-play-wait-mode`

## Goal

Rocksmith-style practice in the **Play** screen: the song scrolls and the
**backing track plays along**; with **wait-mode** on, whenever the player has
not yet hit the notes the highway needs, the highway clock **freezes and the
music pauses with it** — staying in sync — then both resume the instant the
correct notes are held.

## Context

- Crate: `crates/tui`, file `play.rs` (the `PlayScreen`).
- Today the Play screen already syncs the backing via
  `core::backing_position_us` (`backing_target_us` / `tick_backing`), but its
  clock is a free-running `Instant` (`started`, `now_us()`), which **cannot
  pause**. `WaitTracker` is unused.
- Depends on **M5-A (#106)**: `PlayClock` (pausable) and `WaitGate`
  (`WaitTracker` + held notes → `GateState`), plus `Action::ToggleWaitMode` /
  `SetWaitMode`. Depends on **M5-B (#107)**: `BackingHandle::pause/resume`.
- Held notes are available from the input source (mock keyboard works for
  headless tests); `HeldNotes` is already tracked in `play.rs`.
- Invariants: keep the freeze/resume **decision** in the core seams (M5-A) so it
  stays headless-testable; only the audio + render wiring is `loc:local`. Don't
  drive timing off frame rate.

## What to do

1. **Pausable clock.** Replace the raw `Instant`-derived `now_us()` with a
   `PlayClock` advanced from the run-loop frame delta (the loop already computes
   per-iteration timing for the editor `advance`; reuse that seam). `restart`
   resets the clock and re-arms the backing as today.

2. **Wait-gate.** Build a `WaitGate::from_expected(...)` from the loaded song’s
   notes (pitch + the shifted `start_us`). On every note-on/off update its held
   set from `HeldNotes`. Once per loop iteration:
   - `gate.poll(clock.now_us())`:
     - `Frozen` ⇒ `clock.pause()` **and** `backing_handle.pause()` (M5-B);
       surface a "waiting — play <note(s)>" status.
     - `Running` ⇒ `clock.resume()` and `backing_handle.resume()`.
   - When wait-mode is **off** (`gate` disarmed) behaviour is exactly today’s
     free play-through (no regression).

3. **Toggle.** A key (e.g. `w`) and `Action::ToggleWaitMode` flip wait-mode;
   reflect the state in the snapshot/status line so the WS control surface and
   the screenshot tests can observe it.

4. **Sync invariant.** While frozen, the backing position must not drift: it is
   paused, not stopped, so on resume it continues at the same file offset the
   highway expects (`backing_position_us(clock.now_us(), shift_us,
   audio_start_us)` still holds because the clock didn’t advance).

## Tests

- **Headless (mock keyboard + `PlayClock`/`WaitGate` seams):**
  - Wait-mode armed: advancing the clock past a step’s `time_us` with the wrong
    / no notes held does **not** advance the playhead (gate `Frozen`); supplying
    the correct held notes returns `Running` and the playhead advances.
  - Chord step requires all notes; extra notes allowed.
  - Wait-mode disarmed: playhead advances freely (regression guard).
  - The backing target position computed from the (frozen) clock equals the
    pre-freeze value — no drift.
- **Host (`loc:local`, note in PR):** with a real/mock performance, the music
  audibly pauses with the highway and resumes in sync; toggling `w` switches
  between wait and free play.

## Scope boundaries (do NOT)

- Do not reimplement the wait/clock logic locally — consume the M5-A core types.
- Do not change `backing_position_us` or the bundle/meta model.
- Do not add wait-mode to the Edit screen (that is intentionally out of scope;
  Edit gets transport + audible backing in M5-D).
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green (headless wait/clock tests included)
- [ ] Host verification of audible pause/resume sync noted in the PR
- [ ] PR against `main` from `claude/m5-play-wait-mode`, `Closes #108`
