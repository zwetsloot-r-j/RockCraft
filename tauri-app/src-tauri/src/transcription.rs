//! Video-backdrop sidecar — a Tauri-only companion file for the edit screen.
//!
//! The M7-tauri-N "transcription" feature lets a user play a Synthesia-style
//! video *behind* the edit grid to hand-transcribe a piece. The reference to
//! that video (its path) and the alignment offset are persisted next to the
//! bundle as a `transcription.json` sidecar — deliberately **outside** the
//! `core` / `crates` bundle schema (`RecordingMeta`), which this feature must
//! not touch. The sidecar is purely a frontend concern, so it lives here.
//!
//! Two commands mirror the bundle save/load lifecycle:
//! - [`transcription_save`] writes `<dir>/transcription.json`.
//! - [`transcription_load`] reads it back, or `None` when absent.
//!
//! The DTO is plain serde; the command bodies are free functions taking a
//! `&Path` so they round-trip in a unit test with no Tauri window.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Sidecar file name written into the bundle directory.
const SIDECAR_NAME: &str = "transcription.json";

/// The persisted backdrop reference — mirrors the frontend `TranscriptionDto`.
///
/// `video` is the path to the backdrop video (absolute, or bundle-relative if
/// the frontend chooses to store it that way). `offset_us` is the alignment
/// offset applied as `videoTime = songTime + offset` (signed: a positive offset
/// means the video runs ahead of the song).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionDto {
    /// Path to the backdrop video file.
    pub video: String,
    /// Alignment offset in microseconds (`videoTime = songTime + offset`).
    pub offset_us: i64,
}

/// Write the backdrop sidecar into `dir` (creating `dir` if needed).
///
/// Overwrites any existing `transcription.json`. Returns an `io::Error`
/// surfaced as a string by the command wrapper.
pub fn transcription_save_inner(dir: &Path, dto: &TranscriptionDto) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(dto)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(dir.join(SIDECAR_NAME), json)
}

/// Read the backdrop sidecar from `dir`.
///
/// Returns `Ok(None)` when the file is absent (the common case — most bundles
/// have no backdrop). A present-but-unparseable file is `Ok(None)` too, so a
/// stale/corrupt sidecar never blocks loading the bundle.
pub fn transcription_load_inner(dir: &Path) -> std::io::Result<Option<TranscriptionDto>> {
    match std::fs::read_to_string(dir.join(SIDECAR_NAME)) {
        Ok(json) => Ok(serde_json::from_str(&json).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Persist the backdrop reference for the bundle at `dir`. Mirrors the bundle
/// save flow (#165): the frontend calls this after `save_bundle` resolves.
#[tauri::command]
pub fn transcription_save(dir: String, dto: TranscriptionDto) -> Result<(), String> {
    transcription_save_inner(Path::new(&dir), &dto).map_err(|e| e.to_string())
}

/// Load the backdrop reference for the bundle at `dir`, or `None` when no
/// sidecar exists. Mirrors the bundle load flow (#165).
#[tauri::command]
pub fn transcription_load(dir: String) -> Result<Option<TranscriptionDto>, String> {
    transcription_load_inner(Path::new(&dir)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp dir for an isolated round-trip (no Tauri window needed).
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rockcraft-transcription-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let dto = TranscriptionDto {
            video: "/abs/path/to/clip.mp4".to_string(),
            offset_us: -123_456,
        };
        transcription_save_inner(&dir, &dto).expect("save should succeed");
        let loaded = transcription_load_inner(&dir).expect("load should succeed");
        assert_eq!(loaded, Some(dto));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_sidecar_is_none() {
        let dir = temp_dir("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let loaded = transcription_load_inner(&dir).expect("load should succeed");
        assert_eq!(loaded, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_is_none() {
        // A directory that does not exist at all also yields None, not an error.
        let dir = temp_dir("nodir");
        let loaded = transcription_load_inner(&dir).expect("load should succeed");
        assert_eq!(loaded, None);
    }

    #[test]
    fn corrupt_sidecar_is_none() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SIDECAR_NAME), "{ not valid json").unwrap();
        let loaded = transcription_load_inner(&dir).expect("load should not error");
        assert_eq!(loaded, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = temp_dir("overwrite");
        let first = TranscriptionDto {
            video: "/a.mp4".to_string(),
            offset_us: 1000,
        };
        let second = TranscriptionDto {
            video: "/b.webm".to_string(),
            offset_us: -2000,
        };
        transcription_save_inner(&dir, &first).expect("first save");
        transcription_save_inner(&dir, &second).expect("second save");
        let loaded = transcription_load_inner(&dir).expect("load");
        assert_eq!(loaded, Some(second));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
