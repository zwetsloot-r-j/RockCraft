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

use rockcraft_core::{ActionError, ParamInfo};
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

    // ── play ────────────────────────────────────────────────────────────
    /// Load a bundle dir as a play session; returns play info.
    PlayLoad { dir: String },
    /// Arm/disarm note-by-note wait mode for the play session.
    PlaySetWait { on: bool },
    /// Toggle "hear the song" (audible song synth) for the play session.
    PlayToggleHearSong,
    /// Finish the play session; returns the score summary.
    PlayFinish,

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

    // ── import ──────────────────────────────────────────────────────────
    /// Start importing from a URL.
    ImportStart { url: String },

    // ── status / device (read-only) ─────────────────────────────────────
    /// Current audio/backing status as JSON.
    AudioStatus,
    /// Current MIDI input status as JSON.
    MidiStatus,
    /// Current record session status as JSON.
    RecordStatus,
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
            HostCommand::PlayLoad { .. } => "play_load",
            HostCommand::PlaySetWait { .. } => "play_set_wait",
            HostCommand::PlayToggleHearSong => "play_toggle_hear_song",
            HostCommand::PlayFinish => "play_finish",
            HostCommand::RecordStart { .. } => "record_start",
            HostCommand::RecordStop => "record_stop",
            HostCommand::RecordSave => "record_save",
            HostCommand::AttachBacking { .. } => "attach_backing",
            HostCommand::DetachBacking => "detach_backing",
            HostCommand::ImportStart { .. } => "import_start",
            HostCommand::AudioStatus => "audio_status",
            HostCommand::MidiStatus => "midi_status",
            HostCommand::RecordStatus => "record_status",
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
        "play_load",
        "play_set_wait",
        "play_toggle_hear_song",
        "play_finish",
        "record_start",
        "record_stop",
        "record_save",
        "attach_backing",
        "detach_backing",
        "import_start",
        "audio_status",
        "midi_status",
        "record_status",
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
        // ── play ──────────────────────────────────────────────────────────
        HostCommandInfo { name: "play_load", params: &[p("dir", "String")], description: "Load a bundle directory as a play session. Returns play info." },
        HostCommandInfo { name: "play_set_wait", params: &[p("on", "bool")], description: "Arm (true) or disarm (false) note-by-note wait mode for the play session." },
        HostCommandInfo { name: "play_toggle_hear_song", params: &[], description: "Toggle the audible song synth for the play session." },
        HostCommandInfo { name: "play_finish", params: &[], description: "Finish the play session and return the score summary." },
        // ── record ────────────────────────────────────────────────────────
        HostCommandInfo { name: "record_start", params: &[p("backing", "String?")], description: "Start a record session, optionally over a backing audio file path." },
        HostCommandInfo { name: "record_stop", params: &[], description: "Stop the record session without saving." },
        HostCommandInfo { name: "record_save", params: &[], description: "Save the record session as a bundle. Returns the bundle directory path." },
        // ── backing / audio ─────────────────────────────────────────────
        HostCommandInfo { name: "attach_backing", params: &[p("path", "String")], description: "Attach a backing audio file by path." },
        HostCommandInfo { name: "detach_backing", params: &[], description: "Detach the backing audio file." },
        // ── import ──────────────────────────────────────────────────────
        HostCommandInfo { name: "import_start", params: &[p("url", "String")], description: "Start importing audio/video from a URL." },
        // ── status / device ──────────────────────────────────────────────
        HostCommandInfo { name: "audio_status", params: &[], description: "Return the current audio/backing status." },
        HostCommandInfo { name: "midi_status", params: &[], description: "Return the current MIDI input status." },
        HostCommandInfo { name: "record_status", params: &[], description: "Return the current record session status." },
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
            HostCommand::PlayLoad {
                dir: "some/dir".into(),
            },
            HostCommand::PlaySetWait { on: true },
            HostCommand::PlayToggleHearSong,
            HostCommand::PlayFinish,
            HostCommand::RecordStart { backing: None },
            HostCommand::RecordStop,
            HostCommand::RecordSave,
            HostCommand::AttachBacking {
                path: "song.ogg".into(),
            },
            HostCommand::DetachBacking,
            HostCommand::ImportStart {
                url: "https://example.com/v".into(),
            },
            HostCommand::AudioStatus,
            HostCommand::MidiStatus,
            HostCommand::RecordStatus,
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
                    "SaveDest" => json!({ "kind": "quick_save" }),
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
