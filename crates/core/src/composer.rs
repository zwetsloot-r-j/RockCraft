//! Pure composer state machine — the editor lifted out of the TUI.
//!
//! [`Composer`] owns the same cursor / grab / chord / selection / clipboard /
//! input-mode / transport / loop / metronome / count-in logic that lived in
//! `crates/tui/src/edit.rs::EditScreen`, but with **no device, synth, terminal,
//! or wall clock**. Every frontend (TUI now; Tauri/Godot later) and the
//! WebSocket interface drive the *same* editor through [`Composer::apply`].
//!
//! Two seams keep it pure and headless-testable:
//!
//! - **Audition is described, not performed.** [`apply`](Composer::apply),
//!   [`advance`](Composer::advance), and [`ingest`](Composer::ingest) return an
//!   ordered `Vec<`[`Effect`]`>`; the frontend owns the synth and "currently
//!   sounding". A single-note audition is [`Effect::AuditionNote`], a chord is
//!   [`Effect::AuditionChord`], and silencing everything is [`Effect::AllOff`].
//!   During playback a span's note-on is an `AuditionNote` at the default
//!   velocity and its note-off is the same pitch with velocity `0` (the MIDI
//!   note-off convention), since the fixed M4-A [`Effect`] vocabulary has no
//!   dedicated per-note off.
//! - **Time is injected.** The playhead is a plain `u64`; the frontend owns the
//!   clock and advances it via [`advance`](Composer::advance). There is no
//!   `Instant` here, honouring "decouple rendering from timing".
//!
//! This is a **port, not a redesign**: semantics match `EditScreen` 1:1 so the
//! M4-C TUI can delegate here without behaviour drift.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionError, Effect};
use crate::chord::{ChordKind, Key, Scale};
use crate::events::{MidiNote, NoteEvent, NoteEventKind, Velocity};
use crate::grid::{Grid, Subdivision, TimeSig};
use crate::history::History;
use crate::timeline::{Note, NoteId, Timeline};

/// Lowest MIDI note on an 88-key piano (A0).
const LOWEST_MIDI: u8 = 21;
/// Highest MIDI note on an 88-key piano (C8).
const HIGHEST_MIDI: u8 = 108;
/// One semitone above A0 × octave for the default cursor: middle C (MIDI 60).
const DEFAULT_CURSOR_PITCH: u8 = 60;
/// Default velocity for newly added (and auditioned) notes.
const DEFAULT_NOTE_VEL: u8 = 80;
/// Maximum entries on the undo stack; oldest checkpoints drop past this.
const HISTORY_CAPACITY: usize = 100;
/// MIDI note value used for metronome clicks (E5 — a bright piano tone).
const CLICK_MIDI_VALUE: u8 = 76;
/// Duration of each metronome click note in µs (50 ms).
const CLICK_DUR_US: u64 = 50_000;
/// Velocity for beat-1 accent clicks.
const CLICK_VEL_ACCENT: u8 = 110;
/// Velocity for off-beat clicks.
const CLICK_VEL_NORMAL: u8 = 80;

/// A `(pitch, step)` editing cursor.
///
/// `pitch` is a MIDI note constrained to the 88-key range `21..=108`; `step` is
/// a grid-step index along the time axis (its microsecond position is
/// `grid.us_of_step(step)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub pitch: u8,
    pub step: u64,
}

/// How notes get into the editor.
///
/// - `DirectEdit`: cursor + actions place notes; played MIDI is ignored.
/// - `StepRecord`: each played note-on lands at the cursor (with the *played*
///   pitch) and the cursor steps forward one grid step — no transport needed.
/// - `LiveRecord`: played on/off events are written into the timeline at the
///   current record playhead µs (snapped to grid), pairing on→off into a `Note`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMode {
    DirectEdit,
    StepRecord,
    LiveRecord,
}

/// Live state of the key-aware chord selector.
///
/// While active, the composer keeps a *preview* chord inserted in the timeline
/// so it renders as a ghost and auditions. Cycling the degree or quality
/// replaces that preview in place (it never piles up). Committing makes the
/// preview permanent; cancelling removes it via `History::rollback`.
struct ChordMode {
    /// Scale degree `1..=7` of the chord under construction.
    degree: u8,
    /// Triad (3 notes) or Seventh (4 notes).
    kind: ChordKind,
    /// Ids of the notes currently previewing this chord in the timeline.
    preview_ids: Vec<NoteId>,
    /// The pitches of the current preview (ascending), exposed for tests.
    pitches: Vec<MidiNote>,
}

/// A sustained note span used to fire playback auditions — the same pairing the
/// TUI highway's `build_spans` performs, reproduced here so `core` owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NoteSpan {
    note: u8,
    start_us: u64,
    end_us: u64,
}

/// The pure composer state machine. See the module docs for the contract.
pub struct Composer {
    history: History,
    grid: Grid,
    cursor: Cursor,
    /// The note currently held in grab mode; `None` when not grabbing.
    grabbed: Option<NoteId>,
    /// The piece's key, used to voice diatonic chords. Default C major.
    key: Key,
    /// Live chord-selector state; `None` when not in chord mode.
    chord: Option<ChordMode>,
    /// Pitches of the most recently committed chord, exposed for tests.
    last_committed: Vec<MidiNote>,
    /// How played MIDI is consumed: direct-edit (ignore) vs step / live record.
    input_mode: InputMode,
    /// Playhead used to place `LiveRecord` notes. A seam the frontend drives via
    /// [`Composer::set_playhead_us`] / [`Action::SetPlayhead`].
    record_playhead_us: u64,
    /// Note-ons awaiting their off during `LiveRecord`: `(pitch, snapped start
    /// µs, velocity)`. Closed into a `Note` when the matching off arrives.
    live_pending: Vec<(MidiNote, u64, Velocity)>,

    // ── transport (pure µs) ───────────────────────────────────────────────
    /// Whether the transport is playing.
    playing: bool,
    /// Playhead position in song µs while playing, advanced by injected time.
    transport_us: u64,
    /// Spans cached from the timeline at the moment playback started.
    audition_spans: Vec<NoteSpan>,
    /// Span indices whose note_on has been emitted during this playback.
    audition_on_fired: HashSet<usize>,
    /// Span indices whose note_off has been emitted during this playback.
    audition_off_fired: HashSet<usize>,

    // ── loop region ────────────────────────────────────────────────────────
    loop_enabled: bool,
    loop_start_us: u64,
    loop_end_us: u64,

    // ── metronome ──────────────────────────────────────────────────────────
    metronome_enabled: bool,
    last_click_beat: Option<u64>,
    click_pending_off: Option<u64>,
    click_count: usize,

    // ── count-in ───────────────────────────────────────────────────────────
    counting_in: bool,
    count_in_end_us: u64,
    count_in_bars: u32,

    // ── selection / clipboard ──────────────────────────────────────────────
    /// Anchor of a visual selection. `None` = no selection in progress.
    selection_anchor: Option<Cursor>,
    /// In-editor clipboard: notes normalised to the selection's top-left (0, 0).
    clipboard: Vec<Note>,
}

impl Composer {
    /// A fresh composer: empty timeline, default 120 BPM 4/4 grid, C major,
    /// cursor parked at middle C and the song start.
    pub fn new() -> Self {
        Self::from_timeline(Timeline::new(), Grid::default_120())
    }

    /// A composer over an existing timeline and grid. The cursor starts at
    /// middle C and the song start, same as [`Composer::new`].
    pub fn from_timeline(timeline: Timeline, grid: Grid) -> Self {
        Self {
            history: History::new(timeline, HISTORY_CAPACITY),
            grid,
            cursor: Cursor {
                pitch: DEFAULT_CURSOR_PITCH,
                step: 0,
            },
            grabbed: None,
            key: Key {
                root_pc: 0,
                scale: Scale::Major,
            },
            chord: None,
            last_committed: Vec::new(),
            input_mode: InputMode::DirectEdit,
            record_playhead_us: 0,
            live_pending: Vec::new(),
            playing: false,
            transport_us: 0,
            audition_spans: Vec::new(),
            audition_on_fired: HashSet::new(),
            audition_off_fired: HashSet::new(),
            loop_enabled: false,
            loop_start_us: 0,
            loop_end_us: 0,
            metronome_enabled: false,
            last_click_beat: None,
            click_pending_off: None,
            click_count: 0,
            counting_in: false,
            count_in_end_us: 0,
            count_in_bars: 1,
            selection_anchor: None,
            clipboard: Vec::new(),
        }
    }

    /// Set the key used to voice diatonic chords.
    pub fn set_key(&mut self, key: Key) {
        self.key = key;
    }

    /// Set the `LiveRecord` placement playhead. The seam the frontend drives;
    /// tests advance it manually to place recorded notes. Mirrors
    /// [`Action::SetPlayhead`].
    pub fn set_playhead_us(&mut self, us: u64) {
        self.record_playhead_us = us;
    }

    // ── core API ──────────────────────────────────────────────────────────

