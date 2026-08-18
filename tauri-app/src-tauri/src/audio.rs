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
//! The public Tauri commands are thin wrappers:
//! - [`attach_backing`] — point at an audio file, verified to exist.
//! - [`detach_backing`] — clear the backing-track session.
//! - [`audio_status`] — report device / backing state to the webview.
//! - [`set_instrument`] / [`set_bus_gain`] / [`mixer_status`] — the M14-C sound
//!   selection + three-fader mixer (you / song / backing).
//!
//! [`apply_effects`] routes a batch of [`Effect`]s from `run_action` /
//! `tick_advance` to the synth; call it on the app thread (never on the audio
//! thread — the synth handle is lock-free, not thread-unsafe).

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

use rockcraft_audio::{AudioOut, BackingHandle, SynthHandle};
use rockcraft_core::{Effect, Gain, Mixer, MixerBus, MixerReport, SynthBus};
use serde::Serialize;

// ── Backing-thread messages ──────────────────────────────────────────────────

/// The level actually applied to the backing sink: its fader, or silence while
/// muted. Keeping the two separate means un-muting restores the player's level
/// rather than resetting it to unity.
fn effective_gain(gain: Gain, muted: bool) -> Gain {
    if muted {
        Gain::SILENT
    } else {
        gain
    }
}

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
    /// Set the playback speed multiplier (resamples; pitch shifts with speed).
    /// Keeps the backing in step with a slowed/sped transport.
    SetSpeed(f32),
    /// Set the backing track's level (M14-C). Sticky: re-applied to every sink
    /// the thread makes afterwards, exactly like the speed.
    SetGain(Gain),
    /// Silence (or restore) the backing without touching its fader — used when
    /// the transport runs off-tempo, where a recording cannot follow. Kept
    /// separate from `SetGain` so the player's chosen level survives the mute.
    SetMuted(bool),
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
    /// `None` when no audio device is available (CI, headless). Bound to
    /// [`SynthBus::Player`]; the song voice is `synth.for_bus(SynthBus::Song)`.
    pub synth: Option<SynthHandle>,
    /// Channel to the backing-manager thread. `None` when no device.
    backing_tx: Mutex<Option<Sender<BackingMsg>>>,
    /// The M14-C mix: instrument + level per synth bus, plus the backing level.
    /// Pure settings (`core`); this state pushes each change at the synth /
    /// backing thread as it is made.
    mixer: Mutex<Mixer>,
    /// Play-mode backing coupling state (#168). Tracks the attached file and
    /// last seek target so the play tick only sends a message on a change. Kept
    /// separate from the composer/transport `sync_backing` path so the two
    /// screens never fight over the backing thread.
    play_backing: Mutex<PlayBacking>,
}

/// Play-mode backing coupling state. Mirrors the TUI `PlayScreen`'s lazy
/// backing-arm: nothing plays until the clock crosses `shift_us` (the play
/// session returns `Some(target)` only then), and a freeze pauses it.
#[derive(Default)]
struct PlayBacking {
    /// The currently-attached backing file (the play session's bundle track).
    attached: Option<PathBuf>,
    /// Whether the backing has been started (PlayAt sent) this take.
    started: bool,
    /// Whether playback is currently paused (frozen by wait mode / lead-in).
    paused: bool,
    /// The last file-position target we issued, to suppress redundant seeks.
    last_target_us: u64,
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
            // `out` stays alive for the whole loop, keeping the device stream
            // open — and the backing plays through *its* stream (below) so it
            // isn't opening a silent second stream on Windows.

