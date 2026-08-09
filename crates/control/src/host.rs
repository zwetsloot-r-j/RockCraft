//! The **host-command tier**: app-level workflows reachable over the
//! agent-control protocol, alongside `core::Action`.
//!
//! `core::Action`s are pure composer edits (`crates/core/src/action.rs`): they do
//! no I/O and auto-wire to every frontend through `action_from_name` /
//! `action_help`. App-level workflows — load a song to play, run a record
//! session, attach a backing track, import from a URL, scan/save/load the
//! library — instead do I/O (disk, MIDI device, audio, subprocess), which
//! `core` forbids ("`core` stays pure"). They can therefore **never** become
//! `core::Action`s and cannot ride the `action_from_name` path.
//!
//! This module gives those workflows the *same two properties* actions have,
//! without putting any I/O in `control` (which carries none — only the
//! vocabulary):
//!
//! 1. **One catalog.** [`host_help`] mirrors `core::action_help` so `query help`
//!    lists host commands with params + prose, discoverable live by an agent.
//! 2. **Can't-forget wiring.** Dispatch routes through the [`HostServices`]
//!    trait, whose single exhaustive `match` on [`HostCommand`] makes the
//!    compiler the reviewer: adding a variant fails to compile until every
//!    frontend handles it (per `CLAUDE.md`).
//!
//! `HostCommand` reuses `core`'s [`ParamInfo`] / [`ActionError`] so the wire
//! shapes and error vocabulary stay identical to the action tier.

use rockcraft_core::{ActionError, MixerBus, ParamInfo, SynthBus};
use serde::{Deserialize, Serialize};

/// Where [`HostCommand::SaveBundle`] writes the current timeline.
///
/// Mirrors the frontend `SaveDest` (the Tauri `state::SaveDest`): a transport-
/// agnostic copy lives here so the command catalog stays free of any
/// frontend-only type. Each frontend maps this onto its own save path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SaveDest {
    /// A timestamped quick-save take (`recordings/take-<stamp>/`).
    QuickSave,
    /// A named library bundle (`<library_root>/<slug>/`).
    Library { name: String },
    /// Overwrite the bundle the piece was loaded from (falling back to a
    /// quick-save take for a piece that has no home yet). The frontends have
    /// always had this — it is what the editor's plain "save" does — but it was
    /// missing from the protocol, so an agent could only re-save by *naming* the
    /// bundle and relying on the slug matching.
    InPlace,
}

/// One kept part to write as its own standalone bundle by
/// [`HostCommand::SplitBundle`].
///
/// The half-open song-time range `[start_us, end_us)` is sliced out of the
/// loaded piece (notes shifted to t=0, media offsets shifted — see
/// `core::segment::slice_segment`) and written as a new library bundle whose
/// slug/name is `name`. Discarded parts are simply omitted from the
/// `segments` list, which doubles as trimming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentSpec {
    pub start_us: u64,
    pub end_us: u64,
    /// Becomes the new bundle's library slug/name.
    pub name: String,
}

