use std::path::{Path, PathBuf};

use midly::{
    num::{u15, u28, u4, u7},
    Header, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind,
};
use rockcraft_core::{
    BackgroundVideo, BackingTrack, Grid, Key, NoteEvent, NoteEventKind, RecordingMeta, SliceResult,
    TrackOrigin,
};
use serde::Serialize;

use crate::{
    error::ImportError,
    parser::chart_to_timeline,
    schema::{ExtractedChart, NotationMeta, SourceMeta},
};

/// The canonical gitignored output root, e.g. `<workspace>/import-out`.
///
/// Callers should always use this helper rather than hard-coding the path.
/// The directory itself is created by `write_chart_bundle`; M6-B adds the
/// gitignore entry and CI guard.
pub fn import_output_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/import at compile time; go up two levels
    // to reach the workspace root.
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(crate_dir);
    workspace_root.join("import-out")
}

/// Write `<dir>/song.mid` and `<dir>/meta.json` for the given chart.
///
/// Returns the bundle directory path on success. `dir` is created if it does
/// not yet exist.
///
/// Returns [`ImportError::PathNotAllowed`] if `dir` is an absolute path inside
/// the workspace source tree but outside the gitignored `import-out` root —
/// this prevents accidentally committing extracted data.
pub fn write_chart_bundle(chart: &ExtractedChart, dir: &Path) -> Result<PathBuf, ImportError> {
    write_chart_bundle_with_backing(chart, dir, None)
}

/// Like [`write_chart_bundle`], but records an optional `backing` track in the
/// bundle's `meta.json`.
///
/// The `BackingTrack::file` is a bundle-relative filename (e.g. `"backing.wav"`)
/// — the audio file itself must already live next to `song.mid` in `dir`. Used
/// by the import pipeline to attach the source video's audio as the default
/// backing track (issue #152). `None` is equivalent to [`write_chart_bundle`].
pub fn write_chart_bundle_with_backing(
    chart: &ExtractedChart,
    dir: &Path,
    backing: Option<BackingTrack>,
) -> Result<PathBuf, ImportError> {
    write_chart_bundle_full(chart, dir, backing, None)
}

/// Like [`write_chart_bundle_with_backing`], but also records an optional
/// background `video` reference in the bundle's `meta.json` (M9-G).
///
/// As with `backing`, `BackgroundVideo::file` is a bundle-relative filename — the
/// video file itself must already live next to `song.mid` in `dir`. The import
/// pipeline uses this to retain the original source video so imported pieces come
/// with their backdrop already attached. `None` is equivalent to
/// [`write_chart_bundle_with_backing`].
pub fn write_chart_bundle_full(
    chart: &ExtractedChart,
    dir: &Path,
    backing: Option<BackingTrack>,
    video: Option<BackgroundVideo>,
) -> Result<PathBuf, ImportError> {
    guard_path(dir)?;

    let timeline = chart_to_timeline(chart)?;
    let events = timeline.to_events();

    std::fs::create_dir_all(dir)?;

    let midi_bytes = events_to_smf_bytes(&events);
    std::fs::write(dir.join("song.mid"), &midi_bytes)?;

    let mut meta = RecordingMeta::new_midi_only("song.mid");
    meta.backing = backing;
    meta.video = video;
    meta.origin = Some(rockcraft_core::TrackOrigin::Imported);
    // A source that carries notated context (M13-A score files) seeds the
    // editor's grid and key. Video extraction carries none, so its bundles keep
    // `grid`/`key` null exactly as before.
    if let Some(notation) = chart.notation {
        meta.grid = Some(grid_from_notation(&notation));
        meta.key = notation.key;
    }
    std::fs::write(dir.join("meta.json"), meta.to_json())?;

    // When the bundle carries a source movie and the extractor measured the
    // frame geometry, write the recommended edit-grid calibration so the Tauri
    // overlay registers the imported chart on the movie automatically (no manual
    // zoom). Best-effort: a chart with no video or missing geometry simply gets
    // no sidecar, and the editor falls back to its default zoom.
    if meta.video.is_some() {
        if let Some(cal) = alignment_from_source(&chart.source) {
            if let Ok(json) = serde_json::to_string_pretty(&cal) {
                let _ = std::fs::write(dir.join("alignment.json"), json);
            }
        }
    }

    Ok(dir.to_path_buf())
}

