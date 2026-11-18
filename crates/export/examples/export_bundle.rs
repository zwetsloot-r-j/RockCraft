//! Export a saved bundle as MusicXML sheet music, without launching an app.
//!
//! ```text
//! cargo run -p rockcraft-export --example export_bundle -- <bundle-dir> [out.musicxml]
//! ```
//!
//! Reads `song.mid` and `meta.json` from the bundle the same way the editors
//! load it (hands and finger numbers re-applied from the manifest) and writes
//! the same file the `export_score` host command would. The output defaults to
//! `<bundle-dir>/<bundle-name>.musicxml`.

use std::path::PathBuf;
use std::process::ExitCode;

use rockcraft_core::{Grid, Note, RecordingMeta, Timeline};
use rockcraft_export::{export_score_file, title_from_bundle_dir, ScoreSource};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(bundle) = args.next().map(PathBuf::from) else {
        eprintln!("usage: export_bundle <bundle-dir> [out.musicxml]");
        return ExitCode::from(2);
    };
    match run(&bundle, args.next().map(PathBuf::from)) {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(bundle: &std::path::Path, out: Option<PathBuf>) -> Result<PathBuf, String> {
    let midi = std::fs::read(bundle.join("song.mid")).map_err(|e| format!("song.mid: {e}"))?;
    let events =
        rockcraft_midi::smf_bytes_to_events(&midi).map_err(|e| format!("song.mid: {e}"))?;
    let mut timeline = Timeline::from_events(&events);

    // A missing or unreadable manifest is a plain MIDI-only piece.
    let meta = std::fs::read_to_string(bundle.join("meta.json"))
        .ok()
        .and_then(|json| RecordingMeta::from_json(&json).ok())
        .unwrap_or_else(|| RecordingMeta::new_midi_only("song.mid"));
    timeline.apply_hand_overrides(&meta.hand_overrides);
    timeline.apply_fingers(&meta.fingers);

    let notes: Vec<Note> = timeline.notes().map(|(_, n)| *n).collect();
    let grid = meta.grid.unwrap_or_else(Grid::default_120);
    let title = title_from_bundle_dir(Some(bundle));
    let out = out.unwrap_or_else(|| bundle.join(&title));
    export_score_file(
        &ScoreSource {
            notes: &notes,
            grid: &grid,
            bar_starts: &meta.bar_starts,
            key: meta.key,
            hand_split: meta.split_or_default(),
            title: &title,
        },
        &out,
    )
    .map_err(|e| e.to_string())
}