/// Every app-level workflow command, transport-agnostic.
///
/// Each variant serialises with an internal `"command"` tag holding its stable
/// snake_case [`name`](HostCommand::name), mirroring `core::Action`'s `"action"`
/// tag, e.g. `{"command": "load_bundle", "dir": "…"}`. A parity test enforces
/// the tag/name agreement so a remote `run_host_command` can never name a
/// command the dispatcher would reject.
///
/// The set mirrors the app-level helpers the frontends already own
/// (`play.rs`/`record.rs`/`audio.rs`/`import.rs`/`library.rs`/`state.rs`); this
/// tier is the protocol seam, not new capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum HostCommand {
    // ── library / bundles ───────────────────────────────────────────────
    /// Scan the default library roots; returns the bundle list.
    ScanLibrary,
    /// Save the current timeline to a bundle; returns the bundle dir path.
    SaveBundle { dest: SaveDest },
    /// Load a bundle directory into the composer; returns the new snapshot.
    LoadBundle { dir: String },
    /// Whether the timeline has unsaved changes; returns a bool.
    QueryDirty,
    /// Slice the loaded piece into the given kept parts, writing each as a new
    /// library bundle (subset MIDI + copied media + derived offsets,
    /// `origin = Edited`); returns the created bundle dir paths. Discarded
    /// parts are omitted (= trimming); the source piece is left untouched.
    SplitBundle { segments: Vec<SegmentSpec> },

    // ── play ────────────────────────────────────────────────────────────
    /// Load a bundle dir as a play session; returns play info.
    PlayLoad { dir: String },
    /// Arm/disarm note-by-note wait mode for the play session.
    PlaySetWait { on: bool },
    /// Set play-session speed in permille (1000 = 1x), for practising slowly.
    /// Mirrors `core::Action::SetPlaybackRate`, which drives the *editor*
    /// transport; the play session is a separate engine and needs its own.
    PlaySetRate { rate_permille: u16 },
    /// Toggle "hear the song" (audible song synth) for the play session.
    PlayToggleHearSong,
    /// Toggle pause on the active play session (freeze/thaw clock + backing).
    PlayTogglePause,
    /// Finish the play session; returns the score summary.
    PlayFinish,
    /// The live take's full state — clock, wait gate (what it awaits vs what is
    /// held), practice hand, speed, score — or `loaded: false` when none runs.
    /// Read-only: observing never perturbs the take. `play_state` events reach
    /// the webview only, so this is the socket's sole window onto a running game.
    PlayStatus,

    // ── record ──────────────────────────────────────────────────────────
    /// Start a record session, optionally over a backing audio file.
    RecordStart { backing: Option<String> },
    /// Stop the record session without saving.
    RecordStop,
    /// Save the record session as a bundle; returns the bundle dir path.
    RecordSave,

    // ── backing / audio ─────────────────────────────────────────────────
    /// Attach a backing audio file by path.
    AttachBacking { path: String },
    /// Detach the backing audio file.
    DetachBacking,

    // ── sound / mixer (M14-C) ───────────────────────────────────────────
    /// Point one synth bus (the player's notes or the song's) at a curated
    /// instrument by id; returns the new mix.
    SetInstrument { bus: SynthBus, instrument: String },
    /// Set one bus's level in `0.0..=1.0` — the player's notes, the song, or
    /// the backing track; returns the new mix.
    SetBusGain { bus: MixerBus, gain: f32 },
    /// The current mix plus the selectable-instrument catalog.
    QueryMixer,

    // ── backing video ("the movie") ─────────────────────────────────────
    /// Attach (or replace) the background video by path, with an alignment
    /// offset (`videoTime = songTime + offset_us`). Persisted into the bundle
    /// on save. Returns the attached video reference.
    AttachVideo { path: String, offset_us: i64 },
    /// Update only the alignment offset of an already-attached video.
    SetVideoOffset { offset_us: i64 },
    /// Detach the background video.
    DetachVideo,
    /// The currently attached background video, or null.
    QueryVideo,

    // ── background images (M14-D) ───────────────────────────────────────
    /// Attach a background image by path as the front-most layer, selecting it.
    /// Persisted into the bundle on save. Returns the layer list.
    AttachBackground { path: String },
    /// Detach the background image layer with this id. Returns the layer list.
    DetachBackground { id: String },
    /// Every background layer with its transform evaluated at the playhead.
    QueryBackgrounds,

    // ── import ──────────────────────────────────────────────────────────
    /// Start importing from a URL.
    ImportStart { url: String },
    /// Start importing a local score file (MusicXML and friends), or a scan/PDF
    /// that an OMR engine transcribes first. One variant covers both: the
    /// sidecar decides which an input is, so the frontends never have to.
    ImportScore { path: String },

    // ── status / device (read-only) ─────────────────────────────────────
    /// Current audio/backing status as JSON.
    AudioStatus,
    /// Current MIDI input status as JSON.
    MidiStatus,
    /// Current record session status as JSON.
    RecordStatus,

    // ── app lifecycle ───────────────────────────────────────────────────
    /// Shut the application down gracefully (exit code 0). Lets an agent close
    /// the app over the socket; the connection closes as the process exits.
    AppQuit,
}

