//! Song/Recording bundle model and backing-track sync math.
//!
//! Pure domain types and functions — no file I/O, no audio device. The
//! `RecordingMeta` struct mirrors the `meta.json` inside a bundle directory;
//! serialization is in-memory only (callers in `tui`/`audio` do the fs work).

use serde::{Deserialize, Serialize};

/// Describes the backing audio track inside a recording bundle.
///
/// `file` is the bundle-relative filename only (e.g. `"backing.mp3"`) — never
/// an absolute path, so the bundle stays movable. `audio_start_us` is the
/// position in the audio file that lines up with recording time 0 (usually 0;
/// a nonzero value allows a trimmed lead-in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackingTrack {
    pub file: String,
    pub audio_start_us: u64,
}

/// The manifest serialised as `meta.json` inside a recording bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingMeta {
    /// Bundle-relative MIDI filename, e.g. `"song.mid"`.
    pub midi_file: String,
    /// Optional backing audio description. `None` means MIDI-only recording.
    #[serde(default)]
    pub backing: Option<BackingTrack>,
    /// Schema version; always written as `1`. Kept for forward-compat.
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_version() -> u32 {
    1
}

/// Errors that can occur when parsing a `meta.json` string.
#[derive(Debug)]
pub struct MetaError(serde_json::Error);

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "meta.json parse error: {}", self.0)
    }
}

impl std::error::Error for MetaError {}

impl RecordingMeta {
    /// Create a MIDI-only manifest with no backing track.
    pub fn new_midi_only(midi_file: impl Into<String>) -> Self {
        Self {
            midi_file: midi_file.into(),
            backing: None,
            version: 1,
        }
    }

    /// Serialize to a JSON string. Infallible for well-formed types.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("RecordingMeta serialization is infallible")
    }

    /// Deserialize from a JSON string. Returns `Err(MetaError)` on malformed input.
    pub fn from_json(s: &str) -> Result<Self, MetaError> {
        serde_json::from_str(s).map_err(MetaError)
    }
}

/// Compute the whole-song forward shift that Play mode applies to note timestamps.
///
/// Mirrors the shift in `play.rs`: `(pre_roll_us + lead_us).saturating_sub(first_note_us)`.
/// If `first_note_us` is larger than the combined pre-roll + lead, the result is 0
/// (saturating subtraction — the shift never goes negative).
pub fn song_shift_us(first_note_us: u64, pre_roll_us: u64, lead_us: u64) -> u64 {
    (pre_roll_us + lead_us).saturating_sub(first_note_us)
}

/// Compute the playback position in the backing audio file at a given clock time.
///
/// Returns `None` while the clock is before `shift_us` (the audio has not started
/// yet). Once playback has passed the shift point, returns
/// `Some((clock_us - shift_us) + audio_start_us)`.
pub fn backing_position_us(clock_us: u64, shift_us: u64, audio_start_us: u64) -> Option<u64> {
    clock_us
        .checked_sub(shift_us)
        .map(|elapsed| elapsed + audio_start_us)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RecordingMeta round-trips ────────────────────────────────────────────

    #[test]
    fn midi_only_roundtrip() {
        let meta = RecordingMeta::new_midi_only("song.mid");
        let json = meta.to_json();
        let back = RecordingMeta::from_json(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn with_backing_roundtrip() {
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: Some(BackingTrack {
                file: "backing.mp3".into(),
                audio_start_us: 250_000,
            }),
            version: 1,
        };
        let json = meta.to_json();
        let back = RecordingMeta::from_json(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn malformed_json_returns_err() {
        assert!(RecordingMeta::from_json("{not valid json}").is_err());
        assert!(RecordingMeta::from_json("").is_err());
        assert!(RecordingMeta::from_json("null").is_err());
    }

    #[test]
    fn minimal_legacy_json_deserializes() {
        // An older meta.json that only has `midi_file`; `backing` should default to None.
        let minimal = r#"{"midi_file":"song.mid"}"#;
        let meta = RecordingMeta::from_json(minimal).unwrap();
        assert_eq!(meta.midi_file, "song.mid");
        assert!(meta.backing.is_none());
    }

    // ── song_shift_us ────────────────────────────────────────────────────────

    #[test]
    fn shift_standard_case() {
        assert_eq!(song_shift_us(0, 1_500_000, 2_000_000), 3_500_000);
    }

    #[test]
    fn shift_saturates_to_zero() {
        // first_note_us larger than pre_roll + lead → saturating sub gives 0
        assert_eq!(song_shift_us(5_000_000, 1_000_000, 2_000_000), 0);
    }

    #[test]
    fn shift_exact_boundary_is_zero() {
        assert_eq!(song_shift_us(3_000_000, 1_000_000, 2_000_000), 0);
    }

    // ── backing_position_us ──────────────────────────────────────────────────

    #[test]
    fn before_shift_is_none() {
        assert_eq!(backing_position_us(999, 1_000, 0), None);
        assert_eq!(backing_position_us(0, 1_000, 0), None);
    }

    #[test]
    fn at_shift_audio_start_zero() {
        assert_eq!(backing_position_us(1_000, 1_000, 0), Some(0));
    }

    #[test]
    fn one_second_past_shift() {
        assert_eq!(
            backing_position_us(1_000 + 1_000_000, 1_000, 0),
            Some(1_000_000)
        );
    }

    #[test]
    fn at_shift_with_audio_start_offset() {
        assert_eq!(backing_position_us(1_000, 1_000, 250_000), Some(250_000));
    }
}
