//! The TUI's remembered settings (M18-A): the mixer, plus the play screen's
//! shell-wide choices (note colours, scrolling, speed, song audio, wait mode).
//!
//! A small JSON file in the user's config directory. Loading is forgiving —
//! a missing file means defaults, and a bad field falls back to its default
//! without losing the good ones — so a hand-edited or stale file can never
//! stop the TUI from starting. Saving writes a temp file then renames it, so
//! a crash cannot leave half a file.

use std::io;
use std::path::{Path, PathBuf};

use rockcraft_core::{Mixer, MixerBus, SongAudio, SynthBus};
use serde_json::{json, Value};

use crate::palette::ColorMode;
use crate::play::{ScrollMode, PLAY_RATE_MAX, PLAY_RATE_MIN, PLAY_RATE_UNITY};

/// File name inside the config directory.
const FILE_NAME: &str = "tui-settings.json";

/// Current file format version.
const VERSION: u64 = 1;

/// Everything the TUI remembers between runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TuiSettings {
    /// The play-time mix.
    pub mixer: Mixer,
    /// How play-screen notes are coloured (`c`).
    pub color_mode: ColorMode,
    /// How the play highway scrolls (`v`).
    pub scroll_mode: ScrollMode,
    /// Practice speed in permille (`-` / `=`).
    pub rate_permille: u16,
    /// The song audio last picked with `m`; `None` keeps each piece's default.
    /// Re-applied per piece via [`SongAudio::clamp_to`].
    pub song_audio: Option<SongAudio>,
    /// Whether wait mode is armed (`w`).
    pub wait_mode: bool,
    /// Whether the practice loop demos each pass before your turn (`d`).
    pub demo: bool,
}

impl Default for TuiSettings {
    fn default() -> Self {
        Self {
            mixer: Mixer::default(),
            color_mode: ColorMode::default(),
            scroll_mode: ScrollMode::default(),
            rate_permille: PLAY_RATE_UNITY,
            song_audio: None,
            wait_mode: false,
            demo: true,
        }
    }
}

/// Where the settings file lives, or `None` when no home/config directory can
/// be found. `$ROCKCRAFT_CONFIG_DIR` wins (tests use it); otherwise
/// `%APPDATA%\RockCraft` on Windows, `$XDG_CONFIG_HOME/rockcraft`, else
/// `~/.config/rockcraft`.
pub fn settings_path() -> Option<PathBuf> {
    let non_empty = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(dir) = non_empty("ROCKCRAFT_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join(FILE_NAME));
    }
    if cfg!(windows) {
        if let Some(appdata) = non_empty("APPDATA") {
            return Some(PathBuf::from(appdata).join("RockCraft").join(FILE_NAME));
        }
    }
    if let Some(xdg) = non_empty("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("rockcraft").join(FILE_NAME));
    }
    let home = non_empty("HOME").or_else(|| non_empty("USERPROFILE"))?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("rockcraft")
            .join(FILE_NAME),
    )
}

/// Load the settings at `path`. Never fails: returns the settings plus one
/// warning per problem found (unreadable/unparseable file, bad field).
pub fn load(path: &Path) -> (TuiSettings, Vec<String>) {
    let mut settings = TuiSettings::default();
    let mut warnings = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (settings, warnings),
        Err(e) => {
            warnings.push(format!("could not read {}: {e}", path.display()));
            return (settings, warnings);
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!(
                "{} is not valid JSON ({e}); using defaults",
                path.display()
            ));
            return (settings, warnings);
        }
    };
    if let Some(play) = root.get("play") {
        load_play(&mut settings, play, &mut warnings);
    }
    let Some(mixer) = root.get("mixer") else {
        return (settings, warnings);
    };
    for &bus in SynthBus::all() {
        let Some(entry) = mixer.get(bus.name()) else {
            continue;
        };
        if let Some(id) = entry.get("instrument") {
            let ok = id
                .as_str()
                .is_some_and(|id| settings.mixer.set_instrument(bus, id).is_ok());
            if !ok {
                warnings.push(format!("mixer.{}.instrument: bad value {id}", bus.name()));
            }
        }
        if let Some(gain) = entry.get("gain") {
            load_gain(&mut settings.mixer, bus.into(), gain, &mut warnings);
        }
    }
    if let Some(gain) = mixer.get("backing_gain") {
        load_gain(&mut settings.mixer, MixerBus::Backing, gain, &mut warnings);
    }
    (settings, warnings)
}

/// Apply the `play` section, field by field.
fn load_play(settings: &mut TuiSettings, play: &Value, warnings: &mut Vec<String>) {
    let mut bad = |field: &str, value: &Value| {
        warnings.push(format!("play.{field}: bad value {value}"));
    };
    if let Some(v) = play.get("color_mode") {
        match v.as_str().and_then(ColorMode::from_label) {
            Some(mode) => settings.color_mode = mode,
            None => bad("color_mode", v),
        }
    }
    if let Some(v) = play.get("scroll_mode") {
        match v.as_str().and_then(ScrollMode::from_name) {
            Some(mode) => settings.scroll_mode = mode,
            None => bad("scroll_mode", v),
        }
    }
    if let Some(v) = play.get("rate_permille") {
        match v
            .as_u64()
            .filter(|r| (PLAY_RATE_MIN as u64..=PLAY_RATE_MAX as u64).contains(r))
        {
            Some(rate) => settings.rate_permille = rate as u16,
            None => bad("rate_permille", v),
        }
    }
    if let Some(v) = play.get("song_audio") {
        match serde_json::from_value::<Option<SongAudio>>(v.clone()) {
            Ok(mode) => settings.song_audio = mode,
            Err(_) => bad("song_audio", v),
        }
    }
    if let Some(v) = play.get("wait_mode") {
        match v.as_bool() {
            Some(on) => settings.wait_mode = on,
            None => bad("wait_mode", v),
        }
    }
    if let Some(v) = play.get("demo") {
        match v.as_bool() {
            Some(on) => settings.demo = on,
            None => bad("demo", v),
        }
    }
}

