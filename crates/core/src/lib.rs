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

pub mod action;
pub mod buffer;
pub mod chord;
pub mod composer;
pub mod events;
pub mod grid;
pub mod history;
pub mod play_clock;
pub mod scoring;
pub mod segment;
pub mod song;
pub mod stats;
pub mod timeline;
pub mod wait;

pub use action::{
    action_from_name, action_help, action_names, Action, ActionError, ActionInfo, Effect, ParamInfo,
};
pub use buffer::EventBuffer;
pub use chord::{ChordKind, Key, Scale};
pub use composer::{Composer, ComposerSnapshot, Cursor, InputMode, NoteView, SelectionView};
pub use events::{MidiNote, NoteEvent, NoteEventKind, Velocity};
pub use grid::{Grid, Subdivision, TimeSig};
pub use history::History;
pub use play_clock::PlayClock;
pub use scoring::{score, ExpectedNote, Feedback, NoteJudgment, ScoreConfig, ScoreReport, Timing};
pub use segment::{segments_from_splits, slice_segment, Segment, SliceResult};
pub use song::{
    backing_position_us, song_shift_us, BackgroundVideo, BackingTrack, MetaError, RecordingMeta,
    TrackOrigin,
};
pub use stats::Summary;
pub use timeline::{Note, NoteId, Timeline};
pub use wait::{GateState, Step, WaitGate, WaitTracker};