/// Build the bundle's [`Grid`] from a score's notated context.
///
/// Starts from [`Grid::default_120`] so a field the source did not state keeps
/// the editor's default. `bpm` goes through [`Grid::set_bpm`], which clamps to
/// the editable range — an unplayably fast or slow marking lands on a bound
/// rather than failing the import. `subdivision` is deliberately *not* derived
/// from the score's smallest note value: it is a snap preference the user
/// re-picks in the editor.
fn grid_from_notation(notation: &NotationMeta) -> Grid {
    let mut grid = Grid::default_120();
    if let Some(bpm) = notation.bpm {
        grid.set_bpm(bpm);
    }
    if let Some(time_sig) = notation.time_sig {
        grid.time_sig = time_sig;
    }
    grid
}

/// Recommended initial edit-grid calibration, written next to the bundle as
/// `alignment.json`.
///
/// The field names are the contract shared with the Tauri reader
/// (`tauri-app/src-tauri/src/alignment.rs::AlignmentDto` and the frontend
/// `AlignmentDto`); this crate must not depend on the frontend, so the shape is
/// duplicated here intentionally. Only the vertical knobs are derived — `x_scale`
/// / `x_offset` default to the identity (keyboard left/right alignment stays a
/// manual touch-up in the ALIGN overlay).
#[derive(Serialize)]
struct AlignmentSidecar {
    /// µs spanned across the canvas height (vertical zoom).
    span_us: i64,
    /// Fraction of the span kept below the hit/now line.
    hit_frac: f64,
    /// Horizontal keyboard zoom (1 = full 88 across the width).
    x_scale: f64,
    /// Horizontal keyboard pan in px.
    x_offset: f64,
}

/// Derive the grid calibration from the extractor's frame geometry, or `None`
/// when any input is missing or degenerate.
///
/// The source movie is drawn stretched to fill the canvas, so the full native
/// frame height always maps onto the canvas height. A note `Δt` before the
/// hit-line sits `Δt · scroll` px above it in the movie; matching that in the
/// grid requires the canvas to span exactly one frame-height worth of song-time
/// (`span_us = frame_height · 1e6/scroll`) with the now-line at the hit row
/// (`hit_frac = (H − hit)/H`). Both are independent of the canvas size.
fn alignment_from_source(source: &SourceMeta) -> Option<AlignmentSidecar> {
    let scroll = f64::from(source.scroll_px_per_s?);
    let h = f64::from(source.frame_height_px?);
    let hit = f64::from(source.hit_line_px?);
    if scroll <= 0.0 || h <= 0.0 || !(0.0..=h).contains(&hit) {
        return None;
    }
    let us_per_px = 1e6 / scroll;
    let span_us = (h * us_per_px).round() as i64;
    if span_us <= 0 {
        return None;
    }
    Some(AlignmentSidecar {
        span_us,
        hit_frac: (h - hit) / h,
        x_scale: 1.0,
        x_offset: 0.0,
    })
}

