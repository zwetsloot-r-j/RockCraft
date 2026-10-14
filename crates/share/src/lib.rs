//! Song sharing (M26).
//!
//! - [`archive`]: the `.rcsong` single-file form of a song bundle — strict
//!   pack/unpack, a chart fingerprint, and the export/import-to-file helpers
//!   both frontends call from their host-command dispatch.

pub mod archive;

pub use archive::{
    bundle_files, chart_hash, export_song_file, import_song_file, pack, read_manifest, unpack,
    ArchiveError, Manifest, PackSummary, ARCHIVE_EXT,
};
