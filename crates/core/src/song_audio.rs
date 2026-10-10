//! What the play screen sounds of the song itself (the `m` key).
//!
//! A piece can carry two renditions of its music: the **backing** recording (a
//! `backing.wav` extracted from an imported movie, say) and the **synth**
//! replay of the chart's own notes. They are never meant to sound together, so
//! the play screen holds exactly one [`SongAudio`] mode and `m` cycles it:
//!
//! - with a backing track: `Backing` → `Synth` → `Off` → `Backing` …
//! - without one: `Synth` → `Off` → `Synth` … (`Backing` is skipped)
//!
//! The default at load is [`SongAudio::default_for`]: the backing when there is
//! one, else the synth, so a MIDI-only piece is never silent without a live
//! piano (M13-C). Like the rest of `core` this is pure — each frontend maps the
//! mode onto its own synth gating and backing mute.

use serde::{Deserialize, Serialize};

/// The play screen's song-audio mode. Serialises as `"backing"` / `"synth"` /
/// `"off"`, the wire names every frontend reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SongAudio {
    /// The backing recording plays; the song synth is silent.
    Backing,
    /// The synth replays the song's notes; the backing is muted.
    Synth,
    /// Neither sounds — only the player's own piano.
    Off,
}

impl SongAudio {
    /// The mode a freshly loaded piece starts in: `Backing` when it has a
    /// backing track, else `Synth`.
    pub const fn default_for(has_backing: bool) -> Self {
        if has_backing {
            SongAudio::Backing
        } else {
            SongAudio::Synth
        }
    }

    /// The mode `m` moves to next. `Backing` is skipped (and, if somehow
    /// current, left) when the piece has no backing track.
    pub const fn next(self, has_backing: bool) -> Self {
        match self {
            SongAudio::Backing => SongAudio::Synth,
            SongAudio::Synth => SongAudio::Off,
            SongAudio::Off => SongAudio::default_for(has_backing),
        }
    }

    /// Whether this mode is valid for a piece — `Backing` needs a backing track.
    pub const fn is_valid_for(self, has_backing: bool) -> bool {
        !matches!(self, SongAudio::Backing) || has_backing
    }

    /// This mode, or the nearest valid one for the piece: a `Backing` request
    /// on a piece without a backing track becomes `Synth`. Used when
    /// re-applying a remembered preference to a different piece.
    pub const fn clamp_to(self, has_backing: bool) -> Self {
        if self.is_valid_for(has_backing) {
            self
        } else {
            SongAudio::Synth
        }
    }

    /// Does the synth replay the song's notes in this mode?
    pub const fn synth_on(self) -> bool {
        matches!(self, SongAudio::Synth)
    }

    /// Is the backing recording audible in this mode? (A frontend may still
    /// mute it for other reasons, e.g. practising off 1× speed.)
    pub const fn backing_on(self) -> bool {
        matches!(self, SongAudio::Backing)
    }

    /// The wire / display name (`"backing"` / `"synth"` / `"off"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            SongAudio::Backing => "backing",
            SongAudio::Synth => "synth",
            SongAudio::Off => "off",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prefers_the_backing_when_present() {
        assert_eq!(SongAudio::default_for(true), SongAudio::Backing);
        assert_eq!(SongAudio::default_for(false), SongAudio::Synth);
    }

    #[test]
    fn cycle_with_a_backing_visits_all_three() {
        let mut m = SongAudio::default_for(true);
        let mut seen = vec![m];
        for _ in 0..3 {
            m = m.next(true);
            seen.push(m);
        }
        assert_eq!(
            seen,
            [
                SongAudio::Backing,
                SongAudio::Synth,
                SongAudio::Off,
                SongAudio::Backing
            ]
        );
    }

    #[test]
    fn cycle_without_a_backing_skips_it() {
        let mut m = SongAudio::default_for(false);
        let mut seen = vec![m];
        for _ in 0..4 {
            m = m.next(false);
            seen.push(m);
        }
        assert_eq!(
            seen,
            [
                SongAudio::Synth,
                SongAudio::Off,
                SongAudio::Synth,
                SongAudio::Off,
                SongAudio::Synth
            ]
        );
        assert!(!seen.contains(&SongAudio::Backing));
    }

    #[test]
    fn exactly_one_source_per_mode() {
        for m in [SongAudio::Backing, SongAudio::Synth, SongAudio::Off] {
            assert!(!(m.synth_on() && m.backing_on()), "{m:?} sounds both");
        }
        assert!(SongAudio::Backing.backing_on());
        assert!(SongAudio::Synth.synth_on());
        assert!(!SongAudio::Off.synth_on() && !SongAudio::Off.backing_on());
    }

    #[test]
    fn clamp_drops_backing_only_without_one() {
        assert_eq!(SongAudio::Backing.clamp_to(false), SongAudio::Synth);
        assert_eq!(SongAudio::Backing.clamp_to(true), SongAudio::Backing);
        assert_eq!(SongAudio::Off.clamp_to(false), SongAudio::Off);
        assert_eq!(SongAudio::Synth.clamp_to(true), SongAudio::Synth);
        assert!(!SongAudio::Backing.is_valid_for(false));
    }

    #[test]
    fn serialises_snake_case() {
        for m in [SongAudio::Backing, SongAudio::Synth, SongAudio::Off] {
            let json = serde_json::to_value(m).unwrap();
            assert_eq!(json, serde_json::json!(m.as_str()));
            let back: SongAudio = serde_json::from_value(json).unwrap();
            assert_eq!(back, m);
        }
    }
}
