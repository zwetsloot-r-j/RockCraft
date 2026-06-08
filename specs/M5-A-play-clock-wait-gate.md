# M5-A — core: pausable play clock + wait-gate

> Milestone: M5 — Play-along & Backing Sync · Issue: #106 · Suggested tier: opus
> Branch: `claude/m5-play-clock-wait-gate`

## Goal

Add the pure, headless timing primitives the rest of M5 builds on: a **pausable
play clock** and a **wait-gate** that wraps the existing
[`WaitTracker`](../crates/core/src/wait.rs) with held-note tracking, so a
frontend can freeze playback until the required notes are held — and resume in
place. Add a wait-mode `Action` so the toggle is drivable via the action model
(M4-A) and the WS control surface.

## Context

- Crate: `crates/core` only. This is the cloud-testable heart of the feature;
  **no device, no audio, no terminal** (architecture invariant).
- Builds on `wait::WaitTracker` (already present, fully tested) and
  `song::backing_position_us` (the existing audio-sync formula — do **not**
  duplicate it; the play clock just produces the `clock_us` it consumes).
- The composer transport (`composer.rs`) is already time-injected via
  `advance(dt_us)` with no `Instant`; this task adds the *pausable* clock that
  the Play screen (M5-C) lacks today (it uses a raw `Instant`).
- Related: M5-C (#108) consumes both types; `Action` plumbing mirrors
  `specs/M4-A-action-model.md`.

## What to do

### 1. Pausable clock — new `crates/core/src/play_clock.rs`

```rust
/// A monotonic song-time clock the frontend advances by injected deltas. Time
/// accrues only while running; pausing freezes `now_us` until resumed. No
/// `Instant` — honours "decouple rendering from timing".
#[derive(Debug, Clone)]
pub struct PlayClock { /* now_us: u64, running: bool */ }

impl PlayClock {
    pub fn new() -> Self;          // now_us = 0, running = true
    pub fn now_us(&self) -> u64;
    pub fn is_running(&self) -> bool;
    pub fn pause(&mut self);       // idempotent
    pub fn resume(&mut self);      // idempotent
    pub fn set_paused(&mut self, paused: bool);
    /// Advance by `dt_us` **only when running**; a no-op while paused.
    pub fn advance(&mut self, dt_us: u64);
    /// Jump to an absolute position (seek); leaves running/paused unchanged.
    pub fn seek_us(&mut self, us: u64);
    /// Reset to 0 and running.
    pub fn reset(&mut self);
}
```

### 2. Wait-gate — extend `crates/core/src/wait.rs`

A thin layer over `WaitTracker` that owns the held-note set and answers the one
question the frontend needs each tick: *may the clock advance, or must it
freeze on the current step?*

```rust
/// Couples a `WaitTracker` with the live held-note set and the current song
/// time to gate a `PlayClock`. Pure: feed it held notes + clock, read back
/// whether playback should be frozen.
#[derive(Debug, Clone)]
pub struct WaitGate { /* tracker: WaitTracker, held: BTreeSet<u8>, armed: bool */ }

impl WaitGate {
    pub fn from_expected(notes: &[(MidiNote, u64)]) -> Self; // armed = false
    pub fn set_armed(&mut self, armed: bool);  // wait-mode on/off
    pub fn is_armed(&self) -> bool;

    /// Update the held-note set (call on every note-on/off).
    pub fn set_held(&mut self, held: BTreeSet<u8>);

    /// Given the clock position, advance the tracker past any now-satisfied
    /// steps, then report whether the clock must freeze. Returns `Frozen` when
    /// armed AND the current step's `time_us <= now_us` AND it is not satisfied;
    /// otherwise `Running`. When disarmed, always `Running`.
    pub fn poll(&mut self, now_us: u64) -> GateState;

    pub fn is_complete(&self) -> bool; // delegates to tracker
    pub fn awaiting(&self) -> Option<&Step>; // the step being waited on, if frozen-eligible
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState { Running, Frozen }
```

Semantics (the contract M5-C relies on):
- The gate only freezes once the clock has **reached** the step's `time_us`
  (you don't stall before the note is due). Until then, `Running`.
- Extra held notes are allowed (inherited from `WaitTracker`).
- A satisfied current step advances the tracker (possibly several steps in one
  `poll`, reusing `WaitTracker::update`).
- Disarmed ⇒ never freezes (free play-through).

### 3. Wait-mode action — `crates/core/src/action.rs`

Add a variant and wire it into `name()`, `action_names()`, `action_from_name`,
and the `all_variants()` parity oracle:

```rust
Action::ToggleWaitMode            // name: "toggle_wait_mode", no params
Action::SetWaitMode { on: bool }  // name: "set_wait_mode", params {"on": bool}
```

These mutate a `wait-mode armed` flag on whatever owns the gate (the Play
transport in M5-C). In `core` this task only needs the variants + parity; the
behavioural hook lands in M5-C. Keep the composer’s existing actions untouched.

## Tests (headless, `cargo test -p rockcraft-core`)

`PlayClock`:
- `advance` accrues while running; is a no-op while paused; resumes from the
  frozen value (pause at 1000, advance 500 paused → still 1000, resume + advance
  500 → 1500).
- `seek_us` jumps without changing running state; `reset` → 0 + running.

`WaitGate`:
- Disarmed: `poll` is always `Running` regardless of held notes.
- Armed, before the step is due (`now_us < step.time_us`): `Running`.
- Armed, step due and unsatisfied: `Frozen`; `awaiting()` is that step.
- Holding the required note(s) → next `poll` returns `Running` and the tracker
  advanced; chord requires all notes; multiple consecutive satisfied steps skip
  in one `poll`.
- `is_complete()` true after the last step; once complete, never `Frozen`.

`Action`:
- `name()` ↔ `action_names()` ↔ `action_from_name` round-trip for both new
  variants (the existing parity tests must include them); `set_wait_mode`
  rejects missing/mistyped `on`.

## Scope boundaries (do NOT)

- No I/O, audio, threads, or `Instant` in `core`.
- Do not modify `backing_position_us` or the composer transport here.
- Do not change any existing public signature except the additive `Action`
  variants and the `wait` module additions.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m5-play-clock-wait-gate`, `Closes #106`
