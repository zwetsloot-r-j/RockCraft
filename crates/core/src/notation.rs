//! Notation model (M22-A): a piece as measures, staves and note values.
//!
//! A piece stores notes as microsecond times. A printed score needs bars, two
//! staves, note values, rests, ties, chords, pitch spelling and accidentals.
//! [`build_score`] derives all of that; a writer (M22-B's MusicXML) only has to
//! serialise the [`Score`].
//!
//! The model is deliberately simple — one voice per staff, no tuplets, no
//! beaming — and pure, like the rest of `core`.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::bars::{tempo_changes, BarMap};
use crate::chord::{Key, Scale};
use crate::finger::Finger;
use crate::grid::{Grid, Subdivision, TimeSig};
use crate::hand::Hand;
use crate::timeline::Note;

/// Ticks per quarter note. 8 makes a 32nd note one tick.
pub const DIVISIONS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BaseValue {
    Whole,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
}

impl BaseValue {
    /// Length in ticks, undotted.
    pub const fn ticks(self) -> u32 {
        match self {
            BaseValue::Whole => 32,
            BaseValue::Half => 16,
            BaseValue::Quarter => 8,
            BaseValue::Eighth => 4,
            BaseValue::Sixteenth => 2,
            BaseValue::ThirtySecond => 1,
        }
    }
}

/// A printable note or rest length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NoteValue {
    pub base: BaseValue,
    pub dotted: bool,
}

impl NoteValue {
    /// Length in ticks: `Whole` = 32, a dot adds half again.
    pub const fn ticks(self) -> u32 {
        let base = self.base.ticks();
        if self.dotted {
            base + base / 2
        } else {
            base
        }
    }

