# M22-A — Notation model: quantise a piece into measures, staves and note values

> Milestone: M22 — Sheet-music export · Issue: #329 · Suggested tier: opus
> Branch: `claude/m22-notation-model`

## Goal

Make it possible to print a piece as sheet music. A piece only stores notes as
microsecond times (`start_us`, `dur_us`), plus pitch, velocity and hand. A
printed score needs bars, two staves, note values (quarter, dotted eighth, …),
rests, ties, chords, pitch spelling and accidentals. This spec adds the pure
**notation model** that derives all of that. M22-B writes it out as MusicXML,
which MuseScore (or any notation program) opens, engraves and prints.

## Context

- **Bars come from the tempo map.** `Composer::bar_starts` / `meta.bar_starts`
  (`crates/core/src/composer.rs`, `crates/core/src/song.rs`) hold the downbeat
  of every bar. When the map is empty, the grid is uniform:
  `Grid::origin_us` + `Grid::bar_us()`. `BarMap` (`crates/core/src/bars.rs`)
  already wraps both cases (`BarMap::new(starts, grid.bar_us()).with_origin(grid.origin_us)`;
  `bar_start`, `bar_at`, `bar_len`). Use it rather than re-deriving.
- **Tempo marks** come from `bars::tempo_changes(starts, beats_per_bar,
  beat_unit)`, which already filters out sub-0.5 BPM jitter and reports
  quarter-note BPM.
- **Metre.** `Grid::time_sig` (`TimeSig { beats_per_bar, beat_unit }`) is
  constant over the piece. Per-bar time signatures don't exist in the model.
- **Key.** `Option<Key>` (`crates/core/src/chord.rs`: `root_pc`, `Scale::Major |
  NaturalMinor`). `None` (a legacy piece) means C major.
- **Hands.** `Note::effective_hand(split)` (`crates/core/src/timeline.rs`) gives
  the hand: `Hand::Right` → staff 1 (treble), `Hand::Left` → staff 2 (bass).
- **Pickups.** Imported charts often have notes before the first mapped
  downbeat. `Composer::map_snap` extends bar 0's spacing backwards for them;
  this spec does the same.
- Reference scores to sanity-check against by hand: `fixtures/score/*.musicxml`
  (e.g. `two_staff_chords`, `tied_note`, `single_staff_scale`). Import one,
  export it with M22-B, and compare.

## What to do

New module `crates/core/src/notation.rs`, exported from `lib.rs`. Pure: no I/O,
no new dependencies.

### Types

```rust
/// Ticks per quarter note. 8 makes a 32nd note one tick.
pub const DIVISIONS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BaseValue { Whole, Half, Quarter, Eighth, Sixteenth, ThirtySecond }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NoteValue { pub base: BaseValue, pub dotted: bool }
impl NoteValue { pub fn ticks(self) -> u32; } // Whole = 32, dotted = ×1.5

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Step { C, D, E, F, G, A, B }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Accidental { Sharp, Flat, Natural }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WrittenNote {
    pub pitch: u8,           // MIDI pitch (sounding)
    pub step: Step,
    pub alter: i8,           // -1, 0, +1
    pub octave: i8,          // scientific: MIDI 60 = C4
    /// The accidental to print, if one is needed (see "Accidentals").
    pub accidental: Option<Accidental>,
    pub tie_start: bool,     // tied to the same pitch in the next event
    pub tie_stop: bool,      // continues a tie from the previous event
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EventKind {
    Rest,
    /// A whole-bar rest, printed as a centred whole rest whatever the metre.
    BarRest,
    /// One or more notes sharing onset and written value, ascending by pitch.
    Chord(Vec<WrittenNote>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub start: u32,          // ticks from the bar's downbeat
    pub value: NoteValue,    // BarRest: Whole (writers use bar_ticks as its duration)
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Measure {
    pub number: u32,                 // 1-based, consecutive
    /// Quarter-note BPM mark printed at this bar's downbeat, if any.
    pub tempo: Option<u32>,
    /// `staves[0]` = treble (right hand), `staves[1]` = bass (left hand).
    /// Each staff's events tile the bar exactly: starts ascending, no gaps,
    /// no overlaps, and the tick lengths sum to `Score::bar_ticks`.
    pub staves: [Vec<Event>; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Score {
    pub time_sig: TimeSig,
    pub bar_ticks: u32,          // beats_per_bar · DIVISIONS · 4 / beat_unit
    pub key_fifths: i8,          // -7..=7, MusicXML <fifths>
    pub minor: bool,
    pub measures: Vec<Measure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotationOptions {
    /// Quantisation step. Triplet subdivisions fall back to `Sixteenth` in v1.
    pub quantum: Subdivision,
}
impl Default for NotationOptions { /* Sixteenth */ }

pub fn build_score(
    notes: &[Note],
    grid: &Grid,
    bar_starts: &[u64],
    key: Option<Key>,
    hand_split: u8,
    opts: NotationOptions,
) -> Score;
```

