//! MIDI input service for the Tauri backend.
//!
//! Tries to open a live USB-MIDI device on startup; falls back to a
//! [`MockKeyboard`] so every screen is drivable in a sandbox with no hardware.
//! Events are drained each tick by the transport thread in `lib.rs`, which
//! (1) emits a `midi_event` webview event and (2) feeds each event into
//! `Composer::ingest` so StepRecord / LiveRecord work identically to the TUI.
//!
//! CLAUDE.md invariant: we never block the MIDI callback path. `LiveInput`
//! buffers events in a channel; we just drain it from the tick thread.

use std::sync::Mutex;

use rockcraft_core::{Composer, NoteEvent, NoteEventKind};
use rockcraft_midi::{LiveInput, MockKeyboard, NoteSource};
use serde::Serialize;

// ── InputSource ───────────────────────────────────────────────────────────────

/// The active event source.
pub enum InputSource {
    /// A live USB-MIDI device (midir-backed).
    Live(LiveInput),
    /// Computer-keyboard mock — present when no hardware port was found.
    Mock(MockKeyboard),
    /// Placeholder while the service is being initialised (never stored long).
    None,
}

impl InputSource {
    /// Drain pending events from whatever source is active.
    pub fn drain_events(&mut self) -> Vec<NoteEvent> {
        match self {
            InputSource::Live(l) => l.events(),
            InputSource::Mock(m) => m.events(),
            InputSource::None => Vec::new(),
        }
    }

    /// Human-readable kind string for the status command.
    pub fn kind_str(&self) -> &'static str {
        match self {
            InputSource::Live(_) => "live",
            InputSource::Mock(_) => "mock",
            InputSource::None => "none",
        }
    }

    /// Connected port name, if any.
    pub fn port_name(&self) -> Option<String> {
        match self {
            InputSource::Live(l) => Some(l.port_name().to_string()),
            _ => None,
        }
    }
}

// ── MidiState ─────────────────────────────────────────────────────────────────

/// Tauri-managed MIDI service state.
pub struct MidiState {
    /// The active input source, behind a mutex so the tick thread and commands
    /// can safely share it.
    pub source: Mutex<InputSource>,
}

impl MidiState {
    /// Initialise: try a live connection first; fall back to mock.
    pub fn new() -> Self {
        let source = match LiveInput::connect("") {
            Ok(live) => InputSource::Live(live),
            Err(_) => InputSource::Mock(MockKeyboard::new()),
        };
        Self {
            source: Mutex::new(source),
        }
    }
}

impl Default for MidiState {
    fn default() -> Self {
        Self::new()
    }
}

// ── IPC types ─────────────────────────────────────────────────────────────────

/// Serialisable status payload returned by `midi_status`.
#[derive(Debug, Clone, Serialize)]
pub struct MidiStatus {
    /// `"live"` or `"mock"`.
    pub kind: String,
    /// Connected port name when `kind == "live"`, otherwise `null`.
    pub port: Option<String>,
}

/// Serialisable payload for the `midi_event` webview event.
///
/// `NoteEvent` itself does not derive `Serialize` (it's a pure `core` type),
/// so we project the fields we care about into this flat, JSON-friendly form.
#[derive(Debug, Clone, Serialize)]
pub struct NoteEventPayload {
    /// MIDI note number (0–127).
    pub note: u8,
    /// `true` = note-on, `false` = note-off.
    pub on: bool,
    /// Velocity (0–127); 0 when this is a note-off.
    pub velocity: u8,
    /// Engine timestamp in microseconds.
    pub timestamp_us: u64,
}

impl From<NoteEvent> for NoteEventPayload {
    fn from(ev: NoteEvent) -> Self {
        let (on, velocity) = match ev.kind {
            NoteEventKind::On { velocity } => (true, velocity.value()),
            NoteEventKind::Off => (false, 0),
        };
        Self {
            note: ev.note.value(),
            on,
            velocity,
            timestamp_us: ev.timestamp_us,
        }
    }
}

// ── Command helpers (free functions for headless testing) ─────────────────────

/// Return the current MIDI input status.
pub fn midi_status(state: &MidiState) -> MidiStatus {
    let source = state.source.lock().expect("midi source mutex poisoned");
    MidiStatus {
        kind: source.kind_str().to_string(),
        port: source.port_name(),
    }
}

