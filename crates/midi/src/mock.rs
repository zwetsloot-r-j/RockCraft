//! Piano-free [`NoteSource`](crate::NoteSource) implementations for development
//! and testing.
//!
//! - [`MockKeyboard`]: typed computer-keyboard keys become [`NoteEvent`]s, so
//!   the TUI runs without hardware attached.
//! - [`ScriptedSource`]: replays a fixed `NoteEvent` timeline against a fake
//!   clock, for deterministic headless tests.

use std::time::Instant;

use rockcraft_core::{MidiNote, NoteEvent, Velocity};

use crate::NoteSource;

// ── MockKeyboard ─────────────────────────────────────────────────────────────

/// How long a mock note sustains before its note-off, in milliseconds.
const SUSTAIN_MS: u64 = 120;

/// Velocity stamped on mock note-ons (a firm-but-not-max strike).
const MOCK_VELOCITY: u8 = 80;

/// Keyboard → MIDI note map: the **number row** plays a C-major (white-key)
/// scale spanning just over an octave, from middle C (60):
///
/// ```text
///   1   2   3   4   5   6   7   8   9   0
///   C4  D4  E4  F4  G4  A4  B4  C5  D5  E5
///   60  62  64  65  67  69  71  72  74  76
/// ```
///
/// **Why the number row, and not the home row?** In the composer the letter
/// keys are *editor commands* (`a` adds a note, `d` deletes, `s` saves, `hjkl`
/// navigate…), so a letter-based note map collides with editing — the confusion
/// reported in #124. Digits are unbound in the editor's normal mode, so notes on
/// the number row stay clear of the command keys. Shift is deliberately avoided:
/// crossterm delivers `Shift+1` as `'!'`, not `'1'`, so a shifted scheme would be
/// indistinguishable from the symbols; the plain digit row sidesteps that.
///
/// White keys only — this is a no-piano development aid, not a substitute for the
/// real keyboard; play a piano for accidentals and full range.
///
/// Keys not in this table are unmapped (`press` returns `None`). Pinned in tests.
const KEY_MAP: &[(char, u8)] = &[
    ('1', 60), // C4
    ('2', 62), // D4
    ('3', 64), // E4
    ('4', 65), // F4
    ('5', 67), // G4
    ('6', 69), // A4
    ('7', 71), // B4
    ('8', 72), // C5
    ('9', 74), // D5
    ('0', 76), // E5
];

/// The mock keyboard's key → MIDI-note table (the number-row C-major scale).
///
/// Exposed so a frontend can render an accurate, drift-proof legend from the
/// single source of truth instead of duplicating the mapping in view code.
pub fn key_map() -> &'static [(char, u8)] {
    KEY_MAP
}

/// Look up the MIDI note a key maps to, if any.
fn key_to_note(key: char) -> Option<MidiNote> {
    KEY_MAP
        .iter()
        .find(|(k, _)| *k == key)
        .and_then(|(_, n)| MidiNote::new(*n))
}

/// A piano-free note source driven by computer-keyboard hotkeys.
pub struct MockKeyboard {
    /// Clock origin; all timestamps are microseconds since `new()` (monotonic).
    origin: Instant,
    /// Pending events sorted by timestamp; drained as the clock reaches them.
    pending: Vec<NoteEvent>,
    port_name: String,
}

impl MockKeyboard {
    /// A fresh mock keyboard whose clock starts now.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            pending: Vec::new(),
            port_name: "MockKeyboard (no piano)".to_string(),
        }
    }

    /// Microseconds elapsed since `new()`.
    fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64
    }

    /// Map `key` to a note and enqueue a note-on now plus a note-off
    /// `SUSTAIN_MS` later. Returns the note struck, or `None` if unmapped.
    pub fn press(&mut self, key: char) -> Option<MidiNote> {
        let now = self.now_us();
        self.press_at(key, now)
    }

    /// `press` against an explicit clock value — the seam unit tests drive.
    fn press_at(&mut self, key: char, now_us: u64) -> Option<MidiNote> {
        let note = key_to_note(key)?;
        let velocity = Velocity::new(MOCK_VELOCITY)?;
        self.enqueue(NoteEvent::on(note, velocity, now_us));
        self.enqueue(NoteEvent::off(note, now_us + SUSTAIN_MS * 1_000));
        Some(note)
    }

    /// Insert keeping `pending` sorted by timestamp (stable for equal stamps),
    /// so draining in order yields on-before-off for a single press.
    fn enqueue(&mut self, ev: NoteEvent) {
        let pos = self
            .pending
            .iter()
            .position(|e| e.timestamp_us > ev.timestamp_us)
            .unwrap_or(self.pending.len());
        self.pending.insert(pos, ev);
    }

    /// Return (and remove) every queued event whose timestamp is `<= now_us`.
    /// The seam unit tests drive instead of the wall clock.
    fn drain_until(&mut self, now_us: u64) -> Vec<NoteEvent> {
        let split = self
            .pending
            .iter()
            .position(|e| e.timestamp_us > now_us)
            .unwrap_or(self.pending.len());
        self.pending.drain(..split).collect()
    }
}

