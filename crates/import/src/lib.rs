//! Import pipeline data contract for RockCraft's import paths — M6 video
//! extraction, M13-A score files and M13-B scans.
//!
//! Provides:
//! - [`ExtractedChart`] / [`ExtractedNote`] — the JSON schema a Python extractor emits.
//! - [`from_json`] / [`to_json`] — schema round-trip.
//! - [`chart_to_timeline`] — convert to a core [`Timeline`].
//! - [`write_chart_bundle`] — emit `song.mid` + `meta.json` into the gitignored output dir.
//! - [`import_output_dir`] — the canonical output root (`<workspace>/import-out`).
//!
//! This crate depends only on `rockcraft-core`. No device, audio, network, or
//! terminal code lives here — it is fully headless-testable against synthetic fixtures.

pub mod error;
pub mod parser;
pub mod pipeline;
pub mod schema;
pub mod writer;

pub use error::ImportError;
pub use parser::{chart_to_timeline, from_json, to_json};
pub use pipeline::{
    fetch_command_configured, import_source, omr_summary, ImportInput, Progress, OMR_SUMMARY_PREFIX,
};
pub use rockcraft_core::{BackgroundVideo, BackingTrack, RecordingMeta, Timeline};
pub use schema::{ExtractedChart, ExtractedNote, Hand, NotationMeta, SourceMeta};
pub use writer::{
    import_output_dir, write_chart_bundle, write_chart_bundle_full,
    write_chart_bundle_with_backing, write_part_bundle,
};
