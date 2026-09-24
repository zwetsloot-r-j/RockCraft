//! Segment/slice math — the pure-`core` foundation for splitting a piece.
//!
//! Given a source [`Timeline`] and a piece's optional [`BackingTrack`] /
//! [`BackgroundVideo`], [`slice_segment`] cuts a half-open song-time range
//! `[start_us, end_us)` into a new sub-timeline shifted so the range start
//! becomes time 0, together with the **derived** media references for the new
//! bundle. The media strategy is reference + offset (no re-encode): the file
//! name stays the same, only the offset moves; copying the file is a later
//! step's job. [`segments_from_splits`] turns split points into consecutive
//! segments.
//!
//! Like the rest of `core`, this module is pure: no I/O, no device, no media
//! decoding — data and math only, headless-testable.

use crate::timeline::Note;
use crate::{BackgroundImage, BackgroundVideo, BackingTrack, Timeline};

/// A consecutive slice of a piece's timeline, in song-time microseconds.
/// Half-open: `start_us` inclusive, `end_us` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub start_us: u64,
    pub end_us: u64,
}

/// The sliced piece for one part: a sub-timeline shifted to t=0 plus the media
/// references derived for the new bundle (file names unchanged; offsets shifted).
///
/// Not `Eq`: `backgrounds` carries float transforms (M14-D).
#[derive(Debug, Clone, PartialEq)]
pub struct SliceResult {
    pub timeline: Timeline,
    pub backing: Option<BackingTrack>,
    pub video: Option<BackgroundVideo>,
    /// Background image layers rebased onto the new zero, their animation
    /// resampled so each part opens and closes on the layout the whole piece
    /// had there.
    pub backgrounds: Vec<BackgroundImage>,
}

/// Slice `src` to `seg`, returning a sub-timeline shifted so `seg.start_us`
/// maps to 0, with derived `backing`/`video` references.
///
/// - Notes are included iff `start_us` is in `[seg.start_us, seg.end_us)`
///   (same rule as `notes_in_region`); each kept note's `start_us` is reduced
///   by `seg.start_us`.
/// - Durations are **clipped** so a note never extends past the segment length
///   (`dur_us = min(dur_us, seg.end_us - new_start_us)`), clamped to >= 1 µs.
/// - `backing` => `audio_start_us += seg.start_us` (file unchanged); `None`
///   stays `None`.
/// - `video`   => `offset_us += seg.start_us as i64` (file unchanged); `None`
///   stays `None`.
/// - `backgrounds` => each layer's keyframes are resampled onto the segment
///   (see [`BackgroundImage::sliced`]); an empty list stays empty.
pub fn slice_segment(
    src: &Timeline,
    seg: Segment,
    backing: Option<&BackingTrack>,
    video: Option<&BackgroundVideo>,
    backgrounds: &[BackgroundImage],
) -> SliceResult {
    let seg_len = seg.end_us.saturating_sub(seg.start_us);

    let mut timeline = Timeline::new();
    for (_, note) in src.notes() {
        if note.start_us < seg.start_us || note.start_us >= seg.end_us {
            continue;
        }
        let new_start_us = note.start_us - seg.start_us;
        // Clip the duration so the note never runs past the segment end, but
        // keep it sounding for at least 1 µs.
        let max_dur = seg_len - new_start_us;
        let dur_us = note.dur_us.min(max_dur).max(1);
        timeline.insert(Note {
            pitch: note.pitch,
            start_us: new_start_us,
            dur_us,
            velocity: note.velocity,
            // A part keeps the exceptions the author marked on its notes.
            hand: note.hand,
        });
    }

    let backing = backing.map(|b| BackingTrack {
        file: b.file.clone(),
        audio_start_us: b.audio_start_us + seg.start_us as i64,
    });
    let video = video.map(|v| BackgroundVideo {
        file: v.file.clone(),
        offset_us: v.offset_us + seg.start_us as i64,
    });

    let backgrounds = backgrounds
        .iter()
        .map(|b| b.sliced(seg.start_us, seg.end_us))
        .collect();

    SliceResult {
        timeline,
        backing,
        video,
        backgrounds,
    }
}