impl Default for MockKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteSource for MockKeyboard {
    fn events(&mut self) -> Vec<NoteEvent> {
        let now = self.now_us();
        self.drain_until(now)
    }

    fn port_name(&self) -> &str {
        &self.port_name
    }

    fn forward_key(&mut self, key: char) -> bool {
        self.press(key).is_some()
    }
}

// ── ScriptedSource ────────────────────────────────────────────────────────────

/// A controllable event source that replays a pre-loaded timeline.
///
/// Events are released in timestamp order as [`advance`](ScriptedSource::advance)
/// moves the cursor forward; nothing is emitted ahead of its time. This keeps
/// the source's contract identical to live input while being fully controllable
/// in headless tests.
pub struct ScriptedSource {
    /// Events sorted ascending by `timestamp_us`; drained from the front.
    pending: Vec<NoteEvent>,
    /// Current fake-clock position in microseconds.
    cursor_us: u64,
}

impl ScriptedSource {
    /// Create a new source from `events`. They need not be sorted; the
    /// constructor sorts them by `timestamp_us` so `advance` can drain in order.
    pub fn new(mut events: Vec<NoteEvent>) -> Self {
        events.sort_by_key(|e| e.timestamp_us);
        Self {
            pending: events,
            cursor_us: 0,
        }
    }

    /// Advance the fake clock by `dt_us` microseconds. Subsequent calls to
    /// [`NoteSource::events`] will return any events whose `timestamp_us` is
    /// now ≤ the cursor.
    pub fn advance(&mut self, dt_us: u64) {
        self.cursor_us = self.cursor_us.saturating_add(dt_us);
    }

    /// Current fake-clock position (microseconds since creation).
    pub fn cursor_us(&self) -> u64 {
        self.cursor_us
    }
}

impl NoteSource for ScriptedSource {
    /// Drains and returns all events whose `timestamp_us` ≤ the current cursor.
    fn events(&mut self) -> Vec<NoteEvent> {
        let n = self
            .pending
            .partition_point(|e| e.timestamp_us <= self.cursor_us);
        self.pending.drain(..n).collect()
    }

    fn port_name(&self) -> &str {
        "scripted"
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, NoteEventKind, Velocity};

    // -- MockKeyboard tests ---------------------------------------------------

    #[test]
    fn press_1_is_middle_c() {
        let mut kb = MockKeyboard::new();
        assert_eq!(kb.press('1').map(|n| n.value()), Some(60));
    }

    #[test]
    fn unmapped_key_returns_none() {
        let mut kb = MockKeyboard::new();
        // Letters are editor commands, not notes — they must stay unmapped so
        // note-entry and command-entry don't collide (#124).
        assert_eq!(kb.press('z'), None);
        assert_eq!(kb.press('a'), None);
        assert_eq!(kb.press('s'), None);
    }

