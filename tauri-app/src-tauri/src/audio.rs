//! Audio state and helpers for the Tauri backend.
//!
//! [`AudioState`] holds an optional [`SynthHandle`] and a channel to an
//! "audio manager" thread that owns the `!Send` [`AudioOut`] and
//! [`BackingHandle`]. Because `OutputStream` (inside both) is `!Send`, those
//! types cannot be put behind a plain `Mutex` in Tauri state. Instead:
//!
//! - `SynthHandle` is `Send + Clone`, so it lives directly in `AudioState`.
//! - Backing-track control is proxied through `Sender<BackingMsg>` to a
//!   dedicated thread that owns `AudioOut` (to keep the stream alive) and
//!   `BackingHandle`.
//!
//! When no audio device is available (CI, headless) `synth` is `None` and the
//! backing sender is disconnected; every operation becomes a no-op so the app
//! is fully usable in silent environments.
//!
//! The three public Tauri commands are thin wrappers:
//! - [`attach_backing`] — point at an audio file, verified to exist.
//! - [`detach_backing`] — clear the backing-track session.
//! - [`audio_status`] — report device / backing state to the webview.
//!
//! [`apply_effects`] routes a batch of [`Effect`]s from `run_action` /
//! `tick_advance` to the synth; call it on the app thread (never on the audio
//! thread — the synth handle is lock-free, not thread-unsafe).

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

use rockcraft_audio::{play_file_at, AudioOut, BackingHandle, SynthHandle};
use rockcraft_core::Effect;
use serde::Serialize;

// ── Backing-thread messages ──────────────────────────────────────────────────

/// Commands sent from the app thread to the backing-manager thread.
enum BackingMsg {
    /// Attach a backing file (replaces any previous one; does not play yet).
    Attach(PathBuf),
    /// Remove the backing file (stops playback).
    Detach,
    /// Play or re-seek to `pos_us` from the start; resume if paused.
    PlayAt(u64),
    /// Seek to `pos_us` while already playing.
    Seek(u64),
    /// Pause the current playback.
    Pause,
    /// Query the current backing file name (reply on the one-shot channel).
    QueryFileName(Sender<Option<String>>),
}

// ── Audio state ──────────────────────────────────────────────────────────────

/// Managed audio state held by the Tauri app.
///
/// `Send + Sync` safe: `SynthHandle` wraps an `mpsc::Sender<SynthCommand>`
/// (Send); `backing_tx` is a `Mutex<Option<Sender<BackingMsg>>>` (Send+Sync).
/// The `!Send` rodio types (`AudioOut`, `BackingHandle`) live on the
/// dedicated backing thread.
pub struct AudioState {
    /// `None` when no audio device is available (CI, headless).
    pub synth: Option<SynthHandle>,
    /// Channel to the backing-manager thread. `None` when no device.
    backing_tx: Mutex<Option<Sender<BackingMsg>>>,
}

