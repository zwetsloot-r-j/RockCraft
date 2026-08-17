//! Editable note model — the composer's mutable view of a song.
//!
//! A [`Timeline`] is a pure, in-memory collection of [`Note`]s keyed by a stable
//! [`NoteId`]. The composer mutates it note-by-note (insert / remove / move /
//! resize / transpose); ids survive edits to *other* notes so a cursor or
//! selection can hold a handle across mutations.
//!
//! It converts losslessly to/from `Vec<NoteEvent>` ([`Timeline::to_events`] /
//! [`Timeline::from_events`]) so the existing MIDI writer, highway, and scoring
//! keep working unchanged. The on/off pairing mirrors `build_spans` in the TUI
//! highway: each note-on pairs with the next note-off of the same pitch, and a
//! dangling on is closed at the last timestamp seen.
//!
//! Like the rest of `core`, this module is pure: no I/O, no device, no terminal.

use std::collections::BTreeMap;

use crate::events::{MidiNote, NoteEvent, NoteEventKind, Velocity};
use crate::hand::{hand_of, Hand, HandOverride};

/// Opaque, stable handle to a note inside a [`Timeline`].
///
/// Survives edits to other notes; invalidated only when its own note is
/// removed. Ids are assigned monotonically and never reused within a timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoteId(u32);

impl NoteId {
    /// The raw id value. Exposed for stable ordering and display; construction
    /// stays internal so ids can only originate from [`Timeline::insert`].
    pub const fn value(self) -> u32 {
        self.0
    }
}

/// One editable note: a `pitch` sounding for `dur_us` from `start_us`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub pitch: MidiNote,
    pub start_us: u64,
    pub dur_us: u64,
    pub velocity: Velocity,
    /// Per-note hand exception (M14-E). `None` — the default — means the note
    /// follows the piece's split line; `Some(h)` pins it to that hand no matter
    /// where the split sits. Never written to `song.mid`: it round-trips
    /// through `meta.json` (see [`crate::HandOverride`]).
    pub hand: Option<Hand>,
}

impl Note {
    /// The hand that actually plays this note: the override when set, else the
    /// split rule. Every hand-aware consumer (practice grey-out, one-hand
    /// scoring, colouring) reads this rather than re-deriving from pitch.
    pub fn effective_hand(&self, split: u8) -> Hand {
        self.hand.unwrap_or_else(|| hand_of(self.pitch, split))
    }
}

/// A pure, editable timeline of notes with stable ids.
///
/// Notes are stored keyed by their [`NoteId`]; iteration order (see
/// [`Timeline::notes`]) is stable id order, which equals insertion order because
/// ids are handed out monotonically.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timeline {
    notes: BTreeMap<u32, Note>,
    next_id: u32,
}

impl Timeline {
    /// An empty timeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a note, returning its freshly allocated stable id.
    pub fn insert(&mut self, note: Note) -> NoteId {
        let id = self.next_id;
        self.next_id += 1;
        self.notes.insert(id, note);
        NoteId(id)
    }

    /// Remove a note, returning it if `id` was known.
    pub fn remove(&mut self, id: NoteId) -> Option<Note> {
        self.notes.remove(&id.0)
    }

    /// Borrow a note by id.
    pub fn get(&self, id: NoteId) -> Option<&Note> {
        self.notes.get(&id.0)
    }

    /// Reposition a note's start. Returns false if `id` is unknown.
    pub fn set_start(&mut self, id: NoteId, start_us: u64) -> bool {
        match self.notes.get_mut(&id.0) {
            Some(note) => {
                note.start_us = start_us;
                true
            }
            None => false,
        }
    }

    /// Transpose by `semitones`. No-op (returns false) if the result would leave
    /// the MIDI range 0..=127, or if `id` is unknown; otherwise applies the
    /// shift and returns true. A rejected transpose leaves the note unchanged.
    pub fn transpose(&mut self, id: NoteId, semitones: i8) -> bool {
        let Some(note) = self.notes.get_mut(&id.0) else {
            return false;
        };
        let target = note.pitch.value() as i16 + semitones as i16;
        let Ok(raw) = u8::try_from(target) else {
            return false;
        };
        match MidiNote::new(raw) {
            Some(pitch) => {
                note.pitch = pitch;
                true
            }
            None => false,
        }
    }

