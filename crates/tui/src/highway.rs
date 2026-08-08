//! Pure note-highway model — no terminal, no I/O.
//!
//! A highway turns a song (a list of `NoteEvent`s) into sustained **note spans**
//! and projects them onto a vertical time axis: notes fall from the top and
//! reach the keyboard line ("now") at the bottom. Horizontal placement reuses
//! the column geometry in [`crate::keyboard`]; this module only adds the time
//! axis, and stays headless so the projection math is unit-tested in CI.

use rockcraft_core::{NoteEvent, NoteEventKind};

/// A sustained note: a pitch held from `start_us` until `end_us`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteSpan {
    pub note: u8,
    pub start_us: u64,
    pub end_us: u64,
}

/// Build sustained spans from a time-ordered event stream by pairing each
/// note-on with the next note-off (or note-on vel 0) of the same pitch.
///
/// Unmatched note-ons (no off before the song ends) are closed at the last
/// timestamp seen, so a dangling held note still renders.
pub fn build_spans(events: &[NoteEvent]) -> Vec<NoteSpan> {
    // pitch -> (start_us) of an open note.
    let mut open: std::collections::HashMap<u8, u64> = std::collections::HashMap::new();
    let mut spans = Vec::new();
    let mut last_us = 0u64;

    for ev in events {
        last_us = last_us.max(ev.timestamp_us);
        let pitch = ev.note.value();
        match ev.kind {
            NoteEventKind::On { velocity } if velocity.value() > 0 => {
                // A re-press while open closes the previous span first.
                if let Some(start) = open.remove(&pitch) {
                    spans.push(NoteSpan {
                        note: pitch,
                        start_us: start,
                        end_us: ev.timestamp_us,
                    });
                }
                open.insert(pitch, ev.timestamp_us);
            }
            _ => {
                if let Some(start) = open.remove(&pitch) {
                    spans.push(NoteSpan {
                        note: pitch,
                        start_us: start,
                        end_us: ev.timestamp_us,
                    });
                }
            }
        }
    }

    // Close any still-open notes at the end of the song.
    for (pitch, start) in open {
        spans.push(NoteSpan {
            note: pitch,
            start_us: start,
            end_us: last_us.max(start + 1),
        });
    }

    spans.sort_by_key(|s| (s.start_us, s.note));
    spans
}

/// Total song length = the latest end across all spans (0 if empty).
pub fn song_duration_us(spans: &[NoteSpan]) -> u64 {
    spans.iter().map(|s| s.end_us).max().unwrap_or(0)
}

/// A span's projected vertical extent on the highway, in rows from the top.
/// `top_row` is the higher-up (later-in-time) edge, `bottom_row` the edge
/// nearer the keyboard. Both are inclusive and within `0..height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSpan {
    pub note: u8,
    pub top_row: u16,
    pub bottom_row: u16,
}

/// Rows shaved off a span's **trailing** edge so repeated notes read apart.
///
/// Why two and not one: a note's end and the next same-pitch note's onset are
/// the *same* instant, so [`project`] maps them to the same row — the raw
/// extents always overlap there. The first trimmed row only hands that shared
/// row back to the later note; the second is the row that actually reads as a
/// gap. The cost is a flat two rows however long the note is, so sustains
/// barely notice it.
const TAIL_GAP_ROWS: u16 = 2;

impl RowSpan {
    /// The rows to actually paint for this span, with the **trailing**
    /// (later-in-time) edge shaved back — that's the top, since notes fall
    /// toward the keyboard line at the bottom.
    ///
    /// Two same-pitch notes played back to back share an edge: the first one's
    /// end is the second one's onset, so their blocks touch and read as one
    /// unbroken bar. Blanking the trailing rows puts a gap between them. The
    /// onset row is never given up, and a span always keeps at least one row —
    /// a note that vanished would be worse than one that touches.
    ///
    /// Render-only: [`project`] still reports the true extent; this trims what
    /// is drawn, never the timing.
    pub fn body_rows(&self) -> std::ops::RangeInclusive<u16> {
        let top = self
            .top_row
            .saturating_add(TAIL_GAP_ROWS)
            .min(self.bottom_row);
        top..=self.bottom_row
    }
}