    /// The value exactly `ticks` long, if there is one (dotted 32nds excluded).
    fn from_ticks(ticks: u32) -> Option<NoteValue> {
        let (base, dotted) = match ticks {
            32 => (BaseValue::Whole, false),
            24 => (BaseValue::Half, true),
            16 => (BaseValue::Half, false),
            12 => (BaseValue::Quarter, true),
            8 => (BaseValue::Quarter, false),
            6 => (BaseValue::Eighth, true),
            4 => (BaseValue::Eighth, false),
            3 => (BaseValue::Sixteenth, true),
            2 => (BaseValue::Sixteenth, false),
            1 => (BaseValue::ThirtySecond, false),
            _ => return None,
        };
        Some(NoteValue { base, dotted })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Step {
    C,
    D,
    E,
    F,
    G,
    A,
    B,
}

impl Step {
    const ALL: [Step; 7] = [
        Step::C,
        Step::D,
        Step::E,
        Step::F,
        Step::G,
        Step::A,
        Step::B,
    ];

    /// Pitch class of the natural note.
    const fn natural_pc(self) -> i32 {
        match self {
            Step::C => 0,
            Step::D => 2,
            Step::E => 4,
            Step::F => 5,
            Step::G => 7,
            Step::A => 9,
            Step::B => 11,
        }
    }

    /// The letter, as MusicXML's `<step>` wants it.
    pub const fn letter(self) -> char {
        match self {
            Step::C => 'C',
            Step::D => 'D',
            Step::E => 'E',
            Step::F => 'F',
            Step::G => 'G',
            Step::A => 'A',
            Step::B => 'B',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Accidental {
    Sharp,
    Flat,
    Natural,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WrittenNote {
    /// MIDI pitch (sounding).
    pub pitch: u8,
    pub step: Step,
    /// -1, 0, +1.
    pub alter: i8,
    /// Scientific octave: MIDI 60 = C4.
    pub octave: i8,
    /// The accidental to print, if one is needed.
    pub accidental: Option<Accidental>,
    /// Tied to the same pitch in the next event.
    pub tie_start: bool,
    /// Continues a tie from the previous event.
    pub tie_stop: bool,
    /// Finger number (M27-F), on the first written piece of a note only — a
    /// tied continuation never repeats it.
    pub finger: Option<Finger>,
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
    /// Ticks from the bar's downbeat.
    pub start: u32,
    /// For a `BarRest` this is `Whole`; writers use `Score::bar_ticks` as its
    /// duration.
    pub value: NoteValue,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Measure {
    /// 1-based, consecutive.
    pub number: u32,
    /// Quarter-note BPM mark printed at this bar's downbeat, if any.
    pub tempo: Option<u32>,
    /// `staves[0]` = treble (right hand), `staves[1]` = bass (left hand).
    /// Each staff's events tile the bar exactly: starts ascending, no gaps, no
    /// overlaps, and the tick lengths sum to `Score::bar_ticks`.
    pub staves: [Vec<Event>; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Score {
    pub time_sig: TimeSig,
    /// `beats_per_bar · DIVISIONS · 4 / beat_unit`.
    pub bar_ticks: u32,
    /// -7..=7, MusicXML `<fifths>`.
    pub key_fifths: i8,
    pub minor: bool,
    pub measures: Vec<Measure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotationOptions {
    /// Quantisation step. Triplet subdivisions fall back to `Sixteenth` in v1.
    pub quantum: Subdivision,
}

impl Default for NotationOptions {
    fn default() -> Self {
        Self {
            quantum: Subdivision::Sixteenth,
        }
    }
}

// ── bars, including pickup bars before bar 0 ────────────────────────────────

/// The piece's bars with a signed index: bars before bar 0 (a pickup) repeat
/// bar 0's length backwards.
struct Bars<'a> {
    map: BarMap<'a>,
}

impl Bars<'_> {
    fn start(&self, bar: i64) -> i64 {
        if bar >= 0 {
            self.map.bar_start(bar as u64) as i64
        } else {
            self.map.bar_start(0) as i64 + bar * self.map.bar_len(0) as i64
        }
    }

    fn len(&self, bar: i64) -> i64 {
        self.map.bar_len(bar.max(0) as u64) as i64
    }

    fn bar_of(&self, us: u64) -> i64 {
        let first = self.map.bar_start(0);
        if us >= first {
            self.map.bar_at(us) as i64
        } else {
            let len0 = self.map.bar_len(0);
            -((first - us).div_ceil(len0) as i64)
        }
    }

    /// `us` as absolute ticks (`bar · bar_ticks + tick`), rounded to the
    /// nearest multiple of `q` within its bar.
    fn quantise(&self, us: u64, bar_ticks: i64, q: i64) -> i64 {
        let bar = self.bar_of(us);
        let offset = (us as i64 - self.start(bar)).max(0) as i128;
        let len = self.len(bar) as i128;
        let (bt, q) = (bar_ticks as i128, q as i128);
        // round(bar_ticks · offset / (len · q)) quanta, in integers.
        let quanta = (2 * bt * offset + len * q) / (2 * len * q);
        let tick = ((quanta * q) as i64).min(bar_ticks);
        bar * bar_ticks + tick
    }
}

// ── spelling ────────────────────────────────────────────────────────────────

/// `<fifths>` for a major tonic pitch class.
const FIFTHS_OF_PC: [i8; 12] = [0, -5, 2, -3, 4, -1, 6, 1, -4, 3, -2, 5];

/// How a key spells pitches.
struct Speller {
    fifths: i8,
    /// The key signature's alteration for each step, indexed like `Step::ALL`.
    key_alter: [i8; 7],
}

impl Speller {
    fn new(fifths: i8) -> Self {
        // Sharps arrive F C G D A E B; flats B E A D G C F.
        const SHARPS: [usize; 7] = [3, 0, 4, 1, 5, 2, 6];
        const FLATS: [usize; 7] = [6, 2, 5, 1, 4, 0, 3];
        let mut key_alter = [0i8; 7];
        for &step in SHARPS.iter().take(fifths.max(0) as usize) {
            key_alter[step] = 1;
        }
        for &step in FLATS.iter().take((-fifths).max(0) as usize) {
            key_alter[step] = -1;
        }
        Self { fifths, key_alter }
    }

    /// `(step, alter, octave)` for a MIDI pitch: the key's own spelling for a
    /// pitch in its scale, else sharps in a sharp (or neutral) key and flats in
    /// a flat one.
    fn spell(&self, pitch: u8) -> (Step, i8, i8) {
        let pc = i32::from(pitch) % 12;
        let diatonic = Step::ALL.iter().enumerate().find_map(|(i, &step)| {
            let alter = self.key_alter[i];
            ((step.natural_pc() + i32::from(alter)).rem_euclid(12) == pc).then_some((step, alter))
        });
        let (step, alter) = diatonic.unwrap_or_else(|| {
            let natural = Step::ALL.iter().find(|s| s.natural_pc() == pc);
            match natural {
                Some(&step) => (step, 0),
                None if self.fifths >= 0 => {
                    let below = Step::ALL
                        .iter()
                        .find(|s| s.natural_pc() == pc - 1)
                        .expect("a black key has a white key a semitone below");
                    (*below, 1)
                }
                None => {
                    let above = Step::ALL
                        .iter()
                        .find(|s| s.natural_pc() == pc + 1)
                        .expect("a black key has a white key a semitone above");
                    (*above, -1)
                }
            }
        });
        // The octave that makes step + alter sound as `pitch` (B♯ / C♭ shift it).
        let octave = (i32::from(pitch) - i32::from(alter) - step.natural_pc()) / 12 - 1;
        (step, alter, octave as i8)
    }

    fn key_alter_of(&self, step: Step) -> i8 {
        self.key_alter[Step::ALL.iter().position(|&s| s == step).unwrap_or(0)]
    }
}

// ── splitting spans into printable values ───────────────────────────────────

const NOTE_LENGTHS: [u32; 10] = [32, 24, 16, 12, 8, 6, 4, 3, 2, 1];
const REST_LENGTHS: [u32; 6] = [32, 16, 8, 4, 2, 1];

/// Split an in-bar note span `[start, end)` (ticks from the downbeat) into
/// values, largest first, so that syncopations are written the textbook way.
fn split_note(start: u32, end: u32, beat: u32) -> Vec<(u32, NoteValue)> {
    let mut out = Vec::new();
    let mut p = start;
    while p < end {
        let left = end - p;
        let len = NOTE_LENGTHS
            .iter()
            .copied()
            .find(|&len| {
                let value = NoteValue::from_ticks(len).expect("table lengths are values");
                let grid = value.base.ticks().min(beat);
                // Crossing a beat = a beat line strictly inside (p, p + len).
                let crosses = (p / beat) != ((p + len - 1) / beat);
                len <= left
                    && p.is_multiple_of(grid)
                    && (!crosses || (p.is_multiple_of(beat) && (p + len).is_multiple_of(beat)))
            })
            .unwrap_or(1);
        out.push((
            p,
            NoteValue::from_ticks(len).expect("table lengths are values"),
        ));
        p += len;
    }
    out
}

/// Split an in-bar rest span: undotted values, each starting on a multiple of
/// its own length, so a rest never hides a strong beat.
fn split_rest(start: u32, end: u32) -> Vec<(u32, NoteValue)> {
    let mut out = Vec::new();
    let mut p = start;
    while p < end {
        let left = end - p;
        let len = REST_LENGTHS
            .iter()
            .copied()
            .find(|&len| len <= left && p.is_multiple_of(len))
            .unwrap_or(1);
        out.push((
            p,
            NoteValue::from_ticks(len).expect("table lengths are values"),
        ));
        p += len;
    }
    out
}

// ── building ────────────────────────────────────────────────────────────────

/// A quantised note: `(onset, end, pitch, finger)`, times in absolute ticks.
type QuantisedNote = (i64, i64, u8, Option<Finger>);

/// One chord of a staff, in absolute ticks.
struct ChordSpan {
    start: i64,
    end: i64,
    /// `(pitch, finger)`, ascending by pitch, pitches unique.
    notes: Vec<(u8, Option<Finger>)>,
}

/// The chords of one staff: same-onset notes merged, each cut at the next onset.
fn staff_chords(notes: &[QuantisedNote]) -> Vec<ChordSpan> {
    // onset -> (latest end, pitch -> finger; None once two notes disagree).
    let mut by_onset: BTreeMap<i64, (i64, BTreeMap<u8, Option<Finger>>)> = BTreeMap::new();
    for &(start, end, pitch, finger) in notes {
        let entry = by_onset.entry(start).or_insert((end, BTreeMap::new()));
        entry.0 = entry.0.max(end);
        entry
            .1
            .entry(pitch)
            .and_modify(|seen| {
                // Duplicate pitch: keep a finger only if the copies don't clash.
                *seen = match (*seen, finger) {
                    (Some(a), Some(b)) if a != b => None,
                    (a, b) => a.or(b),
                };
            })
            .or_insert(finger);
    }
    let onsets: Vec<i64> = by_onset.keys().copied().collect();
    by_onset
        .into_iter()
        .enumerate()
        .map(|(i, (start, (end, pitches)))| ChordSpan {
            start,
            end: onsets.get(i + 1).map_or(end, |&next| end.min(next)),
            notes: pitches.into_iter().collect(),
        })
        .collect()
}

/// Derive the printable [`Score`] of a piece.
///
/// `bar_starts` is the piece's tempo map (empty = the uniform `grid`), `key`
/// its key (`None` = C major) and `hand_split` its split line; each note goes
/// to the staff of its effective hand.
pub fn build_score(
    notes: &[Note],
    grid: &Grid,
    bar_starts: &[u64],
    key: Option<Key>,
    hand_split: u8,
    opts: NotationOptions,
) -> Score {
    let time_sig = grid.time_sig;
    let beat = (DIVISIONS * 4 / u32::from(time_sig.beat_unit.max(1))).max(1);
    let bar_ticks = u32::from(time_sig.beats_per_bar.max(1)) * beat;

    let (root_pc, minor) = match key {
        Some(k) => (k.root_pc % 12, k.scale == Scale::NaturalMinor),
        None => (0, false),
    };
    let major_pc = if minor { (root_pc + 3) % 12 } else { root_pc };
    let key_fifths = FIFTHS_OF_PC[usize::from(major_pc)];
    let speller = Speller::new(key_fifths);

    let mut score = Score {
        time_sig,
        bar_ticks,
        key_fifths,
        minor,
        measures: Vec::new(),
    };
    if notes.is_empty() {
        return score;
    }

    let bars = Bars {
        map: BarMap::new(bar_starts, grid.bar_us()).with_origin(grid.origin_us),
    };
    let q: i64 = match opts.quantum {
        Subdivision::Quarter => 8,
        Subdivision::Eighth => 4,
        Subdivision::ThirtySecond => 1,
        Subdivision::Sixteenth | Subdivision::EighthTriplet | Subdivision::SixteenthTriplet => 2,
    };
    let bt = i64::from(bar_ticks);

    // Quantise every note and deal it to its staff.
    let mut per_staff: [Vec<QuantisedNote>; 2] = [Vec::new(), Vec::new()];
    for note in notes {
        let start = bars.quantise(note.start_us, bt, q);
        let mut end = bars.quantise(note.start_us.saturating_add(note.dur_us), bt, q);
        if end <= start {
            end = start + q;
        }
        let staff = match note.effective_hand(hand_split) {
            Hand::Right => 0,
            Hand::Left => 1,
        };
        per_staff[staff].push((start, end, note.pitch.value(), note.finger));
    }
    let chords: [Vec<ChordSpan>; 2] = [staff_chords(&per_staff[0]), staff_chords(&per_staff[1])];

    // The bars to write: from the first onset's bar through the last end's.
    let first_tick = chords.iter().flatten().map(|c| c.start).min().unwrap_or(0);
    let last_tick = chords.iter().flatten().map(|c| c.end).max().unwrap_or(0);
    let first_bar = first_tick.div_euclid(bt);
    let last_bar = (last_tick - 1).div_euclid(bt).max(first_bar);
    let bar_count = (last_bar - first_bar + 1) as usize;

    let mut staves: Vec<[Vec<Event>; 2]> =
        (0..bar_count).map(|_| [Vec::new(), Vec::new()]).collect();
    for (staff, staff_chords) in chords.iter().enumerate() {
        // Walk the staff as alternating rest / chord spans over the whole range.
        let range_end = (last_bar + 1) * bt;
        let mut cursor = first_bar * bt;
        let mut spans: Vec<(i64, i64, Option<&ChordSpan>)> = Vec::new();
        for chord in staff_chords {
            if chord.start > cursor {
                spans.push((cursor, chord.start, None));
            }
            spans.push((chord.start, chord.end, Some(chord)));
            cursor = chord.end;
        }
        if cursor < range_end {
            spans.push((cursor, range_end, None));
        }

        for (start, end, chord) in spans {
            // Cut at bar lines, then split each in-bar piece into values.
            let mut pieces: Vec<(usize, u32, NoteValue)> = Vec::new();
            let mut at = start;
            while at < end {
                let bar = at.div_euclid(bt);
                let piece_end = end.min((bar + 1) * bt);
                let (s, e) = ((at - bar * bt) as u32, (piece_end - bar * bt) as u32);
                let index = (bar - first_bar) as usize;
                match chord {
                    Some(_) => pieces.extend(
                        split_note(s, e, beat)
                            .into_iter()
                            .map(|(p, v)| (index, p, v)),
                    ),
                    None if s == 0 && e == bar_ticks => staves[index][staff].push(Event {
                        start: 0,
                        value: NoteValue {
                            base: BaseValue::Whole,
                            dotted: false,
                        },
                        kind: EventKind::BarRest,
                    }),
                    None => {
                        for (p, value) in split_rest(s, e) {
                            staves[index][staff].push(Event {
                                start: p,
                                value,
                                kind: EventKind::Rest,
                            });
                        }
                    }
                }
                at = piece_end;
            }
            let Some(chord) = chord else { continue };
            let last = pieces.len().saturating_sub(1);
            for (i, (index, p, value)) in pieces.into_iter().enumerate() {
                let written = chord
                    .notes
                    .iter()
                    .map(|&(pitch, finger)| {
                        let (step, alter, octave) = speller.spell(pitch);
                        WrittenNote {
                            pitch,
                            step,
                            alter,
                            octave,
                            accidental: None, // filled in below, bar by bar
                            tie_start: i < last,
                            tie_stop: i > 0,
                            finger: if i == 0 { finger } else { None },
                        }
                    })
                    .collect();
                staves[index][staff].push(Event {
                    start: p,
                    value,
                    kind: EventKind::Chord(written),
                });
            }
        }
    }

    // Accidentals: per staff and bar, against the key signature.
    for bar in &mut staves {
        for events in bar.iter_mut() {
            let mut state: HashMap<(Step, i8), i8> = HashMap::new();
            for event in events.iter_mut() {
                let EventKind::Chord(notes) = &mut event.kind else {
                    continue;
                };
                for note in notes.iter_mut() {
                    let in_effect = state
                        .get(&(note.step, note.octave))
                        .copied()
                        .unwrap_or_else(|| speller.key_alter_of(note.step));
                    if note.alter != in_effect && !note.tie_stop {
                        note.accidental = Some(match note.alter {
                            1 => Accidental::Sharp,
                            -1 => Accidental::Flat,
                            _ => Accidental::Natural,
                        });
                    }
                    state.insert((note.step, note.octave), note.alter);
                }
            }
        }
    }

    // Tempo marks: the first measure, then every checkpoint of the map.
    let quarter_bpm = |bar: i64| -> u32 {
        let k = 240_000_000f64 * f64::from(time_sig.beats_per_bar)
            / f64::from(time_sig.beat_unit.max(1));
        (k / bars.len(bar) as f64).round() as u32
    };
    let mut tempo_at: BTreeMap<i64, u32> = BTreeMap::new();
    tempo_at.insert(first_bar, quarter_bpm(first_bar));
    for change in tempo_changes(bar_starts, time_sig.beats_per_bar, time_sig.beat_unit) {
        let bar = bars.bar_of(change.at_us);
        if bar > first_bar && bar <= last_bar {
            tempo_at.insert(bar, change.bpm.round() as u32);
        }
    }
    let mut last_mark = None;
    score.measures = staves
        .into_iter()
        .enumerate()
        .map(|(i, staves)| {
            let mark = tempo_at.get(&(first_bar + i as i64)).copied();
            let tempo = mark.filter(|&bpm| last_mark != Some(bpm));
            if tempo.is_some() {
                last_mark = tempo;
            }
            Measure {
                number: i as u32 + 1,
                tempo,
                staves,
            }
        })
        .collect();
    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MidiNote, Velocity};
    use crate::hand::DEFAULT_SPLIT;

    const Q: u64 = 500_000; // a quarter note at 120 BPM

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
            hand: None,
            finger: None,
        }
    }

    fn key(root_pc: u8, scale: Scale) -> Option<Key> {
        Some(Key { root_pc, scale })
    }

    fn build_with(notes: &[Note], grid: &Grid, bars: &[u64], key: Option<Key>) -> Score {
        let score = build_score(
            notes,
            grid,
            bars,
            key,
            DEFAULT_SPLIT,
            NotationOptions::default(),
        );
        assert_tiles(&score);
        score
    }

    fn build(notes: &[Note]) -> Score {
        build_with(notes, &Grid::default_120(), &[], None)
    }

    /// The invariant every score must hold: each staff of each measure tiles
    /// the bar exactly.
    fn assert_tiles(score: &Score) {
        for m in &score.measures {
            for (s, events) in m.staves.iter().enumerate() {
                let mut at = 0;
                for e in events {
                    assert_eq!(
                        e.start, at,
                        "measure {} staff {s}: gap or overlap",
                        m.number
                    );
                    at += match e.kind {
                        EventKind::BarRest => score.bar_ticks,
                        _ => e.value.ticks(),
                    };
                }
                assert_eq!(
                    at, score.bar_ticks,
                    "measure {} staff {s}: not a full bar",
                    m.number
                );
            }
        }
        for (i, m) in score.measures.iter().enumerate() {
            assert_eq!(m.number, i as u32 + 1);
        }
    }

    /// A staff as compact text: `q:C4` (quarter C4), `h:r` (half rest), `bar`
    /// (bar rest); `.` marks a dot, `~` a tie to the next event.
    fn show(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                let v = match e.value.base {
                    BaseValue::Whole => "w",
                    BaseValue::Half => "h",
                    BaseValue::Quarter => "q",
                    BaseValue::Eighth => "e",
                    BaseValue::Sixteenth => "s",
                    BaseValue::ThirtySecond => "t",
                };
                let dot = if e.value.dotted { "." } else { "" };
                match &e.kind {
                    EventKind::BarRest => "bar".to_string(),
                    EventKind::Rest => format!("{v}{dot}:r"),
                    EventKind::Chord(notes) => {
                        let names: Vec<String> = notes
                            .iter()
                            .map(|n| {
                                let acc = match n.alter {
                                    1 => "#",
                                    -1 => "b",
                                    _ => "",
                                };
                                let tie = if n.tie_start { "~" } else { "" };
                                format!("{}{acc}{}{tie}", n.step.letter(), n.octave)
                            })
                            .collect();
                        format!("{v}{dot}:{}", names.join("+"))
                    }
                }
            })
            .collect()
    }

    fn chord_notes(event: &Event) -> &[WrittenNote] {
        match &event.kind {
            EventKind::Chord(notes) => notes,
            other => panic!("expected a chord, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_piece_has_no_measures() {
        let score = build(&[]);
        assert!(score.measures.is_empty());
        assert_eq!(score.bar_ticks, 32);
    }

    #[test]
    fn one_quarter_note_on_beat_one() {
        let score = build(&[note(60, 0, Q)]);
        assert_eq!(score.measures.len(), 1);
        assert_eq!(show(&score.measures[0].staves[0]), ["q:C4", "q:r", "h:r"]);
        assert_eq!(show(&score.measures[0].staves[1]), ["bar"]);
    }

    #[test]
    fn sloppy_timing_still_gives_a_clean_quarter() {
        // 30 ms late, ending 40 ms early.
        let score = build(&[note(60, 30_000, Q - 70_000)]);
        assert_eq!(show(&score.measures[0].staves[0]), ["q:C4", "q:r", "h:r"]);
    }

    #[test]
    fn a_note_is_tied_across_the_bar_line() {
        let score = build(&[note(60, 3 * Q, 2 * Q)]);
        assert_eq!(score.measures.len(), 2);
        assert_eq!(show(&score.measures[0].staves[0]), ["h:r", "q:r", "q:C4~"]);
        assert_eq!(show(&score.measures[1].staves[0]), ["q:C4", "q:r", "h:r"]);
        let second = &chord_notes(&score.measures[1].staves[0][0])[0];
        assert!(second.tie_stop && !second.tie_start);
        let first = &chord_notes(&score.measures[0].staves[0][2])[0];
        assert!(first.tie_start && !first.tie_stop);
    }

    #[test]
    fn same_onset_notes_form_one_ascending_chord() {
        let score = build(&[
            note(67, 0, Q),
            note(60, 0, Q),
            note(64, 0, Q),
            note(64, 0, Q),
        ]);
        assert_eq!(show(&score.measures[0].staves[0])[0], "q:C4+E4+G4");
    }

    #[test]
    fn hands_pick_the_staff() {
        let mut crossover = note(48, 0, Q);
        let score = build(&[crossover, note(72, 0, Q)]);
        assert_eq!(show(&score.measures[0].staves[0])[0], "q:C5");
        assert_eq!(show(&score.measures[0].staves[1])[0], "q:C3");

        crossover.hand = Some(Hand::Right);
        let score = build(&[crossover, note(72, 0, Q)]);
        assert_eq!(show(&score.measures[0].staves[0])[0], "q:C3+C5");
        assert_eq!(show(&score.measures[0].staves[1]), ["bar"]);
    }

    #[test]
    fn an_overlapping_note_is_cut_at_the_next_onset() {
        // One voice per staff: the whole note gives way to the quarter on beat 2.
        let score = build(&[note(72, 0, 4 * Q), note(76, Q, Q)]);
        assert_eq!(show(&score.measures[0].staves[0]), ["q:C5", "q:E5", "h:r"]);
    }

    #[test]
    fn values_align_to_the_beat() {
        // Three quarters from beat 2: starts and ends on a beat → one dotted half.
        let score = build(&[note(60, Q, 3 * Q)]);
        assert_eq!(show(&score.measures[0].staves[0]), ["q:r", "h.:C4"]);
        // A half-note length from the "and" of 1: eighth ⁀ quarter ⁀ eighth.
        let score = build(&[note(60, Q / 2, 2 * Q)]);
        assert_eq!(
            show(&score.measures[0].staves[0]),
            ["e:r", "e:C4~", "q:C4~", "e:C4", "e:r", "q:r"]
        );
    }

    #[test]
    fn pitches_are_spelt_in_the_key() {
        let g = Grid::default_120();
        // G major: F♯ is in the key — no accidental.
        let score = build_with(&[note(66, 0, Q)], &g, &[], key(7, Scale::Major));
        assert_eq!(score.key_fifths, 1);
        let n = &chord_notes(&score.measures[0].staves[0][0])[0];
        assert_eq!(
            (n.step, n.alter, n.octave, n.accidental),
            (Step::F, 1, 4, None)
        );

        // F major: B♭, no accidental.
        let score = build_with(&[note(70, 0, Q)], &g, &[], key(5, Scale::Major));
        assert_eq!(score.key_fifths, -1);
        let n = &chord_notes(&score.measures[0].staves[0][0])[0];
        assert_eq!((n.step, n.alter, n.accidental), (Step::B, -1, None));

        // C major: C♯ needs a sharp, and C after it in the bar a natural.
        let score = build(&[note(61, 0, Q), note(60, Q, Q)]);
        let sharp = &chord_notes(&score.measures[0].staves[0][0])[0];
        let natural = &chord_notes(&score.measures[0].staves[0][1])[0];
        assert_eq!(
            (sharp.step, sharp.alter, sharp.accidental),
            (Step::C, 1, Some(Accidental::Sharp))
        );
        assert_eq!(
            (natural.alter, natural.accidental),
            (0, Some(Accidental::Natural))
        );

        // A minor shares C major's signature.
        let score = build_with(&[note(69, 0, Q)], &g, &[], key(9, Scale::NaturalMinor));
        assert_eq!((score.key_fifths, score.minor), (0, true));

        // D♭ major: five flats, and 61 is D♭.
        let score = build_with(&[note(61, 0, Q)], &g, &[], key(1, Scale::Major));
        assert_eq!(score.key_fifths, -5);
        let n = &chord_notes(&score.measures[0].staves[0][0])[0];
        assert_eq!(
            (n.step, n.alter, n.octave, n.accidental),
            (Step::D, -1, 4, None)
        );
    }

    #[test]
    fn c_flat_and_b_sharp_land_in_the_right_octave() {
        let flats = Speller::new(-7);
        assert_eq!(flats.spell(59), (Step::C, -1, 4));
        let sharps = Speller::new(7);
        assert_eq!(sharps.spell(60), (Step::B, 1, 3));
        // And every pitch always sounds as written, in every key.
        for fifths in -7..=7 {
            let sp = Speller::new(fifths);
            for pitch in 21..=108u8 {
                let (step, alter, octave) = sp.spell(pitch);
                let sounding = (i32::from(octave) + 1) * 12 + step.natural_pc() + i32::from(alter);
                assert_eq!(sounding, i32::from(pitch), "fifths {fifths} pitch {pitch}");
            }
        }
    }

    #[test]
    fn accidentals_reset_each_bar_and_ties_show_none() {
        // C♯ in bar 1, again in bar 2: both print. C♯ tied over the bar: the
        // continuation prints none.
        let score = build(&[note(61, 0, Q), note(61, 4 * Q, Q)]);
        for m in &score.measures {
            let n = &chord_notes(&m.staves[0][0])[0];
            assert_eq!(
                n.accidental,
                Some(Accidental::Sharp),
                "measure {}",
                m.number
            );
        }
        let score = build(&[note(61, 3 * Q, 2 * Q), note(61, 5 * Q, Q)]);
        let tied = &chord_notes(&score.measures[1].staves[0][0])[0];
        assert!(tied.tie_stop);
        assert_eq!(tied.accidental, None);
        // …but it still sets the state: the next C♯ in that bar needs no sharp.
        let next = &chord_notes(&score.measures[1].staves[0][1])[0];
        assert_eq!(next.accidental, None);
    }

    #[test]
    fn tempo_marks_follow_the_map() {
        // Bars 3 and 4 are faster (1.5 s = 160 BPM).
        let starts = [0, 2_000_000, 4_000_000, 5_500_000, 7_000_000];
        let notes: Vec<Note> = starts[..4].iter().map(|&t| note(60, t, 300_000)).collect();
        let score = build_with(&notes, &Grid::default_120(), &starts, None);
        let marks: Vec<Option<u32>> = score.measures.iter().map(|m| m.tempo).collect();
        assert_eq!(marks, [Some(120), None, Some(160), None]);
    }

    #[test]
    fn a_pickup_gets_a_bar_of_its_own() {
        let mut grid = Grid::default_120();
        grid.set_origin_us(1_000_000);
        // Half a second in: beat 4 of the bar before the grid's first downbeat.
        let score = build_with(&[note(60, 500_000, Q)], &grid, &[], None);
        assert_eq!(score.measures.len(), 1);
        let treble = &score.measures[0].staves[0];
        assert_eq!(show(treble), ["h:r", "q:r", "q:C4"]);
        assert_eq!(treble.last().unwrap().start, 24);
    }

    #[test]
    fn a_silent_intro_is_skipped() {
        let score = build(&[note(60, 4 * 4 * Q, Q)]); // bar 5
        assert_eq!(score.measures.len(), 1);
        assert_eq!(score.measures[0].number, 1);
        assert_eq!(score.measures[0].tempo, Some(120));
        assert_eq!(show(&score.measures[0].staves[0]), ["q:C4", "q:r", "h:r"]);
    }

    #[test]
    fn three_four_and_six_eight_are_both_24_ticks() {
        for (beats, unit) in [(3u8, 4u8), (6, 8)] {
            let mut grid = Grid::default_120();
            grid.set_time_sig(beats, unit);
            let bar = grid.bar_us();
            let notes = [
                note(60, 0, bar / 3),
                note(64, bar / 2, bar / 6),
                note(48, bar, bar),
                note(67, bar * 2 + bar / 6, bar / 2),
            ];
            let score = build_with(&notes, &grid, &[], None);
            assert_eq!(score.bar_ticks, 24, "{beats}/{unit}");
            assert_eq!(score.measures.len(), 3);
        }
        // A rest after a quarter on beat 1 of 3/4 is quarter + quarter.
        let mut grid = Grid::default_120();
        grid.set_time_sig(3, 4);
        let score = build_with(&[note(60, 0, grid.bar_us() / 3)], &grid, &[], None);
        assert_eq!(show(&score.measures[0].staves[0]), ["q:C4", "q:r", "q:r"]);
    }

    #[test]
    fn a_finger_is_written_on_the_first_piece_only() {
        let finger = Finger::new(3);
        let fingered = Note {
            finger,
            ..note(60, 3 * Q, 2 * Q) // tied across the bar line
        };
        let score = build(&[fingered, note(64, 6 * Q, Q)]);
        let first = &chord_notes(&score.measures[0].staves[0][2])[0];
        let tied = &chord_notes(&score.measures[1].staves[0][0])[0];
        assert_eq!(first.finger, finger);
        assert_eq!(tied.finger, None);
        let plain = score.measures[1].staves[0]
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::Chord(n) => Some(n),
                _ => None,
            })
            .nth(1)
            .unwrap();
        assert_eq!(plain[0].finger, None, "an unfingered note has none");
    }

    #[test]
    fn duplicate_pitches_keep_a_finger_only_when_they_agree() {
        let f = |n: u8, finger: u8| Note {
            finger: Finger::new(finger),
            ..note(n, 0, Q)
        };
        // 60 twice with the same finger; 64 with one fingered copy; 67 clashing.
        let score = build(&[
            f(60, 1),
            f(60, 1),
            f(64, 2),
            note(64, 0, Q),
            f(67, 4),
            f(67, 5),
        ]);
        let notes = chord_notes(&score.measures[0].staves[0][0]);
        let fingers: Vec<Option<u8>> = notes.iter().map(|n| n.finger.map(Finger::get)).collect();
        assert_eq!(fingers, [Some(1), Some(2), None]);
    }
}