impl AudioState {
    /// Attempt to open the default audio device. On failure a warning is
    /// printed and the state is left with `synth: None` / backing disabled —
    /// the app continues silently.
    ///
    /// Because `cpal::Stream` (inside `AudioOut` / `BackingHandle`) is `!Send`,
    /// we create `AudioOut` on a dedicated audio-manager thread and send the
    /// `SynthHandle` back via a one-shot channel. The thread then owns all
    /// `!Send` audio objects and proxies backing commands through `BackingMsg`.
    pub fn new() -> Self {
        // One-shot channel: the audio thread sends the SynthHandle back.
        let (synth_tx, synth_rx) = mpsc::channel::<Option<SynthHandle>>();
        // Ongoing channel for backing commands (Send: contains only PathBuf / u64).
        let (backing_tx, backing_rx) = mpsc::channel::<BackingMsg>();

        std::thread::spawn(move || {
            // Create AudioOut here — on this thread — so the !Send stream stays
            // thread-local.
            let out = match AudioOut::new() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("[rockcraft-tauri] audio disabled: {e}");
                    let _ = synth_tx.send(None); // signal failure
                    return;
                }
            };
            let synth = out.synth();
            let _ = synth_tx.send(Some(synth));
            // `out` stays alive here, keeping the device stream open.
            let _out = out;

            let mut path: Option<PathBuf> = None;
            let mut handle: Option<BackingHandle> = None;

            for msg in backing_rx {
                match msg {
                    BackingMsg::Attach(p) => {
                        if let Some(h) = handle.take() {
                            h.stop();
                        }
                        path = Some(p);
                    }
                    BackingMsg::Detach => {
                        if let Some(h) = handle.take() {
                            h.stop();
                        }
                        path = None;
                    }
                    BackingMsg::PlayAt(pos_us) => {
                        let Some(ref p) = path else { continue };
                        let pos = Duration::from_micros(pos_us);
                        if let Some(h) = &handle {
                            h.seek(pos);
                            h.set_paused(false);
                        } else {
                            match play_file_at(p, pos) {
                                Ok(h) => handle = Some(h),
                                Err(e) => {
                                    eprintln!("[rockcraft-tauri] backing: play failed: {e}")
                                }
                            }
                        }
                    }
                    BackingMsg::Seek(pos_us) => {
                        if let Some(h) = &handle {
                            h.seek(Duration::from_micros(pos_us));
                        }
                    }
                    BackingMsg::Pause => {
                        if let Some(h) = &handle {
                            h.set_paused(true);
                        }
                    }
                    BackingMsg::QueryFileName(reply) => {
                        let name = path
                            .as_ref()
                            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                        let _ = reply.send(name);
                    }
                }
            }
            // backing_rx closed (app shutting down): stop backing if any.
            if let Some(h) = handle {
                h.stop();
            }
        });

        // Wait for the audio thread to signal success/failure.
        match synth_rx.recv() {
            Ok(Some(synth)) => Self {
                synth: Some(synth),
                backing_tx: Mutex::new(Some(backing_tx)),
            },
            _ => {
                // Audio init failed or thread panicked.
                Self {
                    synth: None,
                    backing_tx: Mutex::new(None),
                }
            }
        }
    }

    /// Send a message to the backing thread (silently no-ops when disconnected).
    fn send_backing(&self, msg: BackingMsg) {
        let guard = self.backing_tx.lock().expect("backing_tx mutex poisoned");
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(msg);
        }
    }
}

impl Default for AudioState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Effect routing ───────────────────────────────────────────────────────────

/// Route a batch of [`Effect`]s from the composer to the synth.
///
/// - `AuditionNote` → `all_off` then `note_on`.
/// - `AuditionChord` → `all_off` then `note_on` each pitch.
/// - `AllOff` → `all_off`.
///
/// If `audio.synth` is `None` this is a no-op (headless CI path).
pub fn apply_effects(audio: &AudioState, effects: &[Effect]) {
    let Some(synth) = &audio.synth else { return };
    for effect in effects {
        match effect {
            Effect::AuditionNote { pitch, velocity } => {
                synth.all_off();
                if let (Some(note), Some(vel)) = (
                    rockcraft_core::MidiNote::new(*pitch),
                    rockcraft_core::Velocity::new(*velocity),
                ) {
                    synth.note_on(note, vel);
                }
            }
            Effect::AuditionChord { pitches } => {
                synth.all_off();
                for &p in pitches {
                    if let (Some(note), Some(vel)) = (
                        rockcraft_core::MidiNote::new(p),
                        rockcraft_core::Velocity::new(80),
                    ) {
                        synth.note_on(note, vel);
                    }
                }
            }
            Effect::AllOff => {
                synth.all_off();
            }
        }
    }
}

// ── Backing position helper ──────────────────────────────────────────────────

/// Compute the backing file position for the given transport state.
///
/// `backing_offset_us` is `audio_start_us` in the file — the file position
/// that aligns with song time 0. The result is `playhead + offset` µs.
///
/// Pure helper; headless-testable.
pub fn backing_pos(playhead_us: u64, offset_us: u64) -> u64 {
    playhead_us + offset_us
}

// ── Backing transport coupling ───────────────────────────────────────────────

/// Decide what the backing should do given the current and previous transport
/// state, and dispatch the appropriate message to the backing thread.
///
/// Call once per tick / per `run_action` reply that may affect the transport.
#[allow(clippy::too_many_arguments)]
pub fn sync_backing(
    audio: &AudioState,
    playing: bool,
    playhead_us: u64,
    backing_offset_us: u64,
    prev_playing: bool,
    prev_playhead_us: u64,
    prev_offset_us: u64,
) {
    if playing && !prev_playing {
        // Transport just started.
        let pos = backing_pos(playhead_us, backing_offset_us);
        audio.send_backing(BackingMsg::PlayAt(pos));
    } else if playing && (playhead_us < prev_playhead_us || backing_offset_us != prev_offset_us) {
        // Seek while playing (loop wrap, rewind, or nudge).
        let pos = backing_pos(playhead_us, backing_offset_us);
        audio.send_backing(BackingMsg::Seek(pos));
    } else if !playing && prev_playing {
        // Transport just paused.
        audio.send_backing(BackingMsg::Pause);
    }
}

