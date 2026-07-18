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

mod alignment;
mod audio;
mod control;
mod import;
mod library;
mod midi;
mod play;
mod record;
mod state;
mod transcription;

use std::sync::Mutex;
use std::time::Instant;

use rockcraft_core::ComposerSnapshot;
use tauri::{Emitter, Manager, State};

use crate::audio::AudioState;
use crate::import::ImportRunning;
use crate::midi::{MidiState, MidiStatus};
use crate::play::PlayState;
use crate::record::RecordState;
use crate::state::{ActionReply, AppState, BackingRef, SaveDest, SplitSegment, VideoRef};

/// Tick cadence for the transport-advance thread (~4 ms ≈ 250 Hz). Fine-grained
/// so the injected `dt_us` for scoring / audition boundaries is precise.
const TICK_PERIOD: std::time::Duration = std::time::Duration::from_millis(4);

/// Minimum spacing between the lightweight `playhead` pushes **while the
/// transport is playing** (~30 Hz). At the 250 Hz tick rate this would flood the
/// WebView IPC channel; the payload is tiny (position + flag), but the frontend
/// interpolates between pushes anyway, so a modest correction cadence is all it
/// needs. Structural changes (edits / effects) still emit a full snapshot
/// immediately, regardless of this throttle.
const PLAYHEAD_EMIT_PERIOD: std::time::Duration = std::time::Duration::from_millis(33);

/// Event name carrying a fresh [`ComposerSnapshot`] to the webview.
pub(crate) const EVENT_SNAPSHOT: &str = "snapshot";
/// Event name carrying a batch of effects to the webview.
const EVENT_EFFECTS: &str = "effects";
/// Event name carrying a lightweight [`PlayheadEvent`] during playback — just
/// the moving position + playing flag, so the highway scrolls without shipping
/// the whole note list every frame (see the tick thread).
const EVENT_PLAYHEAD: &str = "playhead";
/// Event name carrying a serialised [`NoteEvent`] to the webview.
const EVENT_MIDI: &str = "midi_event";
/// Event name carrying a live [`play::PlayStateEvent`] to the highway (#168).
const EVENT_PLAY_STATE: &str = "play_state";
/// Event name carrying a `Screen` the webview should bring to the foreground —
/// the "auto-follow" behaviour that makes an agent-driven control session
/// visible (the front-end otherwise stays on whatever screen the user opened).
pub(crate) const EVENT_NAVIGATE: &str = "navigate";

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
    audio::apply_effects(&audio, state::is_playing(&state), &reply.effects);
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

/// Absolute directory of the newest bundle across the default scan roots, or
/// `None` when no bundles exist. Backs the menu's "Play/Edit last recording".
#[tauri::command]
fn latest_recording() -> Option<String> {
    library::latest_recording_inner(&rockcraft_midi::bundle::default_scan_roots())
}

/// Save the current composer timeline to a bundle.
///
/// `dest` selects the target:
/// - `{ "kind": "quick_save" }` → `recordings/take-<stamp>/`
/// - `{ "kind": "library", "name": "..." }` → `<library_root>/<slug>/`
///
/// Returns the bundle directory path on success, or an error string.
/// Clears the dirty flag on success.
#[tauri::command]
fn save_bundle(state: State<'_, AppState>, dest: SaveDest) -> Result<String, String> {
    state::save_bundle(&state, dest)
}

/// Load a bundle from `dir` into the composer, replacing its current timeline.
///
/// Reads `song.mid` (required) and `meta.json` (optional). Returns the new
/// composer snapshot so the frontend can refresh immediately, or an error string.
/// Clears the dirty flag after loading.
#[tauri::command]
fn load_bundle(
    state: State<'_, AppState>,
    audio: State<'_, AudioState>,
    dir: String,
) -> Result<rockcraft_core::ComposerSnapshot, String> {
    let snapshot = state::load_bundle(&state, &dir)?;
    // Re-attach the bundle's backing track (if any) to the audio thread so the
    // transport coupling (`sync_backing`) plays it under the editor — mirrors
    // the play screen's lazy attach (M9-E). When the bundle declares no backing,
    // detach so a previously-loaded one does not linger.
    match state::query_backing(&state) {
        Some(b) => audio::attach_backing_inner(&audio, std::path::PathBuf::from(b.path)),
        None => audio::detach_backing_inner(&audio),
    }
    Ok(snapshot)
}

/// Return whether the current timeline has unsaved changes.
#[tauri::command]
fn query_dirty(state: State<'_, AppState>) -> bool {
    state::query_dirty(&state)
}