/// Turn sorted-or-unsorted split points into consecutive segments covering
/// `[0, total_us)`. Points are clamped to `[0, total_us]`, deduplicated and
/// sorted; empty/zero-width gaps are dropped. No splits => one segment
/// `[0, total_us)`. `total_us == 0` => empty vec.
pub fn segments_from_splits(splits: &[u64], total_us: u64) -> Vec<Segment> {
    if total_us == 0 {
        return Vec::new();
    }

    // Boundaries are 0, the clamped split points, and total_us — sorted and
    // deduped, so consecutive boundaries form non-overlapping segments.
    let mut bounds: Vec<u64> = Vec::with_capacity(splits.len() + 2);
    bounds.push(0);
    bounds.push(total_us);
    for &s in splits {
        bounds.push(s.min(total_us));
    }
    bounds.sort_unstable();
    bounds.dedup();

    bounds
        .windows(2)
        .map(|w| Segment {
            start_us: w[0],
            end_us: w[1],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MidiNote, Velocity};

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
            hand: None,
        }
    }

    /// Notes as `(pitch, start_us, dur_us)` sorted for stable comparison.
    fn rows(tl: &Timeline) -> Vec<(u8, u64, u64)> {
        let mut v: Vec<(u8, u64, u64)> = tl
            .notes()
            .map(|(_, n)| (n.pitch.value(), n.start_us, n.dur_us))
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn note_windowing_and_shift() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 0, 100_000));
        tl.insert(note(62, 1_000_000, 100_000));
        tl.insert(note(64, 2_000_000, 100_000));
        tl.insert(note(65, 3_000_000, 100_000));

        let out = slice_segment(
            &tl,
            Segment {
                start_us: 1_000_000,
                end_us: 3_000_000,
            },
            None,
            None,
            &[],
        );

        // Only the 1 s and 2 s notes survive, shifted to 0 and 1 s. The note at
        // 0 is before the window; the note at 3 s is excluded (exclusive end).
        assert_eq!(
            rows(&out.timeline),
            vec![(62, 0, 100_000), (64, 1_000_000, 100_000)]
        );
    }

    #[test]
    fn duration_clips_to_segment_end() {
        let mut tl = Timeline::new();
        // Note at 2.5 s with 1 s duration would end at 3.5 s.
        tl.insert(note(60, 2_500_000, 1_000_000));

        let out = slice_segment(
            &tl,
            Segment {
                start_us: 0,
                end_us: 3_000_000,
            },
            None,
            None,
            &[],
        );

        // Clipped to end at 3 s → 0.5 s duration at start 2.5 s.
        assert_eq!(rows(&out.timeline), vec![(60, 2_500_000, 500_000)]);
    }

    #[test]
    fn duration_clip_never_below_one_us() {
        let mut tl = Timeline::new();
        // A note starting one µs before the segment end: max_dur == 1.
        tl.insert(note(60, 2_999_999, 1_000_000));

        let out = slice_segment(
            &tl,
            Segment {
                start_us: 0,
                end_us: 3_000_000,
            },
            None,
            None,
            &[],
        );
        assert_eq!(rows(&out.timeline), vec![(60, 2_999_999, 1)]);
    }

    #[test]
    fn backing_offset_shifts_and_keeps_file() {
        let backing = BackingTrack {
            file: "backing.mp3".into(),
            audio_start_us: 250_000,
        };
        let out = slice_segment(
            &Timeline::new(),
            Segment {
                start_us: 1_000_000,
                end_us: 2_000_000,
            },
            Some(&backing),
            None,
            &[],
        );
        let derived = out.backing.unwrap();
        assert_eq!(derived.audio_start_us, 1_250_000);
        assert_eq!(derived.file, "backing.mp3");
    }

    #[test]
    fn backing_none_stays_none() {
        let out = slice_segment(
            &Timeline::new(),
            Segment {
                start_us: 1_000_000,
                end_us: 2_000_000,
            },
            None,
            None,
            &[],
        );
        assert!(out.backing.is_none());
    }

    #[test]
    fn video_offset_shifts_and_keeps_file() {
        let video = BackgroundVideo {
            file: "source.mp4".into(),
            offset_us: -200_000,
        };
        let out = slice_segment(
            &Timeline::new(),
            Segment {
                start_us: 1_000_000,
                end_us: 2_000_000,
            },
            None,
            Some(&video),
            &[],
        );
        let derived = out.video.unwrap();
        assert_eq!(derived.offset_us, 800_000);
        assert_eq!(derived.file, "source.mp4");
    }

    #[test]
    fn video_none_stays_none() {
        let out = slice_segment(
            &Timeline::new(),
            Segment {
                start_us: 1_000_000,
                end_us: 2_000_000,
            },
            None,
            None,
            &[],
        );
        assert!(out.video.is_none());
    }

    #[test]
    fn segments_from_splits_basic() {
        let segs = segments_from_splits(&[1_000_000, 2_000_000], 3_000_000);
        assert_eq!(
            segs,
            vec![
                Segment {
                    start_us: 0,
                    end_us: 1_000_000
                },
                Segment {
                    start_us: 1_000_000,
                    end_us: 2_000_000
                },
                Segment {
                    start_us: 2_000_000,
                    end_us: 3_000_000
                },
            ]
        );
    }

    #[test]
    fn segments_from_splits_no_splits() {
        let segs = segments_from_splits(&[], 3_000_000);
        assert_eq!(
            segs,
            vec![Segment {
                start_us: 0,
                end_us: 3_000_000
            }]
        );
    }

    #[test]
    fn segments_from_splits_clamps_dedupes_sorts() {
        // Unsorted, duplicated, and out-of-range (clamped to total) points.
        let segs =
            segments_from_splits(&[2_000_000, 1_000_000, 2_000_000, 9_000_000, 0], 3_000_000);
        assert_eq!(
            segs,
            vec![
                Segment {
                    start_us: 0,
                    end_us: 1_000_000
                },
                Segment {
                    start_us: 1_000_000,
                    end_us: 2_000_000
                },
                Segment {
                    start_us: 2_000_000,
                    end_us: 3_000_000
                },
            ]
        );
    }

    #[test]
    fn segments_from_splits_zero_total_is_empty() {
        assert_eq!(segments_from_splits(&[1_000_000], 0), Vec::<Segment>::new());
    }

    #[test]
    fn ungapped_split_covers_every_note_exactly_once() {
        let mut tl = Timeline::new();
        // Short notes so durations don't span segment boundaries.
        let originals: Vec<(u8, u64, u64)> = vec![
            (60, 0, 100_000),
            (62, 900_000, 100_000),
            (64, 1_500_000, 100_000),
            (65, 2_800_000, 100_000),
        ];
        for &(p, s, d) in &originals {
            tl.insert(note(p, s, d));
        }

        let total_us = 3_000_000;
        let segs = segments_from_splits(&[1_000_000, 2_000_000], total_us);

        // Re-assemble: slice each segment, then shift every note back by the
        // segment start. The union must equal the originals, no dupes.
        let mut reassembled: Vec<(u8, u64, u64)> = Vec::new();
        for seg in &segs {
            let out = slice_segment(&tl, *seg, None, None, &[]);
            for (_, n) in out.timeline.notes() {
                reassembled.push((n.pitch.value(), n.start_us + seg.start_us, n.dur_us));
            }
        }
        reassembled.sort_unstable();

        let mut expected = originals;
        expected.sort_unstable();
        assert_eq!(reassembled, expected);
    }
}
