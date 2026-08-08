//! Pure scoring engine: compare a song's expected notes against the player's
//! actual note-ons and judge each for pitch + timing. No I/O, no device.
//!
//! IMPLEMENTATION NOTE (seeded task): the `#[cfg(test)]` module at the bottom is
//! the contract — it is pre-committed and must pass UNMODIFIED. Implement the
//! public API it exercises (types, fields, methods) so the tests compile and
//! pass. The stubs below fix the API surface; replace the `todo!()` bodies.

use crate::{MidiNote, NoteEvent, NoteEventKind};

/// A note the song expects the player to strike at a given moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedNote {
    pub note: MidiNote,
    pub time_us: u64,
}

impl ExpectedNote {
    pub const fn new(note: MidiNote, time_us: u64) -> Self {
        Self { note, time_us }
    }
}

/// Timing classification of a hit relative to the expected time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// Within ±`perfect_us` of the expected time.
    Perfect,
    /// Played before the expected time (outside perfect, within good).
    Early,
    /// Played after the expected time (outside perfect, within good).
    Late,
}

/// The outcome for one expected note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteJudgment {
    /// Matched by a played note; carries timing class and the signed error in
    /// microseconds (negative = played early, positive = played late).
    Hit { timing: Timing, error_us: i64 },
    /// No played note of this pitch fell within ±`good_us`.
    Miss,
}

/// How strongly a play screen should react to one judged note.
///
/// Every frontend turns a judged note into a one-shot effect at that note's
/// lane, and how *loud* that effect is must follow the judgment. The mapping is
/// pure, so it lives here rather than being re-derived (and drifting) per
/// frontend:
///
/// - [`Timing::Perfect`] → [`Feedback::Clear`] — a well-timed hit reads
///   unmistakably.
/// - [`Timing::Early`] / [`Timing::Late`] → [`Feedback::Near`] — it counted, but
///   the effect is visibly lesser so the player feels the difference.
/// - [`NoteJudgment::Miss`] → [`Feedback::Subtle`] — acknowledged quietly; a
///   missed note should not shout over the notes still falling.
///
/// The ordering (`Subtle < Near < Clear`) is the intensity ordering, so a
/// frontend can compare levels rather than matching every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feedback {
    /// Barely-there acknowledgement (a miss).
    Subtle,
    /// Lesser effect — a hit that was off-time.
    Near,
    /// Full effect — a well-timed hit.
    Clear,
}

impl Feedback {
    /// The feedback level a judgment earns.
    pub const fn from_judgment(judgment: NoteJudgment) -> Self {
        match judgment {
            NoteJudgment::Hit {
                timing: Timing::Perfect,
                ..
            } => Self::Clear,
            NoteJudgment::Hit { .. } => Self::Near,
            NoteJudgment::Miss => Self::Subtle,
        }
    }

    /// Stable wire name, for frontends that receive the level over IPC.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Subtle => "subtle",
            Self::Near => "near",
            Self::Clear => "clear",
        }
    }
}

/// Tunable timing windows (microseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreConfig {
    /// Half-width of the "perfect" window.
    pub perfect_us: u64,
    /// Half-width of the "good" (hit-at-all) window. Must be >= perfect_us.
    pub good_us: u64,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        // 50 ms perfect, 150 ms to count as a hit — comfortable for learning.
        Self {
            perfect_us: 50_000,
            good_us: 150_000,
        }
    }
}

/// The result of scoring a performance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreReport {
    /// One judgment per expected note, in the order given.
    pub judgments: Vec<NoteJudgment>,
    /// Count of expected notes that were hit (any timing).
    pub hits: usize,
    /// Count of expected notes with no match.
    pub misses: usize,
    /// Played note-ons that matched no expected note.
    pub extras: usize,
}

