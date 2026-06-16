//! Song/Recording bundle model and backing-track sync math.
//!
//! Pure domain types and functions — no file I/O, no audio device. The
//! `RecordingMeta` struct mirrors the `meta.json` inside a bundle directory;
//! serialization is in-memory only (callers in `tui`/`audio` do the fs work).

use crate::{Grid, Key};
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

/// Describes a background video attached to a recording bundle.
///
/// `file` is the bundle-relative filename only (e.g. `"source.mp4"`) — never an
/// absolute path, so the bundle stays movable. `offset_us` is the signed
/// alignment offset applied as `videoTime = songTime + offset_us` (a positive
/// offset means the video runs ahead of the song); it mirrors the edit-grid
/// backdrop offset from M7-tauri-N.
///
/// `core` only carries this reference — it never decodes or renders video; the
/// webview's HTML5 `<video>` element does that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundVideo {
    pub file: String,
    pub offset_us: i64,
}

/// Where a chart bundle came from, used by the library browser to label entries.
///
/// Optional in `meta.json` (`#[serde(default)]` → `None`) so older bundles
/// written before this field existed still deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackOrigin {
    /// Captured live from the piano (Record screen).
    Recorded,
    /// Authored from scratch in the composer.
    Composed,
    /// A recording/import opened in the composer and re-saved.
    Edited,
    /// Produced by the video-import pipeline.
    Imported,
}

impl TrackOrigin {
    /// A short human label for the library list.
    pub fn label(self) -> &'static str {
        match self {
            TrackOrigin::Recorded => "recorded",
            TrackOrigin::Composed => "composed",
            TrackOrigin::Edited => "edited",
            TrackOrigin::Imported => "imported",
        }
    }
}

/// The manifest serialised as `meta.json` inside a recording bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingMeta {
    /// Bundle-relative MIDI filename, e.g. `"song.mid"`.
    pub midi_file: String,
    /// Optional backing audio description. `None` means MIDI-only recording.
    #[serde(default)]
    pub backing: Option<BackingTrack>,
    /// Composer grid (tempo/time-sig/snap). `None` for legacy/piano recordings.
    #[serde(default)]
    pub grid: Option<Grid>,
    /// Composer key signature. `None` for legacy/piano recordings.
    #[serde(default)]
    pub key: Option<Key>,
    /// Where the bundle came from. `None` for legacy bundles written before the
    /// field existed; the library browser then shows it as unknown.
    #[serde(default)]
    pub origin: Option<TrackOrigin>,
    /// Optional background video played behind the highway during play/practice
    /// (and the edit grid). `None` for pieces without one, including all legacy
    /// bundles written before this field existed.
    #[serde(default)]
    pub video: Option<BackgroundVideo>,
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
            grid: None,
            key: None,
            origin: None,
            video: None,
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
            grid: None,
            key: None,
            origin: None,
            video: None,
            version: 1,
        };
        let json = meta.to_json();
        let back = RecordingMeta::from_json(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn with_grid_and_key_roundtrip() {
        use crate::{Grid, Key, Scale};
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: None,
            grid: Some(Grid::default_120()),
            key: Some(Key {
                root_pc: 7,
                scale: Scale::NaturalMinor,
            }),
            origin: None,
            video: None,
            version: 1,
        };
        let json = meta.to_json();
        let back = RecordingMeta::from_json(&json).unwrap();
        assert_eq!(meta, back);
    }

    #[test]
    fn edited_bpm_survives_grid_roundtrip() {
        use crate::Grid;
        // Simulate an edit-mode BPM change, then persist+reload the meta.
        let mut grid = Grid::default_120();
        grid.set_bpm(96);
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: None,
            grid: Some(grid),
            key: None,
            origin: None,
            video: None,
            version: 1,
        };
        let back = RecordingMeta::from_json(&meta.to_json()).unwrap();
        assert_eq!(back.grid.unwrap().bpm, 96);
    }

    #[test]
    fn with_video_roundtrip() {
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: None,
            grid: None,
            key: None,
            origin: Some(TrackOrigin::Imported),
            video: Some(BackgroundVideo {
                file: "source.mp4".into(),
                offset_us: -250_000,
            }),
            version: 1,
        };
        let json = meta.to_json();
        let back = RecordingMeta::from_json(&json).unwrap();
        assert_eq!(meta, back);
        assert_eq!(back.video.as_ref().unwrap().file, "source.mp4");
        assert_eq!(back.video.as_ref().unwrap().offset_us, -250_000);
    }

    #[test]
    fn backing_swap_preserves_video() {
        // M10-E: backing audio and the background video are independent piece
        // attributes. Mutating `meta.backing` — whether replacing the file or
        // detaching it entirely — must never disturb `meta.video`.
        let video = BackgroundVideo {
            file: "source.mp4".into(),
            offset_us: -250_000,
        };
        let mut meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: Some(BackingTrack {
                file: "original.mp3".into(),
                audio_start_us: 100_000,
            }),
            grid: None,
            key: None,
            origin: Some(TrackOrigin::Imported),
            video: Some(video.clone()),
            version: 1,
        };

        // Replace the backing with a different file/offset.
        meta.backing = Some(BackingTrack {
            file: "studio.flac".into(),
            audio_start_us: 0,
        });
        assert_eq!(
            meta.video.as_ref(),
            Some(&video),
            "replacing backing leaves video untouched"
        );
        // The swapped backing survives a JSON round-trip alongside the video.
        let back = RecordingMeta::from_json(&meta.to_json()).unwrap();
        assert_eq!(back.backing.as_ref().unwrap().file, "studio.flac");
        assert_eq!(back.video.as_ref(), Some(&video));

        // Detaching the backing also leaves the video intact, byte-for-byte.
        meta.backing = None;
        assert_eq!(
            meta.video.as_ref(),
            Some(&video),
            "detaching backing leaves video untouched"
        );
        let back = RecordingMeta::from_json(&meta.to_json()).unwrap();
        assert!(back.backing.is_none());
        assert_eq!(back.video.as_ref(), Some(&video));
    }

    #[test]
    fn without_video_roundtrip() {
        let meta = RecordingMeta::new_midi_only("song.mid");
        assert!(meta.video.is_none());
        let back = RecordingMeta::from_json(&meta.to_json()).unwrap();
        assert!(back.video.is_none());
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
        // An older meta.json that only has `midi_file`; optional fields default to None.
        let minimal = r#"{"midi_file":"song.mid"}"#;
        let meta = RecordingMeta::from_json(minimal).unwrap();
        assert_eq!(meta.midi_file, "song.mid");
        assert!(meta.backing.is_none());
        assert!(meta.grid.is_none());
        assert!(meta.key.is_none());
        assert!(meta.video.is_none());

        // Legacy with backing but no grid/key/video.
        let with_backing =
            r#"{"midi_file":"song.mid","backing":{"file":"b.mp3","audio_start_us":0}}"#;
        let meta2 = RecordingMeta::from_json(with_backing).unwrap();
        assert!(meta2.backing.is_some());
        assert!(meta2.grid.is_none());
        assert!(meta2.key.is_none());
        assert!(meta2.video.is_none());

        // A pre-video bundle that already carried grid/key/origin (post-M3-H but
        // pre-M9-G) still parses with video == None.
        let pre_video = r#"{"midi_file":"song.mid","origin":"imported","version":1}"#;
        let meta3 = RecordingMeta::from_json(pre_video).unwrap();
        assert_eq!(meta3.origin, Some(TrackOrigin::Imported));
        assert!(meta3.video.is_none());
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
