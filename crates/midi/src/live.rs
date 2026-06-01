//! Live MIDI input via `midir`.
//!
//! Design (this is the real-time path — see `CLAUDE.md`):
//! - `midir` invokes our callback on a high-priority MIDI thread. We do the
//!   *minimum* there: parse the raw bytes into a [`NoteEvent`] and hand it off
//!   through a channel. No allocation-heavy work, no locks held across the
//!   send, no blocking — the channel send is the only synchronisation.
//! - The application thread drains the channel at its own pace (into an
//!   `EventBuffer`, a renderer, etc.).
//!
//! The byte-level parsing ([`parse_note_message`]) is pure and unit-tested with
//! no device, so this crate stays testable in CI; only [`LiveInput::connect`]
//! needs real hardware.

use rockcraft_core::{MidiNote, NoteEvent, Velocity};
use std::sync::mpsc::{self, Receiver};

use midir::{Ignore, MidiInput, MidiInputConnection};

/// MIDI status nibble for note-off (upper nibble of the status byte).
const STATUS_NOTE_OFF: u8 = 0x80;
/// MIDI status nibble for note-on.
const STATUS_NOTE_ON: u8 = 0x90;

/// Errors from opening a live MIDI connection.
#[derive(Debug)]
pub enum LiveInputError {
    /// Could not initialise the MIDI backend.
    Init(String),
    /// No input port matched the requested name.
    NoMatchingPort {
        wanted: String,
        available: Vec<String>,
    },
    /// The backend refused the connection.
    Connect(String),
}

impl std::fmt::Display for LiveInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveInputError::Init(e) => write!(f, "MIDI init failed: {e}"),
            LiveInputError::NoMatchingPort { wanted, available } => write!(
                f,
                "no MIDI input port matching {wanted:?}; available: {available:?}"
            ),
            LiveInputError::Connect(e) => write!(f, "MIDI connect failed: {e}"),
        }
    }
}

impl std::error::Error for LiveInputError {}

/// Translate a raw MIDI message into a [`NoteEvent`], or `None` if it is not a
/// note-on/note-off we care about (control change, clock, active sensing, etc.).
///
/// `timestamp_us` is the engine timestamp to stamp the event with. Per MIDI
/// convention, a note-on with velocity 0 is treated as a note-off.
pub fn parse_note_message(data: &[u8], timestamp_us: u64) -> Option<NoteEvent> {
    // Note messages are 3 bytes: status, key, velocity.
    if data.len() < 3 {
        return None;
    }
    let status = data[0] & 0xF0;
    let note = MidiNote::new(data[1])?;

    match status {
        STATUS_NOTE_ON => {
            let raw_vel = data[2];
            match Velocity::new(raw_vel)? {
                v if v.is_note_off() => Some(NoteEvent::off(note, timestamp_us)),
                v => Some(NoteEvent::on(note, v, timestamp_us)),
            }
        }
        STATUS_NOTE_OFF => Some(NoteEvent::off(note, timestamp_us)),
        _ => None,
    }
}

/// An open live MIDI input connection. Drop to disconnect.
///
/// Hold onto this value: dropping it closes the port. Pull events with
/// [`LiveInput::events`].
pub struct LiveInput {
    // Kept alive so the connection (and its callback thread) stays open.
    _connection: MidiInputConnection<()>,
    receiver: Receiver<NoteEvent>,
    port_name: String,
}

impl LiveInput {
    /// Connect to the first input port whose name contains `name_filter`
    /// (case-insensitive). Pass `""` to take the first available port.
    ///
    /// The PX-150 enumerates as `CASIO USB-MIDI`, so `"casio"` selects it.
    pub fn connect(name_filter: &str) -> Result<Self, LiveInputError> {
        let mut midi_in =
            MidiInput::new("RockCraft").map_err(|e| LiveInputError::Init(e.to_string()))?;
        // Drop active-sensing, timing clock, and sysex at the source — the
        // PX-150 streams active sensing (0xFE) constantly when idle.
        midi_in.ignore(Ignore::All);

        let ports = midi_in.ports();
        let names: Vec<String> = ports
            .iter()
            .map(|p| midi_in.port_name(p).unwrap_or_default())
            .collect();

        let wanted = name_filter.to_lowercase();
        let idx = names
            .iter()
            .position(|n| n.to_lowercase().contains(&wanted))
            .ok_or_else(|| LiveInputError::NoMatchingPort {
                wanted: name_filter.to_string(),
                available: names.clone(),
            })?;

        let port = &ports[idx];
        let port_name = names[idx].clone();

        let (tx, receiver) = mpsc::channel::<NoteEvent>();

        let connection = midi_in
            .connect(
                port,
                "rockcraft-in",
                move |stamp_us, message, _| {
                    // RUNS ON THE MIDI THREAD — keep it tiny, never block.
                    if let Some(ev) = parse_note_message(message, stamp_us) {
                        // If the receiver is gone we're shutting down; ignore.
                        let _ = tx.send(ev);
                    }
                },
                (),
            )
            .map_err(|e| LiveInputError::Connect(e.to_string()))?;

        Ok(Self {
            _connection: connection,
            receiver,
            port_name,
        })
    }

    /// The name of the connected port (e.g. `"CASIO USB-MIDI"`).
    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Non-blocking iterator over note events received since the last call.
    pub fn events(&self) -> impl Iterator<Item = NoteEvent> + '_ {
        self.receiver.try_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::NoteEventKind;

    #[test]
    fn parses_note_on() {
        // channel 0 note-on, middle C, velocity 100
        let ev = parse_note_message(&[0x90, 60, 100], 1_000).unwrap();
        assert_eq!(ev.note.value(), 60);
        assert_eq!(ev.timestamp_us, 1_000);
        assert!(matches!(ev.kind, NoteEventKind::On { .. }));
    }

    #[test]
    fn note_on_velocity_zero_is_off() {
        let ev = parse_note_message(&[0x90, 60, 0], 5).unwrap();
        assert_eq!(ev.kind, NoteEventKind::Off);
    }

    #[test]
    fn parses_note_off() {
        let ev = parse_note_message(&[0x80, 64, 40], 7).unwrap();
        assert_eq!(ev.note.value(), 64);
        assert_eq!(ev.kind, NoteEventKind::Off);
    }

    #[test]
    fn ignores_non_note_messages() {
        // control change (0xB0) and active sensing (0xFE) are not notes
        assert!(parse_note_message(&[0xB0, 7, 127], 0).is_none());
        assert!(parse_note_message(&[0xFE], 0).is_none());
    }

    #[test]
    fn ignores_truncated_messages() {
        assert!(parse_note_message(&[0x90, 60], 0).is_none());
        assert!(parse_note_message(&[], 0).is_none());
    }

    #[test]
    fn note_on_high_channel_still_parsed() {
        // status 0x95 = note-on on channel 5; upper nibble still 0x90
        let ev = parse_note_message(&[0x95, 72, 80], 0).unwrap();
        assert!(matches!(ev.kind, NoteEventKind::On { .. }));
    }
}
