//! Live MIDI input and MIDI-file parse/record for RockCraft.
//!
//! At M0 this gains:
//!   - `midir`-backed live input that emits `rockcraft_core::NoteEvent`s, and
//!   - `midly`-backed file loading/recording.
//!
//! The crate boundary keeps all device- and file-format concerns out of
//! `core`, so `core` stays headless-testable.

// Re-exported so downstream crates have a single import surface.
pub use rockcraft_core as core;

pub mod file;
pub mod live;
pub mod mock;
pub mod source;

pub use file::{events_to_smf_bytes, smf_bytes_to_events, MidiFileError};
pub use live::{parse_note_message, LiveInput, LiveInputError};
pub use mock::{key_map, MockKeyboard, ScriptedSource};
pub use source::NoteSource;
