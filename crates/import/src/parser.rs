use rockcraft_core::{MidiNote, Note, Timeline, Velocity};

use crate::{
    error::ImportError,
    schema::{ExtractedChart, ExtractedNote},
};

/// Velocity used for notes whose extractor did not supply one.
/// Chosen as a comfortable mezzo-forte that produces a valid MIDI file.
pub const DEFAULT_VELOCITY: u8 = 80;

/// Deserialize an [`ExtractedChart`] from a JSON string.
pub fn from_json(s: &str) -> Result<ExtractedChart, ImportError> {
    serde_json::from_str(s).map_err(ImportError::from)
}

/// Serialize an [`ExtractedChart`] to a JSON string. Infallible.
pub fn to_json(chart: &ExtractedChart) -> String {
    serde_json::to_string(chart).expect("ExtractedChart serialization is infallible")
}

/// Convert an [`ExtractedChart`] into a core [`Timeline`].
///
/// - Pitches > 127 are rejected with [`ImportError::InvalidPitch`].
/// - Missing velocity defaults to [`DEFAULT_VELOCITY`].
/// - `Hand` is not stored in `Timeline`; it remains available on the original
///   [`ExtractedNote`] for the M6-E review step.
pub fn chart_to_timeline(chart: &ExtractedChart) -> Result<Timeline, ImportError> {
    let mut timeline = Timeline::new();
    for note in &chart.notes {
        let midi_note = MidiNote::new(note.pitch).ok_or(ImportError::InvalidPitch(note.pitch))?;
        let velocity =
            note_velocity(note).ok_or(ImportError::InvalidPitch(note.velocity.unwrap_or(0)))?;
        timeline.insert(Note {
            pitch: midi_note,
            start_us: note.start_us,
            dur_us: note.dur_us.max(1),
            velocity,
            // Hand is authored in the composer (M14-E), not sniffed on import.
            hand: None,
        });
    }
    Ok(timeline)
}

fn note_velocity(note: &ExtractedNote) -> Option<Velocity> {
    let raw = note.velocity.unwrap_or(DEFAULT_VELOCITY);
    Velocity::new(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ExtractedNote, Hand, SourceMeta};

    fn source() -> SourceMeta {
        SourceMeta {
            title: Some("Test".into()),
            fps: None,
            scroll_px_per_s: None,
            frame_height_px: None,
            hit_line_px: None,
            extractor_version: "test-0.1".into(),
        }
    }

    fn chart_with_notes(notes: Vec<ExtractedNote>) -> ExtractedChart {
        ExtractedChart {
            notes,
            source: source(),
            notation: None,
        }
    }

    fn note(pitch: u8) -> ExtractedNote {
        ExtractedNote {
            pitch,
            start_us: 0,
            dur_us: 500_000,
            hand: Hand::Right,
            velocity: None,
            confidence: None,
        }
    }

    #[test]
    fn json_roundtrip() {
        let chart = chart_with_notes(vec![note(60)]);
        let json = to_json(&chart);
        let back = from_json(&json).unwrap();
        assert_eq!(chart, back);
    }

    #[test]
    fn chart_to_timeline_pitch_and_timing() {
        let chart = chart_with_notes(vec![
            ExtractedNote {
                pitch: 60,
                start_us: 0,
                dur_us: 500_000,
                hand: Hand::Right,
                velocity: Some(100),
                confidence: None,
            },
            ExtractedNote {
                pitch: 64,
                start_us: 500_000,
                dur_us: 500_000,
                hand: Hand::Right,
                velocity: Some(90),
                confidence: None,
            },
        ]);
        let timeline = chart_to_timeline(&chart).unwrap();
        assert_eq!(timeline.len(), 2);
        let notes: Vec<_> = timeline.notes().collect();
        assert_eq!(notes[0].1.pitch.value(), 60);
        assert_eq!(notes[0].1.start_us, 0);
        assert_eq!(notes[1].1.pitch.value(), 64);
        assert_eq!(notes[1].1.start_us, 500_000);
    }

    #[test]
    fn missing_velocity_defaults_to_constant() {
        let chart = chart_with_notes(vec![note(60)]);
        let timeline = chart_to_timeline(&chart).unwrap();
        let (_, n) = timeline.notes().next().unwrap();
        assert_eq!(n.velocity.value(), DEFAULT_VELOCITY);
    }

    #[test]
    fn invalid_pitch_returns_error() {
        let mut n = note(60);
        n.pitch = 200;
        let chart = chart_with_notes(vec![n]);
        assert!(matches!(
            chart_to_timeline(&chart),
            Err(ImportError::InvalidPitch(200))
        ));
    }

    #[test]
    fn unknown_hand_does_not_error() {
        let mut n = note(60);
        n.hand = Hand::Unknown;
        let chart = chart_with_notes(vec![n]);
        assert!(chart_to_timeline(&chart).is_ok());
    }
}
