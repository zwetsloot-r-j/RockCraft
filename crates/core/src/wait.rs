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
    pos: usize,
}

impl WaitTracker {
    /// Build a tracker from expected (pitch, time) notes. Notes sharing a
    /// `time_us` collapse into one chord step; steps are ordered by time.
    pub fn from_expected(notes: &[(MidiNote, u64)]) -> Self {
        use std::collections::BTreeMap;
        let mut by_time: BTreeMap<u64, BTreeSet<u8>> = BTreeMap::new();
        for (note, time_us) in notes {
            by_time.entry(*time_us).or_default().insert(note.value());
        }
        let steps = by_time
            .into_iter()
            .map(|(time_us, pitches)| Step {
                notes: pitches.into_iter().collect(),
                time_us,
            })
            .collect();
        Self { steps, pos: 0 }
    }

    /// The step the player must currently satisfy, or `None` if complete.
    pub fn current(&self) -> Option<&Step> {
        self.steps.get(self.pos)
    }

    /// Is the current step satisfied by this held-note set? (Extra held notes
    /// are allowed.) `false` if already complete.
    pub fn is_satisfied(&self, held: &BTreeSet<u8>) -> bool {
        match self.current() {
            None => false,
            Some(step) => step.notes.iter().all(|n| held.contains(n)),
        }
    }

    /// Advance past every consecutive satisfied step. Returns `true` if the
    /// position moved.
    pub fn update(&mut self, held: &BTreeSet<u8>) -> bool {
        let start = self.pos;
        while self.is_satisfied(held) {
            self.pos += 1;
        }
        self.pos > start
    }

    /// Have all steps been completed?
    pub fn is_complete(&self) -> bool {
        self.pos >= self.steps.len()
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

/// Whether a gated [`PlayClock`](crate::play_clock::PlayClock) should keep
/// advancing or freeze on the current step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    /// The clock may advance.
    Running,
    /// The clock must freeze: an armed wait-step is due and unsatisfied.
    Frozen,
}

/// Couples a [`WaitTracker`] with the live held-note set to gate a
/// [`PlayClock`](crate::play_clock::PlayClock). Pure: feed it held notes plus
/// the clock position, read back whether playback should freeze on the current
/// step. The gate freezes only once the clock has *reached* a step's `time_us`
/// and that step is unsatisfied; while disarmed it never freezes (free
/// play-through).
#[derive(Debug, Clone)]
pub struct WaitGate {
    tracker: WaitTracker,
    held: BTreeSet<u8>,
    armed: bool,
    /// The result of the most recent [`poll`](WaitGate::poll); backs
    /// [`awaiting`](WaitGate::awaiting).
    frozen: bool,
}

impl WaitGate {
    /// Build a gate from expected (pitch, time) notes. Starts **disarmed**
    /// (wait-mode off) with no notes held.
    pub fn from_expected(notes: &[(MidiNote, u64)]) -> Self {
        Self {
            tracker: WaitTracker::from_expected(notes),
            held: BTreeSet::new(),
            armed: false,
            frozen: false,
        }
    }

    /// Turn wait-mode on (`armed = true`) or off.
    pub fn set_armed(&mut self, armed: bool) {
        self.armed = armed;
        if !armed {
            self.frozen = false;
        }
    }

    /// Is wait-mode currently armed?
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Replace the live held-note set (call on every note-on/off).
    pub fn set_held(&mut self, held: BTreeSet<u8>) {
        self.held = held;
    }

    /// Advance the tracker past any now-satisfied steps, then report whether the
    /// clock must freeze. Returns [`GateState::Frozen`] when armed AND the
    /// current step's `time_us <= now_us` AND it is unsatisfied; otherwise
    /// [`GateState::Running`]. Always `Running` while disarmed.
    pub fn poll(&mut self, now_us: u64) -> GateState {
        // Reuse the tracker's own advance-through-satisfied logic.
        self.tracker.update(&self.held);

        if !self.armed {
            self.frozen = false;
            return GateState::Running;
        }

        match self.tracker.current() {
            Some(step) if step.time_us <= now_us && !self.tracker.is_satisfied(&self.held) => {
                self.frozen = true;
                GateState::Frozen
            }
            _ => {
                self.frozen = false;
                GateState::Running
            }
        }
    }

