//! Import pipeline data contract for RockCraft's M6 video import path.
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
pub mod schema;
pub mod writer;

pub use error::ImportError;
pub use parser::{chart_to_timeline, from_json, to_json};
pub use rockcraft_core::{RecordingMeta, Timeline};
pub use schema::{ExtractedChart, ExtractedNote, Hand, SourceMeta};
pub use writer::{import_output_dir, write_chart_bundle};
