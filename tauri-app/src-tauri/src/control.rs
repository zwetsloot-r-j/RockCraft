//! Optional localhost WebSocket control socket for the Tauri app (M7-tauri-O).
//!
//! The TUI already exposes the `rockcraft-control` protocol over a socket
//! (`crates/tui/src/main.rs`); the desktop app previously only re-exposed the
//! same `run_action` / `query state|help` vocabulary over Tauri's internal IPC
//! `invoke`, reachable from its own webview but **not over a socket**. This
//! module wires the existing [`CommandServer`] into the Tauri backend so an
//! external agent can drive the desktop app exactly as it drives the TUI.
//!
//! Design — additive, loopback-only, default-off:
//! - It starts only when `--control` is passed or `ROCKCRAFT_CONTROL_ADDR` is
//!   set (the same opt-in as the TUI). Normal launches are socket-free.
//! - It binds the **same** [`CommandServer`] that the TUI uses, which forwards
//!   each parsed [`Request`] over an `mpsc` channel as a [`RemoteCommand`]. An
//!   *applier* thread drains that channel and applies each request — via the
//!   shared [`handle_with_host`] — against the **one** live [`Composer`] the
//!   backend owns ([`AppState`]), the same composer the IPC commands and tick
//!   thread mutate.
//! - After an agent-driven action it emits the `snapshot` event, mirroring the
//!   IPC `run_action` path, so the webview reflects remote edits. Playback audio
//!   and the transport already flow through the tick thread (`lib.rs`), which
//!   reads the same shared composer, so a remote `play_*` action sounds and
//!   scrolls without any extra wiring here.
//!
//! App-level workflow commands (play/record/import/library/backing) ride the
//! **M8-A host-command tier** on the same socket: [`Request::RunHostCommand`] is
//! parsed into a [`HostCommand`] and dispatched through [`TauriHost`], which
//! implements [`HostServices`] over the backend's existing `play.rs`/`record.rs`/
//! `audio.rs`/`import.rs`/`library.rs`/`state.rs` helpers. The exhaustive `match`
//! there is the compiler-enforced seam: a new [`HostCommand`] variant fails to
//! compile until this frontend handles it. Composer `run_action`s keep their own
//! path; the two tiers coexist.
//!
//! [`Composer`]: rockcraft_core::Composer

use std::net::SocketAddr;

use rockcraft_control::{
    handle_with_host, CommandServer, HostCommand, HostError, HostServices, RemoteCommand, Request,
    Response, SaveDest, SegmentSpec,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use crate::audio::AudioState;
use crate::import::{ImportInputDto, ImportRunning};
use crate::midi::MidiState;
use crate::play::PlayState;
use crate::record::RecordState;
use crate::state::AppState;
use crate::EVENT_SNAPSHOT;

/// Default bind address: an OS-assigned loopback port. Override with
/// `ROCKCRAFT_CONTROL_ADDR` (must stay loopback — the server refuses other
/// interfaces). Mirrors the TUI constant.
const DEFAULT_CONTROL_ADDR: &str = "127.0.0.1:0";

/// Capacity of the socket→applier command channel. Small: requests are applied
/// immediately, so this only buffers a brief burst. Mirrors the TUI constant.
const CONTROL_CHANNEL_CAP: usize = 64;

/// Whether the control socket should start, from CLI args / env. Mirrors the
/// TUI's opt-in exactly so the same launch flags drive either frontend.
pub fn enabled() -> bool {
    std::env::args().any(|a| a == "--control") || std::env::var("ROCKCRAFT_CONTROL_ADDR").is_ok()
}

/// Start the control socket when [`enabled`]. Spawns the tokio server thread
/// (forwards requests over a channel) and the applier thread (applies them to
/// the shared composer and replies). A bind failure is logged and otherwise
/// ignored — the app keeps running without a control socket.
pub fn maybe_start(app: &AppHandle) {
    if !enabled() {
        return;
    }
    let (cmd_tx, cmd_rx) = mpsc::channel::<RemoteCommand>(CONTROL_CHANNEL_CAP);
    match spawn_server(cmd_tx) {
        Ok(addr) => eprintln!("Control server listening on ws://{addr}"),
        Err(e) => {
            eprintln!("Control server disabled: {e}");
            return;
        }
    }
    spawn_applier(app.clone(), cmd_rx);
}

/// Spawn the control server on its own thread with a current-thread tokio
/// runtime (tokio stays off the Tauri/main loop). Blocks only until the bind
/// result is known so the caller can report the port (or the failure)
/// synchronously, then serves for the lifetime of the process.
fn spawn_server(cmd_tx: mpsc::Sender<RemoteCommand>) -> std::io::Result<SocketAddr> {
    let addr = std::env::var("ROCKCRAFT_CONTROL_ADDR")
        .unwrap_or_else(|_| DEFAULT_CONTROL_ADDR.to_string());
    let (bound_tx, bound_rx) = std::sync::mpsc::channel::<std::io::Result<SocketAddr>>();

    std::thread::Builder::new()
        .name("rockcraft-control".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = bound_tx.send(Err(e));
                    return;
                }
            };
            rt.block_on(async move {
                match CommandServer::bind(&addr, cmd_tx).await {
                    Ok(server) => {
                        let _ = bound_tx.send(Ok(server.local_addr()));
                        let _ = server.serve().await;
                    }
                    Err(e) => {
                        let _ = bound_tx.send(Err(e));
                    }
                }
            });
        })?;

    match bound_rx.recv() {
        Ok(res) => res,
        Err(_) => Err(std::io::Error::other(
            "control server thread exited before binding",
        )),
    }
}