/// Slice the loaded piece into the given kept parts, writing each as its own
/// standalone library bundle (M10-C split editor). Each `segment` is the
/// half-open range `[start_us, end_us)` plus a `name`; discarded parts are simply
/// omitted by the caller (= trimming). Returns the created bundle directory
/// paths. The source piece is left untouched (non-destructive).
///
/// This is the webview-facing mirror of `HostCommand::SplitBundle`: both route
/// through the one [`state::split_bundle`] write path (M10-B), exactly as
/// `save_bundle` mirrors `HostCommand::SaveBundle`. No slicing/bundle-writing
/// logic lives in the frontend.
#[tauri::command]
fn split_bundle(
    state: State<'_, AppState>,
    segments: Vec<SplitSegment>,
) -> Result<Vec<String>, String> {
    state::split_bundle(&state, segments)
}

/// Attach (or replace) the edit-screen background video (M9-G). `path` is the
/// absolute file the webview picked; `offset_us` is the alignment offset
/// (`videoTime = songTime + offset_us`). Persisted into the bundle on save.
#[tauri::command]
fn edit_set_video(state: State<'_, AppState>, path: String, offset_us: i64) {
    state::set_video(&state, path, offset_us)
}

/// Update only the alignment offset of the attached background video (M9-G).
#[tauri::command]
fn edit_set_video_offset(state: State<'_, AppState>, offset_us: i64) {
    state::set_video_offset(&state, offset_us)
}

/// Detach the edit-screen background video (M9-G).
#[tauri::command]
fn edit_clear_video(state: State<'_, AppState>) {
    state::clear_video(&state)
}

/// Return the currently attached background video, or `null` (M9-G).
#[tauri::command]
fn edit_query_video(state: State<'_, AppState>) -> Option<VideoRef> {
    state::query_video(&state)
}

/// Attach (or replace) the edit-screen backing audio track (M9-E). `path` is the
/// absolute file the webview picked; it is stored on the live editor so
/// `save_bundle` persists it into the bundle's `RecordingMeta.backing`, and is
/// handed to the existing playback thread (`attach_backing_inner`) so it plays
/// under the transport without duplicating the audio plumbing. The file must
/// exist; returns an error string otherwise.
#[tauri::command]
fn edit_set_backing(
    state: State<'_, AppState>,
    audio: State<'_, AudioState>,
    path: String,
) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("backing file not found: {path}"));
    }
    state::set_backing(&state, path);
    audio::attach_backing_inner(&audio, p);
    Ok(())
}

/// Detach the edit-screen backing audio track (M9-E). Clears it from the live
/// editor (so the next save drops `meta.backing`) and stops playback.
#[tauri::command]
fn edit_clear_backing(state: State<'_, AppState>, audio: State<'_, AudioState>) {
    state::clear_backing(&state);
    audio::detach_backing_inner(&audio);
}

