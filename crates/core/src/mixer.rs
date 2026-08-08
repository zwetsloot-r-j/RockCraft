//! Sound selection and the play-time mixer — **pure settings, no audio**.
//!
//! Playing along produces three simultaneous sound sources, and they need to be
//! shaped independently:
//!
//! - **You** — the notes coming off the piano, echoed by the synth.
//! - **The song** — the chart auditioned by "hear the song" (and the
//!   other-hand accompaniment that rides the same path).
//! - **The backing** — the decoded audio file playing under both.
//!
//! The first two are synth voices and get a [`SynthBus`] each: a MIDI channel
//! with its own program (instrument) and volume. The third is an audio sink and
//! only has a volume. [`MixerBus`] names all three for the volume-setting path;
//! [`SynthBus`] names the two that can carry an instrument.
//!
//! Everything here is data: [`Mixer`] is the settings a frontend holds and
//! `crates/audio` applies (program change + channel volume on the synth, sink
//! volume on the backing). `core` never touches a device, so nothing in this
//! module makes a sound on its own.

use serde::{Deserialize, Serialize};

// ── Buses ───────────────────────────────────────────────────────────────────

/// A synth voice bus: one MIDI channel with its own instrument and volume.
///
/// The two buses exist so the notes you play can be told apart from the notes
/// the song plays — a different sound, a different level, or silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SynthBus {
    /// The player's own notes, echoed live off the piano.
    Player,
    /// The auto-played song ("hear the song" / accompaniment).
    Song,
}

impl SynthBus {
    /// The MIDI channel this bus renders on. Stable: the synth addresses its
    /// program changes, volume, and notes by this number.
    pub fn midi_channel(self) -> u8 {
        match self {
            SynthBus::Player => 0,
            SynthBus::Song => 1,
        }
    }

    /// Stable snake_case name, identical to the serde tag.
    pub fn name(self) -> &'static str {
        match self {
            SynthBus::Player => "player",
            SynthBus::Song => "song",
        }
    }

    /// Every synth bus, for catalogs and exhaustive iteration.
    pub fn all() -> &'static [SynthBus] {
        &[SynthBus::Player, SynthBus::Song]
    }
}

/// Anything with a level in the play-time mixer: the two synth buses plus the
/// backing audio track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixerBus {
    /// The player's own notes (synth).
    Player,
    /// The auto-played song (synth).
    Song,
    /// The backing audio file.
    Backing,
}

impl MixerBus {
    /// The synth bus this refers to, or `None` for [`MixerBus::Backing`] (an
    /// audio sink, not a synth voice — it has a level but no instrument).
    pub fn synth_bus(self) -> Option<SynthBus> {
        match self {
            MixerBus::Player => Some(SynthBus::Player),
            MixerBus::Song => Some(SynthBus::Song),
            MixerBus::Backing => None,
        }
    }

    /// Stable snake_case name, identical to the serde tag.
    pub fn name(self) -> &'static str {
        match self {
            MixerBus::Player => "player",
            MixerBus::Song => "song",
            MixerBus::Backing => "backing",
        }
    }

    /// Every mixer bus, for catalogs and exhaustive iteration.
    pub fn all() -> &'static [MixerBus] {
        &[MixerBus::Player, MixerBus::Song, MixerBus::Backing]
    }
}

impl From<SynthBus> for MixerBus {
    fn from(bus: SynthBus) -> Self {
        match bus {
            SynthBus::Player => MixerBus::Player,
            SynthBus::Song => MixerBus::Song,
        }
    }
}

// ── Gain ────────────────────────────────────────────────────────────────────

/// A linear volume multiplier in `0.0..=1.0` — silence to unity.
///
/// Constructed through [`Gain::new`], which clamps into range and rejects
/// non-finite input, so a bad slider value can never reach an audio sink.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Gain(f32);

impl Gain {
    /// Full level — the default for every bus.
    pub const UNITY: Gain = Gain(1.0);
    /// Silence.
    pub const SILENT: Gain = Gain(0.0);

    /// Build a gain from a raw multiplier, clamping into `0.0..=1.0`.
    ///
    /// Returns `None` for NaN / infinity — the only genuinely invalid input;
    /// everything else has a sensible nearest legal value.
    pub fn new(value: f32) -> Option<Gain> {
        if !value.is_finite() {
            return None;
        }
        Some(Gain(value.clamp(0.0, 1.0)))
    }

    /// The clamped multiplier, for handing to an audio sink.
    pub fn value(self) -> f32 {
        self.0
    }

