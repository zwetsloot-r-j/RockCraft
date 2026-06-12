//! Library scanning command and DTO for the Tauri frontend.
//!
//! Exposes `scan_library` as a Tauri invoke command that calls
//! [`rockcraft_midi::bundle::list_library`] with the default scan roots and
//! returns a serialisable DTO slice — one entry per bundle found.

use std::path::PathBuf;

use rockcraft_midi::bundle::{default_scan_roots, list_library, LibraryEntry};
use serde::Serialize;

/// Serialisable mirror of [`LibraryEntry`].
///
/// `dir` is a `String` (not `PathBuf`) so Tauri's JSON serialiser can encode
/// it without a custom implementation. All other fields are copied verbatim.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryEntryDto {
    /// Display name — bundle directory's file name.
    pub name: String,
    /// Absolute path to the bundle directory as a UTF-8 string.
    pub dir: String,
    /// Number of notes in `song.mid`.
    pub note_count: usize,
    /// Total chart duration in microseconds.
    pub duration_us: u64,
    /// Origin label: `"recorded"`, `"composed"`, `"edited"`, `"imported"`, or
    /// `null` for legacy bundles.
    pub origin: Option<String>,
    /// Whether the bundle declares a backing audio track.
    pub has_backing: bool,
}

impl From<LibraryEntry> for LibraryEntryDto {
    fn from(e: LibraryEntry) -> Self {
        LibraryEntryDto {
            name: e.name,
            dir: e.dir.to_string_lossy().into_owned(),
            note_count: e.note_count,
            duration_us: e.duration_us,
            origin: e.origin.map(|o| o.label().to_owned()),
            has_backing: e.has_backing,
        }
    }
}

/// Scan the default library roots and return a list of bundle DTOs.
///
/// The default roots are `~/.rockcraft/library`, `recordings/`, and
/// `import-out/`; missing roots are silently skipped. Results are sorted by
/// bundle name.
pub fn scan_library_inner(roots: &[PathBuf]) -> Vec<LibraryEntryDto> {
    list_library(roots)
        .into_iter()
        .map(LibraryEntryDto::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::TrackOrigin;
    use rockcraft_midi::bundle::{slug, LibraryEntry};
    use std::path::PathBuf;

    fn make_entry(name: &str, origin: Option<TrackOrigin>, has_backing: bool) -> LibraryEntry {
        LibraryEntry {
            name: name.to_owned(),
            dir: PathBuf::from(format!("/fake/{}", slug(name))),
            note_count: 5,
            duration_us: 3_000_000,
            origin,
            has_backing,
        }
    }

    #[test]
    fn dto_preserves_all_fields() {
        let entry = make_entry("my-song", Some(TrackOrigin::Recorded), true);
        let dto = LibraryEntryDto::from(entry.clone());

        assert_eq!(dto.name, entry.name);
        assert_eq!(dto.dir, entry.dir.to_string_lossy());
        assert_eq!(dto.note_count, entry.note_count);
        assert_eq!(dto.duration_us, entry.duration_us);
        assert_eq!(dto.origin, Some("recorded".to_owned()));
        assert_eq!(dto.has_backing, entry.has_backing);
    }

    #[test]
    fn dto_none_origin_for_legacy_bundle() {
        let entry = make_entry("legacy", None, false);
        let dto = LibraryEntryDto::from(entry);
        assert_eq!(dto.origin, None);
        assert!(!dto.has_backing);
    }

    #[test]
    fn scan_empty_roots_returns_empty() {
        let roots = vec![PathBuf::from("/no/such/path/ever")];
        assert!(scan_library_inner(&roots).is_empty());
    }
}
