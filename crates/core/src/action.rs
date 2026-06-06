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