Also add a convenience `Composer::score(&self, key: Option<Key>, opts)` only if
the composer doesn't expose everything `build_score` needs. Otherwise frontends
call `build_score` with the snapshot's pieces.

### 1. Bars and range

- Bar `b` spans `[bar_start(b), bar_start(b+1))` from the `BarMap`.
- **Pickup:** if any note starts before `bar_start(0)`, prepend bars before it
  with bar 0's length, as many as the earliest note needs. They're written as
  full bars; the leading part is rests.
- Export from the bar containing the **first** note's onset through the bar
  containing the **last** note's (quantised) end. Leading empty bars, such as a
  backing-only intro, are skipped. Measures are numbered 1.. from there.
- No notes → a `Score` with zero measures.

### 2. Quantisation

- `q` = quantum in ticks: Quarter 8, Eighth 4, Sixteenth 2, ThirtySecond 1.
  `EighthTriplet` / `SixteenthTriplet` → Sixteenth.
- Map a time `t` inside bar `b` to ticks: `bar_ticks · (t − start) / len`,
  rounded to the nearest multiple of `q`. A tick equal to `bar_ticks` rolls
  over to tick 0 of bar `b+1`. Work in absolute ticks (`bar_index · bar_ticks
  + tick`) from here on.
- A note's onset and end are quantised independently. If the end ≤ onset after
  rounding, the note lasts one quantum.

### 3. Staves and chords (one voice per staff)

Per staff, using `effective_hand(hand_split)`:

- Notes with the **same quantised onset** form one chord. Duplicate pitches at
  the same onset collapse to one.
- A chord's written end = the latest end among its notes, **cut at the next
  onset in the same staff**. v1 has one voice per staff, so a held note under
  a moving melody gets shortened. That's a known simplification (see Scope).
- Gaps between chords (and from the bar start to the first chord, and after
  the last) are rests.

### 4. Splitting into printable values

Every chord or rest span `[s, e)` in absolute ticks is cut at bar lines
first. Then each in-bar piece is split greedily, left to right, taking the
**largest** allowed value from `32, 24, 16, 12, 8, 6, 4, 3, 2, 1` ticks (whole,
dotted half, half, dotted quarter, quarter, dotted eighth, eighth, dotted 16th,
16th, 32nd). Below, positions are ticks from the downbeat, and
`beat = DIVISIONS · 4 / beat_unit`.

**Notes (chords).** A value of length `len` with undotted base `base` may
start at `p` when:

1. it fits in what's left of the span;
2. `p` is a multiple of `min(base, beat)`;
3. if it spans more than one beat (crosses a multiple of `beat`), it both
   starts **and** ends on a beat.

This writes syncopations in the textbook way: a half-note-long note from
the "and" of beat 1 in 4/4 becomes eighth ⁀ quarter ⁀ eighth. Don't attempt
beaming or other engraving rules.

**Rests.** Undotted values only, and a rest of length `len` may start only
at a multiple of `len` itself (no beat cap). So a rest never hides a strong
beat: the rest after a quarter on beat 1 of 4/4 is quarter + half, and in
3/4 it's quarter + quarter. An entirely empty bar on a staff is one
`BarRest` instead.

When a chord is split into several events, each of its notes gets
`tie_start` on every piece but the last and `tie_stop` on every piece but
the first.

### 5. Spelling and key

