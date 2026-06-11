//! The track library: a unified view over chart bundles regardless of origin.
//!
//! A "bundle" is the one storage contract (M6-A): a directory containing
//! `song.mid` plus a `meta.json` ([`RecordingMeta`]). Imports, recordings, and
//! compositions all use it. This module locates bundles under one or more root
//! directories and reads enough of each to list it (name, duration, note count,
//! origin, has-backing) — the filesystem half that `core` deliberately omits.
//!
//! Saving a *named* bundle (overwriting one of the same name) is done by
//! [`crate::edit::EditScreen::save_to_library`]; this module owns the shared
//! name→directory slug rule it relies on, plus the listing/scan side.

use std::path::{Path, PathBuf};

use rockcraft_core::{RecordingMeta, Timeline, TrackOrigin};
use rockcraft_midi::smf_bytes_to_events;

/// Environment override for the library root, mostly for tests.
const LIBRARY_DIR_ENV: &str = "ROCKCRAFT_LIBRARY_DIR";

/// Default library root directory name (gitignored, like `recordings/`).
const DEFAULT_LIBRARY_DIR: &str = "library";

/// The library root: `$ROCKCRAFT_LIBRARY_DIR` when set, else `library/` under
/// the current directory. This is where *saved* bundles land.
pub fn library_root() -> PathBuf {
    match std::env::var_os(LIBRARY_DIR_ENV) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_LIBRARY_DIR),
    }
}

/// The roots the browser scans, in display order: the library proper first,
/// then the existing recording and import output trees so every chart the app
/// produces is reachable from one screen. Missing roots are simply skipped.
pub fn default_scan_roots() -> Vec<PathBuf> {
    vec![
        library_root(),
        PathBuf::from("recordings"),
        PathBuf::from("import-out"),
    ]
}

/// Turn a user-typed name into a safe directory name: lowercase, with runs of
/// non-alphanumeric characters collapsed to a single `-`, trimmed of leading and
/// trailing `-`. Returns an empty string if nothing survives (caller rejects).
pub fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// One row in the library browser. Carries the bundle directory (so the shell
/// can reuse the existing `song.mid`/`meta.json` load path) plus the summary
/// columns shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    /// Display name — the bundle directory's file name.
    pub name: String,
    /// The bundle directory (contains `song.mid` + `meta.json`).
    pub dir: PathBuf,
    /// Number of notes in `song.mid`.
    pub note_count: usize,
    /// Total chart duration in microseconds (latest note end). `0` when empty.
    pub duration_us: u64,
    /// Provenance from `meta.json`; `None` for legacy bundles without the field.
    pub origin: Option<TrackOrigin>,
    /// Whether the bundle declares a backing audio track.
    pub has_backing: bool,
}

impl LibraryEntry {
    /// A one-line summary for the list, e.g.
    /// `mysong            12 notes  0:08  composed  +backing`.
    pub fn summary(&self) -> String {
        let secs = self.duration_us / 1_000_000;
        let origin = self.origin.map(|o| o.label()).unwrap_or("unknown");
        let backing = if self.has_backing { "  +backing" } else { "" };
        format!(
            "{:<20} {:>4} notes  {}:{:02}  {}{}",
            self.name,
            self.note_count,
            secs / 60,
            secs % 60,
            origin,
            backing,
        )
    }
}

/// Read and summarise a single bundle directory. Returns `None` when `dir` is
/// not a readable bundle (no `song.mid`, or the MIDI fails to parse) so callers
/// can simply skip non-bundle entries.
pub fn read_entry(dir: &Path) -> Option<LibraryEntry> {
    let midi = dir.join("song.mid");
    let bytes = std::fs::read(&midi).ok()?;
    let events = smf_bytes_to_events(&bytes).ok()?;
    let timeline = Timeline::from_events(&events);
    let note_count = timeline.len();
    let duration_us = timeline
        .notes()
        .map(|(_, n)| n.start_us + n.dur_us)
        .max()
        .unwrap_or(0);

    // meta.json is optional: a loose bundle still lists, just without origin /
    // backing info.
    let (origin, has_backing) = match std::fs::read_to_string(dir.join("meta.json")) {
        Ok(json) => match RecordingMeta::from_json(&json) {
            Ok(meta) => (meta.origin, meta.backing.is_some()),
            Err(_) => (None, false),
        },
        Err(_) => (None, false),
    };

    let name = dir.file_name()?.to_string_lossy().into_owned();
    Some(LibraryEntry {
        name,
        dir: dir.to_path_buf(),
        note_count,
        duration_us,
        origin,
        has_backing,
    })
}

/// List every bundle found under `roots`, sorted by name. A "bundle" is any
/// immediate sub-directory of a root that contains a parseable `song.mid`.
/// Missing roots are skipped; duplicate names from different roots are all kept
/// (each carries its own `dir`).
pub fn list_library(roots: &[PathBuf]) -> Vec<LibraryEntry> {
    let mut entries = Vec::new();
    for root in roots {
        let Ok(read) = std::fs::read_dir(root) else {
            continue;
        };
        for dirent in read.flatten() {
            let path = dirent.path();
            if path.is_dir() {
                if let Some(entry) = read_entry(&path) {
                    entries.push(entry);
                }
            }
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.dir.cmp(&b.dir)));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_collapses_and_trims() {
        assert_eq!(slug("My Song!"), "my-song");
        assert_eq!(slug("  spaced  out  "), "spaced-out");
        assert_eq!(slug("a/b\\c"), "a-b-c");
        assert_eq!(slug("UPPER_case-123"), "upper-case-123");
        assert_eq!(slug("***"), "");
        assert_eq!(slug(""), "");
    }

    #[test]
    fn list_skips_missing_roots() {
        let entries = list_library(&[PathBuf::from("/no/such/dir/at/all")]);
        assert!(entries.is_empty());
    }
}