    /// Apply one [`Action`], mutating state and returning the ordered effects
    /// the frontend must perform.
    ///
    /// Actions that don't apply in the current mode are **no-ops returning an
    /// empty `Vec`** (not errors) — e.g. any non-selector action while the chord
    /// selector is open, or [`Action::ToggleRecordFlavour`] in `DirectEdit`.
    pub fn apply(&mut self, action: Action) -> Result<Vec<Effect>, ActionError> {
        // While the chord selector is open it owns the dispatch, exactly as the
        // TUI keymap routes every key to the selector. Non-selector actions are
        // inert (e.g. `add_note` does nothing mid-chord).
        if self.chord.is_some() && !Self::is_chord_selector_action(&action) {
            return Ok(Vec::new());
        }

        let effects = match action {
            // ── navigation ──────────────────────────────────────────────
            Action::CursorLeft => self.cursor_left(),
            Action::CursorRight => self.cursor_right(),
            Action::CursorUp => self.cursor_up(),
            Action::CursorDown => self.cursor_down(),
            Action::CursorBarLeft => {
                self.cursor.step = self.cursor.step.saturating_sub(self.steps_per_bar());
                Vec::new()
            }
            Action::CursorBarRight => {
                self.cursor.step += self.steps_per_bar();
                Vec::new()
            }
            Action::CursorOctaveDown => {
                self.cursor.pitch = self.cursor.pitch.saturating_sub(12).max(LOWEST_MIDI);
                Vec::new()
            }
            Action::CursorOctaveUp => {
                self.cursor.pitch = (self.cursor.pitch + 12).min(HIGHEST_MIDI);
                Vec::new()
            }
            Action::CursorToStart => {
                self.cursor.step = 0;
                Vec::new()
            }
            Action::CursorToPitchMin => {
                self.cursor.pitch = LOWEST_MIDI;
                Vec::new()
            }
            Action::CursorToPitchMax => {
                self.cursor.pitch = HIGHEST_MIDI;
                Vec::new()
            }
            Action::CursorToEnd => {
                self.cursor.step = self.last_step();
                Vec::new()
            }
            // Absolute jump. `pitch` is clamped to the 88-key range `21..=108`;
            // `step` is taken as-is (the time axis is unbounded).
            Action::SetCursor { pitch, step } => {
                self.cursor.pitch = pitch.clamp(LOWEST_MIDI, HIGHEST_MIDI);
                self.cursor.step = step;
                Vec::new()
            }
            Action::SubdivisionFiner => {
                self.change_subdivision_finer();
                Vec::new()
            }
            Action::SubdivisionCoarser => {
                self.change_subdivision_coarser();
                Vec::new()
            }

            // ── edit ────────────────────────────────────────────────────
            Action::AddNote => self.add_note(),
            Action::DeleteNote => self.delete_note(),
            Action::ResizeNote { delta_steps } => self.resize_note(delta_steps),
            Action::AdjustVelocity { delta } => self.adjust_velocity(delta),
            Action::ToggleGrab => self.toggle_grab(),

            // ── chord selector ──────────────────────────────────────────
            Action::EnterChordMode => self.enter_chord_mode(),
            Action::CommitChord => self.commit_chord(),
            Action::CancelChord => self.cancel_chord(),
            Action::ToggleChordKind => self.toggle_chord_kind(),
            Action::SetChordDegree { degree } => self.set_chord_degree(degree),
            Action::CycleChordDegree { delta } => self.cycle_chord_degree(delta),

            // ── input mode ──────────────────────────────────────────────
            Action::ToggleRecordArm => {
                self.toggle_record_arm();
                Vec::new()
            }
            Action::ToggleRecordFlavour => {
                self.toggle_record_flavour();
                Vec::new()
            }

            // ── wait mode ───────────────────────────────────────────────
            // Wait mode is owned by the Play transport (M5-C), not the
            // composer; these are inert here.
            Action::ToggleWaitMode | Action::SetWaitMode { .. } => Vec::new(),

            // ── transport ───────────────────────────────────────────────
            Action::TogglePlayCursor => {
                // Space toggles play only when not grabbing, matching the TUI.
                if self.grabbed.is_none() {
                    self.toggle_play_cursor()
                } else {
                    Vec::new()
                }
            }
            Action::PlayFromStart => self.start_play(0),
            Action::Stop => self.stop_play(),
            Action::Play { from_us } => self.start_play(from_us),
            Action::SetPlayhead { us } => {
                self.record_playhead_us = us;
                Vec::new()
            }

            // ── loop / metronome / count-in ─────────────────────────────
            Action::ToggleLoop => {
                self.toggle_loop();
                Vec::new()
            }
            Action::ToggleMetronome => {
                self.metronome_enabled = !self.metronome_enabled;
                Vec::new()
            }
            Action::StartCountInRecord => self.start_count_in_record(),
            Action::SetLoopBounds { start_us, end_us } => {
                self.loop_start_us = start_us;
                self.loop_end_us = end_us;
                Vec::new()
            }

            // ── selection / clipboard ───────────────────────────────────
            Action::StartSelection => {
                self.selection_anchor = Some(self.cursor);
                Vec::new()
            }
            Action::ClearSelection => {
                self.selection_anchor = None;
                Vec::new()
            }
            Action::YankSelection => {
                self.yank_selection();
                Vec::new()
            }
            Action::PasteClipboard => {
                self.paste_clipboard();
                Vec::new()
            }
            Action::DeleteSelection => {
                self.delete_selection();
                Vec::new()
            }

            // ── history ─────────────────────────────────────────────────
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
        };
        Ok(effects)
    }

    /// Advance the pure playhead by `dt_us` during playback and return the
    /// audition effects for spans whose boundaries were crossed, plus metronome
    /// / count-in clicks and loop-wrap handling. A no-op (empty) when stopped.
    pub fn advance(&mut self, dt_us: u64) -> Vec<Effect> {
        if !self.playing {
            return Vec::new();
        }
        self.transport_us += dt_us;
        self.tick_audition()
    }

    /// Feed a played MIDI note. Routing matches the input mode:
    /// - `DirectEdit`: ignored (cursor-driven).
    /// - `StepRecord`: a note-on places a one-step note at the cursor and steps.
    /// - `LiveRecord`: pairs on→off at the snapped record playhead into a `Note`.
    ///
    /// Returns effects; placement itself does not audition (the frontend echoes
    /// played keys), matching the TUI's `ingest`.
    pub fn ingest(&mut self, ev: NoteEvent) -> Vec<Effect> {
        match self.input_mode {
            InputMode::DirectEdit => {}
            InputMode::StepRecord => self.ingest_step(ev),
            InputMode::LiveRecord => self.ingest_live(ev),
        }
        Vec::new()
    }

    /// Cancel any in-progress chord preview, clear the selection and stop the
    /// transport — what a frontend calls when navigating away. Returns
    /// [`Effect::AllOff`] so the frontend silences anything sounding.
    pub fn leave(&mut self) -> Vec<Effect> {
        if self.chord.take().is_some() {
            self.history.rollback();
        }
        self.selection_anchor = None;
        self.playing = false;
        self.counting_in = false;
        vec![Effect::AllOff]
    }

    /// A serialisable read-only view for `query state` and rendering.
    pub fn snapshot(&self) -> ComposerSnapshot {
        let notes = self
            .timeline()
            .notes()
            .map(|(id, n)| NoteView {
                id: id.value(),
                pitch: n.pitch.value(),
                start_us: n.start_us,
                dur_us: n.dur_us,
                velocity: n.velocity.value(),
            })
            .collect();
        let selection = self
            .selection_bounds()
            .map(|(pitch_lo, pitch_hi, us_lo, us_hi)| SelectionView {
                pitch_lo,
                pitch_hi,
                us_lo,
                us_hi,
            });
        let chord_preview = self
            .chord
            .as_ref()
            .map(|c| c.pitches.iter().map(|p| p.value()).collect());
        ComposerSnapshot {
            notes,
            cursor: self.cursor,
            bpm: self.grid.bpm as f64,
            time_sig: self.grid.time_sig,
            subdivision: self.grid.subdivision,
            input_mode: self.input_mode,
            playing: self.playing,
            playhead_us: self.playhead_us(),
            looping: self.loop_enabled,
            loop_start_us: self.loop_start_us,
            loop_end_us: self.loop_end_us,
            metronome: self.metronome_enabled,
            selection,
            chord_preview,
            clipboard_len: self.clipboard.len(),
        }
    }

    // ── read accessors (mirror EditScreen) ────────────────────────────────

    /// The current (live-editing) timeline.
    pub fn timeline(&self) -> &Timeline {
        self.history.current()
    }

    /// The current grid (BPM / time signature / subdivision).
    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// The current cursor position.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The current subdivision.
    pub fn current_subdivision(&self) -> Subdivision {
        self.grid.subdivision
    }

    /// The current input mode (direct-edit vs step / live record).
    pub fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    /// Whether the transport is currently playing.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Current playhead position in song µs. Returns the cursor position when
    /// stopped (the playhead visually sits at the cursor).
    pub fn playhead_us(&self) -> u64 {
        if self.playing {
            self.transport_us
        } else {
            self.cursor_us()
        }
    }

    /// Total number of notes in the timeline.
    pub fn note_count(&self) -> usize {
        self.timeline().len()
    }

    /// The id of the note whose span covers the cursor's `(pitch, step)`, if any.
    pub fn note_under_cursor(&self) -> Option<NoteId> {
        self.timeline().find_at(self.cursor.pitch, self.cursor_us())
    }

    /// Look up note data by id.
    pub fn get_note(&self, id: NoteId) -> Option<Note> {
        self.timeline().get(id).copied()
    }

    /// The pitches of the chord currently being previewed, or `None`.
    pub fn previewed_chord(&self) -> Option<Vec<MidiNote>> {
        self.chord.as_ref().map(|c| c.pitches.clone())
    }

    /// The pitches of the most recently committed chord (empty before the first).
    pub fn last_committed_pitches(&self) -> &[MidiNote] {
        &self.last_committed
    }

    /// Whether the chord selector is currently active.
    pub fn in_chord_mode(&self) -> bool {
        self.chord.is_some()
    }

    /// Whether a visual selection is in progress.
    pub fn in_visual_mode(&self) -> bool {
        self.selection_anchor.is_some()
    }

