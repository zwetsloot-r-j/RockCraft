//! Note events — the atoms that flow from the piano and from song files.
//!
//! This is intentionally a small, well-typed seed. Expand it deliberately:
//! the scoring engine and song timeline will be built *on top of* these types,
//! so changes here ripple everywhere. Treat it as the contract.

/// A MIDI note number, 0..=127 (60 == middle C).
///
/// Wrapped in a newtype so an invalid value can never be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MidiNote(u8);

impl MidiNote {
    /// Construct from a raw MIDI number, validating the 0..=127 range.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 127 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The raw 0..=127 value.
    pub const fn value(self) -> u8 {
        self.0
    }

    /// Scientific pitch notation using sharps (e.g. 60 → "C4", 69 → "A4", 61 → "C#4").
    pub fn name(self) -> String {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        let v = self.0 as i32;
        let pitch_class = (v % 12) as usize;
        let octave = (v / 12) - 1;
        format!("{}{}", NAMES[pitch_class], octave)
    }
}

/// Note-on velocity, 0..=127. A note-on with velocity 0 is, by MIDI
/// convention, equivalent to a note-off; callers normalise that at the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Velocity(u8);

impl Velocity {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 127 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    /// Returns true if the velocity value is 0.
    /// In MIDI, a note-on with velocity 0 is equivalent to a note-off.
    pub const fn is_note_off(self) -> bool {
        self.0 == 0
    }

    /// Returns the velocity normalized to the range 0.0..=1.0.
    /// Maps linearly from 0..=127 to 0.0..=1.0.
    pub fn normalized(self) -> f32 {
        self.0 as f32 / 127.0
    }
}

/// Whether a note started or stopped sounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteEventKind {
    On { velocity: Velocity },
    Off,
}

/// A single note event positioned in time.
///
/// `timestamp_us` is microseconds from an engine-defined origin (song start for
/// file-loaded events; capture start for live input). Keeping time in integer
/// microseconds avoids float drift in the scoring hot path; rendering converts
/// to whatever it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteEvent {
    pub note: MidiNote,
    pub kind: NoteEventKind,
    pub timestamp_us: u64,
}

impl NoteEvent {
    pub const fn on(note: MidiNote, velocity: Velocity, timestamp_us: u64) -> Self {
        Self {
            note,
            kind: NoteEventKind::On { velocity },
            timestamp_us,
        }
    }

    pub const fn off(note: MidiNote, timestamp_us: u64) -> Self {
        Self {
            note,
            kind: NoteEventKind::Off,
            timestamp_us,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_note_rejects_out_of_range() {
        assert!(MidiNote::new(127).is_some());
        assert!(MidiNote::new(128).is_none());
    }

    #[test]
    fn midi_note_name_middle_c() {
        assert_eq!(MidiNote::new(60).unwrap().name(), "C4");
    }

    #[test]
    fn midi_note_name_a4() {
        assert_eq!(MidiNote::new(69).unwrap().name(), "A4");
    }

    #[test]
    fn midi_note_name_c_sharp_4() {
        assert_eq!(MidiNote::new(61).unwrap().name(), "C#4");
    }

    #[test]
    fn midi_note_name_lowest() {
        assert_eq!(MidiNote::new(0).unwrap().name(), "C-1");
    }

    #[test]
    fn midi_note_name_highest() {
        assert_eq!(MidiNote::new(127).unwrap().name(), "G9");
    }

    #[test]
    fn note_on_roundtrips() {
        let middle_c = MidiNote::new(60).unwrap();
        let vel = Velocity::new(100).unwrap();
        let ev = NoteEvent::on(middle_c, vel, 1_000);

        assert_eq!(ev.note.value(), 60);
        assert_eq!(ev.timestamp_us, 1_000);
        assert_eq!(ev.kind, NoteEventKind::On { velocity: vel });
    }

    #[test]
    fn velocity_is_note_off() {
        let zero = Velocity::new(0).unwrap();
        let one = Velocity::new(1).unwrap();
        let max = Velocity::new(127).unwrap();

        assert!(zero.is_note_off());
        assert!(!one.is_note_off());
        assert!(!max.is_note_off());
    }

    #[test]
    fn velocity_normalized() {
        let zero = Velocity::new(0).unwrap();
        let max = Velocity::new(127).unwrap();
        let epsilon = 1e-6;

        assert!((zero.normalized() - 0.0).abs() < epsilon);
        assert!((max.normalized() - 1.0).abs() < epsilon);
    }
}