/// Score a performance: judge every `expected` note against the `played`
/// note-ons, consuming each played note at most once (nearest-in-time match
/// within the good window wins).
pub fn score(expected: &[ExpectedNote], played: &[NoteEvent], config: ScoreConfig) -> ScoreReport {
    // Collect the real note-ons (a note-on with velocity 0 is a note-off by
    // MIDI convention, and explicit note-offs never count as a strike). Track
    // which ones get consumed so each played note matches at most one expected
    // note and the leftovers can be reported as extras.
    let candidates: Vec<&NoteEvent> = played
        .iter()
        .filter(|e| match e.kind {
            NoteEventKind::On { velocity } => !velocity.is_note_off(),
            NoteEventKind::Off => false,
        })
        .collect();
    let mut consumed = vec![false; candidates.len()];

    let mut judgments = Vec::with_capacity(expected.len());
    let mut hits = 0usize;
    let mut misses = 0usize;

    for exp in expected {
        // Among unconsumed note-ons of the same pitch within the good window,
        // prefer the nearest in time.
        let mut best: Option<(usize, u64)> = None; // (index, abs error)
        for (i, ev) in candidates.iter().enumerate() {
            if consumed[i] || ev.note != exp.note {
                continue;
            }
            let abs_err = ev.timestamp_us.abs_diff(exp.time_us);
            if abs_err > config.good_us {
                continue;
            }
            match best {
                Some((_, best_err)) if abs_err >= best_err => {}
                _ => best = Some((i, abs_err)),
            }
        }

        match best {
            Some((i, abs_err)) => {
                consumed[i] = true;
                let error_us = candidates[i].timestamp_us as i64 - exp.time_us as i64;
                let timing = if abs_err <= config.perfect_us {
                    Timing::Perfect
                } else if error_us < 0 {
                    Timing::Early
                } else {
                    Timing::Late
                };
                judgments.push(NoteJudgment::Hit { timing, error_us });
                hits += 1;
            }
            None => {
                judgments.push(NoteJudgment::Miss);
                misses += 1;
            }
        }
    }

    let extras = consumed.iter().filter(|&&c| !c).count();

    ScoreReport {
        judgments,
        hits,
        misses,
        extras,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Velocity;

    fn expect(note: u8, t: u64) -> ExpectedNote {
        ExpectedNote::new(MidiNote::new(note).unwrap(), t)
    }
    fn played_on(note: u8, t: u64) -> NoteEvent {
        NoteEvent::on(MidiNote::new(note).unwrap(), Velocity::new(80).unwrap(), t)
    }
    fn played_off(note: u8, t: u64) -> NoteEvent {
        NoteEvent::off(MidiNote::new(note).unwrap(), t)
    }

    #[test]
    fn perfect_hit_dead_on_time() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        let play = [played_on(60, 1_000_000)];
        let r = score(&exp, &play, cfg);
        assert_eq!(r.judgments.len(), 1);
        assert_eq!(
            r.judgments[0],
            NoteJudgment::Hit {
                timing: Timing::Perfect,
                error_us: 0
            }
        );
        assert_eq!(r.hits, 1);
        assert_eq!(r.misses, 0);
        assert_eq!(r.extras, 0);
    }

    #[test]
    fn early_and_late_within_good_window() {
        let cfg = ScoreConfig::default(); // perfect 50ms, good 150ms
        let exp = [expect(60, 1_000_000)];

        // 100ms early -> Early, error negative
        let early = score(&exp, &[played_on(60, 900_000)], cfg);
        assert_eq!(
            early.judgments[0],
            NoteJudgment::Hit {
                timing: Timing::Early,
                error_us: -100_000
            }
        );

        // 100ms late -> Late, error positive
        let late = score(&exp, &[played_on(60, 1_100_000)], cfg);
        assert_eq!(
            late.judgments[0],
            NoteJudgment::Hit {
                timing: Timing::Late,
                error_us: 100_000
            }
        );
    }

    #[test]
    fn outside_good_window_is_miss() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        // 200ms late, beyond the 150ms good window
        let r = score(&exp, &[played_on(60, 1_200_000)], cfg);
        assert_eq!(r.judgments[0], NoteJudgment::Miss);
        assert_eq!(r.misses, 1);
        // the stray played note matched nothing -> extra
        assert_eq!(r.extras, 1);
    }

    #[test]
    fn wrong_pitch_does_not_match() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        let r = score(&exp, &[played_on(61, 1_000_000)], cfg);
        assert_eq!(r.judgments[0], NoteJudgment::Miss);
        assert_eq!(r.extras, 1);
    }

    #[test]
    fn note_offs_are_ignored() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        // only a note-off at the right time -> no hit
        let r = score(&exp, &[played_off(60, 1_000_000)], cfg);
        assert_eq!(r.judgments[0], NoteJudgment::Miss);
    }

    #[test]
    fn each_played_note_matches_at_most_one_expected() {
        let cfg = ScoreConfig::default();
        // two expected of the same pitch close together, but only one played
        let exp = [expect(60, 1_000_000), expect(60, 1_040_000)];
        let play = [played_on(60, 1_010_000)];
        let r = score(&exp, &play, cfg);
        // exactly one of the two should be a hit, the other a miss
        let hits = r
            .judgments
            .iter()
            .filter(|j| matches!(j, NoteJudgment::Hit { .. }))
            .count();
        assert_eq!(hits, 1);
        assert_eq!(r.hits, 1);
        assert_eq!(r.misses, 1);
        assert_eq!(r.extras, 0);
    }

    #[test]
    fn nearest_in_time_match_preferred() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        // two candidate plays in window; the nearer (dead-on) should be chosen
        let play = [played_on(60, 1_120_000), played_on(60, 1_000_000)];
        let r = score(&exp, &play, cfg);
        assert_eq!(
            r.judgments[0],
            NoteJudgment::Hit {
                timing: Timing::Perfect,
                error_us: 0
            }
        );
        // the other played note is an extra
        assert_eq!(r.extras, 1);
    }

    #[test]
    fn chord_all_hit() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 0), expect(64, 0), expect(67, 0)];
        let play = [
            played_on(67, 10_000),
            played_on(60, 0),
            played_on(64, 20_000),
        ];
        let r = score(&exp, &play, cfg);
        assert_eq!(r.hits, 3);
        assert_eq!(r.misses, 0);
        assert_eq!(r.extras, 0);
        assert!(r
            .judgments
            .iter()
            .all(|j| matches!(j, NoteJudgment::Hit { .. })));
    }

    #[test]
    fn empty_inputs() {
        let cfg = ScoreConfig::default();
        let r = score(&[], &[], cfg);
        assert_eq!(r.judgments.len(), 0);
        assert_eq!(r.hits, 0);
        assert_eq!(r.misses, 0);
        assert_eq!(r.extras, 0);
    }
}

