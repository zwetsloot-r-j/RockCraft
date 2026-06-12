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
//!   shared [`handle`] — against the **one** live [`Composer`] the backend owns
//!   ([`AppState`]), the same composer the IPC commands and tick thread mutate.
//! - After an agent-driven action it emits the `snapshot` event, mirroring the
//!   IPC `run_action` path, so the webview reflects remote edits. Playback audio
//!   and the transport already flow through the tick thread (`lib.rs`), which
//!   reads the same shared composer, so a remote `play_*` action sounds and
//!   scrolls without any extra wiring here.
//!
//! Scope is the **existing** protocol only — no new actions or verbs. App-level
//! workflow commands (play/record/import/library/backing) are the separate
//! M8-A host-command tier.
//!
//! [`Composer`]: rockcraft_core::Composer

use std::net::SocketAddr;

use rockcraft_control::{handle, CommandServer, RemoteCommand, Request, Response};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

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
/// synchronous [`handle`] call.
fn spawn_applier(app: AppHandle, mut cmd_rx: mpsc::Receiver<RemoteCommand>) {
    std::thread::Builder::new()
        .name("rockcraft-control-apply".into())
        .spawn(move || {
            while let Some(cmd) = cmd_rx.blocking_recv() {
                let mutating = matches!(cmd.req, Request::RunAction { .. });
                let response = apply_request(&app.state::<AppState>(), cmd.req);
                // Keep the webview in sync with agent-driven edits, mirroring the
                // IPC run_action emit. Only after a mutating action — queries must
                // not spam the webview with redundant snapshots.
                if mutating {
                    if let Response::Ok {
                        state: Some(snapshot),
                        ..
                    } = &response
                    {
                        let _ = app.emit(EVENT_SNAPSHOT, snapshot);
                    }
                }
                // The client may have gone away; a failed reply is not fatal.
                let _ = cmd.reply.send(response);
            }
        })
        .expect("spawn control applier thread");
}

/// Apply one [`Request`] against the shared composer and return the wire
/// [`Response`]. Factored out of [`spawn_applier`] so the integration seam
/// (AppState composer ↔ control protocol) is unit-testable without a running
/// Tauri app.
fn apply_request(state: &AppState, req: Request) -> Response {
    let mut composer = state.composer.lock().expect("composer mutex poisoned");
    handle(&mut composer, req)
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
    fn query_help_lists_actions() {
        let state = AppState::new();
        let resp = apply_request(
            &state,
            Request::Query {
                id: None,
                what: QueryKind::Help,
            },
        );
        match resp {
            Response::Help { actions, .. } => {
                assert!(!actions.is_empty(), "help catalog is non-empty");
                assert!(
                    actions.iter().any(|a| a.name == "add_note"),
                    "help includes add_note"
                );
            }
            other => panic!("expected Help, got {other:?}"),
        }
    }
}
