//! A piano-free [`NoteSource`](crate::NoteSource): typed computer-keyboard keys
//! become [`NoteEvent`]s, so the TUI runs and is developed without hardware.
//!
//! Terminals don't reliably deliver key **release**, so we can't mirror the
//! piano's on/off pairing directly. Instead every [`press`](MockKeyboard::press)
//! enqueues a note-on *now* and a matching note-off `SUSTAIN_MS` later;
//! [`events`](MockKeyboard::events) only releases events whose timestamp has
//! arrived. A held or auto-repeating key therefore produces clean on/off pairs
//! and scoring stays deterministic — judged off timestamps, never frame rate.

use std::time::Instant;

use rockcraft_core::{MidiNote, NoteEvent, Velocity};

use crate::NoteSource;

/// How long a mock note sustains before its note-off, in milliseconds.
const SUSTAIN_MS: u64 = 120;

/// Velocity stamped on mock note-ons (a firm-but-not-max strike).
const MOCK_VELOCITY: u8 = 80;

/// Keyboard → MIDI note map, tracker/FL-style over ~two octaves from C4 (60).
///
/// Home row is the white keys; the QWERTY row sits the black keys in the gaps:
///
/// ```text
///   w   e       t   y   u       o   p
/// a   s   d   f   g   h   j   k   l   ;
/// C   D   E   F   G   A   B   C   D   E      (octave 4 → 5)
/// ```
///
/// Keys not in this table are unmapped (`press` returns `None`). Pinned in tests.
const KEY_MAP: &[(char, u8)] = &[
    // White keys: C4 D4 E4 F4 G4 A4 B4 C5 D5 E5.
    ('a', 60),
    ('s', 62),
    ('d', 64),
    ('f', 65),
    ('g', 67),
    ('h', 69),
    ('j', 71),
    ('k', 72),
    ('l', 74),
    (';', 76),
    // Black keys: C#4 D#4 F#4 G#4 A#4 C#5 D#5.
    ('w', 61),
    ('e', 63),
    ('t', 66),
    ('y', 68),
    ('u', 70),
    ('o', 73),
    ('p', 75),
];

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

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::NoteEventKind;

    #[test]
    fn press_a_is_middle_c() {
        let mut kb = MockKeyboard::new();
        assert_eq!(kb.press('a').map(|n| n.value()), Some(60));
    }

    #[test]
    fn unmapped_key_returns_none() {
        let mut kb = MockKeyboard::new();
        assert_eq!(kb.press('z'), None);
        assert_eq!(kb.press('1'), None);
    }

    #[test]
    fn press_enqueues_on_then_off_sustain_apart() {
        let mut kb = MockKeyboard::new();
        kb.press_at('a', 1_000);

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
        let expected: &[(char, u8)] = &[
            ('a', 60),
            ('w', 61),
            ('s', 62),
            ('e', 63),
            ('d', 64),
            ('f', 65),
            ('t', 66),
            ('g', 67),
            ('y', 68),
            ('h', 69),
            ('u', 70),
            ('j', 71),
            ('k', 72),
            ('o', 73),
            ('l', 74),
            ('p', 75),
            (';', 76),
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
    fn events_drain_in_timestamp_order() {
        let mut kb = MockKeyboard::new();
        // Two presses; the second's on lands before the first's off.
        kb.press_at('a', 0);
        kb.press_at('s', 10_000);

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
}
