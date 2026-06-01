//! Pure note-by-note "wait mode" state machine: playback advances only when the
//! player holds the notes the current step requires. No device, no clock — feed
//! it the held-note set, ask whether to advance.
//!
//! IMPLEMENTATION NOTE (seeded task): the `#[cfg(test)]` module at the bottom is
//! the contract — pre-committed, must pass UNMODIFIED. Implement the public API
//! it exercises. The stubs fix the API surface; replace the `todo!()` bodies.

use crate::MidiNote;
use std::collections::BTreeSet;

/// One step of a song: the set of pitches that must be struck together (a single
/// note, or all notes of a chord), with the song time it occurs at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Required MIDI pitches, ascending, de-duplicated.
    pub notes: Vec<u8>,
    pub time_us: u64,
}

/// Tracks progress through an ordered list of steps, advancing as the player
/// satisfies each one.
#[derive(Debug, Clone)]
pub struct WaitTracker {
    steps: Vec<Step>,
    // Read by current()/is_complete() once implemented; allow on the seed stub
    // so the baseline is clippy-clean. Remove the allow when you wire it up.
    #[allow(dead_code)]
    pos: usize,
}

impl WaitTracker {
    /// Build a tracker from expected (pitch, time) notes. Notes sharing a
    /// `time_us` collapse into one chord step; steps are ordered by time.
    pub fn from_expected(notes: &[(MidiNote, u64)]) -> Self {
        let _ = notes;
        todo!("group notes by time into ordered steps")
    }

    /// The step the player must currently satisfy, or `None` if complete.
    pub fn current(&self) -> Option<&Step> {
        todo!("return the step at the current position")
    }

    /// Is the current step satisfied by this held-note set? (Extra held notes
    /// are allowed.) `false` if already complete.
    pub fn is_satisfied(&self, held: &BTreeSet<u8>) -> bool {
        let _ = held;
        todo!("all required notes of the current step present in `held`")
    }

    /// Advance past every consecutive satisfied step. Returns `true` if the
    /// position moved.
    pub fn update(&mut self, held: &BTreeSet<u8>) -> bool {
        let _ = held;
        todo!("advance while current step is satisfied; report movement")
    }

    /// Have all steps been completed?
    pub fn is_complete(&self) -> bool {
        todo!("position is past the last step")
    }

    /// Total number of steps.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// True if there are no steps.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: u8) -> MidiNote {
        MidiNote::new(v).unwrap()
    }
    fn held(notes: &[u8]) -> BTreeSet<u8> {
        notes.iter().copied().collect()
    }

    #[test]
    fn groups_notes_by_time_into_steps() {
        // C and E at t=0 (chord), G at t=1000 (single)
        let t = WaitTracker::from_expected(&[(n(60), 0), (n(64), 0), (n(67), 1000)]);
        assert_eq!(t.len(), 2);
        assert_eq!(t.current().unwrap().notes, vec![60, 64]);
        assert_eq!(t.current().unwrap().time_us, 0);
    }

    #[test]
    fn single_note_step_advances_when_held() {
        let mut t = WaitTracker::from_expected(&[(n(60), 0), (n(62), 1000)]);
        assert!(!t.is_satisfied(&held(&[]))); // nothing held
        assert!(t.is_satisfied(&held(&[60]))); // C held
        let moved = t.update(&held(&[60]));
        assert!(moved);
        assert_eq!(t.current().unwrap().notes, vec![62]); // advanced to D
    }

    #[test]
    fn chord_requires_all_notes() {
        let mut t = WaitTracker::from_expected(&[(n(60), 0), (n(64), 0), (n(67), 0)]);
        assert!(!t.is_satisfied(&held(&[60, 64]))); // missing G
        assert!(!t.update(&held(&[60, 64])));
        assert!(t.is_satisfied(&held(&[60, 64, 67]))); // full chord
        assert!(t.update(&held(&[60, 64, 67])));
        assert!(t.is_complete());
    }

    #[test]
    fn extra_held_notes_are_allowed() {
        let mut t = WaitTracker::from_expected(&[(n(60), 0)]);
        // playing C plus an extra D still satisfies the C step
        assert!(t.is_satisfied(&held(&[60, 62])));
        assert!(t.update(&held(&[60, 62])));
        assert!(t.is_complete());
    }

    #[test]
    fn cannot_advance_without_satisfying() {
        let mut t = WaitTracker::from_expected(&[(n(60), 0), (n(62), 1000)]);
        // wrong note held
        assert!(!t.update(&held(&[65])));
        assert_eq!(t.current().unwrap().notes, vec![60]); // still on first step
    }

    #[test]
    fn advances_through_multiple_satisfied_steps_at_once() {
        // if held covers several consecutive steps, advance through all of them
        let mut t = WaitTracker::from_expected(&[(n(60), 0), (n(60), 100), (n(62), 200)]);
        // holding C satisfies both C-steps in a row
        let moved = t.update(&held(&[60]));
        assert!(moved);
        assert_eq!(t.current().unwrap().notes, vec![62]); // skipped to the D step
    }

    #[test]
    fn complete_when_all_done() {
        let mut t = WaitTracker::from_expected(&[(n(60), 0)]);
        assert!(!t.is_complete());
        t.update(&held(&[60]));
        assert!(t.is_complete());
        assert!(t.current().is_none());
        // satisfied is false once complete
        assert!(!t.is_satisfied(&held(&[60])));
    }

    #[test]
    fn empty_song() {
        let t = WaitTracker::from_expected(&[]);
        assert!(t.is_empty());
        assert!(t.is_complete());
        assert!(t.current().is_none());
    }
}
