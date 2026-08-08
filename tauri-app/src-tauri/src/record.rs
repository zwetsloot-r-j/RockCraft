//! Live-recording session for the Tauri frontend.
//!
//! A [`RecordSession`] is the Tauri counterpart of `crates/tui/src/record.rs`.
//! It accumulates MIDI events into an [`EventBuffer`] (rebased to a session
//! origin, exactly as the TUI does) and serialises the result to a standard
//! bundle directory (`recordings/take-<unix_ts>/`).
//!
//! The session is owned by [`RecordState`], a Tauri-managed mutex.  The tick
//! thread in `lib.rs` calls [`RecordState::push`] for every MIDI event that
//! arrives while a session is active — recording therefore does not depend on
//! the webview being awake.
//!
//! Three Tauri commands are exposed:
//! - [`record_start`]   — opens a session, optionally starting backing audio.
//! - [`record_stop`]    — closes (discards) the current session.
//! - [`record_save`]    — saves the bundle and returns its directory path.
//!
//! ## Backing-track origin anchoring
//!
//! When `record_start` is called with a backing path the session records
//! `backing_start_wall = Instant::now()`.  The first MIDI event received by
//! [`RecordSession::push`] sets the origin:
//!
//! ```text
//! origin_us = event.timestamp_us - elapsed_since_backing_start
//! ```
//!
//! This is the same formula the TUI uses and guarantees the saved MIDI aligns
//! with the audio file.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use rockcraft_core::{BackingTrack, EventBuffer, NoteEvent, RecordingMeta, TrackOrigin};
use rockcraft_midi::events_to_smf_bytes;
use tauri::State;

/// Where saved takes go. Relative to the working directory.
const RECORDINGS_DIR: &str = "recordings";

// ── RecordBacking ──────────────────────────────────────────────────────────────

/// Metadata kept while recording with a backing track.
struct RecordBacking {
    /// Original (absolute) path to the backing audio file.
    path: PathBuf,
    /// Wall-clock instant when the backing started playing.
    started_at: Instant,
}

// ── RecordSession ─────────────────────────────────────────────────────────────

/// An active live-recording session.
struct RecordSession {
    buffer: EventBuffer,
    /// MIDI timestamp of the recording origin.  `None` until the first event.
    origin_us: Option<u64>,
    /// Backing-track metadata, if a path was supplied to `record_start`.
    backing: Option<RecordBacking>,
}

impl RecordSession {
    fn new(backing_path: Option<PathBuf>) -> Self {
        let backing = backing_path.map(|path| RecordBacking {
            path,
            started_at: Instant::now(),
        });
        Self {
            buffer: EventBuffer::new(),
            origin_us: None,
            backing,
        }
    }

    /// Ingest one MIDI event into the session buffer.
    ///
    /// The origin is anchored on the first call (identical logic to the TUI's
    /// `RecordScreen::ingest`).
    fn push(&mut self, ev: NoteEvent) {
        let backing_start = self.backing.as_ref().map(|b| b.started_at);
        let origin = *self.origin_us.get_or_insert_with(|| match backing_start {
            Some(start) => {
                let elapsed = start.elapsed().as_micros() as u64;
                ev.timestamp_us.saturating_sub(elapsed)
            }
            None => ev.timestamp_us,
        });
        let rebased = rebase_event(ev, origin);
        self.buffer.push(rebased);
    }

