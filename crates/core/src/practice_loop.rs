//! Play-mode practice loop: the pure phase machine behind
//! **count-in → demo → count-in → your turn → …** over a range of bars.
//!
//! Clock-agnostic: the session feeds it the play clock's `now` each tick and
//! acts on the returned [`LoopStep`] (enter a phase, or jump the clock). It
//! owns no clock, device or audio — the session seeks, cuts the song bus and
//! plays the count-in clicks it reports via [`PracticeLoop::click_due`].

/// Which pass a count-in leads into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The app plays the target hand; nothing is scored.
    Demo,
    /// The player plays; the pass is scored.
    YourTurn,
}

/// Where the loop is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopPhase {
    /// One bar of clicks before the loop start, leading into `next`.
    CountIn { next: Pass },
    /// `start_us..end_us`, played by the app.
    Demo,
    /// `start_us..end_us`, played (and scored) by the player.
    YourTurn,
}

impl LoopPhase {
    /// Wire name: `"count_in"`, `"demo"` or `"your_turn"`.
    pub fn name(self) -> &'static str {
        match self {
            LoopPhase::CountIn { .. } => "count_in",
            LoopPhase::Demo => "demo",
            LoopPhase::YourTurn => "your_turn",
        }
    }

    fn of_pass(pass: Pass) -> Self {
        match pass {
            Pass::Demo => LoopPhase::Demo,
            Pass::YourTurn => LoopPhase::YourTurn,
        }
    }
}

/// What the session must do after a [`PracticeLoop::tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStep {
    /// Nothing changed.
    Stay,
    /// The phase changed in place (a count-in reached the loop start); the
    /// clock keeps running.
    Enter(LoopPhase),
    /// The pass reached the loop end: seek the clock to `us` and continue in
    /// `phase`. `pass_done` is set when a *your turn* pass just finished (score
    /// and publish it).
    JumpTo {
        us: u64,
        phase: LoopPhase,
        pass_done: bool,
    },
}

/// The practice-loop phase machine over `[start_us, end_us)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PracticeLoop {
    start_us: u64,
    end_us: u64,
    count_in_us: u64,
    beats_per_bar: u8,
    phase: LoopPhase,
    pass: u32,
}

impl PracticeLoop {
    /// A loop over `[start_us, end_us)` with a count-in of `count_in_us`
    /// split into `beats_per_bar` clicks. Starts in the count-in before the
    /// first demo, pass 1. `end_us` is kept strictly after `start_us`.
    pub fn new(start_us: u64, end_us: u64, count_in_us: u64, beats_per_bar: u8) -> Self {
        Self {
            start_us,
            end_us: end_us.max(start_us + 1),
            count_in_us,
            beats_per_bar: beats_per_bar.max(1),
            phase: LoopPhase::CountIn { next: Pass::Demo },
            pass: 1,
        }
    }

    pub fn start_us(&self) -> u64 {
        self.start_us
    }

    pub fn end_us(&self) -> u64 {
        self.end_us
    }

    pub fn count_in_us(&self) -> u64 {
        self.count_in_us
    }

    pub fn beats_per_bar(&self) -> u8 {
        self.beats_per_bar
    }

    pub fn phase(&self) -> LoopPhase {
        self.phase
    }

    /// 1-based pass number: bumped when a *your turn* pass completes.
    pub fn pass(&self) -> u32 {
        self.pass
    }

    /// Where every count-in starts: `start_us - count_in_us`, clamped at 0.
    pub fn entry_us(&self) -> u64 {
        self.start_us.saturating_sub(self.count_in_us)
    }

    /// Advance to `now_us`; returns what the session must do this tick.
    pub fn tick(&mut self, now_us: u64) -> LoopStep {
        match self.phase {
            LoopPhase::CountIn { next } => {
                if now_us >= self.start_us {
                    self.phase = LoopPhase::of_pass(next);
                    LoopStep::Enter(self.phase)
                } else {
                    LoopStep::Stay
                }
            }
            LoopPhase::Demo | LoopPhase::YourTurn => {
                if now_us < self.end_us {
                    return LoopStep::Stay;
                }
                let pass_done = self.phase == LoopPhase::YourTurn;
                let next = if pass_done {
                    self.pass += 1;
                    Pass::Demo
                } else {
                    Pass::YourTurn
                };
                self.phase = LoopPhase::CountIn { next };
                LoopStep::JumpTo {
                    us: self.entry_us(),
                    phase: self.phase,
                    pass_done,
                }
            }
        }
    }

