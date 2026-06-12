//! Managed application state and the command bodies that drive it.
//!
//! The Tauri backend owns a single [`rockcraft_core::Composer`] behind a
//! `Mutex` ([`AppState`]) — the live editor brain. The webview drives it
//! through invoke commands (see [`crate::run`]) that mirror the WebSocket
//! control protocol's vocabulary (`run_action` / `query state` / `query help`)
//! without depending on the `rockcraft-control` crate.
//!
//! The command *bodies* live here as free functions taking `&AppState` so they
//! are unit-testable with no Tauri window: the `#[tauri::command]` wrappers in
//! `lib.rs` are thin shims over these.

use std::sync::Mutex;

use rockcraft_core::{
    action_from_name, action_help, ActionError, Composer, ComposerSnapshot, Effect,
};
use serde::Serialize;

/// Tauri-managed state: the live composer behind a `Mutex`.
///
/// One global editor instance for the app. The tick thread and every command
/// lock it, mutate, clone the payloads they need, and release before doing any
/// I/O (event emission), keeping the critical section tight.
pub struct AppState {
    pub composer: Mutex<Composer>,
}

impl AppState {
    /// A fresh, empty composer.
    pub fn new() -> Self {
        Self {
            composer: Mutex::new(Composer::new()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of a successful [`run_action`]: the effects the action produced
/// (for the frontend to sound, later) plus the resulting state snapshot.
///
/// Mirrors the control protocol's `Response::Ok { effects, state }` shape, but
/// always carries a snapshot — a Tauri command's caller always wants the latest
/// state back.
#[derive(Debug, Clone, Serialize)]
pub struct ActionReply {
    pub effects: Vec<Effect>,
    pub snapshot: ComposerSnapshot,
}

/// Apply a named action to the composer and return the effects + new snapshot.
///
/// `params` is the JSON object of named fields (or `null`/empty for nullary
/// actions). Mirrors `run_action` over the WebSocket control protocol:
/// `action_from_name` → `Composer::apply`. An [`ActionError`] (unknown name or
/// bad params) is flattened to its `Display` string so the command layer can
/// surface it as a plain `Err(String)` to the webview.
pub fn run_action(
    state: &AppState,
    name: &str,
    params: &serde_json::Value,
) -> Result<ActionReply, String> {
    let action = action_from_name(name, params).map_err(|e: ActionError| e.to_string())?;
    let mut composer = state.composer.lock().expect("composer mutex poisoned");
    let effects = composer
        .apply(action)
        .map_err(|e: ActionError| e.to_string())?;
    let snapshot = composer.snapshot();
    Ok(ActionReply { effects, snapshot })
}

/// Current composer snapshot — mirrors `query state`.
pub fn query_state(state: &AppState) -> ComposerSnapshot {
    let composer = state.composer.lock().expect("composer mutex poisoned");
    composer.snapshot()
}

/// The full, self-describing action catalog — mirrors `query help`.
///
/// Serialised from [`action_help`] so the webview can discover every action's
/// name, parameters, and description live (drift-proof).
pub fn query_help() -> serde_json::Value {
    serde_json::to_value(action_help()).expect("action_help serialises")
}

/// Advance the transport by `dt_us` microseconds **only while playing**, and
/// return any effects produced. The tick thread calls this with the measured
/// wall-clock delta; tests call it with a fixed delta.
///
/// When the composer is not playing this is a no-op returning no effects, so
/// the playhead never drifts while stopped.
pub fn tick_advance(state: &AppState, dt_us: u64) -> Vec<Effect> {
    let mut composer = state.composer.lock().expect("composer mutex poisoned");
    if !composer.is_playing() {
        return Vec::new();
    }
    composer.advance(dt_us)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_action_add_note_adds_one_note_at_cursor() {
        let state = AppState::new();
        let before = query_state(&state);
        assert!(before.notes.is_empty());

        let reply = run_action(&state, "add_note", &json!({})).expect("add_note applies");
        assert_eq!(reply.snapshot.notes.len(), 1);
        // The note sits at the cursor's pitch.
        assert_eq!(reply.snapshot.notes[0].pitch, before.cursor.pitch);
    }

    #[test]
    fn run_action_resize_note_grows_duration() {
        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note applies");
        let added = query_state(&state);
        let dur_before = added.notes[0].dur_us;

        let reply = run_action(&state, "resize_note", &json!({ "delta_steps": 2 }))
            .expect("resize_note applies");
        assert_eq!(reply.snapshot.notes.len(), 1);
        assert!(
            reply.snapshot.notes[0].dur_us > dur_before,
            "duration should grow after a positive resize"
        );
    }

    #[test]
    fn run_action_unknown_name_errors() {
        let state = AppState::new();
        let err = run_action(&state, "frobnicate", &json!({})).unwrap_err();
        assert!(
            err.contains("unknown action"),
            "error should mention unknown action, got: {err}"
        );
    }

    #[test]
    fn run_action_bad_params_errors() {
        let state = AppState::new();
        let err = run_action(&state, "resize_note", &json!({ "delta_steps": "lots" })).unwrap_err();
        assert!(
            err.contains("bad params"),
            "error should mention bad params, got: {err}"
        );
    }

    #[test]
    fn tick_advance_moves_playhead_only_while_playing() {
        let state = AppState::new();

        // Stopped: advancing does nothing.
        assert_eq!(query_state(&state).playhead_us, 0);
        let effects = tick_advance(&state, 100_000);
        assert!(effects.is_empty());
        assert_eq!(
            query_state(&state).playhead_us,
            0,
            "playhead must not move while stopped"
        );

        // Playing from start: advancing moves the playhead.
        run_action(&state, "play_from_start", &json!({})).expect("play_from_start applies");
        assert!(query_state(&state).playing);
        tick_advance(&state, 100_000);
        assert_eq!(
            query_state(&state).playhead_us,
            100_000,
            "playhead must advance by dt_us while playing"
        );
    }

    #[test]
    fn query_help_lists_known_actions() {
        let help = query_help();
        let arr = help.as_array().expect("help is a JSON array");
        assert!(!arr.is_empty());
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|info| info.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"add_note"));
        assert!(names.contains(&"resize_note"));
    }
}