    /// Ids of notes whose start falls inside the active selection rectangle.
    pub fn selection_ids(&self) -> Vec<NoteId> {
        let Some((pitch_lo, pitch_hi, us_lo, us_hi)) = self.selection_bounds() else {
            return Vec::new();
        };
        self.timeline()
            .notes_in_region(pitch_lo, pitch_hi, us_lo, us_hi)
    }

    /// Number of notes currently held in the clipboard.
    pub fn clipboard_len(&self) -> usize {
        self.clipboard.len()
    }

    /// Whether loop mode is currently active.
    pub fn is_looping(&self) -> bool {
        self.loop_enabled
    }

    /// The current loop region as `(start_us, end_us)`.
    pub fn loop_bounds(&self) -> (u64, u64) {
        (self.loop_start_us, self.loop_end_us)
    }

    /// Whether the metronome click is armed.
    pub fn is_metronome_on(&self) -> bool {
        self.metronome_enabled
    }

    /// Whether a count-in phase is currently in progress.
    pub fn is_counting_in(&self) -> bool {
        self.counting_in
    }

    /// Number of metronome clicks fired since playback last started.
    pub fn metronome_click_count(&self) -> usize {
        self.click_count
    }

    // ── navigation handlers ───────────────────────────────────────────────

    /// Step left. In grab mode this moves the grabbed note's start (cursor
    /// tracks along) and auditions it; otherwise just moves the cursor.
    fn cursor_left(&mut self) -> Vec<Effect> {
        if let Some(id) = self.grabbed {
            self.history.checkpoint();
            let new_step = self.cursor.step.saturating_sub(1);
            self.history
                .current_mut()
                .set_start(id, self.grid.us_of_step(new_step));
            self.cursor.step = new_step;
            self.audition_note(id)
        } else {
            self.cursor.step = self.cursor.step.saturating_sub(1);
            Vec::new()
        }
    }

    /// Step right (cursor or grabbed note).
    fn cursor_right(&mut self) -> Vec<Effect> {
        if let Some(id) = self.grabbed {
            self.history.checkpoint();
            let new_step = self.cursor.step + 1;
            self.history
                .current_mut()
                .set_start(id, self.grid.us_of_step(new_step));
            self.cursor.step = new_step;
            self.audition_note(id)
        } else {
            self.cursor.step += 1;
            Vec::new()
        }
    }

    /// Semitone down (cursor, or transpose the grabbed note; cursor tracks).
    fn cursor_down(&mut self) -> Vec<Effect> {
        if let Some(id) = self.grabbed {
            self.history.checkpoint();
            if self.history.current_mut().transpose(id, -1) {
                self.cursor.pitch = self.cursor.pitch.saturating_sub(1).max(LOWEST_MIDI);
            }
            self.audition_note(id)
        } else {
            self.cursor.pitch = self.cursor.pitch.saturating_sub(1).max(LOWEST_MIDI);
            Vec::new()
        }
    }

    /// Semitone up (cursor, or transpose the grabbed note; cursor tracks).
    fn cursor_up(&mut self) -> Vec<Effect> {
        if let Some(id) = self.grabbed {
            self.history.checkpoint();
            if self.history.current_mut().transpose(id, 1) {
                self.cursor.pitch = (self.cursor.pitch + 1).min(HIGHEST_MIDI);
            }
            self.audition_note(id)
        } else {
            self.cursor.pitch = (self.cursor.pitch + 1).min(HIGHEST_MIDI);
            Vec::new()
        }
    }

    // ── edit handlers ─────────────────────────────────────────────────────

    /// Add a note at the cursor (pitch=cursor, dur=1 step, vel=80). Replaces any
    /// note already in the cell, then auditions the new note.
    fn add_note(&mut self) -> Vec<Effect> {
        self.history.checkpoint();
        if let Some(id) = self.note_under_cursor() {
            self.history.current_mut().remove(id);
            if self.grabbed == Some(id) {
                self.grabbed = None;
            }
        }
        let pitch = MidiNote::new(self.cursor.pitch).expect("cursor pitch is always valid");
        let velocity = Velocity::new(DEFAULT_NOTE_VEL).expect("80 is always valid");
        let start_us = self.cursor_us();
        let dur_us = self.grid.step_us();
        self.history.current_mut().insert(Note {
            pitch,
            start_us,
            dur_us,
            velocity,
        });
        vec![Effect::AuditionNote {
            pitch: pitch.value(),
            velocity: velocity.value(),
        }]
    }

    /// Delete the note under the cursor. No-op on an empty cell.
    fn delete_note(&mut self) -> Vec<Effect> {
        let Some(id) = self.note_under_cursor() else {
            return Vec::new();
        };
        self.history.checkpoint();
        self.history.current_mut().remove(id);
        if self.grabbed == Some(id) {
            self.grabbed = None;
        }
        Vec::new()
    }

    /// Resize the note under the cursor by `delta_steps` grid steps (positive
    /// lengthens; negative shortens, clamped at one step). No-op on an empty cell.
    fn resize_note(&mut self, delta_steps: i64) -> Vec<Effect> {
        let Some(id) = self.note_under_cursor() else {
            return Vec::new();
        };
        let Some(note) = self.timeline().get(id).copied() else {
            return Vec::new();
        };
        self.history.checkpoint();
        let step = self.grid.step_us();
        let new_dur = if delta_steps >= 0 {
            note.dur_us.saturating_add(step * delta_steps as u64)
        } else {
            note.dur_us
                .saturating_sub(step * (-delta_steps) as u64)
                .max(step)
        };
        self.history.current_mut().resize(id, new_dur);
        Vec::new()
    }

    /// Adjust velocity on the note under the cursor by `delta`, clamped to
    /// `1..=127`. Re-inserts the note; the grab follows the new id if held.
    fn adjust_velocity(&mut self, delta: i16) -> Vec<Effect> {
        let Some(id) = self.note_under_cursor() else {
            return Vec::new();
        };
        let Some(note) = self.timeline().get(id).copied() else {
            return Vec::new();
        };
        self.history.checkpoint();
        let new_vel = (note.velocity.value() as i16 + delta).clamp(1, 127) as u8;
        let new_note = Note {
            velocity: Velocity::new(new_vel).expect("clamped to 1..=127"),
            ..note
        };
        self.history.current_mut().remove(id);
        let new_id = self.history.current_mut().insert(new_note);
        if self.grabbed == Some(id) {
            self.grabbed = Some(new_id);
        }
        Vec::new()
    }

    /// Toggle grab mode. Grabbing a note auditions it; `m` again drops it. A
    /// grab on an empty cell is a no-op.
    fn toggle_grab(&mut self) -> Vec<Effect> {
        if self.grabbed.is_some() {
            self.grabbed = None;
            Vec::new()
        } else if let Some(id) = self.note_under_cursor() {
            self.grabbed = Some(id);
            self.audition_note(id)
        } else {
            Vec::new()
        }
    }

    // ── chord-selector handlers ───────────────────────────────────────────

    /// Whether `action` is one the chord selector consumes while open.
    fn is_chord_selector_action(action: &Action) -> bool {
        matches!(
            action,
            Action::CommitChord
                | Action::CancelChord
                | Action::ToggleChordKind
                | Action::SetChordDegree { .. }
                | Action::CycleChordDegree { .. }
        )
    }

    /// Enter chord mode, rooting the initial preview chord at the cursor pitch.
    /// If the cursor pitch class is a diatonic degree in the current key that
    /// degree is used; otherwise falls back to degree 1. A no-op if already in
    /// chord mode. Checkpoints first so a commit lands as one undo step and a
    /// cancel can `rollback` cleanly.
    fn enter_chord_mode(&mut self) -> Vec<Effect> {
        if self.chord.is_some() {
            return Vec::new();
        }
        self.history.checkpoint();
        let cursor_pc = self.cursor.pitch % 12;
        let degree = self.key.degree_for_pc(cursor_pc).unwrap_or(1);
        self.chord = Some(ChordMode {
            degree,
            kind: ChordKind::Triad,
            preview_ids: Vec::new(),
            pitches: Vec::new(),
        });
        let mut effects = Vec::new();
        self.refresh_preview(&mut effects);
        effects
    }

    /// Set the chord degree directly (1..=7) and re-preview.
    fn set_chord_degree(&mut self, degree: u8) -> Vec<Effect> {
        if let Some(chord) = self.chord.as_mut() {
            chord.degree = degree;
        }
        let mut effects = Vec::new();
        self.refresh_preview(&mut effects);
        effects
    }

    /// Cycle the degree by `delta`, wrapping within `1..=7`, and re-preview.
    fn cycle_chord_degree(&mut self, delta: i8) -> Vec<Effect> {
        if let Some(chord) = self.chord.as_mut() {
            let zero_based = (chord.degree as i8 - 1 + delta).rem_euclid(7);
            chord.degree = zero_based as u8 + 1;
        }
        let mut effects = Vec::new();
        self.refresh_preview(&mut effects);
        effects
    }

    /// Toggle the chord quality (Triad ↔ Seventh) and re-preview.
    fn toggle_chord_kind(&mut self) -> Vec<Effect> {
        if let Some(chord) = self.chord.as_mut() {
            chord.kind = match chord.kind {
                ChordKind::Triad => ChordKind::Seventh,
                ChordKind::Seventh => ChordKind::Triad,
            };
        }
        let mut effects = Vec::new();
        self.refresh_preview(&mut effects);
        effects
    }