/// Spawn the applier thread: drain the command channel and apply each request
/// against the shared composer, replying over the request's one-shot.
///
/// Uses a dedicated std thread with blocking receives so it never touches the
/// tokio runtime or the render loop. The composer lock is held only for the
/// synchronous [`handle_with_host`] call.
fn spawn_applier(app: AppHandle, mut cmd_rx: mpsc::Receiver<RemoteCommand>) {
    std::thread::Builder::new()
        .name("rockcraft-control-apply".into())
        .spawn(move || {
            while let Some(cmd) = cmd_rx.blocking_recv() {
                let is_host = matches!(cmd.req, Request::RunHostCommand { .. });
                let is_action = matches!(cmd.req, Request::RunAction { .. });
                let mut host = TauriHost { app: &app };
                let response = apply_request(&app.state::<AppState>(), Some(&mut host), cmd.req);
                // Keep the webview in sync with agent-driven changes, mirroring
                // the IPC emit. A composer `run_action` returns the post-edit
                // snapshot inline; a host command (e.g. LoadBundle) changes the
                // composer but returns its own value, so re-read and emit the
                // current snapshot. Queries emit nothing (no redundant spam).
                match &response {
                    Response::Ok {
                        state: Some(snapshot),
                        ..
                    } if is_action => {
                        let _ = app.emit(EVENT_SNAPSHOT, snapshot);
                    }
                    Response::HostResult { .. } if is_host => {
                        let snapshot = crate::state::query_state(&app.state::<AppState>());
                        let _ = app.emit(EVENT_SNAPSHOT, &snapshot);
                    }
                    _ => {}
                }
                // The client may have gone away; a failed reply is not fatal.
                let _ = cmd.reply.send(response);
            }
        })
        .expect("spawn control applier thread");
}

/// Apply one [`Request`] against the shared composer (and, for host commands,
/// `host`) and return the wire [`Response`]. Factored out of [`spawn_applier`]
/// so the integration seam (AppState composer ↔ control protocol) is
/// unit-testable without a running Tauri app — composer tests pass `host: None`.
fn apply_request(state: &AppState, host: Option<&mut dyn HostServices>, req: Request) -> Response {
    let mut composer = state.composer.lock().expect("composer mutex poisoned");
    handle_with_host(&mut composer, host, req)
}

/// The Tauri frontend's [`HostServices`] — the app-level command tier (M8-A).
///
/// Holds the [`AppHandle`] so each command can reach the managed state it needs
/// (`PlayState`/`RecordState`/`AudioState`/`MidiState`/`ImportRunning`/
/// `AppState`) and call the **existing** command bodies. The single exhaustive
/// `match` is the compiler-enforced seam (per `CLAUDE.md`): a new
/// [`HostCommand`] variant won't compile until it is handled here.
struct TauriHost<'a> {
    app: &'a AppHandle,
}

/// Serialise a command's return payload to JSON, mapping a (rare) serialisation
/// failure to a [`HostError::Failed`] tagged with the command name.
fn json_payload<T: serde::Serialize>(
    command: &str,
    value: T,
) -> Result<serde_json::Value, HostError> {
    serde_json::to_value(value).map_err(|e| HostError::Failed {
        command: command.to_string(),
        detail: e.to_string(),
    })
}

/// Wrap a frontend command's `Err(String)` into a [`HostError::Failed`].
fn failed(command: &str, detail: String) -> Result<serde_json::Value, HostError> {
    Err(HostError::Failed {
        command: command.to_string(),
        detail,
    })
}

