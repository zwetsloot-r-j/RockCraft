use std::path::{Path, PathBuf};

use midly::{
    num::{u15, u28, u4, u7},
    Header, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind,
};
use rockcraft_core::{BackgroundVideo, BackingTrack, NoteEvent, NoteEventKind, RecordingMeta};

use crate::{error::ImportError, parser::chart_to_timeline, schema::ExtractedChart};

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