/// Apply one gain field; only a number in `0.0..=1.0` is accepted.
fn load_gain(mixer: &mut Mixer, bus: MixerBus, value: &Value, warnings: &mut Vec<String>) {
    let ok = value
        .as_f64()
        .filter(|g| (0.0..=1.0).contains(g))
        .is_some_and(|g| mixer.set_gain(bus, g as f32).is_ok());
    if !ok {
        warnings.push(format!("mixer gain for {}: bad value {value}", bus.name()));
    }
}

/// The JSON document for `settings`.
fn to_json(settings: &TuiSettings) -> Value {
    let bus = |b: SynthBus| {
        let m = settings.mixer.bus(b);
        json!({ "instrument": m.instrument.id, "gain": m.gain.value() })
    };
    json!({
        "version": VERSION,
        "mixer": {
            "player": bus(SynthBus::Player),
            "song": bus(SynthBus::Song),
            "backing_gain": settings.mixer.backing_gain.value(),
        },
        "play": {
            "color_mode": settings.color_mode.label(),
            "scroll_mode": settings.scroll_mode.name(),
            "rate_permille": settings.rate_permille,
            "song_audio": settings.song_audio,
            "wait_mode": settings.wait_mode,
            "demo": settings.demo,
        }
    })
}

/// Save `settings` to `path`, creating the directory if needed. Written to a
/// temp name then renamed into place.
pub fn save(path: &Path, settings: &TuiSettings) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let text = serde_json::to_string_pretty(&to_json(settings)).map_err(io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::Gain;

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rockcraft-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE_NAME)
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn round_trip() {
        let path = temp_file("round-trip");
        let mut s = TuiSettings::default();
        s.mixer.set_instrument(SynthBus::Song, "flute").unwrap();
        s.mixer.set_gain(MixerBus::Song, 0.5).unwrap();
        s.mixer.set_gain(MixerBus::Backing, 0.25).unwrap();
        save(&path, &s).unwrap();
        let (loaded, warnings) = load(&path);
        assert_eq!(loaded, s);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn play_choices_round_trip() {
        let path = temp_file("play-round-trip");
        let s = TuiSettings {
            color_mode: ColorMode::Spectrum,
            scroll_mode: ScrollMode::Sixteenths,
            rate_permille: 750,
            song_audio: Some(SongAudio::Synth),
            wait_mode: true,
            demo: false,
            ..TuiSettings::default()
        };
        save(&path, &s).unwrap();
        let (loaded, warnings) = load(&path);
        assert_eq!(loaded, s);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_mixer_only_file_keeps_play_defaults() {
        let path = temp_file("mixer-only");
        write(
            &path,
            r#"{ "version": 1, "mixer": { "backing_gain": 0.5 } }"#,
        );
        let (s, w) = load(&path);
        assert_eq!(s.color_mode, ColorMode::Hands);
        assert_eq!(s.rate_permille, PLAY_RATE_UNITY);
        assert_eq!(s.song_audio, None);
        assert!(s.demo);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn bad_play_fields_default_and_keep_the_rest() {
        let path = temp_file("bad-play");
        write(
            &path,
            r#"{ "version": 1, "play": {
                "color_mode": "plaid", "scroll_mode": "smooth",
                "rate_permille": 9000, "song_audio": "kazoo", "wait_mode": true } }"#,
        );
        let (s, w) = load(&path);
        assert_eq!(s.color_mode, ColorMode::Hands);
        assert_eq!(s.scroll_mode, ScrollMode::Smooth);
        assert_eq!(s.rate_permille, PLAY_RATE_UNITY);
        assert_eq!(s.song_audio, None);
        assert!(s.wait_mode);
        assert_eq!(w.len(), 3, "{w:?}");
    }

    #[test]
    fn missing_file_is_defaults() {
        let (s, w) = load(&temp_file("missing"));
        assert_eq!(s, TuiSettings::default());
        assert!(w.is_empty());
    }

    #[test]
    fn bad_json_is_defaults_with_a_warning() {
        let path = temp_file("bad-json");
        write(&path, "{ not json");
        let (s, w) = load(&path);
        assert_eq!(s, TuiSettings::default());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn a_bad_field_defaults_and_keeps_the_rest() {
        let path = temp_file("bad-field");
        write(
            &path,
            r#"{ "version": 1, "mixer": {
                "player": { "instrument": "kazoo", "gain": 0.5 },
                "song": { "instrument": "flute", "gain": 3.0 },
                "backing_gain": "loud" } }"#,
        );
        let (s, w) = load(&path);
        assert_eq!(s.mixer.player.instrument.id, "grand_piano");
        assert_eq!(s.mixer.player.gain, Gain::new(0.5).unwrap());
        assert_eq!(s.mixer.song.instrument.id, "flute");
        assert_eq!(s.mixer.song.gain, Gain::UNITY);
        assert_eq!(s.mixer.backing_gain, Gain::BACKING_DEFAULT);
        assert_eq!(w.len(), 3, "{w:?}");
    }

    #[test]
    fn path_honours_config_dir_override() {
        // Only this test touches the variable.
        std::env::set_var("ROCKCRAFT_CONFIG_DIR", "/tmp/rc-cfg-test");
        assert_eq!(
            settings_path(),
            Some(PathBuf::from("/tmp/rc-cfg-test").join(FILE_NAME))
        );
        std::env::remove_var("ROCKCRAFT_CONFIG_DIR");
    }
}