    /// The equivalent MIDI channel-volume value (controller 7), `0..=127`.
    pub fn midi_volume(self) -> u8 {
        (self.0 * 127.0).round().clamp(0.0, 127.0) as u8
    }
}

impl Default for Gain {
    fn default() -> Self {
        Gain::UNITY
    }
}

// ── Instruments ─────────────────────────────────────────────────────────────

/// One selectable sound: a stable id, a display name, and the General MIDI
/// program number the synth switches to.
///
/// Sounding it needs a SoundFont carrying that program; a single-preset piano
/// bank falls back to its one preset (see `crates/audio/assets/NOTICE.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Instrument {
    /// Stable identifier used on the wire and in persisted settings.
    pub id: &'static str,
    /// Human-readable name for a menu.
    pub name: &'static str,
    /// General MIDI program number (`0..=127`).
    pub program: u8,
}

/// The id of the default instrument every bus starts on.
pub const DEFAULT_INSTRUMENT: &str = "grand_piano";

/// The curated instrument list — a small, playable set rather than all 128 GM
/// programs, chosen so each entry is clearly distinguishable from the piano
/// when checking a chart by ear.
static INSTRUMENTS: &[Instrument] = &[
    Instrument {
        id: "grand_piano",
        name: "Grand Piano",
        program: 0,
    },
    Instrument {
        id: "bright_piano",
        name: "Bright Piano",
        program: 1,
    },
    Instrument {
        id: "electric_piano",
        name: "Electric Piano",
        program: 4,
    },
    Instrument {
        id: "harpsichord",
        name: "Harpsichord",
        program: 6,
    },
    Instrument {
        id: "vibraphone",
        name: "Vibraphone",
        program: 11,
    },
    Instrument {
        id: "marimba",
        name: "Marimba",
        program: 12,
    },
    Instrument {
        id: "church_organ",
        name: "Church Organ",
        program: 19,
    },
    Instrument {
        id: "nylon_guitar",
        name: "Nylon Guitar",
        program: 24,
    },
    Instrument {
        id: "steel_guitar",
        name: "Steel Guitar",
        program: 25,
    },
    Instrument {
        id: "electric_bass",
        name: "Electric Bass",
        program: 33,
    },
    Instrument {
        id: "strings",
        name: "String Ensemble",
        program: 48,
    },
    Instrument {
        id: "trumpet",
        name: "Trumpet",
        program: 56,
    },
    Instrument {
        id: "alto_sax",
        name: "Alto Sax",
        program: 65,
    },
    Instrument {
        id: "flute",
        name: "Flute",
        program: 73,
    },
    Instrument {
        id: "square_lead",
        name: "Square Lead",
        program: 80,
    },
];

/// The curated instrument catalog, in menu order. Doubles as the wire catalog
/// an agent (or the instrument dropdown) enumerates, so the UI never hardcodes
/// its own copy.
pub fn instruments() -> &'static [Instrument] {
    INSTRUMENTS
}

/// Look up an instrument by its stable [`id`](Instrument::id).
pub fn instrument(id: &str) -> Option<&'static Instrument> {
    INSTRUMENTS.iter().find(|i| i.id == id)
}

/// The default instrument (grand piano). Infallible: a test pins
/// [`DEFAULT_INSTRUMENT`] to a catalog entry.
pub fn default_instrument() -> &'static Instrument {
    instrument(DEFAULT_INSTRUMENT).expect("DEFAULT_INSTRUMENT is in the catalog")
}

// ── Mixer ───────────────────────────────────────────────────────────────────

/// Settings for one synth bus: what it sounds like and how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BusMix {
    /// The selected sound. Serialises as its full [`Instrument`] record so a
    /// client gets the display name and program without a second lookup.
    pub instrument: &'static Instrument,
    /// The bus level.
    pub gain: Gain,
}

impl Default for BusMix {
    fn default() -> Self {
        BusMix {
            instrument: default_instrument(),
            gain: Gain::UNITY,
        }
    }
}

/// The whole play-time mix: an instrument + level per synth bus, and a level
/// for the backing audio.
///
/// Pure settings. A frontend owns one, mutates it through the setters below,
/// and pushes the result at `crates/audio` (program change + channel volume on
/// the synth, sink volume on the backing). Nothing here makes a sound.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct Mixer {
    /// The player's own notes.
    pub player: BusMix,
    /// The auto-played song.
    pub song: BusMix,
    /// The backing audio track's level (it has no instrument).
    pub backing_gain: Gain,
}

/// A rejected [`Mixer`] change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixerError {
    /// The instrument id is not in [`instruments`].
    UnknownInstrument(String),
    /// The requested gain was NaN or infinite.
    BadGain,
}