    /// Save the session as a bundle under `recordings/take-<unix_ts>/`.
    ///
    /// Returns `Err` if the buffer is empty (nothing recorded).
    fn save(&self) -> std::io::Result<PathBuf> {
        if self.buffer.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "nothing recorded — buffer is empty",
            ));
        }

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bundle_dir = std::path::Path::new(RECORDINGS_DIR).join(format!("take-{stamp}"));
        std::fs::create_dir_all(&bundle_dir)?;

        // Write MIDI.
        let bytes = events_to_smf_bytes(self.buffer.events());
        std::fs::write(bundle_dir.join("song.mid"), bytes)?;

        // Optionally copy the backing audio and record its filename.
        let backing_meta = if let Some(ref b) = self.backing {
            let filename = bundle_backing_filename(&b.path);
            std::fs::copy(&b.path, bundle_dir.join(&filename))?;
            Some(BackingTrack {
                file: filename,
                audio_start_us: 0,
            })
        } else {
            None
        };

        // Write meta.json.
        let meta = RecordingMeta {
            midi_file: "song.mid".to_string(),
            backing: backing_meta,
            grid: None,
            key: None,
            origin: Some(TrackOrigin::Recorded),
            video: None,
            backgrounds: Vec::new(),
            version: 1,
        };
        std::fs::write(bundle_dir.join("meta.json"), meta.to_json())?;

        Ok(bundle_dir)
    }

    /// Number of events captured so far.
    fn event_count(&self) -> usize {
        self.buffer.len()
    }

    /// Elapsed recording time in microseconds (origin to latest event), or 0 if
    /// no events have arrived yet.
    fn elapsed_us(&self) -> u64 {
        self.buffer.duration_us()
    }
}

// ── RecordState ───────────────────────────────────────────────────────────────

/// Tauri-managed live-recording state.
///
/// `None` when no session is active; `Some(session)` during recording.
/// Behind a `Mutex` so the tick thread (event ingestion) and command handlers
/// (start / stop / save) can share it safely.
pub struct RecordState {
    session: Mutex<Option<RecordSession>>,
}

impl RecordState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
        }
    }

    /// Called by the tick thread for every MIDI event.
    ///
    /// If a session is active the event is appended; otherwise this is a no-op.
    pub fn push(&self, ev: NoteEvent) {
        let mut guard = self.session.lock().expect("record session mutex poisoned");
        if let Some(ref mut session) = *guard {
            session.push(ev);
        }
    }
}

impl Default for RecordState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

/// Status payload returned by [`record_status`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordStatus {
    /// Whether a session is currently active.
    pub active: bool,
    /// Elapsed time in microseconds (0 when inactive).
    pub elapsed_us: u64,
    /// Number of events captured so far (0 when inactive).
    pub event_count: usize,
}

/// Start a new recording session.
///
/// If `backing` is `Some(path)` the audio file at that path starts playing
/// through the existing audio state immediately (via `attach_backing` +
/// `PlayAt(0)`), and the origin offset will be computed from the wall-clock
/// instant of this call.
///
/// Returns `Err` if a session is already active.
#[tauri::command]
pub fn record_start(
    record: State<'_, RecordState>,
    audio: State<'_, crate::audio::AudioState>,
    backing: Option<String>,
) -> Result<(), String> {
    let mut guard = record
        .session
        .lock()
        .expect("record session mutex poisoned");
    if guard.is_some() {
        return Err("recording already in progress".to_string());
    }

    let backing_path = backing.as_deref().map(PathBuf::from);

    // Start backing audio immediately if a path was provided.
    if let Some(ref p) = backing_path {
        if !p.exists() {
            return Err(format!("backing file not found: {}", p.display()));
        }
        crate::audio::attach_backing_inner(&audio, p.clone());
        crate::audio::play_backing_now(&audio);
    }

    *guard = Some(RecordSession::new(backing_path));
    Ok(())
}

/// Stop the current recording session without saving.
///
/// The captured buffer is discarded. This is a no-op when no session is active.
#[tauri::command]
pub fn record_stop(record: State<'_, RecordState>) {
    let mut guard = record
        .session
        .lock()
        .expect("record session mutex poisoned");
    *guard = None;
}

/// Save the current session as a bundle and return the bundle directory path.
///
/// Returns `Err` if no session is active, the buffer is empty, or an I/O error
/// occurs. The session remains active after saving (the user can continue
/// recording into the same take).
#[tauri::command]
pub fn record_save(record: State<'_, RecordState>) -> Result<String, String> {
    let guard = record
        .session
        .lock()
        .expect("record session mutex poisoned");
    match guard.as_ref() {
        None => Err("no recording in progress".to_string()),
        Some(session) => session
            .save()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| e.to_string()),
    }
}

/// Return the current recording status (active, elapsed, event count).
#[tauri::command]
pub fn record_status(record: State<'_, RecordState>) -> RecordStatus {
    let guard = record
        .session
        .lock()
        .expect("record session mutex poisoned");
    match guard.as_ref() {
        None => RecordStatus {
            active: false,
            elapsed_us: 0,
            event_count: 0,
        },
        Some(session) => RecordStatus {
            active: true,
            elapsed_us: session.elapsed_us(),
            event_count: session.event_count(),
        },
    }
}

