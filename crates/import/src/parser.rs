use rockcraft_core::{
    hand::hand_of_pitch_value, HandOverride, MidiNote, Note, Timeline, Velocity, DEFAULT_SPLIT,
};

use crate::{
    error::ImportError,
    schema::{ExtractedChart, ExtractedNote, Hand},
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
/// - `Hand` is not stored on the notes: the bundle carries it as a split line
///   plus exceptions, see [`hand_assignment`].
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
            // Detected hands travel as the bundle's split line + overrides
            // (`hand_assignment`), editable in the composer (M14-E).
            hand: None,
        });
    }
    Ok(timeline)
}

/// The hand assignment a chart's detected hands imply (M6-H).
///
/// `split` is the split line that agrees with the most notes of known hand
/// (ties go to the one nearest [`DEFAULT_SPLIT`]); `overrides` pins each
/// known-hand note the split gets wrong. [`Hand::Unknown`] notes follow the
/// split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandAssignment {
    pub split: u8,
    pub overrides: Vec<HandOverride>,
}

/// [`HandAssignment`] for `chart`, or `None` when no note has a known hand.
pub fn hand_assignment(chart: &ExtractedChart) -> Option<HandAssignment> {
    let known: Vec<(u8, u64, rockcraft_core::Hand)> = chart
        .notes
        .iter()
        .filter_map(|n| {
            let hand = match n.hand {
                Hand::Left => rockcraft_core::Hand::Left,
                Hand::Right => rockcraft_core::Hand::Right,
                Hand::Unknown => return None,
            };
            Some((n.pitch, n.start_us, hand))
        })
        .collect();
    if known.is_empty() {
        return None;
    }
    let wrong = |split: u8| {
        known
            .iter()
            .filter(|(pitch, _, hand)| hand_of_pitch_value(*pitch, split) != *hand)
            .count()
    };
    let split = (0..=127u8).min_by_key(|&s| (wrong(s), s.abs_diff(DEFAULT_SPLIT)))?;
    let mut overrides: Vec<HandOverride> = known
        .iter()
        .filter(|(pitch, _, hand)| hand_of_pitch_value(*pitch, split) != *hand)
        .map(|&(pitch, start_us, hand)| HandOverride {
            pitch,
            start_us,
            hand,
        })
        .collect();
    overrides.sort_by_key(|o| (o.start_us, o.pitch));
    Some(HandAssignment { split, overrides })
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
            clock_offset_us: None,
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

    fn handed(pitch: u8, start_us: u64, hand: Hand) -> ExtractedNote {
        ExtractedNote {
            start_us,
            hand,
            ..note(pitch)
        }
    }

    #[test]
    fn hand_assignment_picks_the_split_between_the_hands() {
        let chart = chart_with_notes(vec![
            handed(41, 0, Hand::Left),
            handed(48, 0, Hand::Left),
            handed(53, 0, Hand::Right),
            handed(55, 0, Hand::Right),
            handed(72, 0, Hand::Unknown),
        ]);
        // 49..=53 all agree with every note; 53 is nearest middle C.
        let hands = hand_assignment(&chart).unwrap();
        assert_eq!(hands.split, 53);
        assert!(hands.overrides.is_empty());
    }

    #[test]
    fn hand_assignment_pins_a_crossover() {
        let chart = chart_with_notes(vec![
            handed(48, 0, Hand::Left),
            handed(50, 0, Hand::Left),
            handed(64, 0, Hand::Right),
            handed(67, 0, Hand::Right),
            handed(65, 500_000, Hand::Left),
        ]);
        let hands = hand_assignment(&chart).unwrap();
        assert_eq!(hands.split, 60);
        assert_eq!(
            hands.overrides,
            vec![HandOverride {
                pitch: 65,
                start_us: 500_000,
                hand: rockcraft_core::Hand::Left,
            }]
        );
    }

    #[test]
    fn hand_assignment_needs_a_known_hand() {
        let chart = chart_with_notes(vec![handed(60, 0, Hand::Unknown)]);
        assert_eq!(hand_assignment(&chart), None);
    }

    #[test]
    fn unknown_hand_does_not_error() {
        let mut n = note(60);
        n.hand = Hand::Unknown;
        let chart = chart_with_notes(vec![n]);
        assert!(chart_to_timeline(&chart).is_ok());
    }
}