impl std::fmt::Display for MixerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MixerError::UnknownInstrument(id) => write!(f, "unknown instrument: {id}"),
            MixerError::BadGain => write!(f, "gain must be a finite number"),
        }
    }
}

impl std::error::Error for MixerError {}

impl Mixer {
    /// A fresh mix: every bus on the default instrument at unity gain.
    pub fn new() -> Self {
        Mixer::default()
    }

    /// The settings for one synth bus.
    pub fn bus(&self, bus: SynthBus) -> &BusMix {
        match bus {
            SynthBus::Player => &self.player,
            SynthBus::Song => &self.song,
        }
    }

    /// The level of any bus, backing included.
    pub fn gain(&self, bus: MixerBus) -> Gain {
        match bus {
            MixerBus::Player => self.player.gain,
            MixerBus::Song => self.song.gain,
            MixerBus::Backing => self.backing_gain,
        }
    }

    /// Point a synth bus at a catalog instrument by id, returning the selected
    /// [`Instrument`] so the caller can push its program at the synth.
    pub fn set_instrument(
        &mut self,
        bus: SynthBus,
        id: &str,
    ) -> Result<&'static Instrument, MixerError> {
        let found = instrument(id).ok_or_else(|| MixerError::UnknownInstrument(id.to_string()))?;
        match bus {
            SynthBus::Player => self.player.instrument = found,
            SynthBus::Song => self.song.instrument = found,
        }
        Ok(found)
    }

    /// Set a bus level from a raw multiplier, clamping into `0.0..=1.0`.
    /// Returns the [`Gain`] actually stored.
    pub fn set_gain(&mut self, bus: MixerBus, value: f32) -> Result<Gain, MixerError> {
        let gain = Gain::new(value).ok_or(MixerError::BadGain)?;
        match bus {
            MixerBus::Player => self.player.gain = gain,
            MixerBus::Song => self.song.gain = gain,
            MixerBus::Backing => self.backing_gain = gain,
        }
        Ok(gain)
    }
}

/// A [`Mixer`] plus the instrument catalog — the payload a `query_mixer`
/// answers with, so a client can render the dropdown and the sliders from one
/// round trip and never hardcode the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct MixerReport {
    /// The current mix.
    #[serde(flatten)]
    pub mixer: Mixer,
    /// Every selectable instrument, in menu order.
    pub instruments: &'static [Instrument],
}

