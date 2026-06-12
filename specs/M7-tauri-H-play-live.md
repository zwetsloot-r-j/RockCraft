# M7-tauri-H-play-live — Play screen live wiring: bundles, wait mode, scoring + summary

> Milestone: M7 · Issue: #168 · Suggested tier: opus
> Branch: `claude/tauri-play-live`
> Depends on: #23 (highway screen), #161 (IPC bridge), #166 (audio), #167 (MIDI input)

## Goal

Promote the Highway screen from the Ember Lantern mock to real play sessions
with TUI-parity behaviour: bundle loading, pausable `PlayClock`, `WaitGate`
wait mode, real scoring with live combo/score, "hear the song", backing
audio, and an end-of-take summary. This supersedes the mock-only scope of
`M2-tauri-note-highway.md`.

## Context

- TUI reference: `crates/tui/src/play.rs` — `PlayScreen` (spans, clock, wait,
  held, shift_us pre-roll, backing at `audio_start_us`, hear_song flag,
  on/off fired sets for audition).
- Core: `PlayClock` (`crates/core/src/play_clock.rs`, time injected via
  `advance(dt_us)`, never wall-clock inside), `WaitGate`
  (`crates/core/src/wait.rs`, `set_held`/`poll(now_us)` →
  `GateState::{Frozen, Running}`), `core::scoring::score` (`perfect_us:
  50_000`, `good_us: 150_000`), `core::stats`.
- Frontend: `screens/highway/` from #23 — keep the canvas + header; the
  engine's *internal* fake scoring and `performance.now()` clock are what
  gets replaced.
- Data shape: core `NoteSpan` is `(pitch, start_us, end_us)`; the design
  fixture's `{note, start, end, hand}` ms-based shape maps from it (no hand
  info in bundles — single color mode or derive by pitch split, copy the
  TUI's choice in `play.rs`).

## What to do

### Backend `tauri-app/src-tauri/src/play.rs`

A play session is backend state (timing decisions must come from MIDI
timestamps and the injected clock, not the render loop):

```rust
pub struct PlaySession {
    spans: Vec<NoteSpan>, title: String,
    clock: PlayClock, wait: WaitGate, shift_us: u64,
    hits/judgments accumulation for the summary,
}
#[tauri::command] fn play_load(…, dir: String) -> Result<PlayInfo, String>;
#[tauri::command] fn play_set_wait(…, on: bool);
#[tauri::command] fn play_toggle_hear_song(…) -> bool;
#[tauri::command] fn play_finish(…) -> PlaySummary; // or auto at last span end
```

- `play_load` parses the bundle like the TUI (`build_spans` pairing,
  pre-roll `shift_us`, backing from `meta.json`).
- Tick thread: `wait.poll(now)` gates `clock.advance(dt)` exactly as
  `play.rs::tick()`; backing starts when the clock crosses `shift_us`
  (via #166); hear-song auditions span on/offs through the synth.
- MIDI events (#167) update `wait.set_held` and are collected (with
  `timestamp_us`) for scoring; per-note judgments computed incrementally so
  the header score/combo are live, final report via `core::scoring::score`.
- Emit `"play_state"` events (~60 Hz while running): `{ time_us, frozen,
  score, combo, judgments_delta, held }`.

### Frontend changes (`screens/highway/`)

- `HighwayCanvas` gets an external time source: replace its internal
  `performance.now() - t0` with the latest `play_state.time_us`
  (+ wall-clock interpolation between events) and a `frozen` flag (freeze
  scroll, pulse the awaited notes on the hit line).
- Header score/combo/bar:beat/chord read from `play_state` instead of the
  engine's fake scoring. Keep the mock fixture as a `--demo` fallback path
  (menu "Play" without a bundle? No — route demo only when no bundle dir is
  given by the router, preserving #23's standalone value).
- Keys: `m` hear-song toggle, `w` wait-mode toggle (the TUI has wait mode
  agent-only; the Tauri screen makes it a key — one new binding, documented),
  Esc → menu (session torn down: `play_finish`, backing stopped, all_off).
- Summary panel on song end: hits, misses, extras, accuracy %, best combo;
  Enter replays, Esc → menu.

## Tests

- Rust: session-level test with `ScriptedSource`-style injected events — a
  scripted perfect take over a 4-note fixture bundle yields 4 Perfect, accuracy
  100%; an offset take (+120 ms) yields Good judgments; wait mode: clock does
  not advance past a step until its notes are held.
- Use a committed fixture bundle under `fixtures/` (tiny, MIDI-only).

## Scope boundaries (do NOT)

- Do not change `core` scoring/wait semantics; consume them.
- Do not implement practice-loop UI or difficulty (not in the TUI yet).
- Do not couple judgments to render/RAF timing (CLAUDE.md invariant).

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] Library bundle opens in the highway; mock-keyboard hits judge
      Perfect/Good/Miss and move score/combo
- [ ] Wait mode freezes until held; `m` toggles hear-song audibly (local)
- [ ] Summary totals match `core::stats` for a scripted take (test-proven)
- [ ] PR opened against `main` from `claude/tauri-play-live`, `Closes #168`
