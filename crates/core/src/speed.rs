//! Practice-speed stepping shared by every frontend's speed keys.
//!
//! Play (`-` / `=`) and Edit (`!` / `@`) both step the transport through the
//! same preset speeds. The steps never go above the piece's own tempo: slowing
//! down is for practice, speeding up past 1× is not offered from a key.

/// The speeds the step keys walk through, in permille (1000 = 1×): ½, ⅝, ¾, ⅞, 1×.
pub const RATE_STEPS: [u16; 5] = [500, 625, 750, 875, 1000];

/// The piece's own tempo in permille.
pub const RATE_UNITY: u16 = 1000;

/// The next practice speed from `current` (permille) one step slower
/// (`faster == false`) or faster (`faster == true`).
///
/// An off-step value snaps to the nearest step in the asked direction (800
/// slower → 750, 800 faster → 875). Past either end of [`RATE_STEPS`] the value
/// is returned unchanged (500 slower stays 500; 1000 faster stays 1000).
pub fn nudge_rate_permille(current: u16, faster: bool) -> u16 {
    let next = if faster {
        RATE_STEPS.iter().copied().find(|&s| s > current)
    } else {
        RATE_STEPS.iter().copied().rev().find(|&s| s < current)
    };
    next.unwrap_or(current)
}

/// A playback-rate multiplier (as kept by [`crate::Composer::playback_rate`])
/// as permille, rounded to the nearest unit.
pub fn rate_to_permille(rate: f64) -> u16 {
    (rate * 1000.0).round().clamp(0.0, u16::MAX as f64) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slower_walks_down_the_steps_and_clamps_at_half() {
        let mut r = 1000;
        let mut seen = Vec::new();
        for _ in 0..6 {
            r = nudge_rate_permille(r, false);
            seen.push(r);
        }
        assert_eq!(seen, vec![875, 750, 625, 500, 500, 500]);
    }

    #[test]
    fn faster_walks_up_and_never_passes_unity() {
        assert_eq!(nudge_rate_permille(500, true), 625);
        assert_eq!(nudge_rate_permille(875, true), 1000);
        assert_eq!(nudge_rate_permille(1000, true), 1000);
    }

    #[test]
    fn off_step_values_snap_in_the_asked_direction() {
        assert_eq!(nudge_rate_permille(800, false), 750);
        assert_eq!(nudge_rate_permille(800, true), 875);
        // Below the slowest step: faster joins the steps, slower stays.
        assert_eq!(nudge_rate_permille(250, true), 500);
        assert_eq!(nudge_rate_permille(250, false), 250);
        // Above 1× (set over the socket): slower comes back to 1×.
        assert_eq!(nudge_rate_permille(1500, false), 1000);
        assert_eq!(nudge_rate_permille(1500, true), 1500);
    }

    #[test]
    fn rate_converts_to_permille() {
        assert_eq!(rate_to_permille(1.0), 1000);
        assert_eq!(rate_to_permille(0.875), 875);
        assert_eq!(rate_to_permille(0.625), 625);
    }
}
