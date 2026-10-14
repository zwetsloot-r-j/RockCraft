//! Ordering a tick's worth of note triggers for the synth.
//!
//! A playback tick (~16 ms) can cross several span boundaries at once. Sending
//! every due note-on before every due note-off breaks repeated notes: when a
//! pitch is struck again right as its previous note ends, the old note's off
//! lands *after* the new note's on, and a synth releases every voice on that
//! key — the re-strike is cut off before it is heard. [`order_triggers`] puts
//! the triggers back in song-time order, releases first on a tie, so every note
//! sounds separately.

/// One synth trigger for span `i`: start it sounding, or release it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanTrigger {
    On(usize),
    Off(usize),
}

/// Merge a tick's due note-ons and note-offs — each `(span index, song time µs)`
/// — into the order they happen in the song. At the same instant a release goes
/// before an onset, so a repeated pitch is let go before it is struck again.
pub fn order_triggers(
    ons: impl IntoIterator<Item = (usize, u64)>,
    offs: impl IntoIterator<Item = (usize, u64)>,
) -> Vec<SpanTrigger> {
    // Sort key: time, then off (0) before on (1), then index for determinism.
    let mut keyed: Vec<(u64, u8, usize)> = offs
        .into_iter()
        .map(|(i, t)| (t, 0, i))
        .chain(ons.into_iter().map(|(i, t)| (t, 1, i)))
        .collect();
    keyed.sort_unstable();
    keyed
        .into_iter()
        .map(|(_, kind, i)| {
            if kind == 0 {
                SpanTrigger::Off(i)
            } else {
                SpanTrigger::On(i)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use SpanTrigger::{Off, On};

    #[test]
    fn back_to_back_repeat_releases_before_restriking() {
        // Span 0 ends at 1000 exactly as span 1 (same pitch) starts.
        assert_eq!(
            order_triggers([(1, 1000)], [(0, 1000)]),
            vec![Off(0), On(1)]
        );
    }

    #[test]
    fn triggers_follow_song_time_across_a_tick() {
        // One tick crossed: 0 ends at 900, 1 starts at 950, 1 ends at 990,
        // 2 starts at 1000.
        assert_eq!(
            order_triggers([(1, 950), (2, 1000)], [(0, 900), (1, 990)]),
            vec![Off(0), On(1), Off(1), On(2)]
        );
    }

    #[test]
    fn a_note_shorter_than_a_tick_still_starts_before_it_stops() {
        assert_eq!(order_triggers([(0, 100)], [(0, 150)]), vec![On(0), Off(0)]);
    }

    #[test]
    fn nothing_due_is_empty() {
        assert!(order_triggers([], []).is_empty());
    }
}