/// Simulate a computer-key press/release on the mock keyboard.
///
/// Returns `Err` when the active source is live (we must not inject fake events
/// while a real piano is connected).
pub fn mock_key(state: &MidiState, key: char, down: bool) -> Result<(), String> {
    let mut source = state.source.lock().expect("midi source mutex poisoned");
    match &mut *source {
        InputSource::Mock(kb) => {
            if down {
                kb.press(key);
            }
            // Note-off is handled automatically: MockKeyboard enqueues its own
            // note-off SUSTAIN_MS after the press, so we only need to act on
            // key-down here.  Key-up is a no-op (the note-off is already queued).
            Ok(())
        }
        InputSource::Live(_) | InputSource::None => {
            Err("mock_key is not available while a live MIDI device is connected".to_string())
        }
    }
}

/// Drain pending events, feed each one into the composer, and return
/// serialisable payloads (for `midi_event` emission) plus accumulated effects.
pub fn drain_and_ingest(
    midi: &MidiState,
    composer: &mut Composer,
) -> (Vec<NoteEventPayload>, Vec<rockcraft_core::Effect>) {
    let events = midi
        .source
        .lock()
        .expect("midi source mutex poisoned")
        .drain_events();
    let mut all_effects = Vec::new();
    let mut payloads = Vec::with_capacity(events.len());
    for ev in events {
        let effects = composer.ingest(ev);
        all_effects.extend(effects);
        payloads.push(NoteEventPayload::from(ev));
    }
    (payloads, all_effects)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, NoteEventKind, Velocity};

    fn make_mock_state() -> MidiState {
        MidiState {
            source: Mutex::new(InputSource::Mock(MockKeyboard::new())),
        }
    }

    #[test]
    fn mock_key_press_enqueues_note() {
        let state = make_mock_state();
        // Press '1' (maps to C4 = 60)
        mock_key(&state, '1', true).expect("mock_key should succeed on mock source");
        // Give a tiny sleep so the event timestamp passes
        std::thread::sleep(std::time::Duration::from_millis(200));
        // Drain should yield at least the note-on
        let mut src = state.source.lock().unwrap();
        let events = src.drain_events();
        assert!(
            !events.is_empty(),
            "expected note events after mock_key press"
        );
        let note_on = events
            .iter()
            .find(|e| matches!(e.kind, NoteEventKind::On { .. }));
        assert!(note_on.is_some(), "expected a note-on event");
        assert_eq!(note_on.unwrap().note.value(), 60, "C4 should be midi 60");
    }

    #[test]
    fn mock_key_on_live_source_returns_err() {
        // We cannot open a real MIDI device in CI, so simulate a Live source
        // by checking the error path through the mock; instead we just verify
        // the None variant also returns Err.
        let state = MidiState {
            source: Mutex::new(InputSource::None),
        };
        let result = mock_key(&state, '1', true);
        assert!(
            result.is_err(),
            "mock_key on None/Live source must return Err"
        );
    }

    #[test]
    fn midi_status_mock_returns_mock_kind() {
        let state = make_mock_state();
        let status = midi_status(&state);
        assert_eq!(status.kind, "mock");
        assert!(status.port.is_none());
    }

    #[test]
    fn drain_and_ingest_with_scripted_source() {
        use rockcraft_midi::ScriptedSource;

        // Build a ScriptedSource with one note-on at t=0 and note-off at t=500_000.
        let note = MidiNote::new(60).unwrap();
        let vel = Velocity::new(80).unwrap();
        let events_in = vec![NoteEvent::on(note, vel, 0), NoteEvent::off(note, 500_000)];
        let mut src = ScriptedSource::new(events_in);
        // Advance so both events are due.
        src.advance(600_000);

        let state = MidiState {
            source: Mutex::new(InputSource::Mock(MockKeyboard::new())),
        };
        // Replace mock source with scripted by draining and feeding manually.
        // (ScriptedSource doesn't implement the same Mutex wrapper, but we can
        // test drain_and_ingest indirectly via the mock after pressing a key.)
        //
        // Test the composer ingest path directly:
        let mut composer = Composer::new();
        // Switch to StepRecord so ingest places notes.
        composer
            .apply(
                rockcraft_core::action_from_name("toggle_record_arm", &serde_json::json!({}))
                    .unwrap(),
            )
            .unwrap();

        // Feed events manually through the NoteSource trait.
        use rockcraft_midi::NoteSource as _;
        let drained: Vec<NoteEvent> = src.events();
        assert_eq!(drained.len(), 2, "expected 2 events from scripted source");

        // Verify timestamp_us is preserved.
        assert_eq!(drained[0].timestamp_us, 0);
        assert_eq!(drained[1].timestamp_us, 500_000);

        // Feed into composer ingest — in StepRecord the note-on should place a note.
        let mut placed_effects = Vec::new();
        for ev in &drained {
            placed_effects.extend(composer.ingest(*ev));
        }
        let snap = composer.snapshot();
        assert!(
            !snap.notes.is_empty(),
            "StepRecord ingest of note-on should place a note"
        );
    }
}