impl From<Mixer> for MixerReport {
    fn from(mixer: Mixer) -> Self {
        MixerReport {
            mixer,
            instruments: instruments(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_buses_have_distinct_channels() {
        assert_eq!(SynthBus::Player.midi_channel(), 0);
        assert_eq!(SynthBus::Song.midi_channel(), 1);
        assert_ne!(
            SynthBus::Player.midi_channel(),
            SynthBus::Song.midi_channel()
        );
    }

    #[test]
    fn bus_names_match_serde_tags() {
        for &bus in SynthBus::all() {
            let tag = serde_json::to_value(bus).unwrap();
            assert_eq!(tag.as_str().unwrap(), bus.name());
        }
        for &bus in MixerBus::all() {
            let tag = serde_json::to_value(bus).unwrap();
            assert_eq!(tag.as_str().unwrap(), bus.name());
        }
    }

    #[test]
    fn only_backing_lacks_a_synth_bus() {
        assert_eq!(MixerBus::Player.synth_bus(), Some(SynthBus::Player));
        assert_eq!(MixerBus::Song.synth_bus(), Some(SynthBus::Song));
        assert_eq!(MixerBus::Backing.synth_bus(), None);
    }

    #[test]
    fn synth_bus_widens_to_mixer_bus() {
        assert_eq!(MixerBus::from(SynthBus::Player), MixerBus::Player);
        assert_eq!(MixerBus::from(SynthBus::Song), MixerBus::Song);
    }

    #[test]
    fn gain_clamps_into_range() {
        assert_eq!(Gain::new(-3.0).unwrap().value(), 0.0);
        assert_eq!(Gain::new(2.5).unwrap().value(), 1.0);
        assert_eq!(Gain::new(0.5).unwrap().value(), 0.5);
    }

    #[test]
    fn gain_rejects_non_finite() {
        assert_eq!(Gain::new(f32::NAN), None);
        assert_eq!(Gain::new(f32::INFINITY), None);
        assert_eq!(Gain::new(f32::NEG_INFINITY), None);
    }

    #[test]
    fn gain_maps_onto_midi_volume() {
        assert_eq!(Gain::SILENT.midi_volume(), 0);
        assert_eq!(Gain::UNITY.midi_volume(), 127);
        assert_eq!(Gain::new(0.5).unwrap().midi_volume(), 64);
    }

    #[test]
    fn instrument_catalog_is_non_empty_and_unique() {
        use std::collections::BTreeSet;
        let ids: BTreeSet<&str> = instruments().iter().map(|i| i.id).collect();
        assert!(!instruments().is_empty());
        assert_eq!(ids.len(), instruments().len(), "duplicate instrument ids");
        let programs: BTreeSet<u8> = instruments().iter().map(|i| i.program).collect();
        assert_eq!(
            programs.len(),
            instruments().len(),
            "duplicate GM program numbers"
        );
    }

    #[test]
    fn instrument_programs_are_valid_gm() {
        for i in instruments() {
            assert!(i.program < 128, "{} is not a GM program", i.id);
            assert!(!i.name.is_empty(), "{} has no display name", i.id);
        }
    }

    #[test]
    fn default_instrument_is_in_the_catalog() {
        assert_eq!(default_instrument().id, DEFAULT_INSTRUMENT);
        assert_eq!(default_instrument().program, 0);
    }

    #[test]
    fn instrument_lookup_is_by_id() {
        assert_eq!(instrument("marimba").unwrap().program, 12);
        assert_eq!(instrument("Marimba"), None, "ids are exact, not fuzzy");
        assert_eq!(instrument("nope"), None);
    }

    #[test]
    fn fresh_mixer_is_unity_piano_everywhere() {
        let m = Mixer::new();
        for &bus in SynthBus::all() {
            assert_eq!(m.bus(bus).instrument.id, DEFAULT_INSTRUMENT);
        }
        for &bus in MixerBus::all() {
            assert_eq!(m.gain(bus), Gain::UNITY);
        }
    }

    #[test]
    fn setting_one_bus_leaves_the_other_alone() {
        let mut m = Mixer::new();
        m.set_instrument(SynthBus::Song, "marimba").unwrap();
        m.set_gain(MixerBus::Song, 0.25).unwrap();
        assert_eq!(m.song.instrument.id, "marimba");
        assert_eq!(m.song.gain.value(), 0.25);
        assert_eq!(m.player.instrument.id, DEFAULT_INSTRUMENT);
        assert_eq!(m.player.gain, Gain::UNITY);
        assert_eq!(m.backing_gain, Gain::UNITY);
    }

    #[test]
    fn set_instrument_returns_the_selection_and_rejects_unknown() {
        let mut m = Mixer::new();
        assert_eq!(
            m.set_instrument(SynthBus::Player, "flute").unwrap().program,
            73
        );
        assert_eq!(
            m.set_instrument(SynthBus::Player, "kazoo").unwrap_err(),
            MixerError::UnknownInstrument("kazoo".into())
        );
        // The rejected change left the previous selection standing.
        assert_eq!(m.player.instrument.id, "flute");
    }

    #[test]
    fn set_gain_clamps_and_rejects_non_finite() {
        let mut m = Mixer::new();
        assert_eq!(m.set_gain(MixerBus::Backing, 4.0).unwrap(), Gain::UNITY);
        assert_eq!(m.set_gain(MixerBus::Backing, -1.0).unwrap(), Gain::SILENT);
        assert_eq!(
            m.set_gain(MixerBus::Backing, f32::NAN).unwrap_err(),
            MixerError::BadGain
        );
        assert_eq!(
            m.backing_gain,
            Gain::SILENT,
            "a rejected gain changes nothing"
        );
    }

    #[test]
    fn report_serialises_mix_and_catalog() {
        let mut m = Mixer::new();
        m.set_instrument(SynthBus::Song, "vibraphone").unwrap();
        m.set_gain(MixerBus::Backing, 0.5).unwrap();
        let v = serde_json::to_value(MixerReport::from(m)).unwrap();
        assert_eq!(v["player"]["instrument"]["id"], "grand_piano");
        assert_eq!(v["song"]["instrument"]["name"], "Vibraphone");
        assert_eq!(v["song"]["instrument"]["program"], 11);
        assert_eq!(v["song"]["gain"], 1.0);
        assert_eq!(v["backing_gain"], 0.5);
        assert_eq!(
            v["instruments"].as_array().unwrap().len(),
            instruments().len()
        );
    }

    #[test]
    fn mixer_error_displays() {
        assert_eq!(
            MixerError::UnknownInstrument("kazoo".into()).to_string(),
            "unknown instrument: kazoo"
        );
        assert_eq!(
            MixerError::BadGain.to_string(),
            "gain must be a finite number"
        );
    }
}
