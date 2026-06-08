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

    // ── loop / metronome / count-in ─────────────────────────────────────
    ToggleLoop,
    ToggleMetronome,
    StartCountInRecord,
    SetLoopBounds {
        start_us: u64,
        end_us: u64,
    },

    // ── selection / clipboard ───────────────────────────────────────────
    StartSelection,
    ClearSelection,
    YankSelection,
    PasteClipboard,
    DeleteSelection,

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
            Action::ToggleLoop => "toggle_loop",
            Action::ToggleMetronome => "toggle_metronome",
            Action::StartCountInRecord => "start_count_in_record",
            Action::SetLoopBounds { .. } => "set_loop_bounds",
            Action::StartSelection => "start_selection",
            Action::ClearSelection => "clear_selection",
            Action::YankSelection => "yank_selection",
            Action::PasteClipboard => "paste_clipboard",
            Action::DeleteSelection => "delete_selection",
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
        "toggle_loop",
        "toggle_metronome",
        "start_count_in_record",
        "set_loop_bounds",
        "start_selection",
        "clear_selection",
        "yank_selection",
        "paste_clipboard",
        "delete_selection",
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
        // ── loop / metronome / count-in ─────────────────────────────────
        ActionInfo { name: "toggle_loop", params: &[], description: "Toggle looped playback over the loop region." },
        ActionInfo { name: "toggle_metronome", params: &[], description: "Toggle the metronome click." },
        ActionInfo { name: "start_count_in_record", params: &[], description: "Begin a metronome count-in, then start recording." },
        ActionInfo { name: "set_loop_bounds", params: &[p("start_us", "u64"), p("end_us", "u64")], description: "Set the loop region to [start_us, end_us) microseconds." },
        // ── selection / clipboard ───────────────────────────────────────
        ActionInfo { name: "start_selection", params: &[], description: "Begin a selection rectangle anchored at the cursor." },
        ActionInfo { name: "clear_selection", params: &[], description: "Clear the active selection." },
        ActionInfo { name: "yank_selection", params: &[], description: "Copy the selected notes to the clipboard." },
        ActionInfo { name: "paste_clipboard", params: &[], description: "Paste the clipboard at the cursor." },
        ActionInfo { name: "delete_selection", params: &[], description: "Delete the notes inside the selection." },
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
            Action::ToggleLoop,
            Action::ToggleMetronome,
            Action::StartCountInRecord,
            Action::SetLoopBounds {
                start_us: 0,
                end_us: 1_000,
            },
            Action::StartSelection,
            Action::ClearSelection,
            Action::YankSelection,
            Action::PasteClipboard,
            Action::DeleteSelection,
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