    /// Have all steps been satisfied? Delegates to the tracker.
    pub fn is_complete(&self) -> bool {
        self.tracker.is_complete()
    }

    /// The step currently being waited on, if the last [`poll`](WaitGate::poll)
    /// froze the clock; otherwise `None`.
    pub fn awaiting(&self) -> Option<&Step> {
        if self.frozen {
            self.tracker.current()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    fn n(v: u8) -> MidiNote {
        MidiNote::new(v).unwrap()
    }
    fn held(notes: &[u8]) -> BTreeSet<u8> {
        notes.iter().copied().collect()
    }

    #[test]
    fn disarmed_is_always_running() {
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(62), 1000)]);
        assert!(!g.is_armed());
        // No notes held, well past the first step's time — still Running.
        assert_eq!(g.poll(5000), GateState::Running);
        assert!(g.awaiting().is_none());
        // Wrong notes held — still Running.
        g.set_held(held(&[65]));
        assert_eq!(g.poll(5000), GateState::Running);
    }

    #[test]
    fn armed_runs_before_step_is_due() {
        let mut g = WaitGate::from_expected(&[(n(60), 1000)]);
        g.set_armed(true);
        // now_us < step.time_us → not yet due → Running.
        assert_eq!(g.poll(500), GateState::Running);
        assert!(g.awaiting().is_none());
    }

    #[test]
    fn armed_freezes_on_due_unsatisfied_step() {
        let mut g = WaitGate::from_expected(&[(n(60), 1000)]);
        g.set_armed(true);
        assert_eq!(g.poll(1000), GateState::Frozen);
        let step = g.awaiting().expect("frozen on the due step");
        assert_eq!(step.notes, vec![60]);
        assert_eq!(step.time_us, 1000);
    }

    #[test]
    fn holding_required_note_unfreezes_and_advances() {
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(62), 1000)]);
        g.set_armed(true);
        assert_eq!(g.poll(0), GateState::Frozen);
        // Hold the required C; next poll advances past it and runs.
        g.set_held(held(&[60]));
        assert_eq!(g.poll(0), GateState::Running);
        assert!(g.awaiting().is_none());
        // The tracker advanced: the second step isn't due yet at now=0.
        assert_eq!(g.poll(500), GateState::Running);
        // It becomes due and unsatisfied at its time.
        assert_eq!(g.poll(1000), GateState::Frozen);
        assert_eq!(g.awaiting().unwrap().notes, vec![62]);
    }

    #[test]
    fn chord_requires_all_notes() {
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(64), 0), (n(67), 0)]);
        g.set_armed(true);
        g.set_held(held(&[60, 64])); // missing G
        assert_eq!(g.poll(0), GateState::Frozen);
        g.set_held(held(&[60, 64, 67])); // full chord
        assert_eq!(g.poll(0), GateState::Running);
        assert!(g.is_complete());
    }

    #[test]
    fn multiple_satisfied_steps_skip_in_one_poll() {
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(60), 100), (n(62), 200)]);
        g.set_armed(true);
        // Holding C satisfies both consecutive C-steps at once.
        g.set_held(held(&[60]));
        assert_eq!(g.poll(150), GateState::Running);
        // Now waiting on the D step; due at 200.
        assert_eq!(g.poll(200), GateState::Frozen);
        assert_eq!(g.awaiting().unwrap().notes, vec![62]);
    }

    #[test]
    fn complete_is_never_frozen() {
        let mut g = WaitGate::from_expected(&[(n(60), 0)]);
        g.set_armed(true);
        g.set_held(held(&[60]));
        assert_eq!(g.poll(0), GateState::Running);
        assert!(g.is_complete());
        // Past the end, even with nothing held, never freezes.
        g.set_held(held(&[]));
        assert_eq!(g.poll(10_000), GateState::Running);
        assert!(g.awaiting().is_none());
    }

    #[test]
    fn disarming_clears_frozen_state() {
        let mut g = WaitGate::from_expected(&[(n(60), 0)]);
        g.set_armed(true);
        assert_eq!(g.poll(0), GateState::Frozen);
        assert!(g.awaiting().is_some());
        g.set_armed(false);
        assert!(g.awaiting().is_none());
        assert_eq!(g.poll(0), GateState::Running);
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
