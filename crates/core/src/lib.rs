//! RockCraft core engine.
//!
//! This crate is deliberately **UI-agnostic and I/O-agnostic**: it knows
//! nothing about MIDI devices, audio output, or terminals. Everything here is
//! pure and headless-testable so it can be developed and verified — by humans
//! or remote/cloud agents — against recorded fixtures, no piano required.
//!
//! The module layout is the project's stable contract; frontends (TUI, Tauri,
//! Godot) and I/O crates (`midi`, `audio`) depend *on* these types, never the
//! other way around.

pub mod buffer;
pub mod events;
pub mod scoring;
pub mod song;
pub mod stats;
pub mod wait;

pub use buffer::EventBuffer;
pub use events::{MidiNote, NoteEvent, NoteEventKind, Velocity};
pub use scoring::{score, ExpectedNote, NoteJudgment, ScoreConfig, ScoreReport, Timing};
pub use song::{backing_position_us, song_shift_us, BackingTrack, MetaError, RecordingMeta};
pub use stats::Summary;
pub use wait::{Step, WaitTracker};
