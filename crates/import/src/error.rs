/// Errors produced by the import pipeline.
#[derive(Debug)]
pub enum ImportError {
    /// A pitch value exceeds the valid MIDI range 0..=127.
    InvalidPitch(u8),
    /// JSON serialization or deserialization failed.
    Json(String),
    /// A filesystem operation failed.
    Io(String),
    /// The target path is inside the workspace source tree.
    /// Write to `import-out/` instead (see `import_output_dir`).
    PathNotAllowed(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::InvalidPitch(p) => write!(f, "invalid MIDI pitch: {p}"),
            ImportError::Json(msg) => write!(f, "JSON error: {msg}"),
            ImportError::Io(msg) => write!(f, "I/O error: {msg}"),
            ImportError::PathNotAllowed(msg) => write!(f, "path not allowed: {msg}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<serde_json::Error> for ImportError {
    fn from(e: serde_json::Error) -> Self {
        ImportError::Json(e.to_string())
    }
}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        ImportError::Io(e.to_string())
    }
}
