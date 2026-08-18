//! Transport-agnostic vocabulary of composer operations.
//!
//! This module is the **contract** the rest of M4 builds on: an [`Action`]
//! enumerates every editor operation, an [`Effect`] enumerates the side effects
//! a frontend must carry out (e.g. auditioning sound), and [`ActionError`]
//! reports a failed dispatch.
//!
//! Crucially, actions (de)serialise to a stable string name so a remote
//! `run_action: { name, params }` request maps to an [`Action`] *generically*
//! via [`action_from_name`] — adding a new operation makes it callable over the
//! wire with no transport change. The catalog mirrors the operations currently
//! hard-wired into the TUI keymap (`crates/tui/src/edit.rs::on_key`) but stays
//! pure: no apply logic, no key codes, no I/O. M4-B (`Composer::apply`) gives
//! these meaning; this module only names them.

use crate::background::Easing;
use crate::hand::HandSetting;
use serde::{Deserialize, Serialize};

/// Every composer operation, transport-agnostic.
///
/// Each variant serialises with an internal `"action"` tag holding its stable
/// snake_case [`name`](Action::name), e.g.
/// `{"action": "resize_note", "delta_steps": 2}`. Parametrised variants carry
/// their named fields alongside the tag; nullary variants are just the tag.
///
/// Transport variants stay pure: time is injected as an explicit argument, never
/// read from a wall clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    // ── navigation ──────────────────────────────────────────────────────
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    CursorBarLeft,
    CursorBarRight,
    CursorOctaveDown,
    CursorOctaveUp,
    CursorToStart,
    CursorToEnd,
    /// Jump the cursor pitch to the lowest key (A0, MIDI 21).
    CursorToPitchMin,
    /// Jump the cursor pitch to the highest key (C8, MIDI 108).
    CursorToPitchMax,
    /// Absolute jump to a `(pitch, step)` cell — AI-friendly addressing.
    SetCursor {
        pitch: u8,
        step: u64,
    },
    SubdivisionFiner,
    SubdivisionCoarser,

    // ── edit ────────────────────────────────────────────────────────────
    AddNote,
    DeleteNote,
    ResizeNote {
        delta_steps: i64,
    },
    AdjustVelocity {
        delta: i16,
    },
    ToggleGrab,
    /// Lay a chromatic run of notes from the cursor cell to `end_pitch`
    /// (inclusive), spread evenly across `span_steps` grid steps — a one-shot
    /// glissando/scale traced over the movie backdrop. Each note is one step
    /// long; direction is inferred from `end_pitch` vs the cursor pitch, and any
    /// note already occupying a target cell is replaced.
    InsertRun {
        end_pitch: u8,
        span_steps: u64,
    },
    /// Quantise every note whose onset falls in `[start_us, end_us)` onto the
    /// grid at resolution `step_us` (µs), snapping **both** the onset and the end
    /// to the nearest grid line (phased from the grid origin); a note never
    /// shrinks below one `step_us`. A per-bar "snap this bar to 1/8" tool — pick
    /// `step_us` for the bar's fastest note, and simply skip bars (e.g. glissandi)
    /// you don't want touched.
    QuantizeRegion {
        start_us: u64,
        end_us: u64,
        step_us: u64,
    },

    // ── tempo (piece-wide; lives in the composer Grid) ──────────────────
    /// Nudge the piece tempo by `delta` BPM (clamped to a sane range).
    AdjustBpm {
        delta: i32,
    },
    /// Set the piece tempo to `bpm` BPM (clamped to a sane range).
    SetBpm {
        bpm: u32,
    },
    /// Set the metre, e.g. `3/4`. `beat_unit` is a note value (power of two);
    /// both fields are clamped/snapped rather than rejected.
    SetTimeSig {
        beats_per_bar: u8,
        beat_unit: u8,
    },
    /// Set the grid **phase origin** (µs): the song time bar 1 / beat 1 / step 0
    /// lands on. Align it to a piece's first downbeat so bar/beat lines fall on
    /// the performance when the music doesn't start at song time 0.
    SetGridOrigin {
        us: u64,
    },
    /// Slide the grid **phase origin** by `delta_us` (clamped at 0), keeping the
    /// note times fixed and moving the bar lines under them. The nudge companion
    /// to [`Action::SetGridOrigin`]: dialling the phase in by feel while the
    /// tempo is still being tuned, rather than computing an absolute origin.
    NudgeGridOrigin {
        delta_us: i64,
    },

    // ── chord selector ──────────────────────────────────────────────────
    EnterChordMode,
    CommitChord,
    CancelChord,
    ToggleChordKind,
    SetChordDegree {
        degree: u8,
    },
    CycleChordDegree {
        delta: i8,
    },

    // ── input mode ──────────────────────────────────────────────────────
    ToggleRecordArm,
    ToggleRecordFlavour,

    // ── wait mode (note-by-note play-along) ─────────────────────────────
    ToggleWaitMode,
    SetWaitMode {
        on: bool,
    },

    // ── transport (pure: time is injected, never wall-clock) ────────────
    TogglePlayCursor,
    PlayFromStart,
    Stop,
    Play {
        from_us: u64,
    },
    SetPlayhead {
        us: u64,
    },

    // ── backing alignment ───────────────────────────────────────────────
    /// Slide the attached backing track's `audio_start_us` by `delta_us`
    /// (clamped at 0): positive shifts the audio later in the file, negative
    /// earlier. Editor-side state; a no-op for frontends with no backing.
    NudgeBackingOffset {
        delta_us: i64,
    },

    // ── playback speed ──────────────────────────────────────────────────
    /// Set the transport speed multiplier in permille (1000 = 1× real time;
    /// 500 = half speed). Clamped to 0.25×–2×. Stretches song time for
    /// practice/review without altering the chart; frontends match their
    /// backing-audio speed to it.
    SetPlaybackRate {
        rate_permille: u16,
    },

    // ── loop / metronome / count-in ─────────────────────────────────────
    ToggleLoop,
    ToggleMetronome,
    StartCountInRecord,
    SetLoopBounds {
        start_us: u64,
        end_us: u64,
    },
    /// Set the loop region's **start** to the cursor position. Cursor-relative
    /// so a frontend keypress need not compute microseconds itself.
    SetLoopStart,
    /// Set the loop region's **end** to the cursor position. Cursor-relative
    /// so a frontend keypress need not compute microseconds itself.
    SetLoopEnd,

    // ── selection / clipboard ───────────────────────────────────────────
    StartSelection,
    ClearSelection,
    YankSelection,
    PasteClipboard,
    DeleteSelection,

    // ── background images (M14-D) ───────────────────────────────────────
    // Layout + keyframing for the piece's background image layers. Every
    // variant addresses the *selected* layer at the *edit time*
    // (`Composer::playhead_us()` — the transport while playing, the cursor
    // while stopped), and every one is a no-op when the piece has no layers.
    //
    // The transform deltas are integers (permille / millidegrees) so `Action`
    // keeps its `Eq` derive, mirroring `SetPlaybackRate { rate_permille }`.
    /// Address background layer `index` (back-to-front). Out of range: no-op.
    SelectBackground {
        index: u32,
    },
    /// Move the background selection by `delta`, wrapping both ways.
    CycleBackground {
        delta: i32,
    },
    /// Pan the selected layer by `(dx, dy)` in thousandths of a surface
    /// width/height, auto-keyframing at the edit time.
    NudgeBackgroundPos {
        dx_permille: i32,
        dy_permille: i32,
    },
    /// Zoom the selected layer by `delta_permille` thousandths, auto-keyframing
    /// at the edit time.
    NudgeBackgroundScale {
        delta_permille: i32,
    },
    /// Rotate the selected layer by `delta_millideg` thousandths of a degree,
    /// auto-keyframing at the edit time.
    NudgeBackgroundRotation {
        delta_millideg: i32,
    },
    /// Set the selected layer's opacity in thousandths (1000 = opaque),
    /// auto-keyframing at the edit time.
    SetBackgroundOpacity {
        permille: u16,
    },
    /// Set the curve leaving the keyframe at the edit time. No-op when no
    /// keyframe sits exactly there.
    SetBackgroundEasing {
        easing: Easing,
    },
    /// Pin the selected layer's currently interpolated transform as an explicit
    /// keyframe at the edit time.
    AddBackgroundKeyframe,
    /// Drop the selected layer's keyframe at the edit time, if any.
    DeleteBackgroundKeyframe,

    // ── hand assignment (M14-E) ─────────────────────────────────────────
    /// Set the piece's left/right split line: notes below `pitch` default to
    /// the left hand, at/above it to the right. Per-note overrides win over it.
    SetHandSplit {
        pitch: u8,
    },
    /// Pin the target notes to a hand (or back to `Auto` = follow the split).
    /// The target is the **selection** when one is active, else the note under
    /// the cursor; with neither it is a no-op, never an error.
    SetNoteHand {
        hand: HandSetting,
    },
    /// Cycle the same target's setting `Auto → Left → Right → Auto` — the
    /// one-key convenience. Reads the target's current setting (the cursor
    /// note's, or the first selected note's) to decide the next one.
    CycleNoteHand,

    // ── time / structure (ripple insert & cut) ──────────────────────────
    /// Insert one empty bar at the cursor's bar boundary, sliding every note at
    /// or after it one bar later. Ripple edit: opens a silent bar without
    /// disturbing the tail's internal timing.
    InsertBar,
    /// Cut the bar the cursor sits in — delete the notes that start in it and
    /// slide everything after one bar earlier, leaving no gap. Ripple edit; the
    /// inverse of [`InsertBar`](Action::InsertBar) plus the deletion.
    RemoveBar,
    /// Ripple-shift **everything at or after the cursor** by `delta_steps` grid
    /// steps (signed), re-phasing the rest of the song against the grid/backing in
    /// one move. The one-shot fix for a constant timing offset that begins at a
    /// point (e.g. an added/dropped beat). Notes before the cursor are untouched.
    NudgeTail {
        delta_steps: i32,
    },
    /// Slow (`delta > 0`) or speed (`delta < 0`) the **bar the cursor sits in** by
    /// `delta` grid steps of length, re-timing the notes inside it to stay on
    /// their beats and rippling everything after by the change. Builds/uses the
    /// per-bar tempo map, so bar lines stay put where the tempo isn't touched.
    NudgeBarTempo {
        delta: i32,
    },
    /// Change the length of the bar the cursor sits in by `delta_steps` grid
    /// steps (at the live subdivision), sliding every **bar line after it** by
    /// that amount. Purely a grid edit — **no note is moved and no time is added
    /// or removed** — for fixing an odd-length measure so the bar lines land back
    /// on the (fixed) notes. The step size follows the subdivision, so `<`/`>`
    /// gives finer/coarser control (down to a 1/32 note). Uses the tempo map.
    NudgeBarLength {
        delta_steps: i32,
    },

    // ── history ─────────────────────────────────────────────────────────
    Undo,
    Redo,
}

