//! Song sharing (M26).
//!
//! - [`archive`]: the `.rcsong` single-file form of a song bundle — strict
//!   pack/unpack, a chart fingerprint, and the export/import-to-file helpers
//!   both frontends call from their host-command dispatch (M26-A).
//! - [`server`] / [`client`]: the peer-to-peer LAN share server every instance
//!   can run, and the client that lists, downloads and uploads (M26-B).
//! - [`discovery`]: finding sharing peers over mDNS (M26-C).
//! - [`service`]: [`ShareService`], the sharing state each frontend holds —
//!   the running server plus background transfers.

pub mod archive;
pub mod client;
pub mod discovery;
mod error;
pub mod net;
pub mod server;
pub mod service;

pub use archive::{
    bundle_files, chart_hash, export_song_file, import_song_archive, import_song_file, pack,
    read_manifest, unpack, ArchiveError, Manifest, PackSummary, ARCHIVE_EXT,
};
pub use discovery::{discover, Peer};
pub use error::ShareError;
pub use server::{RemoteSong, ServerInfo, ShareServer, DEFAULT_PORT};
pub use service::{
    Direction, ListedSong, Lister, LocalState, ShareService, ShareStatus, Transfer, TransferState,
};