    /// Replace the preview with the chord for the current degree/quality, voiced
    /// from the cursor pitch. Removes the previous preview notes first (cycling
    /// never accumulates), then pushes the chord audition effect. A no-op when
    /// not in chord mode.
    fn refresh_preview(&mut self, effects: &mut Vec<Effect>) {
        let Some(chord) = self.chord.as_ref() else {
            return;
        };
        let degree = chord.degree;
        let kind = chord.kind;
        let old_ids = chord.preview_ids.clone();

        for id in old_ids {
            self.history.current_mut().remove(id);
        }

        let root = MidiNote::new(self.cursor.pitch).expect("cursor pitch is always valid");
        let pitches = self.key.diatonic_chord(degree, kind, root);
        let start = self.cursor_us();
        let dur = self.grid.step_us();
        let velocity = Velocity::new(DEFAULT_NOTE_VEL).expect("80 is always valid");

        let ids: Vec<NoteId> = pitches
            .iter()
            .map(|&pitch| {
                self.history.current_mut().insert(Note {
                    pitch,
                    start_us: start,
                    dur_us: dur,
                    velocity,
                })
            })
            .collect();

        effects.push(Effect::AuditionChord {
            pitches: pitches.iter().map(|p| p.value()).collect(),
        });

        if let Some(chord) = self.chord.as_mut() {
            chord.preview_ids = ids;
            chord.pitches = pitches;
        }
    }

    /// Commit the previewed chord: its notes stay permanently and chord mode
    /// ends. A no-op (empty) if not in chord mode.
    fn commit_chord(&mut self) -> Vec<Effect> {
        if let Some(chord) = self.chord.take() {
            self.last_committed = chord.pitches;
            vec![Effect::AllOff]
        } else {
            Vec::new()
        }
    }

    /// Cancel the previewed chord: preview mutations are discarded via
    /// `History::rollback` and chord mode ends. A no-op if not in chord mode.
    fn cancel_chord(&mut self) -> Vec<Effect> {
        if self.chord.take().is_some() {
            self.history.rollback();
            vec![Effect::AllOff]
        } else {
            Vec::new()
        }
    }

    // ── input mode ────────────────────────────────────────────────────────

    /// Toggle the record arm: direct-edit ↔ step-record. Disarming from either
    /// record flavour returns to direct-edit.
    fn toggle_record_arm(&mut self) {
        self.input_mode = match self.input_mode {
            InputMode::DirectEdit => InputMode::StepRecord,
            InputMode::StepRecord | InputMode::LiveRecord => InputMode::DirectEdit,
        };
    }

    /// Flip the record flavour step ↔ live. A no-op while disarmed (direct-edit).
    fn toggle_record_flavour(&mut self) {
        self.input_mode = match self.input_mode {
            InputMode::StepRecord => InputMode::LiveRecord,
            InputMode::LiveRecord => InputMode::StepRecord,
            InputMode::DirectEdit => InputMode::DirectEdit,
        };
    }

    /// Step-record a single event: only note-ons place a note (fixed one-step
    /// duration), replacing any note already in the cursor cell, then step on.
    fn ingest_step(&mut self, ev: NoteEvent) {
        let NoteEventKind::On { velocity } = ev.kind else {
            return;
        };
        if velocity.is_note_off() {
            return; // running-status note-off
        }
        self.history.checkpoint();
        let existing = self.timeline().find_at(ev.note.value(), self.cursor_us());
        if let Some(id) = existing {
            self.history.current_mut().remove(id);
            if self.grabbed == Some(id) {
                self.grabbed = None;
            }
        }
        let start_us = self.cursor_us();
        let dur_us = self.grid.step_us();
        self.history.current_mut().insert(Note {
            pitch: ev.note,
            start_us,
            dur_us,
            velocity,
        });
        self.cursor.step += 1;
    }

    /// Live-record a single event at the snapped record playhead. A note-on
    /// opens a pending span; the matching note-off closes it into a `Note`
    /// (minimum one grid step). Events during the count-in are discarded.
    fn ingest_live(&mut self, ev: NoteEvent) {
        if self.counting_in {
            return;
        }
        let at = self.grid.snap(self.record_playhead_us);
        match ev.kind {
            NoteEventKind::On { velocity } if !velocity.is_note_off() => {
                self.live_pending.push((ev.note, at, velocity));
            }
            // Note-off (or a zero-velocity note-on): close the matching on.
            _ => {
                if let Some(pos) = self.live_pending.iter().position(|(p, _, _)| *p == ev.note) {
                    let (pitch, start, velocity) = self.live_pending.remove(pos);
                    let end = at.max(start + self.grid.step_us());
                    self.history.checkpoint();
                    self.history.current_mut().insert(Note {
                        pitch,
                        start_us: start,
                        dur_us: end - start,
                        velocity,
                    });
                }
            }
        }
    }

    // ── transport handlers ────────────────────────────────────────────────

    /// Toggle play-from-cursor / stop.
    fn toggle_play_cursor(&mut self) -> Vec<Effect> {
        if self.playing {
            self.stop_play()
        } else {
            let from = self.cursor_us();
            self.start_play(from)
        }
    }

    /// Begin playback from `from_us`, caching spans and pre-marking notes that
    /// already ended before the start. Resets click / count-in tracking. Returns
    /// [`Effect::AllOff`] so the frontend silences anything already sounding.
    fn start_play(&mut self, from_us: u64) -> Vec<Effect> {
        self.playing = true;
        self.transport_us = from_us;
        self.audition_spans = build_spans(&self.timeline().to_events());
        self.audition_on_fired.clear();
        self.audition_off_fired.clear();
        for (i, span) in self.audition_spans.iter().enumerate() {
            if span.end_us <= from_us {
                self.audition_on_fired.insert(i);
                self.audition_off_fired.insert(i);
            }
        }
        self.last_click_beat = None;
        self.click_pending_off = None;
        self.click_count = 0;
        self.counting_in = false;
        vec![Effect::AllOff]
    }

    /// Stop playback. Cancels any active count-in and returns [`Effect::AllOff`].
    fn stop_play(&mut self) -> Vec<Effect> {
        self.playing = false;
        self.counting_in = false;
        vec![Effect::AllOff]
    }

    /// Arm live record and count in `count_in_bars` bars of clicks (no recording)
    /// before recording begins. Playback starts from the cursor position.
    fn start_count_in_record(&mut self) -> Vec<Effect> {
        let from = self.cursor_us();
        let count_in_dur = self.grid.bar_us() * self.count_in_bars as u64;
        let count_in_end = from + count_in_dur;
        self.input_mode = InputMode::LiveRecord;
        let effects = self.start_play(from); // resets counting_in → false; order matters
        self.counting_in = true;
        self.count_in_end_us = count_in_end;
        effects
    }

    /// Fire audition note_on / note_off effects for spans whose boundaries the
    /// playhead has crossed, plus metronome / count-in clicks and loop wrap.
    fn tick_audition(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if !self.playing {
            return effects;
        }
        let now = self.transport_us;

        // Loop wrap: crossing loop_end restarts from loop_start (next tick fires
        // notes from the new position).
        if self.loop_enabled && self.loop_end_us > self.loop_start_us && now >= self.loop_end_us {
            return self.start_play(self.loop_start_us);
        }

        // Count-in expiry: switch from silent pre-roll to recording.
        if self.counting_in && now >= self.count_in_end_us {
            self.counting_in = false;
        }

        // Metronome / count-in clicks.
        if self.metronome_enabled || self.counting_in {
            self.tick_metronome_click(now, &mut effects);
        }

        let (need_on, need_off) = audition_pending_triggers(
            &self.audition_spans,
            now,
            &self.audition_on_fired,
            &self.audition_off_fired,
        );
        for i in need_on {
            effects.push(Effect::AuditionNote {
                pitch: self.audition_spans[i].note,
                velocity: DEFAULT_NOTE_VEL,
            });
            self.audition_on_fired.insert(i);
        }
        for i in need_off {
            effects.push(Effect::AuditionNote {
                pitch: self.audition_spans[i].note,
                velocity: 0, // velocity 0 == note-off (MIDI convention)
            });
            self.audition_off_fired.insert(i);
        }
        effects
    }

    /// Emit a metronome click when the beat index advances, releasing the prior
    /// click first. Pushes `AuditionNote` effects (velocity 0 = the click off).
    fn tick_metronome_click(&mut self, now: u64, effects: &mut Vec<Effect>) {
        let beat_us = self.grid.quarter_us();
        let current_beat = now / beat_us;

        // Release pending click note_off if its time has come.
        if let Some(off_at) = self.click_pending_off {
            if now >= off_at {
                effects.push(Effect::AuditionNote {
                    pitch: CLICK_MIDI_VALUE,
                    velocity: 0,
                });
                self.click_pending_off = None;
            }
        }

        // Fire note_on when a new beat starts.
        if self.last_click_beat != Some(current_beat) {
            let beats_per_bar = self.grid.time_sig.beats_per_bar as u64;
            let is_accent = current_beat.is_multiple_of(beats_per_bar);
            let velocity = if is_accent {
                CLICK_VEL_ACCENT
            } else {
                CLICK_VEL_NORMAL
            };
            effects.push(Effect::AuditionNote {
                pitch: CLICK_MIDI_VALUE,
                velocity,
            });
            self.click_pending_off = Some(now + CLICK_DUR_US);
            self.last_click_beat = Some(current_beat);
            self.click_count += 1;
        }
    }

    /// Toggle loop on/off. Turning on auto-sets bounds to the bar under the
    /// cursor when no valid bounds have been set yet.
    fn toggle_loop(&mut self) {
        if self.loop_enabled {
            self.loop_enabled = false;
        } else {
            if self.loop_end_us <= self.loop_start_us {
                let (start, end) = self.current_bar_bounds();
                self.loop_start_us = start;
                self.loop_end_us = end;
            }
            self.loop_enabled = true;
        }
    }

