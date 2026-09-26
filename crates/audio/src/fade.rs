//! A linear gain ramp between silence and full level — the one fade shape
//! shared by the synth buses (stepped once per rendered block) and the backing
//! track (stepped once per sample).
//!
//! Used to soften the wait-mode freeze: rather than the music stopping dead
//! when the transport parks on a note you have yet to play, it fades out over a
//! fraction of a second, and fades back in (much faster) when you hit the key.

/// Where a fade is heading and how fast. The rate is fixed per fade — a full
/// swing takes `steps` ticks — so a fade that starts part-way (a resume that
/// interrupts a fade-out) is proportionally shorter, never a jump.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ramp {
    level: f32,
    target: f32,
    step: f32,
}

impl Ramp {
    /// Steady at full level: the resting state.
    pub(crate) const FULL: Ramp = Ramp {
        level: 1.0,
        target: 1.0,
        step: 0.0,
    };

    /// Head for silence (`out`) or full level over `steps` ticks for a full
    /// swing; `steps == 0` jumps straight there.
    pub(crate) fn fade(&mut self, out: bool, steps: u32) {
        self.target = if out { 0.0 } else { 1.0 };
        if steps == 0 {
            self.level = self.target;
            self.step = 0.0;
        } else {
            self.step = 1.0 / steps as f32;
        }
    }

    /// Advance one tick toward the target and return the new level.
    pub(crate) fn tick(&mut self) -> f32 {
        if self.level < self.target {
            self.level = (self.level + self.step).min(self.target);
        } else if self.level > self.target {
            self.level = (self.level - self.step).max(self.target);
        }
        self.level
    }

    pub(crate) fn level(&self) -> f32 {
        self.level
    }

    /// Whether the fade has finished (level == target).
    pub(crate) fn is_steady(&self) -> bool {
        self.level == self.target
    }

    /// Fully faded out and staying there.
    pub(crate) fn is_silent(&self) -> bool {
        self.level == 0.0 && self.target == 0.0
    }

    /// Whether the ramp is heading for silence.
    pub(crate) fn is_fading_out(&self) -> bool {
        self.target == 0.0
    }
}

/// How many ticks of `tick_rate` per second make up `ms` milliseconds (at
/// least one, so a nonzero fade never degenerates into a jump).
pub(crate) fn steps_for(ms: u32, tick_rate: f64) -> u32 {
    if ms == 0 {
        return 0;
    }
    ((ms as f64 / 1000.0 * tick_rate).round() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_out_linearly_then_holds_silence() {
        let mut r = Ramp::FULL;
        r.fade(true, 4);
        let levels: Vec<f32> = (0..6).map(|_| r.tick()).collect();
        assert_eq!(levels, [0.75, 0.5, 0.25, 0.0, 0.0, 0.0]);
        assert!(r.is_silent() && r.is_steady());
    }

    /// A resume mid-fade turns around from where it is, at the fade-in rate.
    #[test]
    fn turns_around_mid_fade_without_a_jump() {
        let mut r = Ramp::FULL;
        r.fade(true, 4);
        r.tick();
        r.tick(); // 0.5
        r.fade(false, 2);
        assert_eq!(r.tick(), 1.0);
        assert!(r.is_steady() && !r.is_fading_out());
    }

    #[test]
    fn zero_steps_jumps() {
        let mut r = Ramp::FULL;
        r.fade(true, 0);
        assert!(r.is_silent());
        assert_eq!(r.level(), 0.0);
    }

    #[test]
    fn steps_for_rounds_and_never_collapses_a_real_fade() {
        assert_eq!(steps_for(250, 1000.0), 250);
        assert_eq!(steps_for(250, 44_100.0 / 64.0), 172);
        assert_eq!(steps_for(1, 10.0), 1);
        assert_eq!(steps_for(0, 44_100.0), 0);
    }
}