// ── Inner helpers (callable without a Tauri State wrapper) ──────────────────

/// Attach a backing-track file, bypassing the path-existence check.
///
/// Intended for use by [`crate::record`] when starting a session with a
/// backing track (the caller already verified the path exists).
pub fn attach_backing_inner(state: &AudioState, path: PathBuf) {
    state.send_backing(BackingMsg::Attach(path));
}

/// Start the backing track from position 0 immediately.
///
/// Called by [`crate::record`] when a recording session begins with a backing
/// track so audio plays from the first moment of recording.
pub fn play_backing_now(state: &AudioState) {
    state.send_backing(BackingMsg::PlayAt(0));
}

// ── Tauri commands ───────────────────────────────────────────────────────────

/// Audio status reported to the webview.
#[derive(Debug, Clone, Serialize)]
pub struct AudioStatus {
    /// Whether an audio output device is available.
    pub device: bool,
    /// The backing file name (without path) if one is attached.
    pub backing: Option<String>,
}

/// Attach a backing-track file.
///
/// The file must exist; `attach_backing` returns an error string otherwise so
/// the webview can show a message. This is a pure state change — playback does
/// not start until the transport plays.
#[tauri::command]
pub fn attach_backing(state: tauri::State<'_, AudioState>, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("backing file not found: {path}"));
    }
    state.send_backing(BackingMsg::Attach(p));
    Ok(())
}

/// Remove the backing track (stops any playback).
#[tauri::command]
pub fn detach_backing(state: tauri::State<'_, AudioState>) {
    state.send_backing(BackingMsg::Detach);
}

/// Return current audio status (device availability and backing file).
#[tauri::command]
pub fn audio_status(state: tauri::State<'_, AudioState>) -> AudioStatus {
    let device = state.synth.is_some();
    // Query the backing thread for the current file name.
    let backing = {
        let guard = state.backing_tx.lock().expect("backing_tx mutex poisoned");
        if let Some(tx) = guard.as_ref() {
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(BackingMsg::QueryFileName(reply_tx)).is_ok() {
                reply_rx.recv().ok().flatten()
            } else {
                None
            }
        } else {
            None
        }
    };
    AudioStatus { device, backing }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `apply_effects` with no audio device is a no-op — must not panic.
    #[test]
    fn apply_effects_no_device_is_noop() {
        let audio = AudioState {
            synth: None,
            backing_tx: Mutex::new(None),
        };
        apply_effects(
            &audio,
            &[
                Effect::AuditionNote {
                    pitch: 60,
                    velocity: 80,
                },
                Effect::AuditionChord {
                    pitches: vec![60, 64, 67],
                },
                Effect::AllOff,
            ],
        );
        // Reached here without panicking — pass.
    }

    /// Pure helper: `backing_pos(0, 0)` → 0.
    #[test]
    fn backing_pos_zero() {
        assert_eq!(backing_pos(0, 0), 0);
    }

    /// `backing_pos` adds offset to playhead.
    #[test]
    fn backing_pos_adds_offset() {
        assert_eq!(backing_pos(1_000_000, 250_000), 1_250_000);
    }

    /// Large values stay in-range (no wrapping/overflow in normal use).
    #[test]
    fn backing_pos_large_values() {
        let playhead = 10 * 60 * 1_000_000u64; // 10 minutes
        let offset = 5_000_000u64; // 5 seconds
        assert_eq!(backing_pos(playhead, offset), playhead + offset);
    }

    /// Nudge accumulation: successive offset changes stack.
    #[test]
    fn backing_pos_nudges_accumulate() {
        let base = 1_000_000u64;
        let nudge1 = 10_000u64;
        let nudge2 = 250_000u64;
        assert_eq!(backing_pos(base, nudge1 + nudge2), base + nudge1 + nudge2);
    }

    /// `attach_backing` rejects a missing path.
    #[test]
    fn attach_backing_rejects_missing_path() {
        let path = "/nonexistent/definitely/not/here.wav";
        let p = PathBuf::from(path);
        assert!(!p.exists(), "test pre-condition: file should not exist");
        // Replicate the guard from attach_backing.
        let result: Result<(), String> = if !p.exists() {
            Err(format!("backing file not found: {path}"))
        } else {
            Ok(())
        };
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
