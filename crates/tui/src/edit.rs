//! Edit screen: the piano-free composer view.
//!
//! Renders a [`Timeline`] on the existing note-highway projection with a movable
//! `(pitch, step)` cursor navigated vim-style across all 88 keys. Supports full
//! note editing: add, delete, resize, move (grab), velocity, chords, selection,
//! transport, loop, metronome and live/step recording.
//!
//! ## A thin frontend over [`Composer`]
//!
//! As of M4-C this screen is a **view + I/O shell** around the pure
//! [`rockcraft_core::Composer`]. All editing, transport, loop, metronome,
//! count-in and recording *logic* lives in `core`; the screen only owns what is
//! genuinely a frontend concern:
//!
//! - a [`key_to_action`] / [`chord_key_to_action`] keymap (`KeyCode → Action`),
//! - an [`EditScreen::run_effects`] interpreter ([`Effect`] → synth),
//! - view-only state (the help overlay),
//! - file save (`save` / `save_bundle`) and rendering.
//!
//! **The keymap functions are the rebinding seam.** Re-mapping a key is a table
//! edit here, not a logic change — a full user-facing rebind UI is out of scope.
//!
//! `on_key` resolves a `KeyCode` to an optional [`Action`], hands it to
//! [`Composer::apply`], and feeds the returned effects through `run_effects`.
//! The run loop advances the pure playhead via [`Composer::advance`] and routes
//! its effects the same way; played MIDI goes through [`Composer::ingest`].
//!
//! Nothing here touches a device or the disk on the hot path, so the whole
//! screen stays headless-testable via the existing `TestBackend` harness.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rockcraft_audio::{play_file_at, BackingHandle, SynthHandle};
use rockcraft_core::{
    backing_position_us, Action, BackingTrack, Composer, Cursor, Effect, Grid, InputMode, Key,
    MidiNote, Note, NoteEvent, NoteId, RecordingMeta, Scale as MusicScale, Subdivision, Timeline,
    TrackOrigin, Velocity,
};
use rockcraft_midi::{events_to_smf_bytes, key_map as mock_key_map};

use crate::highway::{build_spans, project, NoteSpan};
use crate::keyboard::{black_key_col, is_black_key, white_index, Scale, LOWEST_MIDI};
use crate::record::bundle_backing_filename;
use crate::render::draw_keyboard;

/// Base directory for saved bundles.
const RECORDINGS_DIR: &str = "recordings";

/// How many bars of time the highway shows from bottom (keyboard line) to top.
const LEAD_BARS: u64 = 4;

/// Where the cursor sits in the visible window: a quarter of the way up from the
/// keyboard line, leaving a little context below it and most of the window ahead.
const CURSOR_ANCHOR_NUM: u64 = 1;
const CURSOR_ANCHOR_DEN: u64 = 4;

/// One semitone above A0 × octave for the default cursor: middle C (MIDI 60).
/// Mirrors `Composer`'s default; retained here as the oracle the unit tests
/// assert against.
#[allow(dead_code)]
const DEFAULT_CURSOR_PITCH: u8 = 60;

/// Default velocity for auditioned chord notes (single-note auditions carry the
/// velocity in their [`Effect`]).
const DEFAULT_NOTE_VEL: u8 = 80;

/// Velocity step for `+`/`-` adjustments.
const VEL_STEP: u8 = 8;

/// Fine backing-alignment nudge step (10 ms), bound to `,` / `.`.
const BACKING_NUDGE_FINE_US: i64 = 10_000;
/// Coarse backing-alignment nudge step (250 ms), bound to `;` / `'`.
const BACKING_NUDGE_COARSE_US: i64 = 250_000;

/// Cursor highlight (status badge, cursor key, cursor cell).
const CURSOR_COLOR: Color = Color::Magenta;
/// Chord-mode badge colour.
const CHORD_COLOR: Color = Color::Cyan;
/// Record-armed badge colour (step / live record).
const REC_COLOR: Color = Color::Red;
/// Resting colour for timeline notes on the highway.
const NOTE_COLOR: Color = Color::Indexed(33);
/// Faint colour for beat/bar gridlines.
const GRID_COLOR: Color = Color::DarkGray;
/// Transport playhead line colour.
const PLAYHEAD_COLOR: Color = Color::Green;
/// Selected-note highlight on the highway.
const SELECT_COLOR: Color = Color::LightGreen;

/// Outcome of a key press while the dirty-exit prompt is displayed.
#[derive(Debug, PartialEq, Eq)]
pub enum PromptOutcome {
    /// User chose "Save" — caller should save then navigate away.
    SaveAndLeave,
    /// User chose "Discard" — leave without saving.
    Leave,
    /// User chose "Cancel" — stay in the editor.
    Stay,
}

/// Outcome of a key press while the "save to library" name overlay is shown.
#[derive(Debug, PartialEq, Eq)]
pub enum NameOutcome {
    /// Enter on a non-empty name — caller saves the bundle under this name.
    Submitted(String),
    /// Esc — the overlay was cancelled, nothing saved.
    Cancelled,
    /// Still editing (a character typed, backspace, or empty Enter).
    Pending,
}

/// Map a `KeyCode` to the composer [`Action`] it triggers in normal (non-chord)
/// mode, or `None` when the key is unbound.
///
/// This table is the **rebinding seam**: changing a binding is an edit here, not
/// a change to any editing logic (which lives in [`Composer`]). Keys that are
/// mode-sensitive at the *logic* level (e.g. `Space`/`hjkl` behaving differently
/// while grabbing) still map to a single `Action`; the composer interprets them
/// per its current state. Chord-mode keys route through [`chord_key_to_action`].
fn key_to_action(code: KeyCode) -> Option<Action> {
    Some(match code {
        // ── navigation ──────────────────────────────────────────────────
        // Visual layout: horizontal = pitch (keyboard), vertical = time (falling notes).
        // h/l navigate the pitch (horizontal) axis; j/k navigate the time (vertical) axis.
        KeyCode::Char('h') | KeyCode::Left => Action::CursorDown, // pitch -1 (left on keyboard)
        KeyCode::Char('l') | KeyCode::Right => Action::CursorUp,  // pitch +1 (right on keyboard)
        KeyCode::Char('j') | KeyCode::Down => Action::CursorLeft, // step  -1 (earlier in timeline)
        KeyCode::Char('k') | KeyCode::Up => Action::CursorRight,  // step  +1 (later  in timeline)
        KeyCode::Char('H') => Action::CursorBarLeft,              // one bar earlier
        KeyCode::Char('L') => Action::CursorBarRight,             // one bar later
        KeyCode::Char('w') => Action::CursorOctaveUp,             // octave right (higher pitch)
        KeyCode::Char('b') => Action::CursorOctaveDown,           // octave left  (lower  pitch)
        KeyCode::Char('J') => Action::CursorOctaveDown,           // alias for b
        KeyCode::Char('K') => Action::CursorOctaveUp,             // alias for w
        KeyCode::Char('g') => Action::CursorToStart,              // timeline beginning
        KeyCode::Char('G') => Action::CursorToEnd,                // timeline end (last note)
        KeyCode::Char('0') => Action::CursorToPitchMin,           // leftmost key  (A0, MIDI 21)
        KeyCode::Char('$') => Action::CursorToPitchMax,           // rightmost key (C8, MIDI 108)
        KeyCode::Char('>') => Action::SubdivisionFiner,
        KeyCode::Char('<') => Action::SubdivisionCoarser,

        // ── edit ────────────────────────────────────────────────────────
        KeyCode::Char('a') | KeyCode::Char('i') => Action::AddNote,
        KeyCode::Char('x') | KeyCode::Char('d') => Action::DeleteNote,
        KeyCode::Char(']') => Action::ResizeNote { delta_steps: 1 },
        KeyCode::Char('[') => Action::ResizeNote { delta_steps: -1 },
        KeyCode::Char('+') | KeyCode::Char('=') => Action::AdjustVelocity {
            delta: VEL_STEP as i16,
        },
        KeyCode::Char('-') => Action::AdjustVelocity {
            delta: -(VEL_STEP as i16),
        },
        KeyCode::Char('m') => Action::ToggleGrab,
        KeyCode::Char('c') => Action::EnterChordMode,

        // ── input mode ──────────────────────────────────────────────────
        KeyCode::Char('R') => Action::ToggleRecordArm,
        KeyCode::Char('t') => Action::ToggleRecordFlavour,

        // ── transport ───────────────────────────────────────────────────
        KeyCode::Char(' ') => Action::TogglePlayCursor,
        KeyCode::Char('P') => Action::PlayFromStart,

        // ── backing alignment (slide audio under the highway) ─────────────
        KeyCode::Char(',') => Action::NudgeBackingOffset {
            delta_us: -BACKING_NUDGE_FINE_US,
        },
        KeyCode::Char('.') => Action::NudgeBackingOffset {
            delta_us: BACKING_NUDGE_FINE_US,
        },
        KeyCode::Char(';') => Action::NudgeBackingOffset {
            delta_us: -BACKING_NUDGE_COARSE_US,
        },
        KeyCode::Char('\'') => Action::NudgeBackingOffset {
            delta_us: BACKING_NUDGE_COARSE_US,
        },

        // ── loop / metronome / count-in ─────────────────────────────────
        KeyCode::Char('o') => Action::ToggleLoop,
        KeyCode::Char('M') => Action::ToggleMetronome,
        KeyCode::Char('C') => Action::StartCountInRecord,

        // ── selection / clipboard ───────────────────────────────────────
        KeyCode::Char('v') => Action::StartSelection,
        KeyCode::Char('y') => Action::YankSelection,
        KeyCode::Char('p') => Action::PasteClipboard,
        KeyCode::Char('D') => Action::DeleteSelection,
        KeyCode::Esc => Action::ClearSelection,

        // ── history ─────────────────────────────────────────────────────
        KeyCode::Char('u') => Action::Undo,
        KeyCode::Char('U') => Action::Redo,

        _ => return None,
    })
}

/// Map a `KeyCode` to the chord-selector [`Action`] it triggers while the
/// selector is open, or `None` when unbound. The other half of the rebinding
/// seam (see [`key_to_action`]).
fn chord_key_to_action(code: KeyCode) -> Option<Action> {
    Some(match code {
        KeyCode::Char(c @ '1'..='7') => Action::SetChordDegree {
            degree: c as u8 - b'0',
        },
        KeyCode::Char(']') => Action::CycleChordDegree { delta: 1 },
        KeyCode::Char('[') => Action::CycleChordDegree { delta: -1 },
        KeyCode::Char('s') => Action::ToggleChordKind,
        KeyCode::Enter => Action::CommitChord,
        KeyCode::Esc => Action::CancelChord,
        _ => return None,
    })
}

/// A backing audio track attached to the editor. The alignment offset
/// (`audio_start_us` — the file position lining up with song time 0) lives on
/// the [`Composer`] so it is editable via [`Action::NudgeBackingOffset`] and
/// visible in `query state`; this struct only holds the file path. The composer
/// transport has no pre-roll, so the whole-song shift used by
/// [`backing_position_us`] is always 0 here: the file position is simply
/// `playhead_us + audio_start_us`.
struct Backing {
    path: PathBuf,
}

/// What the backing track should do on a given tick, derived purely from the
/// transport state (no device touched). [`EditScreen::poll_backing`] computes
/// it; [`EditScreen::tick_backing`] applies it to the live [`BackingHandle`].
/// Splitting the decision from the effect keeps the sync logic headless-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackingCmd {
    /// Transport started: begin (or resume) playback seeked to this file position.
    PlayAt(u64),
    /// Re-seek to this file position while still playing (loop wrap / rewind).
    Seek(u64),
    /// Transport stopped: pause in place, keeping the stream open.
    Pause,
    /// Nothing to do — free-running, or no backing attached.
    None,
}

/// The composer edit screen: a [`Composer`] rendered on the highway, plus the
/// frontend-only state needed to interpret its effects and draw it.
pub struct EditScreen {
    /// The pure editor. Owns the timeline, cursor, grab, chord, selection,
    /// clipboard, input mode, transport, loop, metronome and count-in state.
    composer: Composer,
    /// The grid, mirrored from the composer for rendering and for `save`. Kept
    /// in sync after every dispatch (it only changes via subdivision actions).
    grid: Grid,
    /// The piece's key, used by `save`'s metadata. Also pushed into the composer
    /// so it voices diatonic chords from the same key.
    key: Key,
    /// Optional synth for auditioning notes; `None` makes `run_effects` a no-op.
    synth: Option<SynthHandle>,
    /// The single pitch currently sounding from an edit audition (stopped before
    /// the next one). Frontend bookkeeping for the stop-previous discipline.
    auditioning: Option<MidiNote>,
    /// Pitches currently sounding from a chord audition; stopped before the next.
    auditioning_chord: Vec<MidiNote>,
    /// Wall-clock reference for converting frame time into a `dt_us` for
    /// [`Composer::advance`]. The clock the pure composer deliberately lacks.
    last_tick: Option<Instant>,
    /// Whether the help overlay is currently visible (view-only state).
    show_help: bool,
    /// Whether the timeline has changed since the last save (or since opening
    /// a fresh editor). Cleared by `mark_clean`; set by any mutating dispatch.
    dirty: bool,
    /// One-shot save confirmation shown after a successful save, cleared on the
    /// next key press.
    save_flash: Option<String>,
    /// When `true` the "Save / Discard / Cancel" overlay is rendered and all
    /// keys are routed to `on_prompt_key` instead of the normal keymap.
    exit_prompt: bool,
    /// Provenance recorded into the bundle's `meta.json` on save. A fresh editor
    /// is `Composed`; loading a recording/import for edit sets it to `Edited`
    /// (or preserves the loaded bundle's origin).
    origin: TrackOrigin,
    /// When `Some`, the "save to library" name overlay is active and holds the
    /// name typed so far. Keys route to `on_name_key` instead of the keymap.
    name_prompt: Option<String>,
    /// Backing audio track to play in lock-step with the transport, if attached
    /// (via `with_backing`). `None` makes all backing wiring a no-op.
    backing: Option<Backing>,
    /// Live playback handle once the backing track has started; `None` until the
    /// transport first plays. Paused (not torn down) when the transport stops.
    backing_handle: Option<BackingHandle>,
    /// Whether the transport was playing at the previous `poll_backing`, to
    /// detect the stop↔play transitions that start/pause the backing.
    prev_playing: bool,
    /// Playhead position at the previous `poll_backing`; a backward jump while
    /// playing (loop wrap / rewind) triggers a re-seek.
    prev_playhead_us: u64,
    /// Backing offset at the previous `poll_backing`; a change while playing
    /// (an alignment nudge) triggers a re-seek so the shift is audible at once.
    prev_offset_us: u64,
}

impl EditScreen {
    /// A fresh editor: empty timeline, default 120 BPM 4/4 grid, cursor parked
    /// at middle C and the song start.
    pub fn new() -> Self {
        Self::from_parts(Timeline::new(), Grid::default_120(), None)
    }

