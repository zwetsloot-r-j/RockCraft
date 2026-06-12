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
//!
//! Audio is routed through [`audio::AudioState`] (managed separately). Effects
//! from `run_action` and tick ticks are passed to [`audio::apply_effects`];
//! the backing track follows the transport via [`audio::sync_backing`].

mod audio;
mod library;
mod midi;
mod play;
mod state;

use std::sync::Mutex;
use std::time::Instant;

use rockcraft_core::ComposerSnapshot;
use tauri::{Emitter, Manager, State};

use crate::audio::AudioState;
use crate::midi::{MidiState, MidiStatus};
use crate::play::PlayState;
use crate::state::{ActionReply, AppState};

/// Tick cadence for the transport-advance thread (~4 ms ≈ 250 Hz).
const TICK_PERIOD: std::time::Duration = std::time::Duration::from_millis(4);

/// Event name carrying a fresh [`ComposerSnapshot`] to the webview.
const EVENT_SNAPSHOT: &str = "snapshot";
/// Event name carrying a batch of effects to the webview.
const EVENT_EFFECTS: &str = "effects";
/// Event name carrying a serialised [`NoteEvent`] to the webview.
const EVENT_MIDI: &str = "midi_event";
/// Event name carrying a live [`play::PlayStateEvent`] to the highway (#168).
const EVENT_PLAY_STATE: &str = "play_state";