/// Feedback-level mapping (M14-B). Kept in its own module so the seeded
/// `tests` module above stays exactly as committed.
#[cfg(test)]
mod feedback_tests {
    use super::*;

    fn hit(timing: Timing, error_us: i64) -> NoteJudgment {
        NoteJudgment::Hit { timing, error_us }
    }

    #[test]
    fn perfect_reads_clear() {
        assert_eq!(
            Feedback::from_judgment(hit(Timing::Perfect, 0)),
            Feedback::Clear
        );
    }

    #[test]
    fn off_time_hits_read_near_regardless_of_direction() {
        assert_eq!(
            Feedback::from_judgment(hit(Timing::Early, -100_000)),
            Feedback::Near
        );
        assert_eq!(
            Feedback::from_judgment(hit(Timing::Late, 100_000)),
            Feedback::Near
        );
    }

    #[test]
    fn miss_reads_subtle() {
        assert_eq!(
            Feedback::from_judgment(NoteJudgment::Miss),
            Feedback::Subtle
        );
    }

    #[test]
    fn levels_order_by_intensity() {
        assert!(Feedback::Clear > Feedback::Near);
        assert!(Feedback::Near > Feedback::Subtle);
    }

    #[test]
    fn wire_names_are_distinct_and_stable() {
        assert_eq!(Feedback::Clear.as_str(), "clear");
        assert_eq!(Feedback::Near.as_str(), "near");
        assert_eq!(Feedback::Subtle.as_str(), "subtle");
    }

    /// Every judgment a `score` run can produce maps to a level — the whole
    /// point is that a frontend never has to invent one.
    #[test]
    fn every_scored_judgment_has_a_level() {
        use crate::{MidiNote, Velocity};
        let cfg = ScoreConfig::default();
        let expected = [
            ExpectedNote::new(MidiNote::new(60).unwrap(), 1_000_000),
            ExpectedNote::new(MidiNote::new(62).unwrap(), 2_000_000),
            ExpectedNote::new(MidiNote::new(64).unwrap(), 3_000_000),
        ];
        let v = Velocity::new(80).unwrap();
        let played = [
            // dead on, 100 ms late, and never played
            NoteEvent::on(MidiNote::new(60).unwrap(), v, 1_000_000),
            NoteEvent::on(MidiNote::new(62).unwrap(), v, 2_100_000),
        ];
        let levels: Vec<Feedback> = score(&expected, &played, cfg)
            .judgments
            .into_iter()
            .map(Feedback::from_judgment)
            .collect();
        assert_eq!(
            levels,
            vec![Feedback::Clear, Feedback::Near, Feedback::Subtle]
        );
    }
}