    /// Length of one count-in beat (µs).
    fn beat_us(&self) -> u64 {
        (self.count_in_us / self.beats_per_bar as u64).max(1)
    }

    /// `Some(accent)` when a count-in beat falls in `[prev_us, now_us)`; the
    /// accent is beat 1 of the count-in. Beats sit on `start_us - k·beat` for
    /// `k = beats..1`; any that would fall before 0 (a clamped count-in) are
    /// silent. Half-open so a tick starting right on a jump target (the first
    /// beat) clicks, and the loop start itself (a downbeat of the pass) never
    /// does. With several beats in one window the accent wins.
    pub fn click_due(&self, prev_us: u64, now_us: u64) -> Option<bool> {
        if now_us <= prev_us {
            return None;
        }
        let beat = self.beat_us();
        let beats = self.beats_per_bar as u64;
        let mut due = None;
        for k in 0..beats {
            let back = (beats - k) * beat;
            let Some(t) = self.start_us.checked_sub(back) else {
                continue;
            };
            if prev_us <= t && t < now_us {
                let accent = k == 0;
                due = Some(due.unwrap_or(false) || accent);
            }
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loop 10_000..18_000, 4-beat count-in of 4_000 (entry 6_000).
    fn lp() -> PracticeLoop {
        PracticeLoop::new(10_000, 18_000, 4_000, 4)
    }

    #[test]
    fn full_cycle_with_exact_boundaries() {
        let mut l = lp();
        assert_eq!(l.phase(), LoopPhase::CountIn { next: Pass::Demo });
        assert_eq!(l.pass(), 1);
        assert_eq!(l.entry_us(), 6_000);
        assert_eq!(l.tick(6_000), LoopStep::Stay);
        assert_eq!(l.tick(9_999), LoopStep::Stay);
        assert_eq!(l.tick(10_000), LoopStep::Enter(LoopPhase::Demo));
        assert_eq!(l.tick(17_999), LoopStep::Stay);
        assert_eq!(
            l.tick(18_000),
            LoopStep::JumpTo {
                us: 6_000,
                phase: LoopPhase::CountIn {
                    next: Pass::YourTurn
                },
                pass_done: false,
            }
        );
        assert_eq!(l.pass(), 1);
        assert_eq!(l.tick(9_999), LoopStep::Stay);
        assert_eq!(l.tick(10_000), LoopStep::Enter(LoopPhase::YourTurn));
        assert_eq!(l.tick(17_999), LoopStep::Stay);
        assert_eq!(
            l.tick(18_000),
            LoopStep::JumpTo {
                us: 6_000,
                phase: LoopPhase::CountIn { next: Pass::Demo },
                pass_done: true,
            }
        );
        assert_eq!(l.pass(), 2);
        assert_eq!(l.tick(10_000), LoopStep::Enter(LoopPhase::Demo));
    }

    #[test]
    fn entry_clamps_at_zero() {
        let l = PracticeLoop::new(1_000, 5_000, 4_000, 4);
        assert_eq!(l.entry_us(), 0);
    }

    #[test]
    fn overshooting_the_end_still_jumps() {
        let mut l = lp();
        l.tick(10_000);
        assert!(matches!(
            l.tick(50_000),
            LoopStep::JumpTo {
                us: 6_000,
                pass_done: false,
                ..
            }
        ));
    }

    #[test]
    fn clicks_once_per_beat_with_accent_on_beat_one() {
        let l = lp();
        // Tick in 250 µs steps across the count-in and into the loop.
        let mut clicks = Vec::new();
        let mut prev = l.entry_us();
        while prev < 12_000 {
            let now = prev + 250;
            if let Some(accent) = l.click_due(prev, now) {
                clicks.push((prev, accent));
            }
            prev = now;
        }
        assert_eq!(
            clicks,
            vec![
                (6_000, true),
                (7_000, false),
                (8_000, false),
                (9_000, false)
            ]
        );
    }

    #[test]
    fn clamped_count_in_drops_beats_before_zero() {
        let l = PracticeLoop::new(2_500, 6_000, 4_000, 4);
        let beats: Vec<_> = (0..2_500u64)
            .filter_map(|t| l.click_due(t, t + 1).map(|a| (t, a)))
            .collect();
        assert_eq!(beats, vec![(500, false), (1_500, false)]);
    }

    #[test]
    fn phase_names() {
        assert_eq!(LoopPhase::CountIn { next: Pass::Demo }.name(), "count_in");
        assert_eq!(LoopPhase::Demo.name(), "demo");
        assert_eq!(LoopPhase::YourTurn.name(), "your_turn");
    }
}
