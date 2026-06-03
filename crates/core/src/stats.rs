//! Pure performance statistics summary over a scoring engine's ScoreReport.
//!
//! This module provides a `Summary` type that aggregates statistics from a
//! `ScoreReport`, including accuracy, counts, timing breakdown, and mean error.
//! No I/O, pure computation.

use crate::scoring::{NoteJudgment, ScoreReport, Timing};

/// A pure summary over a scoring engine's ScoreReport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Total number of expected notes.
    pub total_expected: usize,
    /// Number of hits (any timing).
    pub hits: usize,
    /// Number of misses.
    pub misses: usize,
    /// Number of extra played notes (not matching any expected note).
    pub extras: usize,
    /// Number of Perfect hits.
    pub perfect: usize,
    /// Number of Early hits.
    pub early: usize,
    /// Number of Late hits.
    pub late: usize,
    /// Sum of absolute timing errors over all hits (in microseconds).
    pub total_abs_error_us: u64,
}

impl Summary {
    /// Build a summary from a ScoreReport.
    pub fn from_report(report: &ScoreReport) -> Self {
        let total_expected = report.judgments.len();
        let hits = report.hits;
        let misses = report.misses;
        let extras = report.extras;
        let mut perfect = 0usize;
        let mut early = 0usize;
        let mut late = 0usize;
        let mut total_abs_error_us = 0u64;

        for judgment in &report.judgments {
            match judgment {
                NoteJudgment::Hit { timing, error_us } => {
                    match timing {
                        Timing::Perfect => perfect += 1,
                        Timing::Early => early += 1,
                        Timing::Late => late += 1,
                    }
                    total_abs_error_us += error_us.unsigned_abs();
                }
                NoteJudgment::Miss => {}
            }
        }

        Self {
            total_expected,
            hits,
            misses,
            extras,
            perfect,
            early,
            late,
            total_abs_error_us,
        }
    }

    /// Accuracy as hits / (hits + misses) in 0.0..=1.0.
    /// Returns 0.0 when there are no expected notes.
    pub fn accuracy(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f32 / total as f32
        }
    }

    /// Mean absolute timing error over hits in microseconds.
    /// Returns 0 if there are no hits.
    pub fn mean_abs_error_us(&self) -> f32 {
        if self.hits == 0 {
            0.0
        } else {
            self.total_abs_error_us as f32 / self.hits as f32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::{score, ExpectedNote, ScoreConfig};
    use crate::{MidiNote, NoteEvent, Velocity};

    fn expect(note: u8, t: u64) -> ExpectedNote {
        ExpectedNote::new(MidiNote::new(note).unwrap(), t)
    }

    fn played_on(note: u8, t: u64) -> NoteEvent {
        NoteEvent::on(MidiNote::new(note).unwrap(), Velocity::new(80).unwrap(), t)
    }

    #[test]
    fn accuracy_all_hits() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        let play = [played_on(60, 1_000_000)];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.accuracy(), 1.0);
    }

    #[test]
    fn accuracy_all_misses() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        let play = [];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.accuracy(), 0.0);
    }

    #[test]
    fn accuracy_half_hits() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000), expect(64, 1_000_000)];
        let play = [played_on(60, 1_000_000)];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.accuracy(), 0.5);
    }

    #[test]
    fn accuracy_no_expected_notes() {
        let cfg = ScoreConfig::default();
        let exp = [];
        let play = [];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.accuracy(), 0.0);
    }

    #[test]
    fn counts() {
        let cfg = ScoreConfig::default();
        let exp = [
            expect(60, 1_000_000),
            expect(64, 1_000_000),
            expect(67, 1_000_000),
        ];
        let play = [played_on(60, 1_000_000), played_on(70, 1_000_000)];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.total_expected, 3);
        assert_eq!(summary.hits, 1);
        assert_eq!(summary.misses, 2);
        assert_eq!(summary.extras, 1);
    }

    #[test]
    fn timing_breakdown() {
        let cfg = ScoreConfig::default();
        // Perfect hit at 1_000_000
        // Default config: perfect_us=50_000, good_us=150_000
        let exp = [
            expect(60, 1_000_000),
            expect(64, 1_000_000),
            expect(67, 1_000_000),
        ];
        let play = [
            played_on(60, 1_000_000), // Perfect (0 error)
            played_on(64, 900_000),   // Early (100ms early, outside perfect, within good)
            played_on(67, 1_100_000), // Late (100ms late, outside perfect, within good)
        ];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.perfect, 1);
        assert_eq!(summary.early, 1);
        assert_eq!(summary.late, 1);
    }

    #[test]
    fn mean_abs_error_us_all_perfect() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000), expect(64, 1_000_000)];
        let play = [played_on(60, 1_000_000), played_on(64, 1_000_000)];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.mean_abs_error_us(), 0.0);
    }

    #[test]
    fn mean_abs_error_us_with_errors() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000), expect(64, 1_000_000)];
        let play = [played_on(60, 950_000), played_on(64, 1_050_000)];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        // Both are 50_000 us off, mean = (50_000 + 50_000) / 2 = 50_000
        assert_eq!(summary.mean_abs_error_us(), 50_000.0);
    }

    #[test]
    fn mean_abs_error_us_no_hits() {
        let cfg = ScoreConfig::default();
        let exp = [expect(60, 1_000_000)];
        let play = [];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.mean_abs_error_us(), 0.0);
    }

    #[test]
    fn empty_report() {
        let cfg = ScoreConfig::default();
        let exp = [];
        let play = [];
        let report = score(&exp, &play, cfg);
        let summary = Summary::from_report(&report);
        assert_eq!(summary.total_expected, 0);
        assert_eq!(summary.hits, 0);
        assert_eq!(summary.misses, 0);
        assert_eq!(summary.extras, 0);
        assert_eq!(summary.perfect, 0);
        assert_eq!(summary.early, 0);
        assert_eq!(summary.late, 0);
        assert_eq!(summary.accuracy(), 0.0);
        assert_eq!(summary.mean_abs_error_us(), 0.0);
    }
}
