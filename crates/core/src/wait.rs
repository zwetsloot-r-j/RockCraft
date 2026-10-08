//! Pure note-by-note "wait mode" state machine: playback advances only when the
//! player holds the notes the current step requires. No device, no clock — feed
//! it the held-note set, ask whether to advance.
//!
//! IMPLEMENTATION NOTE (seeded task): the `#[cfg(test)]` module at the bottom is
//! the contract — pre-committed, must pass UNMODIFIED. Implement the public API
//! it exercises. The stubs fix the API surface; replace the `todo!()` bodies.

use crate::MidiNote;
use std::collections::{BTreeMap, BTreeSet};

/// How far (song µs) *before* a step's onset a strike may land and still count
/// for that step. Matches the scoring "good" window
/// ([`ScoreConfig::default`](crate::scoring::ScoreConfig)), so a press that
/// would score as a hit also releases the wait. A key struck earlier than this
/// and merely kept down is stale: the step waits for it to be struck again.
pub const EARLY_STRIKE_US: u64 = 150_000;

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

    /// Advance past consecutive steps that are BOTH due (`time_us <= now_us`) and
    /// satisfied by `held`. Unlike [`update`](Self::update) this never consumes a
    /// step before its time — so a note held early (e.g. the key used to *start*
    /// the take, still down through the lead-in) cannot pre-satisfy a not-yet-due
    /// step and skip its wait. Returns `true` if the position moved.
    pub fn advance_due(&mut self, held: &BTreeSet<u8>, now_us: u64) -> bool {
        let start = self.pos;
        while let Some(step) = self.steps.get(self.pos) {
            if step.time_us <= now_us && step.notes.iter().all(|n| held.contains(n)) {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.pos > start
    }

    /// Seek to the first step at or after `now_us`, treating earlier steps as
    /// already-passed. Steps at exactly `now_us` are kept (still to be played).
    ///
    /// This is for rebuilding a tracker **mid-take** — e.g. the practice hand
    /// changed, so the step list is regenerated. A fresh tracker starts at step
    /// 0 (the song's start), but the player is at the playhead, not the start;
    /// without this seek the gate would freeze on a step already in the past and
    /// never advance (the note it waits for was played long ago). Steps are
    /// time-ordered, so this is the first index whose `time_us >= now_us`.
    pub fn seek_to(&mut self, now_us: u64) {
        self.pos = self.steps.partition_point(|s| s.time_us < now_us);
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
///
/// # Fresh-strike requirement
///
/// A step is satisfied only when **every** required pitch is held *and* carries
/// its own pending strike that is
///
/// - **unconsumed** — a strike is used up by the step it satisfies, so a key
///   kept down from a previous note (or chord) never counts again: a repeated
///   note, a re-hit chord, or a common tone shared with the next chord must
///   each be struck anew; and
/// - **recent** — struck no earlier than [`EARLY_STRIKE_US`] of song time
///   before the step's onset. A key pressed long ago and just held (a wrong
///   note left down, an anticipated note) does not satisfy a step when it
///   finally comes due.
///
/// A strike is inferred from the held set growing and stamped with the song
/// time it was seen at; releasing a pitch drops its pending strike, so a clean
/// release-and-repress always counts. While the clock is frozen on a step, song
/// time stands at the step's onset, so anything struck during the wait is
/// recent by construction.
#[derive(Debug, Clone)]
pub struct WaitGate {
    tracker: WaitTracker,
    held: BTreeSet<u8>,
    /// The held set as of the previous [`set_held`](WaitGate::set_held), to
    /// diff against for freshly-struck pitches.
    prev_held: BTreeSet<u8>,
    /// Pitches with a pending strike (struck since the last time a step
    /// consumed them, and still held), each with the song time it was struck
    /// at. A step consumes its required pitches from this map as it advances,
    /// so a later repeat needs a new strike.
    struck: BTreeMap<u8, u64>,
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
            prev_held: BTreeSet::new(),
            struck: BTreeMap::new(),
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

    /// Replace the live held-note set as of song time `now_us` (call on every
    /// note-on/off, or every tick). Any pitch that newly appears counts as a
    /// *strike* stamped at `now_us`; a pitch that disappears drops its pending
    /// strike, so re-pressing it after a release counts again.
    /// See the [type-level note](WaitGate#fresh-strike-requirement).
    pub fn set_held(&mut self, held: BTreeSet<u8>, now_us: u64) {
        for &n in held.difference(&self.prev_held) {
            self.struck.insert(n, now_us);
        }
        // A released key is no longer a pending strike (and can't satisfy a
        // repeat until it is struck again).
        self.struck.retain(|n, _| held.contains(n));
        self.prev_held = held.clone();
        self.held = held;
    }

    /// Whether `step` is satisfied *now*: every pitch held with its own
    /// unconsumed strike, struck no earlier than [`EARLY_STRIKE_US`] before the
    /// step's onset. See the [type-level note](WaitGate#fresh-strike-requirement).
    fn fresh_satisfied(&self, step: &Step) -> bool {
        let earliest = step.time_us.saturating_sub(EARLY_STRIKE_US);
        step.notes
            .iter()
            .all(|n| self.held.contains(n) && self.struck.get(n).is_some_and(|&at| at >= earliest))
    }

    /// Advance past every consecutive step that is due and freshly struck in
    /// full — consuming that step's pitches from the pending-strike set
    /// so a following repeat of the same pitch still waits for a new strike.
    fn advance_fresh(&mut self, now_us: u64) {
        // `.cloned()` ends the borrow of `steps` in the condition so the body can
        // mutate `struck`/`pos`.
        while let Some(step) = self.tracker.steps.get(self.tracker.pos).cloned() {
            if step.time_us <= now_us && self.fresh_satisfied(&step) {
                for n in &step.notes {
                    self.struck.remove(n);
                }
                self.tracker.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Seek the tracker to the first step at or after `now_us`, discarding
    /// earlier steps as already-passed (see [`WaitTracker::seek_to`]). Clears any
    /// stale frozen flag; the next [`poll`](WaitGate::poll) recomputes it.
    ///
    /// Call this after rebuilding a gate mid-take (the practice hand or split
    /// changed) so it resumes at the playhead instead of freezing on a step the
    /// player already passed.
    pub fn seek_to(&mut self, now_us: u64) {
        self.tracker.seek_to(now_us);
        self.frozen = false;
        // Discard pending strikes: after a jump the player re-articulates the
        // note under the new playhead from scratch.
        self.struck.clear();
    }

    /// Advance the tracker past any now-satisfied steps, then report whether the
    /// clock must freeze. Returns [`GateState::Frozen`] when armed AND the
    /// current step's `time_us <= now_us` AND it is unsatisfied; otherwise
    /// [`GateState::Running`]. Always `Running` while disarmed.
    ///
    /// "Satisfied" here means every required pitch is held with its own recent,
    /// unconsumed strike — a held-over or long-held note does not count (see the
    /// [type-level note](WaitGate#fresh-strike-requirement)). Advancing only
    /// through steps that are BOTH due and satisfied also means a note held
    /// before its time (e.g. the key used to start the take, still down during
    /// the lead-in) never pre-satisfies a not-yet-due step and skips its wait.
    pub fn poll(&mut self, now_us: u64) -> GateState {
        self.advance_fresh(now_us);

        if !self.armed {
            self.frozen = false;
            return GateState::Running;
        }

        match self.tracker.current() {
            Some(step) if step.time_us <= now_us && !self.fresh_satisfied(step) => {
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

    /// The `time_us` of the step the gate is currently pointed at (the next one
    /// that can freeze), or `None` when complete. A caller advancing a clock can
    /// clamp its step to this so a large tick never carries the clock *past* an
    /// unsatisfied wait-step's onset — the freeze then pins the note at its
    /// onset instead of wherever the overshoot landed.
    pub fn next_step_time(&self) -> Option<u64> {
        self.tracker.current().map(|s| s.time_us)
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
        g.set_held(held(&[65]), 5000);
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
        g.set_held(held(&[60]), 0);
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
        g.set_held(held(&[60, 64]), 0); // missing G
        assert_eq!(g.poll(0), GateState::Frozen);
        g.set_held(held(&[60, 64, 67]), 0); // full chord
        assert_eq!(g.poll(0), GateState::Running);
        assert!(g.is_complete());
    }

    #[test]
    fn repeated_note_needs_a_fresh_strike_not_a_hold() {
        // Two C-steps in a row: striking C once clears the first, but the repeat
        // must NOT auto-advance while the key is still held — it waits for a new
        // strike (the "wait mode resumes while I'm still holding the same note"
        // bug). A release + re-press then advances past it.
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(60), 100), (n(62), 200)]);
        g.set_armed(true);
        g.set_held(held(&[60]), 150); // strike C → clears step@0
        assert_eq!(g.poll(150), GateState::Frozen, "the repeated C still waits");
        assert_eq!(g.awaiting().unwrap().notes, vec![60]);
        assert_eq!(g.awaiting().unwrap().time_us, 100);
        // Still holding — no fresh strike — stays frozen.
        assert_eq!(g.poll(150), GateState::Frozen);
        // Release and re-strike C → advances past the repeat.
        g.set_held(held(&[]), 150);
        g.set_held(held(&[60]), 150);
        assert_eq!(g.poll(150), GateState::Running);
        // Now waiting on the D step; due at 200.
        assert_eq!(g.poll(200), GateState::Frozen);
        assert_eq!(g.awaiting().unwrap().notes, vec![62]);
    }

    #[test]
    fn holding_over_the_next_notes_pitch_does_not_advance() {
        // The exact report: the current step's note equals one you're already
        // holding from before. It must still freeze until you re-strike it.
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(60), 1000)]);
        g.set_armed(true);
        g.set_held(held(&[60]), 0); // strike C, satisfies step@0
        assert_eq!(g.poll(0), GateState::Running);
        // Keep holding C through to the repeat's time — it must freeze, not skip.
        assert_eq!(g.poll(1000), GateState::Frozen);
        assert_eq!(g.awaiting().unwrap().notes, vec![60]);
        // A fresh strike (release + press) releases it.
        g.set_held(held(&[]), 1000);
        g.set_held(held(&[60]), 1000);
        assert_eq!(g.poll(1000), GateState::Running);
        assert!(g.is_complete());
    }

    #[test]
    fn a_held_common_tone_must_be_restruck_for_the_next_chord() {
        // C+E then C+G: the C kept down from the first chord was consumed by it,
        // so striking only G does not satisfy the second chord — C must be
        // struck again too.
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(64), 0), (n(60), 100), (n(67), 100)]);
        g.set_armed(true);
        g.set_held(held(&[60, 64]), 0); // strike C+E → clears the C/E chord
        assert_eq!(g.poll(50), GateState::Running);
        g.set_held(held(&[60]), 80); // E up, C sustained
        g.set_held(held(&[60, 67]), 100); // G struck
        assert_eq!(g.poll(100), GateState::Frozen, "held-over C is consumed");
        // Re-strike C → the C/G chord passes.
        g.set_held(held(&[67]), 100);
        g.set_held(held(&[60, 67]), 100);
        assert_eq!(g.poll(100), GateState::Running);
        assert!(g.is_complete());
    }

    #[test]
    fn a_key_held_long_before_the_step_does_not_satisfy_it() {
        // D struck at t=0 (e.g. a wrong note, or anticipated) and kept down
        // until its step at t=1s: the strike is stale, so the step waits.
        let mut g = WaitGate::from_expected(&[(n(62), 1_000_000)]);
        g.set_armed(true);
        g.set_held(held(&[62]), 0);
        assert_eq!(g.poll(0), GateState::Running);
        assert_eq!(g.poll(1_000_000), GateState::Frozen);
        // Re-striking it during the wait releases it.
        g.set_held(held(&[]), 1_000_000);
        g.set_held(held(&[62]), 1_000_000);
        assert_eq!(g.poll(1_000_000), GateState::Running);
        assert!(g.is_complete());
    }

    #[test]
    fn a_slightly_early_strike_counts() {
        // Struck within the early window before the onset and held → passes
        // without a freeze, like a hit that scores.
        let onset = 1_000_000;
        let mut g = WaitGate::from_expected(&[(n(62), onset)]);
        g.set_armed(true);
        g.set_held(held(&[62]), onset - EARLY_STRIKE_US);
        assert_eq!(g.poll(onset), GateState::Running);
        assert!(g.is_complete());
    }

    #[test]
    fn every_chord_note_needs_a_recent_strike() {
        // C struck long ago and held, E struck at the onset: the chord still
        // waits on a fresh C (previously one fresh note carried the chord).
        let onset = 1_000_000;
        let mut g = WaitGate::from_expected(&[(n(60), onset), (n(64), onset)]);
        g.set_armed(true);
        g.set_held(held(&[60]), 0);
        g.set_held(held(&[60, 64]), onset);
        assert_eq!(g.poll(onset), GateState::Frozen);
        g.set_held(held(&[64]), onset);
        g.set_held(held(&[60, 64]), onset);
        assert_eq!(g.poll(onset), GateState::Running);
    }

    #[test]
    fn one_strike_cannot_pass_two_steps() {
        // C@0 then E@50 and C@60 in quick succession: the C struck for the
        // first step is consumed, so the later C (well within the early window
        // of the first strike) still needs its own strike.
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(64), 50), (n(60), 60)]);
        g.set_armed(true);
        g.set_held(held(&[60]), 0);
        assert_eq!(g.poll(0), GateState::Running);
        g.set_held(held(&[60, 64]), 50);
        assert_eq!(g.poll(60), GateState::Frozen);
        assert_eq!(g.awaiting().unwrap().notes, vec![60]);
    }

    #[test]
    fn complete_is_never_frozen() {
        let mut g = WaitGate::from_expected(&[(n(60), 0)]);
        g.set_armed(true);
        g.set_held(held(&[60]), 0);
        assert_eq!(g.poll(0), GateState::Running);
        assert!(g.is_complete());
        // Past the end, even with nothing held, never freezes.
        g.set_held(held(&[]), 10000);
        assert_eq!(g.poll(10_000), GateState::Running);
        assert!(g.awaiting().is_none());
    }

    #[test]
    fn holding_a_note_before_it_is_due_does_not_pre_satisfy() {
        // A note held before its step's time (e.g. the key used to START the take,
        // still down through the lead-in) must NOT consume the step early and skip
        // its wait — the pause on that note must still happen once it comes due.
        let mut g = WaitGate::from_expected(&[(n(64), 1000)]);
        g.set_armed(true);
        g.set_held(held(&[64]), 0); // required note held early (before it is due)
        assert_eq!(g.poll(0), GateState::Running); // not yet due
        assert!(
            !g.is_complete(),
            "step must not be consumed before its time"
        );
        // The player releases the start key before the note comes due.
        g.set_held(held(&[]), 500);
        // Now it is due and unsatisfied → the wait DOES freeze (not skipped).
        assert_eq!(g.poll(1000), GateState::Frozen);
        assert_eq!(g.awaiting().unwrap().notes, vec![64]);
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
mod seek_tests {
    use super::*;

    fn n(v: u8) -> MidiNote {
        MidiNote::new(v).unwrap()
    }
    fn held(notes: &[u8]) -> BTreeSet<u8> {
        notes.iter().copied().collect()
    }

    #[test]
    fn tracker_seek_skips_past_steps_keeps_current() {
        let mut t = WaitTracker::from_expected(&[(n(60), 0), (n(62), 1000), (n(64), 2000)]);
        // Seek to a time between steps: the step at 1000 is kept (>= now).
        t.seek_to(1000);
        assert_eq!(t.current().unwrap().notes, vec![62]);
        // Seek past everything → complete.
        t.seek_to(9999);
        assert!(t.is_complete());
    }

    #[test]
    fn tracker_seek_between_steps_lands_on_next() {
        let mut t = WaitTracker::from_expected(&[(n(60), 0), (n(62), 1000)]);
        t.seek_to(500); // past step@0, before step@1000
        assert_eq!(t.current().unwrap().notes, vec![62]);
    }

    /// The regression this fixes: rebuilding the gate mid-take (a fresh tracker
    /// at step 0) while the clock is far along must NOT freeze on the song's
    /// first note. Seeking to the playhead resumes the wait at the current step.
    #[test]
    fn rebuilt_gate_seeked_to_playhead_does_not_freeze_on_the_past() {
        // Song's first left-hand note is at t=0; the current one is at t=5000.
        let mut g = WaitGate::from_expected(&[(n(48), 0), (n(50), 5000)]);
        g.set_armed(true);
        // Clock is at 5000 (mid-take). Without seeking, poll would freeze on the
        // t=0 step forever. Seek to the playhead first:
        g.seek_to(5000);
        // Now it correctly freezes on the DUE current step (t=5000), not the past.
        assert_eq!(g.poll(5000), GateState::Frozen);
        assert_eq!(g.awaiting().unwrap().notes, vec![50]);
        // Playing the current note advances and unfreezes.
        g.set_held(held(&[50]), 5000);
        assert_eq!(g.poll(5000), GateState::Running);
        assert!(g.is_complete());
    }

    #[test]
    fn seek_clears_stale_frozen_flag() {
        let mut g = WaitGate::from_expected(&[(n(60), 0), (n(62), 5000)]);
        g.set_armed(true);
        assert_eq!(g.poll(0), GateState::Frozen); // frozen on step@0
        g.seek_to(5000); // jump the playhead forward
        assert!(g.awaiting().is_none(), "seek clears the stale frozen flag");
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