impl HostCommand {
    /// Stable snake_case name, identical to the serde `"command"` tag.
    ///
    /// e.g. `HostCommand::LoadBundle { .. }.name() == "load_bundle"`. A test
    /// enforces this parity against [`host_command_names`].
    pub fn name(&self) -> &'static str {
        match self {
            HostCommand::ScanLibrary => "scan_library",
            HostCommand::SaveBundle { .. } => "save_bundle",
            HostCommand::LoadBundle { .. } => "load_bundle",
            HostCommand::QueryDirty => "query_dirty",
            HostCommand::SplitBundle { .. } => "split_bundle",
            HostCommand::PlayLoad { .. } => "play_load",
            HostCommand::PlaySetWait { .. } => "play_set_wait",
            HostCommand::PlaySetRate { .. } => "play_set_rate",
            HostCommand::PlayToggleHearSong => "play_toggle_hear_song",
            HostCommand::PlayTogglePause => "play_toggle_pause",
            HostCommand::PlayFinish => "play_finish",
            HostCommand::PlayStatus => "play_status",
            HostCommand::RecordStart { .. } => "record_start",
            HostCommand::RecordStop => "record_stop",
            HostCommand::RecordSave => "record_save",
            HostCommand::AttachBacking { .. } => "attach_backing",
            HostCommand::DetachBacking => "detach_backing",
            HostCommand::SetInstrument { .. } => "set_instrument",
            HostCommand::SetBusGain { .. } => "set_bus_gain",
            HostCommand::QueryMixer => "query_mixer",
            HostCommand::AttachVideo { .. } => "attach_video",
            HostCommand::SetVideoOffset { .. } => "set_video_offset",
            HostCommand::DetachVideo => "detach_video",
            HostCommand::QueryVideo => "query_video",
            HostCommand::AttachBackground { .. } => "attach_background",
            HostCommand::DetachBackground { .. } => "detach_background",
            HostCommand::QueryBackgrounds => "query_backgrounds",
            HostCommand::ImportStart { .. } => "import_start",
            HostCommand::ImportScore { .. } => "import_score",
            HostCommand::AudioStatus => "audio_status",
            HostCommand::MidiStatus => "midi_status",
            HostCommand::RecordStatus => "record_status",
            HostCommand::AppQuit => "app_quit",
        }
    }
}

/// A frontend's host-command service surface — the **compiler-enforced seam**.
///
/// Each frontend implements [`dispatch`](HostServices::dispatch) with a single
/// exhaustive `match` on [`HostCommand`], calling its own existing
/// play/record/import/library/audio helpers. Because the match must be
/// exhaustive, adding a [`HostCommand`] variant fails to compile in every
/// frontend until it is handled — the API cannot silently drift behind the UI.
///
/// The protocol owns no host services itself; the channel-routed
/// [`crate::CommandServer`] hands `run_host_command` requests to the
/// application loop, which owns the play/record/etc. state and implements this
/// trait. The composer-owning [`crate::ControlServer`] has no host services and
/// rejects host commands (see [`crate::handle`]).
pub trait HostServices {
    /// Apply one host command against the frontend's own services.
    ///
    /// Returns a JSON value for `query`-style commands (status / dirty / info)
    /// or `Value::Null` for commands with no payload. Where a frontend genuinely
    /// cannot support a command it returns [`HostError::Unsupported`] — an
    /// explicit, still-exhaustive match arm.
    fn dispatch(&mut self, cmd: HostCommand) -> Result<serde_json::Value, HostError>;
}

/// A failed [`HostServices::dispatch`].
///
/// Kept light and dependency-free, mirroring `core::ActionError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The command is valid but this frontend cannot perform it.
    Unsupported(String),
    /// The command failed at runtime (I/O, no active session, …).
    Failed { command: String, detail: String },
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Unsupported(name) => write!(f, "unsupported host command: {name}"),
            HostError::Failed { command, detail } => {
                write!(f, "host command `{command}` failed: {detail}")
            }
        }
    }
}

impl std::error::Error for HostError {}

/// Build a [`HostCommand`] from a remote `run_host_command` request.
///
/// `params` is the JSON object of named fields (may be `null`/empty for nullary
/// commands). The command is reconstructed by splicing the name into a
/// serde-tagged value (`{"command": name, ...params}`) and deserialising, so the
/// name/serde-tag parity is the single source of truth — identical to
/// `core::action_from_name`.
///
/// Returns [`ActionError::UnknownAction`] when `name` is not in
/// [`host_command_names`], and [`ActionError::BadParams`] when `params` is not an
/// object/`null` or a field is missing or mistyped.
pub fn host_command_from_name(
    name: &str,
    params: &serde_json::Value,
) -> Result<HostCommand, ActionError> {
    if !host_command_names().contains(&name) {
        return Err(ActionError::UnknownAction(name.to_string()));
    }

    let mut obj = match params {
        serde_json::Value::Object(map) => map.clone(),
        serde_json::Value::Null => serde_json::Map::new(),
        other => {
            return Err(ActionError::BadParams {
                action: name.to_string(),
                detail: format!("params must be a JSON object or null, got `{other}`"),
            });
        }
    };
    // The tag wins over any stray `command` key the caller may have supplied.
    obj.insert(
        "command".to_string(),
        serde_json::Value::String(name.to_string()),
    );

    serde_json::from_value(serde_json::Value::Object(obj)).map_err(|e| ActionError::BadParams {
        action: name.to_string(),
        detail: e.to_string(),
    })
}

