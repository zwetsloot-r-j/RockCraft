//! Which finger plays a note — an optional per-note number (M27-A).
//!
//! A note may carry a finger number, 1 (thumb) to 5 (little finger). It is
//! purely a reading aid: it is shown on the note in edit and play, and never
//! affects scoring, wait mode, practice-hand filtering or audio. Which hand the
//! finger belongs to comes from the note's effective hand
//! ([`Note::effective_hand`](crate::timeline::Note::effective_hand)).
//!
//! Like hand overrides, fingers persist in the bundle's `meta.json` keyed by
//! musical position ([`NoteFinger`]), because `song.mid` holds no per-note
//! metadata and note ids are reassigned on reload.

use serde::{Deserialize, Deserializer, Serialize};

/// A piano finger number: 1 = thumb … 5 = little finger.
///
/// Serialises as a bare number (`3`). `0`, `6` and anything that is not a
/// number fail to deserialise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Finger(u8);

impl Finger {
    /// The finger numbered `n`, or `None` when `n` is outside `1..=5`.
    pub const fn new(n: u8) -> Option<Finger> {
        if n >= 1 && n <= 5 {
            Some(Finger(n))
        } else {
            None
        }
    }

    /// The number, always in `1..=5`.
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Finger {
    type Error = String;

    fn try_from(n: u8) -> Result<Self, Self::Error> {
        Finger::new(n).ok_or_else(|| format!("finger must be 1 to 5, got {n}"))
    }
}

impl From<Finger> for u8 {
    fn from(f: Finger) -> u8 {
        f.0
    }
}

/// One persisted finger number, keyed by musical position.
///
/// Same keying as [`HandOverride`](crate::HandOverride): `(pitch, start_us)` is
/// stable across a save and reload, where a `NoteId` is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteFinger {
    pub pitch: u8,
    pub start_us: u64,
    pub finger: Finger,
}

/// Deserialise a list of [`NoteFinger`], dropping entries that are malformed
/// (for example a finger outside `1..=5`) and keeping the rest.
///
/// Used for `meta.json`'s `fingers`, so one bad entry cannot stop a whole
/// bundle from loading.
pub fn deserialize_lenient_fingers<'de, D>(deserializer: D) -> Result<Vec<NoteFinger>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_to_five_are_fingers() {
        assert_eq!(Finger::new(0), None);
        assert_eq!(Finger::new(1).map(Finger::get), Some(1));
        assert_eq!(Finger::new(5).map(Finger::get), Some(5));
        assert_eq!(Finger::new(6), None);
    }

    #[test]
    fn a_finger_is_a_bare_number_on_the_wire() {
        let f = Finger::new(3).unwrap();
        assert_eq!(serde_json::to_string(&f).unwrap(), "3");
        assert_eq!(serde_json::from_str::<Finger>("3").unwrap(), f);
    }

    #[test]
    fn out_of_range_and_non_numbers_are_rejected() {
        for bad in ["0", "6", "\"3\"", "null"] {
            assert!(
                serde_json::from_str::<Finger>(bad).is_err(),
                "{bad} must not parse as a finger"
            );
        }
    }

    #[test]
    fn note_finger_round_trips() {
        let nf = NoteFinger {
            pitch: 60,
            start_us: 1_000,
            finger: Finger::new(2).unwrap(),
        };
        let json = serde_json::to_string(&nf).unwrap();
        assert_eq!(json, r#"{"pitch":60,"start_us":1000,"finger":2}"#);
        assert_eq!(serde_json::from_str::<NoteFinger>(&json).unwrap(), nf);
    }
}
