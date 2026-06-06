# M4-A — core: Action / Effect model + generic name dispatch

> Milestone: M4 — Agent Interface · Issue: #85 · Suggested tier: opus
> Branch: `claude/m4-action-model`

## Goal

Define the transport-agnostic vocabulary of editor operations as a pure `core`
type: an `Action` enum (every composer operation), an `Effect` enum (side
effects a frontend must carry out, e.g. audition), and an `ActionError`. Add
string-name (de)serialisation so a remote `run_action: { name, params }` maps to
an `Action` generically — new actions become callable with no transport change.

## Context

- Crate: `crates/core`, new module `action.rs` (re-export from `lib.rs`).
- This is the contract M4-B (`Composer::apply`), M4-C (TUI keymap), and M4-D
  (WebSocket protocol) all build on. Get the taxonomy right; it is the public
  surface.
- The catalog mirrors the operations currently hard-wired into the TUI keymap in
  `crates/tui/src/edit.rs::on_key` — read that for the complete behaviour set
  (navigation, edit ops, grab, chord selector, input mode, transport, loop,
  metronome, count-in, selection/clipboard, undo/redo). Do **not** include
  view-only concerns (help overlay) or I/O (save/load) — those stay in the
  frontend / control layer.
- `serde` + `serde_json` are already workspace deps (used by `RecordingMeta`),
  so `core` may use them here. No new third-party deps.

## What to do

```rust
// crates/core/src/action.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    // navigation
    CursorLeft, CursorRight, CursorUp, CursorDown,
    CursorBarLeft, CursorBarRight, CursorOctaveDown, CursorOctaveUp,
    CursorToStart, CursorToEnd,
    SetCursor { pitch: u8, step: u64 },          // absolute jump (AI-friendly)
    SubdivisionFiner, SubdivisionCoarser,
    // edit
    AddNote, DeleteNote,
    ResizeNote { delta_steps: i64 },
    AdjustVelocity { delta: i16 },
    ToggleGrab,
    // chord selector
    EnterChordMode, CommitChord, CancelChord, ToggleChordKind,
    SetChordDegree { degree: u8 },
    CycleChordDegree { delta: i8 },
    // input mode
    ToggleRecordArm, ToggleRecordFlavour,
    // transport (pure: time is injected, never wall-clock)
    TogglePlayCursor, PlayFromStart, Stop,
    Play { from_us: u64 },
    SetPlayhead { us: u64 },
    // loop / metronome / count-in
    ToggleLoop, ToggleMetronome, StartCountInRecord,
    SetLoopBounds { start_us: u64, end_us: u64 },
    // selection / clipboard
    StartSelection, ClearSelection, YankSelection, PasteClipboard, DeleteSelection,
    // history
    Undo, Redo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Sound one note now, stopping any prior audition. (pitch, velocity)
    AuditionNote { pitch: u8, velocity: u8 },
    /// Sound a chord now, stopping any prior audition.
    AuditionChord { pitches: Vec<u8> },
    /// Silence everything the frontend is auditioning.
    AllOff,
}

#[derive(Debug, Clone, PartialEq, thiserror-free; plain enum)]
pub enum ActionError {
    UnknownAction(String),
    BadParams { action: String, detail: String },
}

impl Action {
    /// Stable snake_case name, e.g. `Action::ResizeNote{..}.name() == "resize_note"`.
    pub fn name(&self) -> &'static str;
}

/// Build an `Action` from a remote `run_action` request. `params` is the JSON
/// object of named fields (may be null/empty for nullary actions).
pub fn action_from_name(name: &str, params: &serde_json::Value)
    -> Result<Action, ActionError>;

/// Every action name, for discovery (`query actions`) and self-documentation.
pub fn action_names() -> &'static [&'static str];
```

- Implement `action_from_name` by reconstructing the serde-tagged value
  (`{"action": name, ...params}`) and deserialising; map serde failure to
  `ActionError`. Confirm `name()` and the serde tag agree for every variant
  (a test enforces this against `action_names()`).
- `ActionError` should not pull in a new dep; implement `Display` + `Error` by
  hand (or derive only on std).

## Tests (core, headless)

- Round-trip: every variant serialises and deserialises to itself.
- `action_from_name("resize_note", {"delta_steps": 2})` ==
  `Action::ResizeNote { delta_steps: 2 }`; nullary `"add_note"` with empty/`null`
  params works; unknown name → `UnknownAction`; wrong param type → `BadParams`.
- `action_names()` is non-empty, unique, and every entry parses via
  `action_from_name` (nullary directly; parametrised with a minimal valid params
  object) — guarantees `name()`/serde tag parity so `run_action` can never name
  an action the dispatcher rejects.

## Scope boundaries (do NOT)

- No `Composer` or apply logic here — that is M4-B. This task is pure types +
  name dispatch only.
- No save/load/list, no help-overlay, no key codes. No transport/socket code.
- No new third-party deps (serde/serde_json only).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m4-action-model`, `Closes #85`
