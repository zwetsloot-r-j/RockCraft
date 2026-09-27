//! Tempo-map bar lookups, shared by the editor ([`crate::Composer`]) and play
//! mode. Pure: a borrowed list of bar downbeats plus a uniform fallback.
//!
//! A *usable* map has at least two starts (so the last bar's length is known).
//! Bars past the map extrapolate with the last mapped bar's length. Without a
//! usable map every lookup falls back to uniform bars of `fallback_bar_us`,
//! counted from `origin_us` (0 unless set with [`BarMap::with_origin`]).

/// Bar lookups over a tempo map (`starts[b]` = downbeat of bar `b`, µs).
#[derive(Debug, Clone, Copy)]
pub struct BarMap<'a> {
    starts: &'a [u64],
    fallback_bar_us: u64,
    origin_us: u64,
}

impl<'a> BarMap<'a> {
    /// A map over `starts` (ascending), falling back to uniform bars of
    /// `fallback_bar_us` from time 0 when `starts` has fewer than two entries.
    pub fn new(starts: &'a [u64], fallback_bar_us: u64) -> Self {
        Self {
            starts,
            fallback_bar_us: fallback_bar_us.max(1),
            origin_us: 0,
        }
    }

    /// Set the origin (downbeat of bar 0) of the uniform fallback.
    pub fn with_origin(mut self, origin_us: u64) -> Self {
        self.origin_us = origin_us;
        self
    }

    /// Length of the last mapped bar, when the map is usable.
    fn last_dur(&self) -> Option<u64> {
        let n = self.starts.len();
        (n >= 2).then(|| self.starts[n - 1].saturating_sub(self.starts[n - 2]).max(1))
    }

    /// Index of the bar containing `us`. Before the first start → 0.
    pub fn bar_at(&self, us: u64) -> u64 {
        let Some(dur) = self.last_dur() else {
            return us.saturating_sub(self.origin_us) / self.fallback_bar_us;
        };
        let n = self.starts.len();
        if us < self.starts[0] {
            return 0;
        }
        if us >= self.starts[n - 1] {
            return (n as u64 - 1) + (us - self.starts[n - 1]) / dur;
        }
        match self.starts.binary_search(&us) {
            Ok(i) => i as u64,
            Err(i) => (i - 1) as u64,
        }
    }

    /// Downbeat (µs) of bar `bar`. Past the map: extrapolate the last bar's
    /// length.
    pub fn bar_start(&self, bar: u64) -> u64 {
        if let Some(&t) = self.starts.get(bar as usize) {
            return t;
        }
        match self.last_dur() {
            Some(dur) => {
                let n = self.starts.len() as u64;
                self.starts[n as usize - 1] + (bar - (n - 1)) * dur
            }
            None => self.origin_us + bar * self.fallback_bar_us,
        }
    }

    /// Length (µs) of bar `bar` (at least 1).
    pub fn bar_len(&self, bar: u64) -> u64 {
        self.bar_start(bar + 1)
            .saturating_sub(self.bar_start(bar))
            .max(1)
    }

    /// `[start(first), start(last + 1))` — the span of bars `first..=last`.
    /// The bounds are swapped when `last < first`.
    pub fn bar_range_us(&self, first: u64, last: u64) -> (u64, u64) {
        let (a, b) = if last < first {
            (last, first)
        } else {
            (first, last)
        };
        (self.bar_start(a), self.bar_start(b + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: [u64; 4] = [1_000, 3_000, 4_000, 6_000];

    #[test]
    fn bar_at_at_between_and_past_starts() {
        let m = BarMap::new(&MAP, 500);
        assert_eq!(m.bar_at(1_000), 0);
        assert_eq!(m.bar_at(2_999), 0);
        assert_eq!(m.bar_at(3_000), 1);
        assert_eq!(m.bar_at(3_500), 1);
        assert_eq!(m.bar_at(4_000), 2);
        assert_eq!(m.bar_at(6_000), 3);
        // Past the map: last bar is 2000 µs long.
        assert_eq!(m.bar_at(7_999), 3);
        assert_eq!(m.bar_at(8_000), 4);
        assert_eq!(m.bar_at(12_000), 6);
    }

    #[test]
    fn before_the_first_start_is_bar_zero() {
        let m = BarMap::new(&MAP, 500);
        assert_eq!(m.bar_at(0), 0);
        assert_eq!(m.bar_at(999), 0);
    }

    #[test]
    fn bar_start_extrapolates_past_the_map() {
        let m = BarMap::new(&MAP, 500);
        assert_eq!(m.bar_start(0), 1_000);
        assert_eq!(m.bar_start(3), 6_000);
        assert_eq!(m.bar_start(4), 8_000);
        assert_eq!(m.bar_start(6), 12_000);
        assert_eq!(m.bar_len(0), 2_000);
        assert_eq!(m.bar_len(1), 1_000);
        assert_eq!(m.bar_len(5), 2_000);
    }

    #[test]
    fn empty_map_is_uniform() {
        let m = BarMap::new(&[], 2_000);
        assert_eq!(m.bar_at(0), 0);
        assert_eq!(m.bar_at(1_999), 0);
        assert_eq!(m.bar_at(2_000), 1);
        assert_eq!(m.bar_start(3), 6_000);
        assert_eq!(m.bar_range_us(1, 2), (2_000, 6_000));
        let o = BarMap::new(&[], 2_000).with_origin(500);
        assert_eq!(o.bar_at(499), 0);
        assert_eq!(o.bar_at(2_500), 1);
        assert_eq!(o.bar_start(2), 4_500);
    }

    #[test]
    fn single_start_is_not_a_usable_map() {
        let m = BarMap::new(&[100], 1_000);
        assert_eq!(m.bar_start(0), 100);
        assert_eq!(m.bar_start(1), 1_000);
        assert_eq!(m.bar_at(1_500), 1);
    }

    #[test]
    fn bar_range_is_half_open_and_swaps() {
        let m = BarMap::new(&MAP, 500);
        assert_eq!(m.bar_range_us(1, 2), (3_000, 6_000));
        assert_eq!(m.bar_range_us(2, 1), (3_000, 6_000));
        assert_eq!(m.bar_range_us(3, 3), (6_000, 8_000));
    }
}