impl HostServices for TauriHost<'_> {
    fn dispatch(&mut self, cmd: HostCommand) -> Result<serde_json::Value, HostError> {
        let app = self.app;
        match cmd {
            // ── library / bundles ───────────────────────────────────────
            HostCommand::ScanLibrary => {
                let entries = crate::library::scan_library_inner(
                    &rockcraft_midi::bundle::default_scan_roots(),
                );
                json_payload("scan_library", entries)
            }
            HostCommand::SaveBundle { dest } => {
                match crate::state::save_bundle(&app.state::<AppState>(), map_save_dest(dest)) {
                    Ok(path) => Ok(serde_json::json!({ "dir": path })),
                    Err(e) => failed("save_bundle", e),
                }
            }
            HostCommand::LoadBundle { dir } => {
                match crate::state::load_bundle(&app.state::<AppState>(), &dir) {
                    Ok(snapshot) => json_payload("load_bundle", snapshot),
                    Err(e) => failed("load_bundle", e),
                }
            }
            HostCommand::QueryDirty => Ok(serde_json::json!(crate::state::query_dirty(
                &app.state::<AppState>()
            ))),
            HostCommand::SplitBundle { segments } => {
                let segs = segments.into_iter().map(map_segment_spec).collect();
                match crate::state::split_bundle(&app.state::<AppState>(), segs) {
                    Ok(dirs) => Ok(serde_json::json!({ "dirs": dirs })),
                    Err(e) => failed("split_bundle", e),
                }
            }

            // ── play ────────────────────────────────────────────────────
            HostCommand::PlayLoad { dir } => {
                match crate::play::play_load(
                    app.state::<PlayState>(),
                    app.state::<AudioState>(),
                    dir,
                ) {
                    Ok(info) => json_payload("play_load", info),
                    Err(e) => failed("play_load", e),
                }
            }
            HostCommand::PlaySetWait { on } => {
                let armed = crate::play::play_set_wait(app.state::<PlayState>(), on);
                Ok(serde_json::json!({ "wait": armed }))
            }
            HostCommand::PlayToggleHearSong => {
                let on = crate::play::play_toggle_hear_song(
                    app.state::<PlayState>(),
                    app.state::<AudioState>(),
                );
                Ok(serde_json::json!({ "hear_song": on }))
            }
            HostCommand::PlayFinish => {
                let summary =
                    crate::play::play_finish(app.state::<PlayState>(), app.state::<AudioState>());
                json_payload("play_finish", summary)
            }

            // ── record ──────────────────────────────────────────────────
            HostCommand::RecordStart { backing } => {
                match crate::record::record_start(
                    app.state::<RecordState>(),
                    app.state::<AudioState>(),
                    backing,
                ) {
                    Ok(()) => Ok(serde_json::Value::Null),
                    Err(e) => failed("record_start", e),
                }
            }
            HostCommand::RecordStop => {
                crate::record::record_stop(app.state::<RecordState>());
                Ok(serde_json::Value::Null)
            }
            HostCommand::RecordSave => {
                match crate::record::record_save(app.state::<RecordState>()) {
                    Ok(path) => Ok(serde_json::json!({ "dir": path })),
                    Err(e) => failed("record_save", e),
                }
            }

            // ── backing / audio ─────────────────────────────────────────
            HostCommand::AttachBacking { path } => {
                match crate::audio::attach_backing(app.state::<AudioState>(), path) {
                    Ok(()) => Ok(serde_json::Value::Null),
                    Err(e) => failed("attach_backing", e),
                }
            }
            HostCommand::DetachBacking => {
                crate::audio::detach_backing(app.state::<AudioState>());
                Ok(serde_json::Value::Null)
            }

            // ── backing video ("the movie") ─────────────────────────────
            HostCommand::AttachVideo { path, offset_us } => {
                let state = app.state::<AppState>();
                crate::state::set_video(&state, path, offset_us);
                json_payload("attach_video", crate::state::query_video(&state))
            }
            HostCommand::SetVideoOffset { offset_us } => {
                let state = app.state::<AppState>();
                crate::state::set_video_offset(&state, offset_us);
                json_payload("set_video_offset", crate::state::query_video(&state))
            }
            HostCommand::DetachVideo => {
                crate::state::clear_video(&app.state::<AppState>());
                Ok(serde_json::Value::Null)
            }
            HostCommand::QueryVideo => json_payload(
                "query_video",
                crate::state::query_video(&app.state::<AppState>()),
            ),

            // ── import ──────────────────────────────────────────────────
            HostCommand::ImportStart { url } => {
                match crate::import::import_start(
                    app.clone(),
                    app.state::<ImportRunning>(),
                    ImportInputDto::Url(url),
                ) {
                    Ok(()) => Ok(serde_json::Value::Null),
                    Err(e) => failed("import_start", e),
                }
            }

            // ── status / device (read-only) ─────────────────────────────
            HostCommand::AudioStatus => json_payload(
                "audio_status",
                crate::audio::audio_status(app.state::<AudioState>()),
            ),
            HostCommand::MidiStatus => json_payload(
                "midi_status",
                crate::midi::midi_status(&app.state::<MidiState>()),
            ),
            HostCommand::RecordStatus => json_payload(
                "record_status",
                crate::record::record_status(app.state::<RecordState>()),
            ),

            // ── app lifecycle ───────────────────────────────────────────
            HostCommand::AppQuit => {
                // Exit gracefully. The reply may not flush before the process
                // tears down; the client treats a post-`app_quit` connection
                // close as success (see the backing-movie scenario driver).
                app.exit(0);
                Ok(serde_json::Value::Null)
            }
        }
    }
}

