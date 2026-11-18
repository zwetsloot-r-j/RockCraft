//! Sheet-music export (M22-B).
//!
//! `core` derives a printable [`Score`](rockcraft_core::notation::Score) from a
//! piece; this crate writes it as MusicXML, the format every notation program
//! reads. File formats stay out of `core`, so the writer lives here.

mod musicxml;

use std::path::{Path, PathBuf};

use rockcraft_core::notation::{build_score, NotationOptions};
use rockcraft_core::{Grid, Key, Note};

pub use musicxml::{to_musicxml, with_musicxml_extension};

/// Why a score export failed.
#[derive(Debug)]
pub enum ExportError {
    /// The piece has no notes.
    NothingToExport,
    Io(std::io::Error),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::NothingToExport => write!(f, "nothing to export"),
            ExportError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// The piece to print: everything [`build_score`] needs, plus its title.
pub struct ScoreSource<'a> {
    pub notes: &'a [Note],
    pub grid: &'a Grid,
    pub bar_starts: &'a [u64],
    pub key: Option<Key>,
    pub hand_split: u8,
    pub title: &'a str,
}

/// Write `piece` as MusicXML sheet music to `path` (`.musicxml` appended when
/// the path has no extension) and return the absolute path written.
///
/// The one implementation behind the `export_score` host command, so both
/// frontends behave identically: default notation options (sixteenth
/// quantisation), right hand on the treble staff, left on the bass.
pub fn export_score_file(piece: &ScoreSource<'_>, path: &Path) -> Result<PathBuf, ExportError> {
    if piece.notes.is_empty() {
        return Err(ExportError::NothingToExport);
    }
    let score = build_score(
        piece.notes,
        piece.grid,
        piece.bar_starts,
        piece.key,
        piece.hand_split,
        NotationOptions::default(),
    );
    let path = with_musicxml_extension(path);
    std::fs::write(&path, to_musicxml(&score, piece.title)).map_err(ExportError::Io)?;
    Ok(std::path::absolute(&path).unwrap_or(path))
}

/// The title a piece is exported under: its bundle directory's name, else
/// `"Untitled"`.
pub fn title_from_bundle_dir(dir: Option<&Path>) -> String {
    dir.and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Untitled".to_string())
}