    /// Set a note's duration. `dur_us` of 0 is clamped to 1 µs (a note always
    /// sounds for a positive span). Returns false if `id` is unknown.
    pub fn resize(&mut self, id: NoteId, dur_us: u64) -> bool {
        match self.notes.get_mut(&id.0) {
            Some(note) => {
                note.dur_us = dur_us.max(1);
                true
            }
            None => false,
        }
    }

    /// The note whose span `[start_us, start_us + dur_us)` covers `us` at
    /// `pitch`, if any — what an editing cursor sits on. On overlap the
    /// highest-id (most recently inserted) note wins.
    pub fn find_at(&self, pitch: u8, us: u64) -> Option<NoteId> {
        // BTreeMap iterates ascending id, so the last match is the highest id.
        self.notes
            .iter()
            .rfind(|(_, note)| {
                note.pitch.value() == pitch
                    && note.start_us <= us
                    && us < note.start_us + note.dur_us
            })
            .map(|(&id, _)| NoteId(id))
    }

    /// The highest-id note at `pitch` whose **onset** falls in `[lo_us, hi_us)`.
    ///
    /// [`find_at`](Self::find_at) is a point query: it only sees a note that
    /// *covers* an exact microsecond. The editor cursor, though, is a grid
    /// **cell** one `step_us` wide, and imported charts place onsets at their
    /// true (fractional) microsecond times — so a short note can begin a few µs
    /// past the integer-floored grid line and be missed by a point query at that
    /// line (the cursor visually sits on the note, but just left of its onset).
    /// Matching any note that *starts within the cell* recovers it.
    pub fn find_starting_in(&self, pitch: u8, lo_us: u64, hi_us: u64) -> Option<NoteId> {
        self.notes
            .iter()
            .rfind(|(_, note)| {
                note.pitch.value() == pitch && lo_us <= note.start_us && note.start_us < hi_us
            })
            .map(|(&id, _)| NoteId(id))
    }

    /// Ripple-shift in time: add `delta_us` to the `start_us` of every note that
    /// starts **at or after** `threshold_us`. A negative delta saturates at 0.
    /// Notes before the threshold are untouched (their duration is unchanged, so
    /// a note spanning the threshold keeps its onset and simply overlaps the
    /// shifted region). Backs the insert/cut-bar edits.
    pub fn shift_from(&mut self, threshold_us: u64, delta_us: i64) {
        if delta_us == 0 {
            return;
        }
        for note in self.notes.values_mut() {
            if note.start_us >= threshold_us {
                note.start_us = note.start_us.saturating_add_signed(delta_us);
            }
        }
    }

    /// Remove every note whose **onset** falls in `[from_us, to_us)`. Returns how
    /// many were removed. Notes starting before `from_us` (even if they sustain
    /// into the range) are kept — removal is keyed on the onset, like the editor
    /// cursor. Backs the cut-bar edit.
    pub fn remove_in(&mut self, from_us: u64, to_us: u64) -> usize {
        let ids: Vec<u32> = self
            .notes
            .iter()
            .filter(|(_, n)| from_us <= n.start_us && n.start_us < to_us)
            .map(|(&id, _)| id)
            .collect();
        for id in &ids {
            self.notes.remove(id);
        }
        ids.len()
    }

    /// Set (or clear, with `None`) a note's hand override. Returns false if
    /// `id` is unknown.
    pub fn set_hand(&mut self, id: NoteId, hand: Option<Hand>) -> bool {
        match self.notes.get_mut(&id.0) {
            Some(note) => {
                note.hand = hand;
                true
            }
            None => false,
        }
    }