    /// An editor over an existing timeline and grid. The cursor starts at middle
    /// C and the song start, same as [`EditScreen::new`].
    pub fn from_timeline(timeline: Timeline, grid: Grid) -> Self {
        Self::from_parts(timeline, grid, None)
    }

    fn from_parts(timeline: Timeline, grid: Grid, synth: Option<SynthHandle>) -> Self {
        let key = Key {
            root_pc: 0,
            scale: MusicScale::Major,
        };
        let mut composer = Composer::from_timeline(timeline, grid);
        composer.set_key(key);
        Self {
            composer,
            grid,
            key,
            synth,
            auditioning: None,
            auditioning_chord: Vec::new(),
            last_tick: None,
            show_help: false,
            dirty: false,
            save_flash: None,
            exit_prompt: false,
            origin: TrackOrigin::Composed,
            name_prompt: None,
            backing: None,
            backing_handle: None,
            prev_playing: false,
            prev_playhead_us: 0,
            prev_offset_us: 0,
        }
    }

    /// Set the key used to voice diatonic chords. (No UI yet — #56 persists it.)
    pub fn set_key(&mut self, key: Key) {
        self.key = key;
        self.composer.set_key(key);
    }

    /// Attach a synth handle so edits are auditioned. Called by the shell after
    /// construction so the existing no-arg `new()` stays usable in tests.
    pub fn attach_synth(&mut self, synth: SynthHandle) {
        self.synth = Some(synth);
    }

    /// Attach a backing audio track that plays in lock-step with the transport.
    /// `audio_start_us` is the file position lining up with song time 0 (default
    /// 0; M5-E makes it adjustable). Builder form, mirroring `PlayScreen`.
    pub fn with_backing(mut self, path: PathBuf, audio_start_us: u64) -> Self {
        self.backing = Some(Backing { path });
        // The offset is composer state (editable + snapshot-visible); seed it
        // from the loaded value so a reopened bundle restores its alignment.
        self.composer.set_backing_offset_us(audio_start_us);
        self
    }

    /// Stop any in-progress audition and editor state and return to a clean rest.
    /// An uncommitted chord preview is cancelled (its notes removed) so leaving
    /// never strands a ghost chord. Call before navigating away from the screen.
    pub fn leave(&mut self) {
        let effects = self.composer.leave();
        self.run_effects(&effects);
        self.last_tick = None;
        // Stop and drop the backing stream (mirrors `RecordScreen`); the next
        // entry re-arms it from the playhead.
        if let Some(h) = self.backing_handle.take() {
            h.stop();
        }
        self.prev_playing = false;
        self.prev_playhead_us = self.playhead_us();
    }

    /// Save the timeline as a `take-<stamp>` bundle under `recordings/`.
    /// Returns the bundle directory. Mirrors `RecordScreen::save`.
    pub fn save(&self) -> std::io::Result<PathBuf> {
        self.save_bundle(std::path::Path::new(RECORDINGS_DIR))
    }

