# M3-B — core: musical grid (tempo, time-sig, variable snap)

> Milestone: M3 — Composer · Issue: #50 · Suggested tier: sonnet
> Branch: `claude/m3-grid`

## Goal

Pure grid math so the editor can place a cursor on musical positions and snap
notes to them — at a subdivision the user changes on the fly (1/4 down to 1/32,
plus triplets). Timing stays in integer microseconds (consistent with
`NoteEvent::timestamp_us`); the grid only maps between µs and musical steps.

## Context

- Crate: `crates/core` (new module `grid.rs`, re-export from `lib.rs`).
- Used by the edit screen (#52) for cursor stepping/rendering and by the
  snap control (#58) to cycle subdivisions live. Derive
  `serde::{Serialize,Deserialize}` (workspace dep) so #56 can persist it.

## What to do

```rust
// crates/core/src/grid.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSig { pub beats_per_bar: u8, pub beat_unit: u8 } // e.g. 4/4

/// Cursor/snap resolution. `ALL` is the cycle order for #58.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subdivision { Quarter, Eighth, Sixteenth, ThirtySecond, EighthTriplet, SixteenthTriplet }

impl Subdivision {
    pub const ALL: [Subdivision; 6];
    pub fn finer(self) -> Subdivision;   // saturates at SixteenthTriplet end of ALL
    pub fn coarser(self) -> Subdivision; // saturates at Quarter
    pub fn label(self) -> &'static str;  // "1/4", "1/8", "1/16", "1/32", "1/8T", "1/16T"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid { pub bpm: u32, pub time_sig: TimeSig, pub subdivision: Subdivision }

impl Grid {
    /// Sensible default: 120 BPM, 4/4, 1/16.
    pub fn default_120() -> Self;
    pub fn quarter_us(&self) -> u64;     // 60_000_000 / bpm
    /// Duration of one cursor step at the current subdivision.
    /// 1/8 = quarter/2, 1/16 = quarter/4, 1/32 = quarter/8,
    /// 1/8T = quarter/3, 1/16T = quarter/6.
    pub fn step_us(&self) -> u64;
    pub fn snap(&self, us: u64) -> u64;          // nearest multiple of step_us
    pub fn step_index(&self, us: u64) -> u64;    // floor(us / step_us)
    pub fn us_of_step(&self, step: u64) -> u64;  // step * step_us
    pub fn bar_us(&self) -> u64;                 // beats_per_bar * quarter * 4/beat_unit
    /// (bar, beat) for display; both 0-based. beat within 0..beats_per_bar.
    pub fn bar_beat_of(&self, us: u64) -> (u64, u64);
}
```

Integer division is fine for triplets (no exact ms); pin expected values in
tests at 120 BPM so the rounding is locked.

## Tests

- At 120 BPM: `quarter_us` == 500_000; 1/16 `step_us` == 125_000; 1/8T == ~166_666.
- `snap` rounds to nearest step (e.g. 120_000 → 125_000 at 1/16; 60_000 → 0).
- `us_of_step`/`step_index` round-trip on exact multiples.
- `bar_us` for 4/4 @120 == 2_000_000; `bar_beat_of` maps beat 5 → (bar 1, beat 1).
- `finer`/`coarser` walk `ALL` and saturate at the ends; `label` strings pinned.

## Scope boundaries (do NOT)

- No editor/cursor state here (that is #52); no tempo-mapped MIDI export.
- No changes to `events_to_smf_bytes` timing (its `1 tick = 1 µs` TODO is separate).
- No new third-party deps beyond the existing workspace `serde`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-grid`, `Closes #50`
