//! Live scoring during a take: judge notes as their windows close, without
//! re-scoring everything played so far.
//!
//! A play screen judges a note once its good window has closed, and shows a
//! running score and combo. Re-running [`score`](crate::score) over the whole
//! closed prefix each time does the same work again and again: its cost grows
//! with the song position and the number of strikes, so long takes (and wait
//! mode, whose retries add strikes) slow down towards the end.
//!
//! [`LiveScorer`] gives the same judgments incrementally. `score` walks the
//! expected notes in time order and lets each consume the nearest unconsumed
//! strike in its window. Earlier notes never look at a later note's choice, so
//! judging new notes *after* every note judged so far, with the strikes earlier
//! notes consumed, gives exactly what a full re-score would. A strike too early
//! to reach the newest note can never reach a later one, so it is dropped, and
//! the open strike list stays small.
//!
//! The caller keeps one invariant: by the time a note is judged, every strike
//! that could fall inside its window must already be in `played` (judge a note
//! only once the clock is strictly past `time + good_us`). Anything that breaks
//! "appending" (a seek back, a new loop pass, a changed set of scored notes)
//! goes through [`LiveScorer::reset`] and a re-feed of the notes in order.

use crate::scoring::{hit, is_strike};
use crate::{ExpectedNote, NoteEvent, NoteJudgment, ScoreConfig, Timing};

/// Points for one hit: a perfect hit is worth more, and the running combo adds
/// a bonus to each.
pub const fn award(perfect: bool, combo: u32) -> u64 {
    if perfect {
        100 + combo as u64 * 2
    } else {
        50 + combo as u64
    }
}

/// The running figures of a take, folded from its judgments in time order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveTally {
    pub score: u64,
    pub combo: u32,
    pub best_combo: u32,
    pub hits: u32,
    pub misses: u32,
}

impl LiveTally {
    /// Fold in the next judgment.
    pub fn record(&mut self, judgment: NoteJudgment) {
        match judgment {
            NoteJudgment::Hit { timing, .. } => {
                self.score += award(matches!(timing, Timing::Perfect), self.combo);
                self.combo += 1;
                self.best_combo = self.best_combo.max(self.combo);
                self.hits += 1;
            }
            NoteJudgment::Miss => {
                self.combo = 0;
                self.misses += 1;
            }
        }
    }
}

/// Where a note sorts among the notes of a take: by time, then by the caller's
/// id for notes at the same time (the order `score` sees them in).
pub type NoteKey = (u64, usize);

/// Incremental scoring with the same judgments as [`score`](crate::score).
#[derive(Debug, Clone)]
pub struct LiveScorer {
    config: ScoreConfig,
    /// How many entries of `played` have been looked at.
    absorbed: usize,
    /// Unconsumed strikes (indices into `played`, ascending) that may still
    /// match the next note.
    open: Vec<usize>,
    /// The last note judged.
    last: Option<NoteKey>,
    tally: LiveTally,
}

impl LiveScorer {
    pub fn new(config: ScoreConfig) -> Self {
        Self {
            config,
            absorbed: 0,
            open: Vec::new(),
            last: None,
            tally: LiveTally::default(),
        }
    }

    /// Forget every judgment, to score again from the first note.
    pub fn reset(&mut self) {
        *self = Self::new(self.config);
    }

    /// The running figures.
    pub fn tally(&self) -> LiveTally {
        self.tally
    }

    /// Can a note at `key` be judged next? Only if it sorts after every note
    /// judged so far; otherwise the caller must [`reset`](Self::reset) and
    /// re-feed all its closed notes in order.
    pub fn can_follow(&self, key: NoteKey) -> bool {
        self.last.is_none_or(|last| key > last)
    }

    /// Judge `notes` (sorted by key, each one [following](Self::can_follow)
    /// the last) against `played`, which may only have grown at its end since
    /// the previous call. Returns one judgment per note, in order.
    pub fn judge(
        &mut self,
        notes: &[(usize, ExpectedNote)],
        played: &[NoteEvent],
    ) -> Vec<NoteJudgment> {
        debug_assert!(played.len() >= self.absorbed, "played may only grow");
        let start = self.absorbed.min(played.len());
        self.open
            .extend((start..played.len()).filter(|&i| is_strike(&played[i])));
        self.absorbed = played.len();

        let good_us = self.config.good_us;
        let mut out = Vec::with_capacity(notes.len());
        for &(id, exp) in notes {
            let key = (exp.time_us, id);
            debug_assert!(self.can_follow(key), "notes must arrive in order");
            self.last = Some(key);
            // Too early for this note means too early for every later one.
            self.open
                .retain(|&i| played[i].timestamp_us.saturating_add(good_us) >= exp.time_us);
            // The nearest same-pitch strike in the window; on a tie the first
            // played, as `score` picks.
            let mut best: Option<(usize, u64)> = None;
            for (pos, &i) in self.open.iter().enumerate() {
                let ev = &played[i];
                if ev.note != exp.note {
                    continue;
                }
                let err = ev.timestamp_us.abs_diff(exp.time_us);
                if err > good_us {
                    continue;
                }
                match best {
                    Some((_, best_err)) if err >= best_err => {}
                    _ => best = Some((pos, err)),
                }
            }
            let judgment = match best {
                Some((pos, _)) => {
                    let i = self.open.remove(pos);
                    hit(&exp, &played[i], self.config)
                }
                None => NoteJudgment::Miss,
            };
            self.tally.record(judgment);
            out.push(judgment);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{score, MidiNote, Velocity};

    /// A tiny deterministic generator, so the cross-check needs no deps.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, below: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % below
        }
    }