/// Project a note span onto a highway of `height` rows, given the current play
/// time `now_us` and a `lead_us` window (how far into the future the top of the
/// highway represents). Returns `None` if the span is not currently visible.
///
/// Mapping: a time `t` maps to fraction `f = (t - now) / lead`, where `f = 0` is
/// the keyboard line (bottom row) and `f = 1` is the top row. As `now`
/// advances, notes move downward toward the keyboard.
pub fn project(span: &NoteSpan, now_us: u64, lead_us: u64, height: u16) -> Option<RowSpan> {
    if height == 0 || lead_us == 0 {
        return None;
    }
    let window_end = now_us + lead_us;
    // Visible only if the span overlaps [now, now + lead].
    if span.end_us < now_us || span.start_us > window_end {
        return None;
    }

    let h = height as i64;
    // row_from_top(t) = (1 - (t - now)/lead) * (h - 1)
    let row_of = |t: u64| -> i64 {
        let dt = t as i64 - now_us as i64;
        let frac_num = lead_us as i64 - dt; // = (1 - dt/lead) * lead
                                            // (frac_num / lead) * (h - 1), done in integer with rounding.
        let val = frac_num * (h - 1);
        (val + lead_us as i64 / 2) / lead_us as i64
    };

    // Later time (end) is higher up = smaller row number.
    let top = row_of(span.end_us).clamp(0, h - 1);
    let bottom = row_of(span.start_us).clamp(0, h - 1);
    Some(RowSpan {
        note: span.note,
        top_row: top.min(bottom) as u16,
        bottom_row: top.max(bottom) as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, NoteEvent, Velocity};

    fn on(note: u8, t: u64) -> NoteEvent {
        NoteEvent::on(MidiNote::new(note).unwrap(), Velocity::new(80).unwrap(), t)
    }
    fn off(note: u8, t: u64) -> NoteEvent {
        NoteEvent::off(MidiNote::new(note).unwrap(), t)
    }

    #[test]
    fn pairs_on_off_into_spans() {
        let evs = vec![on(60, 1000), off(60, 3000), on(64, 2000), off(64, 5000)];
        let spans = build_spans(&evs);
        assert_eq!(spans.len(), 2);
        // sorted by start
        assert_eq!(
            spans[0],
            NoteSpan {
                note: 60,
                start_us: 1000,
                end_us: 3000
            }
        );
        assert_eq!(
            spans[1],
            NoteSpan {
                note: 64,
                start_us: 2000,
                end_us: 5000
            }
        );
    }

    #[test]
    fn dangling_note_on_is_closed_at_end() {
        let evs = vec![on(60, 1000), on(64, 2000), off(64, 4000)];
        let spans = build_spans(&evs);
        // note 60 never released -> closed at last timestamp (4000)
        let s60 = spans.iter().find(|s| s.note == 60).unwrap();
        assert_eq!(s60.end_us, 4000);
    }

    #[test]
    fn overlapping_chord_spans() {
        let evs = vec![
            on(60, 0),
            on(64, 0),
            on(67, 0),
            off(60, 1000),
            off(64, 1000),
            off(67, 1000),
        ];
        let spans = build_spans(&evs);
        assert_eq!(spans.len(), 3);
        assert!(spans.iter().all(|s| s.start_us == 0 && s.end_us == 1000));
    }

    #[test]
    fn song_duration_is_latest_end() {
        let spans = build_spans(&[on(60, 0), off(60, 2000), on(62, 1000), off(62, 5000)]);
        assert_eq!(song_duration_us(&spans), 5000);
        assert_eq!(song_duration_us(&[]), 0);
    }

    #[test]
    fn note_at_now_is_at_bottom() {
        // span exactly at now -> bottom row (height-1)
        let span = NoteSpan {
            note: 60,
            start_us: 0,
            end_us: 1,
        };
        let p = project(&span, 0, 1_000_000, 10).unwrap();
        assert_eq!(p.bottom_row, 9); // height-1
    }

    #[test]
    fn note_at_lead_is_at_top() {
        // a note starting exactly lead_us in the future -> top row
        let span = NoteSpan {
            note: 60,
            start_us: 1_000_000,
            end_us: 1_000_001,
        };
        let p = project(&span, 0, 1_000_000, 10).unwrap();
        assert_eq!(p.top_row, 0);
    }

    #[test]
    fn note_falls_as_now_advances() {
        let span = NoteSpan {
            note: 60,
            start_us: 500_000,
            end_us: 500_001,
        };
        let early = project(&span, 0, 1_000_000, 100).unwrap();
        let later = project(&span, 250_000, 1_000_000, 100).unwrap();
        // As now advances, the note moves DOWN (toward larger row numbers).
        assert!(later.bottom_row > early.bottom_row);
    }

    #[test]
    fn out_of_window_is_not_visible() {
        let span = NoteSpan {
            note: 60,
            start_us: 5_000_000,
            end_us: 5_001_000,
        };
        // far in the future, beyond lead
        assert!(project(&span, 0, 1_000_000, 10).is_none());
        // already past
        let past = NoteSpan {
            note: 60,
            start_us: 0,
            end_us: 100,
        };
        assert!(project(&past, 1_000_000, 1_000_000, 10).is_none());
    }

    #[test]
    fn body_rows_trims_the_trailing_edge() {
        let rs = RowSpan {
            note: 60,
            top_row: 2,
            bottom_row: 8,
        };
        // The two topmost (later-in-time) rows are left blank; the onset row
        // at the bottom is untouched.
        assert_eq!(rs.body_rows().collect::<Vec<_>>(), vec![4, 5, 6, 7, 8]);
    }

    #[test]
    fn body_rows_always_keeps_the_onset_row() {
        for (top, bottom) in [(4u16, 4u16), (4, 5), (4, 6)] {
            let rs = RowSpan {
                note: 60,
                top_row: top,
                bottom_row: bottom,
            };
            let rows: Vec<u16> = rs.body_rows().collect();
            assert!(!rows.is_empty(), "{top}..={bottom} painted nothing");
            assert_eq!(*rows.last().unwrap(), bottom, "onset row was trimmed");
        }
    }

    #[test]
    fn back_to_back_same_pitch_notes_are_separated() {
        // Two adjacent notes on the same pitch: the first ends exactly where
        // the second starts, so their projected blocks touch — the "one long
        // bar" look this trim exists to break up.
        let evs = vec![
            on(60, 0),
            off(60, 500_000),
            on(60, 500_000),
            off(60, 1_000_000),
        ];
        let spans = build_spans(&evs);
        assert_eq!(spans.len(), 2);
        // `a` is the lower (earlier) block, `b` the upper (later) one.
        let a = project(&spans[0], 0, 1_000_000, 20).unwrap();
        let b = project(&spans[1], 0, 1_000_000, 20).unwrap();
        // Raw extents meet (they even share the boundary row) — no gap at all.
        assert!(a.top_row <= b.bottom_row);
        // Painted bodies leave at least one blank row between the two blocks.
        let painted: Vec<u16> = a.body_rows().chain(b.body_rows()).collect();
        let hi = b.body_rows().min().unwrap(); // higher up = smaller row
        let lo = a.body_rows().max().unwrap();
        assert!(
            (hi..=lo).any(|r| !painted.contains(&r)),
            "expected a blank row between the two blocks: {painted:?}"
        );
    }

    #[test]
    fn sustained_note_spans_multiple_rows() {
        let span = NoteSpan {
            note: 60,
            start_us: 0,
            end_us: 1_000_000,
        };
        let p = project(&span, 0, 1_000_000, 10).unwrap();
        // start at now (bottom), end a full lead away (top) -> spans whole height
        assert_eq!(p.bottom_row, 9);
        assert_eq!(p.top_row, 0);
    }
}
