use serde::{Deserialize, Serialize};

/// A chart extracted from a video source by the M6-C sidecar.
///
/// This type is the wire format between the Python extractor and the Rust
/// pipeline. It must never be committed with real song data; synthetic
/// fixtures only (see M6-B gitignore guard).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractedChart {
    pub notes: Vec<ExtractedNote>,
    pub source: SourceMeta,
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
    pub extractor_version: String,
}
