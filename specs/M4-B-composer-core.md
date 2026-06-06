# M4-B — core: pure `Composer` model (apply Actions)

> Milestone: M4 — Agent Interface · Issue: #86 · Suggested tier: opus
> Branch: `claude/m4-composer-core`

## Goal

Lift the composer's *state machine* out of the TUI into a pure `core` type so
every frontend (TUI now; Tauri/Godot later) and the WebSocket interface share
one editor. `Composer::apply(Action)` mutates the timeline and returns the
`Effect`s a frontend must perform. No device, no synth, no terminal, no disk.

## Context

- Crate: `crates/core`, new module `composer.rs`. Builds on `Action`/`Effect`
  from M4-A (#85), and on existing `History` (#61), `Timeline` (#49),
  `Grid`, `Key::diatonic_chord`, `MidiNote`, `Velocity`.
- **This is a port, not a redesign.** The exact behaviour to reproduce lives in
  `crates/tui/src/edit.rs` (`EditScreen`): cursor/grab/chord/selection/clipboard/
  input-mode/transport/loop/metronome/count-in logic, plus the audition rules.
  Preserve semantics 1:1 (replace-on-occupied add, one-step minimum resize,
  velocity clamp 1..=127, chord preview/commit/cancel via `History::rollback`,
  step/live record, count-in discard, etc.). M4-C then makes the TUI delegate
  here, so any behaviour drift will break TUI tests.
- **Purity over wall-clock.** Today the TUI transport reads `Instant`. In `core`
  the playhead is a plain `u64` advanced by injected time (the existing
  `advance(dt_us)` / `set_playhead_us` seam). The frontend owns the clock — this
  keeps `core` headless-testable and honours "decouple rendering from timing".
- Move `Cursor` and `InputMode` into `core` (the TUI re-exports them in M4-C).
- Audition is **not** performed here; `apply` returns `Effect::AuditionNote/
  Chord/AllOff` and the frontend interprets them (it owns "currently sounding").

## What to do

```rust
// crates/core/src/composer.rs
pub struct Composer { /* History, Grid, Cursor, grabbed, Key, chord, input_mode,
                         selection_anchor, clipboard, transport(pure µs),
                         playhead_us, loop/metronome/count-in fields,
                         last_committed */ }

impl Composer {
    pub fn new() -> Self;                              // empty, 120bpm 4/4, C major
    pub fn from_timeline(t: Timeline, grid: Grid) -> Self;

    /// Apply one action; mutate state; return ordered effects for the frontend.
    pub fn apply(&mut self, action: Action) -> Result<Vec<Effect>, ActionError>;

    /// Advance the (pure) playhead during playback; returns audition note_on/off
    /// effects for spans whose boundaries were crossed, plus metronome/count-in
    /// and loop-wrap handling — i.e. the logic of today's `tick_audition`.
    pub fn advance(&mut self, dt_us: u64) -> Vec<Effect>;

    /// Serialisable read-only view for `query state` (M4-G) and rendering.
    pub fn snapshot(&self) -> ComposerSnapshot;

    // direct accessors the renderer/tests need
    pub fn timeline(&self) -> &Timeline;
    pub fn grid(&self) -> Grid;
    pub fn cursor(&self) -> Cursor;
    pub fn input_mode(&self) -> InputMode;
    pub fn is_playing(&self) -> bool;
    pub fn playhead_us(&self) -> u64;
    pub fn note_count(&self) -> usize;
    pub fn note_under_cursor(&self) -> Option<NoteId>;
    pub fn previewed_chord(&self) -> Option<Vec<MidiNote>>;
    // ...mirror the read accessors EditScreen exposes today, as needed by M4-C.

    /// Feed a played MIDI note (step/live record); same routing as today's
    /// `EditScreen::ingest`. Returns effects (live record may audition).
    pub fn ingest(&mut self, ev: NoteEvent) -> Vec<Effect>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposerSnapshot {
    pub notes: Vec<NoteView>,         // id, pitch, start_us, dur_us, velocity
    pub cursor: Cursor,               // pitch, step
    pub bpm: f64, pub time_sig: TimeSig, pub subdivision: Subdivision,
    pub input_mode: InputMode,
    pub playing: bool, pub playhead_us: u64,
    pub looping: bool, pub loop_start_us: u64, pub loop_end_us: u64,
    pub metronome: bool,
    pub selection: Option<SelectionView>, // pitch/us bounds when active
    pub chord_preview: Option<Vec<u8>>,
    pub clipboard_len: usize,
}
```

- Every mutating action takes a `History::checkpoint()` exactly where the TUI
  does today, so undo granularity is unchanged (a chord commits as one step).
- Actions that don't apply in the current mode are **no-ops returning `Ok(vec![])`**
  (e.g. `ToggleRecordFlavour` while in `DirectEdit`), matching today's behaviour
  — they are not `ActionError`s.
- `SetCursor` clamps pitch to `21..=108` and is the absolute-positioning action
  agents will lean on; document the clamp.

## Tests (core, headless)

Port/translate the existing `EditScreen` unit tests to drive `Composer::apply`
with `Action`s, asserting identical outcomes. At minimum:

- add/delete/resize/velocity: same results & clamps as #53's tests.
- grab + cursor-tracking move; chord preview cycle/commit/cancel (rollback);
  selection yank/paste/delete; step-record and live-record placement; undo/redo
  ordering and redo-stack clear; loop wrap, metronome click count, count-in
  discard via `advance`.
- `apply` returns the expected `Effect`s (e.g. `AddNote` → one `AuditionNote`
  with the cursor pitch & default velocity; `Stop`/`PlayFromStart` → `AllOff`).
- `snapshot()` reflects state after a representative action sequence.

## Scope boundaries (do NOT)

- Do not call any synth/audio, file, or terminal API. No `Instant`/wall-clock.
- Do not change `Timeline`/`History`/`Grid`/`Key` public signatures; consume
  them as-is. Do not change M4-A's `Action`/`Effect` types.
- Do not touch the TUI in this PR (that is M4-C). Do not add file save/load.
- No new third-party deps (serde/serde_json only).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m4-composer-core`, `Closes #86`
