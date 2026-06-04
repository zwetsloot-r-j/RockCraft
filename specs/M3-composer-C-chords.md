# M3-C — core: chord & scale model

> Milestone: M3 — Composer · Issue: #51 · Suggested tier: sonnet
> Branch: `claude/m3-chords`

## Goal

Pure music-theory helpers so the chord selector (#54) can offer "the chords
that fit the piece": given a key, generate the diatonic triads/sevenths and the
pitches of each, ready to drop onto the timeline as a stack of notes.

## Context

- Crate: `crates/core` (new module `chord.rs`, re-export from `lib.rs`).
- Operates on `MidiNote` (`core/events.rs`). Derive
  `serde::{Serialize,Deserialize}` so #56 can persist the `Key`.
- Scope is intentionally small: Major + Natural minor scales; Triad + Seventh.
  More scales/qualities are a later issue.

## What to do

```rust
// crates/core/src/chord.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scale { Major, NaturalMinor }

impl Scale {
    /// Semitone offsets of the 7 degrees from the tonic, within one octave.
    /// Major = [0,2,4,5,7,9,11]; NaturalMinor = [0,2,3,5,7,8,10].
    pub fn intervals(self) -> [u8; 7];
}

/// A musical key: tonic pitch-class (0..=11, 0 == C) + scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key { pub root_pc: u8, pub scale: Scale }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKind { Triad, Seventh }

impl Key {
    /// Pitch class (0..=11) of scale degree `1..=7` (wraps; degree 1 == root_pc).
    pub fn degree_pc(self, degree: u8) -> u8;

    /// Diatonic chord on `degree` (1..=7), stacked in thirds within the scale,
    /// voiced upward from `root` (the actual MIDI octave to start at). Triad =
    /// 3 notes, Seventh = 4. Notes that exceed 0..=127 are dropped. Returns the
    /// pitches ascending.
    pub fn diatonic_chord(self, degree: u8, kind: ChordKind, root: MidiNote) -> Vec<MidiNote>;

    /// If `root`'s pitch-class is a scale degree, the diatonic chord built on it;
    /// else None. (Used to recognise a chord under the cursor.)
    pub fn chord_for_root(self, root: MidiNote, kind: ChordKind) -> Option<Vec<MidiNote>>;
}
```

Build chords by stacking scale degrees (1,3,5[,7]) measured *within the scale*,
so quality (major/minor/dim) falls out of the key automatically.

## Tests

- C major (`root_pc=0`): triad on degree 1 = C E G (60,64,67 voiced from C4);
  degree 5 = G B D; degree 7 (vii°) = B D F. Seventh on degree 5 = G B D F.
- A natural minor (`root_pc=9`): triad on degree 1 = A C E.
- `diatonic_chord` drops notes above 127 when `root` is near the top.
- `degree_pc` wraps (degree 8 ≡ degree 1).
- `chord_for_root` returns `None` for a non-scale pitch class.

## Scope boundaries (do NOT)

- Only Major + NaturalMinor, Triad + Seventh. No inversions, no extended/altered
  chords, no Roman-numeral parsing (later issues).
- No timeline/UI code here; pure functions only.
- No new third-party deps beyond workspace `serde`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-chords`, `Closes #51`
