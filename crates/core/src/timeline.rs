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
}

/// A pure, editable timeline of notes with stable ids.
///
/// Notes are stored keyed by their [`NoteId`]; iteration order (see
/// [`Timeline::notes`]) is stable id order, which equals insertion order because
/// ids are handed out monotonically.
#[derive(Debug, Clone, Default)]
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

    fn note(pitch: u8, start_us: u64, dur_us: u64, vel: u8) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(vel).unwrap(),
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
}