impl Action {
    /// Stable snake_case name, identical to the serde `"action"` tag.
    ///
    /// e.g. `Action::ResizeNote { delta_steps: 2 }.name() == "resize_note"`.
    /// A test enforces this parity against [`action_names`] so a remote
    /// `run_action` can never name an action the dispatcher would reject.
    pub fn name(&self) -> &'static str {
        match self {
            Action::CursorLeft => "cursor_left",
            Action::CursorRight => "cursor_right",
            Action::CursorUp => "cursor_up",
            Action::CursorDown => "cursor_down",
            Action::CursorBarLeft => "cursor_bar_left",
            Action::CursorBarRight => "cursor_bar_right",
            Action::CursorOctaveDown => "cursor_octave_down",
            Action::CursorOctaveUp => "cursor_octave_up",
            Action::CursorToStart => "cursor_to_start",
            Action::CursorToEnd => "cursor_to_end",
            Action::CursorToPitchMin => "cursor_to_pitch_min",
            Action::CursorToPitchMax => "cursor_to_pitch_max",
            Action::SetCursor { .. } => "set_cursor",
            Action::SubdivisionFiner => "subdivision_finer",
            Action::SubdivisionCoarser => "subdivision_coarser",
            Action::AddNote => "add_note",
            Action::DeleteNote => "delete_note",
            Action::ResizeNote { .. } => "resize_note",
            Action::AdjustVelocity { .. } => "adjust_velocity",
            Action::ToggleGrab => "toggle_grab",
            Action::InsertRun { .. } => "insert_run",
            Action::AdjustBpm { .. } => "adjust_bpm",
            Action::SetBpm { .. } => "set_bpm",
            Action::SetTimeSig { .. } => "set_time_sig",
            Action::SetGridOrigin { .. } => "set_grid_origin",
            Action::NudgeGridOrigin { .. } => "nudge_grid_origin",
            Action::QuantizeRegion { .. } => "quantize_region",
            Action::EnterChordMode => "enter_chord_mode",
            Action::CommitChord => "commit_chord",
            Action::CancelChord => "cancel_chord",
            Action::ToggleChordKind => "toggle_chord_kind",
            Action::SetChordDegree { .. } => "set_chord_degree",
            Action::CycleChordDegree { .. } => "cycle_chord_degree",
            Action::ToggleRecordArm => "toggle_record_arm",
            Action::ToggleRecordFlavour => "toggle_record_flavour",
            Action::ToggleWaitMode => "toggle_wait_mode",
            Action::SetWaitMode { .. } => "set_wait_mode",
            Action::TogglePlayCursor => "toggle_play_cursor",
            Action::PlayFromStart => "play_from_start",
            Action::Stop => "stop",
            Action::Play { .. } => "play",
            Action::SetPlayhead { .. } => "set_playhead",
            Action::NudgeBackingOffset { .. } => "nudge_backing_offset",
            Action::SetPlaybackRate { .. } => "set_playback_rate",
            Action::ToggleLoop => "toggle_loop",
            Action::ToggleMetronome => "toggle_metronome",
            Action::StartCountInRecord => "start_count_in_record",
            Action::SetLoopBounds { .. } => "set_loop_bounds",
            Action::SetLoopStart => "set_loop_start",
            Action::SetLoopEnd => "set_loop_end",
            Action::StartSelection => "start_selection",
            Action::ClearSelection => "clear_selection",
            Action::YankSelection => "yank_selection",
            Action::PasteClipboard => "paste_clipboard",
            Action::DeleteSelection => "delete_selection",
            Action::SelectBackground { .. } => "select_background",
            Action::CycleBackground { .. } => "cycle_background",
            Action::NudgeBackgroundPos { .. } => "nudge_background_pos",
            Action::NudgeBackgroundScale { .. } => "nudge_background_scale",
            Action::NudgeBackgroundRotation { .. } => "nudge_background_rotation",
            Action::SetBackgroundOpacity { .. } => "set_background_opacity",
            Action::SetBackgroundEasing { .. } => "set_background_easing",
            Action::AddBackgroundKeyframe => "add_background_keyframe",
            Action::DeleteBackgroundKeyframe => "delete_background_keyframe",
            Action::SetHandSplit { .. } => "set_hand_split",
            Action::SetNoteHand { .. } => "set_note_hand",
            Action::CycleNoteHand => "cycle_note_hand",
            Action::InsertBar => "insert_bar",
            Action::RemoveBar => "remove_bar",
            Action::NudgeTail { .. } => "nudge_tail",
            Action::NudgeBarTempo { .. } => "nudge_bar_tempo",
            Action::NudgeBarLength { .. } => "nudge_bar_length",
            Action::Undo => "undo",
            Action::Redo => "redo",
        }
    }
}