    // ── history handlers ──────────────────────────────────────────────────

    /// Stop the transport, then undo the most recent edit. Always returns
    /// [`Effect::AllOff`] (the stop silences sound) — matching the TUI's `u`.
    fn undo(&mut self) -> Vec<Effect> {
        let effects = self.stop_play();
        if self.history.undo() {
            self.grabbed = None;
            self.live_pending.clear();
            self.selection_anchor = None;
        }
        effects
    }

    /// Stop the transport, then redo the most recently undone edit.
    fn redo(&mut self) -> Vec<Effect> {
        let effects = self.stop_play();
        if self.history.redo() {
            self.grabbed = None;
            self.live_pending.clear();
            self.selection_anchor = None;
        }
        effects
    }

    // ── selection / clipboard handlers ────────────────────────────────────

    /// Bounding rectangle of the active selection, or `None`. Returns
    /// `(pitch_lo, pitch_hi, us_lo, us_hi)`.
    fn selection_bounds(&self) -> Option<(u8, u8, u64, u64)> {
        let anchor = self.selection_anchor?;
        let pitch_lo = anchor.pitch.min(self.cursor.pitch);
        let pitch_hi = anchor.pitch.max(self.cursor.pitch);
        let step_lo = anchor.step.min(self.cursor.step);
        let step_hi = anchor.step.max(self.cursor.step);
        let us_lo = self.grid.us_of_step(step_lo);
        // Include the whole last step so a single-step selection is non-empty.
        let us_hi = self.grid.us_of_step(step_hi) + self.grid.step_us();
        Some((pitch_lo, pitch_hi, us_lo, us_hi))
    }

    /// Copy the selected notes into the clipboard, normalised to the selection's
    /// top-left. Clears the selection on success; no-op when empty/inactive.
    fn yank_selection(&mut self) {
        let Some((pitch_lo, _, us_lo, _)) = self.selection_bounds() else {
            return;
        };
        let ids = self.selection_ids();
        if ids.is_empty() {
            return;
        }
        let notes: Vec<Note> = ids
            .iter()
            .filter_map(|&id| self.timeline().get(id).copied())
            .collect();
        self.clipboard = notes
            .into_iter()
            .map(|n| Note {
                pitch: MidiNote::new(n.pitch.value().saturating_sub(pitch_lo))
                    .expect("relative pitch always 0..=127"),
                start_us: n.start_us.saturating_sub(us_lo),
                ..n
            })
            .collect();
        self.selection_anchor = None;
    }

    /// Paste the clipboard at the cursor (top-left lands at the cursor). No-op
    /// when the clipboard is empty.
    fn paste_clipboard(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        self.history.checkpoint();
        let d_pitch = self.cursor.pitch as i8;
        let d_us = self.cursor_us();
        let clipboard = self.clipboard.clone();
        self.history
            .current_mut()
            .insert_shifted(&clipboard, d_pitch, d_us);
    }

    /// Delete all notes in the current selection. Clears the selection. No-op
    /// when inactive or empty.
    fn delete_selection(&mut self) {
        let ids = self.selection_ids();
        if ids.is_empty() {
            self.selection_anchor = None;
            return;
        }
        self.history.checkpoint();
        for id in ids {
            self.history.current_mut().remove(id);
            if self.grabbed == Some(id) {
                self.grabbed = None;
            }
        }
        self.selection_anchor = None;
    }

    // ── small helpers ─────────────────────────────────────────────────────

    /// Audition the note `id`: a single [`Effect::AuditionNote`], or empty if
    /// the id is unknown.
    fn audition_note(&self, id: NoteId) -> Vec<Effect> {
        match self.timeline().get(id) {
            Some(note) => vec![Effect::AuditionNote {
                pitch: note.pitch.value(),
                velocity: note.velocity.value(),
            }],
            None => Vec::new(),
        }
    }

    /// Steps per bar = `bar_us / step_us` (at least 1).
    fn steps_per_bar(&self) -> u64 {
        (self.grid.bar_us() / self.grid.step_us()).max(1)
    }

    /// Grid step of the last note's end (0 for an empty timeline) — the `$` jump.
    fn last_step(&self) -> u64 {
        let end_us = self
            .timeline()
            .notes()
            .map(|(_, n)| n.start_us + n.dur_us)
            .max()
            .unwrap_or(0);
        self.grid.step_index(end_us)
    }

    /// Change to a finer subdivision, re-snapping the cursor to the new grid.
    fn change_subdivision_finer(&mut self) {
        let cursor_us = self.cursor_us();
        self.grid.subdivision = self.grid.subdivision.finer();
        self.resnap_cursor_from_us(cursor_us);
    }

    /// Change to a coarser subdivision, re-snapping the cursor to the new grid.
    fn change_subdivision_coarser(&mut self) {
        let cursor_us = self.cursor_us();
        self.grid.subdivision = self.grid.subdivision.coarser();
        self.resnap_cursor_from_us(cursor_us);
    }

    /// Re-snap the cursor to the nearest grid line given a µs position from the
    /// previous grid.
    fn resnap_cursor_from_us(&mut self, cursor_us: u64) {
        let snapped_us = self.grid.snap(cursor_us);
        self.cursor.step = self.grid.step_index(snapped_us);
    }

    /// Bar bounds `(start_us, end_us)` for the bar containing the cursor.
    fn current_bar_bounds(&self) -> (u64, u64) {
        let bar_us = self.grid.bar_us();
        let cursor_us = self.cursor_us();
        let bar_start = cursor_us / bar_us * bar_us;
        (bar_start, bar_start + bar_us)
    }

    /// Microsecond position of the cursor on the time axis.
    fn cursor_us(&self) -> u64 {
        self.grid.us_of_step(self.cursor.step)
    }
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

/// One note in a [`ComposerSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteView {
    /// The note's stable id (raw [`NoteId`] value).
    pub id: u32,
    pub pitch: u8,
    pub start_us: u64,
    pub dur_us: u64,
    pub velocity: u8,
}

/// The active selection rectangle in a [`ComposerSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionView {
    pub pitch_lo: u8,
    pub pitch_hi: u8,
    pub us_lo: u64,
    pub us_hi: u64,
}

/// A serialisable, read-only snapshot of a [`Composer`] for `query state` and
/// rendering. Everything a frontend needs to draw, without exposing internals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposerSnapshot {
    pub notes: Vec<NoteView>,
    pub cursor: Cursor,
    pub bpm: f64,
    pub time_sig: TimeSig,
    pub subdivision: Subdivision,
    pub input_mode: InputMode,
    pub playing: bool,
    pub playhead_us: u64,
    pub looping: bool,
    pub loop_start_us: u64,
    pub loop_end_us: u64,
    pub metronome: bool,
    pub selection: Option<SelectionView>,
    pub chord_preview: Option<Vec<u8>>,
    pub clipboard_len: usize,
}

/// Build sustained spans from a time-ordered event stream by pairing each
/// note-on with the next note-off of the same pitch. Mirrors the TUI highway's
/// `build_spans` so playback auditions fire identically. Dangling note-ons are
/// closed at the last timestamp seen.
fn build_spans(events: &[NoteEvent]) -> Vec<NoteSpan> {
    use std::collections::HashMap;
    let mut open: HashMap<u8, u64> = HashMap::new();
    let mut spans = Vec::new();
    let mut last_us = 0u64;

    for ev in events {
        last_us = last_us.max(ev.timestamp_us);
        let pitch = ev.note.value();
        match ev.kind {
            NoteEventKind::On { velocity } if velocity.value() > 0 => {
                if let Some(start) = open.remove(&pitch) {
                    spans.push(NoteSpan {
                        note: pitch,
                        start_us: start,
                        end_us: ev.timestamp_us,
                    });
                }
                open.insert(pitch, ev.timestamp_us);
            }
            _ => {
                if let Some(start) = open.remove(&pitch) {
                    spans.push(NoteSpan {
                        note: pitch,
                        start_us: start,
                        end_us: ev.timestamp_us,
                    });
                }
            }
        }
    }

    for (pitch, start) in open {
        spans.push(NoteSpan {
            note: pitch,
            start_us: start,
            end_us: last_us.max(start + 1),
        });
    }

    spans.sort_by_key(|s| (s.start_us, s.note));
    spans
}

