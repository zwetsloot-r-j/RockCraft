//! The **host-command tier** — app-level workflows alongside composer actions.
//!
//! Where [`crate::protocol`]'s `run_action` drives the pure [`Composer`] (a
//! [`rockcraft_core::Action`] — no I/O, by [`core`]'s invariant), a *host
//! command* drives the **frontend's own services**: load a song to play, run a
//! record session, attach/detach a backing track, import from a URL, scan / save
//! / load the library. These touch disk, the MIDI device, audio, and
//! subprocesses, so they can **never** be `core::Action`s — they live here as a
//! parallel vocabulary that each frontend dispatches over its services.
//!
//! ## The drift backstop
//!
//! A host command is two things kept in lockstep:
//!
//! 1. [`HostCommand`] — the typed, serde-tagged request a client sends (parsed
//!    by [`host_command_from_name`], catalogued by [`host_command_help`]).
//! 2. A method on the [`HostServices`] trait each frontend implements.
//!
//! [`dispatch`] is the single seam between them, and its `match` over
//! [`HostCommand`] is **exhaustive** — so adding a variant fails to compile
//! until both a `dispatch` arm *and* a [`HostServices`] method exist for it.
//! That is the compiler-enforced guarantee that a new command cannot silently
//! drift behind the UI. [`query help`](crate::protocol::QueryKind::Help) lists
//! this catalog beside the action catalog, so one query discovers both tiers.
//!
//! [`Composer`]: rockcraft_core::Composer
//! [`core`]: rockcraft_core
//! [`query help`]: crate::protocol

use rockcraft_core::ParamInfo;
use serde::{Deserialize, Serialize};

/// An app-level workflow command — the host-services counterpart to
/// [`rockcraft_core::Action`].
///
/// Serialises with a `command` tag carrying the snake_case name, so the wire
/// form mirrors `run_action`'s `{ "command": "play_load", "dir": "…" }`. The
/// names round-trip through [`host_command_from_name`] / [`host_command_names`]
/// (a test enforces parity), so the enum is the single source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum HostCommand {
    /// Load the bundle at `dir` and start playing it (the note highway).
    PlayLoad { dir: String },
    /// Stop / tear down the active play session.
    PlayStop,
    /// Begin a record session, optionally with a backing track playing under it.
    RecordStart {
        #[serde(default)]
        backing: Option<String>,
    },
    /// Stop the active record session, discarding the take.
    RecordStop,
    /// Save the active record session to a bundle (the session stays open).
    RecordSave,
    /// Attach a backing audio file to the session.
    BackingAttach { path: String },
    /// Detach the current backing audio file.
    BackingDetach,
    /// Import a song from a video URL (runs the M6 import pipeline).
    ImportUrl { url: String },
    /// Scan the library roots and list the bundles found.
    LibraryScan,
    /// Save the current composition into the library under `name`.
    LibrarySave { name: String },
    /// Load the bundle at `dir` into the composer for editing.
    LibraryLoad { dir: String },
}

/// Why a `run_command` request could not be turned into a [`HostCommand`].
///
/// The host-tier mirror of [`rockcraft_core::ActionError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCommandError {
    /// No command is registered under this name.
    UnknownCommand(String),
    /// The command exists, but its parameters were missing or the wrong shape.
    BadParams { command: String, detail: String },
}

impl std::fmt::Display for HostCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostCommandError::UnknownCommand(name) => write!(f, "unknown command: {name}"),
            HostCommandError::BadParams { command, detail } => {
                write!(f, "bad params for command `{command}`: {detail}")
            }
        }
    }
}

impl std::error::Error for HostCommandError {}

/// Build a [`HostCommand`] from a remote `run_command` request.
///
/// Mirrors [`rockcraft_core::action_from_name`]: `params` is the JSON object of
/// named fields (may be `null` or empty for nullary commands). The command is
/// reconstructed by splicing the name into a serde-tagged value
/// (`{"command": name, ...params}`) and deserialising, so name/serde-tag parity
/// is the single source of truth.
///
/// Returns [`HostCommandError::UnknownCommand`] when `name` is not in
/// [`host_command_names`], and [`HostCommandError::BadParams`] when `params` is
/// not an object/`null` or a field is missing or mistyped.
pub fn host_command_from_name(
    name: &str,
    params: &serde_json::Value,
) -> Result<HostCommand, HostCommandError> {
    if !host_command_names().contains(&name) {
        return Err(HostCommandError::UnknownCommand(name.to_string()));
    }

    // Splice the params into a serde-tagged object: {"command": name, ...params}.
    let mut obj = match params {
        serde_json::Value::Object(map) => map.clone(),
        serde_json::Value::Null => serde_json::Map::new(),
        other => {
            return Err(HostCommandError::BadParams {
                command: name.to_string(),
                detail: format!("params must be a JSON object or null, got `{other}`"),
            });
        }
    };
    // The tag wins over any stray `command` key the caller may have supplied.
    obj.insert(
        "command".to_string(),
        serde_json::Value::String(name.to_string()),
    );

    serde_json::from_value(serde_json::Value::Object(obj)).map_err(|e| {
        HostCommandError::BadParams {
            command: name.to_string(),
            detail: e.to_string(),
        }
    })
}

/// Every host-command name, for discovery and self-documentation.
///
/// Exhaustive and parity-tested against [`HostCommand`] and
/// [`host_command_help`], so it doubles as the wire catalog a remote client can
/// enumerate.
pub fn host_command_names() -> &'static [&'static str] {
    &[
        "play_load",
        "play_stop",
        "record_start",
        "record_stop",
        "record_save",
        "backing_attach",
        "backing_detach",
        "import_url",
        "library_scan",
        "library_save",
        "library_load",
    ]
}