    #[test]
    fn press_enqueues_on_then_off_sustain_apart() {
        let mut kb = MockKeyboard::new();
        kb.press_at('1', 1_000);

        // Nothing is due before the on-stamp.
        assert!(kb.drain_until(999).is_empty());

        // At the on-stamp, only the note-on is released.
        let on = kb.drain_until(1_000);
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].note.value(), 60);
        assert!(matches!(on[0].kind, NoteEventKind::On { .. }));
        assert_eq!(on[0].timestamp_us, 1_000);

        // The off is still pending until SUSTAIN_MS later.
        let off_stamp = 1_000 + SUSTAIN_MS * 1_000;
        assert!(kb.drain_until(off_stamp - 1).is_empty());
        let off = kb.drain_until(off_stamp);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].note.value(), 60);
        assert_eq!(off[0].kind, NoteEventKind::Off);
        assert_eq!(off[0].timestamp_us, off_stamp);
    }

    #[test]
    fn keymap_is_pinned() {
        let mut kb = MockKeyboard::new();
        // The number row, left to right, is the C-major scale from middle C.
        let expected: &[(char, u8)] = &[
            ('1', 60), // C4
            ('2', 62), // D4
            ('3', 64), // E4
            ('4', 65), // F4
            ('5', 67), // G4
            ('6', 69), // A4
            ('7', 71), // B4
            ('8', 72), // C5
            ('9', 74), // D5
            ('0', 76), // E5
        ];
        for (key, note) in expected {
            assert_eq!(
                kb.press(*key).map(|n| n.value()),
                Some(*note),
                "key {key:?} should map to note {note}"
            );
        }
    }

    #[test]
    fn key_map_getter_matches_press() {
        // The exported table is the same source of truth `press` consults, so a
        // legend built from it can never drift from the actual behaviour.
        let mut kb = MockKeyboard::new();
        for (key, note) in key_map() {
            assert_eq!(kb.press(*key).map(|n| n.value()), Some(*note));
        }
    }

    #[test]
    fn key_map_is_a_white_key_c_major_scale() {
        // Every mapped pitch is a white key (no accidentals): pitch-class is one
        // of C D E F G A B.
        const WHITE_PCS: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
        for (_, note) in key_map() {
            assert!(
                WHITE_PCS.contains(&(note % 12)),
                "note {note} is not a white key"
            );
        }
    }

    #[test]
    fn events_drain_in_timestamp_order() {
        let mut kb = MockKeyboard::new();
        // Two presses; the second's on lands before the first's off.
        kb.press_at('1', 0);
        kb.press_at('2', 10_000);

        let all = kb.drain_until(1_000_000);
        let stamps: Vec<u64> = all.iter().map(|e| e.timestamp_us).collect();
        let mut sorted = stamps.clone();
        sorted.sort_unstable();
        assert_eq!(stamps, sorted, "events must come out in timestamp order");
    }

    #[test]
    fn port_name_is_descriptive() {
        let kb = MockKeyboard::new();
        assert!(!kb.port_name().is_empty());
    }

    // -- ScriptedSource tests -------------------------------------------------

    fn note(n: u8) -> MidiNote {
        MidiNote::new(n).unwrap()
    }
    fn vel(v: u8) -> Velocity {
        Velocity::new(v).unwrap()
    }
    fn on(n: u8, ts: u64) -> NoteEvent {
        NoteEvent::on(note(n), vel(80), ts)
    }
    fn off(n: u8, ts: u64) -> NoteEvent {
        NoteEvent::off(note(n), ts)
    }

    #[test]
    fn no_events_before_advance() {
        let mut src = ScriptedSource::new(vec![on(60, 1_000)]);
        assert!(src.events().is_empty());
    }

    #[test]
    fn events_release_when_cursor_reaches_timestamp() {
        let mut src = ScriptedSource::new(vec![on(60, 1_000)]);
        src.advance(1_000);
        let evs = src.events();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].note.value(), 60);
        assert_eq!(evs[0].timestamp_us, 1_000);
    }

    #[test]
    fn events_drain_exactly_once() {
        let mut src = ScriptedSource::new(vec![on(60, 500)]);
        src.advance(1_000);
        assert_eq!(src.events().len(), 1);
        // Second drain returns nothing.
        assert!(src.events().is_empty());
    }

    #[test]
    fn events_released_in_timestamp_order() {
        // Supply out-of-order; expect them back in order.
        let evs = vec![on(64, 3_000), on(60, 1_000), on(62, 2_000)];
        let mut src = ScriptedSource::new(evs);
        src.advance(5_000);
        let out = src.events();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].timestamp_us, 1_000);
        assert_eq!(out[1].timestamp_us, 2_000);
        assert_eq!(out[2].timestamp_us, 3_000);
    }

    #[test]
    fn only_due_events_released_each_advance() {
        let mut src = ScriptedSource::new(vec![on(60, 1_000), on(62, 3_000), on(64, 5_000)]);

        src.advance(2_000);
        let batch1 = src.events();
        assert_eq!(batch1.len(), 1);
        assert_eq!(batch1[0].note.value(), 60);

        src.advance(2_000); // cursor now at 4_000
        let batch2 = src.events();
        assert_eq!(batch2.len(), 1);
        assert_eq!(batch2[0].note.value(), 62);

        // Nothing at 4_000 < 5_000
        assert!(src.events().is_empty());

        src.advance(2_000); // cursor at 6_000
        let batch3 = src.events();
        assert_eq!(batch3.len(), 1);
        assert_eq!(batch3[0].note.value(), 64);

        // Queue exhausted.
        src.advance(100_000);
        assert!(src.events().is_empty());
    }

    #[test]
    fn port_name_is_scripted() {
        let src = ScriptedSource::new(vec![]);
        assert_eq!(src.port_name(), "scripted");
    }

    #[test]
    fn events_at_exact_cursor_are_released() {
        let mut src = ScriptedSource::new(vec![on(60, 500)]);
        src.advance(500);
        assert_eq!(src.events().len(), 1);
    }

    #[test]
    fn empty_timeline_stays_empty() {
        let mut src = ScriptedSource::new(vec![]);
        src.advance(1_000_000);
        assert!(src.events().is_empty());
    }

    #[test]
    fn on_off_pair_released_in_order() {
        let mut src = ScriptedSource::new(vec![on(60, 0), off(60, 500_000)]);
        src.advance(600_000);
        let evs = src.events();
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0].kind, NoteEventKind::On { .. }));
        assert_eq!(evs[1].kind, NoteEventKind::Off);
    }
}
