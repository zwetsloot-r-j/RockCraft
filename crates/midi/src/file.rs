//! MIDI file I/O using midly.
//!
//! Provides serialization of `core::NoteEvent` sequences to Standard MIDI File
//! bytes and deserialization back to `NoteEvent`s.

use midly::{
    num::{u15, u28, u4, u7},
    Header, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind,
};
use rockcraft_core::{MidiNote, NoteEvent, NoteEventKind, Velocity};

/// A division value for MIDI files (ticks per quarter note).
/// Using 480 as a common default that provides good resolution.
/// TODO: real tempo mapping — currently 1 tick = 1 microsecond for round-trip
/// fidelity, which is inconsistent with this header value.
fn ticks_per_quarter() -> u15 {
    u15::from(480u16)
}

/// Error type for MIDI file operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiFileError {
    /// Failed to parse MIDI bytes.
    ParseError(String),
}

impl std::fmt::Display for MidiFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiFileError::ParseError(msg) => write!(f, "MIDI parse error: {}", msg),
        }
    }
}

impl std::error::Error for MidiFileError {}

impl From<midly::Error> for MidiFileError {
    fn from(err: midly::Error) -> Self {
        MidiFileError::ParseError(format!("{:?}", err))
    }
}

/// Serialize a sequence of note events to Standard MIDI File bytes.
///
/// Uses a single track, channel 0, with a fixed ticks-per-quarter resolution.
/// Timestamps are mapped directly to MIDI ticks (1 tick = 1 microsecond) for
/// round-trip fidelity.
pub fn events_to_smf_bytes(events: &[NoteEvent]) -> Vec<u8> {
    let mut track = Track::new();

    // Sort events by timestamp
    let mut sorted_events = events.to_vec();
    sorted_events.sort_by_key(|e| e.timestamp_us);

    // Track the previous timestamp for delta encoding
    let mut prev_timestamp: u64 = 0;

    for event in &sorted_events {
        // Convert u64 to u28, truncating if necessary (max ~268 million microseconds = ~4.5 minutes)
        let delta_ticks = u28::from((event.timestamp_us - prev_timestamp) as u32);
        prev_timestamp = event.timestamp_us;

        let track_event_kind = match event.kind {
            NoteEventKind::On { velocity } => TrackEventKind::Midi {
                channel: u4::from(0u8), // Use channel 0 for simplicity
                message: MidiMessage::NoteOn {
                    key: u7::from(event.note.value()),
                    vel: u7::from(velocity.value()),
                },
            },
            NoteEventKind::Off => TrackEventKind::Midi {
                channel: u4::from(0u8),
                message: MidiMessage::NoteOff {
                    key: u7::from(event.note.value()),
                    vel: u7::from(0u8), // Note-off velocity is typically 0
                },
            },
        };

        track.push(TrackEvent {
            delta: delta_ticks,
            kind: track_event_kind,
        });
    }

    // Create the SMF with a single track
    let smf = Smf {
        header: Header {
            format: midly::Format::SingleTrack,
            timing: Timing::Metrical(ticks_per_quarter()),
        },
        tracks: vec![track],
    };

    // Write to bytes
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).unwrap();
    bytes
}

