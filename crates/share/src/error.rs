//! The error type of the share server, client and service.

use crate::archive::ArchiveError;

/// A failed share operation.
#[derive(Debug)]
pub enum ShareError {
    /// Packing or unpacking an archive failed.
    Archive(ArchiveError),
    /// The target song name is taken. Displays as `exists:<slug>`, the
    /// convention the frontends key their Replace / Keep both prompt on.
    Exists(String),
    /// Local disk or socket I/O failed.
    Io(std::io::Error),
    /// No free port to share on.
    Bind(String),
    /// The server can't be reached (refused, timed out, bad address).
    Unreachable { server: String, detail: String },
    /// The server answered with an error status.
    Status { code: u16, detail: String },
    /// The server's reply isn't what a RockCraft share server sends.
    BadResponse(String),
    /// No such song on the server.
    NotFound(String),
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareError::Archive(e) => write!(f, "{e}"),
            ShareError::Exists(slug) => write!(f, "exists:{slug}"),
            ShareError::Io(e) => write!(f, "i/o error: {e}"),
            ShareError::Bind(e) => write!(f, "can't start sharing: {e}"),
            ShareError::Unreachable { server, detail } => {
                write!(f, "can't reach {server}: {detail}")
            }
            ShareError::Status { code, detail } => write!(f, "server answered {code}: {detail}"),
            ShareError::BadResponse(e) => write!(f, "unexpected server reply: {e}"),
            ShareError::NotFound(id) => write!(f, "no song {id:?} on the server"),
        }
    }
}

impl std::error::Error for ShareError {}

impl From<ArchiveError> for ShareError {
    fn from(e: ArchiveError) -> Self {
        match e {
            ArchiveError::Exists(slug) => ShareError::Exists(slug),
            ArchiveError::Io(e) => ShareError::Io(e),
            other => ShareError::Archive(other),
        }
    }
}

impl From<std::io::Error> for ShareError {
    fn from(e: std::io::Error) -> Self {
        ShareError::Io(e)
    }
}