/// Return the currently attached backing track, or `null` (M9-E).
#[tauri::command]
fn edit_query_backing(state: State<'_, AppState>) -> Option<BackingRef> {
    state::query_backing(&state)
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

/// Lightweight playback push (see [`EVENT_PLAYHEAD`]): the moving playhead and
/// the playing flag, without the note list. Serialised to the webview ~30×/s
/// during playback so the highway scrolls cheaply.
#[derive(Clone, Copy, serde::Serialize)]
struct PlayheadEvent {
    playhead_us: u64,
    playing: bool,
}

/// Tauri-managed wrapper for `TransportSnapshot`.
struct PrevTransport(Mutex<TransportSnapshot>);

impl Default for PrevTransport {
    fn default() -> Self {
        Self(Mutex::new(TransportSnapshot::default()))
    }
}

/// Derive the bundle-relative filename for a backing audio file.
///
/// Mirrors `crates/tui/src/record.rs::bundle_backing_filename`.
/// The result is always `"backing.<ext>"` where `<ext>` is the source
/// file's extension (lowercased), or `"backing.audio"` when there is none.
pub(crate) fn record_bundle_backing_filename(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".to_string());
    format!("backing.{ext}")
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
        // Wall-clock of the last pushed `playhead` event, to throttle it.
        let mut last_playhead_emit = Instant::now();
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
            let record_state = app.state::<RecordState>();

            // When a play session is active (#168), MIDI feeds the highway, not
            // the composer: drain raw events, drive the session, emit play_state.
            // Scoring/wait/clock all run off the injected `dt_us`, never the
            // render loop. The composer path is skipped entirely this tick.
            let play_active = play_state.0.lock().expect("play state poisoned").is_some();
            if play_active {
                let raw = crate::midi::drain(&midi_state);
                for ev in &raw {
                    let _ = app.emit(EVENT_MIDI, crate::midi::NoteEventPayload::from(*ev));
                    // Feed record session even during play mode.
                    record_state.push(*ev);
                }
                if let Some(snapshot) = crate::play::tick_play(&play_state, &audio, &raw, dt_us) {
                    let _ = app.emit(EVENT_PLAY_STATE, &snapshot);
                }
                continue;
            }

            // Drain pending MIDI events, ingest into composer, emit to webview,
            // and also push into the active recording session (if any).
            let (midi_events, raw_events, midi_effects) = {
                let mut composer = app_state.composer.lock().expect("composer mutex poisoned");
                crate::midi::drain_and_ingest_raw(&midi_state, &mut composer)
            };
            // Feed raw (pre-rebase) events into the record session so timestamps
            // are preserved for backing-offset calculation.
            for ev in &raw_events {
                record_state.push(*ev);
            }
            for ev in &midi_events {
                let _ = app.emit(EVENT_MIDI, ev);
            }

            let effects = state::tick_advance(&app_state, dt_us);
            let all_effects: Vec<_> = midi_effects.into_iter().chain(effects).collect();
            // Cheap transport read (no note list); the full snapshot is built
            // only when the notes actually change (see the emit below).
            let (playing, playhead_us, backing_offset_us) = state::transport_fields(&app_state);

            // Route effects to the synth.
            if !all_effects.is_empty() {
                audio::apply_effects(&audio, playing, &all_effects);
            }

            // Sync backing to transport state.
            {
                let mut prev = prev_transport
                    .0
                    .lock()
                    .expect("prev_transport mutex poisoned");
                audio::sync_backing(
                    &audio,
                    playing,
                    playhead_us,
                    backing_offset_us,
                    prev.playing,
                    prev.playhead_us,
                    prev.offset_us,
                );
                *prev = TransportSnapshot {
                    playing,
                    playhead_us,
                    offset_us: backing_offset_us,
                };
            }

            // Emit strategy (the #1 performance fix): a *structural* change
            // (edit / MIDI / effect) ships the full ~900-note snapshot at once;
            // ordinary playback ticks ship only the lightweight `playhead` push,
            // throttled to ~30 Hz. So the note list is serialised on edits, not
            // 250×/second — which is what backlogged the IPC channel and dragged
            // the highway behind real time. Idle-and-stopped ticks stay silent.
            let changed = !all_effects.is_empty() || !midi_events.is_empty();
            if changed {
                let snapshot = state::query_state(&app_state);
                let _ = app.emit(EVENT_SNAPSHOT, &snapshot);
                last_playhead_emit = now;
            } else if playing && now.duration_since(last_playhead_emit) >= PLAYHEAD_EMIT_PERIOD {
                let _ = app.emit(
                    EVENT_PLAYHEAD,
                    &PlayheadEvent {
                        playhead_us,
                        playing,
                    },
                );
                last_playhead_emit = now;
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
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .manage(AudioState::new())
        .manage(MidiState::new())
        .manage(RecordState::new())
        .manage(PrevTransport::default())
        .manage(PlayState::default())
        .manage(ImportRunning::default())
        .invoke_handler(tauri::generate_handler![
            run_action,
            query_state,
            query_help,
            scan_library,
            latest_recording,
            save_bundle,
            load_bundle,
            query_dirty,
            split_bundle,
            edit_set_video,
            edit_set_video_offset,
            edit_clear_video,
            edit_query_video,
            edit_set_backing,
            edit_clear_backing,
            edit_query_backing,
            midi_status,
            mock_key,
            audio::attach_backing,
            audio::detach_backing,
            audio::audio_status,
            record::record_start,
            record::record_stop,
            record::record_save,
            record::record_status,
            play::play_load,
            play::play_set_wait,
            play::play_toggle_hear_song,
            play::play_toggle_pause,
            play::play_finish,
            import::import_url_available,
            import::import_start,
            transcription::transcription_save,
            transcription::transcription_load,
            alignment::alignment_save,
            alignment::alignment_load,
        ])
        .setup(|app| {
            spawn_tick_thread(app.handle().clone());
            // Optional localhost control socket (default-off; `--control` or
            // ROCKCRAFT_CONTROL_ADDR). Drives the same live composer over the
            // existing rockcraft-control protocol — see `control`.
            control::maybe_start(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
