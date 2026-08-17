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

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionError, Effect};
use crate::background::{BackgroundStack, BackgroundView, Easing, Transform};
use crate::chord::{ChordKind, Key, Scale};
use crate::events::{MidiNote, NoteEvent, NoteEventKind, Velocity};
use crate::grid::{Grid, Subdivision, TimeSig};
use crate::hand::{Hand, HandSetting, DEFAULT_SPLIT};
use crate::history::History;
use crate::timeline::{Note, NoteId, Timeline};
use crate::wait::{GateState, WaitGate};

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

    // ── backing alignment ───────────────────────────────────────────────────
    /// File position (µs) in the attached backing track that lines up with song
    /// time 0. Adjusted by [`Action::NudgeBackingOffset`] and consumed by the
    /// frontend's [`crate::backing_position_us`] seek. Pure state: the composer
    /// owns no audio, only the number frontends and `query state` read.
    backing_offset_us: u64,

    // ── background images (M14-D) ───────────────────────────────────────────
    /// The piece's background image layers plus the one edit actions address.
    /// Pure editor state exactly like [`Composer::backing_offset_us`]: the
    /// composer owns the layout and the keyframes, never the image files —
    /// attaching one is I/O and therefore a `control::HostCommand`.
    backgrounds: BackgroundStack,

    // ── playback speed ──────────────────────────────────────────────────────
    /// Transport speed multiplier (1.0 = real time). [`Composer::advance`]
    /// scales the injected `dt_us` by this, so the playhead — and thus every
    /// audition boundary — moves slower/faster while the *chart timing stays
    /// untouched*. A practice/review slow-down (Rocksmith-style). Frontends read
    /// it from the snapshot to match their backing-audio playback speed.
    playback_rate: f64,

    // ── wait mode (note-by-note play-along) ──────────────────────────────────
    /// Whether note-by-note wait mode is armed. When armed, playback freezes on
    /// each note (or chord) until the required pitches are held on the piano —
    /// a precise "pause on note" review/verify mode.
    wait_enabled: bool,
    /// The gate driving wait mode, rebuilt from the chart at each
    /// [`start_play`](Composer::start_play) (filtered to notes at/after the play
    /// start). `None` before the first playback.
    wait: Option<WaitGate>,
    /// Whether the last [`advance`](Composer::advance) left the transport frozen
    /// on an unsatisfied wait step. Cached so [`snapshot`](Composer::snapshot)
    /// and the frontend see the transport as "not advancing" while frozen —
    /// which pauses the highway, the backdrop video, and the backing audio via
    /// the existing paused-transport path.
    wait_frozen: bool,
    /// Live set of MIDI pitches currently held on the piano, fed by
    /// [`ingest`](Composer::ingest) and consumed by the wait gate.
    held: BTreeSet<u8>,

    // ── hand assignment (M14-E) ─────────────────────────────────────────────
    /// The piece's left/right split line: notes below it default to the left
    /// hand, at/above it to the right. An authored, persisted property
    /// (`meta.json`'s `hand_split`); per-note overrides live on the notes
    /// themselves and win over it.
    hand_split: u8,
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
            backing_offset_us: 0,
            backgrounds: BackgroundStack::new(),
            playback_rate: 1.0,
            wait_enabled: false,
            wait: None,
            wait_frozen: false,
            held: BTreeSet::new(),
            hand_split: DEFAULT_SPLIT,
        }
    }

    // ── hand assignment (M14-E) ────────────────────────────────────────────

    /// The piece's left/right split pitch.
    pub fn hand_split(&self) -> u8 {
        self.hand_split
    }

    /// Set the split pitch, e.g. when loading a bundle whose `meta.json`
    /// declared one. Live editing goes through [`Action::SetHandSplit`].
    pub fn set_hand_split(&mut self, pitch: u8) {
        self.hand_split = pitch;
    }

    /// The notes [`Action::SetNoteHand`] / [`Action::CycleNoteHand`] act on:
    /// the selection when one is active, else the note under the cursor, else
    /// nothing (the actions are no-ops, never errors).
    fn hand_target_ids(&self) -> Vec<NoteId> {
        let selected = self.selection_ids();
        if !selected.is_empty() {
            return selected;
        }
        self.note_under_cursor().into_iter().collect()
    }

    /// Pin every target note to `hand` (`None` = follow the split line).
    fn set_note_hand(&mut self, hand: Option<Hand>) -> Vec<Effect> {
        let ids = self.hand_target_ids();
        if ids.is_empty() {
            return Vec::new();
        }
        self.history.checkpoint();
        for id in ids {
            self.history.current_mut().set_hand(id, hand);
        }
        Vec::new()
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

    /// The backing-track alignment offset (`audio_start_us`): the file position
    /// lining up with song time 0. Frontends read this to seek the audio.
    pub fn backing_offset_us(&self) -> u64 {
        self.backing_offset_us
    }

    /// Set the backing-track alignment offset, e.g. when loading a bundle whose
    /// `meta.json` declared one. Live editing goes through
    /// [`Action::NudgeBackingOffset`] instead.
    pub fn set_backing_offset_us(&mut self, us: u64) {
        self.backing_offset_us = us;
    }

    // ── background images (M14-D) ──────────────────────────────────────────

    /// The piece's background image layers and the selected one.
    pub fn backgrounds(&self) -> &BackgroundStack {
        &self.backgrounds
    }

    /// Mutable access to the background layers, for the host tier: attaching or
    /// detaching an image file is I/O, so it arrives as a `HostCommand` rather
    /// than an [`Action`]. Layout and keyframing stay pure and go through
    /// [`apply`](Composer::apply).
    pub fn backgrounds_mut(&mut self) -> &mut BackgroundStack {
        &mut self.backgrounds
    }

    /// Replace the background layers, e.g. when loading a bundle whose
    /// `meta.json` declared some.
    pub fn set_backgrounds(&mut self, backgrounds: BackgroundStack) {
        self.backgrounds = backgrounds;
    }

    /// Every background layer with its transform evaluated at the edit time —
    /// what a frontend renders straight from.
    pub fn background_views(&self) -> Vec<BackgroundView> {
        self.backgrounds.views_at(self.playhead_us())
    }

    /// Write the selected layer's keyframe at the edit time, seeding a new one
    /// from the currently interpolated transform (auto-keyframing).
    ///
    /// `edit` receives the transform to modify. A no-op when the piece has no
    /// background layers, so a frontend may bind the keys unconditionally.
    fn edit_background(&mut self, edit: impl FnOnce(&mut Transform)) -> Vec<Effect> {
        let at_us = self.playhead_us();
        if let Some(layer) = self.backgrounds.selected_mut() {
            let mut transform = layer.transform_at(at_us);
            edit(&mut transform);
            // An existing keyframe keeps its departing curve; a fresh one is
            // linear.
            let easing = layer
                .keyframe_at(at_us)
                .map(|k| k.easing)
                .unwrap_or(Easing::Linear);
            layer.set_keyframe(at_us, transform, easing);
        }
        Vec::new()
    }

    /// The transport speed multiplier (1.0 = real time). Frontends read this to
    /// match backing-audio playback speed to the (possibly slowed) playhead.
    pub fn playback_rate(&self) -> f64 {
        self.playback_rate
    }

    /// Set the transport speed multiplier, clamped to a sane practice range
    /// (0.25×–2×). Set via [`Action::SetPlaybackRate`] during playback.
    pub fn set_playback_rate(&mut self, rate: f64) {
        self.playback_rate = rate.clamp(0.25, 2.0);
    }

    /// Arm or disarm note-by-note wait mode ("pause on note"). When armed and a
    /// [`WaitGate`] exists (i.e. after [`start_play`](Composer::start_play)), the
    /// gate is (dis)armed in step so it takes effect on the next
    /// [`advance`](Composer::advance). Disarming immediately clears the frozen
    /// flag so playback resumes without waiting a tick.
    pub fn set_wait_mode(&mut self, on: bool) {
        self.wait_enabled = on;
        if let Some(gate) = self.wait.as_mut() {
            gate.set_armed(on);
        }
        if !on {
            self.wait_frozen = false;
        }
    }

    /// Whether note-by-note wait mode is armed.
    pub fn is_wait_mode(&self) -> bool {
        self.wait_enabled
    }

    /// Whether the transport is currently frozen on an unsatisfied wait step.
    pub fn is_wait_frozen(&self) -> bool {
        self.wait_frozen
    }

    /// Whether the transport is *actively advancing*: playing and not held by a
    /// wait-mode freeze. Frontends and audio treat a wait freeze exactly like a
    /// pause (highway, video, and backing all hold), so this — not
    /// [`is_playing`](Composer::is_playing) — is the "is time moving?" signal.
    pub fn is_advancing(&self) -> bool {
        self.playing && !self.wait_frozen
    }

    /// The pitches the player must currently strike to un-freeze wait mode, or
    /// `None` when not frozen. Lets a frontend highlight the awaited note(s).
    pub fn awaiting_notes(&self) -> Option<Vec<u8>> {
        if !self.wait_frozen {
            return None;
        }
        self.wait
            .as_ref()
            .and_then(|g| g.awaiting())
            .map(|step| step.notes.clone())
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

            // ── hand assignment (M14-E) ──────────────────────────────────
            Action::SetHandSplit { pitch } => {
                self.hand_split = pitch;
                Vec::new()
            }
            Action::SetNoteHand { hand } => self.set_note_hand(hand.override_value()),
            // Read the target's *current* setting so the cycle is predictable;
            // with a multi-note selection the first note leads and the whole
            // selection lands on the same setting.
            Action::CycleNoteHand => {
                let current = self
                    .hand_target_ids()
                    .first()
                    .and_then(|&id| self.get_note(id))
                    .map(|n| HandSetting::from_override(n.hand))
                    .unwrap_or(HandSetting::Auto);
                self.set_note_hand(current.next().override_value())
            }

            // ── time / structure (ripple insert & cut) ──────────────────
            Action::InsertBar => self.insert_bar(),
            Action::RemoveBar => self.remove_bar(),

            Action::ToggleGrab => self.toggle_grab(),
            Action::InsertRun {
                end_pitch,
                span_steps,
            } => self.insert_run(end_pitch, span_steps),

            // ── tempo ────────────────────────────────────────────────────
            // Tempo lives in the grid (single source of truth, persisted in
            // RecordingMeta.grid). Changing it re-snaps the cursor so its
            // position holds steady across the new step spacing.
            Action::AdjustBpm { delta } => {
                let cursor_us = self.cursor_us();
                self.grid.adjust_bpm(delta);
                self.resnap_cursor_from_us(cursor_us);
                Vec::new()
            }
            Action::SetBpm { bpm } => {
                let cursor_us = self.cursor_us();
                self.grid.set_bpm(bpm);
                self.resnap_cursor_from_us(cursor_us);
                Vec::new()
            }
            Action::SetTimeSig {
                beats_per_bar,
                beat_unit,
            } => {
                // Metre moves bar lines only — the step grid and note times are
                // unchanged — but resnap anyway so the cursor keeps its song
                // time, exactly as SetBpm does.
                let cursor_us = self.cursor_us();
                self.grid.set_time_sig(beats_per_bar, beat_unit);
                self.resnap_cursor_from_us(cursor_us);
                Vec::new()
            }
            Action::SetGridOrigin { us } => {
                let cursor_us = self.cursor_us();
                self.grid.set_origin_us(us);
                self.resnap_cursor_from_us(cursor_us);
                Vec::new()
            }
            Action::NudgeGridOrigin { delta_us } => {
                let cursor_us = self.cursor_us();
                // The origin is a u64 phase that can never go negative, so a
                // nudge past zero clamps there rather than wrapping.
                let origin = self.grid.origin_us.saturating_add_signed(delta_us);
                self.grid.set_origin_us(origin);
                self.resnap_cursor_from_us(cursor_us);
                Vec::new()
            }
            Action::QuantizeRegion {
                start_us,
                end_us,
                step_us,
            } => self.quantize_region(start_us, end_us, step_us),

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
            // Note-by-note "pause on note" gating for the composer/edit
            // transport: freeze playback on each note until it is played.
            Action::ToggleWaitMode => {
                self.set_wait_mode(!self.wait_enabled);
                Vec::new()
            }
            Action::SetWaitMode { on } => {
                self.set_wait_mode(on);
                Vec::new()
            }

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

            // ── backing alignment ───────────────────────────────────────
            // Slide the backing offset, clamped at 0 (it can never be
            // negative). Frontends re-seek the audio to the new mapping.
            Action::NudgeBackingOffset { delta_us } => {
                self.backing_offset_us = (self.backing_offset_us as i64)
                    .saturating_add(delta_us)
                    .max(0) as u64;
                Vec::new()
            }

            // ── background images (M14-D) ───────────────────────────────
            // Layout + keyframing for the selected layer at the edit time.
            // Every arm is a no-op on a piece with no background images, so a
            // frontend can bind the keys unconditionally.
            Action::SelectBackground { index } => {
                self.backgrounds.select(index as usize);
                Vec::new()
            }
            Action::CycleBackground { delta } => {
                self.backgrounds.cycle(delta);
                Vec::new()
            }
            Action::NudgeBackgroundPos {
                dx_permille,
                dy_permille,
            } => self.edit_background(|t| {
                t.x += dx_permille as f32 / 1000.0;
                t.y += dy_permille as f32 / 1000.0;
            }),
            Action::NudgeBackgroundScale { delta_permille } => self.edit_background(|t| {
                t.scale += delta_permille as f32 / 1000.0;
            }),
            Action::NudgeBackgroundRotation { delta_millideg } => self.edit_background(|t| {
                t.rotation_deg += delta_millideg as f32 / 1000.0;
            }),
            Action::SetBackgroundOpacity { permille } => self.edit_background(|t| {
                t.opacity = permille as f32 / 1000.0;
            }),
            // Easing describes an *existing* keyframe's departure, so unlike the
            // nudges this one never creates a keyframe.
            Action::SetBackgroundEasing { easing } => {
                let at_us = self.playhead_us();
                if let Some(layer) = self.backgrounds.selected_mut() {
                    layer.set_easing_at(at_us, easing);
                }
                Vec::new()
            }
            // Pin the interpolated transform as-is: a hold, or the anchor a
            // later nudge animates away from.
            Action::AddBackgroundKeyframe => self.edit_background(|_| {}),
            Action::DeleteBackgroundKeyframe => {
                let at_us = self.playhead_us();
                if let Some(layer) = self.backgrounds.selected_mut() {
                    layer.remove_keyframe_at(at_us);
                }
                Vec::new()
            }

            // ── playback speed ──────────────────────────────────────────
            // Set the transport speed multiplier (permille: 1000 = 1×).
            // `advance` scales dt by it; frontends match backing-audio speed.
            Action::SetPlaybackRate { rate_permille } => {
                self.set_playback_rate(rate_permille as f64 / 1000.0);
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
            Action::SetLoopStart => {
                self.set_loop_start_at_cursor();
                Vec::new()
            }
            Action::SetLoopEnd => {
                self.set_loop_end_at_cursor();
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
        // Wait-mode gate: if the current note/chord is due and not yet held on
        // the piano, freeze the transport here (don't advance, fire no effects).
        // The note-on for this step already fired on the tick that crossed its
        // onset, so it rings under the playhead until the player matches it. The
        // frontend sees a non-advancing transport (via `wait_frozen`) and holds
        // the highway, video, and backing — the "pause on note" behaviour.
        if self.wait_enabled {
            if let Some(gate) = self.wait.as_mut() {
                gate.set_held(self.held.clone());
                if gate.poll(self.transport_us) == GateState::Frozen {
                    self.wait_frozen = true;
                    return Vec::new();
                }
            }
            self.wait_frozen = false;
        }
        // Scale wall-clock dt by the playback-speed multiplier so a slow-down
        // stretches song time without touching the chart. `round` keeps the
        // per-tick truncation unbiased (net tempo error ~0 over many ticks).
        let scaled = if self.playback_rate == 1.0 {
            dt_us
        } else {
            (dt_us as f64 * self.playback_rate).round() as u64
        };
        self.transport_us += scaled;
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
        // Track held keys for wait-mode gating, independent of input mode: a
        // note-on adds the pitch, a note-off (or zero-velocity on) removes it.
        // The next `advance` polls the wait gate against this set to decide
        // whether to un-freeze.
        match ev.kind {
            NoteEventKind::On { velocity } if !velocity.is_note_off() => {
                self.held.insert(ev.note.value());
            }
            _ => {
                self.held.remove(&ev.note.value());
            }
        }
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
        self.wait_frozen = false;
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
                hand: n.hand,
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
            grid_origin_us: self.grid.origin_us,
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
            backing_offset_us: self.backing_offset_us,
            backgrounds: self.background_views(),
            selected_background: self.backgrounds.selected_index(),
            playback_rate: self.playback_rate,
            wait_mode: self.wait_enabled,
            frozen: self.wait_frozen,
            awaiting: self.awaiting_notes(),
            hand_split: self.hand_split,
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

    /// Song-time of the start of the bar the cursor sits in, honouring the grid's
    /// phase [`origin_us`](crate::Grid). Used by the ripple bar edits.
    fn cursor_bar_start_us(&self) -> u64 {
        let bar_us = self.grid.bar_us().max(1);
        let origin = self.grid.origin_us;
        let rel = self.cursor_us().saturating_sub(origin);
        origin + (rel / bar_us) * bar_us
    }

    /// Insert one empty bar at the cursor's bar boundary: every note starting at
    /// or after that boundary slides one bar later, opening a silent bar. Ripple
    /// edit — the tail keeps its internal timing. One undo checkpoint.
    fn insert_bar(&mut self) -> Vec<Effect> {
        let bar_us = self.grid.bar_us();
        if bar_us == 0 {
            return Vec::new();
        }
        let at = self.cursor_bar_start_us();
        self.history.checkpoint();
        self.history.current_mut().shift_from(at, bar_us as i64);
        Vec::new()
    }

    /// Cut the bar the cursor sits in: delete every note that starts within it,
    /// then slide everything after it one bar earlier so no gap is left. Ripple
    /// edit. Clears any grab/selection (their targets may have moved or gone).
    /// One undo checkpoint.
    fn remove_bar(&mut self) -> Vec<Effect> {
        let bar_us = self.grid.bar_us();
        if bar_us == 0 {
            return Vec::new();
        }
        let start = self.cursor_bar_start_us();
        let end = start + bar_us;
        self.history.checkpoint();
        let tl = self.history.current_mut();
        tl.remove_in(start, end);
        tl.shift_from(end, -(bar_us as i64));
        self.grabbed = None;
        self.selection_anchor = None;
        Vec::new()
    }

    /// The id of the note at the cursor's `(pitch, step)`, if any.
    ///
    /// Prefers a note whose span *covers* the cursor's grid line, then falls back
    /// to a note that *starts within* the cursor's one-step-wide cell. The
    /// fallback matters for imported charts: their onsets sit at true fractional
    /// microsecond times while the grid's `step_us` is integer-floored, so a short
    /// note can start a few µs past the grid line and be missed by the exact-point
    /// query — the cursor looks like it is on the note but edits (hand, move,
    /// delete) find nothing. See [`Timeline::find_starting_in`].
    pub fn note_under_cursor(&self) -> Option<NoteId> {
        let us = self.cursor_us();
        self.timeline().find_at(self.cursor.pitch, us).or_else(|| {
            self.timeline()
                .find_starting_in(self.cursor.pitch, us, us + self.grid.step_us())
        })
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
            hand: None,
        });
        vec![Effect::AuditionNote {
            pitch: pitch.value(),
            velocity: velocity.value(),
        }]
    }

    /// Lay a chromatic run from the cursor to `end_pitch` (inclusive), its notes
    /// spread evenly across `span_steps` grid steps. A one-shot glissando/scale:
    /// position the cursor at the run's start cell, then trace to the end pitch.
    /// Each note is one grid step long; the run steps one semitone per note
    /// (direction inferred from `end_pitch` vs the cursor pitch), and any note
    /// already occupying a target cell is replaced. Auditions the final pitch.
    fn insert_run(&mut self, end_pitch: u8, span_steps: u64) -> Vec<Effect> {
        let start_pitch = self.cursor.pitch;
        let end = end_pitch.clamp(LOWEST_MIDI, HIGHEST_MIDI);
        let start_step = self.cursor.step;
        let n = start_pitch.abs_diff(end) as u64 + 1; // one note per semitone
        let up = end >= start_pitch;
        let dur_us = self.grid.step_us();
        let vel = Velocity::new(DEFAULT_NOTE_VEL).expect("80 is always valid");

        self.history.checkpoint();
        for k in 0..n {
            let pitch_val = if up {
                start_pitch + k as u8
            } else {
                start_pitch - k as u8
            };
            let pitch = MidiNote::new(pitch_val).expect("run pitch stays in 21..=108");
            // Spread the k-th tap evenly across the time span (rounded to a step).
            let step = if n > 1 {
                start_step + (k * span_steps + (n - 1) / 2) / (n - 1)
            } else {
                start_step
            };
            let start_us = self.grid.us_of_step(step);
            if let Some(id) = self.timeline().find_at(pitch_val, start_us) {
                self.history.current_mut().remove(id);
                if self.grabbed == Some(id) {
                    self.grabbed = None;
                }
            }
            self.history.current_mut().insert(Note {
                pitch,
                start_us,
                dur_us,
                velocity: vel,
                hand: None,
            });
        }
        vec![Effect::AuditionNote {
            pitch: end,
            velocity: vel.value(),
        }]
    }

    /// Quantise every note whose **onset** lies in `[start_us, end_us)` onto the
    /// grid at resolution `step_us`: snap the onset to the nearest grid line
    /// (phased from the grid origin) and the end likewise, never shrinking a note
    /// below one `step_us`. One undo checkpoint covers the whole region. A no-op
    /// (empty, no checkpoint) when no note starts in the range — so skipping a
    /// bar is free. Pitch and velocity are untouched.
    fn quantize_region(&mut self, start_us: u64, end_us: u64, step_us: u64) -> Vec<Effect> {
        let step = step_us.max(1);
        let ids: Vec<NoteId> = self
            .timeline()
            .notes()
            .filter(|(_, n)| n.start_us >= start_us && n.start_us < end_us)
            .map(|(id, _)| id)
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        self.history.checkpoint();
        for id in ids {
            let Some(note) = self.timeline().get(id).copied() else {
                continue;
            };
            let new_start = self.grid.snap_to_step(note.start_us, step);
            let snapped_end = self.grid.snap_to_step(note.start_us + note.dur_us, step);
            let new_end = snapped_end.max(new_start + step);
            self.history.current_mut().set_start(id, new_start);
            self.history.current_mut().resize(id, new_end - new_start);
        }
        Vec::new()
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
                    hand: None,
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
            hand: None,
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
                        hand: None,
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
        // Rebuild the wait gate from the notes at/after this start position so
        // the first freeze lands on the first note we actually play (not the
        // song's very first note when starting mid-piece / mid-loop).
        let expected: Vec<(MidiNote, u64)> = self
            .audition_spans
            .iter()
            .filter(|s| s.start_us >= from_us)
            .filter_map(|s| MidiNote::new(s.note).map(|n| (n, s.start_us)))
            .collect();
        let mut gate = WaitGate::from_expected(&expected);
        gate.set_armed(self.wait_enabled);
        self.wait = Some(gate);
        self.wait_frozen = false;
        vec![Effect::AllOff]
    }

    /// Stop playback. Cancels any active count-in and returns [`Effect::AllOff`].
    fn stop_play(&mut self) -> Vec<Effect> {
        self.playing = false;
        self.counting_in = false;
        self.wait_frozen = false;
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

    /// Set the loop **start** to the cursor position. The end is pushed out to
    /// keep the region non-empty (at least one grid step) if start would meet or
    /// cross it.
    fn set_loop_start_at_cursor(&mut self) {
        let start = self.cursor_us();
        self.loop_start_us = start;
        if self.loop_end_us <= start {
            self.loop_end_us = start + self.grid.step_us();
        }
    }

    /// Set the loop **end** to just past the step under the cursor (so a single
    /// cell yields a non-empty region). The start is pulled back to keep the
    /// region non-empty if end would meet or cross it.
    fn set_loop_end_at_cursor(&mut self) {
        let step = self.grid.step_us();
        let end = self.cursor_us() + step;
        self.loop_end_us = end;
        if self.loop_start_us >= end {
            self.loop_start_us = end.saturating_sub(step);
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
    /// The note's **raw** hand override (M14-E): `None` = follows the piece's
    /// split line. Frontends derive the *effective* hand from this plus
    /// [`ComposerSnapshot::hand_split`], and mark the overridden ones.
    #[serde(default)]
    pub hand: Option<Hand>,
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
    /// Grid phase origin (µs): the song time bar 1 / beat 1 / step 0 lands on.
    /// `0` for a piece whose grid starts at song time 0. Frontends phase their
    /// bar/beat gridlines by this so the lines match the performance.
    #[serde(default)]
    pub grid_origin_us: u64,
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
    /// Backing-track alignment offset (`audio_start_us`): the file position that
    /// lines up with song time 0. `0` when no backing or no nudge applied.
    pub backing_offset_us: u64,
    /// Background image layers with each transform **already evaluated** at the
    /// playhead, back-to-front. Empty for pieces without any. Frontends render
    /// these verbatim — the interpolation math lives in `core`.
    #[serde(default)]
    pub backgrounds: Vec<BackgroundView>,
    /// Index of the layer background actions address; `None` when the piece has
    /// no background images.
    #[serde(default)]
    pub selected_background: Option<usize>,
    /// Transport speed multiplier (1.0 = real time). Frontends match their
    /// backing-audio playback speed to this so a slow-down stays in sync.
    #[serde(default = "default_playback_rate")]
    pub playback_rate: f64,
    /// Whether note-by-note wait mode ("pause on note") is armed.
    #[serde(default)]
    pub wait_mode: bool,
    /// Whether the transport is currently frozen on an unsatisfied wait step.
    /// `playing` stays `true` while frozen (so the highway anchors on the
    /// playhead, not the cursor); frontends treat `playing && frozen` as a
    /// pause — holding the highway, the backdrop video, and the backing audio.
    #[serde(default)]
    pub frozen: bool,
    /// While frozen, the MIDI pitches the player must strike to advance (so a
    /// frontend can highlight them); `None` when not frozen.
    #[serde(default)]
    pub awaiting: Option<Vec<u8>>,
    /// The piece's left/right hand split pitch (M14-E): notes below it default
    /// to the left hand, at/above it to the right. Combine with
    /// [`NoteView::hand`] for a note's effective hand.
    #[serde(default = "default_hand_split")]
    pub hand_split: u8,
}

/// Serde default for [`ComposerSnapshot::hand_split`] so older snapshots (no
/// field) deserialise at middle C.
fn default_hand_split() -> u8 {
    DEFAULT_SPLIT
}

/// Serde default for [`ComposerSnapshot::playback_rate`] so older snapshots
/// (no field) deserialise as real-time.
fn default_playback_rate() -> f64 {
    1.0
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
    use crate::hand::HandOverride;

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
            hand: None,
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

    /// Regression: an imported note whose onset sits a few µs past the
    /// integer-floored grid line must still be editable. The grid `step_us` is
    /// floored (170 bpm eighth = 176_470, not 176_470.59…), so a short note drifts
    /// between two cursor grid lines; `note_under_cursor` must still resolve it
    /// (via the cell fallback) or hand/move/delete silently no-op.
    #[test]
    fn note_under_cursor_finds_short_note_drifted_off_the_grid_line() {
        let grid = Grid {
            bpm: 170,
            time_sig: TimeSig {
                beats_per_bar: 3,
                beat_unit: 4,
            },
            subdivision: Subdivision::Eighth,
            origin_us: 0,
        };
        let step = 193u64;
        let line = grid.us_of_step(step); // 34_058_710, with step_us floored to 176_470
                                          // A short note starting 96 µs past the grid line and ending before the
                                          // NEXT line — so no grid line lands inside it (the drift failure).
        let start = line + 96;
        let dur = 175_000; // < step_us, ends before us_of_step(step+1)
        let mut tl = Timeline::new();
        let id = tl.insert(note(47, start, dur));
        // The exact-point query misses it at both adjacent grid lines.
        assert_eq!(tl.find_at(47, line), None);
        assert_eq!(tl.find_at(47, grid.us_of_step(step + 1)), None);

        let mut c = Composer::from_timeline(tl, grid);
        apply(&mut c, Action::SetCursor { pitch: 47, step });
        assert_eq!(
            c.note_under_cursor(),
            Some(id),
            "the note starting in the cursor's grid cell must be editable"
        );
        // And a real edit now lands on it.
        apply(&mut c, Action::CycleNoteHand);
        assert_eq!(c.get_note(id).unwrap().hand, Some(Hand::Left));
    }

    #[test]
    fn insert_bar_and_remove_bar_ripple_the_tail() {
        let grid = Grid {
            bpm: 120,
            time_sig: TimeSig {
                beats_per_bar: 4,
                beat_unit: 4,
            },
            subdivision: Subdivision::Quarter,
            origin_us: 0,
        };
        let bar = grid.bar_us(); // 2_000_000
        let build = || {
            let mut tl = Timeline::new();
            tl.insert(note(60, 0, 100_000)); // bar 0
            tl.insert(note(62, bar, 100_000)); // bar 1
            tl.insert(note(64, 2 * bar, 100_000)); // bar 2
            tl
        };
        let step_in_bar1 = grid.step_index(bar);
        let starts = |c: &Composer| -> Vec<(u8, u64)> {
            let mut v: Vec<(u8, u64)> = c
                .timeline()
                .notes()
                .map(|(_, n)| (n.pitch.value(), n.start_us))
                .collect();
            v.sort();
            v
        };

        // remove_bar: the cursor's bar (bar 1) is deleted and the tail slides left.
        let mut c = Composer::from_timeline(build(), grid);
        apply(
            &mut c,
            Action::SetCursor {
                pitch: 60,
                step: step_in_bar1,
            },
        );
        apply(&mut c, Action::RemoveBar);
        assert_eq!(
            starts(&c),
            vec![(60, 0), (64, bar)],
            "bar-1 note gone; bar-2 note slid to bar 1"
        );
        apply(&mut c, Action::Undo);
        assert_eq!(c.note_count(), 3, "remove_bar is one undo step");

        // insert_bar: everything at/after the cursor's bar slides one bar later.
        let mut c = Composer::from_timeline(build(), grid);
        apply(
            &mut c,
            Action::SetCursor {
                pitch: 60,
                step: step_in_bar1,
            },
        );
        apply(&mut c, Action::InsertBar);
        assert_eq!(
            starts(&c),
            vec![(60, 0), (62, 2 * bar), (64, 3 * bar)],
            "bar-0 untouched; bars 1 and 2 pushed one bar later"
        );
    }

    #[test]
    fn insert_run_lays_chromatic_ascending_staircase() {
        let mut c = Composer::new();
        // Cursor at C4 (60), step 0. A run to E4 (64) over 4 steps = 5 notes,
        // one semitone per grid step.
        let effects = apply(
            &mut c,
            Action::InsertRun {
                end_pitch: 64,
                span_steps: 4,
            },
        );
        assert_eq!(c.note_count(), 5);
        assert_eq!(
            effects,
            vec![Effect::AuditionNote {
                pitch: 64,
                velocity: DEFAULT_NOTE_VEL
            }]
        );
        let step = c.grid().step_us();
        for (k, pitch) in (60u8..=64).enumerate() {
            apply(
                &mut c,
                Action::SetCursor {
                    pitch,
                    step: k as u64,
                },
            );
            let id = c
                .note_under_cursor()
                .unwrap_or_else(|| panic!("note at pitch {pitch} step {k}"));
            let n = c.get_note(id).unwrap();
            assert_eq!(n.start_us, k as u64 * step);
            assert_eq!(n.dur_us, step, "each run note is one step long");
        }
    }

    #[test]
    fn insert_run_descends_when_end_below_cursor() {
        let mut c = Composer::new();
        apply(&mut c, Action::SetCursor { pitch: 64, step: 0 });
        apply(
            &mut c,
            Action::InsertRun {
                end_pitch: 60,
                span_steps: 4,
            },
        );
        assert_eq!(c.note_count(), 5);
        // k-th note steps down one semitone and forward one grid step.
        for k in 0u64..=4 {
            let pitch = 64 - k as u8;
            apply(&mut c, Action::SetCursor { pitch, step: k });
            assert!(
                c.note_under_cursor().is_some(),
                "descending note at pitch {pitch} step {k}"
            );
        }
    }

    #[test]
    fn insert_run_spreads_notes_evenly_across_span() {
        let mut c = Composer::new();
        // 3 notes (60,61,62) spread across 10 steps -> steps 0, 5, 10 (rounded).
        apply(
            &mut c,
            Action::InsertRun {
                end_pitch: 62,
                span_steps: 10,
            },
        );
        assert_eq!(c.note_count(), 3);
        for (pitch, step) in [(60u8, 0u64), (61, 5), (62, 10)] {
            apply(&mut c, Action::SetCursor { pitch, step });
            assert!(
                c.note_under_cursor().is_some(),
                "note at pitch {pitch} step {step}"
            );
        }
    }

    #[test]
    fn insert_run_same_pitch_is_single_note_and_run_undoes_atomically() {
        let mut c = Composer::new();
        // Pre-existing note in a target cell is replaced, not stacked.
        apply(&mut c, Action::SetCursor { pitch: 61, step: 1 });
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::SetCursor { pitch: 60, step: 0 });
        apply(
            &mut c,
            Action::InsertRun {
                end_pitch: 62,
                span_steps: 2,
            },
        );
        assert_eq!(c.note_count(), 3, "replaced the 61 cell, did not stack");
        // A run is one checkpoint: a single undo clears the whole staircase,
        // leaving only the pre-existing note.
        apply(&mut c, Action::Undo);
        assert_eq!(c.note_count(), 1);

        // end_pitch == cursor pitch collapses to a single note.
        let mut c2 = Composer::new();
        apply(
            &mut c2,
            Action::InsertRun {
                end_pitch: DEFAULT_CURSOR_PITCH,
                span_steps: 8,
            },
        );
        assert_eq!(c2.note_count(), 1);
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

    // ── tempo ─────────────────────────────────────────────────────────────

    #[test]
    fn set_bpm_changes_grid_and_snapshot_and_clamps() {
        let mut c = Composer::new();
        assert_eq!(c.grid().bpm, 120);

        apply(&mut c, Action::SetBpm { bpm: 90 });
        assert_eq!(c.grid().bpm, 90);
        assert_eq!(c.snapshot().bpm, 90.0);

        // Clamp below the floor and above the ceiling.
        apply(&mut c, Action::SetBpm { bpm: 1 });
        assert_eq!(c.grid().bpm, Grid::MIN_BPM);
        apply(&mut c, Action::SetBpm { bpm: 100_000 });
        assert_eq!(c.grid().bpm, Grid::MAX_BPM);
    }

    #[test]
    fn set_time_sig_changes_metre_and_bar_numbering() {
        let mut c = Composer::new();
        assert_eq!(c.grid().time_sig.beats_per_bar, 4);

        apply(
            &mut c,
            Action::SetTimeSig {
                beats_per_bar: 3,
                beat_unit: 4,
            },
        );
        assert_eq!(c.grid().time_sig.beats_per_bar, 3);
        assert_eq!(c.grid().time_sig.beat_unit, 4);

        // At 120 BPM a 3/4 bar is 1.5 s, so 4.5 s in is bar 3 (0-indexed) beat 0
        // — under 4/4 the same instant would still be bar 2.
        assert_eq!(c.grid().bar_beat_of(4_500_000), (3, 0));

        // Illegal values are snapped, not rejected.
        apply(
            &mut c,
            Action::SetTimeSig {
                beats_per_bar: 0,
                beat_unit: 0,
            },
        );
        assert_eq!(c.grid().time_sig.beats_per_bar, Grid::MIN_BEATS_PER_BAR);
        assert!(c.grid().bar_us() > 0);
    }

    #[test]
    fn adjust_bpm_nudges_and_clamps_and_updates_snapshot() {
        let mut c = Composer::new();
        apply(&mut c, Action::AdjustBpm { delta: 10 });
        assert_eq!(c.grid().bpm, 130);
        assert_eq!(c.snapshot().bpm, 130.0);

        apply(&mut c, Action::AdjustBpm { delta: -20 });
        assert_eq!(c.grid().bpm, 110);

        // Extreme deltas clamp to the bounds.
        apply(&mut c, Action::AdjustBpm { delta: -100_000 });
        assert_eq!(c.grid().bpm, Grid::MIN_BPM);
        apply(&mut c, Action::AdjustBpm { delta: 100_000 });
        assert_eq!(c.grid().bpm, Grid::MAX_BPM);
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

    // ── grid origin + region quantise ────────────────────────────────────────

    #[test]
    fn set_grid_origin_phases_snapshot_and_snapping() {
        let mut c = Composer::new();
        apply(&mut c, Action::SetGridOrigin { us: 5_191_846 });
        assert_eq!(c.snapshot().grid_origin_us, 5_191_846);
        // Snap is now phased from the origin: origin itself is a grid line.
        assert_eq!(c.grid().snap(5_191_846), 5_191_846);
    }

    #[test]
    fn nudge_grid_origin_slides_phase_both_ways() {
        let mut c = Composer::new();
        apply(&mut c, Action::SetGridOrigin { us: 1_000_000 });

        apply(&mut c, Action::NudgeGridOrigin { delta_us: 10_000 });
        assert_eq!(c.snapshot().grid_origin_us, 1_010_000);

        apply(&mut c, Action::NudgeGridOrigin { delta_us: -250_000 });
        assert_eq!(c.snapshot().grid_origin_us, 760_000);
    }

    #[test]
    fn nudge_grid_origin_clamps_at_zero_instead_of_wrapping() {
        let mut c = Composer::new();
        apply(&mut c, Action::SetGridOrigin { us: 5_000 });
        // A nudge past zero must clamp — the origin is an unsigned phase, so a
        // wrap here would throw the bar lines to the far end of the timeline.
        apply(&mut c, Action::NudgeGridOrigin { delta_us: -250_000 });
        assert_eq!(c.snapshot().grid_origin_us, 0);

        apply(&mut c, Action::NudgeGridOrigin { delta_us: i64::MIN });
        assert_eq!(c.snapshot().grid_origin_us, 0);
    }

    #[test]
    fn quantize_region_snaps_onsets_and_ends_from_origin() {
        // Grid origin at the first note; 172 BPM 1/8 ≈ 174_418 µs step.
        let origin = 5_191_846u64;
        let step = 174_418u64;
        let mut tl = Timeline::new();
        // Onset a hair past the origin, off-grid end.
        tl.insert(note(70, origin + 4_000, 200_000));
        // Onset ~1.4 steps past origin → snaps to the 1st step; tiny duration.
        tl.insert(note(59, origin + step + 30_000, 20_000));
        // Onset outside the region → untouched.
        tl.insert(note(47, origin + 20 * step, 90_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        apply(&mut c, Action::SetGridOrigin { us: origin });

        apply(
            &mut c,
            Action::QuantizeRegion {
                start_us: origin,
                end_us: origin + 5 * step,
                step_us: step,
            },
        );

        let snap = c.snapshot();
        let by_pitch = |p: u8| snap.notes.iter().find(|n| n.pitch == p).copied().unwrap();
        // Note 70: onset snaps back to the origin (grid line 0); end snaps to the
        // nearest step, at least one step long.
        let n70 = by_pitch(70);
        assert_eq!(n70.start_us, origin, "onset snaps to origin grid line");
        assert_eq!(
            (n70.start_us - origin) % step,
            0,
            "onset lands on a grid line"
        );
        assert_eq!(
            (n70.dur_us) % step,
            0,
            "duration is a whole number of steps"
        );
        // Note 59: never shorter than one step even though it was ~20 ms.
        let n59 = by_pitch(59);
        assert!(
            n59.dur_us >= step,
            "quantised note is at least one step long"
        );
        assert_eq!((n59.start_us - origin) % step, 0);
        // Note 47 (outside region) is unchanged.
        let n47 = by_pitch(47);
        assert_eq!(n47.start_us, origin + 20 * step);
        assert_eq!(n47.dur_us, 90_000);
    }

    #[test]
    fn quantize_region_empty_is_noop_and_undoable() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 10_000_000, 100_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        // No note starts in this range → no-op.
        apply(
            &mut c,
            Action::QuantizeRegion {
                start_us: 0,
                end_us: 1_000_000,
                step_us: 100_000,
            },
        );
        assert_eq!(c.snapshot().notes.len(), 1);
        assert_eq!(c.snapshot().notes[0].start_us, 10_000_000);
    }

    // ── wait mode ("pause on note") ──────────────────────────────────────────

    /// Build a composer with two single notes and start it playing with wait
    /// mode armed. Notes: C4@500ms (100ms), E4@1000ms (100ms).
    fn wait_composer() -> Composer {
        let mut tl = Timeline::new();
        tl.insert(note(60, 500_000, 100_000));
        tl.insert(note(64, 1_000_000, 100_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        apply(&mut c, Action::SetWaitMode { on: true });
        assert!(c.is_wait_mode());
        apply(&mut c, Action::PlayFromStart);
        c
    }

    #[test]
    fn wait_mode_freezes_on_due_note_until_it_is_played() {
        let mut c = wait_composer();

        // Advance up to (past) the first onset: the note fires and the transport
        // reaches the note.
        let effects = c.advance(520_000);
        assert!(
            effects.iter().any(
                |e| matches!(e, Effect::AuditionNote { pitch: 60, velocity } if *velocity > 0)
            ),
            "the target note should sound as the playhead crosses its onset"
        );

        // Next tick with nothing held: frozen on the due note, playhead held.
        let head = c.playhead_us();
        let effects = c.advance(100_000);
        assert!(effects.is_empty(), "frozen: no effects fire");
        assert!(
            c.is_wait_frozen(),
            "should be frozen on the unsatisfied note"
        );
        assert!(!c.is_advancing(), "a freeze reads as not advancing");
        assert!(c.is_playing(), "but the transport is still in play mode");
        assert_eq!(c.playhead_us(), head, "playhead must not move while frozen");
        assert_eq!(c.awaiting_notes(), Some(vec![60]), "awaiting the C4");

        // Hold the required note: the next tick un-freezes and advances.
        c.ingest(on_ev(60));
        let head_before = c.playhead_us();
        c.advance(50_000);
        assert!(!c.is_wait_frozen(), "playing the note un-freezes");
        assert!(c.is_advancing());
        assert!(
            c.playhead_us() > head_before,
            "transport resumes after the note is played"
        );
        assert_eq!(c.awaiting_notes(), None);
    }

    #[test]
    fn wait_mode_freezes_again_on_the_next_note() {
        let mut c = wait_composer();
        // Cross + satisfy the first note.
        c.advance(520_000);
        c.ingest(on_ev(60));
        c.advance(20_000);
        assert!(!c.is_wait_frozen());
        // Release C, hold nothing, and run toward the second note.
        c.ingest(NoteEvent::off(MidiNote::new(60).unwrap(), 0));
        c.advance(500_000); // now well past E4's onset
        c.advance(50_000);
        assert!(c.is_wait_frozen(), "freezes again on the second note");
        assert_eq!(c.awaiting_notes(), Some(vec![64]));
    }

    #[test]
    fn disarmed_wait_mode_advances_freely() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 500_000, 100_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        apply(&mut c, Action::PlayFromStart); // wait mode OFF
        c.advance(2_000_000);
        assert!(!c.is_wait_frozen(), "disarmed never freezes");
        assert!(c.is_advancing());
        assert_eq!(c.playhead_us(), 2_000_000, "playhead runs straight through");
    }

    #[test]
    fn disarming_wait_mode_mid_freeze_resumes() {
        let mut c = wait_composer();
        c.advance(520_000);
        c.advance(50_000);
        assert!(c.is_wait_frozen());
        // Turn wait mode off: the freeze clears immediately.
        apply(&mut c, Action::SetWaitMode { on: false });
        assert!(!c.is_wait_frozen());
        assert!(c.is_advancing());
        let head = c.playhead_us();
        c.advance(100_000);
        assert!(c.playhead_us() > head, "resumes freely once disarmed");
    }

    #[test]
    fn wait_mode_snapshot_reports_frozen_and_awaiting() {
        let mut c = wait_composer();
        c.advance(520_000);
        c.advance(50_000);
        let snap = c.snapshot();
        assert!(snap.wait_mode, "snapshot reports wait mode armed");
        assert!(snap.frozen, "snapshot reports the freeze");
        assert!(
            snap.playing,
            "playing stays true while frozen (anchor on head)"
        );
        assert_eq!(snap.awaiting, Some(vec![60]));
    }

    #[test]
    fn wait_mode_starting_mid_song_freezes_on_the_first_later_note() {
        // Start playback after the first note: the gate must freeze on the
        // second note (E4@1s), not the already-passed first note.
        let mut tl = Timeline::new();
        tl.insert(note(60, 500_000, 100_000));
        tl.insert(note(64, 1_000_000, 100_000));
        let mut c = Composer::from_timeline(tl, Grid::default_120());
        apply(&mut c, Action::SetWaitMode { on: true });
        apply(&mut c, Action::Play { from_us: 700_000 });
        c.advance(320_000); // cross E4's onset at 1.0s
        c.advance(50_000);
        assert!(c.is_wait_frozen());
        assert_eq!(c.awaiting_notes(), Some(vec![64]));
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
    fn set_loop_start_uses_cursor_position() {
        let mut c = Composer::new();
        let step = c.grid().step_us();
        // Move cursor two steps right, then set the loop start there.
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::SetLoopStart);
        let (start, end) = c.loop_bounds();
        assert_eq!(start, 2 * step, "loop start follows the cursor");
        assert!(end > start, "region kept non-empty");
        assert_eq!(end, 3 * step, "end pushed one step past start");
    }

    #[test]
    fn set_loop_end_uses_cursor_position() {
        let mut c = Composer::new();
        let step = c.grid().step_us();
        // Set start at step 0 first.
        apply(&mut c, Action::SetLoopStart);
        // Move cursor to step 3 and mark the loop end there.
        for _ in 0..3 {
            apply(&mut c, Action::CursorRight);
        }
        apply(&mut c, Action::SetLoopEnd);
        let (start, end) = c.loop_bounds();
        assert_eq!(start, 0, "start unchanged");
        assert_eq!(end, 4 * step, "end includes the step under the cursor");
    }

    #[test]
    fn set_loop_start_past_end_keeps_region_non_empty() {
        let mut c = Composer::new();
        let step = c.grid().step_us();
        // Establish a small region [0, step).
        apply(
            &mut c,
            Action::SetLoopBounds {
                start_us: 0,
                end_us: step,
            },
        );
        // Move the cursor well past the end, then set the start there.
        for _ in 0..5 {
            apply(&mut c, Action::CursorRight);
        }
        apply(&mut c, Action::SetLoopStart);
        let (start, end) = c.loop_bounds();
        assert_eq!(start, 5 * step);
        assert_eq!(end, 6 * step, "end pushed past the new start");
        assert!(end > start);
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

    // ── backing alignment ──────────────────────────────────────────────────

    #[test]
    fn nudge_backing_offset_adjusts_clamps_and_shows_in_snapshot() {
        let mut c = Composer::new();
        assert_eq!(c.backing_offset_us(), 0);

        // Forward nudges accumulate.
        apply(&mut c, Action::NudgeBackingOffset { delta_us: 250_000 });
        apply(&mut c, Action::NudgeBackingOffset { delta_us: 10_000 });
        assert_eq!(c.backing_offset_us(), 260_000);
        assert_eq!(c.snapshot().backing_offset_us, 260_000);

        // Backward nudges subtract.
        apply(&mut c, Action::NudgeBackingOffset { delta_us: -10_000 });
        assert_eq!(c.backing_offset_us(), 250_000);

        // It clamps at 0 — never negative.
        apply(
            &mut c,
            Action::NudgeBackingOffset {
                delta_us: -1_000_000,
            },
        );
        assert_eq!(c.backing_offset_us(), 0);
        assert_eq!(c.snapshot().backing_offset_us, 0);
    }

    #[test]
    fn nudge_backing_offset_feeds_backing_position() {
        // The seek target the frontend uses is backing_position_us with the
        // nudged offset and the editor's zero shift.
        let mut c = Composer::new();
        apply(&mut c, Action::NudgeBackingOffset { delta_us: 250_000 });
        let offset = c.backing_offset_us();
        assert_eq!(
            crate::backing_position_us(1_000_000, 0, offset),
            Some(1_250_000)
        );
    }

    // ── background images (M14-D) ──────────────────────────────────────────

    /// A composer carrying two background layers, the first one selected.
    fn composer_with_backgrounds() -> Composer {
        use crate::background::BackgroundImage;
        let mut c = Composer::new();
        let stack = c.backgrounds_mut();
        stack.push(BackgroundImage::new("bg0", "bg0.png"));
        stack.push(BackgroundImage::new("bg1", "bg1.png"));
        stack.select(0);
        c
    }

    #[test]
    fn background_actions_are_no_ops_without_layers() {
        // Every background action on a piece with no images must be a harmless
        // no-op, so a frontend can bind the keys unconditionally.
        let mut c = Composer::new();
        for action in [
            Action::SelectBackground { index: 3 },
            Action::CycleBackground { delta: 1 },
            Action::NudgeBackgroundPos {
                dx_permille: 100,
                dy_permille: -100,
            },
            Action::NudgeBackgroundScale {
                delta_permille: 250,
            },
            Action::NudgeBackgroundRotation {
                delta_millideg: 5_000,
            },
            Action::SetBackgroundOpacity { permille: 500 },
            Action::SetBackgroundEasing {
                easing: Easing::Hold,
            },
            Action::AddBackgroundKeyframe,
            Action::DeleteBackgroundKeyframe,
        ] {
            assert!(apply(&mut c, action.clone()).is_empty(), "{action:?}");
        }
        assert!(c.backgrounds().is_empty());
        assert_eq!(c.snapshot().selected_background, None);
        assert!(c.snapshot().backgrounds.is_empty());
    }

    #[test]
    fn selection_actions_address_a_layer() {
        let mut c = composer_with_backgrounds();
        assert_eq!(c.backgrounds().selected_index(), Some(0));
        apply(&mut c, Action::SelectBackground { index: 1 });
        assert_eq!(c.backgrounds().selected_index(), Some(1));
        // Out of range leaves the selection alone.
        apply(&mut c, Action::SelectBackground { index: 9 });
        assert_eq!(c.backgrounds().selected_index(), Some(1));
        // Cycling wraps.
        apply(&mut c, Action::CycleBackground { delta: 1 });
        assert_eq!(c.backgrounds().selected_index(), Some(0));
        apply(&mut c, Action::CycleBackground { delta: -1 });
        assert_eq!(c.snapshot().selected_background, Some(1));
    }

    #[test]
    fn a_nudge_auto_keyframes_at_the_edit_time() {
        let mut c = composer_with_backgrounds();
        // Stopped, so the edit time is the cursor: move it off zero first.
        apply(&mut c, Action::CursorRight);
        let at_us = c.playhead_us();
        assert!(at_us > 0);

        apply(
            &mut c,
            Action::NudgeBackgroundPos {
                dx_permille: 250,
                dy_permille: -125,
            },
        );
        let layer = &c.backgrounds().layers()[0];
        assert_eq!(layer.keyframes.len(), 1, "the nudge created the keyframe");
        assert_eq!(layer.keyframes[0].time_us, at_us);
        assert!((layer.keyframes[0].transform.x - 0.25).abs() < 1e-5);
        assert!((layer.keyframes[0].transform.y + 0.125).abs() < 1e-5);

        // A second nudge at the same time edits that keyframe rather than
        // stacking another one.
        apply(
            &mut c,
            Action::NudgeBackgroundPos {
                dx_permille: 250,
                dy_permille: 0,
            },
        );
        let layer = &c.backgrounds().layers()[0];
        assert_eq!(layer.keyframes.len(), 1);
        assert!((layer.keyframes[0].transform.x - 0.5).abs() < 1e-5);

        // Only the selected layer moved.
        assert!(c.backgrounds().layers()[1].keyframes.is_empty());
    }

    #[test]
    fn scale_rotation_and_opacity_nudges_write_the_same_keyframe() {
        let mut c = composer_with_backgrounds();
        apply(
            &mut c,
            Action::NudgeBackgroundScale {
                delta_permille: 500,
            },
        );
        apply(
            &mut c,
            Action::NudgeBackgroundRotation {
                delta_millideg: 30_000,
            },
        );
        apply(&mut c, Action::SetBackgroundOpacity { permille: 400 });
        let layer = &c.backgrounds().layers()[0];
        assert_eq!(layer.keyframes.len(), 1);
        let t = layer.keyframes[0].transform;
        assert!((t.scale - 1.5).abs() < 1e-5);
        assert!((t.rotation_deg - 30.0).abs() < 1e-5);
        assert!((t.opacity - 0.4).abs() < 1e-5);
    }

    #[test]
    fn a_later_nudge_seeds_from_the_interpolated_transform() {
        let mut c = composer_with_backgrounds();
        // Keyframe at t=0 …
        apply(&mut c, Action::NudgeBackgroundScale { delta_permille: 0 });
        // … and one a bar later at 2×.
        apply(&mut c, Action::CursorBarRight);
        let end_us = c.playhead_us();
        apply(
            &mut c,
            Action::NudgeBackgroundScale {
                delta_permille: 1_000,
            },
        );
        assert_eq!(c.backgrounds().layers()[0].keyframes.len(), 2);

        // Halfway between them the layer is interpolated to 1.5×; nudging there
        // must start from 1.5, not from the identity.
        c.apply(Action::SetCursor { pitch: 60, step: 0 }).unwrap();
        c.start_play(end_us / 2);
        assert_eq!(c.playhead_us(), end_us / 2);
        apply(
            &mut c,
            Action::NudgeBackgroundScale {
                delta_permille: 100,
            },
        );
        let mid = c
            .backgrounds()
            .layers()
            .first()
            .and_then(|l| l.keyframe_at(end_us / 2))
            .expect("keyframe at the playhead");
        assert!(
            (mid.transform.scale - 1.6).abs() < 1e-4,
            "{:?}",
            mid.transform
        );
    }

    #[test]
    fn add_keyframe_pins_the_interpolated_transform_without_moving_it() {
        let mut c = composer_with_backgrounds();
        apply(&mut c, Action::AddBackgroundKeyframe);
        let layer = &c.backgrounds().layers()[0];
        assert_eq!(layer.keyframes.len(), 1);
        assert_eq!(layer.keyframes[0].transform, Transform::IDENTITY);
    }

    #[test]
    fn delete_keyframe_only_removes_the_one_at_the_edit_time() {
        let mut c = composer_with_backgrounds();
        apply(&mut c, Action::AddBackgroundKeyframe); // at 0
        apply(&mut c, Action::CursorRight);
        let second_us = c.playhead_us();
        apply(&mut c, Action::AddBackgroundKeyframe);
        assert_eq!(c.backgrounds().layers()[0].keyframes.len(), 2);

        apply(&mut c, Action::DeleteBackgroundKeyframe);
        let layer = &c.backgrounds().layers()[0];
        assert_eq!(layer.keyframes.len(), 1);
        assert!(layer.keyframe_at(second_us).is_none());
        assert!(layer.keyframe_at(0).is_some());

        // Deleting where there is nothing is a no-op.
        apply(&mut c, Action::DeleteBackgroundKeyframe);
        assert_eq!(c.backgrounds().layers()[0].keyframes.len(), 1);
    }

    #[test]
    fn set_easing_targets_an_existing_keyframe_and_survives_further_nudges() {
        let mut c = composer_with_backgrounds();
        // No keyframe here yet: setting the easing must not create one.
        apply(
            &mut c,
            Action::SetBackgroundEasing {
                easing: Easing::Hold,
            },
        );
        assert!(c.backgrounds().layers()[0].keyframes.is_empty());

        apply(&mut c, Action::AddBackgroundKeyframe);
        apply(
            &mut c,
            Action::SetBackgroundEasing {
                easing: Easing::EaseInOut,
            },
        );
        assert_eq!(
            c.backgrounds().layers()[0].keyframes[0].easing,
            Easing::EaseInOut
        );
        // A later nudge at the same time keeps the chosen curve.
        apply(
            &mut c,
            Action::NudgeBackgroundScale {
                delta_permille: 100,
            },
        );
        assert_eq!(
            c.backgrounds().layers()[0].keyframes[0].easing,
            Easing::EaseInOut
        );
    }

    #[test]
    fn snapshot_backgrounds_track_the_playhead() {
        let mut c = composer_with_backgrounds();
        apply(&mut c, Action::AddBackgroundKeyframe); // identity at 0
        apply(&mut c, Action::CursorBarRight);
        let end_us = c.playhead_us();
        apply(
            &mut c,
            Action::NudgeBackgroundPos {
                dx_permille: 1_000,
                dy_permille: 0,
            },
        );

        // Stopped at the far keyframe: fully panned.
        let snap = c.snapshot();
        assert_eq!(snap.backgrounds.len(), 2);
        assert!(snap.backgrounds[0].selected);
        assert!((snap.backgrounds[0].transform.x - 1.0).abs() < 1e-5);
        assert_eq!(snap.backgrounds[0].keyframes.len(), 2);
        // The unkeyframed sibling stays put.
        assert_eq!(snap.backgrounds[1].transform, Transform::IDENTITY);

        // Scrub the transport to the midpoint: half panned, no edit involved.
        c.start_play(end_us / 2);
        let snap = c.snapshot();
        assert!(
            (snap.backgrounds[0].transform.x - 0.5).abs() < 1e-3,
            "{:?}",
            snap.backgrounds[0].transform
        );
    }

    #[test]
    fn snapshot_with_backgrounds_round_trips_json() {
        let mut c = composer_with_backgrounds();
        apply(
            &mut c,
            Action::NudgeBackgroundScale {
                delta_permille: 500,
            },
        );
        let json = serde_json::to_string(&c.snapshot()).unwrap();
        let back: ComposerSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.selected_background, Some(0));
        assert_eq!(back.backgrounds.len(), 2);
        assert_eq!(back.backgrounds[0].file, "bg0.png");
        assert!((back.backgrounds[0].transform.scale - 1.5).abs() < 1e-5);
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

    // ── hand assignment (M14-E) ────────────────────────────────────────────

    /// The raw override of the note under the cursor.
    fn cursor_hand(c: &Composer) -> Option<Hand> {
        c.note_under_cursor()
            .and_then(|id| c.get_note(id))
            .and_then(|n| n.hand)
    }

    #[test]
    fn split_defaults_to_middle_c_and_is_settable() {
        let mut c = Composer::new();
        assert_eq!(c.hand_split(), DEFAULT_SPLIT);
        assert_eq!(c.snapshot().hand_split, DEFAULT_SPLIT);

        apply(&mut c, Action::SetHandSplit { pitch: 55 });
        assert_eq!(c.hand_split(), 55);
        assert_eq!(c.snapshot().hand_split, 55);
    }

    #[test]
    fn set_note_hand_with_no_target_is_a_noop_not_an_error() {
        let mut c = Composer::new();
        // Empty cell, no selection.
        assert!(c.note_under_cursor().is_none());
        assert_eq!(
            c.apply(Action::SetNoteHand {
                hand: HandSetting::Left
            }),
            Ok(Vec::new())
        );
        assert_eq!(c.apply(Action::CycleNoteHand), Ok(Vec::new()));
        assert_eq!(c.note_count(), 0, "no note is conjured");
    }

    #[test]
    fn set_note_hand_pins_the_cursor_note() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote); // middle C, right hand by default
        assert_eq!(cursor_hand(&c), None);

        apply(
            &mut c,
            Action::SetNoteHand {
                hand: HandSetting::Left,
            },
        );
        assert_eq!(cursor_hand(&c), Some(Hand::Left));
        let id = c.note_under_cursor().unwrap();
        assert_eq!(
            c.get_note(id).unwrap().effective_hand(c.hand_split()),
            Hand::Left,
            "the override beats the split line"
        );

        // Back to Auto clears the override entirely.
        apply(
            &mut c,
            Action::SetNoteHand {
                hand: HandSetting::Auto,
            },
        );
        assert_eq!(cursor_hand(&c), None);
    }

    #[test]
    fn set_note_hand_applies_to_the_whole_selection() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::SetCursor { pitch: 60, step: 0 });
        apply(&mut c, Action::StartSelection);
        apply(&mut c, Action::CursorRight);
        apply(
            &mut c,
            Action::SetNoteHand {
                hand: HandSetting::Left,
            },
        );

        let hands: Vec<Option<Hand>> = c.snapshot().notes.iter().map(|n| n.hand).collect();
        assert_eq!(hands, vec![Some(Hand::Left), Some(Hand::Left)]);
    }

    #[test]
    fn cycle_note_hand_walks_auto_left_right_auto() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        assert_eq!(cursor_hand(&c), None);
        apply(&mut c, Action::CycleNoteHand);
        assert_eq!(cursor_hand(&c), Some(Hand::Left));
        apply(&mut c, Action::CycleNoteHand);
        assert_eq!(cursor_hand(&c), Some(Hand::Right));
        apply(&mut c, Action::CycleNoteHand);
        assert_eq!(cursor_hand(&c), None);
    }

    #[test]
    fn hand_edits_undo_as_one_step() {
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(&mut c, Action::CycleNoteHand);
        assert_eq!(cursor_hand(&c), Some(Hand::Left));
        apply(&mut c, Action::Undo);
        assert_eq!(
            cursor_hand(&c),
            None,
            "undo restores the un-overridden note"
        );
    }

    #[test]
    fn snapshot_exposes_raw_override_and_split() {
        let mut c = Composer::new();
        apply(&mut c, Action::SetHandSplit { pitch: 64 });
        apply(&mut c, Action::AddNote); // middle C (60) — left of a 64 split
        apply(
            &mut c,
            Action::SetNoteHand {
                hand: HandSetting::Right,
            },
        );
        let snap = c.snapshot();
        assert_eq!(snap.hand_split, 64);
        assert_eq!(snap.notes.len(), 1);
        assert_eq!(snap.notes[0].hand, Some(Hand::Right));
        // The snapshot round-trips over the wire with both fields intact.
        let json = serde_json::to_string(&snap).unwrap();
        let back: ComposerSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hand_split, 64);
        assert_eq!(back.notes[0].hand, Some(Hand::Right));
    }

    #[test]
    fn override_rides_the_note_id_across_a_move() {
        // Grabbing and dragging a note keeps its exception in-session; the
        // saved `(pitch, start_us)` key just moves with it.
        let mut c = Composer::new();
        apply(&mut c, Action::AddNote);
        apply(
            &mut c,
            Action::SetNoteHand {
                hand: HandSetting::Left,
            },
        );
        let id = c.note_under_cursor().unwrap();
        apply(&mut c, Action::ToggleGrab);
        apply(&mut c, Action::CursorRight); // drag one step later
        apply(&mut c, Action::CursorUp); // and one semitone up
        apply(&mut c, Action::ToggleGrab);

        let moved = c.get_note(id).expect("same id after the drag");
        assert_eq!(moved.hand, Some(Hand::Left));
        assert_eq!(moved.pitch.value(), 61);
        // ...and the persisted key follows the note to its new position.
        assert_eq!(
            c.timeline().hand_overrides(),
            vec![HandOverride {
                pitch: 61,
                start_us: moved.start_us,
                hand: Hand::Left,
            }]
        );
    }

    #[test]
    fn hand_overrides_survive_a_save_load_round_trip() {
        // Save = MIDI events + meta overrides; load = from_events +
        // apply_hand_overrides. The exception and the split both come back.
        let mut c = Composer::new();
        apply(&mut c, Action::SetHandSplit { pitch: 55 });
        apply(&mut c, Action::AddNote);
        apply(
            &mut c,
            Action::SetNoteHand {
                hand: HandSetting::Left,
            },
        );
        apply(&mut c, Action::CursorRight);
        apply(&mut c, Action::AddNote); // no override

        let events = c.timeline().to_events();
        let saved = c.timeline().hand_overrides();
        let split = c.hand_split();
        assert_eq!(saved.len(), 1);

        let mut reloaded = Timeline::from_events(&events);
        reloaded.apply_hand_overrides(&saved);
        let mut c2 = Composer::from_timeline(reloaded, c.grid());
        c2.set_hand_split(split);

        assert_eq!(c2.hand_split(), 55);
        let snap = c2.snapshot();
        assert_eq!(snap.notes.len(), 2);
        assert_eq!(snap.notes[0].hand, Some(Hand::Left));
        assert_eq!(snap.notes[1].hand, None);
    }
}