    /// Every overridden note as a persistable [`HandOverride`], sorted by
    /// `(start_us, pitch)` so a saved `meta.json` is stable across re-saves.
    /// Notes following the split line are omitted.
    pub fn hand_overrides(&self) -> Vec<HandOverride> {
        let mut out: Vec<HandOverride> = self
            .notes
            .values()
            .filter_map(|note| {
                note.hand.map(|hand| HandOverride {
                    pitch: note.pitch.value(),
                    start_us: note.start_us,
                    hand,
                })
            })
            .collect();
        out.sort_by_key(|o| (o.start_us, o.pitch));
        out
    }

    /// Re-apply persisted overrides after a reload: for each entry, pin the note
    /// sitting at exactly `(pitch, start_us)`.
    ///
    /// Entries with no matching note (the note was deleted since the save) are
    /// silently ignored — a stale override must never resurrect a note or fail
    /// a load.
    pub fn apply_hand_overrides(&mut self, overrides: &[HandOverride]) {
        for o in overrides {
            if let Some(id) = self.find_starting_at(o.pitch, o.start_us) {
                self.set_hand(id, Some(o.hand));
            }
        }
    }

    /// The note *starting* exactly at `(pitch, start_us)`, if any — the key
    /// persisted overrides are matched on. On overlap the highest-id (most
    /// recently inserted) note wins, mirroring [`Timeline::find_at`].
    fn find_starting_at(&self, pitch: u8, start_us: u64) -> Option<NoteId> {
        self.notes
            .iter()
            .rfind(|(_, note)| note.pitch.value() == pitch && note.start_us == start_us)
            .map(|(&id, _)| NoteId(id))
    }

