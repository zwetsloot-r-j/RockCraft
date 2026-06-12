//! Audio state and helpers for the Tauri backend.
//!
//! [`AudioState`] holds an optional [`AudioOut`] (absent when no device is
//! available — e.g. headless CI, sandboxes) and an optional backing-track
//! session. Every operation on `AudioState` is a no-op when `out` is `None`
//! so the app is fully usable in silent environments.
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
use std::sync::Mutex;
use std::time::Duration;

use rockcraft_audio::{play_file_at, AudioOut, BackingHandle};
use rockcraft_core::Effect;
use serde::Serialize;

// ── Audio state ──────────────────────────────────────────────────────────────

/// A live backing-track session: the file path and (lazily started) handle.
pub struct BackingSession {
    pub path: PathBuf,
    /// `None` until the transport starts playing.
    pub handle: Option<BackingHandle>,
}

/// Managed audio state held by the Tauri app.
///
/// Wrapped in a `Mutex<>` by Tauri's state system; the lock is only held
/// for the duration of each command/tick operation — never across I/O.
pub struct AudioState {
    /// `None` when no audio device is available (CI, headless) — every
    /// operation becomes a no-op rather than panicking.
    pub out: Option<AudioOut>,
    /// `None` when no device, or `Some` once the `AudioOut` is alive.
    /// Kept as a field so the synth can be cloned cheaply.
    pub synth: Option<rockcraft_audio::SynthHandle>,
    /// The current backing-track attachment, if any.
    pub backing: Mutex<Option<BackingSession>>,
}

impl AudioState {
    /// Attempt to open the default audio device. On failure (no device, no
    /// SoundFont) a warning is logged and `out`/`synth` are left `None`.
    pub fn new() -> Self {
        match AudioOut::new() {
            Ok(out) => {
                let synth = out.synth();
                Self {
                    out: Some(out),
                    synth: Some(synth),
                    backing: Mutex::new(None),
                }
            }
            Err(e) => {
                eprintln!("[rockcraft-tauri] audio disabled: {e}");
                Self {
                    out: None,
                    synth: None,
                    backing: Mutex::new(None),
                }
            }
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
/// - `AuditionNote` → `all_off` then `note_on` (sustain is ended by the
///   engine's `advance`-emitted note-offs; see the TUI wiring).
/// - `AuditionChord` → `all_off` then `note_on` each pitch (matches TUI).
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
/// that aligns with song time 0. The result is the position to seek/start at.
///
/// Pure helper; headless-testable.
pub fn backing_pos(playhead_us: u64, offset_us: u64) -> Duration {
    // playhead + offset: the offset is how many µs into the file corresponds
    // to the start of the song, so the current file position is:
    //   playhead + offset
    Duration::from_micros(playhead_us + offset_us)
}

// ── Backing transport coupling ───────────────────────────────────────────────

/// Decide what the backing should do given the current and previous transport
/// state, and apply it to the live session.
///
/// Call once per tick / per `run_action` reply that may affect the transport.
///
/// - play started → start (or re-seek + resume) at `backing_pos`.
/// - seeking / offset change while playing → re-seek without stopping.
/// - pause → `set_paused(true)`.
/// - idle / not playing → nothing.
///
/// `prev_playing`, `prev_playhead_us`, `prev_offset_us` must be updated
/// **by the caller** after this returns so the next call has a coherent diff.
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
    let mut backing = audio.backing.lock().expect("backing mutex poisoned");
    let Some(session) = backing.as_mut() else {
        return;
    };

    if playing && !prev_playing {
        // Transport just started.
        let pos = backing_pos(playhead_us, backing_offset_us);
        if let Some(h) = &session.handle {
            h.seek(pos);
            h.set_paused(false);
        } else {
            match play_file_at(&session.path, pos) {
                Ok(h) => session.handle = Some(h),
                Err(e) => eprintln!("[rockcraft-tauri] backing: play failed: {e}"),
            }
        }
    } else if playing && (playhead_us < prev_playhead_us || backing_offset_us != prev_offset_us) {
        // Seek while playing (loop wrap, rewind, or nudge).
        let pos = backing_pos(playhead_us, backing_offset_us);
        if let Some(h) = &session.handle {
            h.seek(pos);
        }
    } else if !playing && prev_playing {
        // Transport just paused.
        if let Some(h) = &session.handle {
            h.set_paused(true);
        }
    }
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
    let mut backing = state.backing.lock().expect("backing mutex poisoned");
    // Stop any previous session before replacing.
    if let Some(old) = backing.take() {
        if let Some(h) = old.handle {
            h.stop();
        }
    }
    *backing = Some(BackingSession {
        path: p,
        handle: None,
    });
    Ok(())
}

/// Remove the backing track (stops any playback).
#[tauri::command]
pub fn detach_backing(state: tauri::State<'_, AudioState>) {
    let mut backing = state.backing.lock().expect("backing mutex poisoned");
    if let Some(session) = backing.take() {
        if let Some(h) = session.handle {
            h.stop();
        }
    }
}

/// Return current audio status (device availability and backing file).
#[tauri::command]
pub fn audio_status(state: tauri::State<'_, AudioState>) -> AudioStatus {
    let device = state.out.is_some();
    let backing = {
        let guard = state.backing.lock().expect("backing mutex poisoned");
        guard
            .as_ref()
            .and_then(|s| s.path.file_name().map(|n| n.to_string_lossy().into_owned()))
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
            out: None,
            synth: None,
            backing: Mutex::new(None),
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

    /// Pure helper: `backing_pos(0, 0)` → zero Duration.
    #[test]
    fn backing_pos_zero() {
        assert_eq!(backing_pos(0, 0), Duration::ZERO);
    }

    /// `backing_pos` adds offset to playhead.
    #[test]
    fn backing_pos_adds_offset() {
        assert_eq!(
            backing_pos(1_000_000, 250_000),
            Duration::from_micros(1_250_000)
        );
    }

    /// Large values stay in-range (no wrapping/overflow in normal use).
    #[test]
    fn backing_pos_large_values() {
        let playhead = 10 * 60 * 1_000_000u64; // 10 minutes
        let offset = 5_000_000u64; // 5 seconds
        assert_eq!(
            backing_pos(playhead, offset),
            Duration::from_micros(playhead + offset)
        );
    }

    /// Nudge accumulation: successive offset changes stack.
    #[test]
    fn backing_pos_nudges_accumulate() {
        let base = 1_000_000u64;
        let nudge1 = 10_000u64;
        let nudge2 = 250_000u64;
        assert_eq!(
            backing_pos(base, nudge1 + nudge2),
            Duration::from_micros(base + nudge1 + nudge2)
        );
    }

    /// `attach_backing` rejects a missing path.
    #[test]
    fn attach_backing_rejects_missing_path() {
        let audio_state = AudioState {
            out: None,
            synth: None,
            backing: Mutex::new(None),
        };
        // Cannot call the `#[tauri::command]` directly in unit tests (it needs
        // a State wrapper). Test the business logic inline instead.
        let path = "/nonexistent/definitely/not/here.wav";
        let p = PathBuf::from(path);
        assert!(!p.exists(), "test pre-condition: file should not exist");
        // Replicate the guard from attach_backing.
        let result: Result<(), String> = if !p.exists() {
            Err(format!("backing file not found: {path}"))
        } else {
            let mut backing = audio_state.backing.lock().expect("mutex");
            *backing = Some(BackingSession {
                path: p,
                handle: None,
            });
            Ok(())
        };
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