/// A side effect a frontend must carry out after applying an [`Action`].
///
/// `core` describes *what* to sound; the frontend owns the synth and decides
/// *how*. Serialises with an internal `"effect"` tag, mirroring [`Action`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Sound one note now, stopping any prior audition.
    AuditionNote { pitch: u8, velocity: u8 },
    /// Sound a chord now, stopping any prior audition.
    AuditionChord { pitches: Vec<u8> },
    /// Silence everything the frontend is auditioning.
    AllOff,
}

/// A failed [`action_from_name`] dispatch.
///
/// Kept dependency-free on purpose (`core` stays light): `Display` + `Error`
/// are implemented by hand rather than derived via `thiserror`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    /// No action is registered under this name.
    UnknownAction(String),
    /// The action exists, but its parameters were missing or the wrong shape.
    BadParams { action: String, detail: String },
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionError::UnknownAction(name) => write!(f, "unknown action: {name}"),
            ActionError::BadParams { action, detail } => {
                write!(f, "bad params for action `{action}`: {detail}")
            }
        }
    }
}

impl std::error::Error for ActionError {}

/// Build an [`Action`] from a remote `run_action` request.
///
/// `params` is the JSON object of named fields (may be `null` or an empty object
/// for nullary actions). The action is reconstructed by splicing the name into a
/// serde-tagged value (`{"action": name, ...params}`) and deserialising, so the
/// name/serde-tag parity is the single source of truth.
///
/// Returns [`ActionError::UnknownAction`] when `name` is not in
/// [`action_names`], and [`ActionError::BadParams`] when `params` is not an
/// object/`null` or a field is missing or mistyped.
pub fn action_from_name(name: &str, params: &serde_json::Value) -> Result<Action, ActionError> {
    if !action_names().contains(&name) {
        return Err(ActionError::UnknownAction(name.to_string()));
    }

    // Splice the params into a serde-tagged object: {"action": name, ...params}.
    let mut obj = match params {
        serde_json::Value::Object(map) => map.clone(),
        serde_json::Value::Null => serde_json::Map::new(),
        other => {
            return Err(ActionError::BadParams {
                action: name.to_string(),
                detail: format!("params must be a JSON object or null, got `{other}`"),
            });
        }
    };
    // The tag wins over any stray `action` key the caller may have supplied.
    obj.insert(
        "action".to_string(),
        serde_json::Value::String(name.to_string()),
    );

    serde_json::from_value(serde_json::Value::Object(obj)).map_err(|e| ActionError::BadParams {
        action: name.to_string(),
        detail: e.to_string(),
    })
}