/// Apply a named action and return its effects plus the new snapshot.
///
/// Mirrors `run_action` over the control protocol. After applying, the
/// `snapshot` event (and `effects`, when non-empty) is emitted so every
/// listener stays in sync without polling. An unknown name or bad params is
/// surfaced as `Err(String)`.
///
/// Effects are routed to the synth and the backing transport is synced.
#[tauri::command]
fn run_action(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    audio: State<'_, AudioState>,
    prev_transport: State<'_, PrevTransport>,
    name: String,
    params: serde_json::Value,
) -> Result<ActionReply, String> {
    let reply = state::run_action(&state, &name, &params)?;
    // Route effects to the synth (note audition, chord preview, all-off).
    audio::apply_effects(&audio, &reply.effects);
    // Sync the backing track to the new transport state.
    {
        let mut prev = prev_transport
            .0
            .lock()
            .expect("prev_transport mutex poisoned");
        audio::sync_backing(
            &audio,
            reply.snapshot.playing,
            reply.snapshot.playhead_us,
            reply.snapshot.backing_offset_us,
            prev.playing,
            prev.playhead_us,
            prev.offset_us,
        );
        *prev = TransportSnapshot {
            playing: reply.snapshot.playing,
            playhead_us: reply.snapshot.playhead_us,
            offset_us: reply.snapshot.backing_offset_us,
        };
    }
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

/// Scan the default library roots and return a list of bundle DTOs.
///
/// Calls [`library::scan_library_inner`] with [`rockcraft_midi::bundle::default_scan_roots`].
/// Missing roots are silently skipped.
#[tauri::command]
fn scan_library() -> Vec<library::LibraryEntryDto> {
    library::scan_library_inner(&rockcraft_midi::bundle::default_scan_roots())
}

/// Return the current MIDI input status (`kind` + optional `port` name).
#[tauri::command]
fn midi_status(midi: State<'_, MidiState>) -> MidiStatus {
    crate::midi::midi_status(&midi)
}

/// Simulate a computer-key press (down=true) or release (down=false) on the
/// mock keyboard. Returns `Err` when a real device is connected (mock keys must
/// not inject fake events into live input).
#[tauri::command]
fn mock_key(midi: State<'_, MidiState>, key: char, down: bool) -> Result<(), String> {
    crate::midi::mock_key(&midi, key, down)
}

/// Previous transport state for the backing-sync diff in the tick thread and
/// `run_action`. Held behind a `Mutex` so both paths can update it.
#[derive(Default)]
struct TransportSnapshot {
    playing: bool,
    playhead_us: u64,
    offset_us: u64,
}

/// Tauri-managed wrapper for `TransportSnapshot`.
struct PrevTransport(Mutex<TransportSnapshot>);

impl Default for PrevTransport {
    fn default() -> Self {
        Self(Mutex::new(TransportSnapshot::default()))
    }
}

/// Spawn the transport-advance thread.
///
/// Every [`TICK_PERIOD`] it measures the wall-clock delta since the last tick,
/// drains pending MIDI events (emitting `midi_event` and feeding the composer),
/// calls [`state::tick_advance`] (a no-op unless playing), and — while the
/// transport is playing — emits the moving `snapshot` (plus `effects` whenever
/// a batch was produced). The composer lock is held only inside `tick_advance`
/// and `query_state`; event emission happens with the lock released.
///
/// Effects and backing are handled here too, mirroring the `run_action` path.
fn spawn_tick_thread(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut last = Instant::now();
        loop {
            std::thread::sleep(TICK_PERIOD);
            let now = Instant::now();
            let dt_us = now.duration_since(last).as_micros() as u64;
            last = now;

            let app_state = app.state::<AppState>();
            let midi_state = app.state::<MidiState>();
            let audio = app.state::<AudioState>();
            let prev_transport = app.state::<PrevTransport>();
            let play_state = app.state::<PlayState>();

            // When a play session is active (#168), MIDI feeds the highway, not
            // the composer: drain raw events, drive the session, emit play_state.
            // Scoring/wait/clock all run off the injected `dt_us`, never the
            // render loop. The composer path is skipped entirely this tick.
            let play_active = play_state.0.lock().expect("play state poisoned").is_some();
            if play_active {
                let raw = crate::midi::drain(&midi_state);
                for ev in &raw {
                    let _ = app.emit(EVENT_MIDI, crate::midi::NoteEventPayload::from(*ev));
                }
                if let Some(snapshot) = crate::play::tick_play(&play_state, &audio, &raw, dt_us) {
                    let _ = app.emit(EVENT_PLAY_STATE, &snapshot);
                }
                continue;
            }

            // Drain pending MIDI events, ingest into composer, emit to webview.
            let (midi_events, midi_effects) = {
                let mut composer = app_state.composer.lock().expect("composer mutex poisoned");
                crate::midi::drain_and_ingest(&midi_state, &mut composer)
            };
            for ev in &midi_events {
                let _ = app.emit(EVENT_MIDI, ev);
            }

            let effects = state::tick_advance(&app_state, dt_us);
            let all_effects: Vec<_> = midi_effects.into_iter().chain(effects).collect();
            let snapshot = state::query_state(&app_state);

            // Route effects to the synth.
            if !all_effects.is_empty() {
                audio::apply_effects(&audio, &all_effects);
            }

            // Sync backing to transport state.
            {
                let mut prev = prev_transport
                    .0
                    .lock()
                    .expect("prev_transport mutex poisoned");
                audio::sync_backing(
                    &audio,
                    snapshot.playing,
                    snapshot.playhead_us,
                    snapshot.backing_offset_us,
                    prev.playing,
                    prev.playhead_us,
                    prev.offset_us,
                );
                *prev = TransportSnapshot {
                    playing: snapshot.playing,
                    playhead_us: snapshot.playhead_us,
                    offset_us: snapshot.backing_offset_us,
                };
            }

            // Push the snapshot while playing (so the highway scrolls) and on
            // any tick that produced effects. Idle-and-stopped ticks stay
            // silent so we don't spam the webview with identical snapshots.
            if snapshot.playing || !all_effects.is_empty() || !midi_events.is_empty() {
                let _ = app.emit(EVENT_SNAPSHOT, &snapshot);
            }
            if !all_effects.is_empty() {
                let _ = app.emit(EVENT_EFFECTS, &all_effects);
            }
        }
    });
}

/// Build and run the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .manage(AudioState::new())
        .manage(MidiState::new())
        .manage(PrevTransport::default())
        .manage(PlayState::default())
        .invoke_handler(tauri::generate_handler![
            run_action,
            query_state,
            query_help,
            scan_library,
            midi_status,
            mock_key,
            audio::attach_backing,
            audio::detach_backing,
            audio::audio_status,
            play::play_load,
            play::play_set_wait,
            play::play_toggle_hear_song,
            play::play_finish,
        ])
        .setup(|app| {
            spawn_tick_thread(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