/// Parse Standard MIDI File bytes into a vector of note events.
///
/// Assumes the file uses the same timing as `events_to_smf_bytes` (1 tick = 1 microsecond).
pub fn smf_bytes_to_events(bytes: &[u8]) -> Result<Vec<NoteEvent>, MidiFileError> {
    let smf = Smf::parse(bytes)?;

    let mut events = Vec::new();
    let mut absolute_time_us: u64 = 0;

    // We assume the file uses the same timing as we wrote (1 tick = 1 microsecond)
    // For simplicity, we'll use the first track and ignore tempo changes
    // This works for our synthetic test fixtures

    for track in smf.tracks {
        for track_event in track {
            absolute_time_us += track_event.delta.as_int() as u64;

            match track_event.kind {
                TrackEventKind::Midi {
                    channel: _,
                    message,
                } => {
                    match message {
                        MidiMessage::NoteOn { key, vel } => {
                            let vel_u8: u8 = vel.into();
                            if let (Some(midi_note), Some(vel)) =
                                (MidiNote::new(key.into()), Velocity::new(vel_u8))
                            {
                                // In MIDI, note-on with velocity 0 is equivalent to note-off
                                if vel_u8 == 0 {
                                    events.push(NoteEvent::off(midi_note, absolute_time_us));
                                } else {
                                    events.push(NoteEvent::on(midi_note, vel, absolute_time_us));
                                }
                            }
                        }
                        MidiMessage::NoteOff { key, vel: _ } => {
                            if let Some(midi_note) = MidiNote::new(key.into()) {
                                events.push(NoteEvent::off(midi_note, absolute_time_us));
                            }
                        }
                        _ => {
                            // Ignore other MIDI messages (control change, program change, etc.)
                        }
                    }
                }
                TrackEventKind::Meta(_) => {
                    // Ignore meta messages for now
                }
                TrackEventKind::SysEx(_) => {
                    // Ignore system exclusive messages
                }
                TrackEventKind::Escape(_) => {
                    // Ignore escape messages
                }
            }
        }
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, NoteEvent, NoteEventKind, Velocity};

    #[test]
    fn test_roundtrip_simple() {
        // Create some test events
        let middle_c = MidiNote::new(60).unwrap();
        let a4 = MidiNote::new(69).unwrap();
        let vel = Velocity::new(100).unwrap();

        let events = vec![
            NoteEvent::on(middle_c, vel, 1000),
            NoteEvent::off(middle_c, 2000),
            NoteEvent::on(a4, vel, 3000),
            NoteEvent::off(a4, 4000),
        ];

        // Serialize to bytes
        let bytes = events_to_smf_bytes(&events);

        // Deserialize back
        let read_events = smf_bytes_to_events(&bytes).unwrap();

        // Compare
        assert_eq!(events.len(), read_events.len());

        for (original, read) in events.iter().zip(read_events.iter()) {
            assert_eq!(original.note, read.note);
            assert_eq!(original.timestamp_us, read.timestamp_us);
            assert_eq!(original.kind, read.kind);
        }
    }

    #[test]
    fn test_roundtrip_overlapping_notes() {
        // Test with overlapping notes
        let c4 = MidiNote::new(60).unwrap();
        let e4 = MidiNote::new(64).unwrap();
        let g4 = MidiNote::new(67).unwrap();
        let vel = Velocity::new(110).unwrap();

        let events = vec![
            NoteEvent::on(c4, vel, 0),
            NoteEvent::on(e4, vel, 100),
            NoteEvent::on(g4, vel, 200),
            NoteEvent::off(c4, 500),
            NoteEvent::off(e4, 600),
            NoteEvent::off(g4, 700),
        ];

        let bytes = events_to_smf_bytes(&events);
        let read_events = smf_bytes_to_events(&bytes).unwrap();

        assert_eq!(events.len(), read_events.len());

        for (original, read) in events.iter().zip(read_events.iter()) {
            assert_eq!(original.note, read.note);
            assert_eq!(original.timestamp_us, read.timestamp_us);
            assert_eq!(original.kind, read.kind);
        }
    }

    #[test]
    fn test_roundtrip_velocity_zero_is_note_off() {
        // Test that velocity 0 in NoteOn is treated as note-off
        let c4 = MidiNote::new(60).unwrap();
        let vel = Velocity::new(100).unwrap();
        let zero_vel = Velocity::new(0).unwrap();

        let events = vec![
            NoteEvent::on(c4, vel, 1000),
            NoteEvent::on(c4, zero_vel, 2000), // This should be treated as note-off
        ];

        let bytes = events_to_smf_bytes(&events);
        let read_events = smf_bytes_to_events(&bytes).unwrap();

        // The second event should be read back as NoteOff
        assert_eq!(read_events.len(), 2);
        assert_eq!(read_events[0].kind, NoteEventKind::On { velocity: vel });
        assert_eq!(read_events[1].kind, NoteEventKind::Off);
    }
}
