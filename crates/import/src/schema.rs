use serde::{Deserialize, Serialize};

/// A chart extracted from a source by an importer sidecar.
///
/// This type is the wire format between a Python extractor and the Rust
/// pipeline — the M6-C video extractor and the M13-A score-file converter both
/// emit it. It must never be committed with real song data; synthetic fixtures
/// only (see M6-B gitignore guard).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractedChart {
    pub notes: Vec<ExtractedNote>,
    pub source: SourceMeta,
    /// Notated musical context, when the source carries it. Score files do;
    /// video does not. Drives `meta.grid` / `meta.key` in the written bundle.
    ///
    /// `#[serde(default)]` keeps every chart written before M13-A — including
    /// the committed `tests/fixtures/synthetic_chart.json` — deserializable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notation: Option<NotationMeta>,
}

/// Tempo / metre / key as *notated* in the source score.
///
/// Only a source that states these explicitly should fill them in — nothing here
/// is inferred. A `None` field means "the source did not say", not "default".
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct NotationMeta {
    /// Tempo at the start of the piece, in BPM. The writer clamps it to
    /// [`Grid::MIN_BPM`](rockcraft_core::Grid::MIN_BPM)..=[`Grid::MAX_BPM`](rockcraft_core::Grid::MAX_BPM);
    /// out-of-range is not an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_sig: Option<rockcraft_core::TimeSig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<rockcraft_core::Key>,
}

/// One detected note in an extracted chart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractedNote {
    /// MIDI note number 0..=127.
    pub pitch: u8,
    /// Note start in microseconds from the beginning of the video.
    pub start_us: u64,
    /// Note duration in microseconds (at least 1 µs after conversion).
    pub dur_us: u64,
    pub hand: Hand,
    /// Explicit velocity 0..=127. `None` until M6-F audio-fusion fills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u8>,
    /// Extractor confidence 0.0..=1.0. Used in the M6-E review step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// Which hand played a note, as determined by the extractor.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
    Unknown,
}

/// Provenance metadata emitted by the extractor sidecar.
///
/// Diagnostic only — never committed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceMeta {
    /// Song title as inferred from the video, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Video frame rate used during extraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f32>,
    /// Scroll speed in pixels per second used to convert positions to times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_px_per_s: Option<f32>,
    /// Native source-frame height in pixels. With `hit_line_px` and
    /// `scroll_px_per_s`, the importer derives the edit overlay's grid
    /// calibration (`alignment.json`) so an imported chart registers on its
    /// source movie automatically. `None` when the extractor could not measure it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_height_px: Option<u32>,
    /// Hit-line row in pixels from the top of the source frame (where falling
    /// notes are struck). Pairs with `frame_height_px` for the overlay calibration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_line_px: Option<u32>,
    pub extractor_version: String,
}