/// Write one kept part of a split piece as its own standalone bundle (M10-B).
///
/// `sliced` is the [`SliceResult`] from `core::segment::slice_segment` for this
/// part: a sub-timeline already shifted to t=0, plus the **derived**
/// `backing`/`video` references whose offsets have been shifted by the segment
/// start (file names unchanged) and the background image layers whose keyframes
/// have been resampled onto the segment. `backing_src` / `video_src` are the
/// absolute source paths of the media in the loaded bundle; each is copied
/// **unchanged** (no re-encode, no trim) into the new bundle under the
/// bundle-relative name carried by `sliced.backing.file` / `sliced.video.file`.
/// `background_srcs` pairs each background layer id with its absolute source
/// image, copied the same way under `sliced.backgrounds[..].file`. `grid`/`key`
/// and `hand_split` are copied from the source piece — the part's per-note hand
/// exceptions ride along on `sliced.timeline` — and the new bundle's `origin` is
/// recorded as [`TrackOrigin::Edited`].
///
/// Writes `<dir>/song.mid`, the copied media file(s) (when present), and
/// `<dir>/meta.json`; returns the bundle directory path. `dir` is created if it
/// does not yet exist. The source bundle is never touched.
///
/// This is the shared write half of `HostCommand::SplitBundle`: each frontend
/// builds the per-segment [`SliceResult`] from its own live editor state and
/// calls this helper, rather than duplicating the meta-writing logic. It is
/// kept distinct from [`write_chart_bundle_full`] because the loaded editor
/// timeline is not an [`ExtractedChart`], it carries `grid`/`key`, and its
/// `origin` is `Edited` rather than `Imported`.
#[allow(clippy::too_many_arguments)]
pub fn write_part_bundle(
    dir: &Path,
    sliced: &SliceResult,
    grid: Grid,
    key: Key,
    backing_src: Option<&Path>,
    video_src: Option<&Path>,
    background_srcs: &[(String, PathBuf)],
    hand_split: u8,
) -> Result<PathBuf, ImportError> {
    std::fs::create_dir_all(dir)?;

    let midi_bytes = events_to_smf_bytes(&sliced.timeline.to_events());
    std::fs::write(dir.join("song.mid"), &midi_bytes)?;

    // Copy the media as-is into the new bundle, under the derived (unchanged)
    // filename. A source path with no matching derived reference is a no-op.
    if let (Some(src), Some(backing)) = (backing_src, sliced.backing.as_ref()) {
        std::fs::copy(src, dir.join(&backing.file))?;
    }
    if let (Some(src), Some(video)) = (video_src, sliced.video.as_ref()) {
        std::fs::copy(src, dir.join(&video.file))?;
    }
    for layer in &sliced.backgrounds {
        if let Some((_, src)) = background_srcs.iter().find(|(id, _)| *id == layer.id) {
            std::fs::copy(src, dir.join(&layer.file))?;
        }
    }

    let meta = RecordingMeta {
        midi_file: "song.mid".into(),
        backing: sliced.backing.clone(),
        grid: Some(grid),
        key: Some(key),
        origin: Some(TrackOrigin::Edited),
        video: sliced.video.clone(),
        backgrounds: sliced.backgrounds.clone(),
        // The part inherits the source piece's hand assignment: its split line
        // and the exceptions on the notes it kept (M14-E).
        hand_split: Some(hand_split),
        hand_overrides: sliced.timeline.hand_overrides(),
        version: 1,
    };
    std::fs::write(dir.join("meta.json"), meta.to_json())?;

    Ok(dir.to_path_buf())
}

/// Reject absolute paths that are inside the workspace source tree but outside
/// the gitignored `import-out` output root. Relative paths are allowed through
/// because the calling context determines their meaning.
fn guard_path(dir: &Path) -> Result<(), ImportError> {
    if !dir.is_absolute() {
        return Ok(());
    }

    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = match crate_dir.parent().and_then(|p| p.parent()) {
        Some(p) => p.to_path_buf(),
        None => return Ok(()),
    };
    let import_out = import_output_dir();

    if dir.starts_with(&workspace_root) && !dir.starts_with(&import_out) {
        return Err(ImportError::PathNotAllowed(format!(
            "\"{}\" is inside the workspace source tree; \
             write to {} instead",
            dir.display(),
            import_out.display()
        )));
    }

    Ok(())
}