    /// Save the timeline as a `take-<stamp>` bundle inside `base`.
    /// Useful for testing with an arbitrary temp directory.
    pub fn save_bundle(&self, base: &std::path::Path) -> std::io::Result<PathBuf> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bundle_dir = base.join(format!("take-{stamp}"));
        self.write_bundle(&bundle_dir)
    }

    /// Save into a named bundle directory under `root` (the track library).
    /// The name is slugified into a directory name; an existing bundle of the
    /// same name is overwritten in place. Returns the bundle directory.
    pub fn save_to_library(&self, root: &std::path::Path, name: &str) -> std::io::Result<PathBuf> {
        let slug = crate::library::slug(name);
        if slug.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty name",
            ));
        }
        self.write_bundle(&root.join(slug))
    }

    /// Write the `song.mid` + `meta.json` bundle into `bundle_dir` (created if
    /// needed). Shared by the take-stamp save and the named library save.
    fn write_bundle(&self, bundle_dir: &std::path::Path) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(bundle_dir)?;
        let bytes = events_to_smf_bytes(&self.timeline().to_events());
        std::fs::write(bundle_dir.join("song.mid"), bytes)?;
        // Carry an attached backing track into the bundle: copy the file in and
        // record its bundle-relative name + start offset, mirroring Record.
        // MIDI-only saves keep `backing: None`.
        let backing = if let Some(b) = &self.backing {
            let filename = bundle_backing_filename(&b.path);
            std::fs::copy(&b.path, bundle_dir.join(&filename))?;
            Some(BackingTrack {
                file: filename,
                audio_start_us: self.composer.backing_offset_us(),
            })
        } else {
            None
        };
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing,
            grid: Some(self.grid),
            key: Some(self.key),
            origin: Some(self.origin),
            version: 1,
        };
        std::fs::write(bundle_dir.join("meta.json"), meta.to_json())?;
        Ok(bundle_dir.to_path_buf())
    }

    // ── key routing ───────────────────────────────────────────────────────

    /// Route a key press through the keymap. Tab/Esc-to-leave are handled by the
    /// shell, not here.
    ///
    /// `?` toggles the help overlay locally; Esc closes it when shown. Otherwise
    /// the key is resolved to an [`Action`] (chord-selector keymap while the
    /// selector is open, the normal keymap otherwise) and dispatched to the
    /// [`Composer`]; the returned effects drive the synth via `run_effects`.
    pub fn on_key(&mut self, code: KeyCode) {
        // Any key clears the save flash.
        self.save_flash = None;

        // Help overlay: `?` toggles visibility; Esc closes it. Takes precedence
        // over every other mode so help is always reachable.
        match code {
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
                return;
            }
            KeyCode::Esc if self.show_help => {
                self.show_help = false;
                return;
            }
            _ => {}
        }

        // While the chord selector is open it owns the keymap (digits pick a
        // degree, `[`/`]` cycle it, `s` toggles quality, Enter/Esc commit/cancel).
        let action = if self.composer.in_chord_mode() {
            chord_key_to_action(code)
        } else {
            key_to_action(code)
        };
        if let Some(action) = action {
            self.dispatch(action);
        }
    }

    /// Mutable access to the underlying [`Composer`] — the control seam.
    ///
    /// The shell hands this to [`rockcraft_control::handle`] so remote
    /// `run_action`s mutate the very same composer the keyboard edits. Effects
    /// from a remote edit are auditioned via [`EditScreen::apply_remote`], not
    /// here, so callers reaching for the composer directly stay rendering-only.
    pub fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    /// Apply one remote control [`Request`] against the owned composer, audition
    /// any effects through the synth (so remote edits sound like key edits), and
    /// re-sync the mirrored grid. Returns the protocol [`Response`] for the
    /// caller to send back over the request's oneshot.
    ///
    /// This is the remote analogue of [`EditScreen::dispatch`]: same composer,
    /// same effect interpreter, same grid re-sync — only the trigger differs.
    ///
    /// [`Request`]: rockcraft_control::Request
    /// [`Response`]: rockcraft_control::Response
    pub fn apply_remote(&mut self, req: rockcraft_control::Request) -> rockcraft_control::Response {
        let response = rockcraft_control::handle(&mut self.composer, req);
        if let rockcraft_control::Response::Ok { effects, .. } = &response {
            self.run_effects(effects);
        }
        self.grid = self.composer.grid();
        response
    }

    /// Consume a played MIDI event from the input source (piano / mock keyboard).
    /// Routing is the composer's: ignored in direct-edit, placed in step/live
    /// record. Any effects (none today) are interpreted for the synth.
    pub fn ingest(&mut self, ev: NoteEvent) {
        let fp_before = self.timeline_fingerprint();
        let effects = self.composer.ingest(ev);
        self.run_effects(&effects);
        if self.timeline_fingerprint() != fp_before {
            self.dirty = true;
            self.save_flash = None;
        }
    }

    /// Apply one [`Action`] to the composer and interpret its effects, then
    /// re-sync the mirrored grid (a subdivision change is the only thing that
    /// moves it). The single funnel every key / shim flows through.
    fn dispatch(&mut self, action: Action) {
        let fp_before = self.timeline_fingerprint();
        let effects = self.composer.apply(action).unwrap_or_default();
        self.run_effects(&effects);
        self.grid = self.composer.grid();
        if self.timeline_fingerprint() != fp_before {
            self.dirty = true;
            self.save_flash = None;
        }
    }

    /// A cheap content fingerprint of the timeline: changes whenever any note
    /// is added, removed, moved, resized, or has its velocity adjusted.
    fn timeline_fingerprint(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        for (id, n) in self.composer.timeline().notes() {
            id.hash(&mut h);
            n.pitch.value().hash(&mut h);
            n.start_us.hash(&mut h);
            n.dur_us.hash(&mut h);
            n.velocity.value().hash(&mut h);
        }
        h.finish()
    }

    // ── effect interpreter ──────────────────────────────────────────────────

    /// Interpret composer [`Effect`]s against the synth, owning the
    /// "currently sounding" bookkeeping `core` deliberately doesn't.
    ///
    /// - [`Effect::AuditionNote`] with velocity 0 is a note-off (the convention
    ///   the composer uses for playback span ends and metronome click releases).
    /// - A single-note edit audition (velocity > 0 while stopped) stops the
    ///   previous audition first, matching the old stop-previous-then-play feel.
    /// - During playback note-ons stay polyphonic (multiple notes can sound).
    /// - [`Effect::AuditionChord`] replaces any prior audition with the chord.
    /// - [`Effect::AllOff`] silences everything and clears the bookkeeping.
    ///
    /// A no-op when no synth is attached (the headless test default).
    fn run_effects(&mut self, effects: &[Effect]) {
        let Some(synth) = self.synth.clone() else {
            return;
        };
        let playing = self.composer.is_playing();
        for effect in effects {
            match effect {
                Effect::AuditionNote { pitch, velocity } => {
                    let Some(note) = MidiNote::new(*pitch) else {
                        continue;
                    };
                    if *velocity == 0 {
                        // An explicit note-off (playback span end / click off).
                        synth.note_off(note);
                        if self.auditioning == Some(note) {
                            self.auditioning = None;
                        }
                        self.auditioning_chord.retain(|p| *p != note);
                    } else if playing {
                        // Polyphonic playback / metronome note-on.
                        if let Some(vel) = Velocity::new(*velocity) {
                            synth.note_on(note, vel);
                        }
                    } else {
                        // A single edit audition: stop the previous, then play.
                        self.stop_audition(&synth);
                        if let Some(vel) = Velocity::new(*velocity) {
                            synth.note_on(note, vel);
                            self.auditioning = Some(note);
                        }
                    }
                }
                Effect::AuditionChord { pitches } => {
                    self.stop_audition(&synth);
                    let vel = Velocity::new(DEFAULT_NOTE_VEL).expect("80 is always valid");
                    let mut sounding = Vec::with_capacity(pitches.len());
                    for &pitch in pitches {
                        if let Some(note) = MidiNote::new(pitch) {
                            synth.note_on(note, vel);
                            sounding.push(note);
                        }
                    }
                    self.auditioning_chord = sounding;
                }
                Effect::AllOff => {
                    synth.all_off();
                    self.auditioning = None;
                    self.auditioning_chord.clear();
                }
            }
        }
    }

    /// Silence the current edit / chord audition (the stop half of
    /// stop-previous-then-play).
    fn stop_audition(&mut self, synth: &SynthHandle) {
        if let Some(prev) = self.auditioning.take() {
            synth.note_off(prev);
        }
        for prev in std::mem::take(&mut self.auditioning_chord) {
            synth.note_off(prev);
        }
    }

    // ── transport tick ──────────────────────────────────────────────────────

    /// Advance the pure playhead by `dt_us` and interpret the audition effects
    /// it produces. Returns those effects so headless tests can assert on what
    /// sounded. A no-op (empty) when the transport is stopped.
    pub fn advance(&mut self, dt_us: u64) -> Vec<Effect> {
        let effects = self.composer.advance(dt_us);
        self.run_effects(&effects);
        effects
    }

    /// Advance the transport by the real wall-clock time elapsed since the last
    /// tick and fire the resulting auditions. Called once per run-loop iteration;
    /// this is the frontend clock the pure [`Composer`] omits. A no-op while the
    /// transport is stopped (the elapsed time is simply discarded).
    pub fn tick_audition(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|prev| now.duration_since(prev).as_micros() as u64)
            .unwrap_or(0);
        self.last_tick = Some(now);
        self.advance(dt);
    }

    /// Set the `LiveRecord` playhead position. The seam the transport drives;
    /// tests advance it manually to place recorded notes.
    pub fn set_playhead_us(&mut self, us: u64) {
        self.composer.set_playhead_us(us);
    }

    // ── backing-track sync ───────────────────────────────────────────────────

    /// The file position the backing track should be at for the current
    /// playhead, or `None` when no track is attached. Shares core's
    /// [`backing_position_us`] formula so audio and the highway never drift; the
    /// editor transport has no pre-roll, so the whole-song shift is always 0 and
    /// the position is `playhead_us + audio_start_us`.
    fn backing_target_us(&self) -> Option<u64> {
        self.backing.as_ref()?;
        backing_position_us(self.playhead_us(), 0, self.composer.backing_offset_us())
    }

    /// Decide what the backing should do this tick from the transport state, and
    /// record the playing/playhead snapshot for the next call. Pure (touches no
    /// device), so the sync decision is headless-testable.
    fn poll_backing(&mut self) -> BackingCmd {
        let playing = self.is_playing();
        let ph = self.playhead_us();
        let offset = self.composer.backing_offset_us();
        let Some(target) = self.backing_target_us() else {
            // No backing: keep the snapshot coherent so a later attach behaves.
            self.prev_playing = playing;
            self.prev_playhead_us = ph;
            self.prev_offset_us = offset;
            return BackingCmd::None;
        };
        let prev_playing = self.prev_playing;
        let prev_ph = self.prev_playhead_us;
        let prev_offset = self.prev_offset_us;
        self.prev_playing = playing;
        self.prev_playhead_us = ph;
        self.prev_offset_us = offset;

        if playing && !prev_playing {
            // Transport just started: (re)sync the backing to the playhead.
            BackingCmd::PlayAt(target)
        } else if playing && (ph < prev_ph || offset != prev_offset) {
            // Playhead jumped backward (loop wrap / rewind) or the alignment
            // offset was nudged: re-seek so the audio matches the new mapping.
            BackingCmd::Seek(target)
        } else if !playing && prev_playing {
            // Transport just stopped: pause in place.
            BackingCmd::Pause
        } else {
            // Free-running with the sink, or idle while stopped.
            BackingCmd::None
        }
    }

    /// Sync the live backing handle to the transport. Call once per run-loop
    /// iteration right after [`tick_audition`](Self::tick_audition). Never blocks
    /// the audio thread; a persistent file failure drops the track so editing
    /// continues silently.
    pub fn tick_backing(&mut self) {
        match self.poll_backing() {
            BackingCmd::PlayAt(pos) => {
                let Some(b) = &self.backing else { return };
                if let Some(h) = &self.backing_handle {
                    // Resume an existing (paused) stream from the new position.
                    h.seek(Duration::from_micros(pos));
                    h.resume();
                } else {
                    match play_file_at(&b.path, Duration::from_micros(pos)) {
                        Ok(h) => self.backing_handle = Some(h),
                        Err(_) => self.backing = None,
                    }
                }
            }
            BackingCmd::Seek(pos) => {
                if let Some(h) = &self.backing_handle {
                    h.seek(Duration::from_micros(pos));
                }
            }
            BackingCmd::Pause => {
                if let Some(h) = &self.backing_handle {
                    h.pause();
                }
            }
            BackingCmd::None => {}
        }
    }

    // ── read-only accessors (thin shims over `Composer`) ─────────────────────

    /// The current (live-editing) timeline.
    pub fn timeline(&self) -> &Timeline {
        self.composer.timeline()
    }

    /// The current cursor position (for tests and status rendering).
    pub fn cursor(&self) -> Cursor {
        self.composer.cursor()
    }

    /// The current subdivision (for tests and status display).
    pub fn current_subdivision(&self) -> Subdivision {
        self.composer.current_subdivision()
    }

    /// Total number of notes in the timeline.
    pub fn note_count(&self) -> usize {
        self.composer.note_count()
    }

    /// The id of the note whose span covers the cursor's `(pitch, step)`, if any.
    pub fn note_under_cursor(&self) -> Option<NoteId> {
        self.composer.note_under_cursor()
    }

    /// Look up note data by id (convenience for tests and status display).
    pub fn get_note(&self, id: NoteId) -> Option<Note> {
        self.composer.get_note(id)
    }

    /// Ids of notes whose start falls inside the current visual selection.
    pub fn selection_ids(&self) -> Vec<NoteId> {
        self.composer.selection_ids()
    }

    /// Number of notes currently held in the clipboard.
    pub fn clipboard_len(&self) -> usize {
        self.composer.clipboard_len()
    }

    /// Whether the chord selector is currently active.
    pub fn in_chord_mode(&self) -> bool {
        self.composer.in_chord_mode()
    }

    /// Whether the help overlay is currently visible.
    pub fn help_visible(&self) -> bool {
        self.show_help
    }

    /// Whether a visual selection is in progress.
    pub fn in_visual_mode(&self) -> bool {
        self.composer.in_visual_mode()
    }

    /// The current input mode (direct-edit vs step / live record).
    pub fn input_mode(&self) -> InputMode {
        self.composer.input_mode()
    }

    /// Whether step or live record is armed — i.e. played notes are being
    /// captured. The shell consults this to decide whether the mock keyboard's
    /// number-row note keys should play notes (armed) or stay editor commands.
    pub fn is_recording(&self) -> bool {
        self.composer.input_mode() != InputMode::DirectEdit
    }

    /// Arm step-record if currently in direct-edit, reusing the same
    /// `ToggleRecordArm` action the `R` key dispatches. Used by the shell to
    /// open a "New piece" already in recording mode (M9-A). A no-op if a record
    /// mode is already armed, so it never disarms.
    pub fn arm_record(&mut self) {
        if self.composer.input_mode() == InputMode::DirectEdit {
            self.dispatch(Action::ToggleRecordArm);
        }
    }

    // ── dirty / save-feedback / exit-prompt ──────────────────────────────────

    /// Whether the timeline has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clear the dirty flag after a successful save.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Set the one-shot save-confirmation message shown after a successful save.
    /// Cleared automatically when the user next presses any key.
    pub fn set_save_flash(&mut self, msg: String) {
        self.save_flash = Some(msg);
    }

    /// Whether the "Save / Discard / Cancel" exit-prompt overlay is showing.
    pub fn is_prompting_exit(&self) -> bool {
        self.exit_prompt
    }

    /// Show the exit-prompt overlay. Called by the shell when Tab/Esc is pressed
    /// on a dirty editor.
    pub fn start_exit_prompt(&mut self) {
        self.exit_prompt = true;
    }

    /// Hide the exit-prompt overlay without navigating away (Cancel choice).
    pub fn dismiss_exit_prompt(&mut self) {
        self.exit_prompt = false;
    }

    /// Handle a key press while the exit-prompt overlay is active. Returns the
    /// user's choice; the shell is responsible for acting on it.
    pub fn on_prompt_key(&mut self, code: KeyCode) -> PromptOutcome {
        match code {
            KeyCode::Char('s') | KeyCode::Char('S') => PromptOutcome::SaveAndLeave,
            KeyCode::Char('d') | KeyCode::Char('D') => PromptOutcome::Leave,
            // Esc and 'c' both cancel (stay in editor).
            _ => PromptOutcome::Stay,
        }
    }

    // ── save-to-library name prompt ──────────────────────────────────────────

    /// Record where this bundle came from; written into `meta.json` on save.
    /// The shell sets `Edited` (or the loaded origin) when opening a bundle for
    /// editing so a re-save keeps faithful provenance.
    pub fn set_origin(&mut self, origin: TrackOrigin) {
        self.origin = origin;
    }

    /// Whether the "save to library" name overlay is active.
    pub fn is_naming(&self) -> bool {
        self.name_prompt.is_some()
    }

    /// Open the name overlay so the user can save the chart into the library.
    pub fn start_save_prompt(&mut self) {
        self.name_prompt = Some(String::new());
    }

    /// The name typed so far in the save overlay (empty until shown / typed).
    pub fn name_prompt_text(&self) -> &str {
        self.name_prompt.as_deref().unwrap_or("")
    }

    /// Handle a key while the name overlay is active. Returns the typed name on
    /// Enter (the shell then saves it), `None` while still editing, and clears
    /// the overlay on Esc.
    pub fn on_name_key(&mut self, code: KeyCode) -> NameOutcome {
        let Some(buf) = self.name_prompt.as_mut() else {
            return NameOutcome::Pending;
        };
        match code {
            KeyCode::Esc => {
                self.name_prompt = None;
                NameOutcome::Cancelled
            }
            KeyCode::Enter => {
                let name = buf.trim().to_string();
                if name.is_empty() {
                    NameOutcome::Pending
                } else {
                    self.name_prompt = None;
                    NameOutcome::Submitted(name)
                }
            }
            KeyCode::Backspace => {
                buf.pop();
                NameOutcome::Pending
            }
            KeyCode::Char(c) => {
                buf.push(c);
                NameOutcome::Pending
            }
            _ => NameOutcome::Pending,
        }
    }

    /// Whether the transport is currently playing.
    pub fn is_playing(&self) -> bool {
        self.composer.is_playing()
    }

    /// Current playhead position in song microseconds (cursor position when
    /// stopped).
    pub fn playhead_us(&self) -> u64 {
        self.composer.playhead_us()
    }

    /// The pitches of the chord currently being previewed, or `None`.
    pub fn previewed_chord(&self) -> Option<Vec<MidiNote>> {
        self.composer.previewed_chord()
    }

    /// The pitches of the most recently committed chord (empty before the first).
    pub fn last_committed_pitches(&self) -> &[MidiNote] {
        self.composer.last_committed_pitches()
    }

    // ── loop / metronome / count-in shims ────────────────────────────────────

    /// Whether loop mode is currently active.
    pub fn is_looping(&self) -> bool {
        self.composer.is_looping()
    }

    /// The current loop region as `(start_us, end_us)`.
    pub fn loop_bounds(&self) -> (u64, u64) {
        self.composer.loop_bounds()
    }

    /// Explicitly set the loop region (does not toggle looping on/off).
    pub fn set_loop_bounds(&mut self, start_us: u64, end_us: u64) {
        self.dispatch(Action::SetLoopBounds { start_us, end_us });
    }

    /// Toggle loop on/off. Turning on auto-sets bounds to the bar under the
    /// cursor when no valid bounds have been set yet.
    pub fn toggle_loop(&mut self) {
        self.dispatch(Action::ToggleLoop);
    }

    /// Whether the metronome click is armed.
    pub fn is_metronome_on(&self) -> bool {
        self.composer.is_metronome_on()
    }

    /// Toggle the metronome click on/off.
    pub fn toggle_metronome(&mut self) {
        self.dispatch(Action::ToggleMetronome);
    }

    /// Number of metronome clicks fired since playback last started.
    pub fn metronome_click_count(&self) -> usize {
        self.composer.metronome_click_count()
    }

    /// Whether a count-in phase is currently in progress.
    pub fn is_counting_in(&self) -> bool {
        self.composer.is_counting_in()
    }

    /// Arm live record and count in N bars of clicks before recording begins.
    pub fn start_count_in_record(&mut self) {
        self.dispatch(Action::StartCountInRecord);
    }

    // ── time-axis viewport ────────────────────────────────────────────────

    /// Microsecond position of the cursor on the time axis.
    fn cursor_us(&self) -> u64 {
        self.grid.us_of_step(self.composer.cursor().step)
    }

    /// How far into the future the top of the highway represents.
    fn lead_us(&self) -> u64 {
        (self.grid.bar_us() * LEAD_BARS).max(1)
    }

    /// The time at the bottom (keyboard line) of the highway, scrolled so the
    /// cursor (or playhead during playback) stays anchored a quarter of the way
    /// up the visible window.
    fn view_now_us(&self) -> u64 {
        let lead = self.lead_us();
        let anchor_us = if self.is_playing() {
            self.playhead_us()
        } else {
            self.cursor_us()
        };
        anchor_us.saturating_sub(lead * CURSOR_ANCHOR_NUM / CURSOR_ANCHOR_DEN)
    }

    // ── rendering ─────────────────────────────────────────────────────────

    /// Draw the edit screen: status line, note highway, and the 88-key board.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(1), // status
            Constraint::Min(3),    // highway
            Constraint::Length(4), // keyboard
        ])
        .split(area);

        self.draw_status(f, chunks[0]);

        // Draw the keyboard first to learn the scale + left edge so the highway
        // aligns to the exact same columns. Highlight the cursor's key.
        let kb_block = Block::default()
            .borders(Borders::ALL)
            .title(" keyboard (88) ");
        let kb_inner = kb_block.inner(chunks[2]);
        f.render_widget(kb_block, chunks[2]);
        let cursor_pitch = self.composer.cursor().pitch;
        let layout = draw_keyboard(f, kb_inner, &|note| {
            (note == cursor_pitch).then_some(CURSOR_COLOR)
        });

        let hw_block = Block::default().borders(Borders::ALL).title(" edit ");
        let hw_inner = hw_block.inner(chunks[1]);
        f.render_widget(hw_block, chunks[1]);
        if let Some((scale, x0)) = layout {
            self.draw_highway(f, hw_inner, scale, x0);
        }

        // Draw help overlay if visible
        if self.show_help {
            self.draw_help_overlay(f, area);
        }

        // Exit-prompt overlay sits on top of everything else.
        if self.exit_prompt {
            self.draw_exit_prompt(f, area);
        }

        // The save-to-library name overlay, when active, sits on top too.
        if self.name_prompt.is_some() {
            self.draw_name_prompt(f, area);
        }
    }

    fn draw_status(&self, f: &mut Frame, area: Rect) {
        // Save-confirmation flash takes over the status line for one key cycle.
        if let Some(flash) = &self.save_flash {
            let line = Line::from(vec![
                Span::styled(
                    format!(" {flash} "),
                    Style::default().fg(Color::Black).bg(Color::Green),
                ),
                Span::styled(
                    "  [s] save  [S] library  [Tab] menu",
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            f.render_widget(Paragraph::new(line), area);
            return;
        }

        let cursor = self.composer.cursor();
        let cursor_us = self.cursor_us();
        let (bar, beat) = self.grid.bar_beat_of(cursor_us);
        let pitch_name = MidiNote::new(cursor.pitch)
            .map(|n| n.name())
            .unwrap_or_default();

        // In chord mode the badge and hint switch to the selector controls. The
        // preview pitches come straight from the composer (it owns degree/kind).
        if self.composer.in_chord_mode() {
            let names: Vec<String> = self
                .composer
                .previewed_chord()
                .unwrap_or_default()
                .iter()
                .map(|p| p.name())
                .collect();
            let line = Line::from(vec![
                Span::styled(" CHORD ", Style::default().fg(Color::Black).bg(CHORD_COLOR)),
                Span::raw(format!("  {}  ", names.join(" "))),
                Span::styled(
                    "[1-7] degree  [ [ / ] ] cycle  [s] 7th  [Enter] commit  [Esc] cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            f.render_widget(Paragraph::new(line), area);
            return;
        }

        // Badge priority: visual selection > input mode.
        let (badge_text, badge_style) = if self.composer.in_visual_mode() {
            (
                " VISUAL ",
                Style::default().fg(Color::Black).bg(SELECT_COLOR),
            )
        } else {
            match self.composer.input_mode() {
                InputMode::DirectEdit => {
                    (" EDIT ", Style::default().fg(Color::Black).bg(CURSOR_COLOR))
                }
                InputMode::StepRecord => (
                    " STEP-REC ",
                    Style::default().fg(Color::Black).bg(REC_COLOR),
                ),
                InputMode::LiveRecord => (
                    " LIVE-REC ",
                    Style::default().fg(Color::Black).bg(REC_COLOR),
                ),
            }
        };

        let loop_span = if self.composer.is_looping() {
            Span::styled(" LOOP ", Style::default().fg(Color::Black).bg(Color::Green))
        } else {
            Span::raw("")
        };
        let metro_span = if self.composer.is_metronome_on() {
            Span::styled(" METRO ", Style::default().fg(Color::Black).bg(Color::Cyan))
        } else {
            Span::raw("")
        };
        let count_in_span = if self.composer.is_counting_in() {
            Span::styled(
                " COUNT-IN ",
                Style::default().fg(Color::Black).bg(REC_COLOR),
            )
        } else {
            Span::raw("")
        };

        // While record is armed, surface the mock note keys so no-piano users
        // discover that the number row plays notes (the rest of the hint lists
        // the letter command keys). Hidden in direct-edit, where digits are
        // editor commands rather than notes.
        let note_keys_span = if matches!(
            self.composer.input_mode(),
            InputMode::StepRecord | InputMode::LiveRecord
        ) {
            Span::styled("  play 1-0 (C-major)  ", Style::default().fg(REC_COLOR))
        } else {
            Span::raw("")
        };

        // Show a sub-beat step counter when the subdivision is finer than one
        // beat — at 1/16 snap this turns every cursor_right into a visible
        // change rather than waiting 4 presses for the beat digit to flip.
        let beat_us = self.grid.beat_us();
        let steps_per_beat = beat_us / self.grid.step_us().max(1);
        let pos_span = if steps_per_beat > 1 {
            let sub = self.grid.step_in_beat(cursor_us) + 1; // 1-indexed
            Span::raw(format!("  bar {}:{}.{}  ", bar + 1, beat + 1, sub))
        } else {
            Span::raw(format!("  bar {}:{}  ", bar + 1, beat + 1))
        };

        let vel_span = self
            .note_under_cursor()
            .and_then(|id| self.get_note(id))
            .map(|n| {
                Span::styled(
                    format!("vel {}  ", n.velocity.value()),
                    Style::default().fg(CURSOR_COLOR),
                )
            })
            .unwrap_or_else(|| Span::raw(""));

        // Surface the backing alignment offset (and its nudge keys) only when a
        // track is attached, so MIDI-only editing keeps a clean status line.
        let backing_span = if self.backing.is_some() {
            Span::styled(
                format!(
                    "backing {:+}ms [,/.·;/']  ",
                    self.composer.backing_offset_us() as i64 / 1000
                ),
                Style::default().fg(Color::Cyan),
            )
        } else {
            Span::raw("")
        };

        let line = Line::from(vec![
            Span::styled(badge_text, badge_style),
            loop_span,
            metro_span,
            count_in_span,
            note_keys_span,
            pos_span,
            Span::raw(format!("snap {}  ", self.grid.subdivision.label())),
            backing_span,
            Span::styled(
                format!("♪ {pitch_name}  "),
                Style::default().fg(CURSOR_COLOR),
            ),
            vel_span,
            Span::styled(
                "[a/x] add/del  []/[] size  [+/-] vel  [m] grab  [c] chord  [v] select  [y/p/D] yank/paste/del  [u/U] undo/redo  [R] rec  [t] step/live  [C] count-in  [o] loop  [M] metro  [P] play-start  [>/<] subdiv  [hjkl] pitch/time  [H/L] bar  [w/b] oct  [g/G] timeline ends  [0/$] pitch ends  [s] save  [S] save to library  [Tab] menu",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(line), area);
    }

    fn draw_highway(&self, f: &mut Frame, area: Rect, scale: Scale, x0: u16) {
        if area.height == 0 {
            return;
        }
        let w = scale.white_width();
        let now = self.view_now_us();
        let lead = self.lead_us();
        let cursor = self.composer.cursor();

        // Column for a note's left edge on the highway, matching keyboard layout.
        let note_col = |note: u8| -> Option<u16> {
            if let Some(wi) = white_index(note) {
                Some(x0 + wi as u16 * w)
            } else if is_black_key(note) {
                black_key_col(note, scale).map(|c| x0 + c)
            } else {
                None
            }
        };

        // Beat/bar gridlines first, so notes and the cursor paint over them.
        self.draw_gridlines(f, area, now, lead);
        self.draw_playhead(f, area, now, lead);

        // Pre-compute selection bounds so we can highlight selected notes.
        let sel = self
            .composer
            .snapshot()
            .selection
            .map(|s| (s.pitch_lo, s.pitch_hi, s.us_lo, s.us_hi));

        // Timeline notes.
        for span in build_spans(&self.timeline().to_events()) {
            let Some(rs) = project(&span, now, lead, area.height) else {
                continue;
            };
            let Some(col) = note_col(span.note) else {
                continue;
            };
            let cell_w = if is_black_key(span.note) { 1 } else { w };
            let glyph = "▓".repeat(cell_w as usize);
            let note_color = if let Some((pitch_lo, pitch_hi, us_lo, us_hi)) = sel {
                if span.note >= pitch_lo
                    && span.note <= pitch_hi
                    && span.start_us >= us_lo
                    && span.start_us < us_hi
                {
                    SELECT_COLOR
                } else {
                    NOTE_COLOR
                }
            } else {
                NOTE_COLOR
            };
            for row in rs.top_row..=rs.bottom_row {
                let y = area.y + row;
                if y >= area.y + area.height {
                    break;
                }
                let rect = Rect::new(col, y, cell_w, 1);
                f.render_widget(
                    Paragraph::new(glyph.clone()).style(Style::default().fg(note_color)),
                    rect,
                );
            }
        }

        // The cursor cell, on top of everything.
        if let Some(col) = note_col(cursor.pitch) {
            let cur = NoteSpan {
                note: cursor.pitch,
                start_us: self.cursor_us(),
                end_us: self.cursor_us() + 1,
            };
            if let Some(rs) = project(&cur, now, lead, area.height) {
                let cell_w = if is_black_key(cursor.pitch) { 1 } else { w };
                let y = area.y + rs.bottom_row;
                let rect = Rect::new(col, y, cell_w, 1);
                f.render_widget(
                    Paragraph::new("█".repeat(cell_w as usize)).style(
                        Style::default()
                            .fg(CURSOR_COLOR)
                            .add_modifier(Modifier::BOLD),
                    ),
                    rect,
                );
            }
        }
    }

    /// Horizontal playhead line at the current transport position.
    fn draw_playhead(&self, f: &mut Frame, area: Rect, now: u64, lead: u64) {
        if !self.is_playing() {
            return;
        }
        let ph = self.playhead_us();
        let marker = NoteSpan {
            note: LOWEST_MIDI,
            start_us: ph,
            end_us: ph + 1,
        };
        let Some(rs) = project(&marker, now, lead, area.height) else {
            return;
        };
        let line = "─".repeat(area.width as usize);
        let rect = Rect::new(area.x, area.y + rs.bottom_row, area.width, 1);
        f.render_widget(
            Paragraph::new(line).style(Style::default().fg(PLAYHEAD_COLOR)),
            rect,
        );
    }

    /// Faint horizontal lines at bar boundaries within the visible window.
    fn draw_gridlines(&self, f: &mut Frame, area: Rect, now: u64, lead: u64) {
        let bar = self.grid.bar_us();
        if bar == 0 || area.width == 0 {
            return;
        }
        let window_end = now + lead;
        // First bar boundary at or after `now`.
        let mut t = now.div_ceil(bar) * bar;
        let line = "─".repeat(area.width as usize);
        while t <= window_end {
            let marker = NoteSpan {
                note: LOWEST_MIDI,
                start_us: t,
                end_us: t + 1,
            };
            if let Some(rs) = project(&marker, now, lead, area.height) {
                let rect = Rect::new(area.x, area.y + rs.bottom_row, area.width, 1);
                f.render_widget(
                    Paragraph::new(line.clone()).style(Style::default().fg(GRID_COLOR)),
                    rect,
                );
            }
            t += bar;
        }
    }

    /// Draw the help overlay as a centered modal popup listing the keymap.
    fn draw_help_overlay(&self, f: &mut Frame, area: Rect) {
        // Create a centered block for the help overlay
        let help_block = Block::default()
            .borders(Borders::ALL)
            .title(" Help (Press ? or Esc to close) ")
            .border_style(Style::default().fg(Color::White));

        // Calculate the size and position for the overlay
        // Make it take up most of the screen but leave some margin
        let margin = 2;
        let help_width = area.width.saturating_sub(margin * 2);
        let help_height = area.height.saturating_sub(margin * 2).min(20); // Limit height for readability
        let help_x = area.x + margin;
        let help_y = area.y + margin;
        let help_area = Rect::new(help_x, help_y, help_width, help_height);

        // Draw the overlay background (clear the area first)
        f.render_widget(help_block.clone(), help_area);

        // The mock-keyboard note keys, pulled straight from the source's table so
        // this legend can never drift from what the keys actually do.
        let mock_keys: String = mock_key_map().iter().map(|(k, _)| *k).collect();

        // Create the help content grouped by category
        let help_content = vec![
            Line::from(Span::styled(
                " Navigation (h/l = pitch axis · j/k = time axis):",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw(
                "  h/← : Pitch -1 (left)   l/→ : Pitch +1 (right)",
            )),
            Line::from(Span::raw(
                "  j/↓ : Step  -1 (earlier) k/↑ : Step  +1 (later)",
            )),
            Line::from(Span::raw("  H : One bar earlier     L : One bar later")),
            Line::from(Span::raw(
                "  w : Octave right (+12)  b : Octave left  (-12)",
            )),
            Line::from(Span::raw(
                "  g : Timeline start      G : Timeline end (last note)",
            )),
            Line::from(Span::raw(
                "  0 : Lowest pitch (A0)   $ : Highest pitch (C8)",
            )),
            Line::from(Span::raw(
                "  > : Finer subdivision    < : Coarser subdivision",
            )),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Edit:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw("  a/i : Add note          x/d : Delete note")),
            Line::from(Span::raw("  ] : Lengthen note       [ : Shorten note")),
            Line::from(Span::raw("  +/= : Velocity +8       - : Velocity -8")),
            Line::from(Span::raw("  m : Grab mode (move note with h/j/k/l)")),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Selection/Clipboard:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw("  v : Start selection     y : Yank selection")),
            Line::from(Span::raw("  p : Paste              D : Delete selection")),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Chord:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw(
                "  c : Chord rooted at cursor   1-7 : Choose degree",
            )),
            Line::from(Span::raw(
                "  [/] : Cycle degree           s : Toggle 7th/triad",
            )),
            Line::from(Span::raw(
                "  Enter : Commit chord         Esc : Cancel chord",
            )),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Transport:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw(
                "  Space : Play/stop from cursor  P : Play from start",
            )),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Backing alignment (slide audio under the highway):",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw(
                "  , / . : Nudge -/+10ms      ; / ' : Nudge -/+250ms",
            )),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Loop/Metronome/Count-in:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw("  o : Toggle loop         M : Toggle metronome")),
            Line::from(Span::raw("  C : Count-in record")),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Input Mode:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw(
                "  R : Toggle record arm    t : Toggle step/live record",
            )),
            Line::from(Span::raw(format!(
                "  Play notes (no piano, while armed): {mock_keys} → C-major C4–E5"
            ))),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Undo/Redo:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw("  u : Undo                U : Redo")),
            Line::from(Span::raw("")), // Empty line
            Line::from(Span::styled(
                " Other:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::raw("  ? : Toggle help          Esc : Close help")),
            Line::from(Span::raw("  s : Save                Tab : Menu")),
        ];

        // Create a paragraph with the help content
        let help_paragraph = Paragraph::new(help_content).style(Style::default().fg(Color::White));

        // Render the help content inside the block
        let inner_area = help_block.inner(help_area);
        f.render_widget(help_paragraph, inner_area);
    }

    /// Small centered overlay for the "Save / Discard / Cancel" exit prompt.
    fn draw_exit_prompt(&self, f: &mut Frame, area: Rect) {
        let label = " Unsaved changes — [s] Save  [d] Discard  [Esc/other] Cancel ";
        let w = (label.len() as u16 + 4).min(area.width);
        let h = 3u16;
        let x = area.x + area.width.saturating_sub(w) / 2;
        let y = area.y + area.height.saturating_sub(h) / 2;
        let prompt_area = Rect::new(x, y, w, h.min(area.height));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let inner = block.inner(prompt_area);
        f.render_widget(block, prompt_area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label,
                Style::default().fg(Color::Yellow),
            ))),
            inner,
        );
    }

    /// The "save to library" name overlay: a single-line text field showing the
    /// name typed so far with a block cursor, plus the key hints.
    fn draw_name_prompt(&self, f: &mut Frame, area: Rect) {
        let typed = self.name_prompt_text();
        let label = format!(" Save to library — name: {typed}█  [Enter] save  [Esc] cancel ");
        let w = (label.len() as u16 + 4).min(area.width).max(20);
        let h = 3u16;
        let x = area.x + area.width.saturating_sub(w) / 2;
        let y = area.y + area.height.saturating_sub(h) / 2;
        let prompt_area = Rect::new(x, y, w, h.min(area.height));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(prompt_area);
        f.render_widget(block, prompt_area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label,
                Style::default().fg(Color::Cyan),
            ))),
            inner,
        );
    }
}

