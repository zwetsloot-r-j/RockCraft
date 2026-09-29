//! Keeping a free-running audio stream in step with the play clock.
//!
//! A backing recording plays as one continuous stream at the audio device's
//! pace, while the highway, the synth and scoring follow the play clock. The two
//! agree when the stream starts, then can part: every output underrun (the
//! device starved for a moment) delays the stream for good, and pausing it for
//! a wait-mode freeze stops and restarts it a few milliseconds off. Over a long
//! piece that adds up to an audible lag.
//!
//! [`DriftGuard`] watches the stream's reported position against where the
//! clock says it should be and asks for a re-seek once they part by more than
//! [`DRIFT_TOLERANCE_US`]. Pure: the frontend feeds it positions and performs
//! the seek.
//!
//! The reported position runs ahead of what is audible by the output buffer, a
//! constant. So the guard doesn't compare against zero: once the stream has
//! settled it takes the offset it sees as the *baseline*, and corrects drift
//! away from that.

/// How far the stream may drift from the clock before it is re-seeked. Below
/// ~40 ms a lag is hard to hear; a re-seek is a small skip in the audio, so it
/// shouldn't fire for jitter.
pub const DRIFT_TOLERANCE_US: u64 = 40_000;

/// After the stream starts or is re-seeked, how long (play-clock µs) to let
/// the output buffer fill before trusting its position.
pub const SETTLE_US: u64 = 300_000;

/// How long (play-clock µs) the drift must stay past the tolerance before
/// acting, so a single late position update never triggers a seek.
pub const PERSIST_US: u64 = 150_000;

/// Watches one playing stream's position against the play clock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriftGuard {
    /// Clock time the stream (re)started or was last re-seeked.
    since_us: Option<u64>,
    /// `position - target` once settled: the output buffer's lead.
    baseline_us: Option<i64>,
    /// Clock time the drift first went past the tolerance, while it stays past.
    over_since_us: Option<u64>,
}

impl DriftGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new stream started at clock `now_us` (or the old one was seeked to a
    /// new place by the frontend): settle again and take a fresh baseline.
    pub fn restart(&mut self, now_us: u64) {
        *self = Self {
            since_us: Some(now_us),
            ..Self::default()
        };
    }

    /// The settled baseline — how far the reported position leads the clock
    /// (µs) — or `None` until the stream has settled.
    pub fn baseline_us(&self) -> Option<i64> {
        self.baseline_us
    }

    /// Check the stream at clock `now_us`: it reports `position_us`, and should
    /// be at `target_us`. Call only while both the clock and the stream run.
    /// Returns `Some(target_us)` when the stream must be seeked there.
    pub fn check(&mut self, now_us: u64, target_us: u64, position_us: u64) -> Option<u64> {
        let since = *self.since_us.get_or_insert(now_us);
        if now_us < since + SETTLE_US {
            return None;
        }
        let offset = position_us as i64 - target_us as i64;
        let baseline = *self.baseline_us.get_or_insert(offset);
        if offset.abs_diff(baseline) <= DRIFT_TOLERANCE_US {
            self.over_since_us = None;
            return None;
        }
        let over_since = *self.over_since_us.get_or_insert(now_us);
        if now_us < over_since + PERSIST_US {
            return None;
        }
        // Re-seek, keeping the baseline: the buffer's lead doesn't change.
        self.since_us = Some(now_us);
        self.over_since_us = None;
        Some(target_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check every 10 ms from `from` to `to` (clock µs) with the target equal to
    /// the clock and the position from `pos`. Returns the first `(now, seek)`.
    fn run(g: &mut DriftGuard, from: u64, to: u64, pos: impl Fn(u64) -> u64) -> Option<(u64, u64)> {
        let mut now = from;
        while now <= to {
            if let Some(seek) = g.check(now, now, pos(now)) {
                return Some((now, seek));
            }
            now += 10_000;
        }
        None
    }

    #[test]
    fn a_stream_in_step_is_left_alone() {
        let mut g = DriftGuard::new();
        g.restart(0);
        // A 20 ms buffer lead, with ±5 ms of update jitter.
        let pos = |t: u64| t + 20_000 + (t / 10_000 % 3) * 5_000;
        assert_eq!(run(&mut g, 0, 60_000_000, pos), None);
        assert!(g.baseline_us().is_some());
    }

    #[test]
    fn nothing_is_judged_while_the_stream_settles() {
        let mut g = DriftGuard::new();
        g.restart(1_000_000);
        // Wildly off, but only during the settle window.
        assert_eq!(g.check(1_000_000, 1_000_000, 0), None);
        assert_eq!(g.check(1_290_000, 1_290_000, 0), None);
        assert_eq!(g.baseline_us(), None);
    }

    #[test]
    fn a_stream_that_falls_behind_is_seeked_back_after_it_persists() {
        let mut g = DriftGuard::new();
        g.restart(0);
        // In step (20 ms lead) until 10 s, then an underrun costs 60 ms.
        let pos = |t: u64| {
            if t < 10_000_000 {
                t + 20_000
            } else {
                t - 40_000
            }
        };
        let (at, seek) = run(&mut g, 0, 20_000_000, pos).expect("seeks");
        assert_eq!(seek, at, "to where the clock says");
        assert!(
            (10_000_000 + PERSIST_US..10_000_000 + PERSIST_US + 20_000).contains(&at),
            "after the drift persisted: {at}"
        );
    }

    #[test]
    fn a_stream_that_runs_ahead_is_seeked_back_too() {
        let mut g = DriftGuard::new();
        g.restart(0);
        let pos = |t: u64| if t < 5_000_000 { t } else { t + 50_000 };
        assert!(run(&mut g, 0, 10_000_000, pos).is_some());
    }

    #[test]
    fn a_single_late_update_does_not_seek() {
        let mut g = DriftGuard::new();
        g.restart(0);
        let pos = |t: u64| if t == 5_000_000 { t - 100_000 } else { t };
        assert_eq!(run(&mut g, 0, 10_000_000, pos), None);
    }

    #[test]
    fn drift_within_the_tolerance_is_ignored() {
        let mut g = DriftGuard::new();
        g.restart(0);
        let pos = |t: u64| if t < 5_000_000 { t } else { t - 35_000 };
        assert_eq!(run(&mut g, 0, 10_000_000, pos), None);
    }

    #[test]
    fn after_a_seek_it_settles_and_keeps_the_baseline() {
        let mut g = DriftGuard::new();
        g.restart(0);
        let behind = |t: u64| {
            if t < 1_000_000 {
                t + 20_000
            } else {
                t - 60_000
            }
        };
        let (at, _) = run(&mut g, 0, 5_000_000, behind).expect("seeks");
        let baseline = g.baseline_us();
        // Just after the seek the position is still off (old audio in the
        // buffer): no second seek while it settles.
        assert_eq!(g.check(at + 10_000, at + 10_000, at - 60_000), None);
        // Back in step with the original lead: left alone, same baseline.
        let fixed = |t: u64| t + 20_000;
        assert_eq!(run(&mut g, at + 20_000, at + 5_000_000, fixed), None);
        assert_eq!(g.baseline_us(), baseline);
    }

    #[test]
    fn restart_forgets_the_baseline() {
        let mut g = DriftGuard::new();
        g.restart(0);
        g.check(400_000, 400_000, 420_000);
        assert_eq!(g.baseline_us(), Some(20_000));
        g.restart(1_000_000);
        assert_eq!(g.baseline_us(), None);
    }
}