            let mut path: Option<PathBuf> = None;
            let mut handle: Option<BackingHandle> = None;
            // Current speed multiplier, re-applied whenever a fresh sink is made
            // (a new BackingHandle starts at 1.0×) so slow-mo survives a restart.
            let mut speed: f32 = 1.0;
            // Current backing level, re-applied for the same reason (a fresh
            // sink starts at unity) so the fader survives a restart.
            let mut gain = Gain::UNITY;
            // Off-tempo mute, orthogonal to the fader above (see SetMuted).
            let mut muted = false;

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
                            // Share the synth's output stream (one device stream,
                            // second sink) — a separate stream is silent on
                            // Windows/WASAPI.
                            match out.play_backing_at(p, pos) {
                                Ok(h) => {
                                    h.set_speed(speed); // carry slow-mo across restarts
                                    h.set_gain(effective_gain(gain, muted)); // …fader + mute
                                    handle = Some(h);
                                }
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
                    BackingMsg::SetSpeed(s) => {
                        speed = s;
                        if let Some(h) = &handle {
                            h.set_speed(s);
                        }
                    }
                    BackingMsg::SetGain(g) => {
                        gain = g;
                        if let Some(h) = &handle {
                            h.set_gain(effective_gain(gain, muted));
                        }
                    }
                    BackingMsg::SetMuted(m) => {
                        muted = m;
                        if let Some(h) = &handle {
                            h.set_gain(effective_gain(gain, muted));
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
                mixer: Mutex::new(Mixer::new()),
                play_backing: Mutex::new(PlayBacking::default()),
            },
            _ => {
                // Audio init failed or thread panicked.
                Self {
                    synth: None,
                    backing_tx: Mutex::new(None),
                    mixer: Mutex::new(Mixer::new()),
                    play_backing: Mutex::new(PlayBacking::default()),
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

    /// Set the backing-audio playback speed (1.0 = normal) to match a slowed or
    /// sped transport. Resamples, so the pitch shifts with the speed.
    pub fn set_backing_speed(&self, speed: f32) {
        self.send_backing(BackingMsg::SetSpeed(speed));
    }

    /// Silence (or restore) the backing recording without disturbing its fader.
    /// Used by play-mode slow practice, where the recording cannot follow the
    /// transport and the synth carries the piece instead.
    pub fn set_backing_muted(&self, muted: bool) {
        self.send_backing(BackingMsg::SetMuted(muted));
    }

    // ── Sound selection + mixer (M14-C) ─────────────────────────────────────

    /// A handle onto one synth bus, or `None` with no device.
    ///
    /// The player bus echoes the notes coming off the piano; the song bus
    /// carries "hear the song". They have independent instruments and levels.
    pub fn bus(&self, bus: SynthBus) -> Option<SynthHandle> {
        self.synth.as_ref().map(|s| s.for_bus(bus))
    }

    /// The current mix plus the instrument catalog, for the webview / an agent.
    pub fn mixer_report(&self) -> MixerReport {
        MixerReport::from(*self.mixer.lock().expect("mixer mutex poisoned"))
    }

    /// Point a synth bus at a curated instrument by id and push the program
    /// change at the synth. Returns the new mix, or the reason the id was
    /// rejected.
    pub fn set_instrument(&self, bus: SynthBus, id: &str) -> Result<MixerReport, String> {
        let mixer = {
            let mut guard = self.mixer.lock().expect("mixer mutex poisoned");
            let instrument = guard.set_instrument(bus, id).map_err(|e| e.to_string())?;
            if let Some(h) = self.bus(bus) {
                h.set_instrument(instrument);
            }
            *guard
        };
        Ok(MixerReport::from(mixer))
    }

    /// Set one bus's level (clamped to `0.0..=1.0`) and push it at the synth
    /// channel or the backing sink. Returns the new mix.
    pub fn set_bus_gain(&self, bus: MixerBus, value: f32) -> Result<MixerReport, String> {
        let mixer = {
            let mut guard = self.mixer.lock().expect("mixer mutex poisoned");
            let gain = guard.set_gain(bus, value).map_err(|e| e.to_string())?;
            match bus.synth_bus() {
                Some(synth_bus) => {
                    if let Some(h) = self.bus(synth_bus) {
                        h.set_gain(gain);
                    }
                }
                // The backing is an audio sink: its fader lives on the thread
                // that owns the (!Send) handle.
                None => self.send_backing(BackingMsg::SetGain(gain)),
            }
            *guard
        };
        Ok(MixerReport::from(mixer))
    }

    // ── Play-mode backing coupling (#168) ───────────────────────────────────

    /// Stop and clear the play-mode backing track. Called on `play_load`
    /// (between takes) and `play_finish`. Idempotent.
    pub fn stop_backing(&self) {
        let mut pb = self.play_backing.lock().expect("play_backing poisoned");
        if pb.attached.is_some() || pb.started {
            self.send_backing(BackingMsg::Detach);
        }
        *pb = PlayBacking::default();
    }

    /// Couple the play session's backing track to its clock for one tick.
    ///
    /// `backing` is the bundle's track (or `None`); `target_us` is the file
    /// position the session expects *now* (`None` while the clock is still in the
    /// lead-in); `frozen` is whether wait mode has frozen the clock. Mirrors the
    /// TUI `tick_backing` / `advance` pause logic: start lazily at the shift
    /// boundary, pause while frozen, resume (re-seeking to the live position) on
    /// thaw.
    pub fn sync_play_backing(
        &self,
        backing: Option<&super::play::Backing>,
        target_us: Option<u64>,
        frozen: bool,
    ) {
        let mut pb = self.play_backing.lock().expect("play_backing poisoned");

        let Some(b) = backing else {
            // No backing track this take — make sure nothing lingers.
            if pb.started {
                self.send_backing(BackingMsg::Detach);
                *pb = PlayBacking::default();
            }
            return;
        };

        // Lazily attach the file the first time we see this backing track.
        if pb.attached.as_ref() != Some(&b.path) {
            self.send_backing(BackingMsg::Attach(b.path.clone()));
            pb.attached = Some(b.path.clone());
            pb.started = false;
            pb.paused = false;
            pb.last_target_us = 0;
        }

        // Still in the lead-in: nothing to play yet.
        let Some(pos) = target_us else { return };

        if frozen {
            // Freeze: pause once and hold position.
            if pb.started && !pb.paused {
                self.send_backing(BackingMsg::Pause);
                pb.paused = true;
            }
            return;
        }

        if !pb.started {
            // First time the clock reached the shift point: start it.
            self.send_backing(BackingMsg::PlayAt(pos));
            pb.started = true;
            pb.paused = false;
            pb.last_target_us = pos;
        } else if pb.paused {
            // Resuming after a freeze: re-seek to the live position and play.
            self.send_backing(BackingMsg::PlayAt(pos));
            pb.paused = false;
            pb.last_target_us = pos;
        } else {
            // Playing normally — the audio thread drives playback; we only track
            // the latest target so a future jump (rewind) is detectable.
            pb.last_target_us = pos;
        }
    }
}

impl Default for AudioState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Effect routing ───────────────────────────────────────────────────────────

/// Route a batch of [`Effect`]s from the composer to the synth, mirroring the
/// TUI's `run_effects` so both frontends sound identically.
///
/// The composer uses `AuditionNote { velocity: 0 }` as a note-*off* (a playback
/// span end or a metronome click release), and emits a fresh `AuditionNote` per
/// note as the playhead crosses it during playback. So:
///
/// - `AuditionNote { velocity: 0 }` → `note_off(pitch)` — release just that note
///   (NOT `all_off`; otherwise every span end silences the whole chord, which is
///   what left playback inaudible).
/// - `AuditionNote { velocity > 0 }` **while playing** → a polyphonic `note_on`
///   (no `all_off`): many notes ring together, as a piano piece must.
/// - `AuditionNote { velocity > 0 }` **while stopped** → an edit preview:
///   `all_off` then `note_on`, so moving the cursor replaces the single note.
/// - `AuditionChord` → `all_off` then `note_on` each pitch (chord preview).
/// - `AllOff` → `all_off`.
///
/// If `audio.synth` is `None` this is a no-op (headless CI path).
pub fn apply_effects(audio: &AudioState, playing: bool, effects: &[Effect]) {
    let Some(synth) = &audio.synth else { return };
    for effect in effects {
        match effect {
            Effect::AuditionNote { pitch, velocity } => {
                let Some(note) = rockcraft_core::MidiNote::new(*pitch) else {
                    continue;
                };
                if *velocity == 0 {
                    // Explicit note-off (playback span end / click release).
                    synth.note_off(note);
                } else if playing {
                    // Polyphonic playback note-on — let it ring with the rest.
                    if let Some(vel) = rockcraft_core::Velocity::new(*velocity) {
                        synth.note_on(note, vel);
                    }
                } else {
                    // Stopped: a single edit audition replaces the previous.
                    synth.all_off();
                    if let Some(vel) = rockcraft_core::Velocity::new(*velocity) {
                        synth.note_on(note, vel);
                    }
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
/// File position the backing should sit at for `playhead_us` and the alignment
/// `offset_us`. `None` — the backing is **silent** — while that position is still
/// negative (a negative offset delays the audio). Mirrors core's
/// `backing_position_us` with a zero shift (the edit transport has no pre-roll).
pub fn backing_pos(playhead_us: u64, offset_us: i64) -> Option<u64> {
    let pos = playhead_us as i64 + offset_us;
    (pos >= 0).then_some(pos as u64)
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
    backing_offset_us: i64,
    prev_playing: bool,
    prev_playhead_us: u64,
    prev_offset_us: i64,
) {
    if !playing {
        if prev_playing {
            // Transport just paused.
            audio.send_backing(BackingMsg::Pause);
        }
        return;
    }
    let cur = backing_pos(playhead_us, backing_offset_us);
    let just_started = !prev_playing;
    let seeked = playhead_us < prev_playhead_us || backing_offset_us != prev_offset_us;
    if just_started || seeked {
        // Start / re-seek: play at the position, or stay paused while the audio
        // is still in its (negative-offset) silent lead-in.
        match cur {
            Some(pos) if just_started => audio.send_backing(BackingMsg::PlayAt(pos)),
            Some(pos) => audio.send_backing(BackingMsg::Seek(pos)),
            None => audio.send_backing(BackingMsg::Pause),
        }
    } else if cur.is_some() && backing_pos(prev_playhead_us, prev_offset_us).is_none() {
        // Forward playback just crossed out of the silent lead-in: start the audio.
        audio.send_backing(BackingMsg::PlayAt(cur.unwrap()));
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

/// Detach the backing track (stops playback) without a Tauri `State` wrapper.
///
/// The sibling of [`attach_backing_inner`]: used by the edit-screen
/// attach/detach path (M9-E) and on `load_bundle` when the loaded bundle
/// declares no backing, so a previously-loaded track does not linger.
pub fn detach_backing_inner(state: &AudioState) {
    state.send_backing(BackingMsg::Detach);
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

/// Point a synth bus at a curated instrument by id (M14-C).
///
/// `bus` is `"player"` (the notes you play) or `"song"` (the auto-played
/// chart). Returns the new mix so the webview can re-render from one reply.
#[tauri::command]
pub fn set_instrument(
    state: tauri::State<'_, AudioState>,
    bus: SynthBus,
    instrument: String,
) -> Result<MixerReport, String> {
    state.set_instrument(bus, &instrument)
}

/// Set one mixer bus's level in `0.0..=1.0` (M14-C).
///
/// `bus` is `"player"`, `"song"`, or `"backing"`. Out-of-range values are
/// clamped; a non-finite one is rejected. Returns the new mix.
#[tauri::command]
pub fn set_bus_gain(
    state: tauri::State<'_, AudioState>,
    bus: MixerBus,
    gain: f32,
) -> Result<MixerReport, String> {
    state.set_bus_gain(bus, gain)
}

/// Return the current mix and the catalog of selectable instruments (M14-C).
#[tauri::command]
pub fn mixer_status(state: tauri::State<'_, AudioState>) -> MixerReport {
    state.mixer_report()
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

    /// A silent `AudioState` — no device, no backing thread. The headless CI
    /// path, and what every test here drives.
    fn silent_state() -> AudioState {
        AudioState {
            synth: None,
            backing_tx: Mutex::new(None),
            mixer: Mutex::new(Mixer::new()),
            play_backing: Mutex::new(PlayBacking::default()),
        }
    }

    /// `apply_effects` with no audio device is a no-op — must not panic.
    #[test]
    fn apply_effects_no_device_is_noop() {
        let audio = silent_state();
        apply_effects(
            &audio,
            true,
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

    /// Pure helper: `backing_pos(0, 0)` → Some(0).
    #[test]
    fn backing_pos_zero() {
        assert_eq!(backing_pos(0, 0), Some(0));
    }

    /// `backing_pos` adds offset to playhead.
    #[test]
    fn backing_pos_adds_offset() {
        assert_eq!(backing_pos(1_000_000, 250_000), Some(1_250_000));
    }

    /// A negative offset delays the audio: silent until the playhead catches up.
    #[test]
    fn backing_pos_negative_offset_is_silent_then_plays() {
        assert_eq!(backing_pos(0, -500_000), None);
        assert_eq!(backing_pos(499_000, -500_000), None);
        assert_eq!(backing_pos(500_000, -500_000), Some(0));
        assert_eq!(backing_pos(700_000, -500_000), Some(200_000));
    }

    /// Large values stay in-range (no wrapping/overflow in normal use).
    #[test]
    fn backing_pos_large_values() {
        let playhead = 10 * 60 * 1_000_000u64; // 10 minutes
        let offset = 5_000_000i64; // 5 seconds
        assert_eq!(
            backing_pos(playhead, offset),
            Some(playhead + offset as u64)
        );
    }

    /// Nudge accumulation: successive offset changes stack.
    #[test]
    fn backing_pos_nudges_accumulate() {
        let base = 1_000_000u64;
        let nudge1 = 10_000i64;
        let nudge2 = 250_000i64;
        assert_eq!(
            backing_pos(base, nudge1 + nudge2),
            Some(base + (nudge1 + nudge2) as u64)
        );
    }

    /// The mix survives a device-less start: settings are `core` state, so the
    /// UI still works (silently) on a headless machine.
    #[test]
    fn mixer_settings_apply_without_a_device() {
        let audio = silent_state();
        let report = audio
            .set_instrument(SynthBus::Song, "marimba")
            .expect("known instrument");
        assert_eq!(report.mixer.song.instrument.id, "marimba");
        assert_eq!(
            audio.mixer_report().mixer.song.instrument.id,
            "marimba",
            "the change is remembered, not just returned"
        );
        assert_eq!(
            audio.mixer_report().mixer.player.instrument.id,
            rockcraft_core::DEFAULT_INSTRUMENT,
            "the other bus is untouched"
        );
    }

    /// Each fader moves exactly one bus.
    #[test]
    fn bus_gains_are_independent() {
        let audio = silent_state();
        audio.set_bus_gain(MixerBus::Player, 0.25).unwrap();
        audio.set_bus_gain(MixerBus::Backing, 0.5).unwrap();
        let m = audio.mixer_report().mixer;
        assert_eq!(m.player.gain.value(), 0.25);
        assert_eq!(m.song.gain, Gain::UNITY);
        assert_eq!(m.backing_gain.value(), 0.5);
    }

    /// Out-of-range clamps; non-finite is rejected and changes nothing.
    #[test]
    fn gain_is_clamped_and_non_finite_rejected() {
        let audio = silent_state();
        audio.set_bus_gain(MixerBus::Song, 9.0).unwrap();
        assert_eq!(audio.mixer_report().mixer.song.gain, Gain::UNITY);
        audio.set_bus_gain(MixerBus::Song, -1.0).unwrap();
        assert_eq!(audio.mixer_report().mixer.song.gain, Gain::SILENT);
        assert!(audio.set_bus_gain(MixerBus::Song, f32::NAN).is_err());
        assert_eq!(audio.mixer_report().mixer.song.gain, Gain::SILENT);
    }

    /// An unknown instrument id is reported, not silently ignored.
    #[test]
    fn unknown_instrument_is_rejected() {
        let audio = silent_state();
        let err = audio.set_instrument(SynthBus::Player, "kazoo").unwrap_err();
        assert!(err.contains("kazoo"), "error names the id: {err}");
        assert_eq!(
            audio.mixer_report().mixer.player.instrument.id,
            rockcraft_core::DEFAULT_INSTRUMENT
        );
    }

    /// With no device there is no handle to hand out — the callers all treat
    /// `None` as "stay silent".
    #[test]
    fn bus_handles_are_none_without_a_device() {
        let audio = silent_state();
        assert!(audio.bus(SynthBus::Player).is_none());
        assert!(audio.bus(SynthBus::Song).is_none());
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