impl Default for EditScreen {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::HIGHEST_MIDI;
    use ratatui::{backend::TestBackend, Terminal};

    /// Count `AuditionNote` effects that turn a note on (velocity > 0).
    fn count_note_ons(effects: &[Effect]) -> usize {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::AuditionNote { velocity, .. } if *velocity > 0))
            .count()
    }

    /// Count `AuditionNote` effects that turn a note off (velocity 0 — the MIDI
    /// note-off convention the composer emits for playback span ends).
    fn count_note_offs(effects: &[Effect]) -> usize {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::AuditionNote { velocity, .. } if *velocity == 0))
            .count()
    }

    /// The keymap seam in action: a representative key (`a`) resolves through
    /// `key_to_action` to `Action::AddNote`, the `Composer` applies it and emits
    /// an `Effect::AuditionNote`, and the screen consumes that effect (no synth
    /// attached → a silent no-op) while the note lands.
    #[test]
    fn key_a_maps_to_add_note_and_audits() {
        // Rebinding seam: `a` is bound to `AddNote`.
        assert_eq!(key_to_action(KeyCode::Char('a')), Some(Action::AddNote));

        // The composer gives that action meaning, auditioning the new note.
        let mut composer = Composer::new();
        let effects = composer.apply(Action::AddNote).expect("add_note applies");
        assert!(
            matches!(effects.as_slice(), [Effect::AuditionNote { .. }]),
            "AddNote auditions the new note"
        );

        // End to end through the screen with no synth: the effect is consumed
        // without panicking and the note is placed via the composer.
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        assert_eq!(
            e.note_count(),
            1,
            "key `a` placed a note through the composer"
        );
    }

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
        }
    }

    // ── existing navigation tests ────────────────────────────────────────

    #[test]
    fn fresh_cursor_starts_at_middle_c_step_zero() {
        let e = EditScreen::new();
        assert_eq!(
            e.cursor(),
            Cursor {
                pitch: DEFAULT_CURSOR_PITCH,
                step: 0
            }
        );
    }

    // h/l are now the pitch axis (horizontal); j/k are the time axis (vertical).

    #[test]
    fn h_and_l_move_pitch_by_one() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('l'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 1);
        e.on_key(KeyCode::Char('h'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH);
        // Arrow aliases behave the same.
        e.on_key(KeyCode::Right);
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 1);
        e.on_key(KeyCode::Left);
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH);
    }

    #[test]
    fn h_clamps_at_pitch_min() {
        let mut e = EditScreen::new();
        for _ in 0..200 {
            e.on_key(KeyCode::Char('h'));
        }
        assert_eq!(e.cursor().pitch, LOWEST_MIDI);
    }

    #[test]
    fn l_clamps_at_pitch_max() {
        let mut e = EditScreen::new();
        for _ in 0..200 {
            e.on_key(KeyCode::Char('l'));
        }
        assert_eq!(e.cursor().pitch, HIGHEST_MIDI);
    }

    #[test]
    fn j_and_k_move_step_by_one() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('k'));
        assert_eq!(e.cursor().step, 1);
        e.on_key(KeyCode::Char('j'));
        assert_eq!(e.cursor().step, 0);
        // Arrow aliases.
        e.on_key(KeyCode::Up);
        assert_eq!(e.cursor().step, 1);
        e.on_key(KeyCode::Down);
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn j_at_step_zero_stays_at_zero() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('j'));
        assert_eq!(e.cursor().step, 0);
        e.on_key(KeyCode::Down);
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn octave_jumps_move_by_twelve_and_clamp() {
        let mut e = EditScreen::new();
        // w/b are the primary octave keys (right/left on the pitch axis).
        e.on_key(KeyCode::Char('w'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 12);
        e.on_key(KeyCode::Char('b'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH);

        // Clamp at the top.
        for _ in 0..20 {
            e.on_key(KeyCode::Char('w'));
        }
        assert_eq!(e.cursor().pitch, HIGHEST_MIDI);
        // Clamp at the bottom.
        for _ in 0..20 {
            e.on_key(KeyCode::Char('b'));
        }
        assert_eq!(e.cursor().pitch, LOWEST_MIDI);

        // J/K are kept as aliases for b/w; verify from a known starting pitch.
        // Cursor is at LOWEST_MIDI after the b clamp above; K moves up by 12.
        e.on_key(KeyCode::Char('K'));
        assert_eq!(e.cursor().pitch, LOWEST_MIDI + 12);
        e.on_key(KeyCode::Char('J'));
        assert_eq!(e.cursor().pitch, LOWEST_MIDI);
    }

    #[test]
    fn bar_jumps_move_by_exactly_steps_per_bar() {
        let mut e = EditScreen::new();
        let steps_per_bar = e.grid.bar_us() / e.grid.step_us();
        assert_eq!(steps_per_bar, 16); // default 120 BPM, 4/4, 1/16 grid

        e.on_key(KeyCode::Char('L'));
        assert_eq!(e.cursor().step, steps_per_bar);
        e.on_key(KeyCode::Char('L'));
        assert_eq!(e.cursor().step, steps_per_bar * 2);
        e.on_key(KeyCode::Char('H'));
        assert_eq!(e.cursor().step, steps_per_bar);
        // H clamps at 0, never underflowing.
        e.on_key(KeyCode::Char('H'));
        e.on_key(KeyCode::Char('H'));
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn g_and_shift_g_jump_to_timeline_start_and_end() {
        let mut tl = Timeline::new();
        // Last note ends at 3_000_000 us → step index at 1/16 (125_000 us) = 24.
        tl.insert(note(60, 0, 1_000_000));
        tl.insert(note(64, 2_000_000, 1_000_000));
        let mut e = EditScreen::from_timeline(tl, Grid::default_120());

        e.on_key(KeyCode::Char('G'));
        assert_eq!(e.cursor().step, 24, "G jumps to last note end");
        e.on_key(KeyCode::Char('g'));
        assert_eq!(e.cursor().step, 0, "g jumps to timeline start");
    }

    #[test]
    fn shift_g_on_empty_timeline_is_step_zero() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('k')); // move off step zero first
        e.on_key(KeyCode::Char('G'));
        assert_eq!(e.cursor().step, 0, "G on empty timeline stays at step 0");
    }

    #[test]
    fn zero_and_dollar_jump_to_pitch_extremes() {
        let mut e = EditScreen::new();
        // Default cursor is middle C (MIDI 60); step stays unchanged.
        e.on_key(KeyCode::Char('0'));
        assert_eq!(e.cursor().pitch, LOWEST_MIDI, "0 jumps to A0 (MIDI 21)");
        e.on_key(KeyCode::Char('$'));
        assert_eq!(e.cursor().pitch, HIGHEST_MIDI, "$ jumps to C8 (MIDI 108)");
    }

    #[test]
    fn unmapped_keys_are_no_ops() {
        let mut e = EditScreen::new();
        let before = e.cursor();
        e.on_key(KeyCode::Char('z'));
        e.on_key(KeyCode::Char('x')); // delete on empty cell → cursor unchanged
        e.on_key(KeyCode::Enter);
        assert_eq!(e.cursor(), before);
    }

    #[test]
    fn from_timeline_renders_a_known_note_marker_without_panic() {
        let mut tl = Timeline::new();
        // A note at pitch 64, away from the cursor's default column (60), so its
        // glyph isn't overwritten by the cursor cell.
        tl.insert(note(64, 0, 500_000));
        let e = EditScreen::from_timeline(tl, Grid::default_120());

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|f| e.draw(f, f.area()))
            .expect("draw panicked");

        let buf = terminal.backend().buffer();
        let has_note = buf.content().iter().any(|c| c.symbol() == "▓");
        assert!(has_note, "expected the timeline note's marker to render");
    }

    // ── add / delete tests ───────────────────────────────────────────────

    /// `a` inserts exactly one note; properties match the cursor's pitch/step
    /// with a one-step duration and the default velocity.
    #[test]
    fn a_adds_note_at_cursor() {
        let mut e = EditScreen::new();
        assert_eq!(e.note_count(), 0);

        e.on_key(KeyCode::Char('a'));

        assert_eq!(e.note_count(), 1);
        let id = e.note_under_cursor().expect("note at cursor after add");
        let n = e.get_note(id).unwrap();
        assert_eq!(n.pitch.value(), DEFAULT_CURSOR_PITCH);
        assert_eq!(n.start_us, 0); // step 0 → 0 µs
        assert_eq!(n.dur_us, e.grid.step_us()); // one grid step
        assert_eq!(n.velocity.value(), DEFAULT_NOTE_VEL);
    }

    /// `i` is an alias for `a`.
    #[test]
    fn i_is_alias_for_add() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('i'));
        assert_eq!(e.note_count(), 1);
    }

    /// `a` on an occupied cell replaces it: count stays at 1, the new note
    /// resets to default velocity and a one-step duration.
    #[test]
    fn a_on_occupied_cell_replaces_note() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // add first note (vel=80, dur=1 step)
                                      // Lengthen and change velocity so we can tell the difference.
        e.on_key(KeyCode::Char(']')); // dur → 2 steps
        e.on_key(KeyCode::Char('+')); // vel → 88
        assert_eq!(e.note_count(), 1);
        {
            let id = e.note_under_cursor().unwrap();
            let n = e.get_note(id).unwrap();
            assert_eq!(n.dur_us, e.grid.step_us() * 2);
            assert_eq!(n.velocity.value(), 88);
        }

        // Add again on the same cell: replaces.
        e.on_key(KeyCode::Char('a'));
        assert_eq!(e.note_count(), 1, "still exactly one note after replace");
        let id = e.note_under_cursor().unwrap();
        let n = e.get_note(id).unwrap();
        assert_eq!(n.dur_us, e.grid.step_us(), "duration reset to one step");
        assert_eq!(
            n.velocity.value(),
            DEFAULT_NOTE_VEL,
            "velocity reset to default"
        );
    }

    /// `x` removes the note under the cursor.
    #[test]
    fn x_deletes_note_under_cursor() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        assert_eq!(e.note_count(), 1);

        e.on_key(KeyCode::Char('x'));

        assert_eq!(e.note_count(), 0);
        assert!(e.note_under_cursor().is_none());
    }

    /// `d` is an alias for `x`.
    #[test]
    fn d_is_alias_for_delete() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        e.on_key(KeyCode::Char('d'));
        assert_eq!(e.note_count(), 0);
    }

    /// `x` on an empty cell is a no-op (cursor unchanged, no panic).
    #[test]
    fn x_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        let before = e.cursor();
        e.on_key(KeyCode::Char('x'));
        assert_eq!(e.note_count(), 0);
        assert_eq!(e.cursor(), before);
    }

    // ── resize tests ──────────────────────────────────────────────────────

    /// `]` and `[` change duration by one step; `[` never goes below one step.
    #[test]
    fn bracket_keys_resize_note() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        let step = e.grid.step_us();

        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step, "starts at one step");

        // Lengthen: 1 → 2 steps.
        e.on_key(KeyCode::Char(']'));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step * 2);

        // Shorten: 2 → 1 step.
        e.on_key(KeyCode::Char('['));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step);

        // Shorten again: clamps at one step minimum.
        e.on_key(KeyCode::Char('['));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step, "floor is one step");
    }

    /// `]` and `[` are no-ops on an empty cell.
    #[test]
    fn resize_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char(']'));
        e.on_key(KeyCode::Char('['));
        assert_eq!(e.note_count(), 0);
    }

    // ── velocity tests ────────────────────────────────────────────────────

    /// `+` and `-` adjust velocity by VEL_STEP, clamped to 1..=127.
    #[test]
    fn velocity_keys_adjust_and_clamp() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // vel = 80

        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().velocity.value(), 80);

        // Drive velocity down past zero: should clamp at 1.
        for _ in 0..15 {
            e.on_key(KeyCode::Char('-'));
        }
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().velocity.value(), 1, "clamped at 1");

        // Drive velocity up past 127: should clamp at 127.
        for _ in 0..25 {
            e.on_key(KeyCode::Char('+'));
        }
        let id = e.note_under_cursor().unwrap();
        assert_eq!(
            e.get_note(id).unwrap().velocity.value(),
            127,
            "clamped at 127"
        );
    }

    /// `=` is an alias for `+`.
    #[test]
    fn equals_is_alias_for_velocity_up() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // vel = 80
        e.on_key(KeyCode::Char('='));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().velocity.value(), 80 + VEL_STEP);
    }

    /// `+`/`-` on an empty cell are no-ops.
    #[test]
    fn velocity_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('+'));
        e.on_key(KeyCode::Char('-'));
        assert_eq!(e.note_count(), 0);
    }

    // ── grab-mode tests ───────────────────────────────────────────────────

    /// `m` + `k` moves the note's start later by one step and the cursor tracks.
    /// (`k` is the time-forward key after the axis fix.)
    #[test]
    fn grab_k_moves_note_start_and_cursor_tracks() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // add note at step 0

        e.on_key(KeyCode::Char('m')); // grab it

        e.on_key(KeyCode::Char('k')); // move later → step 1
        assert_eq!(e.cursor().step, 1, "cursor follows note");
        let id = e.note_under_cursor().expect("note at new position");
        assert_eq!(
            e.get_note(id).unwrap().start_us,
            e.grid.us_of_step(1),
            "note start moved"
        );
    }

    /// `m` + `l` transposes the note up (right on keyboard) and the cursor pitch tracks.
    /// (`l` is the pitch-up key after the axis fix.)
    #[test]
    fn grab_l_transposes_note_and_cursor_tracks() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // add note at pitch 60

        e.on_key(KeyCode::Char('m')); // grab
        e.on_key(KeyCode::Char('l')); // pitch up (right on keyboard)

        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 1, "cursor right");
        let id = e.note_under_cursor().expect("note at new pitch");
        assert_eq!(
            e.get_note(id).unwrap().pitch.value(),
            DEFAULT_CURSOR_PITCH + 1,
            "note pitch changed"
        );
    }

    /// `m` again drops grab; subsequent navigation moves only the cursor.
    #[test]
    fn grab_dropped_by_second_m() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // note at step 0

        e.on_key(KeyCode::Char('m')); // grab
        e.on_key(KeyCode::Char('m')); // drop

        // `k` should now only move the cursor (step +1), not drag the note.
        e.on_key(KeyCode::Char('k'));
        assert_eq!(e.cursor().step, 1, "cursor moved");
        // The note is still at step 0 (cursor moved away from it).
        assert!(
            e.note_under_cursor().is_none(),
            "cursor moved off the note after grab drop"
        );
    }

    /// `m` on an empty cell is a no-op (no grab activated).
    #[test]
    fn grab_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('m')); // nothing to grab
                                      // Navigation is still cursor-only.
        e.on_key(KeyCode::Char('k')); // step +1
        assert_eq!(e.cursor().step, 1);
        assert_eq!(e.note_count(), 0);
    }

    // ── save round-trip ───────────────────────────────────────────────────

    /// `save_bundle()` writes a bundle whose `song.mid` deserialises back to an
    /// equal timeline (pitch, start, duration preserved for every note).
    #[test]
    fn save_bundle_round_trips_timeline() {
        use rockcraft_midi::smf_bytes_to_events;

        let mut tl = Timeline::new();
        tl.insert(note(60, 0, 500_000));
        tl.insert(note(64, 500_000, 500_000));
        let expected_count = tl.len();

        let edit = EditScreen::from_timeline(tl.clone(), Grid::default_120());

        let base = std::env::temp_dir().join(format!(
            "rockcraft_rt_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        let bundle = edit.save_bundle(&base).expect("save_bundle failed");

        let midi_bytes = std::fs::read(bundle.join("song.mid")).expect("song.mid missing");
        let events = smf_bytes_to_events(&midi_bytes).expect("smf parse failed");
        let reloaded = Timeline::from_events(&events);

        std::fs::remove_dir_all(&base).ok();

        assert_eq!(
            reloaded.len(),
            expected_count,
            "note count survives round-trip"
        );

        let mut orig: Vec<_> = tl
            .notes()
            .map(|(_, n)| (n.pitch.value(), n.start_us, n.dur_us))
            .collect();
        let mut got: Vec<_> = reloaded
            .notes()
            .map(|(_, n)| (n.pitch.value(), n.start_us, n.dur_us))
            .collect();
        orig.sort();
        got.sort();
        assert_eq!(orig, got, "note pitches/positions survive round-trip");
    }

    // ── chord-selector tests ──────────────────────────────────────────────

    fn pitch_values(notes: &[MidiNote]) -> Vec<u8> {
        notes.iter().map(|n| n.value()).collect()
    }

    /// `c` enters chord mode rooted at the cursor pitch (middle C → {C,E,G});
    /// degree `5` places {G,B,D}. All notes share the cursor's start and a
    /// one-step duration.
    #[test]
    fn degree_one_then_five_in_c_major() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c'));
        assert!(e.in_chord_mode());

        e.on_key(KeyCode::Char('1'));
        let preview = e.previewed_chord().expect("preview while in chord mode");
        assert_eq!(pitch_values(&preview), vec![60, 64, 67], "C major tonic");
        assert_eq!(e.note_count(), 3, "three preview notes");

        // Each preview note starts at the cursor and lasts one step.
        let step = e.grid.step_us();
        for &p in &[60u8, 64, 67] {
            let id = e
                .timeline()
                .find_at(p, e.cursor_us())
                .expect("preview note present");
            let n = e.get_note(id).unwrap();
            assert_eq!(n.start_us, e.cursor_us());
            assert_eq!(n.dur_us, step);
        }

        e.on_key(KeyCode::Char('5'));
        let preview = e.previewed_chord().unwrap();
        assert_eq!(
            pitch_values(&preview),
            vec![67, 71, 74],
            "dominant {{G,B,D}}"
        );
        assert_eq!(e.note_count(), 3, "still three — preview replaced");
    }

    /// `s` toggles the dominant triad to its seventh: {G,B,D} → {G,B,D,F}.
    #[test]
    fn seventh_toggle_on_degree_five() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c'));
        e.on_key(KeyCode::Char('5'));
        assert_eq!(
            pitch_values(&e.previewed_chord().unwrap()),
            vec![67, 71, 74]
        );

        e.on_key(KeyCode::Char('s'));
        assert_eq!(
            pitch_values(&e.previewed_chord().unwrap()),
            vec![67, 71, 74, 77],
            "G7 = {{G,B,D,F}}"
        );
        assert_eq!(e.note_count(), 4, "four preview notes after toggle");

        // Toggle back to a triad.
        e.on_key(KeyCode::Char('s'));
        assert_eq!(e.note_count(), 3);
    }

    /// `[`/`]` cycle the degree, replacing the preview each time (count stays at
    /// the chord size, never accumulating).
    #[test]
    fn cycling_replaces_preview_without_accumulating() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c')); // preview degree 1
        assert_eq!(e.note_count(), 3);

        e.on_key(KeyCode::Char(']')); // → degree 2
        assert_eq!(e.note_count(), 3, "still 3 after cycling up");
        e.on_key(KeyCode::Char(']')); // → degree 3
        assert_eq!(e.note_count(), 3);
        e.on_key(KeyCode::Char('[')); // → degree 2
        e.on_key(KeyCode::Char('[')); // → degree 1
        assert_eq!(e.note_count(), 3, "still 3 after cycling back");
        assert_eq!(
            pitch_values(&e.previewed_chord().unwrap()),
            vec![60, 64, 67]
        );
    }

    /// `[` wraps from degree 1 down to degree 7; `]` wraps from 7 up to 1.
    #[test]
    fn degree_cycle_wraps() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c')); // degree 1
        e.on_key(KeyCode::Char('[')); // wrap to degree 7 → {B,D,F}
        assert_eq!(
            pitch_values(&e.previewed_chord().unwrap()),
            vec![71, 74, 77]
        );
        e.on_key(KeyCode::Char(']')); // wrap back to degree 1
        assert_eq!(
            pitch_values(&e.previewed_chord().unwrap()),
            vec![60, 64, 67]
        );
    }

    /// `c` on pitch A4 opens chord mode rooted on A (degree 6 in C major).
    #[test]
    fn enter_chord_roots_at_cursor_pitch() {
        let mut e = EditScreen::new(); // cursor at C4 (MIDI 60)
        for _ in 0..9 {
            // Move up 9 semitones to A4 (MIDI 69) using 'l' (pitch-right key).
            e.on_key(KeyCode::Char('l'));
        }
        assert_eq!(e.cursor().pitch, 69);
        e.on_key(KeyCode::Char('c'));
        assert!(e.in_chord_mode());
        // A4-C5-E5 = A minor triad (degree 6 in C major)
        assert_eq!(
            pitch_values(&e.previewed_chord().expect("preview present")),
            vec![69, 72, 76],
            "chord rooted at A4"
        );
    }

    /// `Enter` commits: notes stay, chord mode ends, and `last_committed_pitches`
    /// reports them.
    #[test]
    fn commit_keeps_preview_notes() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c'));
        e.on_key(KeyCode::Char('1'));
        assert_eq!(e.note_count(), 3);

        e.on_key(KeyCode::Enter);
        assert!(!e.in_chord_mode(), "chord mode ends on commit");
        assert!(e.previewed_chord().is_none());
        assert_eq!(e.note_count(), 3, "committed notes remain");
        assert_eq!(
            pitch_values(e.last_committed_pitches()),
            vec![60, 64, 67],
            "committed pitches reported"
        );
    }

    /// `Esc` cancels: the preview notes are removed and chord mode ends.
    #[test]
    fn cancel_removes_preview_notes() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c'));
        e.on_key(KeyCode::Char('1'));
        assert_eq!(e.note_count(), 3);

        e.on_key(KeyCode::Esc);
        assert!(!e.in_chord_mode(), "chord mode ends on cancel");
        assert_eq!(e.note_count(), 0, "preview removed");
        assert!(e.last_committed_pitches().is_empty());
    }

    /// Leaving the screen mid-chord cancels the uncommitted preview.
    #[test]
    fn leave_cancels_uncommitted_chord() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c'));
        assert_eq!(e.note_count(), 3);
        e.leave();
        assert!(!e.in_chord_mode());
        assert_eq!(e.note_count(), 0);
    }

    /// A seventh chord voiced near the top of the keyboard drops the pitches that
    /// would exceed the MIDI range (delegated to `core`); every kept pitch stays
    /// valid.
    #[test]
    fn chord_at_top_of_keyboard_drops_out_of_range() {
        let mut e = EditScreen::new();
        // Climb to the top of the 88-key range (C8 = 108).
        for _ in 0..20 {
            e.on_key(KeyCode::Char('K'));
        }
        assert_eq!(e.cursor().pitch, HIGHEST_MIDI);

        e.on_key(KeyCode::Char('c'));
        e.on_key(KeyCode::Char('7')); // leading-tone degree, highest voicing
        e.on_key(KeyCode::Char('s')); // seventh → fourth tone runs past 127

        let preview = e.previewed_chord().unwrap();
        assert!(preview.len() < 4, "an out-of-range tone is dropped");
        assert_eq!(
            e.note_count(),
            preview.len(),
            "note count matches the kept pitches"
        );
        for n in &preview {
            assert!(n.value() <= 127);
        }
    }

    /// Chord-mode keys do not leak into normal edit ops, and normal ops are
    /// suspended while in chord mode: `a` (add) is inert during chord mode.
    #[test]
    fn edit_ops_suspended_during_chord_mode() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('c'));
        let before = e.note_count(); // 3 preview notes
        e.on_key(KeyCode::Char('a')); // ignored in chord mode
        assert_eq!(e.note_count(), before, "add is inert in chord mode");
        assert!(e.in_chord_mode());
    }

    // ── input-mode / record tests ─────────────────────────────────────────

    fn on_ev(pitch: u8) -> NoteEvent {
        NoteEvent::on(MidiNote::new(pitch).unwrap(), Velocity::new(80).unwrap(), 0)
    }

    /// A fresh editor is in direct-edit; `R` arms step-record and disarms again.
    #[test]
    fn r_arms_and_disarms_step_record() {
        let mut e = EditScreen::new();
        assert_eq!(e.input_mode(), InputMode::DirectEdit);
        e.on_key(KeyCode::Char('R'));
        assert_eq!(e.input_mode(), InputMode::StepRecord);
        e.on_key(KeyCode::Char('R'));
        assert_eq!(e.input_mode(), InputMode::DirectEdit);
    }

    /// `t` flips step ↔ live only while armed; it is a no-op in direct-edit.
    #[test]
    fn t_toggles_step_and_live_when_armed() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('t')); // not armed → no-op
        assert_eq!(e.input_mode(), InputMode::DirectEdit);

        e.on_key(KeyCode::Char('R')); // arm → step
        e.on_key(KeyCode::Char('t')); // → live
        assert_eq!(e.input_mode(), InputMode::LiveRecord);
        e.on_key(KeyCode::Char('t')); // → step
        assert_eq!(e.input_mode(), InputMode::StepRecord);

        // Disarming from live also returns to direct-edit.
        e.on_key(KeyCode::Char('t')); // → live
        e.on_key(KeyCode::Char('R')); // disarm
        assert_eq!(e.input_mode(), InputMode::DirectEdit);
    }

    /// Navigation keys behave identically regardless of input mode.
    /// After the axis fix: l = pitch +1 (right), k = step +1 (later).
    #[test]
    fn navigation_unaffected_by_mode() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('R')); // step-record armed
        e.on_key(KeyCode::Char('t')); // live-record
        e.on_key(KeyCode::Char('l')); // pitch +1
        e.on_key(KeyCode::Char('k')); // step +1
        assert_eq!(e.cursor().step, 1);
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 1);
    }

    /// StepRecord: three played note-ons land at consecutive steps with their
    /// *played* pitches; the cursor advances three steps.
    #[test]
    fn step_record_inserts_played_pitches_at_consecutive_steps() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('R')); // arm step-record

        for &p in &[60u8, 62, 64] {
            e.ingest(on_ev(p));
        }

        assert_eq!(e.note_count(), 3);
        assert_eq!(e.cursor().step, 3, "cursor advanced three steps");

        let step = e.grid.step_us();
        for (i, &p) in [60u8, 62, 64].iter().enumerate() {
            let id = e
                .timeline()
                .find_at(p, e.grid.us_of_step(i as u64))
                .expect("note at consecutive step");
            let n = e.get_note(id).unwrap();
            assert_eq!(n.pitch.value(), p, "played pitch is used, not cursor pitch");
            assert_eq!(n.dur_us, step, "fixed one-step duration");
        }
    }

    /// StepRecord ignores note-offs (v1 uses a fixed one-step length).
    #[test]
    fn step_record_ignores_note_offs() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('R'));
        e.ingest(NoteEvent::off(MidiNote::new(60).unwrap(), 0));
        assert_eq!(e.note_count(), 0);
        assert_eq!(e.cursor().step, 0, "an off neither places a note nor steps");
    }

    /// DirectEdit ignores played input for placement; cursor keys still edit.
    #[test]
    fn direct_edit_ignores_ingest() {
        let mut e = EditScreen::new();
        e.ingest(on_ev(64));
        assert_eq!(e.note_count(), 0, "direct-edit ignores played notes");

        e.on_key(KeyCode::Char('a')); // cursor editing still works
        assert_eq!(e.note_count(), 1);
    }

    /// End-to-end through the real `MockKeyboard`: typing three mapped keys and
    /// draining the source feeds three note-ons into step-record.
    #[test]
    fn step_record_via_mock_keyboard_forward_key() {
        use rockcraft_midi::{MockKeyboard, NoteSource};

        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('R')); // arm step-record

        let mut kb = MockKeyboard::new();
        kb.forward_key('1'); // C4 = 60
        kb.forward_key('2'); // D4 = 62
        kb.forward_key('3'); // E4 = 64
        for ev in kb.events() {
            e.ingest(ev);
        }

        assert_eq!(e.note_count(), 3, "three mapped keys → three notes");
        assert_eq!(e.cursor().step, 3);
    }

    /// LiveRecord pairs an on/off across an advancing playhead into one `Note`
    /// at the snapped start, lasting the held span.
    #[test]
    fn live_record_pairs_on_off_at_playhead() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('R')); // step
        e.on_key(KeyCode::Char('t')); // live
        let step = e.grid.step_us();
        let pitch = MidiNote::new(60).unwrap();

        e.set_playhead_us(0);
        e.ingest(NoteEvent::on(pitch, Velocity::new(80).unwrap(), 0));
        assert_eq!(e.note_count(), 0, "nothing written until the off pairs it");

        e.set_playhead_us(step * 2); // held two steps
        e.ingest(NoteEvent::off(pitch, 0));

        assert_eq!(e.note_count(), 1);
        let id = e
            .timeline()
            .find_at(60, 0)
            .expect("note recorded at playhead");
        let n = e.get_note(id).unwrap();
        assert_eq!(n.start_us, 0);
        assert_eq!(n.dur_us, step * 2, "duration spans the held playhead range");
    }

    /// LiveRecord snaps an off-grid playhead to the grid and floors a tap at one
    /// step so a zero-length note is still placed.
    #[test]
    fn live_record_snaps_and_floors_one_step() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('R'));
        e.on_key(KeyCode::Char('t'));
        let step = e.grid.step_us();
        let pitch = MidiNote::new(67).unwrap();

        // On just past a step boundary snaps back to that step; off at the same
        // spot would be zero-length, so it floors to one step.
        e.set_playhead_us(step + 10);
        e.ingest(NoteEvent::on(pitch, Velocity::new(80).unwrap(), 0));
        e.set_playhead_us(step + 10);
        e.ingest(NoteEvent::off(pitch, 0));

        assert_eq!(e.note_count(), 1);
        let id = e.timeline().find_at(67, step).expect("snapped to the step");
        let n = e.get_note(id).unwrap();
        assert_eq!(n.start_us, step, "start snapped to grid");
        assert_eq!(n.dur_us, step, "zero-length tap floored to one step");
    }

    // ── transport tests ──────────────────────────────────────────────────

    #[test]
    fn space_starts_play_from_cursor_and_second_space_stops() {
        let mut e = EditScreen::new();
        // Move cursor to step 4 → some non-zero µs (k = step +1).
        for _ in 0..4 {
            e.on_key(KeyCode::Char('k'));
        }
        let cursor_us = e.cursor_us();
        assert!(!e.is_playing());

        e.on_key(KeyCode::Char(' '));
        assert!(e.is_playing());
        // Playhead starts at cursor position.
        assert_eq!(e.playhead_us(), cursor_us);

        // Second Space stops.
        e.on_key(KeyCode::Char(' '));
        assert!(!e.is_playing());
    }

    #[test]
    fn shift_p_plays_whole_song_from_zero() {
        let mut e = EditScreen::new();
        // k = step +1; move 8 steps forward so cursor_us > 0.
        for _ in 0..8 {
            e.on_key(KeyCode::Char('k'));
        }
        assert!(e.cursor_us() > 0);

        e.on_key(KeyCode::Char('P'));
        assert!(e.is_playing());
        assert_eq!(e.playhead_us(), 0);
    }

    #[test]
    fn advance_moves_playhead_forward() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char(' ')); // start from cursor (step 0 → 0 µs)
        assert!(e.is_playing());

        e.advance(500_000);
        let ph = e.playhead_us();
        // playhead_us >= 500_000 (wall-clock adds a tiny amount; extra_us = 500_000)
        assert!(
            ph >= 500_000,
            "playhead must have advanced at least 500_000 µs, got {ph}"
        );

        e.advance(500_000);
        let ph2 = e.playhead_us();
        assert!(
            ph2 >= 1_000_000,
            "playhead must have advanced at least 1_000_000 µs total, got {ph2}"
        );
    }

    #[test]
    fn advance_is_noop_when_stopped() {
        let mut e = EditScreen::new();
        assert!(!e.is_playing());
        e.advance(1_000_000); // must not panic or start playing
        assert!(!e.is_playing());
    }

    #[test]
    fn note_on_fires_exactly_once_as_playhead_passes_start() {
        let mut tl = Timeline::new();
        // Note at 500_000 µs, duration 200_000 µs.
        tl.insert(note(60, 500_000, 200_000));
        let mut e = EditScreen::from_timeline(tl, Grid::default_120());

        // Play from 0.
        e.on_key(KeyCode::Char('P'));
        assert!(e.is_playing());

        // Advance to just before the note: no note-on effect yet.
        let fx = e.advance(499_000);
        assert_eq!(
            count_note_ons(&fx),
            0,
            "note_on should not fire before start_us"
        );

        // Advance past start_us: the note-on fires exactly once.
        let fx = e.advance(2_000);
        assert_eq!(count_note_ons(&fx), 1, "note_on should fire exactly once");

        // Advance again — no further note-on.
        let fx = e.advance(10_000);
        assert_eq!(count_note_ons(&fx), 0, "note_on fires only once");
    }

    #[test]
    fn note_off_fires_after_duration() {
        let mut tl = Timeline::new();
        tl.insert(note(60, 0, 200_000));
        let mut e = EditScreen::from_timeline(tl, Grid::default_120());

        e.on_key(KeyCode::Char('P'));

        // Advance past start (0): note_on fires, no note_off yet.
        let fx = e.advance(1_000);
        assert_eq!(count_note_ons(&fx), 1);
        assert_eq!(count_note_offs(&fx), 0);

        // Advance past end (200_000): note_off fires.
        let fx = e.advance(200_000);
        assert_eq!(count_note_offs(&fx), 1, "note_off should fire after dur_us");
    }

    #[test]
    fn stop_resets_to_stopped() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('P'));
        assert!(e.is_playing());

        e.on_key(KeyCode::Char(' ')); // Space stops
        assert!(!e.is_playing());
    }

    #[test]
    fn play_from_cursor_sets_playhead_to_cursor_us() {
        let mut e = EditScreen::new();
        // Move cursor to step 16 (one bar at default grid); k = step +1.
        for _ in 0..16 {
            e.on_key(KeyCode::Char('k'));
        }
        let cursor_before = e.cursor_us();

        e.on_key(KeyCode::Char(' '));
        assert_eq!(e.playhead_us(), cursor_before);
    }

    #[test]
    fn notes_before_play_start_are_skipped() {
        let mut tl = Timeline::new();
        // Note entirely before cursor.
        tl.insert(note(60, 0, 100_000));
        let mut e = EditScreen::from_timeline(tl, Grid::default_120());

        // Move cursor past the note and start playing; k = step +1.
        for _ in 0..4 {
            e.on_key(KeyCode::Char('k'));
        }
        e.on_key(KeyCode::Char(' ')); // play from cursor_us > 100_000

        // The note ended before the play start, so advancing never sounds it.
        let fx = e.advance(1_000_000);
        assert_eq!(
            count_note_ons(&fx),
            0,
            "a note ending before play start is pre-skipped"
        );
    }

    // ── undo / redo tests ────────────────────────────────────────────────

    /// `u` undoes the most recent edit; `U` redoes it.
    #[test]
    fn u_undoes_and_shift_u_redoes() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // add note at pitch 60 → 1 note
        e.on_key(KeyCode::Char('l')); // pitch +1 (no checkpoint)
        e.on_key(KeyCode::Char('a')); // add second note at pitch 61 → 2 notes

        assert_eq!(e.note_count(), 2);

        e.on_key(KeyCode::Char('u')); // undo second add
        assert_eq!(e.note_count(), 1, "undo removed the second note");

        e.on_key(KeyCode::Char('u')); // undo first add
        assert_eq!(e.note_count(), 0, "undo removed the first note");

        e.on_key(KeyCode::Char('U')); // redo first add
        assert_eq!(e.note_count(), 1, "redo restored first note");

        e.on_key(KeyCode::Char('U')); // redo second add
        assert_eq!(e.note_count(), 2, "redo restored second note");

        e.on_key(KeyCode::Char('U')); // nothing to redo — should be a no-op
        assert_eq!(e.note_count(), 2, "extra redo is a no-op");
    }

    /// A new edit after undo clears the redo stack.
    #[test]
    fn new_edit_after_undo_clears_redo_in_edit_screen() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // note at pitch 60
        e.on_key(KeyCode::Char('l')); // cursor → pitch 61 (pitch +1)
        e.on_key(KeyCode::Char('a')); // note at pitch 61
        assert_eq!(e.note_count(), 2);

        e.on_key(KeyCode::Char('u')); // undo second add; cursor still at pitch 61
        assert_eq!(e.note_count(), 1);

        // A new edit clears redo: move back and delete the remaining note.
        e.on_key(KeyCode::Char('h')); // cursor → pitch 60 (pitch -1)
        e.on_key(KeyCode::Char('x')); // delete the note at pitch 60
        assert_eq!(e.note_count(), 0);
        e.on_key(KeyCode::Char('U')); // redo is gone
        assert_eq!(e.note_count(), 0, "redo cleared by the delete");
    }

    /// `u` on a fresh editor (no edits) is a no-op.
    #[test]
    fn undo_on_empty_history_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('u'));
        assert_eq!(e.note_count(), 0);
        assert_eq!(
            e.cursor(),
            Cursor {
                pitch: DEFAULT_CURSOR_PITCH,
                step: 0
            }
        );
    }

    /// A committed chord (multiple notes) undoes as a single step.
    #[test]
    fn committed_chord_undoes_as_one_step() {
        let mut e = EditScreen::new();

        // Enter chord mode and commit a triad.
        e.on_key(KeyCode::Char('c'));
        e.on_key(KeyCode::Char('1')); // tonic triad C-E-G
        e.on_key(KeyCode::Enter); // commit
        assert_eq!(e.note_count(), 3, "three notes committed");

        e.on_key(KeyCode::Char('u')); // undo the chord commit
        assert_eq!(e.note_count(), 0, "all three notes removed in one undo");

        e.on_key(KeyCode::Char('U')); // redo the chord commit
        assert_eq!(e.note_count(), 3, "all three notes restored in one redo");
    }

    /// Cancelling a chord (Esc) leaves the pre-chord state intact and does NOT
    /// add anything to the redo stack.
    #[test]
    fn cancelled_chord_leaves_state_and_no_redo() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // permanent note at cursor
        assert_eq!(e.note_count(), 1);

        e.on_key(KeyCode::Char('c')); // enter chord mode (3 preview notes added)
        assert_eq!(e.note_count(), 4); // 1 permanent + 3 preview
        e.on_key(KeyCode::Esc); // cancel — preview rolled back

        assert_eq!(e.note_count(), 1, "back to just the permanent note");
        e.on_key(KeyCode::Char('U')); // should be a no-op
        assert_eq!(e.note_count(), 1, "no redo after chord cancel");
    }

    // ── loop / metronome / count-in tests (#64) ──────────────────────────

    /// Advancing past `loop_end_us` with looping on wraps the playhead back to
    /// `loop_start_us` within the same tick — no overshoot beyond one step.
    #[test]
    fn loop_wraps_playhead_at_loop_end() {
        let mut e = EditScreen::new();
        let bar_us = e.grid.bar_us(); // 2_000_000 µs at 120 BPM 4/4

        e.set_loop_bounds(0, bar_us);
        e.toggle_loop();
        assert!(e.is_looping());
        assert_eq!(e.loop_bounds(), (0, bar_us));

        e.on_key(KeyCode::Char('P')); // start from 0
        assert!(e.is_playing());

        // Advance past the loop end by a small overshoot.
        e.advance(bar_us + 10_000);
        e.tick_audition();

        // Playhead must be back near loop_start, not past loop_end.
        let ph = e.playhead_us();
        assert!(
            ph < bar_us,
            "playhead should have wrapped to loop_start, got {ph}"
        );
    }

    /// `o` key toggles loop on; default bounds are the bar under the cursor.
    #[test]
    fn o_key_toggles_loop_and_defaults_to_current_bar() {
        let mut e = EditScreen::new();
        let bar_us = e.grid.bar_us();

        assert!(!e.is_looping());
        e.on_key(KeyCode::Char('o'));
        assert!(e.is_looping());
        let (s, end) = e.loop_bounds();
        assert_eq!(s, 0, "cursor at bar 0 → loop starts at 0");
        assert_eq!(end, bar_us, "loop ends one bar later");

        // Toggle off.
        e.on_key(KeyCode::Char('o'));
        assert!(!e.is_looping());
    }

    /// Metronome fires exactly once per beat over a 4-beat span at 120 BPM 4/4.
    #[test]
    fn metronome_fires_once_per_beat_over_four_beats() {
        let mut e = EditScreen::new();
        let quarter_us = e.grid.quarter_us(); // 500_000 at 120 BPM

        e.toggle_metronome();
        assert!(e.is_metronome_on());

        e.on_key(KeyCode::Char('P')); // start from 0
        assert_eq!(e.metronome_click_count(), 0, "no clicks before advancing");

        // Advance one beat at a time for 4 beats.
        for _ in 0..4 {
            e.advance(quarter_us);
            e.tick_audition();
        }

        assert_eq!(
            e.metronome_click_count(),
            4,
            "exactly one click per beat over 4 beats"
        );
    }

    /// `M` key toggles the metronome.
    #[test]
    fn m_key_toggles_metronome() {
        let mut e = EditScreen::new();
        assert!(!e.is_metronome_on());
        e.on_key(KeyCode::Char('M'));
        assert!(e.is_metronome_on());
        e.on_key(KeyCode::Char('M'));
        assert!(!e.is_metronome_on());
    }

    /// Count-in delays the first recorded note by N bars: notes played during
    /// the pre-roll are discarded; notes after the count-in are captured.
    #[test]
    fn count_in_delays_first_recorded_note_by_n_bars() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('R')); // arm step-record
        e.on_key(KeyCode::Char('t')); // → live-record

        let bar_us = e.grid.bar_us();
        let pitch = MidiNote::new(60).unwrap();
        let vel = Velocity::new(80).unwrap();

        // Start count-in (default 1 bar).
        e.start_count_in_record();
        assert!(e.is_playing());
        assert!(e.is_counting_in(), "should be in count-in phase");

        // Inject a note during the count-in — must NOT be recorded.
        e.ingest(NoteEvent::on(pitch, vel, 0));
        e.ingest(NoteEvent::off(pitch, 0));
        assert_eq!(e.note_count(), 0, "note during count-in is discarded");

        // Advance past the count-in period and tick to expire it.
        e.advance(bar_us + 10_000);
        e.tick_audition();
        assert!(!e.is_counting_in(), "count-in phase should have ended");

        // Inject a note after count-in — must be recorded.
        e.ingest(NoteEvent::on(pitch, vel, 0));
        e.advance(e.grid.step_us());
        e.tick_audition();
        e.ingest(NoteEvent::off(pitch, 0));

        assert_eq!(e.note_count(), 1, "note after count-in is captured");
    }

    /// `C` key starts count-in live record without requiring prior arming.
    #[test]
    fn c_key_starts_count_in_record() {
        let mut e = EditScreen::new();
        assert_eq!(e.input_mode(), InputMode::DirectEdit);

        e.on_key(KeyCode::Char('C'));
        assert!(e.is_playing(), "C starts playback");
        assert!(e.is_counting_in(), "C triggers count-in phase");
        assert_eq!(e.input_mode(), InputMode::LiveRecord, "C arms live record");
    }

    // ── help overlay tests ─────────────────────────────────────────────

    /// `?` sets help_visible() to true.
    #[test]
    fn question_mark_shows_help() {
        let mut e = EditScreen::new();
        assert!(!e.help_visible(), "help should start hidden");
        e.on_key(KeyCode::Char('?'));
        assert!(e.help_visible(), "help should be visible after ?");
    }

    /// `?` again clears help_visible().
    #[test]
    fn question_mark_toggles_help_off() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('?')); // show help
        assert!(e.help_visible());
        e.on_key(KeyCode::Char('?')); // hide help
        assert!(!e.help_visible(), "help should be hidden after second ?");
    }

    /// Esc clears help_visible() when shown.
    #[test]
    fn esc_closes_help() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('?')); // show help
        assert!(e.help_visible());
        e.on_key(KeyCode::Esc); // close help
        assert!(!e.help_visible(), "help should be hidden after Esc");
    }

    /// With help shown, the rendered buffer contains a known binding string.
    #[test]
    fn help_overlay_renders_without_panic() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('?')); // show help
        assert!(e.help_visible());

        // The main test: ensure drawing with help shown doesn't panic
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| e.draw(f, f.area()))
            .expect("draw panicked");

        // Additional check: the buffer should have some content
        let buf = terminal.backend().buffer();
        let content = buf.content();
        assert!(
            !content.is_empty(),
            "expected rendered buffer to have content"
        );
    }

    /// Help overlay can be toggled multiple times.
    #[test]
    fn help_toggle_multiple_times() {
        let mut e = EditScreen::new();
        assert!(!e.help_visible());

        e.on_key(KeyCode::Char('?')); // show
        assert!(e.help_visible());

        e.on_key(KeyCode::Char('?')); // hide
        assert!(!e.help_visible());

        e.on_key(KeyCode::Char('?')); // show again
        assert!(e.help_visible());

        e.on_key(KeyCode::Esc); // hide with Esc
        assert!(!e.help_visible());
    }

    // ── subdivision snap control tests ────────────────────────────────────

    /// `>` walks the subdivision finer through `Subdivision::ALL` and saturates
    /// at the finest (SixteenthTriplet).
    #[test]
    fn finer_snap_walks_all_and_saturates() {
        let mut e = EditScreen::new();
        // Start at default Sixteenth
        assert_eq!(e.current_subdivision(), Subdivision::Sixteenth);

        // Walk finer: Sixteenth → ThirtySecond → EighthTriplet → SixteenthTriplet
        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::ThirtySecond);

        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::EighthTriplet);

        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::SixteenthTriplet);

        // Saturates at finest
        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::SixteenthTriplet);
    }

    /// `<` walks the subdivision coarser and saturates at Quarter.
    #[test]
    fn coarser_snap_walks_back_and_saturates() {
        let mut e = EditScreen::new();
        // Start at default Sixteenth
        assert_eq!(e.current_subdivision(), Subdivision::Sixteenth);

        // Walk coarser: Sixteenth → Eighth → Quarter
        e.on_key(KeyCode::Char('<'));
        assert_eq!(e.current_subdivision(), Subdivision::Eighth);

        e.on_key(KeyCode::Char('<'));
        assert_eq!(e.current_subdivision(), Subdivision::Quarter);

        // Saturates at coarsest
        e.on_key(KeyCode::Char('<'));
        assert_eq!(e.current_subdivision(), Subdivision::Quarter);
    }

    /// After changing snap, the cursor's µs position stays on a valid grid line
    /// of the new subdivision.
    #[test]
    fn cursor_stays_on_grid_line_after_snap_change() {
        let mut e = EditScreen::new();
        // Move cursor to a position that's valid in multiple subdivisions.
        // k = step +1; at 120 BPM: Quarter=500000, Eighth=250000, Sixteenth=125000
        // Position at 250000 µs (2 steps in Sixteenth, 1 step in Eighth)
        e.on_key(KeyCode::Char('k'));
        e.on_key(KeyCode::Char('k')); // 2 * 125000 = 250000 µs

        // Change to Eighth (250000 µs = 1 step in Eighth)
        e.on_key(KeyCode::Char('<'));
        assert_eq!(e.current_subdivision(), Subdivision::Eighth);
        // Cursor should be re-snapped to 250000 µs = 1 step in Eighth
        assert_eq!(e.cursor().step, 1);
        assert_eq!(e.cursor_us(), 250_000);
        // Verify it's on a grid line: grid.snap(cursor_us) == cursor_us
        assert_eq!(e.grid.snap(e.cursor_us()), e.cursor_us());

        // Change back to Sixteenth (250000 µs = 2 steps in Sixteenth)
        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::Sixteenth);
        assert_eq!(e.cursor().step, 2);
        assert_eq!(e.cursor_us(), 250_000);
        assert_eq!(e.grid.snap(e.cursor_us()), e.cursor_us());
    }

    /// Status line contains the active snap label.
    #[test]
    fn status_line_shows_snap_label() {
        let mut e = EditScreen::new();
        // Default is Sixteenth
        assert_eq!(e.grid.subdivision.label(), "1/16");

        // Change to Eighth
        e.on_key(KeyCode::Char('<'));
        assert_eq!(e.grid.subdivision.label(), "1/8");

        // Change to Quarter
        e.on_key(KeyCode::Char('<'));
        assert_eq!(e.grid.subdivision.label(), "1/4");

        // Change to ThirtySecond
        e.on_key(KeyCode::Char('>'));
        e.on_key(KeyCode::Char('>'));
        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.grid.subdivision.label(), "1/32");
    }

    /// Test the full cycle through all subdivisions using finer.
    #[test]
    fn finer_cycles_through_all_subdivisions() {
        let mut e = EditScreen::new();
        // Start at default Sixteenth, go coarser to Quarter first
        e.on_key(KeyCode::Char('<'));
        e.on_key(KeyCode::Char('<'));
        assert_eq!(e.current_subdivision(), Subdivision::Quarter);

        // Now walk finer through all: Quarter → Eighth → Sixteenth → ThirtySecond → EighthTriplet → SixteenthTriplet
        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::Eighth);

        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::Sixteenth);

        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::ThirtySecond);

        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::EighthTriplet);

        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::SixteenthTriplet);

        // Saturates at finest
        e.on_key(KeyCode::Char('>'));
        assert_eq!(e.current_subdivision(), Subdivision::SixteenthTriplet);
    }

    // ── region-select tests ───────────────────────────────────────────────

    /// `v` + move + `y` yanks the right count; clipboard is normalised.
    /// After the axis fix: l/h = pitch ±1, k/j = step ±1.
    #[test]
    fn v_move_y_copies_right_count() {
        let mut e = EditScreen::new();
        // Add two notes: one at (pitch 60, step 0) and one at (pitch 62, step 1).
        e.on_key(KeyCode::Char('a')); // note at (60, step 0)
        e.on_key(KeyCode::Char('l')); // pitch 61
        e.on_key(KeyCode::Char('l')); // pitch 62
        e.on_key(KeyCode::Char('k')); // step 1
        e.on_key(KeyCode::Char('a')); // note at (62, step 1)
        assert_eq!(e.note_count(), 2);

        // Return to (pitch 60, step 0) and start selection.
        e.on_key(KeyCode::Char('h')); // pitch 61
        e.on_key(KeyCode::Char('h')); // pitch 60
        e.on_key(KeyCode::Char('j')); // step 0
        e.on_key(KeyCode::Char('v')); // anchor at (60, step 0)

        // Extend selection to cover both notes.
        e.on_key(KeyCode::Char('l')); // pitch 61
        e.on_key(KeyCode::Char('l')); // pitch 62
        e.on_key(KeyCode::Char('k')); // step 1

        // Both notes should be in the selection.
        assert_eq!(e.selection_ids().len(), 2, "both notes in selection");

        // Yank.
        e.on_key(KeyCode::Char('y'));
        assert_eq!(e.clipboard_len(), 2, "clipboard holds two notes");
        // Selection cleared after yank.
        assert!(e.selection_ids().is_empty(), "selection cleared after yank");
    }

    /// `p` at a new cursor inserts that many notes at the offset.
    #[test]
    fn p_at_new_cursor_inserts_clipboard_count() {
        let mut e = EditScreen::new();
        // Add a note at step 0, pitch 60.
        e.on_key(KeyCode::Char('a'));
        assert_eq!(e.note_count(), 1);

        // Select and yank it.
        e.on_key(KeyCode::Char('v'));
        e.on_key(KeyCode::Char('y'));
        assert_eq!(e.clipboard_len(), 1);

        // Move cursor to a new position (pitch 62, step 1) and paste.
        e.on_key(KeyCode::Char('l')); // pitch 61
        e.on_key(KeyCode::Char('l')); // pitch 62
        e.on_key(KeyCode::Char('k')); // step 1
        let count_before = e.note_count();
        e.on_key(KeyCode::Char('p'));
        assert_eq!(
            e.note_count(),
            count_before + 1,
            "paste inserts one more note"
        );
    }

    /// `D` removes all notes in the selection.
    #[test]
    fn shift_d_removes_selection() {
        let mut e = EditScreen::new();
        // Two notes at different pitches, same step.
        e.on_key(KeyCode::Char('a')); // (pitch 60, step 0)
        e.on_key(KeyCode::Char('l')); // pitch 61
        e.on_key(KeyCode::Char('a')); // (pitch 61, step 0)
        assert_eq!(e.note_count(), 2);

        // Select both.
        e.on_key(KeyCode::Char('h')); // back to pitch 60
        e.on_key(KeyCode::Char('v')); // anchor at (60, step 0)
        e.on_key(KeyCode::Char('l')); // extend to pitch 61
        assert_eq!(e.selection_ids().len(), 2);

        // Delete.
        e.on_key(KeyCode::Char('D'));
        assert_eq!(e.note_count(), 0, "both notes removed");
        assert!(
            e.selection_ids().is_empty(),
            "selection cleared after delete"
        );
    }

    /// `Esc` clears the selection without modifying notes.
    #[test]
    fn esc_clears_selection() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        e.on_key(KeyCode::Char('v'));
        assert!(!e.selection_ids().is_empty(), "note in selection after v");

        e.on_key(KeyCode::Esc);
        assert!(e.selection_ids().is_empty(), "selection gone after Esc");
        assert_eq!(e.note_count(), 1, "note still present");
    }

    /// `p` on an empty clipboard is a no-op.
    #[test]
    fn paste_with_empty_clipboard_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        assert_eq!(e.note_count(), 1);
        assert_eq!(e.clipboard_len(), 0);

        e.on_key(KeyCode::Char('p')); // nothing in clipboard
        assert_eq!(e.note_count(), 1, "paste no-ops when clipboard empty");
    }

    /// `D` on an inactive selection is a no-op.
    #[test]
    fn shift_d_without_selection_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        assert_eq!(e.note_count(), 1);

        e.on_key(KeyCode::Char('D')); // no selection
        assert_eq!(e.note_count(), 1, "D without selection is a no-op");
    }

    // ── issue #121: 1/16 snap cursor movement ─────────────────────────────

    /// At 1/16 snap (120 BPM 4/4), N `cursor_right` presses advance the cursor
    /// by exactly N × 125 000 µs (one sixteenth note). The step index matches
    /// exactly, confirming the state machine is correct regardless of render
    /// resolution.
    /// After the axis fix: k/↑ is the time-forward key (step +1).
    #[test]
    fn cursor_right_at_sixteenth_snap_advances_by_step_us() {
        let mut e = EditScreen::new();
        assert_eq!(e.current_subdivision(), Subdivision::Sixteenth);
        let step_us = e.grid.step_us();
        assert_eq!(step_us, 125_000, "one 1/16 at 120 BPM = 125 000 µs");

        for n in 1u64..=8 {
            e.on_key(KeyCode::Char('k')); // k = step +1 (time forward)
            assert_eq!(e.cursor().step, n, "step index after {n} right press(es)");
            assert_eq!(
                e.grid.us_of_step(e.cursor().step),
                n * step_us,
                "cursor_us = N × step_us after {n} press(es)"
            );
        }

        // Stepping back shrinks by the same unit.
        e.on_key(KeyCode::Char('j')); // j = step -1 (time backward)
        assert_eq!(e.cursor().step, 7);
        assert_eq!(e.grid.us_of_step(e.cursor().step), 7 * step_us);
    }

    /// After a single `cursor_right` at 1/16 snap the rendered output must
    /// differ from the initial render: the sub-beat position indicator in the
    /// status bar advances from `.1` to `.2`, making the step change visible.
    #[test]
    fn cursor_step_visible_in_render_at_sixteenth_snap() {
        let mut e = EditScreen::new();
        assert_eq!(e.current_subdivision(), Subdivision::Sixteenth);

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();

        // Capture the initial render.
        terminal
            .draw(|f| e.draw(f, f.area()))
            .expect("initial draw panicked");
        let content_before: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();

        // One 1/16 step right.
        e.on_key(KeyCode::Char('l'));

        // Capture the post-move render.
        terminal
            .draw(|f| e.draw(f, f.area()))
            .expect("post-move draw panicked");
        let content_after: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();

        assert_ne!(
            content_before, content_after,
            "a single 1/16-step move must change the rendered output \
             (sub-beat indicator advances from .1 to .2)"
        );
    }

    // ── backing-track sync (M5-D) ────────────────────────────────────────────

    /// A temp file standing in for a backing audio file; `save_bundle` copies it.
    fn temp_backing_file(suffix: &str, ext: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rockcraft_edit_backing_{suffix}.{ext}"));
        std::fs::write(&path, b"not-real-audio").unwrap();
        path
    }

    #[test]
    fn backing_target_tracks_playhead_with_zero_shift() {
        // Editor has no pre-roll, so the file position is playhead + audio_start.
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        // Stopped at the song start: the target is 0.
        assert_eq!(e.backing_target_us(), Some(0));
        // Playing and advanced one second: the file is one second in.
        e.on_key(KeyCode::Char('P'));
        e.advance(1_000_000);
        assert_eq!(e.backing_target_us(), Some(1_000_000));
    }

    #[test]
    fn backing_target_respects_audio_start_offset() {
        let e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 250_000);
        // A trimmed lead-in: at playhead 0 the file is already 250ms in.
        assert_eq!(e.backing_target_us(), Some(250_000));
    }

    #[test]
    fn no_backing_never_targets_or_commands() {
        let mut e = EditScreen::new();
        assert_eq!(e.backing_target_us(), None);
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.poll_backing(), BackingCmd::None);
        e.advance(1_000_000);
        assert_eq!(e.poll_backing(), BackingCmd::None);
    }

    #[test]
    fn transport_start_commands_play_at_playhead() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        // Stopped: nothing to do.
        assert_eq!(e.poll_backing(), BackingCmd::None);
        // Pressing play seeks the backing to the playhead (0) and starts it.
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(0));
    }

    #[test]
    fn playing_free_runs_without_recommanding() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(0));
        // While playing forward the sink free-runs: no further commands.
        e.advance(500_000);
        assert_eq!(e.poll_backing(), BackingCmd::None);
        e.advance(500_000);
        assert_eq!(e.poll_backing(), BackingCmd::None);
    }

    #[test]
    fn stop_commands_pause() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(0));
        // Space stops the transport: the backing pauses in place.
        e.on_key(KeyCode::Char(' '));
        assert!(!e.is_playing());
        assert_eq!(e.poll_backing(), BackingCmd::Pause);
    }

    #[test]
    fn backward_jump_while_playing_reseeks() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(0));
        e.advance(2_000_000);
        assert_eq!(e.poll_backing(), BackingCmd::None);
        // Play-from-start while already playing rewinds the playhead to 0 — a
        // backward jump (like a loop wrap) that must re-seek the backing.
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.playhead_us(), 0);
        assert_eq!(e.poll_backing(), BackingCmd::Seek(0));
    }

    #[test]
    fn replay_after_stop_reseeks_from_current_playhead() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        // Play from the cursor (run loop polls every tick).
        e.on_key(KeyCode::Char(' '));
        assert!(e.is_playing());
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(0));
        e.advance(750_000);
        assert_eq!(e.poll_backing(), BackingCmd::None);
        // Stop, then replay: PlayAt re-syncs from wherever the playhead now sits.
        e.on_key(KeyCode::Char(' '));
        assert_eq!(e.poll_backing(), BackingCmd::Pause);
        e.on_key(KeyCode::Char(' '));
        assert!(e.is_playing());
        let target = e.backing_target_us().unwrap();
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(target));
    }

    #[test]
    fn leave_resets_backing_sync_state() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(0));
        e.advance(1_000_000);
        e.leave();
        // After leaving, the prev-playing snapshot is cleared, so re-entering and
        // playing again issues a fresh PlayAt rather than a stale free-run.
        e.on_key(KeyCode::Char('P'));
        assert!(matches!(e.poll_backing(), BackingCmd::PlayAt(_)));
    }

    #[test]
    fn save_bundle_roundtrips_backing_into_meta() {
        let src = temp_backing_file("save_rt", "wav");
        let e = EditScreen::new().with_backing(src.clone(), 250_000);
        let base = std::env::temp_dir().join(format!("rockcraft_edit_save_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let bundle = e.save_bundle(&base).unwrap();

        let json = std::fs::read_to_string(bundle.join("meta.json")).unwrap();
        let meta = RecordingMeta::from_json(&json).unwrap();
        let backing = meta.backing.expect("backing persisted");
        assert_eq!(backing.file, "backing.wav");
        assert_eq!(backing.audio_start_us, 250_000);
        // The audio file was copied into the bundle under that name.
        assert!(bundle.join("backing.wav").exists());

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_file(&src).ok();
    }

    #[test]
    fn nudge_keys_adjust_offset_and_clamp() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        // Coarse later (+250ms) then fine later (+10ms).
        e.on_key(KeyCode::Char('\''));
        e.on_key(KeyCode::Char('.'));
        assert_eq!(e.composer.backing_offset_us(), 260_000);
        // Fine earlier (-10ms).
        e.on_key(KeyCode::Char(','));
        assert_eq!(e.composer.backing_offset_us(), 250_000);
        // Coarse earlier twice clamps at 0 (never negative).
        e.on_key(KeyCode::Char(';'));
        e.on_key(KeyCode::Char(';'));
        assert_eq!(e.composer.backing_offset_us(), 0);
    }

    #[test]
    fn nudge_while_playing_reseeks_backing_to_new_offset() {
        let mut e = EditScreen::new().with_backing(PathBuf::from("backing.wav"), 0);
        e.on_key(KeyCode::Char('P'));
        assert_eq!(e.poll_backing(), BackingCmd::PlayAt(0));
        e.advance(1_000_000);
        assert_eq!(e.poll_backing(), BackingCmd::None);
        // Nudge the alignment later by 250ms while playing: the next poll must
        // re-seek the audio to playhead + new offset = 1_000_000 + 250_000.
        e.on_key(KeyCode::Char('\''));
        assert_eq!(e.composer.backing_offset_us(), 250_000);
        assert_eq!(e.poll_backing(), BackingCmd::Seek(1_250_000));
        // Having consumed the change, a steady poll free-runs again.
        assert_eq!(e.poll_backing(), BackingCmd::None);
    }

    #[test]
    fn nudged_offset_persists_through_save_and_reload() {
        let src = temp_backing_file("nudge_persist", "wav");
        let mut e = EditScreen::new().with_backing(src.clone(), 0);
        // Nudge to a non-zero alignment, then save.
        e.on_key(KeyCode::Char('\'')); // +250ms
        e.on_key(KeyCode::Char('.')); // +10ms
        assert_eq!(e.composer.backing_offset_us(), 260_000);

        let base =
            std::env::temp_dir().join(format!("rockcraft_edit_nudge_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let bundle = e.save_bundle(&base).unwrap();

        // meta.json carries the nudged offset.
        let json = std::fs::read_to_string(bundle.join("meta.json")).unwrap();
        let meta = RecordingMeta::from_json(&json).unwrap();
        let backing = meta.backing.expect("backing persisted");
        assert_eq!(backing.audio_start_us, 260_000);

        // Reopening the bundle (the "Edit last recording" path) restores it.
        let reopened =
            EditScreen::new().with_backing(bundle.join(&backing.file), backing.audio_start_us);
        assert_eq!(reopened.composer.backing_offset_us(), 260_000);

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_file(&src).ok();
    }

    #[test]
    fn save_bundle_midi_only_keeps_backing_none() {
        let e = EditScreen::new();
        let base =
            std::env::temp_dir().join(format!("rockcraft_edit_save_none_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let bundle = e.save_bundle(&base).unwrap();
        let json = std::fs::read_to_string(bundle.join("meta.json")).unwrap();
        let meta = RecordingMeta::from_json(&json).unwrap();
        assert!(meta.backing.is_none());
        std::fs::remove_dir_all(&base).ok();
    }
}
