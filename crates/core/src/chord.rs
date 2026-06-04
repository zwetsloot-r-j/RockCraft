//! Pure music-theory helpers: scales, keys, and diatonic chords.

use serde::{Deserialize, Serialize};

use crate::events::MidiNote;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scale {
    Major,
    NaturalMinor,
}

impl Scale {
    /// Semitone offsets of the 7 degrees from the tonic, within one octave.
    pub fn intervals(self) -> [u8; 7] {
        match self {
            Scale::Major => [0, 2, 4, 5, 7, 9, 11],
            Scale::NaturalMinor => [0, 2, 3, 5, 7, 8, 10],
        }
    }
}

/// A musical key: tonic pitch-class (0..=11, 0 == C) + scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub root_pc: u8,
    pub scale: Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKind {
    Triad,
    Seventh,
}

impl Key {
    /// Pitch class (0..=11) of scale degree `1..=7` (wraps; degree 1 == root_pc).
    pub fn degree_pc(self, degree: u8) -> u8 {
        let intervals = self.scale.intervals();
        let idx = degree.wrapping_sub(1) as usize % 7;
        (self.root_pc + intervals[idx]) % 12
    }

    /// Diatonic chord on `degree` (1..=7), stacked in thirds within the scale,
    /// voiced upward from `root` (the actual MIDI octave to start at). Triad =
    /// 3 notes, Seventh = 4. Notes that exceed 0..=127 are dropped. Returns the
    /// pitches ascending.
    pub fn diatonic_chord(self, degree: u8, kind: ChordKind, root: MidiNote) -> Vec<MidiNote> {
        let note_count = match kind {
            ChordKind::Triad => 3usize,
            ChordKind::Seventh => 4usize,
        };

        let mut notes = Vec::with_capacity(note_count);
        let mut from: u16 = root.value() as u16;

        for i in 0..note_count {
            let deg = degree.wrapping_add((i as u8).wrapping_mul(2));
            let target_pc = self.degree_pc(deg);
            let from_pc = (from % 12) as u8;
            let semitones_up: u16 = if target_pc >= from_pc {
                (target_pc - from_pc) as u16
            } else {
                (12 - from_pc + target_pc) as u16
            };
            let note_val = from + semitones_up;
            if note_val > 127 {
                break;
            }
            notes.push(MidiNote::new(note_val as u8).unwrap());
            from = note_val + 1;
        }

        notes
    }

    /// If `root`'s pitch-class is a scale degree, the diatonic chord built on it;
    /// else None. (Used to recognise a chord under the cursor.)
    pub fn chord_for_root(self, root: MidiNote, kind: ChordKind) -> Option<Vec<MidiNote>> {
        let pc = root.value() % 12;
        (1u8..=7)
            .find(|&degree| self.degree_pc(degree) == pc)
            .map(|degree| self.diatonic_chord(degree, kind, root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c_major() -> Key {
        Key {
            root_pc: 0,
            scale: Scale::Major,
        }
    }

    fn c4() -> MidiNote {
        MidiNote::new(60).unwrap()
    }

    fn notes(values: &[u8]) -> Vec<MidiNote> {
        values.iter().map(|&v| MidiNote::new(v).unwrap()).collect()
    }

    #[test]
    fn c_major_triad_degree1() {
        assert_eq!(
            c_major().diatonic_chord(1, ChordKind::Triad, c4()),
            notes(&[60, 64, 67])
        );
    }

    #[test]
    fn c_major_triad_degree5() {
        // G B D voiced from C4
        assert_eq!(
            c_major().diatonic_chord(5, ChordKind::Triad, c4()),
            notes(&[67, 71, 74])
        );
    }

    #[test]
    fn c_major_triad_degree7() {
        // B D F (vii°) voiced from C4
        assert_eq!(
            c_major().diatonic_chord(7, ChordKind::Triad, c4()),
            notes(&[71, 74, 77])
        );
    }

    #[test]
    fn c_major_seventh_degree5() {
        // G B D F voiced from C4
        assert_eq!(
            c_major().diatonic_chord(5, ChordKind::Seventh, c4()),
            notes(&[67, 71, 74, 77])
        );
    }

    #[test]
    fn a_natural_minor_triad_degree1() {
        let key = Key {
            root_pc: 9,
            scale: Scale::NaturalMinor,
        };
        let a4 = MidiNote::new(69).unwrap();
        let chord = key.diatonic_chord(1, ChordKind::Triad, a4);
        // A C E — pitch classes 9, 0, 4
        let pcs: Vec<u8> = chord.iter().map(|n| n.value() % 12).collect();
        assert_eq!(pcs, vec![9, 0, 4]);
    }

    #[test]
    fn drops_notes_above_127() {
        // Root near top: some chord tones will exceed 127
        let root = MidiNote::new(125).unwrap();
        let chord = c_major().diatonic_chord(1, ChordKind::Seventh, root);
        for n in &chord {
            assert!(n.value() <= 127);
        }
        // Should have fewer than 4 notes since root=125 and notes go up
        assert!(chord.len() < 4);
    }

    #[test]
    fn degree_pc_wraps() {
        // degree 8 ≡ degree 1
        assert_eq!(c_major().degree_pc(8), c_major().degree_pc(1));
    }

    #[test]
    fn chord_for_root_none_for_non_scale_pitch() {
        // C# is not in C major
        let c_sharp4 = MidiNote::new(61).unwrap();
        assert!(c_major()
            .chord_for_root(c_sharp4, ChordKind::Triad)
            .is_none());
    }

    #[test]
    fn chord_for_root_some_for_scale_pitch() {
        // G is degree 5 in C major
        let g4 = MidiNote::new(67).unwrap();
        let chord = c_major().chord_for_root(g4, ChordKind::Triad).unwrap();
        assert_eq!(chord, notes(&[67, 71, 74]));
    }
}
