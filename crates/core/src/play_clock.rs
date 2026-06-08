//! A pausable, monotonic song-time clock the frontend advances by injected
//! deltas. Time accrues only while running; pausing freezes `now_us` until
//! resumed. There is no `Instant` anywhere — this honours the architecture
//! invariant "decouple rendering from timing": scoring/sync run off this
//! injected clock, never off frame rate.

/// A monotonic song-time clock, advanced by injected `dt_us` deltas. Pausing
/// freezes `now_us` until resumed; seeking jumps without changing run state.
#[derive(Debug, Clone)]
pub struct PlayClock {
    now_us: u64,
    running: bool,
}

impl PlayClock {
    /// A fresh clock at time 0, running.
    pub fn new() -> Self {
        Self {
            now_us: 0,
            running: true,
        }
    }

    /// Current song time in microseconds.
    pub fn now_us(&self) -> u64 {
        self.now_us
    }

    /// Is the clock currently advancing on `advance`?
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Freeze the clock. Idempotent.
    pub fn pause(&mut self) {
        self.running = false;
    }

    /// Unfreeze the clock. Idempotent.
    pub fn resume(&mut self) {
        self.running = true;
    }

    /// Set the paused state directly (`true` = paused, `false` = running).
    pub fn set_paused(&mut self, paused: bool) {
        self.running = !paused;
    }

    /// Advance by `dt_us` **only when running**; a no-op while paused.
    pub fn advance(&mut self, dt_us: u64) {
        if self.running {
            self.now_us = self.now_us.saturating_add(dt_us);
        }
    }

    /// Jump to an absolute position (seek); leaves running/paused unchanged.
    pub fn seek_us(&mut self, us: u64) {
        self.now_us = us;
    }

    /// Reset to time 0 and running.
    pub fn reset(&mut self) {
        self.now_us = 0;
        self.running = true;
    }
}

impl Default for PlayClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_at_zero_running() {
        let c = PlayClock::new();
        assert_eq!(c.now_us(), 0);
        assert!(c.is_running());
    }

    #[test]
    fn advance_accrues_while_running() {
        let mut c = PlayClock::new();
        c.advance(500);
        c.advance(500);
        assert_eq!(c.now_us(), 1000);
    }

    #[test]
    fn advance_is_noop_while_paused_and_resumes_from_frozen_value() {
        let mut c = PlayClock::new();
        c.advance(1000);
        assert_eq!(c.now_us(), 1000);
        c.pause();
        assert!(!c.is_running());
        c.advance(500); // no-op while paused
        assert_eq!(c.now_us(), 1000);
        c.resume();
        c.advance(500); // resumes from the frozen value
        assert_eq!(c.now_us(), 1500);
    }

    #[test]
    fn pause_and_resume_are_idempotent() {
        let mut c = PlayClock::new();
        c.pause();
        c.pause();
        assert!(!c.is_running());
        c.resume();
        c.resume();
        assert!(c.is_running());
    }

    #[test]
    fn set_paused_mirrors_pause_resume() {
        let mut c = PlayClock::new();
        c.set_paused(true);
        assert!(!c.is_running());
        c.set_paused(false);
        assert!(c.is_running());
    }

    #[test]
    fn seek_jumps_without_changing_running_state() {
        let mut c = PlayClock::new();
        c.seek_us(4000);
        assert_eq!(c.now_us(), 4000);
        assert!(c.is_running());

        c.pause();
        c.seek_us(9000);
        assert_eq!(c.now_us(), 9000);
        assert!(!c.is_running()); // still paused
    }

    #[test]
    fn reset_returns_to_zero_and_running() {
        let mut c = PlayClock::new();
        c.advance(1234);
        c.pause();
        c.reset();
        assert_eq!(c.now_us(), 0);
        assert!(c.is_running());
    }
}
