//! Grid-alignment sidecar — a Tauri-only companion file for the edit screen.
//!
//! The M-align "calibration" feature resizes the edit grid (note-space) so an
//! imported Synthesia-style movie registers under it: a vertical zoom
//! (`span_us`), the hit-line position (`hit_frac`), and a horizontal keyboard
//! zoom/pan (`x_scale` / `x_offset`). These are *view* geometry — how this
//! particular frontend overlays the grid on the movie — so, like
//! [`crate::transcription`], they live **outside** the pure `core` bundle schema
//! (`RecordingMeta`) as an `alignment.json` sidecar written next to the bundle.
//!
//! The backdrop video reference and its time `offset_us` already travel in
//! `meta.json` (M9-G); this sidecar carries only the grid-geometry knobs so the
//! alignment a user dials in survives Save-As and travels with the bundle.
//!
//! Two commands mirror the bundle save/load lifecycle:
//! - [`alignment_save`] writes `<dir>/alignment.json`.
//! - [`alignment_load`] reads it back, or `None` when absent.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Sidecar file name written into the bundle directory.
const SIDECAR_NAME: &str = "alignment.json";

/// The persisted grid calibration — mirrors the frontend `AlignmentDto`.
///
/// All four knobs are view geometry for the edit grid: `span_us` is the µs
/// spanned across the canvas height (vertical zoom — smaller = zoomed in),
/// `hit_frac` the fraction of the span kept below the now-line (hit-line
/// position), and `x_scale` / `x_offset` the horizontal keyboard zoom and pan
/// (px) used to line the grid's keys up with the movie's drawn piano.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentDto {
    /// µs across the canvas height (vertical zoom).
    pub span_us: i64,
    /// Fraction of the span kept below the hit/now line.
    pub hit_frac: f64,
    /// Horizontal keyboard zoom (1 = full 88 across the width).
    pub x_scale: f64,
    /// Horizontal keyboard pan in px.
    pub x_offset: f64,
}

/// Write the alignment sidecar into `dir` (creating `dir` if needed).
///
/// Overwrites any existing `alignment.json`. Returns an `io::Error` surfaced as
/// a string by the command wrapper.
pub fn alignment_save_inner(dir: &Path, dto: &AlignmentDto) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(dto)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(dir.join(SIDECAR_NAME), json)
}

/// Read the alignment sidecar from `dir`.
///
/// Returns `Ok(None)` when the file is absent (the common case — most bundles
/// have no backdrop calibration). A present-but-unparseable file is `Ok(None)`
/// too, so a stale/corrupt sidecar never blocks loading the bundle.
pub fn alignment_load_inner(dir: &Path) -> std::io::Result<Option<AlignmentDto>> {
    match std::fs::read_to_string(dir.join(SIDECAR_NAME)) {
        Ok(json) => Ok(serde_json::from_str(&json).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Persist the grid calibration for the bundle at `dir`. Mirrors the bundle
/// save flow: the frontend calls this after `save_bundle` resolves.
#[tauri::command]
pub fn alignment_save(dir: String, dto: AlignmentDto) -> Result<(), String> {
    alignment_save_inner(Path::new(&dir), &dto).map_err(|e| e.to_string())
}

/// Load the grid calibration for the bundle at `dir`, or `None` when no sidecar
/// exists. Mirrors the bundle load flow.
#[tauri::command]
pub fn alignment_load(dir: String) -> Result<Option<AlignmentDto>, String> {
    alignment_load_inner(Path::new(&dir)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp dir for an isolated round-trip (no Tauri window needed).
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rockcraft-alignment-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    fn sample() -> AlignmentDto {
        AlignmentDto {
            span_us: 12_000_000,
            hit_frac: 0.42,
            x_scale: 1.15,
            x_offset: -24.0,
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let dto = sample();
        alignment_save_inner(&dir, &dto).expect("save should succeed");
        let loaded = alignment_load_inner(&dir).expect("load should succeed");
        assert_eq!(loaded, Some(dto));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_sidecar_is_none() {
        let dir = temp_dir("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let loaded = alignment_load_inner(&dir).expect("load should succeed");
        assert_eq!(loaded, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_is_none() {
        let dir = temp_dir("nodir");
        let loaded = alignment_load_inner(&dir).expect("load should succeed");
        assert_eq!(loaded, None);
    }

    #[test]
    fn corrupt_sidecar_is_none() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SIDECAR_NAME), "{ not valid json").unwrap();
        let loaded = alignment_load_inner(&dir).expect("load should not error");
        assert_eq!(loaded, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = temp_dir("overwrite");
        let first = sample();
        let second = AlignmentDto {
            span_us: 8_000_000,
            hit_frac: 0.2,
            x_scale: 0.9,
            x_offset: 40.0,
        };
        alignment_save_inner(&dir, &first).expect("first save");
        alignment_save_inner(&dir, &second).expect("second save");
        let loaded = alignment_load_inner(&dir).expect("load");
        assert_eq!(loaded, Some(second));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
