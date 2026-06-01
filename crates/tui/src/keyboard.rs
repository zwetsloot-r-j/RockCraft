//! Pure keyboard geometry and held-note state — no terminal, no I/O.
//!
//! This module is deliberately headless so the fiddly 88-key layout math is
//! unit-tested in CI (the device/rendering parts live in `app.rs`). The view
//! layer asks this module "given a width of N columns, where does every key
//! go?" and "which keys are currently held?", then just paints the answer.

use std::collections::BTreeSet;

/// Lowest key of a standard 88-key piano: A0 (MIDI 21).
pub const LOWEST_MIDI: u8 = 21;
/// Highest key: C8 (MIDI 108).
pub const HIGHEST_MIDI: u8 = 108;
/// Number of white keys on an 88-key board.
pub const WHITE_KEY_COUNT: usize = 52;

/// Is this MIDI note a black (sharp/flat) key?
///
/// Within an octave the black pitch classes are C#, D#, F#, G#, A#
/// (offsets 1, 3, 6, 8, 10 from C).
pub fn is_black_key(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// The 0-based index of a white key counting from A0, or `None` for black keys
/// and notes outside the 88-key range.
pub fn white_index(note: u8) -> Option<usize> {
    if !(LOWEST_MIDI..=HIGHEST_MIDI).contains(&note) || is_black_key(note) {
        return None;
    }
    // Count white keys in [LOWEST_MIDI, note).
    Some((LOWEST_MIDI..note).filter(|n| !is_black_key(*n)).count())
}

/// How wide (in terminal columns) each white key is rendered, chosen from the
/// available width. Drives the whole layout; black keys straddle boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// 3 columns per white key — roomy, octave labels fit (needs ≥156 cols).
    Roomy,
    /// 2 columns per white key — clean black keys (needs ≥104 cols).
    Clean,
    /// 1 column per white key — compact, degenerate black keys (needs ≥52).
    Compact,
}

impl Scale {
    /// Columns per white key.
    pub const fn white_width(self) -> u16 {
        match self {
            Scale::Roomy => 3,
            Scale::Clean => 2,
            Scale::Compact => 1,
        }
    }

    /// Total columns needed to draw all 88 keys at this scale.
    pub const fn total_width(self) -> u16 {
        self.white_width() * WHITE_KEY_COUNT as u16
    }

    /// The largest scale whose full 88-key board fits in `available` columns,
    /// or `None` if even the compact board doesn't fit.
    pub fn best_for_width(available: u16) -> Option<Scale> {
        [Scale::Roomy, Scale::Clean, Scale::Compact]
            .into_iter()
            .find(|s| s.total_width() <= available)
    }
}

/// The set of currently-held notes, updated from the live `NoteEvent` stream.
///
/// Pure: feed it events, ask which notes are down. `on(note, vel0)` is treated
/// as note-off to match MIDI convention (the midi crate already normalises
/// this, but we stay correct regardless of source).
#[derive(Debug, Default, Clone)]
pub struct HeldNotes {
    down: BTreeSet<u8>,
}

impl HeldNotes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a note event.
    pub fn apply(&mut self, ev: &rockcraft_core::NoteEvent) {
        use rockcraft_core::NoteEventKind;
        match ev.kind {
            NoteEventKind::On { velocity } if velocity.value() > 0 => {
                self.down.insert(ev.note.value());
            }
            // note-on vel 0 and note-off both release.
            _ => {
                self.down.remove(&ev.note.value());
            }
        }
    }

    /// Is this MIDI note currently held?
    pub fn is_held(&self, note: u8) -> bool {
        self.down.contains(&note)
    }

    /// Number of notes currently held.
    pub fn len(&self) -> usize {
        self.down.len()
    }

    /// Are no notes currently held?
    pub fn is_empty(&self) -> bool {
        self.down.is_empty()
    }

    /// Currently-held notes, ascending.
    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        self.down.iter().copied()
    }
}

/// The horizontal placement of one white key: its starting column (relative to
/// the keyboard's left edge) and width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhiteKeySlot {
    pub note: u8,
    pub col: u16,
    pub width: u16,
}

/// Compute the column placement of every white key at the given scale.
///
/// Black keys are drawn as an overlay between white keys (see `app.rs`); this
/// returns just the 52 white-key slots in pitch order.
pub fn white_key_slots(scale: Scale) -> Vec<WhiteKeySlot> {
    let w = scale.white_width();
    (LOWEST_MIDI..=HIGHEST_MIDI)
        .filter(|n| !is_black_key(*n))
        .enumerate()
        .map(|(i, note)| WhiteKeySlot {
            note,
            col: i as u16 * w,
            width: w,
        })
        .collect()
}

