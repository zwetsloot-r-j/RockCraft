//! Which hand plays a note — the split line plus per-note exceptions (M14-E).
//!
//! A piece assigns every note to the left or the right hand. The **default** is
//! a pitch **split line**: notes below it are the left hand, at/above it the
//! right. On top of that an author can mark individual notes as **exceptions**
//! (a crossover, a thumb-under passage) and those overrides persist in the
//! bundle's `meta.json`.
//!
//! Everything that keys off "hand" — hand-practice grey-out, one-hand scoring,
//! colouring — reads the *effective* hand: [`Note::effective_hand`](
//! crate::timeline::Note::effective_hand), i.e. the override when set, else the
//! split rule. This module owns both halves so edit, play, and every frontend
//! share one definition; like the rest of `core` it is pure.

use crate::events::MidiNote;
use serde::{Deserialize, Serialize};

/// Which hand plays a note.
///
/// Serialises as `"left"` / `"right"`, the wire names every frontend reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hand {
    Left,
    Right,
}

impl Hand {
    /// A short human/wire label (`"left"` / `"right"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Hand::Left => "left",
            Hand::Right => "right",
        }
    }

    /// The edit setting that pins a note to this hand — the inverse of
    /// [`HandSetting::override_value`], so a snapshot round-trips.
    pub const fn setting(self) -> HandSetting {
        match self {
            Hand::Left => HandSetting::Left,
            Hand::Right => HandSetting::Right,
        }
    }
}

/// What an edit action sets on a note: follow the split line, or pin a hand.
///
/// The parameter type of [`Action::SetNoteHand`](crate::Action::SetNoteHand);
/// `Auto` clears an override rather than storing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandSetting {
    /// Follow the piece's split line (no stored override).
    Auto,
    Left,
    Right,
}

impl HandSetting {
    /// The override this setting stores on a note: `Auto → None`.
    pub const fn override_value(self) -> Option<Hand> {
        match self {
            HandSetting::Auto => None,
            HandSetting::Left => Some(Hand::Left),
            HandSetting::Right => Some(Hand::Right),
        }
    }

    /// The setting an existing override reads as (`None → Auto`).
    pub const fn from_override(hand: Option<Hand>) -> Self {
        match hand {
            None => HandSetting::Auto,
            Some(h) => h.setting(),
        }
    }

    /// The next setting in the edit cycle `Auto → Left → Right → Auto`.
    pub const fn next(self) -> Self {
        match self {
            HandSetting::Auto => HandSetting::Left,
            HandSetting::Left => HandSetting::Right,
            HandSetting::Right => HandSetting::Auto,
        }
    }
}

/// Default left/right split pitch (middle C = 60): notes below it are the left
/// hand, at/above it the right.
pub const DEFAULT_SPLIT: u8 = 60;

/// The split rule, in one place: `pitch < split` → [`Hand::Left`], otherwise
/// [`Hand::Right`].
pub fn hand_of(pitch: MidiNote, split: u8) -> Hand {
    hand_of_pitch_value(pitch.value(), split)
}

/// [`hand_of`] for a raw MIDI number, for the call sites that carry the pitch as
/// a plain `u8` (play spans, wire views). Same rule — the two never diverge.
pub const fn hand_of_pitch_value(pitch: u8, split: u8) -> Hand {
    if pitch < split {
        Hand::Left
    } else {
        Hand::Right
    }
}

/// One persisted per-note hand exception, keyed by musical position.
///
/// In-session notes carry a [`NoteId`](crate::timeline::NoteId), but ids are
/// reassigned on reload and `song.mid` holds no per-note metadata — so overrides
/// travel in `meta.json` keyed by `(pitch, start_us)`, which *is* stable (it is
/// exactly what `Timeline::from_events` sorts by, and it is unique).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandOverride {
    pub pitch: u8,
    pub start_us: u64,
    pub hand: Hand,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(pitch: u8) -> MidiNote {
        MidiNote::new(pitch).unwrap()
    }

    #[test]
    fn split_rule_puts_below_left_and_at_or_above_right() {
        assert_eq!(hand_of(n(59), DEFAULT_SPLIT), Hand::Left);
        // At the split is the RIGHT hand (the boundary belongs to the top half).
        assert_eq!(hand_of(n(60), DEFAULT_SPLIT), Hand::Right);
        assert_eq!(hand_of(n(61), DEFAULT_SPLIT), Hand::Right);
        assert_eq!(hand_of(n(21), DEFAULT_SPLIT), Hand::Left);
        assert_eq!(hand_of(n(108), DEFAULT_SPLIT), Hand::Right);
    }

    #[test]
    fn custom_split_moves_the_boundary() {
        // A split at F3 (53): everything below is left, F3 itself is right.
        assert_eq!(hand_of(n(52), 53), Hand::Left);
        assert_eq!(hand_of(n(53), 53), Hand::Right);
        // Middle C now reads as the right hand under a low split, and as the
        // left hand under a high one.
        assert_eq!(hand_of(n(60), 53), Hand::Right);
        assert_eq!(hand_of(n(60), 72), Hand::Left);
    }

    #[test]
    fn raw_and_typed_split_rules_agree() {
        for pitch in 0u8..=127 {
            for split in [0u8, 21, 53, DEFAULT_SPLIT, 108, 127] {
                assert_eq!(
                    hand_of(n(pitch), split),
                    hand_of_pitch_value(pitch, split),
                    "pitch {pitch} split {split}"
                );
            }
        }
    }

    #[test]
    fn setting_round_trips_through_option_hand() {
        for setting in [HandSetting::Auto, HandSetting::Left, HandSetting::Right] {
            assert_eq!(
                HandSetting::from_override(setting.override_value()),
                setting
            );
        }
        assert_eq!(HandSetting::Auto.override_value(), None);
        assert_eq!(HandSetting::Left.override_value(), Some(Hand::Left));
        assert_eq!(HandSetting::Right.override_value(), Some(Hand::Right));
        assert_eq!(HandSetting::from_override(None), HandSetting::Auto);
        assert_eq!(Hand::Left.setting(), HandSetting::Left);
        assert_eq!(Hand::Right.setting(), HandSetting::Right);
    }

    #[test]
    fn cycle_walks_auto_left_right_auto() {
        let mut s = HandSetting::Auto;
        s = s.next();
        assert_eq!(s, HandSetting::Left);
        s = s.next();
        assert_eq!(s, HandSetting::Right);
        s = s.next();
        assert_eq!(s, HandSetting::Auto);
    }

    #[test]
    fn wire_names_are_stable() {
        assert_eq!(serde_json::to_string(&Hand::Left).unwrap(), "\"left\"");
        assert_eq!(serde_json::to_string(&Hand::Right).unwrap(), "\"right\"");
        assert_eq!(Hand::Left.as_str(), "left");
        assert_eq!(Hand::Right.as_str(), "right");
        assert_eq!(
            serde_json::to_string(&HandSetting::Auto).unwrap(),
            "\"auto\""
        );
        let back: HandSetting = serde_json::from_str("\"right\"").unwrap();
        assert_eq!(back, HandSetting::Right);
    }

    #[test]
    fn hand_override_round_trips() {
        let o = HandOverride {
            pitch: 55,
            start_us: 1_500_000,
            hand: Hand::Right,
        };
        let json = serde_json::to_string(&o).unwrap();
        assert_eq!(o, serde_json::from_str(&json).unwrap());
    }
}