/// Self-describing metadata for one [`HostCommand`]: its wire name, the
/// parameters it accepts, and a one-line human description.
///
/// The host-tier mirror of [`rockcraft_core::ActionInfo`] — it reuses
/// [`ParamInfo`] so `query help` presents both tiers with one uniform schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HostCommandInfo {
    pub name: &'static str,
    pub params: &'static [ParamInfo],
    pub description: &'static str,
}

/// Structured, self-describing catalog of every host command — the host-tier
/// half of `query help`. Mirrors [`host_command_names`] one-to-one (a test
/// enforces parity) and adds parameters + a one-line description per command.
pub fn host_command_help() -> &'static [HostCommandInfo] {
    HOST_COMMAND_HELP
}

const fn p(name: &'static str, ty: &'static str) -> ParamInfo {
    ParamInfo { name, ty }
}

/// The const catalog backing [`host_command_help`]. A `static` so the nested
/// `&[ParamInfo]` slices promote to `'static` rather than being temporaries.
static HOST_COMMAND_HELP: &[HostCommandInfo] = {
    &[
        HostCommandInfo { name: "play_load", params: &[p("dir", "string")], description: "Load the bundle directory `dir` and start playing it on the note highway." },
        HostCommandInfo { name: "play_stop", params: &[], description: "Stop and tear down the active play session." },
        HostCommandInfo { name: "record_start", params: &[p("backing", "string?")], description: "Begin a record session; if `backing` is given, that audio file plays under the take." },
        HostCommandInfo { name: "record_stop", params: &[], description: "Stop the active record session, discarding the captured take." },
        HostCommandInfo { name: "record_save", params: &[], description: "Save the active record session to a bundle; the session stays open. Returns the bundle directory." },
        HostCommandInfo { name: "backing_attach", params: &[p("path", "string")], description: "Attach the backing audio file at `path` to the session." },
        HostCommandInfo { name: "backing_detach", params: &[], description: "Detach the current backing audio file." },
        HostCommandInfo { name: "import_url", params: &[p("url", "string")], description: "Import a song from the video `url` (runs the import pipeline). Requires a fetch command to be configured." },
        HostCommandInfo { name: "library_scan", params: &[], description: "Scan the library roots and return the bundles found (name, dir, note_count, duration_us, origin, has_backing)." },
        HostCommandInfo { name: "library_save", params: &[p("name", "string")], description: "Save the current composition into the library under `name`. Returns the bundle directory." },
        HostCommandInfo { name: "library_load", params: &[p("dir", "string")], description: "Load the bundle at `dir` into the composer for editing." },
    ]
};

/// The data a successful host command returns, or the error string it failed
/// with. The `Ok` payload is command-specific JSON (e.g. the saved bundle path,
/// or the library listing); nullary side effects return [`serde_json::Value::Null`].
pub type HostResult = Result<serde_json::Value, String>;

/// The frontend services a host command drives.
///
/// Each frontend (TUI, Tauri, …) implements this over its own session, screens,
/// and audio/MIDI/disk services. The methods are intentionally I/O-bearing —
/// that is exactly why they live in a frontend trait and not in `core`.
///
/// Implementations should return `Err(message)` (not panic) for a precondition
/// that is not met — e.g. `record_save` with no active session — so the agent
/// receives a protocol error rather than the connection dropping.
pub trait HostServices {
    /// Load `dir` and start playing it. Returns info about the loaded song.
    fn play_load(&mut self, dir: &str) -> HostResult;
    /// Stop / tear down the active play session.
    fn play_stop(&mut self) -> HostResult;
    /// Begin a record session, optionally with `backing` playing under it.
    fn record_start(&mut self, backing: Option<&str>) -> HostResult;
    /// Stop the active record session, discarding the take.
    fn record_stop(&mut self) -> HostResult;
    /// Save the active record session to a bundle. Returns the bundle dir.
    fn record_save(&mut self) -> HostResult;
    /// Attach the backing audio file at `path`.
    fn backing_attach(&mut self, path: &str) -> HostResult;
    /// Detach the current backing audio file.
    fn backing_detach(&mut self) -> HostResult;
    /// Import a song from the video `url`.
    fn import_url(&mut self, url: &str) -> HostResult;
    /// Scan the library roots and return the listing.
    fn library_scan(&mut self) -> HostResult;
    /// Save the current composition into the library under `name`.
    fn library_save(&mut self, name: &str) -> HostResult;
    /// Load the bundle at `dir` into the composer.
    fn library_load(&mut self, dir: &str) -> HostResult;
}

/// Route a parsed [`HostCommand`] to the matching [`HostServices`] method.
///
/// **This `match` is the drift backstop.** It is exhaustive over [`HostCommand`],
/// so adding a variant fails to compile here until an arm — and therefore a
/// trait method — exists for it. A new command can never be catalogued (and
/// shown by `query help`) without also being dispatchable by every frontend.
pub fn dispatch(host: &mut dyn HostServices, command: HostCommand) -> HostResult {
    match command {
        HostCommand::PlayLoad { dir } => host.play_load(&dir),
        HostCommand::PlayStop => host.play_stop(),
        HostCommand::RecordStart { backing } => host.record_start(backing.as_deref()),
        HostCommand::RecordStop => host.record_stop(),
        HostCommand::RecordSave => host.record_save(),
        HostCommand::BackingAttach { path } => host.backing_attach(&path),
        HostCommand::BackingDetach => host.backing_detach(),
        HostCommand::ImportUrl { url } => host.import_url(&url),
        HostCommand::LibraryScan => host.library_scan(),
        HostCommand::LibrarySave { name } => host.library_save(&name),
        HostCommand::LibraryLoad { dir } => host.library_load(&dir),
    }
}
