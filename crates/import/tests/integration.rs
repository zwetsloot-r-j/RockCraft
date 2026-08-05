use rockcraft_core::{Key, Scale, TimeSig};
use rockcraft_import::{
    chart_to_timeline, from_json, to_json, write_chart_bundle, ExtractedChart, ExtractedNote, Hand,
    ImportError, NotationMeta, RecordingMeta, SourceMeta,
};
use std::fs;

fn c_major_scale() -> ExtractedChart {
    let pitches: [u8; 8] = [60, 62, 64, 65, 67, 69, 71, 72];
    let notes = pitches
        .iter()
        .enumerate()
        .map(|(i, &pitch)| ExtractedNote {
            pitch,
            start_us: i as u64 * 500_000,
            dur_us: 400_000,
            hand: Hand::Right,
            velocity: None,
            confidence: None,
        })
        .collect();
    ExtractedChart {
        notes,
        source: SourceMeta {
            title: Some("Synthetic C-major scale".into()),
            fps: None,
            scroll_px_per_s: None,
            frame_height_px: None,
            hit_line_px: None,
            extractor_version: "synthetic-test-0.1".into(),
        },
        notation: None,
    }
}

#[test]
fn json_round_trip() {
    let chart = c_major_scale();
    let json = to_json(&chart);
    let back = from_json(&json).unwrap();
    assert_eq!(chart, back);
}

#[test]
fn fixture_parses_and_round_trips() {
    let fixture = include_str!("fixtures/synthetic_chart.json");
    let chart = from_json(fixture).expect("fixture must parse");
    assert_eq!(chart.notes.len(), 8);
    assert_eq!(chart.notes[0].pitch, 60, "first note is C4");
    assert_eq!(chart.notes[7].pitch, 72, "last note is C5");
    let json = to_json(&chart);
    let back = from_json(&json).unwrap();
    assert_eq!(chart, back);
}

#[test]
fn chart_to_timeline_note_count_and_pitches() {
    let chart = c_major_scale();
    let timeline = chart_to_timeline(&chart).unwrap();
    assert_eq!(timeline.len(), 8);
    let pitches: Vec<u8> = timeline.notes().map(|(_, n)| n.pitch.value()).collect();
    assert_eq!(pitches, vec![60, 62, 64, 65, 67, 69, 71, 72]);
}

#[test]
fn chart_to_timeline_timings() {
    let chart = c_major_scale();
    let timeline = chart_to_timeline(&chart).unwrap();
    for (i, (_, note)) in timeline.notes().enumerate() {
        assert_eq!(note.start_us, i as u64 * 500_000);
        assert_eq!(note.dur_us, 400_000);
    }
}

#[test]
fn chart_to_timeline_missing_velocity_defaults() {
    let chart = c_major_scale();
    let timeline = chart_to_timeline(&chart).unwrap();
    for (_, note) in timeline.notes() {
        assert_eq!(
            note.velocity.value(),
            rockcraft_import::parser::DEFAULT_VELOCITY
        );
    }
}

#[test]
fn chart_to_timeline_invalid_pitch_errors() {
    let mut chart = c_major_scale();
    chart.notes[0].pitch = 200;
    let err = chart_to_timeline(&chart).unwrap_err();
    assert!(matches!(err, ImportError::InvalidPitch(200)));
}

#[test]
fn write_bundle_produces_readable_files() {
    let chart = c_major_scale();
    let tmp = tempfile::tempdir().unwrap();
    let bundle_dir = write_chart_bundle(&chart, tmp.path()).unwrap();

    let meta_json = fs::read_to_string(bundle_dir.join("meta.json")).unwrap();
    let meta = RecordingMeta::from_json(&meta_json).expect("meta.json must be valid");
    assert_eq!(meta.midi_file, "song.mid");
    assert_eq!(meta.version, 1);
    assert!(meta.backing.is_none());

    let mid_bytes = fs::read(bundle_dir.join("song.mid")).unwrap();
    assert!(!mid_bytes.is_empty());
    assert_eq!(
        &mid_bytes[..4],
        b"MThd",
        "song.mid must start with MIDI header"
    );

    // Verify song.mid can be parsed by midly
    midly::Smf::parse(&mid_bytes).expect("song.mid must be a valid MIDI file");
}

#[test]
fn write_bundle_rejects_tracked_workspace_path() {
    let chart = c_major_scale();
    // Construct an absolute path inside the workspace source tree
    let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_dir.parent().unwrap().parent().unwrap();
    let bad_path = workspace_root
        .join("crates")
        .join("import")
        .join("tests")
        .join("out");
    let result = write_chart_bundle(&chart, &bad_path);
    assert!(
        matches!(result, Err(ImportError::PathNotAllowed(_))),
        "must refuse to write inside workspace source tree"
    );
}

// ── Notated context (M13-A) ──────────────────────────────────────────────────

/// The `notation` block survives a JSON round-trip intact.
#[test]
fn notation_round_trips() {
    let chart = ExtractedChart {
        notation: Some(NotationMeta {
            bpm: Some(90),
            time_sig: Some(TimeSig {
                beats_per_bar: 3,
                beat_unit: 4,
            }),
            key: Some(Key {
                root_pc: 7,
                scale: Scale::Major,
            }),
        }),
        ..c_major_scale()
    };
    let back = from_json(&to_json(&chart)).unwrap();
    assert_eq!(chart, back);
    assert_eq!(back.notation.unwrap().bpm, Some(90));
}

/// A chart with no `notation` key — every chart the video extractor has ever
/// emitted — must still deserialize, and must not gain the key on the way out.
#[test]
fn chart_without_notation_still_parses() {
    let fixture = include_str!("fixtures/synthetic_chart.json");
    let chart = from_json(fixture).expect("pre-M13-A fixture must still parse");
    assert_eq!(chart.notation, None);
    assert!(
        !to_json(&chart).contains("notation"),
        "a chart with no notation must not serialize the key"
    );
}
