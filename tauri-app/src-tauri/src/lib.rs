//! RockCraft Tauri frontend — integration layer over `rockcraft-core`.
//!
//! This is the M7 IPC bridge: the backend owns a live [`rockcraft_core::Composer`]
//! as managed state ([`AppState`]) and exposes invoke commands that mirror the
//! WebSocket control protocol — `run_action(name, params)` → effects + snapshot,
//! `query_state`, `query_help`. A background tick thread advances the transport
//! while it is playing and pushes `snapshot` / `effects` events to the webview,
//! so the UI never polls.
//!
//! The command bodies live in [`state`] as `&AppState` free functions for
//! headless unit testing; the `#[tauri::command]` wrappers below are thin shims.

mod state;

use std::time::Instant;

use rockcraft_core::ComposerSnapshot;
use tauri::{Emitter, Manager, State};

use crate::state::{ActionReply, AppState};

/// Tick cadence for the transport-advance thread (~4 ms ≈ 250 Hz).
const TICK_PERIOD: std::time::Duration = std::time::Duration::from_millis(4);

/// Event name carrying a fresh [`ComposerSnapshot`] to the webview.
const EVENT_SNAPSHOT: &str = "snapshot";
/// Event name carrying a batch of effects to the webview.
const EVENT_EFFECTS: &str = "effects";

/// Apply a named action and return its effects plus the new snapshot.
///
/// Mirrors `run_action` over the control protocol. After applying, the
/// `snapshot` event (and `effects`, when non-empty) is emitted so every
/// listener stays in sync without polling. An unknown name or bad params is
/// surfaced as `Err(String)`.
#[tauri::command]
fn run_action(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    name: String,
    params: serde_json::Value,
) -> Result<ActionReply, String> {
    let reply = state::run_action(&state, &name, &params)?;
    // The helper has already released the composer lock; emit afterwards.
    let _ = app.emit(EVENT_SNAPSHOT, &reply.snapshot);
    if !reply.effects.is_empty() {
        let _ = app.emit(EVENT_EFFECTS, &reply.effects);
    }
    Ok(reply)
}

/// Current composer snapshot — mirrors `query state`.
#[tauri::command]
fn query_state(state: State<'_, AppState>) -> ComposerSnapshot {
    state::query_state(&state)
}

/// The full, self-describing action catalog — mirrors `query help`.
#[tauri::command]
fn query_help() -> serde_json::Value {
    state::query_help()
}

/// Spawn the transport-advance thread.
///
/// Every [`TICK_PERIOD`] it measures the wall-clock delta since the last tick,
/// calls [`state::tick_advance`] (a no-op unless playing), and — while the
/// transport is playing — emits the moving `snapshot` (plus `effects` whenever
/// a batch was produced). The composer lock is held only inside `tick_advance`
/// and `query_state`; event emission happens with the lock released.
fn spawn_tick_thread(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut last = Instant::now();
        loop {
            std::thread::sleep(TICK_PERIOD);
            let now = Instant::now();
            let dt_us = now.duration_since(last).as_micros() as u64;
            last = now;

            let state = app.state::<AppState>();
            let effects = state::tick_advance(&state, dt_us);
            let snapshot = state::query_state(&state);

            // Push the snapshot while playing (so the highway scrolls) and on
            // any tick that produced effects. Idle-and-stopped ticks stay
            // silent so we don't spam the webview with identical snapshots.
            if snapshot.playing || !effects.is_empty() {
                let _ = app.emit(EVENT_SNAPSHOT, &snapshot);
            }
            if !effects.is_empty() {
                let _ = app.emit(EVENT_EFFECTS, &effects);
            }
        }
    });
}

/// Build and run the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            run_action,
            query_state,
            query_help
        ])
        .setup(|app| {
            spawn_tick_thread(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