    fn note(n: u64) -> MidiNote {
        MidiNote::new(60 + n as u8).unwrap()
    }

    /// Expected notes on a coarse grid (so chords and repeats happen) and
    /// strikes around them: hits, near misses, wrong notes, note-offs.
    fn take(rng: &mut Lcg, len: usize) -> (Vec<(usize, ExpectedNote)>, Vec<NoteEvent>) {
        let mut notes: Vec<(usize, ExpectedNote)> = (0..len)
            .map(|id| {
                let t = rng.next(len as u64 * 2) * 100_000;
                (id, ExpectedNote::new(note(rng.next(4)), t))
            })
            .collect();
        notes.sort_by_key(|&(id, e)| (e.time_us, id));
        let mut played = Vec::new();
        for &(_, e) in &notes {
            for _ in 0..rng.next(3) {
                let t = (e.time_us + rng.next(400_000)).saturating_sub(200_000);
                let pitch = if rng.next(4) == 0 {
                    note(rng.next(4))
                } else {
                    e.note
                };
                played.push(match rng.next(8) {
                    0 => NoteEvent::off(pitch, t),
                    1 => NoteEvent::on(pitch, Velocity::new(0).unwrap(), t),
                    _ => NoteEvent::on(pitch, Velocity::new(80).unwrap(), t),
                });
            }
        }
        played.sort_by_key(|e| e.timestamp_us);
        (notes, played)
    }

    fn expected(notes: &[(usize, ExpectedNote)]) -> Vec<ExpectedNote> {
        notes.iter().map(|&(_, e)| e).collect()
    }

    #[test]
    fn matches_a_full_score_all_at_once() {
        let cfg = ScoreConfig::default();
        let mut rng = Lcg(7);
        for _ in 0..200 {
            let (notes, mut played) = take(&mut rng, 30);
            // Order of strikes must not matter when all are known up front.
            if rng.next(2) == 0 {
                played.reverse();
            }
            let mut live = LiveScorer::new(cfg);
            let got = live.judge(&notes, &played);
            assert_eq!(got, score(&expected(&notes), &played, cfg).judgments);
        }
    }

    /// The real loop: the clock moves on, strikes arrive as they happen, and
    /// a note is judged once the clock is strictly past its window.
    #[test]
    fn matches_a_full_score_as_the_clock_moves() {
        let cfg = ScoreConfig::default();
        let mut rng = Lcg(42);
        for _ in 0..200 {
            let (notes, played) = take(&mut rng, 40);
            let mut live = LiveScorer::new(cfg);
            let mut seen = 0;
            let mut judged = 0;
            let mut got = Vec::new();
            let mut now = 0;
            while judged < notes.len() {
                now += rng.next(300_000);
                while seen < played.len() && played[seen].timestamp_us <= now {
                    seen += 1;
                }
                let closed = notes[judged..]
                    .iter()
                    .take_while(|(_, e)| e.time_us + cfg.good_us < now)
                    .count();
                got.extend(live.judge(&notes[judged..judged + closed], &played[..seen]));
                judged += closed;
            }
            let full = score(&expected(&notes), &played, cfg);
            assert_eq!(got, full.judgments);
            let mut tally = LiveTally::default();
            full.judgments.iter().for_each(|&j| tally.record(j));
            assert_eq!(live.tally(), tally);
        }
    }

    #[test]
    fn only_later_notes_can_follow() {
        let cfg = ScoreConfig::default();
        let mut live = LiveScorer::new(cfg);
        assert!(live.can_follow((0, 0)));
        live.judge(&[(3, ExpectedNote::new(note(0), 500))], &[]);
        assert!(!live.can_follow((500, 3)));
        assert!(!live.can_follow((400, 9)));
        assert!(live.can_follow((500, 4)));
        live.reset();
        assert!(live.can_follow((0, 0)));
        assert_eq!(live.tally(), LiveTally::default());
    }

    #[test]
    fn tally_scores_combo_like_the_play_screens() {
        let mut t = LiveTally::default();
        let perfect = NoteJudgment::Hit {
            timing: Timing::Perfect,
            error_us: 0,
        };
        let late = NoteJudgment::Hit {
            timing: Timing::Late,
            error_us: 90_000,
        };
        t.record(perfect); // 100
        t.record(late); // 50 + 1
        t.record(NoteJudgment::Miss);
        t.record(perfect); // 100
        assert_eq!(t.score, 251);
        assert_eq!((t.combo, t.best_combo, t.hits, t.misses), (1, 2, 3, 1));
    }
}
