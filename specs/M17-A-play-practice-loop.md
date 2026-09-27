# M17-A — Play-mode practice loop

> Milestone: M17 · Issues: #283 (core), #284 (session + control), #285 (play screen) · Suggested tier: opus
> Branch: `claude/play-practice-loop` (one branch per part: `-core`, `-session`, `-ui`)

## Goal

Let the player drill a passage in play mode: mark one or more bars, then loop
**count-in → demo (the app plays the part) → count-in → your turn (you play it,
scored) → …** until the loop is stopped. Speed, wait mode ("pause until
played") and target hand stay adjustable throughout, and apply to the loop.

## Context

- Play session: `tauri-app/src-tauri/src/play.rs` (`PlaySession`, `tick_play`).
  It only moves forward today — no seek, no loop.
- Reusable pieces:
  - `core::PlayClock::seek_us` (`crates/core/src/play_clock.rs`) — exists, unused by play.
  - `core::WaitGate::seek_to` (`crates/core/src/wait.rs`) plus
    `PlaySession::rebuild_wait_gate` — re-point the gate after a jump.
  - The editor's loop wrap and audition reset: `Composer::start_play` /
    `tick_audition` (`crates/core/src/composer.rs`, ~:1656-1721). These clear the
    fired-note sets and pre-fill them for notes that ended before the jump target.
  - Tempo-map bar lookups, private to `Composer`: `bar_start_us` / `bar_at_us`
    (~:1966-1995). `PlaySession.bar_starts_us` holds the map in play-clock µs.
  - Speed `set_rate`, wait `set_wait_mode`, hand `set_practice` (and the other
    hand's autoplay in `pending_song_triggers`) — all exist and keep working.
  - Metronome click precedent: `composer.rs` `CLICK_MIDI_VALUE` (76) / velocities.
- Related: `specs/M3-composer-P-loop-metronome.md`, `specs/M9-C-transport-loop-control.md`
  (editor loop), `specs/M5-C-play-wait-mode.md`, `specs/M14-E-per-note-hand-override.md`.
  `specs/M7-tauri-H-play-live.md` deferred the practice loop; this spec lifts that.
- Control-surface rule (CLAUDE.md): every capability here gets a
  `control::HostCommand` (it drives the play session / audio), wired through both
  frontends' exhaustive `HostServices` match.

## Behaviour

**Marking a loop (keys only, play screen).**

| Key | Effect |
|-----|--------|
| `←` / `→` | Pause, and jump the playhead to the start of the previous / next bar. Space plays on from there. Outside a loop only. |
| `[` | Loop start = the bar under the playhead. Doesn't pause, so a passage can be marked on the fly while listening. |
| `]` | Loop end = the bar under the playhead (inclusive). |
| `l` | Start the practice loop over the marked bars / stop it. Nothing marked → loop the bar under the playhead; only a start marked → loop that one bar. |

- "Bar under the playhead" = the bar containing `now` via the tempo map. Before
  the first bar (the pre-roll) it is bar 0.
- Marks are bar indices. If end < start, swap them. The marked range is drawn
  as a faint band on the highway and named in the header ("Loop 5–8"), whether
  or not the loop is running.
- Changing a mark while looping restarts the loop (from its count-in) on the new
  range.

**The loop.** Range `[start_us, end_us)` = start of the first marked bar to the
start of the bar after the last one. The count-in is one bar long: the length of
the loop's first bar.

1. **Count-in** — the clock jumps to `start_us - count_in_us` (clamped at 0) and
   runs. Each beat sounds a click (song bus, MIDI 76; accent on beat 1). No song
   notes autoplay, nothing is scored, and the wait gate is disarmed. Notes before
   `start_us` stay visible but dimmed.
2. **Demo** (`start_us..end_us`) — the app plays the **target hand only**: the
   practised hand, or both when practising both. The other hand is silent. No
   scoring and no wait gate. Player strikes still sound through the input
   monitor but are ignored.
3. **Count-in** again, then **your turn** (`start_us..end_us`) — normal play
   restricted to the loop: the other hand autoplays as today, "hear the song"
   applies, and the wait gate is armed if wait mode is on (built from the loop's
   notes only). Scoring counts only spans starting in the loop.
4. At `end_us` the pass is scored and published (`last_pass`), the live
   score/combo reset, and the loop returns to step 1 (demo) with `pass += 1`.

- The wrap check runs **before** the wait-gate poll each tick, so a step at
  exactly `end_us` (the next bar's first note) never freezes the loop.
- `is_finished` never fires while looping.
- Every jump (wrap, count-in, `←`/`→`): seek the clock; rebuild and re-seek
  the wait gate; reset the song-trigger sets, pre-filling spans that end at or
  before the target (the `Composer::start_play` model); cut the song bus (`all_off`
  on the song bus's notes — the freeze fade may be mid-flight); re-seek the backing
  (see below).
- Speed (`set_rate`), wait mode and target hand stay live. Changing the hand or
  wait mode mid-loop takes effect at the next phase boundary. Speed takes effect
  at once.
- **Stop (`l`):** leave the loop and pause the transport at `start_us`, restoring
  normal play (full-song gate, triggers pre-filled, whole-song scoring resumes
  from there). Space then plays on from the loop start.

**Backing track.** `AudioState::sync_play_backing` gains jump detection: a
target earlier than `last_target_us`, or more than 250 ms past the expected
position, re-seeks (`BackingMsg::play_at`). The backing stays muted at rates ≠ 1×,
as today.

## What to do

### Part 1 — core (`claude/play-practice-loop-core`)

```rust
// crates/core/src/bars.rs — tempo-map bar lookups, shared by Composer and play.
pub struct BarMap<'a> { starts: &'a [u64], fallback_bar_us: u64 }
impl<'a> BarMap<'a> {
    pub fn new(starts: &'a [u64], fallback_bar_us: u64) -> Self;
    pub fn bar_at(&self, us: u64) -> u64;        // before the first start → 0
    pub fn bar_start(&self, bar: u64) -> u64;    // past the map: extrapolate the last bar's length
    pub fn bar_range_us(&self, first: u64, last: u64) -> (u64, u64); // [start(first), start(last+1))
}
```

`Composer::bar_start_us` / `bar_at_us` delegate to `BarMap` (behaviour
unchanged; the existing composer tests are the guard).

```rust
// crates/core/src/practice_loop.rs — the phase machine, pure and clock-agnostic.
pub enum LoopPhase { CountIn { next: Pass }, Demo, YourTurn }
pub enum Pass { Demo, YourTurn }
pub struct PracticeLoop { /* start_us, end_us, count_in_us, beats_per_bar, phase, pass */ }
impl PracticeLoop {
    pub fn new(start_us: u64, end_us: u64, count_in_us: u64, beats_per_bar: u8) -> Self; // phase = CountIn{Demo}, pass 1
    pub fn entry_us(&self) -> u64;               // start_us - count_in_us, clamped at 0
    /// Advance to `now`; returns what the session must do this tick.
    pub fn tick(&mut self, now_us: u64) -> LoopStep;
    pub fn click_due(&self, prev_us: u64, now_us: u64) -> Option<bool>; // Some(accent) when a count-in beat falls in (prev, now]
}
pub enum LoopStep { Stay, Enter(LoopPhase), JumpTo { us: u64, phase: LoopPhase, pass_done: bool } }
```

### Part 2 — session and control (`claude/play-practice-loop-session`)

- `PlaySession`: `seek_to(us)`; `step_bar(delta)` (pauses, then seeks to the bar start); `set_loop(first_bar, last_bar)` / `clear_loop()` /
  `mark_loop_start()` / `mark_loop_end()`; `loop_view()`. Phase-aware
  `pending_song_triggers` and `score_due`. Per-pass scoring (`PassSummary {
  pass, hits, misses, accuracy }`). The wrap happens in `advance` before the gate poll.
- `tick_play`: act on `LoopStep` (song-bus cut, count-in clicks on the song bus).
- `PlayStateEvent` / `PlayStatusView` gain `practice_loop: Option<LoopView>`,
  where `LoopView { first_bar, last_bar, running, phase: "count_in"|"demo"|"your_turn",
  pass, last_pass: Option<PassSummary> }`, plus the marks when not running.
- `HostCommand`s (`crates/control/src/host.rs`, catalog + help + parity tests):
  `PlaySeekBar { delta: i32 }`, `PlayMarkLoop { edge: "start"|"end" }`,
  `PlaySetLoop { first_bar: u32, last_bar: u32 }`, `PlayClearLoop`, and
  `PlaySetPractice { hand: Option<"left"|"right"> }` (closes the existing gap).
  Tauri: `control.rs` dispatch arms + `#[tauri::command]`s + `lib.rs` registration.
  TUI: `HostError::Unsupported` arms.
- `audio.rs`: backing jump detection (above).

### Part 3 — play screen (`claude/play-practice-loop-ui`)

- `ipc/bridge.ts` + `ipc/types.ts` wrappers and types.
- `HighwayScreen.tsx`: the keys above.
- `HighwayCanvas.ts`: a loop band between the marked bars, and dimmed notes
  outside the loop while it runs.
- `HighwayHeader.tsx`: a loop badge ("Loop 5–8 · DEMO / YOUR TURN · pass 3") and,
  after each pass, its accuracy.
- `docs/AGENT-CONTROL.md`: the new host commands.

## Tests

- `BarMap`: map lookups at, between and past starts; before the first start → 0;
  extrapolation past the map; empty map → uniform `fallback_bar_us`.
- `PracticeLoop`: a full cycle CountIn→Demo→CountIn→YourTurn→(JumpTo, pass 2)→Demo
  with exact µs boundaries; `entry_us` clamps at 0; `click_due` fires once per beat
  with the accent on beat 1; a tick that overshoots `end_us` still yields `JumpTo`.
- `PlaySession` (headless, like the existing `play.rs` tests):
  - The demo autoplays only the target hand and scores nothing.
  - Your turn scores only loop spans, and the pass summary is correct for a
    perfect take and for a take with one miss.
  - Wait mode freezes in your turn and never in the demo or count-in.
  - A step at exactly `end_us` doesn't freeze the loop.
  - Song triggers re-fire on every pass (no burst after a wrap).
  - `clear_loop` pauses at `start_us` with normal play restored.
  - `seek_to` backwards re-arms the gate at the target.
- `host.rs` parity tests cover the new variants. TUI arms return `Unsupported`.
- Frontend: key → command mapping and loop badge text as pure helpers with vitest.

## Scope boundaries (do NOT)

- No mouse/drag selection on the highway (keys only).
- No loop in the editor transport changes (`Composer` only loses its private bar
  helpers to `BarMap`).
- No TUI implementation (the TUI returns `Unsupported`).
- No new dependencies. No count-in sound other than the synth click.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; `npx tsc --noEmit` + `npm test` green (tauri-app)
- [ ] Manually: mark bars, run the loop through several passes at 0.5× with wait
      mode on and one hand; stop it and continue playing.
- [ ] One PR per part against `main`, each closing its issue (#283, #284, #285)