/// Every host-command name, for discovery and self-documentation.
///
/// Exhaustive and each entry round-trips through [`host_command_from_name`] (a
/// test enforces both), so it doubles as the wire catalog an agent can
/// enumerate. Mirrors `core::action_names`.
pub fn host_command_names() -> &'static [&'static str] {
    &[
        "scan_library",
        "save_bundle",
        "load_bundle",
        "query_dirty",
        "split_bundle",
        "play_load",
        "play_set_wait",
        "play_set_rate",
        "play_toggle_hear_song",
        "play_toggle_pause",
        "play_finish",
        "play_status",
        "record_start",
        "record_stop",
        "record_save",
        "attach_backing",
        "detach_backing",
        "set_instrument",
        "set_bus_gain",
        "query_mixer",
        "attach_video",
        "set_video_offset",
        "detach_video",
        "query_video",
        "attach_background",
        "detach_background",
        "query_backgrounds",
        "import_start",
        "import_score",
        "audio_status",
        "midi_status",
        "record_status",
        "app_quit",
    ]
}

/// Self-describing metadata for one [`HostCommand`]: its wire name, the
/// parameters it accepts, and a one-line human description.
///
/// The host-tier counterpart to `core::ActionInfo`; reuses `core::ParamInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HostCommandInfo {
    pub name: &'static str,
    pub params: &'static [ParamInfo],
    pub description: &'static str,
}

/// Structured, self-describing catalog of every host command — the host half of
/// `query help`. Mirrors [`host_command_names`] one-to-one (a test enforces
/// parity) and adds parameters + a one-line description per command.
pub fn host_help() -> &'static [HostCommandInfo] {
    HOST_HELP
}

const fn p(name: &'static str, ty: &'static str) -> ParamInfo {
    ParamInfo { name, ty }
}