// ── Pure helpers (tested headlessly) ─────────────────────────────────────────

/// Rebase a note event so its timestamp is relative to `origin_us`.
///
/// A pure helper with no I/O — identical to `crates/tui/src/record.rs::rebase_event`.
pub fn rebase_event(ev: NoteEvent, origin_us: u64) -> NoteEvent {
    NoteEvent {
        timestamp_us: ev.timestamp_us.saturating_sub(origin_us),
        ..ev
    }
}

/// Return the bundle-relative filename for a backing audio file.
pub fn bundle_backing_filename(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".to_string());
    format!("backing.{ext}")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, NoteEventKind, Velocity};

    fn note60() -> MidiNote {
        MidiNote::new(60).unwrap()
    }
    fn vel80() -> Velocity {
        Velocity::new(80).unwrap()
    }
    fn on_ev(ts: u64) -> NoteEvent {
        NoteEvent::on(note60(), vel80(), ts)
    }
    fn off_ev(ts: u64) -> NoteEvent {
        NoteEvent::off(note60(), ts)
    }

    // ── rebase_event ─────────────────────────────────────────────────────────

    #[test]
    fn rebase_shifts_by_origin() {
        let rebased = rebase_event(on_ev(1_000_000), 500_000);
        assert_eq!(rebased.timestamp_us, 500_000);
    }

    #[test]
    fn rebase_saturates_at_zero() {
        let rebased = rebase_event(on_ev(100), 500);
        assert_eq!(rebased.timestamp_us, 0);
    }

    #[test]
    fn rebase_zero_origin_is_identity() {
        let rebased = rebase_event(on_ev(42_000), 0);
        assert_eq!(rebased.timestamp_us, 42_000);
    }

    #[test]
    fn rebase_preserves_note_and_kind() {
        let rebased = rebase_event(on_ev(1_000), 500);
        assert_eq!(rebased.note.value(), 60);
        assert!(matches!(rebased.kind, NoteEventKind::On { .. }));
    }

    // ── bundle_backing_filename ───────────────────────────────────────────────

    #[test]
    fn backing_filename_preserves_extension() {
        assert_eq!(
            bundle_backing_filename(std::path::Path::new("track.mp3")),
            "backing.mp3"
        );
        assert_eq!(
            bundle_backing_filename(std::path::Path::new("/some/path/music.ogg")),
            "backing.ogg"
        );
    }

    #[test]
    fn backing_filename_fallback_for_no_extension() {
        assert_eq!(
            bundle_backing_filename(std::path::Path::new("noext")),
            "backing.audio"
        );
    }

    // ── RecordSession ─────────────────────────────────────────────────────────

    #[test]
    fn session_empty_buffer_save_fails() {
        let session = RecordSession::new(None);
        let err = session.save().unwrap_err();
        assert!(err.to_string().contains("empty"), "expected empty error");
    }

    #[test]
    fn session_push_sets_origin_on_first_event() {
        let mut session = RecordSession::new(None);
        assert!(session.origin_us.is_none());
        session.push(on_ev(5_000));
        assert_eq!(session.origin_us, Some(5_000));
    }

    #[test]
    fn session_push_rebases_events() {
        let mut session = RecordSession::new(None);
        session.push(on_ev(5_000));
        session.push(off_ev(10_000));
        let evs = session.buffer.events();
        assert_eq!(evs[0].timestamp_us, 0, "first event should be at t=0");
        assert_eq!(evs[1].timestamp_us, 5_000, "second event rebased by 5_000");
    }

    #[test]
    fn session_elapsed_us_zero_before_events() {
        let session = RecordSession::new(None);
        assert_eq!(session.elapsed_us(), 0);
    }

    #[test]
    fn session_elapsed_us_after_events() {
        let mut session = RecordSession::new(None);
        session.push(on_ev(1_000));
        session.push(off_ev(3_000));
        // After rebasing, origin=1_000 so elapsed = 3_000 − 1_000 = 2_000
        assert_eq!(session.elapsed_us(), 2_000);
    }

    #[test]
    fn session_event_count() {
        let mut session = RecordSession::new(None);
        assert_eq!(session.event_count(), 0);
        session.push(on_ev(0));
        session.push(off_ev(1_000));
        assert_eq!(session.event_count(), 2);
    }

    // ── Backing-anchored origin ───────────────────────────────────────────────

    /// When a backing starts Δ µs before the first event, the origin should be
    /// offset by that Δ so saved timestamps align with the audio.
    #[test]
    fn backing_origin_offset_by_delta() {
        // Simulate: backing started 200 ms (200_000 µs) before the first note.
        // We fake this by giving the session a backing whose `started_at` is
        // sufficiently in the past.
        let past = Instant::now()
            .checked_sub(std::time::Duration::from_micros(200_000))
            .expect("instant subtract");
        let mut session = RecordSession {
            buffer: EventBuffer::new(),
            origin_us: None,
            backing: Some(RecordBacking {
                path: PathBuf::from("fake.wav"),
                started_at: past,
            }),
        };
        // First note arrives at timestamp 1_000_000 µs.
        // elapsed_since_backing_start ≈ 200_000 µs (may be slightly more).
        session.push(on_ev(1_000_000));
        let origin = session.origin_us.unwrap();
        // origin should be close to 1_000_000 − 200_000 = 800_000 µs.
        // Allow ±10 ms slop for test scheduling jitter.
        let delta = (origin as i64 - 800_000i64).unsigned_abs();
        assert!(
            delta < 10_000,
            "origin {origin} not within 10ms of expected 800_000"
        );
    }

    // ── RecordState ───────────────────────────────────────────────────────────

    #[test]
    fn record_state_inactive_by_default() {
        let state = RecordState::new();
        let session = state.session.lock().unwrap();
        assert!(session.is_none(), "session should be None on startup");
    }

    #[test]
    fn record_state_push_ignored_when_inactive() {
        let state = RecordState::new();
        state.push(on_ev(0)); // must not panic
        let session = state.session.lock().unwrap();
        assert!(
            session.is_none(),
            "push with no session must not create a session"
        );
    }

    // ── save integration: round-trip ─────────────────────────────────────────

    #[test]
    fn save_bundle_round_trip() {
        use rockcraft_midi::smf_bytes_to_events;
        let tmp = tempfile::tempdir().expect("tempdir");
        // Patch RECORDINGS_DIR: we write into the tempdir directly.
        let bundle_dir = tmp.path().join("take-test");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        // Build a session manually and exercise the low-level save path.
        let mut session = RecordSession::new(None);
        session.push(NoteEvent::on(note60(), vel80(), 1_000));
        session.push(NoteEvent::off(note60(), 500_000));

        // We can't invoke the actual `save()` because it uses a fixed path.
        // Instead exercise the same steps manually.
        let bytes = events_to_smf_bytes(session.buffer.events());
        let mid_path = bundle_dir.join("song.mid");
        std::fs::write(&mid_path, &bytes).unwrap();
        let meta = RecordingMeta {
            midi_file: "song.mid".to_string(),
            backing: None,
            grid: None,
            key: None,
            origin: Some(TrackOrigin::Recorded),
            video: None,
            backgrounds: Vec::new(),
            version: 1,
        };
        std::fs::write(bundle_dir.join("meta.json"), meta.to_json()).unwrap();

        // Re-parse the MIDI.
        let raw = std::fs::read(&mid_path).unwrap();
        let reparsed = smf_bytes_to_events(&raw).unwrap();
        // The buffer has 2 events (note-on, note-off).
        assert_eq!(reparsed.len(), 2, "expected 2 events in saved MIDI");
        assert_eq!(reparsed[0].note.value(), 60);
        assert!(matches!(reparsed[0].kind, NoteEventKind::On { .. }));

        // Re-parse meta.json.
        let meta_raw = std::fs::read_to_string(bundle_dir.join("meta.json")).unwrap();
        let back_meta = RecordingMeta::from_json(&meta_raw).unwrap();
        assert_eq!(
            back_meta.origin,
            Some(TrackOrigin::Recorded),
            "meta.json must declare origin=Recorded"
        );
        assert!(back_meta.backing.is_none());
    }
}