/// For a black key, the column it should be drawn at: nestled at the boundary
/// between the white key below it and the next. Returns `None` for white keys.
/// Column is relative to the keyboard's left edge.
pub fn black_key_col(note: u8, scale: Scale) -> Option<u16> {
    if !is_black_key(note) || !(LOWEST_MIDI..=HIGHEST_MIDI).contains(&note) {
        return None;
    }
    // The white key immediately below this black key.
    let white_below = white_index(note - 1)?;
    let w = scale.white_width();
    // Sit at the right edge of the white key below (the boundary).
    Some(white_below as u16 * w + w.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, NoteEvent, Velocity};

    fn n(v: u8) -> MidiNote {
        MidiNote::new(v).unwrap()
    }

    #[test]
    fn black_key_classification() {
        assert!(!is_black_key(60)); // C4
        assert!(is_black_key(61)); // C#4
        assert!(!is_black_key(62)); // D4
        assert!(is_black_key(70)); // A#4
        assert!(!is_black_key(71)); // B4
    }

    #[test]
    fn eighty_eight_keys_have_52_whites_36_blacks() {
        let whites = (LOWEST_MIDI..=HIGHEST_MIDI)
            .filter(|n| !is_black_key(*n))
            .count();
        let blacks = (LOWEST_MIDI..=HIGHEST_MIDI)
            .filter(|n| is_black_key(*n))
            .count();
        assert_eq!(whites, 52);
        assert_eq!(blacks, 36);
        assert_eq!(whites + blacks, 88);
    }

    #[test]
    fn white_index_endpoints() {
        assert_eq!(white_index(LOWEST_MIDI), Some(0)); // A0 is the first white
        assert_eq!(white_index(HIGHEST_MIDI), Some(51)); // C8 is the 52nd white
        assert_eq!(white_index(61), None); // black key
        assert_eq!(white_index(20), None); // below range
        assert_eq!(white_index(109), None); // above range
    }

    #[test]
    fn scale_total_widths() {
        assert_eq!(Scale::Compact.total_width(), 52);
        assert_eq!(Scale::Clean.total_width(), 104);
        assert_eq!(Scale::Roomy.total_width(), 156);
    }

    #[test]
    fn best_scale_for_width() {
        assert_eq!(Scale::best_for_width(200), Some(Scale::Roomy));
        assert_eq!(Scale::best_for_width(156), Some(Scale::Roomy));
        assert_eq!(Scale::best_for_width(120), Some(Scale::Clean));
        assert_eq!(Scale::best_for_width(104), Some(Scale::Clean));
        assert_eq!(Scale::best_for_width(80), Some(Scale::Compact));
        assert_eq!(Scale::best_for_width(52), Some(Scale::Compact));
        assert_eq!(Scale::best_for_width(51), None); // too narrow
    }

    #[test]
    fn white_slots_span_full_width_without_gaps() {
        let slots = white_key_slots(Scale::Clean);
        assert_eq!(slots.len(), 52);
        assert_eq!(slots[0].col, 0);
        // Each slot starts right after the previous (no gaps/overlaps).
        for pair in slots.windows(2) {
            assert_eq!(pair[1].col, pair[0].col + pair[0].width);
        }
        // Last slot ends exactly at total width.
        let last = slots.last().unwrap();
        assert_eq!(last.col + last.width, Scale::Clean.total_width());
    }

    #[test]
    fn black_key_sits_between_whites() {
        // C#4 (61) should sit at the boundary after C4 (60).
        let c4_white = white_index(60).unwrap();
        let col = black_key_col(61, Scale::Clean).unwrap();
        assert_eq!(col, c4_white as u16 * 2 + 1);
        // White keys return None.
        assert_eq!(black_key_col(60, Scale::Clean), None);
    }

    #[test]
    fn held_notes_press_and_release() {
        let mut held = HeldNotes::new();
        assert!(held.is_empty());

        held.apply(&NoteEvent::on(n(60), Velocity::new(80).unwrap(), 0));
        held.apply(&NoteEvent::on(n(64), Velocity::new(80).unwrap(), 10));
        assert!(held.is_held(60));
        assert!(held.is_held(64));
        assert_eq!(held.len(), 2);

        held.apply(&NoteEvent::off(n(60), 20));
        assert!(!held.is_held(60));
        assert!(held.is_held(64));
        assert_eq!(held.len(), 1);
    }

    #[test]
    fn note_on_velocity_zero_releases() {
        let mut held = HeldNotes::new();
        held.apply(&NoteEvent::on(n(72), Velocity::new(90).unwrap(), 0));
        assert!(held.is_held(72));
        // on with vel 0 == off
        held.apply(&NoteEvent::on(n(72), Velocity::new(0).unwrap(), 5));
        assert!(!held.is_held(72));
    }
}