/// Every action name, for discovery (`query actions`) and self-documentation.
///
/// The list is exhaustive and each entry round-trips through
/// [`action_from_name`] (a test enforces both), so it doubles as the wire
/// catalog a remote client can enumerate.
pub fn action_names() -> &'static [&'static str] {
    &[
        "cursor_left",
        "cursor_right",
        "cursor_up",
        "cursor_down",
        "cursor_bar_left",
        "cursor_bar_right",
        "cursor_octave_down",
        "cursor_octave_up",
        "cursor_to_start",
        "cursor_to_end",
        "cursor_to_pitch_min",
        "cursor_to_pitch_max",
        "set_cursor",
        "subdivision_finer",
        "subdivision_coarser",
        "add_note",
        "delete_note",
        "resize_note",
        "adjust_velocity",
        "toggle_grab",
        "insert_run",
        "adjust_bpm",
        "set_bpm",
        "set_time_sig",
        "set_grid_origin",
        "nudge_grid_origin",
        "quantize_region",
        "enter_chord_mode",
        "commit_chord",
        "cancel_chord",
        "toggle_chord_kind",
        "set_chord_degree",
        "cycle_chord_degree",
        "toggle_record_arm",
        "toggle_record_flavour",
        "toggle_wait_mode",
        "set_wait_mode",
        "toggle_play_cursor",
        "play_from_start",
        "stop",
        "play",
        "set_playhead",
        "nudge_backing_offset",
        "set_playback_rate",
        "toggle_loop",
        "toggle_metronome",
        "start_count_in_record",
        "set_loop_bounds",
        "set_loop_start",
        "set_loop_end",
        "start_selection",
        "clear_selection",
        "yank_selection",
        "paste_clipboard",
        "delete_selection",
        "select_background",
        "cycle_background",
        "nudge_background_pos",
        "nudge_background_scale",
        "nudge_background_rotation",
        "set_background_opacity",
        "set_background_easing",
        "add_background_keyframe",
        "delete_background_keyframe",
        "set_hand_split",
        "set_note_hand",
        "cycle_note_hand",
        "insert_bar",
        "remove_bar",
        "nudge_tail",
        "nudge_bar_tempo",
        "nudge_bar_length",
        "undo",
        "redo",
    ]
}

/// One parameter of an [`Action`], for `query help` discovery.
///
/// `ty` is the Rust scalar name (`"u8"`, `"u64"`, `"i64"`, …) so a client knows
/// what JSON value to send. Kept `&'static` — the catalog is fully const.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ParamInfo {
    pub name: &'static str,
    pub ty: &'static str,
}

/// Self-describing metadata for one [`Action`]: its wire name, the parameters it
/// accepts, and a one-line human description.
///
/// This is the machine-readable counterpart to [`action_names`]: where the
/// latter lists only names, [`action_help`] adds params and prose so an agent
/// can discover the *whole* call shape live, with no hand-maintained doc table
/// to drift. A test enforces that [`action_help`] covers exactly the same set of
/// names as [`action_names`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ActionInfo {
    pub name: &'static str,
    pub params: &'static [ParamInfo],
    pub description: &'static str,
}

/// Structured, self-describing catalog of every action — the source for
/// `query help`. Mirrors [`action_names`] one-to-one (a test enforces parity)
/// and adds parameters + a one-line description per action.
pub fn action_help() -> &'static [ActionInfo] {
    ACTION_HELP
}

const fn p(name: &'static str, ty: &'static str) -> ParamInfo {
    ParamInfo { name, ty }
}

