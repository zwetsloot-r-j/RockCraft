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
    let row_of = |t: u64| row_of_time(t, now_us, lead_us, height);

    // Later time (end) is higher up = smaller row number.
    let top = row_of(span.end_us).clamp(0, h - 1);
    let bottom = row_of(span.start_us).clamp(0, h - 1);
    Some(RowSpan {
        note: span.note,
        top_row: top.min(bottom) as u16,
        bottom_row: top.max(bottom) as u16,
    })
}

/// The (unclamped) highway row of song time `t`: row 0 is `now + lead` (the
/// top), row `height - 1` is `now` (the keyboard line). The one time→row
/// mapping every highway layer shares, so notes, lines, and guides agree.
pub fn row_of_time(t: u64, now_us: u64, lead_us: u64, height: u16) -> i64 {
    let h = height as i64;
    // row_from_top(t) = (1 - (t - now)/lead) * (h - 1), in integers, rounded.
    let dt = t as i64 - now_us as i64;
    let frac_num = lead_us as i64 - dt; // = (1 - dt/lead) * lead
    let val = frac_num * (h - 1);
    (val + lead_us as i64 / 2).div_euclid(lead_us.max(1) as i64)
}

// ── sub-row geometry (smooth scrolling) ──────────────────────────────────────
//
// A terminal cell can be split vertically into eighths with the Unicode "lower
// block" glyphs (`▁▂▃▄▅▆▇█`). Positioning note edges and grid lines to the
// eighth, instead of the whole row, makes the highway scroll ~8× finer, and
// lets a note's onset edge sit exactly where its bar line is drawn.

/// Sub-row steps per terminal row.
pub const SUB: i64 = 8;

/// The highway position of song time `t`, in eighths of a row from the top:
/// `now + lead` is the top edge (0), `now` the bottom edge (`height × 8`) —
/// the keyboard line. Unclamped; rounded to the nearest eighth.
pub fn y8_of_time(t: u64, now_us: u64, lead_us: u64, height: u16) -> i64 {
    let e = height as i64 * SUB;
    let lead = lead_us.max(1) as i64;
    let dt = t as i64 - now_us as i64;
    e - (dt * e + lead / 2).div_euclid(lead)
}

/// Eighths shaved off a note's **trailing** (top) edge so back-to-back notes
/// on one pitch read as separate blocks — half a row.
const TAIL_GAP_8: i64 = 4;
/// A note always keeps at least this many eighths, however short.
const MIN_NOTE_8: i64 = 2;

/// A span's visible extent `[top, bottom)` in eighths, clipped to the highway,
/// with the trailing edge trimmed (see [`TAIL_GAP_8`]). `bottom` is the onset:
/// it is exact, so a note on a downbeat sits on its bar line. `None` when the
/// span is off-screen.
pub fn span_extent8(span: &NoteSpan, now_us: u64, lead_us: u64, height: u16) -> Option<(i64, i64)> {
    let e = height as i64 * SUB;
    if height == 0 {
        return None;
    }
    let bottom = y8_of_time(span.start_us, now_us, lead_us, height);
    let top_raw = y8_of_time(span.end_us, now_us, lead_us, height);
    let top = (top_raw + TAIL_GAP_8).min(bottom - MIN_NOTE_8);
    let (top, bottom) = (top.max(0), bottom.min(e));
    (top < bottom).then_some((top, bottom))
}

/// How a note covers one terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellFill {
    /// The whole cell.
    Full,
    /// The bottom `n` eighths (`1..=7`).
    Bottom(u8),
    /// The top `n` eighths (`1..=7`).
    Top(u8),
}

/// The cells a `[top, bottom)` eighths extent covers, as `(row, fill)` from top
/// to bottom. A note smaller than a cell that sits inside one keeps its onset
/// (bottom) edge exact and grows upward to the cell top — a cell can show only
/// one edge, and the onset is the one that matters.
pub fn cells_of_extent(top: i64, bottom: i64) -> Vec<(u16, CellFill)> {
    let mut out = Vec::new();
    if bottom <= top || bottom <= 0 {
        return out;
    }
    let first = top.div_euclid(SUB);
    let last = (bottom - 1).div_euclid(SUB);
    for row in first.max(0)..=last {
        let (c0, c1) = (row * SUB, row * SUB + SUB);
        let (a, b) = (top.max(c0), bottom.min(c1));
        let fill = if a == c0 && b == c1 {
            CellFill::Full
        } else if b == c1 {
            CellFill::Bottom((b - a) as u8)
        } else {
            CellFill::Top((b - c0) as u8)
        };
        out.push((row as u16, fill));
    }
    out
}