/// Returns `(need_on, need_off)`: span indices whose note_on / note_off should
/// fire at `now_us` but haven't yet.
fn audition_pending_triggers(
    spans: &[NoteSpan],
    now_us: u64,
    on_fired: &HashSet<usize>,
    off_fired: &HashSet<usize>,
) -> (Vec<usize>, Vec<usize>) {
    let mut need_on = Vec::new();
    let mut need_off = Vec::new();
    for (i, span) in spans.iter().enumerate() {
        if now_us >= span.start_us && !on_fired.contains(&i) {
            need_on.push(i);
        }
        if now_us >= span.end_us && !off_fired.contains(&i) {
            need_off.push(i);
        }
    }
    (need_on, need_off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
        }
    }

    fn on_ev(pitch: u8) -> NoteEvent {
        NoteEvent::on(MidiNote::new(pitch).unwrap(), Velocity::new(80).unwrap(), 0)
    }

    fn apply(c: &mut Composer, a: Action) -> Vec<Effect> {
        c.apply(a).expect("apply never errors")
    }

    // ── construction / navigation ─────────────────────────────────────────

    #[test]
    fn fresh_cursor_starts_at_middle_c_step_zero() {
        let c = Composer::new();
        assert_eq!(
            c.cursor(),
            Cursor {
                pitch: DEFAULT_CURSOR_PITCH,
                step: 0
            }
        );
        assert_eq!(c.note_count(), 0);
        assert!(!c.is_playing());
    }

    #[test]
    fn left_at_step_zero_stays_at_zero() {
        let mut c = Composer::new();
        apply(&mut c, Action::CursorLeft);
        assert_eq!(c.cursor().step, 0);
    }

    #[test]
    fn right_then_left_returns_to_start_step() {
        let mut c = Composer::new();
        apply(&mut c, Action::CursorRight);
        assert_eq!(c.cursor().step, 1);
        apply(&mut c, Action::CursorLeft);
        assert_eq!(c.cursor().step, 0);
    }

    #[test]
    fn up_and_down_move_pitch_by_one_and_clamp() {
        let mut c = Composer::new();
        apply(&mut c, Action::CursorUp);
        assert_eq!(c.cursor().pitch, DEFAULT_CURSOR_PITCH + 1);
        apply(&mut c, Action::CursorDown);
        assert_eq!(c.cursor().pitch, DEFAULT_CURSOR_PITCH);

        // Clamp at the top.
        apply(
            &mut c,
            Action::SetCursor {
                pitch: HIGHEST_MIDI,
                step: 0,
            },
        );
        apply(&mut c, Action::CursorUp);
        assert_eq!(c.cursor().pitch, HIGHEST_MIDI);

        // Clamp at the bottom.
        apply(
            &mut c,
            Action::SetCursor {
                pitch: LOWEST_MIDI,
                step: 0,
            },
        );
        apply(&mut c, Action::CursorDown);
        assert_eq!(c.cursor().pitch, LOWEST_MIDI);
    }

    #[test]
    fn octave_and_bar_jumps_clamp() {
        let mut c = Composer::new();
        apply(&mut c, Action::CursorOctaveUp);
        assert_eq!(c.cursor().pitch, DEFAULT_CURSOR_PITCH + 12);
        apply(&mut c, Action::CursorOctaveDown);
        assert_eq!(c.cursor().pitch, DEFAULT_CURSOR_PITCH);

        let spb = c.grid().bar_us() / c.grid().step_us();
        apply(&mut c, Action::CursorBarRight);
        assert_eq!(c.cursor().step, spb);
        apply(&mut c, Action::CursorBarLeft);
        assert_eq!(c.cursor().step, 0);
    }

    #[test]
    fn set_cursor_clamps_pitch_to_88_range() {
        let mut c = Composer::new();
        apply(&mut c, Action::SetCursor { pitch: 0, step: 9 });
        assert_eq!(
            c.cursor(),
            Cursor {
                pitch: LOWEST_MIDI,
                step: 9
            }
        );
        apply(
            &mut c,
            Action::SetCursor {
                pitch: 127,
                step: 2,
            },
        );
        assert_eq!(
            c.cursor(),
            Cursor {
                pitch: HIGHEST_MIDI,
                step: 2
            }
        );
    }

    #[test]
    fn cursor_to_end_jumps_to_last_note_end() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 0, 1_000));
        tl.insert(note(64, 2_000, 1_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        apply(&mut c, Action::CursorToEnd);
        assert_eq!(c.cursor().step, c.grid().step_index(3_000));
        apply(&mut c, Action::CursorToStart);
        assert_eq!(c.cursor().step, 0);
    }

    // ── edit ──────────────────────────────────────────────────────────────

    #[test]
    fn add_note_places_at_cursor_and_returns_audition() {
        let mut c = Composer::new();
        let effects = apply(&mut c, Action::AddNote);
        assert_eq!(c.note_count(), 1);
        assert_eq!(
            effects,
            vec![Effect::AuditionNote {
                pitch: DEFAULT_CURSOR_PITCH,
                velocity: DEFAULT_NOTE_VEL
            }]
        );
        let id = c.note_under_cursor().unwrap();
        let n = c.get_note(id).unwrap();
        assert_eq!(n.start_us, 0);
        assert_eq!(n.dur_us, c.grid().step_us());
    }

    #[test]
    fn add_on_occupied_cell_replaces_note() {
        let mut c = Composer::new();
        // First add, bump velocity, then re-add: a fresh default-velocity note.
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::AdjustVelocity { delta: 40 });
        let id = c.note_under_cursor().unwrap();
        assert_eq!(c.get_note(id).unwrap().velocity.value(), 80 + 40);

        apply(&mut c, Action::AddNote);
        assert_eq!(c.note_count(), 1, "replaced, not stacked");
        let id2 = c.note_under_cursor().unwrap();
        assert_eq!(c.get_note(id2).unwrap().velocity.value(), DEFAULT_NOTE_VEL);
    }

    #[test]
    fn delete_removes_note_and_empty_cell_is_noop() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        assert_eq!(c.note_count(), 1);
        apply(&mut c, Action::DeleteNote);
        assert_eq!(c.note_count(), 0);
        // No-op on empty cell.
        let effects = apply(&mut c, Action::DeleteNote);
        assert_eq!(c.note_count(), 0);
        assert!(effects.is_empty());
    }

    #[test]
    fn resize_lengthens_and_clamps_at_one_step() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        let step = c.grid().step_us();
        let id = c.note_under_cursor().unwrap();
        assert_eq!(c.get_note(id).unwrap().dur_us, step);

        apply(&mut c, Action::ResizeNote { delta_steps: 2 });
        assert_eq!(c.get_note(id).unwrap().dur_us, step * 3);

        // Shorten well past zero clamps at one step.
        apply(&mut c, Action::ResizeNote { delta_steps: -100 });
        assert_eq!(c.get_note(id).unwrap().dur_us, step);
    }

    #[test]
    fn velocity_adjust_clamps_to_1_127() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);

        apply(&mut c, Action::AdjustVelocity { delta: 1000 });
        let id = c.note_under_cursor().unwrap();
        assert_eq!(c.get_note(id).unwrap().velocity.value(), 127);

        apply(&mut c, Action::AdjustVelocity { delta: -1000 });
        let id = c.note_under_cursor().unwrap();
        assert_eq!(c.get_note(id).unwrap().velocity.value(), 1);
    }

    // ── grab ────────────────────────────────────────────────────────────

    #[test]
    fn grab_right_moves_note_start_and_cursor_tracks() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        let id = c.note_under_cursor().unwrap();
        let step = c.grid().step_us();

        apply(&mut c, Action::ToggleGrab);
        let effects = apply(&mut c, Action::CursorRight);
        assert_eq!(c.cursor().step, 1, "cursor tracks the grabbed note");
        assert_eq!(c.get_note(id).unwrap().start_us, step);
        assert_eq!(
            effects,
            vec![Effect::AuditionNote {
                pitch: 60,
                velocity: 80
            }]
        );
    }

    #[test]
    fn grab_up_transposes_note_and_cursor_tracks() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        let id = c.note_under_cursor().unwrap();

        apply(&mut c, Action::ToggleGrab);
        apply(&mut c, Action::CursorUp);
        assert_eq!(c.cursor().pitch, DEFAULT_CURSOR_PITCH + 1);
        assert_eq!(
            c.get_note(id).unwrap().pitch.value(),
            DEFAULT_CURSOR_PITCH + 1
        );

        // A second toggle drops the grab.
        apply(&mut c, Action::ToggleGrab);
        apply(&mut c, Action::CursorRight);
        assert_eq!(
            c.get_note(id).unwrap().start_us,
            0,
            "note unchanged after grab dropped"
        );
    }

    #[test]
    fn grab_on_empty_cell_is_noop() {
        let mut c = Composer::new();
        let effects = apply(&mut c, Action::ToggleGrab);
        assert!(effects.is_empty());
        // h/l just navigate (no checkpoint / move) since nothing is grabbed.
        apply(&mut c, Action::CursorRight);
        assert_eq!(c.cursor().step, 1);
        assert_eq!(c.note_count(), 0);
    }

    // ── chord selector ────────────────────────────────────────────────────

    fn pitch_values(notes: &[MidiNote]) -> Vec<u8> {
        notes.iter().map(|n| n.value()).collect()
    }

    #[test]
    fn enter_chord_previews_tonic_and_returns_audition_chord() {
        let mut c = Composer::new();
        let effects = apply(&mut c, Action::EnterChordMode);
        assert!(c.in_chord_mode());
        assert_eq!(
            pitch_values(&c.previewed_chord().unwrap()),
            vec![60, 64, 67]
        );
        assert_eq!(c.note_count(), 3);
        assert_eq!(
            effects,
            vec![Effect::AuditionChord {
                pitches: vec![60, 64, 67]
            }]
        );
    }

    #[test]
    fn enter_chord_roots_at_cursor_pitch() {
        // Cursor on A4 (MIDI 69) in C major → degree 6 (A minor triad A-C-E).
        let mut c = Composer::new();
        apply(&mut c, Action::SetCursor { pitch: 69, step: 0 });
        apply(&mut c, Action::EnterChordMode);
        assert!(c.in_chord_mode());
        assert_eq!(
            pitch_values(&c.previewed_chord().unwrap()),
            vec![69, 72, 76],
            "A minor triad (A4-C5-E5) rooted at cursor A4"
        );
    }

    #[test]
    fn re_entering_chord_mode_after_cursor_move_changes_root() {
        // Enter on C4, cancel, move to E4, re-enter → E chord (degree 3).
        let mut c = Composer::new();
        apply(&mut c, Action::EnterChordMode);
        apply(&mut c, Action::CancelChord);
        apply(&mut c, Action::SetCursor { pitch: 64, step: 0 });
        apply(&mut c, Action::EnterChordMode);
        // E is degree 3 in C major → E-G-B voiced from E4 (64-67-71)
        assert_eq!(
            pitch_values(&c.previewed_chord().unwrap()),
            vec![64, 67, 71],
            "E minor triad (E4-G4-B4) rooted at cursor E4"
        );
    }

    #[test]
    fn enter_chord_non_scale_pitch_falls_back_to_degree_1() {
        // C# (MIDI 61) is not in C major → falls back to degree 1 (C major triad).
        let mut c = Composer::new();
        apply(&mut c, Action::SetCursor { pitch: 61, step: 0 });
        apply(&mut c, Action::EnterChordMode);
        // Degree 1 voiced from C# upward: finds C above C# = C5 (72), E5, G5
        assert!(c.in_chord_mode());
        let preview = c.previewed_chord().unwrap();
        // Root pitch class of the first note should be 0 (C) since degree 1 = C
        assert_eq!(preview[0].value() % 12, 0, "falls back to degree 1 (C)");
    }

    #[test]
    fn set_degree_and_seventh_toggle_replace_preview() {
        let mut c = Composer::new();
        apply(&mut c, Action::EnterChordMode);
        apply(&mut c, Action::SetChordDegree { degree: 5 });
        assert_eq!(
            pitch_values(&c.previewed_chord().unwrap()),
            vec![67, 71, 74]
        );
        assert_eq!(c.note_count(), 3, "preview replaced, not stacked");

        apply(&mut c, Action::ToggleChordKind);
        assert_eq!(
            pitch_values(&c.previewed_chord().unwrap()),
            vec![67, 71, 74, 77]
        );
        assert_eq!(c.note_count(), 4);
    }

    #[test]
    fn cycle_degree_wraps_and_does_not_accumulate() {
        let mut c = Composer::new();
        apply(&mut c, Action::EnterChordMode); // degree 1
        apply(&mut c, Action::CycleChordDegree { delta: -1 }); // wrap to 7
        assert_eq!(
            pitch_values(&c.previewed_chord().unwrap()),
            vec![71, 74, 77]
        );
        apply(&mut c, Action::CycleChordDegree { delta: 1 }); // back to 1
        assert_eq!(
            pitch_values(&c.previewed_chord().unwrap()),
            vec![60, 64, 67]
        );
        assert_eq!(c.note_count(), 3);
    }

    #[test]
    fn commit_keeps_notes_and_reports_last_committed() {
        let mut c = Composer::new();
        apply(&mut c, Action::EnterChordMode);
        let effects = apply(&mut c, Action::CommitChord);
        assert!(!c.in_chord_mode());
        assert!(c.previewed_chord().is_none());
        assert_eq!(c.note_count(), 3);
        assert_eq!(pitch_values(c.last_committed_pitches()), vec![60, 64, 67]);
        assert_eq!(effects, vec![Effect::AllOff]);
    }

    #[test]
    fn cancel_rolls_back_preview() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote); // one permanent note
        apply(&mut c, Action::EnterChordMode);
        assert_eq!(c.note_count(), 4); // 1 permanent + 3 preview
        let effects = apply(&mut c, Action::CancelChord);
        assert_eq!(c.note_count(), 1, "preview rolled back");
        assert!(!c.in_chord_mode());
        assert!(c.last_committed_pitches().is_empty());
        assert_eq!(effects, vec![Effect::AllOff]);
    }

    #[test]
    fn edit_actions_suspended_during_chord_mode() {
        let mut c = Composer::new();
        apply(&mut c, Action::EnterChordMode);
        let before = c.note_count();
        let effects = apply(&mut c, Action::AddNote); // inert mid-chord
        assert_eq!(c.note_count(), before);
        assert!(effects.is_empty());
        assert!(c.in_chord_mode());
    }

    #[test]
    fn chord_at_top_of_range_drops_out_of_range_tones() {
        let mut c = Composer::new();
        apply(
            &mut c,
            Action::SetCursor {
                pitch: HIGHEST_MIDI,
                step: 0,
            },
        );
        apply(&mut c, Action::EnterChordMode);
        apply(&mut c, Action::SetChordDegree { degree: 7 });
        apply(&mut c, Action::ToggleChordKind);
        let preview = c.previewed_chord().unwrap();
        assert!(preview.len() < 4);
        assert_eq!(c.note_count(), preview.len());
        for n in &preview {
            assert!(n.value() <= 127);
        }
    }

    // ── input modes ───────────────────────────────────────────────────────

    #[test]
    fn record_arm_and_flavour_toggle() {
        let mut c = Composer::new();
        assert_eq!(c.input_mode(), InputMode::DirectEdit);
        // `t` is a no-op while disarmed.
        apply(&mut c, Action::ToggleRecordFlavour);
        assert_eq!(c.input_mode(), InputMode::DirectEdit);

        apply(&mut c, Action::ToggleRecordArm);
        assert_eq!(c.input_mode(), InputMode::StepRecord);
        apply(&mut c, Action::ToggleRecordFlavour);
        assert_eq!(c.input_mode(), InputMode::LiveRecord);
        apply(&mut c, Action::ToggleRecordArm);
        assert_eq!(c.input_mode(), InputMode::DirectEdit);
    }

    #[test]
    fn direct_edit_ignores_played_notes() {
        let mut c = Composer::new();
        c.ingest(on_ev(64));
        assert_eq!(c.note_count(), 0);
    }

    #[test]
    fn step_record_places_played_pitches_and_advances() {
        let mut c = Composer::new();
        apply(&mut c, Action::ToggleRecordArm);
        for &p in &[60u8, 62, 64] {
            c.ingest(on_ev(p));
        }
        assert_eq!(c.note_count(), 3);
        assert_eq!(c.cursor().step, 3);
        for (i, &p) in [60u8, 62, 64].iter().enumerate() {
            let id = c
                .timeline()
                .find_at(p, c.grid().us_of_step(i as u64))
                .expect("note at consecutive step");
            assert_eq!(c.get_note(id).unwrap().pitch.value(), p);
        }
    }

    #[test]
    fn step_record_ignores_note_offs() {
        let mut c = Composer::new();
        apply(&mut c, Action::ToggleRecordArm);
        c.ingest(NoteEvent::off(MidiNote::new(60).unwrap(), 0));
        assert_eq!(c.note_count(), 0);
        assert_eq!(c.cursor().step, 0);
    }

    #[test]
    fn live_record_pairs_on_off_at_record_playhead() {
        let mut c = Composer::new();
        apply(&mut c, Action::ToggleRecordArm);
        apply(&mut c, Action::ToggleRecordFlavour);
        let step = c.grid().step_us();
        let pitch = MidiNote::new(60).unwrap();

        c.set_playhead_us(0);
        c.ingest(NoteEvent::on(pitch, Velocity::new(80).unwrap(), 0));
        assert_eq!(c.note_count(), 0, "nothing until the off pairs it");

        c.set_playhead_us(step * 2);
        c.ingest(NoteEvent::off(pitch, 0));
        assert_eq!(c.note_count(), 1);
        let id = c.timeline().find_at(60, 0).unwrap();
        let n = c.get_note(id).unwrap();
        assert_eq!(n.start_us, 0);
        assert_eq!(n.dur_us, step * 2);
    }

    #[test]
    fn live_record_snaps_and_floors_one_step() {
        let mut c = Composer::new();
        apply(&mut c, Action::ToggleRecordArm);
        apply(&mut c, Action::ToggleRecordFlavour);
        let step = c.grid().step_us();
        let pitch = MidiNote::new(67).unwrap();

        // SetPlayhead action drives the same seam as set_playhead_us.
        apply(&mut c, Action::SetPlayhead { us: step + 10 });
        c.ingest(NoteEvent::on(pitch, Velocity::new(80).unwrap(), 0));
        apply(&mut c, Action::SetPlayhead { us: step + 10 });
        c.ingest(NoteEvent::off(pitch, 0));

        assert_eq!(c.note_count(), 1);
        let id = c.timeline().find_at(67, step).unwrap();
        let n = c.get_note(id).unwrap();
        assert_eq!(n.start_us, step);
        assert_eq!(n.dur_us, step);
    }

    // ── transport ───────────────────────────────────────────────────────

    #[test]
    fn play_from_start_resets_playhead_and_returns_all_off() {
        let mut c = Composer::new();
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::CursorRight);
        let effects = apply(&mut c, Action::PlayFromStart);
        assert!(c.is_playing());
        assert_eq!(c.playhead_us(), 0);
        assert_eq!(effects, vec![Effect::AllOff]);
    }

    #[test]
    fn toggle_play_from_cursor_and_stop() {
        let mut c = Composer::new();
        for _ in 0..4 {
            apply(&mut c, Action::CursorRight);
        }
        let cursor_us = c.grid().us_of_step(4);
        apply(&mut c, Action::TogglePlayCursor);
        assert!(c.is_playing());
        assert_eq!(c.playhead_us(), cursor_us);

        let effects = apply(&mut c, Action::Stop);
        assert!(!c.is_playing());
        assert_eq!(effects, vec![Effect::AllOff]);
    }

    #[test]
    fn play_action_starts_from_given_position() {
        let mut c = Composer::new();
        apply(&mut c, Action::Play { from_us: 750_000 });
        assert!(c.is_playing());
        assert_eq!(c.playhead_us(), 750_000);
    }

    #[test]
    fn toggle_play_inert_while_grabbing() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::ToggleGrab);
        let effects = apply(&mut c, Action::TogglePlayCursor);
        assert!(!c.is_playing(), "space does not toggle play in grab mode");
        assert!(effects.is_empty());
    }

    #[test]
    fn advance_is_noop_when_stopped() {
        let mut c = Composer::new();
        let effects = c.advance(1_000_000);
        assert!(!c.is_playing());
        assert!(effects.is_empty());
    }

    #[test]
    fn advance_fires_note_on_once_then_off_after_duration() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 500_000, 200_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        apply(&mut c, Action::PlayFromStart);

        // Before start: no audition.
        let e = c.advance(499_000);
        assert!(e.is_empty());

        // Cross the start: one note-on at default velocity.
        let e = c.advance(2_000);
        assert_eq!(
            e,
            vec![Effect::AuditionNote {
                pitch: 60,
                velocity: DEFAULT_NOTE_VEL
            }]
        );

        // Still inside the note: nothing new.
        let e = c.advance(10_000);
        assert!(e.is_empty());

        // Cross the end (>= 700_000): a velocity-0 note-off.
        let e = c.advance(200_000);
        assert_eq!(
            e,
            vec![Effect::AuditionNote {
                pitch: 60,
                velocity: 0
            }]
        );
    }

    #[test]
    fn notes_before_play_start_are_skipped() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 0, 100_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        // Play from a position after the note ends.
        apply(&mut c, Action::Play { from_us: 200_000 });
        let e = c.advance(50_000);
        assert!(e.is_empty(), "an already-ended note never re-fires");
    }

    // ── loop / metronome / count-in ───────────────────────────────────────

    #[test]
    fn loop_wraps_playhead_at_loop_end() {
        let mut c = Composer::new();
        let bar_us = c.grid().bar_us();
        apply(
            &mut c,
            Action::SetLoopBounds {
                start_us: 0,
                end_us: bar_us,
            },
        );
        apply(&mut c, Action::ToggleLoop);
        assert!(c.is_looping());
        assert_eq!(c.loop_bounds(), (0, bar_us));

        apply(&mut c, Action::PlayFromStart);
        let effects = c.advance(bar_us + 10_000);
        assert_eq!(effects, vec![Effect::AllOff], "loop wrap restarts playback");
        assert!(c.playhead_us() < bar_us, "wrapped back to loop start");
    }

    #[test]
    fn toggle_loop_defaults_to_current_bar() {
        let mut c = Composer::new();
        let bar_us = c.grid().bar_us();
        assert!(!c.is_looping());
        apply(&mut c, Action::ToggleLoop);
        assert!(c.is_looping());
        assert_eq!(c.loop_bounds(), (0, bar_us));
        apply(&mut c, Action::ToggleLoop);
        assert!(!c.is_looping());
    }

    #[test]
    fn metronome_fires_once_per_beat() {
        let mut c = Composer::new();
        let quarter_us = c.grid().quarter_us();
        apply(&mut c, Action::ToggleMetronome);
        assert!(c.is_metronome_on());
        apply(&mut c, Action::PlayFromStart);
        assert_eq!(c.metronome_click_count(), 0);
        for _ in 0..4 {
            c.advance(quarter_us);
        }
        assert_eq!(c.metronome_click_count(), 4);
    }

    #[test]
    fn count_in_discards_notes_during_preroll() {
        let mut c = Composer::new();
        apply(&mut c, Action::ToggleRecordArm);
        apply(&mut c, Action::ToggleRecordFlavour);
        let bar_us = c.grid().bar_us();
        let pitch = MidiNote::new(60).unwrap();
        let vel = Velocity::new(80).unwrap();

        apply(&mut c, Action::StartCountInRecord);
        assert!(c.is_playing());
        assert!(c.is_counting_in());

        // A note during the count-in is discarded.
        c.ingest(NoteEvent::on(pitch, vel, 0));
        c.ingest(NoteEvent::off(pitch, 0));
        assert_eq!(c.note_count(), 0);

        // Advance past the count-in; it expires.
        c.advance(bar_us + 10_000);
        assert!(!c.is_counting_in());

        // A note after the count-in is captured.
        c.ingest(NoteEvent::on(pitch, vel, 0));
        c.advance(c.grid().step_us());
        c.ingest(NoteEvent::off(pitch, 0));
        assert_eq!(c.note_count(), 1);
    }

    #[test]
    fn start_count_in_arms_live_record() {
        let mut c = Composer::new();
        let effects = apply(&mut c, Action::StartCountInRecord);
        assert!(c.is_playing());
        assert!(c.is_counting_in());
        assert_eq!(c.input_mode(), InputMode::LiveRecord);
        assert_eq!(effects, vec![Effect::AllOff]);
    }

    // ── undo / redo ───────────────────────────────────────────────────────

    #[test]
    fn undo_and_redo_round_trip() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::AddNote);
        assert_eq!(c.note_count(), 2);

        apply(&mut c, Action::Undo);
        assert_eq!(c.note_count(), 1);
        apply(&mut c, Action::Undo);
        assert_eq!(c.note_count(), 0);

        apply(&mut c, Action::Redo);
        assert_eq!(c.note_count(), 1);
        apply(&mut c, Action::Redo);
        assert_eq!(c.note_count(), 2);
        apply(&mut c, Action::Redo); // nothing to redo
        assert_eq!(c.note_count(), 2);
    }

    #[test]
    fn new_edit_after_undo_clears_redo() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::Undo);
        assert_eq!(c.note_count(), 1);

        apply(&mut c, Action::CursorLeft);
        apply(&mut c, Action::DeleteNote);
        assert_eq!(c.note_count(), 0);
        apply(&mut c, Action::Redo);
        assert_eq!(c.note_count(), 0, "redo cleared by the delete");
    }

    #[test]
    fn committed_chord_undoes_as_one_step() {
        let mut c = Composer::new();
        apply(&mut c, Action::EnterChordMode);
        apply(&mut c, Action::SetChordDegree { degree: 1 });
        apply(&mut c, Action::CommitChord);
        assert_eq!(c.note_count(), 3);
        apply(&mut c, Action::Undo);
        assert_eq!(c.note_count(), 0);
        apply(&mut c, Action::Redo);
        assert_eq!(c.note_count(), 3);
    }

    #[test]
    fn cancelled_chord_leaves_no_redo() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::EnterChordMode);
        assert_eq!(c.note_count(), 4);
        apply(&mut c, Action::CancelChord);
        assert_eq!(c.note_count(), 1);
        apply(&mut c, Action::Redo);
        assert_eq!(c.note_count(), 1, "no redo after chord cancel");
    }

    // ── selection / clipboard ─────────────────────────────────────────────

    #[test]
    fn select_yank_paste_copies_notes() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::AddNote);
        // Select both notes: anchor at start, cursor already at the second.
        apply(&mut c, Action::SetCursor { pitch: 60, step: 0 });
        apply(&mut c, Action::StartSelection);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::YankSelection);
        assert_eq!(c.clipboard_len(), 2);
        assert!(!c.in_visual_mode());

        // Paste at a fresh location.
        apply(&mut c, Action::SetCursor { pitch: 72, step: 8 });
        apply(&mut c, Action::PasteClipboard);
        assert_eq!(c.note_count(), 4, "two originals + two pasted");
    }

    #[test]
    fn delete_selection_removes_notes() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::SetCursor { pitch: 60, step: 0 });
        apply(&mut c, Action::StartSelection);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::DeleteSelection);
        assert_eq!(c.note_count(), 0);
        assert!(!c.in_visual_mode());
    }

    #[test]
    fn paste_with_empty_clipboard_is_noop() {
        let mut c = Composer::new();
        apply(&mut c, Action::PasteClipboard);
        assert_eq!(c.note_count(), 0);
    }

    // ── snapshot ──────────────────────────────────────────────────────────

    #[test]
    fn snapshot_reflects_state_and_round_trips_json() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::ToggleMetronome);

        let snap = c.snapshot();
        assert_eq!(snap.notes.len(), 1);
        assert_eq!(snap.notes[0].pitch, 60);
        assert_eq!(snap.cursor, Cursor { pitch: 60, step: 1 });
        assert_eq!(snap.bpm, 120.0);
        assert_eq!(snap.subdivision, Subdivision::Sixteenth);
        assert_eq!(snap.input_mode, InputMode::DirectEdit);
        assert!(!snap.playing);
        assert!(snap.metronome);
        assert!(snap.selection.is_none());
        assert!(snap.chord_preview.is_none());

        let json = serde_json::to_string(&snap).expect("serialises");
        let back: ComposerSnapshot = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back.notes.len(), snap.notes.len());
        assert_eq!(back.cursor, snap.cursor);
    }

    #[test]
    fn snapshot_carries_selection_and_chord_preview() {
        let mut c = Composer::new();
        apply(&mut c, Action::StartSelection);
        apply(&mut c, Action::CursorRight);
        let snap = c.snapshot();
        let sel = snap.selection.expect("selection active");
        assert_eq!(sel.pitch_lo, 60);
        assert_eq!(sel.pitch_hi, 60);
        apply(&mut c, Action::ClearSelection);

        apply(&mut c, Action::EnterChordMode);
        let snap = c.snapshot();
        assert_eq!(snap.chord_preview, Some(vec![60, 64, 67]));
    }

    #[test]
    fn leave_cancels_chord_and_stops() {
        let mut c = Composer::new();
        apply(&mut c, Action::EnterChordMode);
        assert_eq!(c.note_count(), 3);
        apply(&mut c, Action::PlayFromStart);
        let effects = c.leave();
        assert!(!c.in_chord_mode());
        assert_eq!(c.note_count(), 0);
        assert!(!c.is_playing());
        assert_eq!(effects, vec![Effect::AllOff]);
    }
}
