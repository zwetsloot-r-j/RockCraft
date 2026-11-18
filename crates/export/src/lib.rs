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
    let document = score_musicxml(piece)?;
    let path = with_musicxml_extension(path);
    std::fs::write(&path, document).map_err(ExportError::Io)?;
    Ok(std::path::absolute(&path).unwrap_or(path))
}

/// `piece` as a MusicXML document, as text — exactly what
/// [`export_score_file`] writes. Behind the `query_score` host command, which
/// the desktop sheet view draws (M22-C). Pure.
pub fn score_musicxml(piece: &ScoreSource<'_>) -> Result<String, ExportError> {
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
    Ok(to_musicxml(&score, piece.title))
}

/// The title a piece is exported under: its bundle directory's name, else
/// `"Untitled"`.
pub fn title_from_bundle_dir(dir: Option<&Path>) -> String {
    dir.and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Untitled".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, Velocity, DEFAULT_SPLIT};

    fn piece<'a>(notes: &'a [Note], grid: &'a Grid) -> ScoreSource<'a> {
        ScoreSource {
            notes,
            grid,
            bar_starts: &[],
            key: None,
            hand_split: DEFAULT_SPLIT,
            title: "Test",
        }
    }

    #[test]
    fn the_text_is_exactly_what_the_file_export_writes() {
        let notes = [Note {
            pitch: MidiNote::new(60).unwrap(),
            start_us: 0,
            dur_us: 500_000,
            velocity: Velocity::new(80).unwrap(),
            hand: None,
            finger: None,
        }];
        let grid = Grid::default_120();
        let text = score_musicxml(&piece(&notes, &grid)).unwrap();
        assert!(text.contains("<work-title>Test</work-title>"));

        let dir = std::env::temp_dir().join(format!(
            "rockcraft-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let written = export_score_file(&piece(&notes, &grid), &dir.join("sheet")).unwrap();
        assert_eq!(std::fs::read_to_string(&written).unwrap(), text);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_piece_has_nothing_to_export() {
        let grid = Grid::default_120();
        assert!(matches!(
            score_musicxml(&piece(&[], &grid)),
            Err(ExportError::NothingToExport)
        ));
    }
}
