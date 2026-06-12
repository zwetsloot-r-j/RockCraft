//! Library scanning command and DTO for the Tauri frontend.
//!
//! Exposes `scan_library` as a Tauri invoke command that calls
//! [`rockcraft_midi::bundle::list_library`] with the default scan roots and
//! returns a serialisable DTO slice — one entry per bundle found.

use std::path::PathBuf;

use rockcraft_midi::bundle::{list_library, LibraryEntry};
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

/// Absolute directory of the newest bundle across `roots`, or `None` when no
/// bundles exist. Backs the menu's "Play/Edit last recording" entry points.
pub fn latest_recording_inner(roots: &[PathBuf]) -> Option<String> {
    let entries: Vec<(String, u64)> = list_library(roots)
        .into_iter()
        .map(|e| {
            let mtime = dir_mtime_secs(&e.dir);
            (e.dir.to_string_lossy().into_owned(), mtime)
        })
        .collect();
    newest(&entries)
}

/// Pick the newest bundle directory from `(dir, mtime_secs)` pairs.
///
/// The ranking key is the bundle's `take-<unix_ts>` suffix when present
/// (recordings carry their capture time in the directory name and sort
/// chronologically), otherwise the filesystem mtime (library/imported bundles).
/// Ties break by mtime, then by the dir string, so the result is deterministic.
///
/// Pure — no filesystem access — so it is unit-testable from `(name, mtime)`
/// pairs without staging real bundles on disk.
fn newest(entries: &[(String, u64)]) -> Option<String> {
    entries
        .iter()
        .max_by(|a, b| {
            let ka = take_timestamp(&a.0).unwrap_or(a.1);
            let kb = take_timestamp(&b.0).unwrap_or(b.1);
            ka.cmp(&kb).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0))
        })
        .map(|(dir, _)| dir.clone())
}

/// Parse the `take-<unix_ts>` suffix of a bundle directory name into its
/// capture timestamp (unix seconds). Returns `None` for any other shape
/// (library/imported bundles), so the caller falls back to filesystem mtime.
fn take_timestamp(dir: &str) -> Option<u64> {
    let name = std::path::Path::new(dir).file_name()?.to_string_lossy();
    name.strip_prefix("take-")?.parse::<u64>().ok()
}

/// Filesystem mtime of `dir` as unix seconds, or `0` when unavailable.
fn dir_mtime_secs(dir: &std::path::Path) -> u64 {
    std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

    #[test]
    fn newest_picks_highest_take_timestamp() {
        let entries = vec![
            ("/r/take-100".to_string(), 0),
            ("/r/take-300".to_string(), 0),
            ("/r/take-200".to_string(), 0),
        ];
        assert_eq!(newest(&entries), Some("/r/take-300".to_string()));
    }

    #[test]
    fn newest_falls_back_to_mtime_for_non_take_dirs() {
        let entries = vec![
            ("/lib/song-a".to_string(), 50),
            ("/lib/song-b".to_string(), 90),
        ];
        assert_eq!(newest(&entries), Some("/lib/song-b".to_string()));
    }

    #[test]
    fn newest_take_ts_beats_mtime() {
        // A take captured at ts=500 (mtime field 0) wins over an imported
        // bundle whose mtime is 400 but whose name has no take suffix.
        let entries = vec![
            ("/i/imported".to_string(), 400),
            ("/r/take-500".to_string(), 0),
        ];
        assert_eq!(newest(&entries), Some("/r/take-500".to_string()));
    }

    #[test]
    fn newest_empty_is_none() {
        assert_eq!(newest(&[]), None);
    }
}