/// The glyph drawing `n` eighths filled from the bottom of a cell (`1..=8`).
pub fn lower_block(n: u8) -> &'static str {
    ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][(n.clamp(1, 8) - 1) as usize]
}

/// Where a thin horizontal line at `y8` lands: its row, and the glyph that
/// draws it at the right height within the cell (top edge → middle → bottom).
pub fn line_cell(y8: i64) -> (i64, &'static str) {
    let (row, frac) = (y8.div_euclid(SUB), y8.rem_euclid(SUB));
    let glyph = match frac {
        0 | 1 => "▔",
        2 => "⎺",
        3..=5 => "─",
        6 => "⎽",
        _ => "▁",
    };
    (row, glyph)
}

/// A horizontal grid line on the highway: its position in eighths from the
/// top (see [`y8_of_time`]), and whether it is a bar line (downbeat) or a beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridRow {
    pub y8: i64,
    pub bar: bool,
}

/// Fewest rows a beat must span for beat lines to be drawn. Below this (a fast
/// tempo, or a short terminal) they would crowd into a ruled page and swamp the
/// notes; bar lines are drawn regardless.
pub const MIN_BEAT_ROWS: u64 = 3;

/// The bar and beat lines visible on a `height`-row highway at `now_us`.
///
/// Bars come from `bars` (the piece's tempo map, or uniform bars), each split
/// into `beats_per_bar` even beats. Beat lines are left out when a beat spans
/// fewer than [`MIN_BEAT_ROWS`] rows. Where two lines round to one row, the bar
/// line wins. Returned top to bottom. The keyboard edge itself (`now`) is
/// kept just inside the highway, on its last row.
pub fn grid_rows(
    bars: &rockcraft_core::BarMap,
    beats_per_bar: u8,
    now_us: u64,
    lead_us: u64,
    height: u16,
) -> Vec<GridRow> {
    if height == 0 || lead_us == 0 {
        return Vec::new();
    }
    let per_bar = beats_per_bar.max(1) as u64;
    let horizon = now_us + lead_us;
    let rows_per_us = (height as f64 - 1.0).max(1.0) / lead_us as f64;
    let mut out: Vec<GridRow> = Vec::new();
    let mut bar = bars.bar_at(now_us);
    loop {
        let start = bars.bar_start(bar);
        if start > horizon {
            break;
        }
        let len = bars.bar_len(bar);
        let beat_rows = (len / per_bar) as f64 * rows_per_us;
        let beats = if beat_rows >= MIN_BEAT_ROWS as f64 {
            per_bar
        } else {
            1
        };
        for k in 0..beats {
            let t = start + len * k / per_bar;
            if t < now_us || t > horizon {
                continue;
            }
            let y8 = y8_of_time(t, now_us, lead_us, height).clamp(0, height as i64 * SUB - 1);
            match out.iter_mut().find(|g| g.y8 == y8) {
                Some(g) => g.bar |= k == 0,
                None => out.push(GridRow { y8, bar: k == 0 }),
            }
        }
        bar += 1;
    }
    out.sort_by_key(|g| g.y8);
    out
}

/// `now_us` snapped back to the latest 16th-note step of the bar grid — the
/// "rhythm steps" scroll mode, where the highway advances one 16th at a time.
/// A 16th is a quarter of a beat (`bar length / beats_per_bar / 4`), so it
/// follows the tempo map bar by bar. Before the first downbeat the steps run
/// back from it at the first bar's rate.
pub fn step_to_sixteenth(bars: &rockcraft_core::BarMap, beats_per_bar: u8, now_us: u64) -> u64 {
    let per_bar = beats_per_bar.max(1) as u64 * 4;
    let first = bars.bar_start(0);
    if now_us < first {
        let step = (bars.bar_len(0) / per_bar).max(1);
        let back = (first - now_us).div_ceil(step) * step;
        return first.saturating_sub(back);
    }
    let bar = bars.bar_at(now_us);
    let (start, len) = (bars.bar_start(bar), bars.bar_len(bar));
    // Exact step edges (len·k/per_bar), so steps never drift within a bar.
    let k = ((now_us - start) as u128 * per_bar as u128 / len as u128) as u64;
    start + len * k.min(per_bar - 1) / per_bar
}