/// The const catalog backing [`action_help`]. A `static` so all the nested
/// `&[ParamInfo]` slices promote to `'static` rather than being temporaries.
static ACTION_HELP: &[ActionInfo] = {
    &[
        // ── navigation ──────────────────────────────────────────────────
        ActionInfo { name: "cursor_left", params: &[], description: "Move the cursor one grid step left." },
        ActionInfo { name: "cursor_right", params: &[], description: "Move the cursor one grid step right." },
        ActionInfo { name: "cursor_up", params: &[], description: "Move the cursor up one semitone (or transpose the grabbed note)." },
        ActionInfo { name: "cursor_down", params: &[], description: "Move the cursor down one semitone (or transpose the grabbed note)." },
        ActionInfo { name: "cursor_bar_left", params: &[], description: "Move the cursor one bar left." },
        ActionInfo { name: "cursor_bar_right", params: &[], description: "Move the cursor one bar right." },
        ActionInfo { name: "cursor_octave_down", params: &[], description: "Move the cursor down one octave." },
        ActionInfo { name: "cursor_octave_up", params: &[], description: "Move the cursor up one octave." },
        ActionInfo { name: "cursor_to_start", params: &[], description: "Jump the cursor to the start of the timeline." },
        ActionInfo { name: "cursor_to_end", params: &[], description: "Jump the cursor to the end of the timeline." },
        ActionInfo { name: "cursor_to_pitch_min", params: &[], description: "Jump the cursor pitch to the lowest key (A0, MIDI 21)." },
        ActionInfo { name: "cursor_to_pitch_max", params: &[], description: "Jump the cursor pitch to the highest key (C8, MIDI 108)." },
        ActionInfo { name: "set_cursor", params: &[p("pitch", "u8"), p("step", "u64")], description: "Absolute jump to a (pitch, step) cell — AI-friendly addressing." },
        ActionInfo { name: "subdivision_finer", params: &[], description: "Halve the grid step for finer placement." },
        ActionInfo { name: "subdivision_coarser", params: &[], description: "Double the grid step for coarser placement." },
        // ── edit ────────────────────────────────────────────────────────
        ActionInfo { name: "add_note", params: &[], description: "Add a note at the cursor (duration 1 step, velocity 80); replaces any note already in that cell." },
        ActionInfo { name: "delete_note", params: &[], description: "Delete the note under the cursor." },
        ActionInfo { name: "resize_note", params: &[p("delta_steps", "i64")], description: "Lengthen (positive) or shorten (negative) the note under the cursor by delta_steps grid steps." },
        ActionInfo { name: "adjust_velocity", params: &[p("delta", "i16")], description: "Adjust the velocity of the note under the cursor by delta (clamped 1..=127)." },
        ActionInfo { name: "toggle_grab", params: &[], description: "Grab/drop the note under the cursor so cursor moves drag it." },
        ActionInfo { name: "insert_run", params: &[p("end_pitch", "u8"), p("span_steps", "u64")], description: "Lay a chromatic run from the cursor to end_pitch (inclusive), spread evenly across span_steps grid steps — a one-shot glissando/scale; replaces notes in target cells." },
        // ── tempo ─────────────────────────────────────────────────────────
        ActionInfo { name: "adjust_bpm", params: &[p("delta", "i32")], description: "Nudge the piece tempo by delta BPM (clamped to 20..=300)." },
        ActionInfo { name: "set_bpm", params: &[p("bpm", "u32")], description: "Set the piece tempo to bpm BPM (clamped to 20..=300)." },
        ActionInfo { name: "set_time_sig", params: &[p("beats_per_bar", "u8"), p("beat_unit", "u8")], description: "Set the metre, e.g. beats_per_bar 3 / beat_unit 4 for 3/4. beats_per_bar is clamped to 1..=32; beat_unit snaps to the nearest note value in 1/2/4/8/16/32. Bar lines move; note times do not." },
        ActionInfo { name: "set_grid_origin", params: &[p("us", "u64")], description: "Set the grid phase origin (us): the song time bar 1/beat 1/step 0 lands on. Align to the first downbeat so bar lines fall on the performance." },
        ActionInfo { name: "nudge_grid_origin", params: &[p("delta_us", "i64")], description: "Slide the grid phase origin by delta_us (clamped at 0), moving the bar lines under fixed note times. The nudge companion to set_grid_origin, for dialling the phase in by feel while the tempo is still being tuned." },
        ActionInfo { name: "quantize_region", params: &[p("start_us", "u64"), p("end_us", "u64"), p("step_us", "u64")], description: "Snap notes whose onset is in [start_us, end_us) onto the grid at resolution step_us (both onset and end, phased from the grid origin; min one step long). Per-bar snapping — skip bars you don't want touched (e.g. glissandi)." },
        // ── chord selector ──────────────────────────────────────────────
        ActionInfo { name: "enter_chord_mode", params: &[], description: "Open the chord selector at the cursor and start previewing a chord." },
        ActionInfo { name: "commit_chord", params: &[], description: "Write the previewed chord into the timeline and close the selector." },
        ActionInfo { name: "cancel_chord", params: &[], description: "Close the chord selector without writing anything." },
        ActionInfo { name: "toggle_chord_kind", params: &[], description: "Toggle the chord quality (e.g. triad ↔ seventh)." },
        ActionInfo { name: "set_chord_degree", params: &[p("degree", "u8")], description: "Set the chord scale degree (1..=7)." },
        ActionInfo { name: "cycle_chord_degree", params: &[p("delta", "i8")], description: "Cycle the chord scale degree by delta." },
        // ── input mode ──────────────────────────────────────────────────
        ActionInfo { name: "toggle_record_arm", params: &[], description: "Arm/disarm recording from live MIDI input." },
        ActionInfo { name: "toggle_record_flavour", params: &[], description: "Flip the record flavour between step and live (no-op while disarmed)." },
        // ── wait mode ─────────────────────────────────────────────────────
        ActionInfo { name: "toggle_wait_mode", params: &[], description: "Toggle note-by-note wait mode: playback freezes until the required notes are held." },
        ActionInfo { name: "set_wait_mode", params: &[p("on", "bool")], description: "Set note-by-note wait mode on (true) or off (false)." },
        // ── transport ───────────────────────────────────────────────────
        ActionInfo { name: "toggle_play_cursor", params: &[], description: "Toggle playback starting at the cursor position." },
        ActionInfo { name: "play_from_start", params: &[], description: "Play from the start of the timeline." },
        ActionInfo { name: "stop", params: &[], description: "Stop playback." },
        ActionInfo { name: "play", params: &[p("from_us", "u64")], description: "Start playback from from_us microseconds." },
        ActionInfo { name: "set_playhead", params: &[p("us", "u64")], description: "Move the playhead to us microseconds." },
        // ── backing alignment ───────────────────────────────────────────
        ActionInfo { name: "nudge_backing_offset", params: &[p("delta_us", "i64")], description: "Slide the backing track's audio_start_us by delta_us to align it under the highway. May go negative: a negative offset delays the audio, holding the backing silent until its start reaches the highway." },
        ActionInfo { name: "set_playback_rate", params: &[p("rate_permille", "u16")], description: "Set playback speed in permille (1000 = 1x, 500 = half speed), clamped 0.25x-2x. Slows/speeds the transport for practice without changing the chart." },
        // ── loop / metronome / count-in ─────────────────────────────────
        ActionInfo { name: "toggle_loop", params: &[], description: "Toggle looped playback over the loop region." },
        ActionInfo { name: "toggle_metronome", params: &[], description: "Toggle the metronome click." },
        ActionInfo { name: "start_count_in_record", params: &[], description: "Begin a metronome count-in, then start recording." },
        ActionInfo { name: "set_loop_bounds", params: &[p("start_us", "u64"), p("end_us", "u64")], description: "Set the loop region to [start_us, end_us) microseconds." },
        ActionInfo { name: "set_loop_start", params: &[], description: "Set the loop region's start to the cursor position." },
        ActionInfo { name: "set_loop_end", params: &[], description: "Set the loop region's end to the cursor position." },
        // ── selection / clipboard ───────────────────────────────────────
        ActionInfo { name: "start_selection", params: &[], description: "Begin a selection rectangle anchored at the cursor." },
        ActionInfo { name: "clear_selection", params: &[], description: "Clear the active selection." },
        ActionInfo { name: "yank_selection", params: &[], description: "Copy the selected notes to the clipboard." },
        ActionInfo { name: "paste_clipboard", params: &[], description: "Paste the clipboard at the cursor." },
        ActionInfo { name: "delete_selection", params: &[], description: "Delete the notes inside the selection." },
        // ── background images (M14-D) ───────────────────────────────────
        ActionInfo { name: "select_background", params: &[p("index", "u32")], description: "Address background image layer `index` (0 = furthest back). Out of range: no-op." },
        ActionInfo { name: "cycle_background", params: &[p("delta", "i32")], description: "Move the background-layer selection by delta, wrapping in both directions." },
        ActionInfo { name: "nudge_background_pos", params: &[p("dx_permille", "i32"), p("dy_permille", "i32")], description: "Pan the selected background layer by (dx, dy) thousandths of a surface width/height, writing (and creating if needed) the keyframe at the edit time." },
        ActionInfo { name: "nudge_background_scale", params: &[p("delta_permille", "i32")], description: "Zoom the selected background layer by delta_permille thousandths (1000 = 1x), writing the keyframe at the edit time. Clamped to 0.05x-10x." },
        ActionInfo { name: "nudge_background_rotation", params: &[p("delta_millideg", "i32")], description: "Rotate the selected background layer by delta_millideg thousandths of a degree, writing the keyframe at the edit time." },
        ActionInfo { name: "set_background_opacity", params: &[p("permille", "u16")], description: "Set the selected background layer's opacity in thousandths (1000 = fully opaque), writing the keyframe at the edit time." },
        ActionInfo { name: "set_background_easing", params: &[p("easing", "Easing")], description: "Set the curve leaving the selected layer's keyframe at the edit time: \"linear\", \"ease_in\", \"ease_out\", \"ease_in_out\" or \"hold\" (a cut). No-op when no keyframe sits exactly there." },
        ActionInfo { name: "add_background_keyframe", params: &[], description: "Pin the selected background layer's currently interpolated transform as an explicit keyframe at the edit time." },
        ActionInfo { name: "delete_background_keyframe", params: &[], description: "Delete the selected background layer's keyframe at the edit time, if one sits exactly there." },
        // ── hand assignment (M14-E) ─────────────────────────────────────
        ActionInfo { name: "set_hand_split", params: &[p("pitch", "u8")], description: "Set the piece's left/right hand split line: notes below `pitch` default to the left hand, at/above it to the right. Per-note overrides win over it." },
        ActionInfo { name: "set_note_hand", params: &[p("hand", "HandSetting")], description: "Pin the target notes to a hand: \"left\", \"right\", or \"auto\" to follow the split line. Targets the selection when one is active, else the note under the cursor; a no-op with neither." },
        ActionInfo { name: "cycle_note_hand", params: &[], description: "Cycle the target notes' hand setting auto -> left -> right -> auto. Same target as set_note_hand (selection, else the cursor note)." },
        ActionInfo { name: "insert_bar", params: &[], description: "Insert one empty bar at the cursor's bar boundary, sliding every note at or after it one bar later (ripple). Opens a silent bar to make room; the tail keeps its internal timing." },
        ActionInfo { name: "remove_bar", params: &[], description: "Cut the bar the cursor sits in: delete the notes starting in it and slide everything after one bar earlier so no gap is left (ripple)." },
        ActionInfo { name: "nudge_tail", params: &[p("delta_steps", "i32")], description: "Ripple-shift every note at or after the cursor by delta_steps grid steps (signed) — re-phases the rest of the song in one move to fix a constant timing offset that starts at a point. Notes before the cursor are untouched." },
        ActionInfo { name: "nudge_bar_tempo", params: &[p("delta", "i32")], description: "Slow (delta>0) or speed (delta<0) the bar the cursor sits in by delta grid steps of length; the notes inside re-time to stay on their beats and everything after ripples. Uses the per-bar tempo map so untouched bars stay put." },
        ActionInfo { name: "nudge_bar_length", params: &[p("delta_steps", "i32")], description: "Change the length of the cursor's bar by delta_steps grid steps (at the live subdivision) and slide every bar line after it by that amount. Grid-only: NO note moves and NO time is added/removed — for fixing an odd-length measure so bar lines land back on the fixed notes. Use </> to change the subdivision for finer/coarser steps (down to 1/32)." },
        // ── history ─────────────────────────────────────────────────────
        ActionInfo { name: "undo", params: &[], description: "Undo the last edit." },
        ActionInfo { name: "redo", params: &[], description: "Redo the last undone edit." },
    ]
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One sample of **every** [`Action`] variant. Parametrised variants use
    /// minimal valid values. This is the exhaustiveness oracle: the parity tests
    /// below cross-check it against [`action_names`], so a forgotten variant in
    /// either list surfaces as a failure.
    fn all_variants() -> Vec<Action> {
        vec![
            Action::CursorLeft,
            Action::CursorRight,
            Action::CursorUp,
            Action::CursorDown,
            Action::CursorBarLeft,
            Action::CursorBarRight,
            Action::CursorOctaveDown,
            Action::CursorOctaveUp,
            Action::CursorToStart,
            Action::CursorToEnd,
            Action::CursorToPitchMin,
            Action::CursorToPitchMax,
            Action::SetCursor { pitch: 60, step: 4 },
            Action::SubdivisionFiner,
            Action::SubdivisionCoarser,
            Action::AddNote,
            Action::DeleteNote,
            Action::ResizeNote { delta_steps: 2 },
            Action::AdjustVelocity { delta: -8 },
            Action::ToggleGrab,
            Action::InsertRun {
                end_pitch: 72,
                span_steps: 8,
            },
            Action::AdjustBpm { delta: -5 },
            Action::SetBpm { bpm: 90 },
            Action::SetTimeSig {
                beats_per_bar: 3,
                beat_unit: 4,
            },
            Action::SetGridOrigin { us: 5_191_846 },
            Action::NudgeGridOrigin { delta_us: -10_000 },
            Action::QuantizeRegion {
                start_us: 5_000_000,
                end_us: 7_000_000,
                step_us: 174_418,
            },
            Action::EnterChordMode,
            Action::CommitChord,
            Action::CancelChord,
            Action::ToggleChordKind,
            Action::SetChordDegree { degree: 5 },
            Action::CycleChordDegree { delta: -1 },
            Action::ToggleRecordArm,
            Action::ToggleRecordFlavour,
            Action::ToggleWaitMode,
            Action::SetWaitMode { on: true },
            Action::TogglePlayCursor,
            Action::PlayFromStart,
            Action::Stop,
            Action::Play { from_us: 1_000 },
            Action::SetPlayhead { us: 2_000 },
            Action::NudgeBackingOffset { delta_us: 10_000 },
            Action::SetPlaybackRate { rate_permille: 500 },
            Action::ToggleLoop,
            Action::ToggleMetronome,
            Action::StartCountInRecord,
            Action::SetLoopBounds {
                start_us: 0,
                end_us: 1_000,
            },
            Action::SetLoopStart,
            Action::SetLoopEnd,
            Action::StartSelection,
            Action::ClearSelection,
            Action::YankSelection,
            Action::PasteClipboard,
            Action::DeleteSelection,
            Action::SelectBackground { index: 0 },
            Action::CycleBackground { delta: 1 },
            Action::NudgeBackgroundPos {
                dx_permille: 25,
                dy_permille: -25,
            },
            Action::NudgeBackgroundScale { delta_permille: 50 },
            Action::NudgeBackgroundRotation {
                delta_millideg: 1_500,
            },
            Action::SetBackgroundOpacity { permille: 600 },
            Action::SetBackgroundEasing {
                easing: Easing::EaseInOut,
            },
            Action::AddBackgroundKeyframe,
            Action::DeleteBackgroundKeyframe,
            Action::SetHandSplit { pitch: 55 },
            Action::SetNoteHand {
                hand: HandSetting::Left,
            },
            Action::CycleNoteHand,
            Action::InsertBar,
            Action::RemoveBar,
            Action::NudgeTail { delta_steps: -2 },
            Action::NudgeBarTempo { delta: 1 },
            Action::NudgeBarLength { delta_steps: -1 },
            Action::Undo,
            Action::Redo,
        ]
    }

    #[test]
    fn every_variant_round_trips() {
        for action in all_variants() {
            let value = serde_json::to_value(&action).expect("serialises");
            let back: Action = serde_json::from_value(value).expect("deserialises");
            assert_eq!(action, back, "round-trip mismatch for {}", action.name());
        }
    }

    #[test]
    fn name_matches_serde_tag() {
        for action in all_variants() {
            let value = serde_json::to_value(&action).expect("serialises");
            let tag = value
                .get("action")
                .and_then(|v| v.as_str())
                .expect("tagged with `action`");
            assert_eq!(
                tag,
                action.name(),
                "serde tag and name() disagree for {action:?}"
            );
        }
    }

    #[test]
    fn action_names_is_exhaustive_and_matches_variants() {
        use std::collections::BTreeSet;
        let from_variants: BTreeSet<&str> = all_variants().iter().map(|a| a.name()).collect();
        let from_catalog: BTreeSet<&str> = action_names().iter().copied().collect();
        assert_eq!(
            from_variants, from_catalog,
            "action_names() must list exactly the Action variants"
        );
    }

    #[test]
    fn action_help_matches_action_names_exactly() {
        use std::collections::BTreeSet;
        let from_help: BTreeSet<&str> = action_help().iter().map(|a| a.name).collect();
        let from_names: BTreeSet<&str> = action_names().iter().copied().collect();
        assert_eq!(
            from_help, from_names,
            "action_help() must describe exactly the names in action_names()"
        );
        // Same length too — guards against a duplicate masking a missing entry.
        assert_eq!(action_help().len(), action_names().len());
    }

    #[test]
    fn action_help_every_param_set_dispatches() {
        // Each described action, called with a minimal valid value per param,
        // must build via action_from_name — proving the documented param names
        // and the deserialiser agree.
        for info in action_help() {
            let mut params = serde_json::Map::new();
            for p in info.params {
                // A type-appropriate sample value per documented scalar type.
                let sample = match p.ty {
                    "bool" => json!(true),
                    "Easing" => json!("linear"),
                    "HandSetting" => json!("auto"),
                    _ => json!(1), // small in-range value for every numeric type
                };
                params.insert(p.name.to_string(), sample);
            }
            action_from_name(info.name, &serde_json::Value::Object(params)).unwrap_or_else(|e| {
                panic!("{} should dispatch from its help params: {e}", info.name)
            });
        }
    }

    #[test]
    fn action_help_descriptions_are_non_empty() {
        for info in action_help() {
            assert!(
                !info.description.is_empty(),
                "{} has an empty description",
                info.name
            );
        }
    }

    #[test]
    fn action_names_non_empty_and_unique() {
        use std::collections::BTreeSet;
        let names = action_names();
        assert!(!names.is_empty());
        let unique: BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "action_names() has duplicates");
    }

    #[test]
    fn every_name_parses_via_action_from_name() {
        // Drive the dispatcher with each variant's own serialised params,
        // guaranteeing name()/serde-tag parity end to end.
        for action in all_variants() {
            let mut value = serde_json::to_value(&action).expect("serialises");
            let obj = value.as_object_mut().expect("tagged object");
            obj.remove("action");
            let params = serde_json::Value::Object(obj.clone());
            let parsed = action_from_name(action.name(), &params)
                .unwrap_or_else(|e| panic!("{} should parse: {e}", action.name()));
            assert_eq!(parsed, action);
        }
    }

    #[test]
    fn parametrised_dispatch() {
        assert_eq!(
            action_from_name("resize_note", &json!({ "delta_steps": 2 })).unwrap(),
            Action::ResizeNote { delta_steps: 2 }
        );
        assert_eq!(
            action_from_name("set_cursor", &json!({ "pitch": 60, "step": 4 })).unwrap(),
            Action::SetCursor { pitch: 60, step: 4 }
        );
    }

    #[test]
    fn nullary_dispatch_accepts_empty_or_null_params() {
        assert_eq!(
            action_from_name("add_note", &json!({})).unwrap(),
            Action::AddNote
        );
        assert_eq!(
            action_from_name("add_note", &serde_json::Value::Null).unwrap(),
            Action::AddNote
        );
    }

    #[test]
    fn unknown_name_is_rejected() {
        let err = action_from_name("frobnicate", &json!({})).unwrap_err();
        assert_eq!(err, ActionError::UnknownAction("frobnicate".to_string()));
    }

    #[test]
    fn wrong_param_type_is_bad_params() {
        let err = action_from_name("resize_note", &json!({ "delta_steps": "lots" })).unwrap_err();
        match err {
            ActionError::BadParams { action, .. } => assert_eq!(action, "resize_note"),
            other => panic!("expected BadParams, got {other:?}"),
        }
    }

    #[test]
    fn non_object_params_is_bad_params() {
        let err = action_from_name("add_note", &json!([1, 2, 3])).unwrap_err();
        match err {
            ActionError::BadParams { action, .. } => assert_eq!(action, "add_note"),
            other => panic!("expected BadParams, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_param_is_bad_params() {
        let err = action_from_name("set_cursor", &json!({ "pitch": 60 })).unwrap_err();
        assert!(matches!(err, ActionError::BadParams { .. }));
    }

    #[test]
    fn wait_mode_actions_round_trip_via_name() {
        // Nullary toggle dispatches from empty/null params.
        assert_eq!(
            action_from_name("toggle_wait_mode", &json!({})).unwrap(),
            Action::ToggleWaitMode
        );
        // Parametrised set dispatches both polarities.
        assert_eq!(
            action_from_name("set_wait_mode", &json!({ "on": true })).unwrap(),
            Action::SetWaitMode { on: true }
        );
        assert_eq!(
            action_from_name("set_wait_mode", &json!({ "on": false })).unwrap(),
            Action::SetWaitMode { on: false }
        );
    }

    #[test]
    fn set_wait_mode_rejects_missing_or_mistyped_on() {
        // Missing `on`.
        assert!(matches!(
            action_from_name("set_wait_mode", &json!({})).unwrap_err(),
            ActionError::BadParams { .. }
        ));
        // Mistyped `on` (number instead of bool).
        match action_from_name("set_wait_mode", &json!({ "on": 1 })).unwrap_err() {
            ActionError::BadParams { action, .. } => assert_eq!(action, "set_wait_mode"),
            other => panic!("expected BadParams, got {other:?}"),
        }
    }

    #[test]
    fn nudge_backing_offset_round_trips_via_name() {
        // Both signs dispatch from their JSON params.
        assert_eq!(
            action_from_name("nudge_backing_offset", &json!({ "delta_us": 10_000 })).unwrap(),
            Action::NudgeBackingOffset { delta_us: 10_000 }
        );
        assert_eq!(
            action_from_name("nudge_backing_offset", &json!({ "delta_us": -250_000 })).unwrap(),
            Action::NudgeBackingOffset { delta_us: -250_000 }
        );
        // Missing param is rejected.
        assert!(matches!(
            action_from_name("nudge_backing_offset", &json!({})).unwrap_err(),
            ActionError::BadParams { .. }
        ));
    }

    #[test]
    fn bpm_actions_round_trip_via_name() {
        assert_eq!(
            action_from_name("adjust_bpm", &json!({ "delta": 5 })).unwrap(),
            Action::AdjustBpm { delta: 5 }
        );
        assert_eq!(
            action_from_name("adjust_bpm", &json!({ "delta": -10 })).unwrap(),
            Action::AdjustBpm { delta: -10 }
        );
        assert_eq!(
            action_from_name("set_bpm", &json!({ "bpm": 90 })).unwrap(),
            Action::SetBpm { bpm: 90 }
        );
        // Missing params are rejected.
        assert!(matches!(
            action_from_name("adjust_bpm", &json!({})).unwrap_err(),
            ActionError::BadParams { .. }
        ));
        assert!(matches!(
            action_from_name("set_bpm", &json!({})).unwrap_err(),
            ActionError::BadParams { .. }
        ));
    }

    #[test]
    fn hand_actions_round_trip_via_name() {
        assert_eq!(
            action_from_name("set_hand_split", &json!({ "pitch": 55 })).unwrap(),
            Action::SetHandSplit { pitch: 55 }
        );
        for (wire, setting) in [
            ("auto", HandSetting::Auto),
            ("left", HandSetting::Left),
            ("right", HandSetting::Right),
        ] {
            assert_eq!(
                action_from_name("set_note_hand", &json!({ "hand": wire })).unwrap(),
                Action::SetNoteHand { hand: setting }
            );
        }
        assert_eq!(
            action_from_name("cycle_note_hand", &json!({})).unwrap(),
            Action::CycleNoteHand
        );
        // Missing / unknown params are rejected rather than silently defaulted.
        assert!(matches!(
            action_from_name("set_hand_split", &json!({})).unwrap_err(),
            ActionError::BadParams { .. }
        ));
        assert!(matches!(
            action_from_name("set_note_hand", &json!({ "hand": "both" })).unwrap_err(),
            ActionError::BadParams { .. }
        ));
    }

    #[test]
    fn effect_round_trips() {
        let effects = [
            Effect::AuditionNote {
                pitch: 60,
                velocity: 80,
            },
            Effect::AuditionChord {
                pitches: vec![60, 64, 67],
            },
            Effect::AllOff,
        ];
        for effect in effects {
            let value = serde_json::to_value(&effect).expect("serialises");
            let back: Effect = serde_json::from_value(value).expect("deserialises");
            assert_eq!(effect, back);
        }
    }

    #[test]
    fn action_error_displays() {
        assert_eq!(
            ActionError::UnknownAction("foo".to_string()).to_string(),
            "unknown action: foo"
        );
        assert_eq!(
            ActionError::BadParams {
                action: "resize_note".to_string(),
                detail: "nope".to_string(),
            }
            .to_string(),
            "bad params for action `resize_note`: nope"
        );
    }
}
