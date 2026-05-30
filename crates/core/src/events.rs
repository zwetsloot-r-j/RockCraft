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
    fn note_on_roundtrips() {
        let middle_c = MidiNote::new(60).unwrap();
        let vel = Velocity::new(100).unwrap();
        let ev = NoteEvent::on(middle_c, vel, 1_000);

        assert_eq!(ev.note.value(), 60);
        assert_eq!(ev.timestamp_us, 1_000);
        assert_eq!(ev.kind, NoteEventKind::On { velocity: vel });
    }
}