/// The next note due in each lane: for every pitch, the index (into `spans`,
/// sorted by start) of its earliest note starting in `[now, now + lead]`. A
/// note already sounding has reached the keyboard, so it gets no guide; the
/// next one in its lane does. Mirrors the desktop's `nextPerLane`.
pub fn next_per_lane(spans: &[NoteSpan], now_us: u64, lead_us: u64) -> Vec<usize> {
    let horizon = now_us + lead_us;
    let mut taken = [false; 128];
    let mut out = Vec::new();
    for (i, s) in spans.iter().enumerate() {
        if s.start_us > horizon {
            break; // sorted by start: nothing later is visible
        }
        if s.start_us < now_us || taken[s.note as usize & 0x7f] {
            continue;
        }
        taken[s.note as usize & 0x7f] = true;
        out.push(i);
    }
    out
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

    // ── grid lines + lane guides ─────────────────────────────────────────────

    use rockcraft_core::BarMap;

    #[test]
    fn row_of_time_spans_keyboard_to_top() {
        assert_eq!(row_of_time(1_000, 1_000, 2_000_000, 21), 20); // now → keys
        assert_eq!(row_of_time(2_001_000, 1_000, 2_000_000, 21), 0); // lead → top
        assert_eq!(row_of_time(1_001_000, 1_000, 2_000_000, 21), 10); // halfway
    }

    #[test]
    fn slow_tempo_draws_bar_and_beat_lines() {
        // 2 s bars of 4 beats (120 BPM 4/4) on a 2 s, 40-row (320-eighth)
        // highway: a beat is 10 rows, so every beat gets a line.
        let map = BarMap::new(&[], 2_000_000);
        let rows = grid_rows(&map, 4, 0, 2_000_000, 40);
        assert_eq!(
            rows,
            vec![
                GridRow { y8: 0, bar: true },   // t = 2.0 s, the top edge
                GridRow { y8: 80, bar: false }, // 1.5 s
                GridRow {
                    y8: 160,
                    bar: false
                }, // 1.0 s
                GridRow {
                    y8: 240,
                    bar: false
                }, // 0.5 s
                GridRow { y8: 319, bar: true }, // 0.0 s, on the keys (kept inside)
            ]
        );
    }

    #[test]
    fn fast_tempo_keeps_only_bar_lines() {
        // 240 BPM 4/4: 1 s bars, 0.25 s beats = 2.5 rows on a 20-row highway,
        // under MIN_BEAT_ROWS — beat lines drop out, bar lines stay.
        let map = BarMap::new(&[], 1_000_000);
        let rows = grid_rows(&map, 4, 0, 2_000_000, 20);
        assert!(rows.iter().all(|g| g.bar), "{rows:?}");
        assert_eq!(rows.len(), 3); // 0 s, 1 s, 2 s
    }

    #[test]
    fn grid_follows_a_tempo_map() {
        // Bars at 0, 0.5 s, 1.5 s: uneven, so lines land unevenly.
        let starts = [0, 500_000, 1_500_000];
        let map = BarMap::new(&starts, 2_000_000);
        let ys: Vec<i64> = grid_rows(&map, 1, 0, 2_000_000, 40)
            .into_iter()
            .map(|g| g.y8)
            .collect();
        // 1.5 s → 80, 0.5 s → 240, 0 → 320 (kept inside: 319); past the map,
        // +1 s → 2.5 s (off).
        assert_eq!(ys, vec![80, 240, 319]);
    }

    #[test]
    fn y8_moves_an_eighth_per_eighth_row_of_time() {
        // 2 s over 40 rows = 320 eighths: one eighth is 6.25 ms.
        assert_eq!(y8_of_time(0, 0, 2_000_000, 40), 320);
        assert_eq!(y8_of_time(2_000_000, 0, 2_000_000, 40), 0);
        assert_eq!(y8_of_time(1_000_000, 6_250, 2_000_000, 40), 161);
    }

    #[test]
    fn a_note_on_a_downbeat_has_its_onset_on_the_bar_line() {
        let map = BarMap::new(&[], 2_000_000);
        // (Near now = 0 the note sits on the top edge, not yet on screen.)
        for now in [100_000, 400_000, 777_777, 1_500_001] {
            let bar = grid_rows(&map, 4, now, 2_000_000, 40)
                .into_iter()
                .find(|g| g.bar && g.y8 < 319)
                .expect("the next downbeat is on screen");
            let span = NoteSpan {
                note: 60,
                start_us: 2_000_000,
                end_us: 2_400_000,
            };
            let (_, onset) = span_extent8(&span, now, 2_000_000, 40).unwrap();
            assert_eq!(onset, bar.y8, "now = {now}");
        }
    }

    #[test]
    fn extent_trims_the_tail_and_clips_to_the_highway() {
        let span = NoteSpan {
            note: 60,
            start_us: 1_000_000,
            end_us: 1_500_000,
        };
        // 40 rows over 2 s: start → 160, end → 80; tail gap 4 → top 84.
        assert_eq!(span_extent8(&span, 0, 2_000_000, 40), Some((84, 160)));
        // Sounding past the keyboard: the bottom clips to the keyboard edge.
        assert_eq!(
            span_extent8(&span, 1_200_000, 2_000_000, 40),
            Some((276, 320)) // end 0.3 s ahead → 272, + tail gap 4
        );
        // Entirely below the keys: gone.
        assert_eq!(span_extent8(&span, 1_600_000, 2_000_000, 40), None);
    }

    #[test]
    fn cells_split_a_note_into_partial_edges_and_full_middles() {
        // [13, 35): top cell row 1 from eighth 5 (bottom 3), rows 2-3 full,
        // row 4 top 3 eighths.
        assert_eq!(
            cells_of_extent(13, 35),
            vec![
                (1, CellFill::Bottom(3)),
                (2, CellFill::Full),
                (3, CellFill::Full),
                (4, CellFill::Top(3)),
            ]
        );
        // Inside one cell: onset edge kept, grown to the cell top.
        assert_eq!(cells_of_extent(10, 13), vec![(1, CellFill::Top(5))]);
        // Cell-aligned: all full.
        assert_eq!(
            cells_of_extent(8, 24),
            vec![(1, CellFill::Full), (2, CellFill::Full)]
        );
    }

    #[test]
    fn line_glyph_tracks_the_height_within_the_cell() {
        assert_eq!(line_cell(16), (2, "▔"));
        assert_eq!(line_cell(20), (2, "─"));
        assert_eq!(line_cell(23), (2, "▁"));
        assert_eq!(lower_block(8), "█");
        assert_eq!(lower_block(1), "▁");
    }

    #[test]
    fn rhythm_steps_snap_back_to_the_sixteenth() {
        // 2 s bars in 4/4: a 16th is 125 ms, from origin 1 s.
        let map = BarMap::new(&[], 2_000_000).with_origin(1_000_000);
        assert_eq!(step_to_sixteenth(&map, 4, 1_000_000), 1_000_000);
        assert_eq!(step_to_sixteenth(&map, 4, 1_124_999), 1_000_000);
        assert_eq!(step_to_sixteenth(&map, 4, 1_125_000), 1_125_000);
        assert_eq!(step_to_sixteenth(&map, 4, 3_100_000), 3_000_000);
        // Lead-in, before the first downbeat: steps count back from it.
        assert_eq!(step_to_sixteenth(&map, 4, 900_000), 875_000);
        assert_eq!(step_to_sixteenth(&map, 4, 0), 0);
    }

    #[test]
    fn rhythm_steps_follow_a_tempo_map() {
        // Bar 0 is 1.6 s (100 ms 16ths), bar 1 is 3.2 s (200 ms 16ths).
        let starts = [0, 1_600_000, 4_800_000];
        let map = BarMap::new(&starts, 2_000_000);
        assert_eq!(step_to_sixteenth(&map, 4, 150_000), 100_000);
        assert_eq!(step_to_sixteenth(&map, 4, 1_950_000), 1_800_000);
    }

    #[test]
    fn next_per_lane_takes_the_earliest_upcoming_note_per_pitch() {
        let spans = [
            NoteSpan {
                note: 60,
                start_us: 0,
                end_us: 400,
            }, // sounding
            NoteSpan {
                note: 60,
                start_us: 500,
                end_us: 900,
            }, // next C
            NoteSpan {
                note: 62,
                start_us: 600,
                end_us: 700,
            }, // next D
            NoteSpan {
                note: 60,
                start_us: 1_000,
                end_us: 1_100,
            }, // later C
            NoteSpan {
                note: 64,
                start_us: 5_000,
                end_us: 5_100,
            }, // off-screen
        ];
        assert_eq!(next_per_lane(&spans, 100, 2_000), vec![1, 2]);
    }
}