    /// Notes in stable id order (insertion order).
    pub fn notes(&self) -> impl Iterator<Item = (NoteId, &Note)> + '_ {
        self.notes.iter().map(|(&id, note)| (NoteId(id), note))
    }

    /// Number of notes.
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// Whether the timeline holds no notes.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Emit on/off events sorted by timestamp, with note-off ordered before
    /// note-on at an equal timestamp so a note that ends exactly when another
    /// begins re-pairs cleanly. Feeds `events_to_smf_bytes` and `build_spans`.
    pub fn to_events(&self) -> Vec<NoteEvent> {
        let mut events = Vec::with_capacity(self.notes.len() * 2);
        for note in self.notes.values() {
            events.push(NoteEvent::on(note.pitch, note.velocity, note.start_us));
            events.push(NoteEvent::off(note.pitch, note.start_us + note.dur_us));
        }
        // Sort by timestamp; within a timestamp, Off (0) before On (1).
        events.sort_by_key(|ev| {
            let kind_rank = match ev.kind {
                NoteEventKind::Off => 0,
                NoteEventKind::On { .. } => 1,
            };
            (ev.timestamp_us, kind_rank, ev.note.value())
        });
        events
    }

    /// Ids of notes whose start falls within the rectangle
    /// `[pitch_lo..=pitch_hi] × [us_lo..us_hi)`.
    pub fn notes_in_region(
        &self,
        pitch_lo: u8,
        pitch_hi: u8,
        us_lo: u64,
        us_hi: u64,
    ) -> Vec<NoteId> {
        self.notes
            .iter()
            .filter(|(_, note)| {
                let p = note.pitch.value();
                p >= pitch_lo && p <= pitch_hi && note.start_us >= us_lo && note.start_us < us_hi
            })
            .map(|(&id, _)| NoteId(id))
            .collect()
    }

    /// Insert clones of `notes` each shifted by `d_pitch` semitones and `d_us`
    /// microseconds. Notes whose shifted pitch would leave `0..=127` are
    /// dropped. Returns the new ids in insertion order.
    pub fn insert_shifted(&mut self, notes: &[Note], d_pitch: i8, d_us: u64) -> Vec<NoteId> {
        let mut ids = Vec::new();
        for note in notes {
            let shifted = note.pitch.value() as i16 + d_pitch as i16;
            let Ok(raw) = u8::try_from(shifted) else {
                continue;
            };
            let Some(pitch) = MidiNote::new(raw) else {
                continue;
            };
            let new_note = Note {
                pitch,
                start_us: note.start_us.saturating_add(d_us),
                ..*note
            };
            ids.push(self.insert(new_note));
        }
        ids
    }

    /// Rebuild a timeline from an event stream, pairing each note-on with the
    /// next note-off (or note-on with velocity 0) of the same pitch — the same
    /// rule as `build_spans`. A re-press while a pitch is still open closes the
    /// previous span first. Dangling note-ons are closed at the last timestamp
    /// seen, with duration clamped to at least 1 µs.
    ///
    /// Resulting notes are inserted in `(start_us, pitch)` order so ids are
    /// deterministic regardless of input event ordering.
    pub fn from_events(events: &[NoteEvent]) -> Self {
        // Process in time order, Off before On at equal timestamps, matching
        // `to_events` so a round-trip is exact.
        let mut ordered: Vec<&NoteEvent> = events.iter().collect();
        ordered.sort_by_key(|ev| {
            let kind_rank = match ev.kind {
                NoteEventKind::Off => 0,
                NoteEventKind::On { .. } => 1,
            };
            (ev.timestamp_us, kind_rank, ev.note.value())
        });

        // pitch -> (start_us, velocity) of an open note.
        let mut open: BTreeMap<u8, (u64, Velocity)> = BTreeMap::new();
        let mut closed: Vec<Note> = Vec::new();
        let mut last_us = 0u64;

        let close = |closed: &mut Vec<Note>, pitch: u8, start: u64, vel: Velocity, end: u64| {
            // Unwrap is safe: `pitch` came from an existing `MidiNote`.
            let pitch = MidiNote::new(pitch).expect("pitch originated from a MidiNote");
            closed.push(Note {
                pitch,
                start_us: start,
                dur_us: (end.saturating_sub(start)).max(1),
                velocity: vel,
                // `song.mid` carries no per-note hand; overrides are re-applied
                // from `meta.json` via `apply_hand_overrides`.
                hand: None,
            });
        };

        for ev in ordered {
            last_us = last_us.max(ev.timestamp_us);
            let pitch = ev.note.value();
            match ev.kind {
                NoteEventKind::On { velocity } if velocity.value() > 0 => {
                    // A re-press while open closes the previous span first.
                    if let Some((start, vel)) = open.remove(&pitch) {
                        close(&mut closed, pitch, start, vel, ev.timestamp_us);
                    }
                    open.insert(pitch, (ev.timestamp_us, velocity));
                }
                _ => {
                    if let Some((start, vel)) = open.remove(&pitch) {
                        close(&mut closed, pitch, start, vel, ev.timestamp_us);
                    }
                }
            }
        }

        // Close any still-open notes at the end of the stream.
        for (pitch, (start, vel)) in open {
            close(&mut closed, pitch, start, vel, last_us.max(start + 1));
        }

        closed.sort_by_key(|n| (n.start_us, n.pitch.value()));

        let mut timeline = Timeline::new();
        for note in closed {
            timeline.insert(note);
        }
        timeline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hand::DEFAULT_SPLIT;

    fn note(pitch: u8, start_us: u64, dur_us: u64, vel: u8) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(vel).unwrap(),
            hand: None,
        }
    }

    #[test]
    fn insert_get_remove() {
        let mut tl = Timeline::new();
        let n = note(60, 0, 1_000, 100);
        let id = tl.insert(n);

        assert_eq!(tl.get(id), Some(&n));
        assert_eq!(tl.len(), 1);
        assert!(!tl.is_empty());

        assert_eq!(tl.remove(id), Some(n));
        assert_eq!(tl.get(id), None);
        assert!(tl.is_empty());
        // Removing again is a clean None.
        assert_eq!(tl.remove(id), None);
    }

    #[test]
    fn set_start_and_resize_target_only_one_note() {
        let mut tl = Timeline::new();
        let a = tl.insert(note(60, 0, 1_000, 100));
        let b = tl.insert(note(62, 2_000, 1_000, 100));

        assert!(tl.set_start(a, 500));
        assert!(tl.resize(a, 750));

        assert_eq!(tl.get(a).unwrap().start_us, 500);
        assert_eq!(tl.get(a).unwrap().dur_us, 750);
        // b untouched.
        assert_eq!(tl.get(b).unwrap().start_us, 2_000);
        assert_eq!(tl.get(b).unwrap().dur_us, 1_000);
    }

    #[test]
    fn resize_clamps_zero_to_one() {
        let mut tl = Timeline::new();
        let id = tl.insert(note(60, 0, 1_000, 100));
        assert!(tl.resize(id, 0));
        assert_eq!(tl.get(id).unwrap().dur_us, 1);
    }

    #[test]
    fn mutators_return_false_for_unknown_id() {
        let mut tl = Timeline::new();
        let id = tl.insert(note(60, 0, 1_000, 100));
        tl.remove(id);
        assert!(!tl.set_start(id, 5));
        assert!(!tl.resize(id, 5));
        assert!(!tl.transpose(id, 1));
    }

    #[test]
    fn ids_stable_across_other_inserts_and_removes() {
        let mut tl = Timeline::new();
        let a = tl.insert(note(60, 0, 1_000, 100));
        let b = tl.insert(note(62, 1_000, 1_000, 100));
        tl.remove(b);
        let _c = tl.insert(note(64, 2_000, 1_000, 100));

        // `a` still resolves to the same note after b's removal and c's insert.
        assert_eq!(tl.get(a).unwrap().pitch.value(), 60);
        assert!(tl.set_start(a, 42));
        assert_eq!(tl.get(a).unwrap().start_us, 42);
    }

    #[test]
    fn transpose_succeeds_within_range() {
        let mut tl = Timeline::new();
        // A0 = 21; -1 → 20 is valid.
        let id = tl.insert(note(21, 0, 1_000, 100));
        assert!(tl.transpose(id, -1));
        assert_eq!(tl.get(id).unwrap().pitch.value(), 20);
    }

    #[test]
    fn transpose_rejects_below_zero_and_leaves_note_unchanged() {
        let mut tl = Timeline::new();
        // C-1 = 0; -1 leaves the range.
        let id = tl.insert(note(0, 0, 1_000, 100));
        assert!(!tl.transpose(id, -1));
        assert_eq!(tl.get(id).unwrap().pitch.value(), 0);
    }

    #[test]
    fn transpose_rejects_above_top_and_leaves_note_unchanged() {
        let mut tl = Timeline::new();
        // 120 + 12 = 132 leaves the range near the top.
        let id = tl.insert(note(120, 0, 1_000, 100));
        assert!(!tl.transpose(id, 12));
        assert_eq!(tl.get(id).unwrap().pitch.value(), 120);
        // The very top boundary: 127 + 1 fails, 126 + 1 succeeds.
        let top = tl.insert(note(127, 0, 1_000, 100));
        assert!(!tl.transpose(top, 1));
        let near = tl.insert(note(126, 0, 1_000, 100));
        assert!(tl.transpose(near, 1));
        assert_eq!(tl.get(near).unwrap().pitch.value(), 127);
    }

    #[test]
    fn find_at_covers_span_and_misses_empty_slot() {
        let mut tl = Timeline::new();
        let id = tl.insert(note(60, 1_000, 1_000, 100)); // covers [1000, 2000)

        assert_eq!(tl.find_at(60, 1_000), Some(id)); // inclusive start
        assert_eq!(tl.find_at(60, 1_999), Some(id));
        assert_eq!(tl.find_at(60, 2_000), None); // exclusive end
        assert_eq!(tl.find_at(60, 500), None); // before start
        assert_eq!(tl.find_at(61, 1_500), None); // wrong pitch
    }

    #[test]
    fn shift_from_and_remove_in_ripple_time() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 0, 100, 100));
        let mid = tl.insert(note(62, 1_000, 100, 100));
        let tail = tl.insert(note(64, 2_000, 100, 100));
        // remove the middle onset, then slide the tail left to close the gap.
        assert_eq!(tl.remove_in(1_000, 2_000), 1);
        assert!(tl.get(mid).is_none());
        tl.shift_from(2_000, -1_000);
        assert_eq!(tl.get(tail).unwrap().start_us, 1_000);
        // a positive shift opens a gap; the note before the threshold stays put.
        tl.shift_from(1_000, 500);
        assert_eq!(tl.get(tail).unwrap().start_us, 1_500);
        assert_eq!(tl.notes().next().unwrap().1.start_us, 0);
    }

    #[test]
    fn find_starting_in_matches_onset_within_the_cell() {
        let mut tl = Timeline::new();
        let id = tl.insert(note(60, 1_050, 100, 100)); // onset 1050, ends 1150
                                                       // Onset falls in the half-open window.
        assert_eq!(tl.find_starting_in(60, 1_000, 2_000), Some(id));
        // The exact-point query at the cell's left edge misses the drifted onset.
        assert_eq!(tl.find_at(60, 1_000), None);
        // Onset before / at-or-after the window, and wrong pitch, all miss.
        assert_eq!(tl.find_starting_in(60, 1_100, 2_000), None);
        assert_eq!(tl.find_starting_in(60, 500, 1_050), None); // hi is exclusive
        assert_eq!(tl.find_starting_in(61, 1_000, 2_000), None);
    }

    #[test]
    fn find_at_highest_id_wins_on_overlap() {
        let mut tl = Timeline::new();
        let _first = tl.insert(note(60, 0, 2_000, 100));
        let second = tl.insert(note(60, 0, 2_000, 100));
        assert_eq!(tl.find_at(60, 1_000), Some(second));
    }

    #[test]
    fn notes_iterate_in_insertion_order() {
        let mut tl = Timeline::new();
        let a = tl.insert(note(64, 0, 1, 100));
        let b = tl.insert(note(60, 0, 1, 100));
        let c = tl.insert(note(62, 0, 1, 100));
        let ids: Vec<NoteId> = tl.notes().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn to_events_orders_off_before_on_at_equal_timestamp() {
        let mut tl = Timeline::new();
        // Note A ends at 1000; note B starts at 1000.
        tl.insert(note(60, 0, 1_000, 100));
        tl.insert(note(62, 1_000, 1_000, 100));

        let events = tl.to_events();
        // Find the two events at t=1000.
        let at_1000: Vec<&NoteEvent> = events.iter().filter(|e| e.timestamp_us == 1_000).collect();
        assert_eq!(at_1000.len(), 2);
        assert!(matches!(at_1000[0].kind, NoteEventKind::Off));
        assert!(matches!(at_1000[1].kind, NoteEventKind::On { .. }));
    }

    #[test]
    fn round_trip_overlapping_chord() {
        let mut tl = Timeline::new();
        // A C-major triad plus an overlapping melody note — overlapping spans
        // across different pitches.
        tl.insert(note(60, 0, 2_000, 90));
        tl.insert(note(64, 0, 2_000, 80));
        tl.insert(note(67, 0, 2_000, 70));
        tl.insert(note(72, 1_000, 1_500, 110));

        let rebuilt = Timeline::from_events(&tl.to_events());

        // Compare as (pitch, start, dur, velocity) sets — ids may differ but the
        // musical content must be identical.
        let mut a: Vec<(u8, u64, u64, u8)> = tl
            .notes()
            .map(|(_, n)| (n.pitch.value(), n.start_us, n.dur_us, n.velocity.value()))
            .collect();
        let mut b: Vec<(u8, u64, u64, u8)> = rebuilt
            .notes()
            .map(|(_, n)| (n.pitch.value(), n.start_us, n.dur_us, n.velocity.value()))
            .collect();
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }

    #[test]
    fn from_events_closes_dangling_on() {
        let on = NoteEvent::on(MidiNote::new(60).unwrap(), Velocity::new(100).unwrap(), 500);
        let tl = Timeline::from_events(&[on]);
        assert_eq!(tl.len(), 1);
        let (_, note) = tl.notes().next().unwrap();
        assert_eq!(note.start_us, 500);
        assert!(note.dur_us >= 1);
    }

    #[test]
    fn notes_in_region_covers_exactly_the_rectangle() {
        let mut tl = Timeline::new();
        let v = Velocity::new(80).unwrap();
        let mk = |pitch: u8, start_us: u64| Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us: 100,
            velocity: v,
            hand: None,
        };
        // Inside region [60..=64] × [1000..5000)
        let a = tl.insert(mk(60, 1_000)); // pitch_lo, us_lo — inclusive
        let b = tl.insert(mk(64, 4_999)); // pitch_hi, us_hi-1 — inclusive
                                          // Outside: pitch below lo
        tl.insert(mk(59, 2_000));
        // Outside: pitch above hi
        tl.insert(mk(65, 2_000));
        // Outside: start before us_lo
        tl.insert(mk(62, 999));
        // Outside: start exactly at us_hi (exclusive)
        tl.insert(mk(62, 5_000));

        let mut ids = tl.notes_in_region(60, 64, 1_000, 5_000);
        ids.sort();
        assert_eq!(ids, [a, b]);
    }

    #[test]
    fn insert_shifted_offsets_and_drops_out_of_range() {
        let mut tl = Timeline::new();
        let v = Velocity::new(80).unwrap();
        let notes = [
            Note {
                pitch: MidiNote::new(0).unwrap(),
                start_us: 0,
                dur_us: 100,
                velocity: v,
                hand: None,
            },
            Note {
                pitch: MidiNote::new(60).unwrap(),
                start_us: 100,
                dur_us: 100,
                velocity: v,
                hand: None,
            },
            Note {
                pitch: MidiNote::new(127).unwrap(),
                start_us: 200,
                dur_us: 100,
                velocity: v,
                hand: None,
            },
        ];
        // Shift +2 semitones, +1000 µs: 0→2, 60→62, 127→129 (out of range, dropped).
        let ids = tl.insert_shifted(&notes, 2, 1_000);
        assert_eq!(ids.len(), 2, "pitch 127+2=129 must be dropped");
        let mut pitches: Vec<u8> = ids
            .iter()
            .filter_map(|&id| tl.get(id))
            .map(|n| n.pitch.value())
            .collect();
        pitches.sort_unstable();
        assert_eq!(pitches, [2, 62]);
        for &id in &ids {
            assert!(
                tl.get(id).unwrap().start_us >= 1_000,
                "start_us shifted by d_us"
            );
        }
        // Shift -3: pitch 2→-1, out of range.
        let low = Note {
            pitch: MidiNote::new(2).unwrap(),
            start_us: 0,
            dur_us: 100,
            velocity: v,
            hand: None,
        };
        let ids2 = tl.insert_shifted(&[low], -3, 0);
        assert_eq!(ids2.len(), 0, "pitch 2-3=-1 must be dropped");
    }

    #[test]
    fn from_events_repress_closes_previous_span() {
        let pitch = MidiNote::new(60).unwrap();
        let vel = Velocity::new(100).unwrap();
        // Two presses of the same pitch with no intervening off: re-press at
        // 1000 closes the first span, second closes at the last timestamp.
        let events = [
            NoteEvent::on(pitch, vel, 0),
            NoteEvent::on(pitch, vel, 1_000),
        ];
        let tl = Timeline::from_events(&events);
        assert_eq!(tl.len(), 2);
        let starts: Vec<u64> = tl.notes().map(|(_, n)| n.start_us).collect();
        assert_eq!(starts, vec![0, 1_000]);
    }

    // ── per-note hand (M14-E) ────────────────────────────────────────────────

    #[test]
    fn new_notes_follow_the_split_line() {
        let mut tl = Timeline::new();
        let id = tl.insert(note(55, 0, 1_000, 100));
        let n = tl.get(id).unwrap();
        assert_eq!(n.hand, None, "a fresh note carries no override");
        assert_eq!(n.effective_hand(DEFAULT_SPLIT), Hand::Left);
        // Moving the split line alone flips it — that is the default rule.
        assert_eq!(n.effective_hand(50), Hand::Right);
    }

    #[test]
    fn effective_hand_prefers_the_override() {
        let mut tl = Timeline::new();
        // A low note the author wants on the right hand (a crossover).
        let id = tl.insert(note(48, 0, 1_000, 100));
        assert!(tl.set_hand(id, Some(Hand::Right)));
        assert_eq!(
            tl.get(id).unwrap().effective_hand(DEFAULT_SPLIT),
            Hand::Right
        );
        // Clearing it falls back to the split.
        assert!(tl.set_hand(id, None));
        assert_eq!(
            tl.get(id).unwrap().effective_hand(DEFAULT_SPLIT),
            Hand::Left
        );
    }

    #[test]
    fn set_hand_reports_whether_the_id_existed() {
        let mut tl = Timeline::new();
        let id = tl.insert(note(60, 0, 1_000, 100));
        assert!(tl.set_hand(id, Some(Hand::Left)));
        tl.remove(id);
        assert!(!tl.set_hand(id, Some(Hand::Left)));
    }

    #[test]
    fn hand_overrides_lists_only_overridden_notes_sorted() {
        let mut tl = Timeline::new();
        let a = tl.insert(note(72, 2_000, 500, 100));
        let _plain = tl.insert(note(64, 1_000, 500, 100));
        let b = tl.insert(note(48, 1_000, 500, 100));
        let c = tl.insert(note(50, 1_000, 500, 100));
        tl.set_hand(a, Some(Hand::Left));
        tl.set_hand(b, Some(Hand::Right));
        tl.set_hand(c, Some(Hand::Right));

        let overrides = tl.hand_overrides();
        assert_eq!(
            overrides,
            vec![
                HandOverride {
                    pitch: 48,
                    start_us: 1_000,
                    hand: Hand::Right
                },
                HandOverride {
                    pitch: 50,
                    start_us: 1_000,
                    hand: Hand::Right
                },
                HandOverride {
                    pitch: 72,
                    start_us: 2_000,
                    hand: Hand::Left
                },
            ],
            "sorted by (start_us, pitch), un-overridden notes omitted"
        );
    }

    #[test]
    fn apply_hand_overrides_matches_by_pitch_and_start() {
        let mut tl = Timeline::new();
        let target = tl.insert(note(48, 1_000, 500, 100));
        // Same pitch, different start — must NOT be matched.
        let other_time = tl.insert(note(48, 9_000, 500, 100));
        // Same start, different pitch — must NOT be matched.
        let other_pitch = tl.insert(note(49, 1_000, 500, 100));

        tl.apply_hand_overrides(&[
            HandOverride {
                pitch: 48,
                start_us: 1_000,
                hand: Hand::Right,
            },
            // No note here at all: silently ignored (deleted since the save).
            HandOverride {
                pitch: 100,
                start_us: 7_777,
                hand: Hand::Left,
            },
        ]);

        assert_eq!(tl.get(target).unwrap().hand, Some(Hand::Right));
        assert_eq!(tl.get(other_time).unwrap().hand, None);
        assert_eq!(tl.get(other_pitch).unwrap().hand, None);
        assert_eq!(tl.len(), 3, "a stale override never creates a note");
    }

    #[test]
    fn overrides_round_trip_through_events_plus_meta() {
        // The MIDI file drops hand; `meta.json`'s overrides restore it.
        let mut tl = Timeline::new();
        let id = tl.insert(note(48, 1_000, 500, 100));
        tl.insert(note(72, 2_000, 500, 100));
        tl.set_hand(id, Some(Hand::Right));
        let saved = tl.hand_overrides();

        let mut reloaded = Timeline::from_events(&tl.to_events());
        assert!(
            reloaded.notes().all(|(_, n)| n.hand.is_none()),
            "song.mid carries no hand"
        );
        reloaded.apply_hand_overrides(&saved);
        assert_eq!(reloaded.hand_overrides(), saved);
    }

    #[test]
    fn insert_shifted_carries_the_override_to_the_copy() {
        // Copy/paste keeps a hand exception on the pasted note.
        let mut tl = Timeline::new();
        let src = Note {
            hand: Some(Hand::Right),
            ..note(48, 0, 500, 100)
        };
        let ids = tl.insert_shifted(&[src], 0, 4_000);
        assert_eq!(ids.len(), 1);
        assert_eq!(tl.get(ids[0]).unwrap().hand, Some(Hand::Right));
    }
}
