use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSig {
    pub beats_per_bar: u8,
    pub beat_unit: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subdivision {
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthTriplet,
}

impl Subdivision {
    pub const ALL: [Subdivision; 6] = [
        Subdivision::Quarter,
        Subdivision::Eighth,
        Subdivision::Sixteenth,
        Subdivision::ThirtySecond,
        Subdivision::EighthTriplet,
        Subdivision::SixteenthTriplet,
    ];

    pub fn finer(self) -> Subdivision {
        let pos = Self::ALL.iter().position(|s| s == &self).unwrap_or(0);
        Self::ALL[pos.min(Self::ALL.len() - 2) + 1]
    }

    pub fn coarser(self) -> Subdivision {
        let pos = Self::ALL.iter().position(|s| s == &self).unwrap_or(0);
        Self::ALL[pos.saturating_sub(1)]
    }

    pub fn label(self) -> &'static str {
        match self {
            Subdivision::Quarter => "1/4",
            Subdivision::Eighth => "1/8",
            Subdivision::Sixteenth => "1/16",
            Subdivision::ThirtySecond => "1/32",
            Subdivision::EighthTriplet => "1/8T",
            Subdivision::SixteenthTriplet => "1/16T",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub bpm: u32,
    pub time_sig: TimeSig,
    pub subdivision: Subdivision,
    /// Phase offset (µs) of the grid: the song time at which bar 1, beat 1,
    /// step 0 falls. `0` (the default) reproduces the original origin-at-zero
    /// behaviour. Set it to a piece's first downbeat so bar/beat/step lines land
    /// on the performance when the music doesn't start at song time 0 (e.g. an
    /// imported chart whose first note is several seconds in).
    #[serde(default)]
    pub origin_us: u64,
}

impl Grid {
    /// Lowest editable tempo, in BPM.
    pub const MIN_BPM: u32 = 20;
    /// Highest editable tempo, in BPM.
    pub const MAX_BPM: u32 = 300;
    /// Fewest beats a bar may hold. One is degenerate but legal (every beat a
    /// downbeat); zero would make [`bar_us`](Grid::bar_us) zero and divide by it.
    pub const MIN_BEATS_PER_BAR: u8 = 1;
    /// Most beats a bar may hold.
    pub const MAX_BEATS_PER_BAR: u8 = 32;
    /// Legal beat units (note values). Powers of two only, as a time signature's
    /// lower numeral always is.
    pub const BEAT_UNITS: [u8; 6] = [1, 2, 4, 8, 16, 32];

    /// Set the tempo to `bpm`, clamped to [`MIN_BPM`](Grid::MIN_BPM)..=[`MAX_BPM`](Grid::MAX_BPM).
    pub fn set_bpm(&mut self, bpm: u32) {
        self.bpm = bpm.clamp(Self::MIN_BPM, Self::MAX_BPM);
    }

    /// Nudge the tempo by `delta` BPM, clamped to the editable range. Saturating
    /// so extreme deltas land cleanly on a bound rather than wrapping.
    pub fn adjust_bpm(&mut self, delta: i32) {
        let next =
            (self.bpm as i64 + delta as i64).clamp(Self::MIN_BPM as i64, Self::MAX_BPM as i64);
        self.bpm = next as u32;
    }

    pub fn default_120() -> Self {
        Grid {
            bpm: 120,
            time_sig: TimeSig {
                beats_per_bar: 4,
                beat_unit: 4,
            },
            subdivision: Subdivision::Sixteenth,
            origin_us: 0,
        }
    }

    /// Set the grid phase origin: the song time bar 1 / beat 1 / step 0 lands on.
    pub fn set_origin_us(&mut self, origin_us: u64) {
        self.origin_us = origin_us;
    }

    /// Set the metre, e.g. `(3, 4)` for 3/4.
    ///
    /// `beats_per_bar` is clamped to
    /// [`MIN_BEATS_PER_BAR`](Grid::MIN_BEATS_PER_BAR)..=[`MAX_BEATS_PER_BAR`](Grid::MAX_BEATS_PER_BAR).
    /// `beat_unit` is a note value, so it must be a power of two in
    /// [`BEAT_UNITS`](Grid::BEAT_UNITS); anything else — including `0`, which
    /// would divide by zero in [`bar_us`](Grid::bar_us) — snaps to the nearest
    /// legal unit. Out of range is not an error, matching
    /// [`set_bpm`](Grid::set_bpm).
    pub fn set_time_sig(&mut self, beats_per_bar: u8, beat_unit: u8) {
        self.time_sig = TimeSig {
            beats_per_bar: beats_per_bar.clamp(Self::MIN_BEATS_PER_BAR, Self::MAX_BEATS_PER_BAR),
            beat_unit: Self::nearest_beat_unit(beat_unit),
        };
    }

    /// Snap `unit` to the nearest legal [`BEAT_UNITS`](Grid::BEAT_UNITS) entry
    /// (ties round down, to the longer note value).
    fn nearest_beat_unit(unit: u8) -> u8 {
        *Self::BEAT_UNITS
            .iter()
            .min_by_key(|u| (**u as i32 - unit as i32).abs())
            .expect("BEAT_UNITS is non-empty")
    }

    /// Snap `us` to the nearest multiple of `step` from the grid origin — the
    /// generalisation of [`snap`](Grid::snap) to an arbitrary resolution (used by
    /// region quantise, which may snap a bar at a coarser step than the live
    /// grid subdivision). Positions at/below the origin snap to the origin.
    pub fn snap_to_step(&self, us: u64, step: u64) -> u64 {
        let step = step.max(1);
        let rel = us.saturating_sub(self.origin_us);
        self.origin_us + (rel + step / 2) / step * step
    }

    pub fn quarter_us(&self) -> u64 {
        60_000_000 / self.bpm as u64
    }

    pub fn step_us(&self) -> u64 {
        let q = self.quarter_us();
        match self.subdivision {
            Subdivision::Quarter => q,
            Subdivision::Eighth => q / 2,
            Subdivision::Sixteenth => q / 4,
            Subdivision::ThirtySecond => q / 8,
            Subdivision::EighthTriplet => q / 3,
            Subdivision::SixteenthTriplet => q / 6,
        }
    }

    pub fn snap(&self, us: u64) -> u64 {
        self.snap_to_step(us, self.step_us())
    }

    pub fn step_index(&self, us: u64) -> u64 {
        us.saturating_sub(self.origin_us) / self.step_us()
    }

    pub fn us_of_step(&self, step: u64) -> u64 {
        self.origin_us + step * self.step_us()
    }

    pub fn bar_us(&self) -> u64 {
        self.time_sig.beats_per_bar as u64 * self.quarter_us() * 4 / self.time_sig.beat_unit as u64
    }

    pub fn bar_beat_of(&self, us: u64) -> (u64, u64) {
        let bar_us = self.bar_us();
        let beat_us = self.beat_us();
        let rel = us.saturating_sub(self.origin_us);
        let bar = rel / bar_us;
        let beat = (rel % bar_us) / beat_us;
        (bar, beat)
    }

    /// Song time (µs) of bar `bar` (0-indexed from the origin): `origin + bar ·
    /// bar_us`. The left edge of the `bar`-th bar, for region quantise.
    pub fn us_of_bar(&self, bar: u64) -> u64 {
        self.origin_us + bar * self.bar_us()
    }

    /// Duration of one beat (the time-signature beat unit) in µs.
    pub fn beat_us(&self) -> u64 {
        self.quarter_us() * 4 / self.time_sig.beat_unit as u64
    }

    /// Step index within the current beat at position `us` (0-indexed).
    /// Returns 0 when the subdivision is coarser than or equal to one beat.
    pub fn step_in_beat(&self, us: u64) -> u64 {
        let beat = self.beat_us().max(1);
        let in_beat = us.saturating_sub(self.origin_us) % beat;
        in_beat / self.step_us().max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_120_4_4(sub: Subdivision) -> Grid {
        Grid {
            bpm: 120,
            time_sig: TimeSig {
                beats_per_bar: 4,
                beat_unit: 4,
            },
            subdivision: sub,
            origin_us: 0,
        }
    }

    #[test]
    fn quarter_us_120() {
        assert_eq!(grid_120_4_4(Subdivision::Quarter).quarter_us(), 500_000);
    }

    #[test]
    fn set_bpm_clamps_to_range() {
        let mut g = grid_120_4_4(Subdivision::Quarter);
        g.set_bpm(140);
        assert_eq!(g.bpm, 140);
        g.set_bpm(5); // below MIN
        assert_eq!(g.bpm, Grid::MIN_BPM);
        g.set_bpm(10_000); // above MAX
        assert_eq!(g.bpm, Grid::MAX_BPM);
    }

    #[test]
    fn adjust_bpm_nudges_and_clamps() {
        let mut g = grid_120_4_4(Subdivision::Quarter);
        g.adjust_bpm(5);
        assert_eq!(g.bpm, 125);
        g.adjust_bpm(-10);
        assert_eq!(g.bpm, 115);
        // Clamp at the floor with an extreme negative delta.
        g.adjust_bpm(-100_000);
        assert_eq!(g.bpm, Grid::MIN_BPM);
        // Clamp at the ceiling with an extreme positive delta.
        g.adjust_bpm(100_000);
        assert_eq!(g.bpm, Grid::MAX_BPM);
    }

    #[test]
    fn step_us_sixteenth_120() {
        assert_eq!(grid_120_4_4(Subdivision::Sixteenth).step_us(), 125_000);
    }

    #[test]
    fn step_us_eighth_triplet_120() {
        assert_eq!(grid_120_4_4(Subdivision::EighthTriplet).step_us(), 166_666);
    }

    #[test]
    fn snap_rounds_to_nearest() {
        let g = grid_120_4_4(Subdivision::Sixteenth);
        assert_eq!(g.snap(120_000), 125_000);
        assert_eq!(g.snap(60_000), 0);
        assert_eq!(g.snap(125_000), 125_000);
        assert_eq!(g.snap(188_000), 250_000); // 63_000 past 125k → rounds up to 250k
    }

    #[test]
    fn step_round_trip() {
        let g = grid_120_4_4(Subdivision::Sixteenth);
        for step in [0u64, 1, 7, 100] {
            assert_eq!(g.step_index(g.us_of_step(step)), step);
        }
    }

    #[test]
    fn bar_us_4_4_120() {
        let g = grid_120_4_4(Subdivision::Quarter);
        assert_eq!(g.bar_us(), 2_000_000);
    }

    #[test]
    fn bar_beat_of_beat5() {
        let g = grid_120_4_4(Subdivision::Quarter);
        // beat 5 (0-based) = bar 1, beat 1
        let us = 5 * g.quarter_us();
        assert_eq!(g.bar_beat_of(us), (1, 1));
    }

    #[test]
    fn bar_beat_of_zero() {
        let g = grid_120_4_4(Subdivision::Quarter);
        assert_eq!(g.bar_beat_of(0), (0, 0));
    }

    #[test]
    fn subdivision_finer_saturates() {
        assert_eq!(
            Subdivision::SixteenthTriplet.finer(),
            Subdivision::SixteenthTriplet
        );
        assert_eq!(Subdivision::Quarter.finer(), Subdivision::Eighth);
    }

    #[test]
    fn subdivision_coarser_saturates() {
        assert_eq!(Subdivision::Quarter.coarser(), Subdivision::Quarter);
        assert_eq!(Subdivision::Eighth.coarser(), Subdivision::Quarter);
        assert_eq!(
            Subdivision::SixteenthTriplet.coarser(),
            Subdivision::EighthTriplet
        );
    }

    #[test]
    fn subdivision_all_walk() {
        let mut s = Subdivision::Quarter;
        for &expected in &Subdivision::ALL[1..] {
            s = s.finer();
            assert_eq!(s, expected);
        }
        // now at end — finer saturates
        assert_eq!(s.finer(), Subdivision::SixteenthTriplet);
    }

    #[test]
    fn subdivision_labels() {
        assert_eq!(Subdivision::Quarter.label(), "1/4");
        assert_eq!(Subdivision::Eighth.label(), "1/8");
        assert_eq!(Subdivision::Sixteenth.label(), "1/16");
        assert_eq!(Subdivision::ThirtySecond.label(), "1/32");
        assert_eq!(Subdivision::EighthTriplet.label(), "1/8T");
        assert_eq!(Subdivision::SixteenthTriplet.label(), "1/16T");
    }

    #[test]
    fn default_120() {
        let g = Grid::default_120();
        assert_eq!(g.bpm, 120);
        assert_eq!(g.time_sig.beats_per_bar, 4);
        assert_eq!(g.time_sig.beat_unit, 4);
        assert_eq!(g.subdivision, Subdivision::Sixteenth);
    }

    #[test]
    fn set_time_sig_sets_metre_and_shortens_the_bar() {
        let mut g = Grid::default_120();
        let four_four_bar = g.bar_us();
        g.set_time_sig(3, 4);
        assert_eq!(g.time_sig.beats_per_bar, 3);
        assert_eq!(g.time_sig.beat_unit, 4);
        // 3/4 at 120 BPM is a 1.5 s bar — three quarters of the 4/4 bar.
        assert_eq!(g.bar_us(), 1_500_000);
        assert_eq!(g.bar_us(), four_four_bar / 4 * 3);
        // The beat itself is untouched by the metre.
        assert_eq!(g.beat_us(), 500_000);
    }

    #[test]
    fn set_time_sig_clamps_beats_and_snaps_beat_unit() {
        let mut g = Grid::default_120();

        g.set_time_sig(0, 4);
        assert_eq!(g.time_sig.beats_per_bar, Grid::MIN_BEATS_PER_BAR);
        g.set_time_sig(200, 4);
        assert_eq!(g.time_sig.beats_per_bar, Grid::MAX_BEATS_PER_BAR);

        // A zero beat_unit would divide by zero in bar_us — it must not survive.
        g.set_time_sig(4, 0);
        assert_eq!(g.time_sig.beat_unit, 1);
        assert!(g.bar_us() > 0);

        // Non-powers of two snap to the nearest legal note value.
        g.set_time_sig(6, 7);
        assert_eq!(g.time_sig.beat_unit, 8);
        g.set_time_sig(6, 5);
        assert_eq!(g.time_sig.beat_unit, 4);
        g.set_time_sig(6, 200);
        assert_eq!(g.time_sig.beat_unit, 32);
    }

    #[test]
    fn compound_metre_bar_length() {
        let mut g = Grid::default_120();
        g.set_time_sig(6, 8);
        // 6/8 at 120 BPM: six eighths = three quarters = 1.5 s.
        assert_eq!(g.bar_us(), 1_500_000);
    }
}