/// Serialize note events to Standard MIDI File bytes (single track, channel 0).
///
/// Delta times are in ticks with 1 tick = 1 µs, matching the convention used
/// by `rockcraft-midi`. Timestamps beyond ~4.5 minutes (u28 max) are silently
/// truncated — sufficient for import chart segments.
fn events_to_smf_bytes(events: &[NoteEvent]) -> Vec<u8> {
    let mut track = Track::new();
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|e| e.timestamp_us);

    let mut prev_us: u64 = 0;
    for ev in &sorted {
        let delta_us = ev.timestamp_us - prev_us;
        let delta = u28::from(delta_us as u32);
        prev_us = ev.timestamp_us;

        let kind = match ev.kind {
            NoteEventKind::On { velocity } => TrackEventKind::Midi {
                channel: u4::from(0u8),
                message: MidiMessage::NoteOn {
                    key: u7::from(ev.note.value()),
                    vel: u7::from(velocity.value()),
                },
            },
            NoteEventKind::Off => TrackEventKind::Midi {
                channel: u4::from(0u8),
                message: MidiMessage::NoteOff {
                    key: u7::from(ev.note.value()),
                    vel: u7::from(0u8),
                },
            },
        };
        track.push(TrackEvent { delta, kind });
    }

    let smf = Smf {
        header: Header {
            format: midly::Format::SingleTrack,
            timing: Timing::Metrical(u15::from(480u16)),
        },
        tracks: vec![track],
    };

    let mut bytes = Vec::new();
    smf.write_std(&mut bytes)
        .expect("MIDI serialization is infallible");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ExtractedNote, Hand};
    use tempfile::TempDir;

    fn chart_with_source(source: SourceMeta) -> ExtractedChart {
        ExtractedChart {
            notes: vec![ExtractedNote {
                pitch: 60,
                start_us: 0,
                dur_us: 500_000,
                hand: Hand::Right,
                velocity: Some(80),
                confidence: None,
            }],
            source,
            notation: None,
        }
    }

    fn geometry_source() -> SourceMeta {
        SourceMeta {
            title: None,
            fps: Some(30.0),
            scroll_px_per_s: Some(200.0),
            frame_height_px: Some(360),
            hit_line_px: Some(300),
            extractor_version: "test".into(),
        }
    }

    fn video() -> BackgroundVideo {
        BackgroundVideo {
            file: "source.mp4".into(),
            offset_us: 0,
        }
    }

    /// A video-backed chart with frame geometry writes `alignment.json` with the
    /// vertical calibration derived from the movie: span = H·1e6/scroll, hit_frac
    /// = (H−hit)/H, identity horizontal knobs.
    #[test]
    fn alignment_sidecar_written_from_geometry() {
        let tmp = TempDir::new().unwrap();
        let chart = chart_with_source(geometry_source());
        write_chart_bundle_full(&chart, tmp.path(), None, Some(video())).unwrap();

        let json = std::fs::read_to_string(tmp.path().join("alignment.json"))
            .expect("alignment.json should be written");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // scroll 200 px/s → 5000 µs/px; 360 px tall → 1.8 s span; hit at 300/360.
        assert_eq!(v["span_us"].as_i64(), Some(1_800_000));
        assert!((v["hit_frac"].as_f64().unwrap() - (60.0 / 360.0)).abs() < 1e-9);
        assert_eq!(v["x_scale"].as_f64(), Some(1.0));
        assert_eq!(v["x_offset"].as_f64(), Some(0.0));
    }

    /// No movie → no calibration sidecar (it would have nothing to register on).
    #[test]
    fn no_sidecar_without_video() {
        let tmp = TempDir::new().unwrap();
        let chart = chart_with_source(geometry_source());
        write_chart_bundle_full(&chart, tmp.path(), None, None).unwrap();
        assert!(!tmp.path().join("alignment.json").exists());
    }

    /// A movie but no measured geometry → no sidecar (editor uses its default zoom).
    #[test]
    fn no_sidecar_without_geometry() {
        let tmp = TempDir::new().unwrap();
        let source = SourceMeta {
            scroll_px_per_s: Some(200.0),
            frame_height_px: None,
            hit_line_px: None,
            ..geometry_source()
        };
        write_chart_bundle_full(&chart_with_source(source), tmp.path(), None, Some(video()))
            .unwrap();
        assert!(!tmp.path().join("alignment.json").exists());
    }

    /// A degenerate hit-line outside the frame is rejected rather than producing
    /// a nonsensical (e.g. >1 or negative) hit-fraction.
    #[test]
    fn degenerate_geometry_rejected() {
        let source = SourceMeta {
            hit_line_px: Some(9999),
            ..geometry_source()
        };
        assert!(alignment_from_source(&source).is_none());
    }

    // ── Notated context → meta.grid / meta.key (M13-A) ───────────────────────

    fn plain_source() -> SourceMeta {
        SourceMeta {
            title: None,
            fps: None,
            scroll_px_per_s: None,
            frame_height_px: None,
            hit_line_px: None,
            extractor_version: "score-import-0.1".into(),
        }
    }

    /// Write a bundle for a chart carrying `notation` and read its `meta.json` back.
    fn meta_for_notation(tmp: &TempDir, notation: Option<NotationMeta>) -> RecordingMeta {
        let chart = ExtractedChart {
            notation,
            ..chart_with_source(plain_source())
        };
        write_chart_bundle_full(&chart, tmp.path(), None, None).unwrap();
        let json = std::fs::read_to_string(tmp.path().join("meta.json")).unwrap();
        RecordingMeta::from_json(&json).unwrap()
    }

    /// A score's notated tempo, metre and key land on the bundle's grid/key so
    /// the piece opens in the editor already snapping to its own bars.
    #[test]
    fn notation_populates_grid_and_key() {
        let tmp = TempDir::new().unwrap();
        let meta = meta_for_notation(
            &tmp,
            Some(NotationMeta {
                bpm: Some(90),
                time_sig: Some(rockcraft_core::TimeSig {
                    beats_per_bar: 3,
                    beat_unit: 4,
                }),
                key: Some(Key {
                    root_pc: 7,
                    scale: rockcraft_core::Scale::Major,
                }),
            }),
        );

        let grid = meta.grid.expect("notation must produce a grid");
        assert_eq!(grid.bpm, 90);
        assert_eq!(grid.time_sig.beats_per_bar, 3);
        assert_eq!(grid.time_sig.beat_unit, 4);
        assert_eq!(
            grid.subdivision,
            rockcraft_core::Subdivision::Sixteenth,
            "subdivision stays the editor default — it is a snap preference, \
             not a property of the score"
        );
        assert_eq!(
            meta.key,
            Some(Key {
                root_pc: 7,
                scale: rockcraft_core::Scale::Major,
            })
        );
    }

    /// The video path carries no notation, and must keep writing `grid`/`key`
    /// as null exactly as it did before M13-A.
    #[test]
    fn no_notation_leaves_grid_and_key_null() {
        let tmp = TempDir::new().unwrap();
        let meta = meta_for_notation(&tmp, None);
        assert_eq!(meta.grid, None);
        assert_eq!(meta.key, None);
    }

    /// A tempo outside the editable range clamps rather than failing the import.
    #[test]
    fn notation_bpm_is_clamped() {
        for (notated, expected) in [(5u32, Grid::MIN_BPM), (5000, Grid::MAX_BPM)] {
            let tmp = TempDir::new().unwrap();
            let meta = meta_for_notation(
                &tmp,
                Some(NotationMeta {
                    bpm: Some(notated),
                    time_sig: None,
                    key: None,
                }),
            );
            assert_eq!(meta.grid.unwrap().bpm, expected, "bpm {notated} clamps");
        }
    }

    /// A notation block that states nothing still yields the editor default grid
    /// (and no key) — partial notation must never leave a half-built grid.
    #[test]
    fn empty_notation_yields_default_grid() {
        let tmp = TempDir::new().unwrap();
        let meta = meta_for_notation(
            &tmp,
            Some(NotationMeta {
                bpm: None,
                time_sig: None,
                key: None,
            }),
        );
        assert_eq!(meta.grid, Some(Grid::default_120()));
        assert_eq!(meta.key, None);
    }
}