/// Map the protocol's transport-agnostic [`SaveDest`] onto the Tauri
/// `state::SaveDest`. Kept tiny and total so the two stay in lockstep.
fn map_segment_spec(seg: SegmentSpec) -> crate::state::SplitSegment {
    crate::state::SplitSegment {
        start_us: seg.start_us,
        end_us: seg.end_us,
        name: seg.name,
    }
}

fn map_save_dest(dest: SaveDest) -> crate::state::SaveDest {
    match dest {
        SaveDest::QuickSave => crate::state::SaveDest::QuickSave,
        SaveDest::Library { name } => crate::state::SaveDest::Library { name },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_control::QueryKind;

    #[test]
    fn run_action_over_protocol_mutates_the_shared_composer() {
        let state = AppState::new();

        // A run_action request applies against the one composer the state owns.
        let resp = apply_request(
            &state,
            None,
            Request::RunAction {
                id: Some(1),
                action: "add_note".into(),
                params: serde_json::json!({}),
            },
        );
        match resp {
            Response::Ok {
                id: Some(1),
                state: Some(snap),
                ..
            } => assert_eq!(snap.notes.len(), 1, "add_note adds one note"),
            other => panic!("expected Ok with snapshot, got {other:?}"),
        }

        // A following query observes the *same* composer (the note persisted).
        let resp = apply_request(
            &state,
            None,
            Request::Query {
                id: Some(2),
                what: QueryKind::State,
            },
        );
        match resp {
            Response::Ok {
                state: Some(snap), ..
            } => assert_eq!(snap.notes.len(), 1, "state persists across requests"),
            other => panic!("expected Ok state, got {other:?}"),
        }
    }

    #[test]
    fn unknown_action_is_a_protocol_error() {
        let state = AppState::new();
        let resp = apply_request(
            &state,
            None,
            Request::RunAction {
                id: Some(7),
                action: "frobnicate".into(),
                params: serde_json::json!({}),
            },
        );
        match resp {
            Response::Err { id: Some(7), error } => {
                assert!(error.contains("unknown_action"), "got: {error}");
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[test]
    fn query_help_lists_actions_and_host_commands() {
        let state = AppState::new();
        let resp = apply_request(
            &state,
            None,
            Request::Query {
                id: None,
                what: QueryKind::Help,
            },
        );
        match resp {
            Response::Help {
                actions,
                host_commands,
                ..
            } => {
                assert!(!actions.is_empty(), "help catalog is non-empty");
                assert!(
                    actions.iter().any(|a| a.name == "add_note"),
                    "help includes add_note"
                );
                // The host-command tier ships in the same catalog.
                assert!(
                    host_commands.iter().any(|c| c.name == "load_bundle"),
                    "help includes load_bundle host command"
                );
            }
            other => panic!("expected Help, got {other:?}"),
        }
    }

    #[test]
    fn host_command_without_host_is_unavailable() {
        // The composer-only path (host: None) rejects host commands rather than
        // silently dropping them. The applier always supplies a TauriHost in
        // production; this guards the seam itself.
        let state = AppState::new();
        let resp = apply_request(
            &state,
            None,
            Request::RunHostCommand {
                id: Some(9),
                command: "query_dirty".into(),
                params: serde_json::json!({}),
            },
        );
        match resp {
            Response::Err { id: Some(9), error } => {
                assert!(error.starts_with("unavailable:"), "got: {error}");
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[test]
    fn save_dest_maps_to_state_dest() {
        // The protocol SaveDest maps 1:1 onto the Tauri state::SaveDest.
        assert!(matches!(
            map_save_dest(SaveDest::QuickSave),
            crate::state::SaveDest::QuickSave
        ));
        match map_save_dest(SaveDest::Library {
            name: "Tune".into(),
        }) {
            crate::state::SaveDest::Library { name } => assert_eq!(name, "Tune"),
            other => panic!("expected Library, got {other:?}"),
        }
    }
}
