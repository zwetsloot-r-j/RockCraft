//! End-to-end coverage for the `SplitBundle` write path (M10-B): slice the
//! loaded piece with `core::segment::slice_segment`, then write each kept part
//! as a standalone bundle with `write_part_bundle`. Verifies copied media,
//! derived offsets, `origin = Edited`, and that the source bundle is untouched.

use std::fs;

use rockcraft_core::{
    slice_segment, BackgroundVideo, BackingTrack, Grid, Key, MidiNote, Note, RecordingMeta, Scale,
    Segment, Timeline, TrackOrigin, Velocity,
};
use rockcraft_import::write_part_bundle;

fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
    Note {
        pitch: MidiNote::new(pitch).unwrap(),
        start_us,
        dur_us,
        velocity: Velocity::new(80).unwrap(),
    }
}

fn four_note_timeline() -> Timeline {
    let mut tl = Timeline::new();
    tl.insert(note(60, 0, 100_000));
    tl.insert(note(62, 1_000_000, 100_000));
    tl.insert(note(64, 2_000_000, 100_000));
    tl.insert(note(65, 3_000_000, 100_000));
    tl
}

fn test_key() -> Key {
    Key {
        root_pc: 2,
        scale: Scale::NaturalMinor,
    }
}

#[test]
fn split_writes_kept_parts_with_copied_media_and_derived_offsets() {
    let root = tempfile::tempdir().unwrap();

    // A "loaded" source bundle with backing + video next to song.mid.
    let src = root.path().join("source");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("song.mid"), b"SOURCE-MIDI").unwrap();
    fs::write(src.join("backing.wav"), b"AUDIO-BYTES").unwrap();
    fs::write(src.join("background.mp4"), b"VIDEO-BYTES").unwrap();
    fs::write(src.join("meta.json"), b"{\"midi_file\":\"song.mid\"}").unwrap();

    let tl = four_note_timeline();
    let backing = BackingTrack {
        file: "backing.wav".into(),
        audio_start_us: 250_000,
    };
    let video = BackgroundVideo {
        file: "background.mp4".into(),
        offset_us: -100_000,
    };

    // Keep [0,1s) and [2s,3s); the middle [1s,2s) is discarded by omission.
    let kept = [
        (
            "intro",
            Segment {
                start_us: 0,
                end_us: 1_000_000,
            },
        ),
        (
            "outro",
            Segment {
                start_us: 2_000_000,
                end_us: 3_000_000,
            },
        ),
    ];

    let out = root.path().join("library");
    let mut dirs = Vec::new();
    for (slug, seg) in kept {
        let sliced = slice_segment(&tl, seg, Some(&backing), Some(&video));
        let dir = out.join(slug);
        let written = write_part_bundle(
            &dir,
            &sliced,
            Grid::default_120(),
            test_key(),
            Some(&src.join("backing.wav")),
            Some(&src.join("background.mp4")),
        )
        .unwrap();
        assert_eq!(written, dir);
        dirs.push((dir, seg));
    }

    for (dir, seg) in &dirs {
        assert!(dir.join("song.mid").exists(), "song.mid written");

        // Media copied verbatim (no re-encode).
        assert_eq!(fs::read(dir.join("backing.wav")).unwrap(), b"AUDIO-BYTES");
        assert_eq!(
            fs::read(dir.join("background.mp4")).unwrap(),
            b"VIDEO-BYTES"
        );

        let meta =
            RecordingMeta::from_json(&fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta.origin, Some(TrackOrigin::Edited));
        assert!(meta.grid.is_some());
        assert_eq!(meta.key, Some(test_key()));

        let b = meta.backing.expect("backing persisted");
        assert_eq!(b.file, "backing.wav");
        assert_eq!(b.audio_start_us, 250_000 + seg.start_us);

        let v = meta.video.expect("video persisted");
        assert_eq!(v.file, "background.mp4");
        assert_eq!(v.offset_us, -100_000 + seg.start_us as i64);
    }

    // The source bundle is left completely untouched.
    assert_eq!(fs::read(src.join("song.mid")).unwrap(), b"SOURCE-MIDI");
    assert_eq!(fs::read(src.join("backing.wav")).unwrap(), b"AUDIO-BYTES");
    assert_eq!(
        fs::read(src.join("background.mp4")).unwrap(),
        b"VIDEO-BYTES"
    );
    assert_eq!(
        fs::read(src.join("meta.json")).unwrap(),
        b"{\"midi_file\":\"song.mid\"}"
    );
}

#[test]
fn split_midi_only_piece_writes_media_less_bundles() {
    let root = tempfile::tempdir().unwrap();
    let tl = four_note_timeline();

    let sliced = slice_segment(
        &tl,
        Segment {
            start_us: 0,
            end_us: 2_000_000,
        },
        None,
        None,
    );
    let dir = root.path().join("part");
    write_part_bundle(&dir, &sliced, Grid::default_120(), test_key(), None, None).unwrap();

    assert!(dir.join("song.mid").exists());
    let meta =
        RecordingMeta::from_json(&fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
    assert!(meta.backing.is_none());
    assert!(meta.video.is_none());
    assert_eq!(meta.origin, Some(TrackOrigin::Edited));
    assert!(!dir.join("backing.wav").exists());
    assert!(!dir.join("background.mp4").exists());
}