/// The const catalog backing [`host_help`]. A `static` so all the nested
/// `&[ParamInfo]` slices promote to `'static`.
static HOST_HELP: &[HostCommandInfo] = {
    &[
        // ── library / bundles ───────────────────────────────────────────
        HostCommandInfo { name: "scan_library", params: &[], description: "Scan the default library roots and return the list of bundles." },
        HostCommandInfo { name: "save_bundle", params: &[p("dest", "SaveDest")], description: "Save the current timeline to a bundle; dest is {kind:\"quick_save\"} or {kind:\"library\",name:\"…\"}. Returns the bundle directory path." },
        HostCommandInfo { name: "load_bundle", params: &[p("dir", "String")], description: "Load a bundle directory into the composer, replacing its timeline. Returns the new snapshot." },
        HostCommandInfo { name: "query_dirty", params: &[], description: "Return whether the timeline has unsaved changes." },
        HostCommandInfo { name: "split_bundle", params: &[p("segments", "Vec<SegmentSpec>")], description: "Slice the loaded piece into the given kept parts, writing each as a new library bundle (subset MIDI + copied media + derived offsets, origin=Edited). Each segment is {start_us,end_us,name}; discarded parts are omitted (= trimming). Returns the created bundle directory paths; the source piece is left untouched." },
        // ── play ──────────────────────────────────────────────────────────
        HostCommandInfo { name: "play_load", params: &[p("dir", "String")], description: "Load a bundle directory as a play session. Returns play info." },
        HostCommandInfo { name: "play_set_wait", params: &[p("on", "bool")], description: "Arm (true) or disarm (false) note-by-note wait mode for the play session." },
        HostCommandInfo { name: "play_set_rate", params: &[p("rate_permille", "u16")], description: "Set play-session speed in permille (1000 = 1x, 500 = half speed), clamped 0.25x-2x. Slows the highway, wait gate and scoring together; the backing recording mutes below 1x." },
        HostCommandInfo { name: "play_toggle_hear_song", params: &[], description: "Toggle the audible song synth for the play session." },
        HostCommandInfo { name: "play_toggle_pause", params: &[], description: "Toggle pause on the active play session, freezing/thawing the clock and backing at the current position. No-op when no session is active." },
        HostCommandInfo { name: "play_finish", params: &[], description: "Finish the play session and return the score summary." },
        HostCommandInfo { name: "play_status", params: &[], description: "The live take's full state: clock, paused/frozen, wait gate (awaiting vs held pitches), practice hand + split, speed, score, and the chart's note count. Returns loaded:false when no take is running. Read-only — observing never perturbs the take." },
        // ── record ────────────────────────────────────────────────────────
        HostCommandInfo { name: "record_start", params: &[p("backing", "String?")], description: "Start a record session, optionally over a backing audio file path." },
        HostCommandInfo { name: "record_stop", params: &[], description: "Stop the record session without saving." },
        HostCommandInfo { name: "record_save", params: &[], description: "Save the record session as a bundle. Returns the bundle directory path." },
        // ── backing / audio ─────────────────────────────────────────────
        HostCommandInfo { name: "attach_backing", params: &[p("path", "String")], description: "Attach a backing audio file by path." },
        HostCommandInfo { name: "detach_backing", params: &[], description: "Detach the backing audio file." },
        // ── sound / mixer ───────────────────────────────────────────────
        HostCommandInfo { name: "set_instrument", params: &[p("bus", "SynthBus"), p("instrument", "String")], description: "Point a synth bus at a curated instrument by id; bus is \"player\" (the notes you play) or \"song\" (the auto-played chart). Call query_mixer for the selectable ids. Returns the new mix." },
        HostCommandInfo { name: "set_bus_gain", params: &[p("bus", "MixerBus"), p("gain", "f32")], description: "Set one bus's level, clamped to 0.0..=1.0; bus is \"player\", \"song\", or \"backing\". Returns the new mix." },
        HostCommandInfo { name: "query_mixer", params: &[], description: "Return the current mix (instrument + level per synth bus, plus the backing level) and the catalog of selectable instruments." },
        // ── backing video ("the movie") ─────────────────────────────────
        HostCommandInfo { name: "attach_video", params: &[p("path", "String"), p("offset_us", "i64")], description: "Attach (or replace) the background video by path, with an alignment offset (videoTime = songTime + offset_us). Persisted into the bundle on save. Returns the attached video reference." },
        HostCommandInfo { name: "set_video_offset", params: &[p("offset_us", "i64")], description: "Update only the alignment offset of the already-attached background video. Returns the attached video reference." },
        HostCommandInfo { name: "detach_video", params: &[], description: "Detach the background video." },
        HostCommandInfo { name: "query_video", params: &[], description: "Return the currently attached background video, or null." },
        // ── background images (M14-D) ───────────────────────────────────
        HostCommandInfo { name: "attach_background", params: &[p("path", "String")], description: "Attach a background image by path as the front-most layer and select it. The layer starts still (no keyframes); animate it with the background actions (nudge_background_pos/scale/rotation, set_background_opacity, add_background_keyframe). Persisted into the bundle on save. Returns the layer list." },
        HostCommandInfo { name: "detach_background", params: &[p("id", "String")], description: "Detach the background image layer with this id, dropping its keyframes. Returns the remaining layer list." },
        HostCommandInfo { name: "query_backgrounds", params: &[], description: "Return every background image layer — id, bundle file, absolute path, selection, keyframes, and the transform evaluated at the playhead." },
        // ── import ──────────────────────────────────────────────────────
        HostCommandInfo { name: "import_start", params: &[p("url", "String")], description: "Start importing audio/video from a URL." },
        HostCommandInfo { name: "import_score", params: &[p("path", "String")], description: "Start importing a local score file (MusicXML/.xml/.mxl/.abc/.krn) or a scan (.pdf/.png/.jpg/.jpeg/.tif/.tiff/.bmp). A score file is a deterministic transform whose notated tempo, metre and key seed the new bundle's grid. A scan goes through an optical music recognition engine first, which is lossy: its notes carry a derived confidence, the import log reports how many were flagged, and it needs an OMR engine installed (see docs/IMPORT.md)." },
        // ── status / device ──────────────────────────────────────────────
        HostCommandInfo { name: "audio_status", params: &[], description: "Return the current audio/backing status." },
        HostCommandInfo { name: "midi_status", params: &[], description: "Return the current MIDI input status." },
        HostCommandInfo { name: "record_status", params: &[], description: "Return the current record session status." },
        // ── app lifecycle ────────────────────────────────────────────────
        HostCommandInfo { name: "app_quit", params: &[], description: "Shut the application down gracefully (exit code 0). The connection closes as the process exits." },
    ]
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One sample of **every** [`HostCommand`] variant, minimal valid values for
    /// parametrised ones. The exhaustiveness oracle the parity tests cross-check
    /// against [`host_command_names`].
    fn all_variants() -> Vec<HostCommand> {
        vec![
            HostCommand::ScanLibrary,
            HostCommand::SaveBundle {
                dest: SaveDest::QuickSave,
            },
            HostCommand::LoadBundle {
                dir: "some/dir".into(),
            },
            HostCommand::QueryDirty,
            HostCommand::SplitBundle {
                segments: vec![
                    SegmentSpec {
                        start_us: 0,
                        end_us: 1_000_000,
                        name: "Verse".into(),
                    },
                    SegmentSpec {
                        start_us: 2_000_000,
                        end_us: 3_000_000,
                        name: "Chorus".into(),
                    },
                ],
            },
            HostCommand::PlayLoad {
                dir: "some/dir".into(),
            },
            HostCommand::PlaySetWait { on: true },
            HostCommand::PlaySetRate { rate_permille: 500 },
            HostCommand::PlayToggleHearSong,
            HostCommand::PlayTogglePause,
            HostCommand::PlayFinish,
            HostCommand::PlayStatus,
            HostCommand::RecordStart { backing: None },
            HostCommand::RecordStop,
            HostCommand::RecordSave,
            HostCommand::AttachBacking {
                path: "song.ogg".into(),
            },
            HostCommand::DetachBacking,
            HostCommand::SetInstrument {
                bus: SynthBus::Song,
                instrument: "marimba".into(),
            },
            HostCommand::SetBusGain {
                bus: MixerBus::Backing,
                gain: 0.5,
            },
            HostCommand::QueryMixer,
            HostCommand::AttachVideo {
                path: "movie.mp4".into(),
                offset_us: -100_000,
            },
            HostCommand::SetVideoOffset { offset_us: 50_000 },
            HostCommand::DetachVideo,
            HostCommand::QueryVideo,
            HostCommand::AttachBackground {
                path: "art.png".into(),
            },
            HostCommand::DetachBackground { id: "bg-0".into() },
            HostCommand::QueryBackgrounds,
            HostCommand::ImportStart {
                url: "https://example.com/v".into(),
            },
            HostCommand::ImportScore {
                path: "score.musicxml".into(),
            },
            HostCommand::AudioStatus,
            HostCommand::MidiStatus,
            HostCommand::RecordStatus,
            HostCommand::AppQuit,
        ]
    }

    #[test]
    fn every_variant_round_trips() {
        for cmd in all_variants() {
            let value = serde_json::to_value(&cmd).expect("serialises");
            let back: HostCommand = serde_json::from_value(value).expect("deserialises");
            assert_eq!(cmd, back, "round-trip mismatch for {}", cmd.name());
        }
    }

    #[test]
    fn name_matches_serde_tag() {
        for cmd in all_variants() {
            let value = serde_json::to_value(&cmd).expect("serialises");
            let tag = value
                .get("command")
                .and_then(|v| v.as_str())
                .expect("tagged with `command`");
            assert_eq!(tag, cmd.name(), "serde tag and name() disagree for {cmd:?}");
        }
    }

    #[test]
    fn host_command_names_is_exhaustive_and_matches_variants() {
        use std::collections::BTreeSet;
        let from_variants: BTreeSet<&str> = all_variants().iter().map(|c| c.name()).collect();
        let from_catalog: BTreeSet<&str> = host_command_names().iter().copied().collect();
        assert_eq!(
            from_variants, from_catalog,
            "host_command_names() must list exactly the HostCommand variants"
        );
    }

    #[test]
    fn host_help_matches_host_command_names_exactly() {
        use std::collections::BTreeSet;
        let from_help: BTreeSet<&str> = host_help().iter().map(|c| c.name).collect();
        let from_names: BTreeSet<&str> = host_command_names().iter().copied().collect();
        assert_eq!(
            from_help, from_names,
            "host_help() must describe exactly the names in host_command_names()"
        );
        assert_eq!(host_help().len(), host_command_names().len());
    }

    #[test]
    fn host_help_every_param_set_dispatches() {
        // Each described command, called with a minimal valid value per param,
        // must build via host_command_from_name — proving the documented param
        // names and the deserialiser agree.
        for info in host_help() {
            let mut params = serde_json::Map::new();
            for p in info.params {
                let sample = match p.ty {
                    "bool" => json!(true),
                    "i64" => json!(0),
                    "u16" => json!(1000),
                    "f32" => json!(0.5),
                    "SynthBus" | "MixerBus" => json!("player"),
                    "SaveDest" => json!({ "kind": "quick_save" }),
                    "Vec<SegmentSpec>" => {
                        json!([{ "start_us": 0, "end_us": 1, "name": "x" }])
                    }
                    // String, String?, and any other scalar accept a string.
                    _ => json!("x"),
                };
                params.insert(p.name.to_string(), sample);
            }
            host_command_from_name(info.name, &serde_json::Value::Object(params)).unwrap_or_else(
                |e| panic!("{} should dispatch from its help params: {e}", info.name),
            );
        }
    }

    #[test]
    fn host_help_descriptions_are_non_empty() {
        for info in host_help() {
            assert!(
                !info.description.is_empty(),
                "{} has an empty description",
                info.name
            );
        }
    }

    #[test]
    fn host_command_names_non_empty_and_unique() {
        use std::collections::BTreeSet;
        let names = host_command_names();
        assert!(!names.is_empty());
        let unique: BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "host_command_names() has duplicates"
        );
    }

    #[test]
    fn every_name_parses_via_host_command_from_name() {
        for cmd in all_variants() {
            let mut value = serde_json::to_value(&cmd).expect("serialises");
            let obj = value.as_object_mut().expect("tagged object");
            obj.remove("command");
            let params = serde_json::Value::Object(obj.clone());
            let parsed = host_command_from_name(cmd.name(), &params)
                .unwrap_or_else(|e| panic!("{} should parse: {e}", cmd.name()));
            assert_eq!(parsed, cmd);
        }
    }

    #[test]
    fn parametrised_dispatch() {
        assert_eq!(
            host_command_from_name("load_bundle", &json!({ "dir": "a/b" })).unwrap(),
            HostCommand::LoadBundle { dir: "a/b".into() }
        );
        assert_eq!(
            host_command_from_name("play_set_wait", &json!({ "on": false })).unwrap(),
            HostCommand::PlaySetWait { on: false }
        );
        assert_eq!(
            host_command_from_name(
                "save_bundle",
                &json!({ "dest": { "kind": "library", "name": "Tune" } })
            )
            .unwrap(),
            HostCommand::SaveBundle {
                dest: SaveDest::Library {
                    name: "Tune".into()
                }
            }
        );
        assert_eq!(
            host_command_from_name(
                "split_bundle",
                &json!({ "segments": [
                    { "start_us": 0, "end_us": 1_000_000, "name": "Intro" },
                    { "start_us": 1_000_000, "end_us": 2_000_000, "name": "Verse" }
                ] })
            )
            .unwrap(),
            HostCommand::SplitBundle {
                segments: vec![
                    SegmentSpec {
                        start_us: 0,
                        end_us: 1_000_000,
                        name: "Intro".into()
                    },
                    SegmentSpec {
                        start_us: 1_000_000,
                        end_us: 2_000_000,
                        name: "Verse".into()
                    },
                ]
            }
        );
    }

    #[test]
    fn mixer_commands_parse_their_typed_buses() {
        assert_eq!(
            host_command_from_name(
                "set_instrument",
                &json!({ "bus": "player", "instrument": "flute" })
            )
            .unwrap(),
            HostCommand::SetInstrument {
                bus: SynthBus::Player,
                instrument: "flute".into()
            }
        );
        assert_eq!(
            host_command_from_name("set_bus_gain", &json!({ "bus": "backing", "gain": 0.25 }))
                .unwrap(),
            HostCommand::SetBusGain {
                bus: MixerBus::Backing,
                gain: 0.25
            }
        );
    }

    #[test]
    fn set_instrument_rejects_the_backing_bus_at_the_type_level() {
        // `backing` is an audio sink, not a synth voice: it has a level but no
        // instrument, and `SynthBus` simply cannot name it.
        let err = host_command_from_name(
            "set_instrument",
            &json!({ "bus": "backing", "instrument": "flute" }),
        )
        .unwrap_err();
        match err {
            ActionError::BadParams { action, .. } => assert_eq!(action, "set_instrument"),
            other => panic!("expected BadParams, got {other:?}"),
        }
    }

    #[test]
    fn record_start_backing_optional() {
        assert_eq!(
            host_command_from_name("record_start", &json!({})).unwrap(),
            HostCommand::RecordStart { backing: None }
        );
        assert_eq!(
            host_command_from_name("record_start", &json!({ "backing": "b.ogg" })).unwrap(),
            HostCommand::RecordStart {
                backing: Some("b.ogg".into())
            }
        );
    }

    #[test]
    fn nullary_dispatch_accepts_empty_or_null_params() {
        assert_eq!(
            host_command_from_name("scan_library", &json!({})).unwrap(),
            HostCommand::ScanLibrary
        );
        assert_eq!(
            host_command_from_name("scan_library", &serde_json::Value::Null).unwrap(),
            HostCommand::ScanLibrary
        );
    }

    #[test]
    fn unknown_name_is_rejected() {
        let err = host_command_from_name("frobnicate", &json!({})).unwrap_err();
        assert_eq!(err, ActionError::UnknownAction("frobnicate".to_string()));
    }

    #[test]
    fn wrong_param_type_is_bad_params() {
        let err = host_command_from_name("play_set_wait", &json!({ "on": "yes" })).unwrap_err();
        match err {
            ActionError::BadParams { action, .. } => assert_eq!(action, "play_set_wait"),
            other => panic!("expected BadParams, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_param_is_bad_params() {
        let err = host_command_from_name("load_bundle", &json!({})).unwrap_err();
        assert!(matches!(err, ActionError::BadParams { .. }));
    }

    #[test]
    fn non_object_params_is_bad_params() {
        let err = host_command_from_name("scan_library", &json!([1, 2, 3])).unwrap_err();
        match err {
            ActionError::BadParams { action, .. } => assert_eq!(action, "scan_library"),
            other => panic!("expected BadParams, got {other:?}"),
        }
    }

    #[test]
    fn host_error_displays() {
        assert_eq!(
            HostError::Unsupported("import_start".to_string()).to_string(),
            "unsupported host command: import_start"
        );
        assert_eq!(
            HostError::Failed {
                command: "record_save".to_string(),
                detail: "no recording in progress".to_string(),
            }
            .to_string(),
            "host command `record_save` failed: no recording in progress"
        );
    }

    /// A fake `HostServices` proving the seam dispatches and reports errors.
    struct FakeHost {
        dirty: bool,
    }

    impl HostServices for FakeHost {
        fn dispatch(&mut self, cmd: HostCommand) -> Result<serde_json::Value, HostError> {
            match cmd {
                HostCommand::QueryDirty => Ok(json!(self.dirty)),
                HostCommand::LoadBundle { dir } => {
                    self.dirty = false;
                    Ok(json!({ "loaded": dir }))
                }
                HostCommand::ImportStart { .. } => {
                    Err(HostError::Unsupported("import_start".to_string()))
                }
                // Every other arm is a no-op for this fake.
                other => Err(HostError::Failed {
                    command: other.name().to_string(),
                    detail: "not implemented in fake".to_string(),
                }),
            }
        }
    }

    #[test]
    fn fake_host_dispatch_round_trips_value_and_error() {
        let mut host = FakeHost { dirty: true };
        assert_eq!(host.dispatch(HostCommand::QueryDirty).unwrap(), json!(true));
        host.dispatch(HostCommand::LoadBundle { dir: "x".into() })
            .unwrap();
        assert_eq!(
            host.dispatch(HostCommand::QueryDirty).unwrap(),
            json!(false)
        );
        assert_eq!(
            host.dispatch(HostCommand::ImportStart { url: "u".into() })
                .unwrap_err(),
            HostError::Unsupported("import_start".to_string())
        );
    }
}