- `key_fifths` from the key's major tonic (a minor key uses its relative major,
  `root_pc + 3`): pc → fifths `0:0, 1:-5, 2:2, 3:-3, 4:4, 5:-1, 6:6, 7:1,
  8:-4, 9:3, 10:-2, 11:5`. `minor` = `scale == NaturalMinor`. `None` → C major.
- Spell each pitch: pitch classes in the key's scale use the key's spelling
  (e.g. F♯ in G major, B♭ in F major, F♯ in E minor). Chromatic pitches use
  sharps when `key_fifths >= 0`, flats otherwise. Octave: `pitch / 12 − 1`,
  with B♯/C♭ adjusting the octave so that `step`, `alter` and `octave` always
  sound as `pitch`.

### 6. Accidentals

Per staff and per bar, track the alteration in effect for each `(step,
octave)`. It starts from the key signature and resets every bar. A note
prints an accidental when its `alter` differs from that state: `Sharp`,
`Flat`, or `Natural` when returning to 0. The note then updates the state. A
`tie_stop` note never prints one but still updates the state.

### 7. Tempo marks

- Measure 1 gets the rounded quarter-note BPM of its bar (from the map, or
  `grid.bpm` when uniform).
- Each `TempoChange` from `tempo_changes(..)` whose `at_us` is a bar inside
  the export range marks that measure with `bpm.round()`, unless it rounds to
  the same value as the last mark.

## Tests

In `crates/core/src/notation.rs`, building notes by hand, with 4/4 at 120 BPM
(bar = 2 s, quarter = 500 ms) and a uniform grid unless stated:

1. **Empty** piece → zero measures.
2. **One quarter note C4 on beat 1** → one measure: treble is exactly
   `[quarter C4, quarter rest, half rest]`, bass is `BarRest`.
3. **Quantisation:** onset 30 ms late and end 40 ms early still produce a
   clean quarter.
4. **Tie across the bar line:** a half note starting on beat 4 → quarter
   (`tie_start`) in bar 1 + quarter (`tie_stop`) in bar 2.
5. **Chord:** C4, E4, G4 at the same onset → one `Chord` with 3 ascending notes.
6. **Hands:** pitch 48 with split 60 lands on staff 2, pitch 72 on staff 1. An
   override `hand: Some(Right)` on pitch 48 moves it to staff 1.
7. **Overlap in one staff:** a whole note C5 plus a quarter E5 on beat 2 → the
   C5 is cut to a quarter (documented v1 behaviour).
8. **Alignment:** a note of 3 quarters starting on beat 2 → one dotted half
   (starts and ends on a beat). A half-note-long note starting on the "and"
   of beat 1 → eighth ⁀ quarter ⁀ eighth.
9. **Spelling:** G major: pitch 66 → F♯ with no accidental. F major: pitch 70
   → B♭ with no accidental. C major: 61 → C♯ with `Sharp`, then 60 in the same
   bar → C with `Natural`. A minor → `key_fifths = 0`, `minor = true`.
   D♭ major (`root_pc` 1) → fifths −5 and pitch 61 spelt D♭.
10. **Accidentals reset per bar**, and a tied continuation shows none.
11. **Tempo map:** bar starts at 0, 2 s, 4 s, 5.5 s, 7 s (bars 3 and 4 are
    faster), one note per bar → measure 1 is marked 120, measure 3 is marked
    160, and measures 2 and 4 have no mark.
12. **Pickup:** grid origin 1 s, one note at 0.5 s → a prepended measure
    whose treble is rests and then the note on beat 4 (tick 24).
13. **Skip intro:** first note in bar 5 → measure 1 is that bar.
14. **3/4 and 6/8** both give `bar_ticks` 24. Every staff in every measure
    tiles the bar exactly; assert this invariant in all tests with a
    helper.

## Scope boundaries (do NOT)

- No file format, writer or I/O (M22-B). No frontend changes.
- No tuplets (triplet quanta fall back to Sixteenth), multiple voices per
  staff, grace notes, pedal, dynamics/velocity, articulations, beaming,
  per-bar time signatures, or clef changes. They may be follow-ups.
- Don't change `Note`, `Grid`, `BarMap` or any existing public signature.
- No new dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #329`
