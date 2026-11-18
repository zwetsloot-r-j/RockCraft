# M27-A — core: per-note finger number (model, persistence, edit actions)

> Milestone: M27 — Finger numbers · Issue: #370 · Suggested tier: opus
> Branch: `claude/m27-finger-model`

## Goal

Let a piece say which finger plays a note. Each note gets an **optional** finger
number, 1 (thumb) to 5 (little finger); the default is "none". The author sets
and clears it in edit mode through pure `Action`s, and it persists in the bundle.

This spec delivers the model, the persistence and the actions, wired through
both frontends' save/load so nothing is lost on a round trip. Showing the number
(M27-B edit, M27-C play), reading it from imports (M27-D, M27-E) and writing it
to sheet-music export (M27-F) build on it.

A finger number is **display-only**. It never affects scoring, wait mode,
practice-hand filtering or audio.

## Context

- This mirrors the per-note hand override (`specs/M14-E-per-note-hand-override.md`)
  almost exactly; follow that code path by path. The differences: there is no
  "split" default (no finger is simply `None`), and there is no cycle action.
- The editable note is `core::timeline::Note` (`crates/core/src/timeline.rs`),
  `Copy`, no `Default`; today `{ pitch, start_us, dur_us, velocity, hand }`.
- `song.mid` carries no per-note metadata and `NoteId`s are reassigned on
  reload, so — like `hand_overrides` — fingers persist in `meta.json`
  (`RecordingMeta`, `crates/core/src/song.rs`) keyed by `(pitch, start_us)`.
- Actions auto-wire to every frontend and the control socket through
  `action_from_name` / `action_help`; the parity tests in `action.rs` enforce
  the catalog.
- Undo/redo checkpoints whole `Timeline` clones (`crates/core/src/history.rs`),
  so a field on `Note` rides undo/redo with no extra code.
- The song archive (`crates/share/src/archive.rs`) packs `meta.json` bytes
  verbatim, so `.rcsong` export/import and LAN sharing carry fingers with no
  change.

## What to do

### A1 — the type (`crates/core/src/finger.rs`, new; re-export from `lib.rs`)

```rust
/// A piano finger number: 1 = thumb … 5 = little finger. Which hand it belongs
/// to comes from the note's effective hand, not from this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Finger(u8);

impl Finger {
    pub const fn new(n: u8) -> Option<Finger>;   // Some for 1..=5, else None
    pub const fn get(self) -> u8;
}
impl TryFrom<u8> for Finger { /* Err for 0 and 6.. */ }
impl From<Finger> for u8 { .. }

/// One persisted finger number, keyed by musical position like `HandOverride`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteFinger { pub pitch: u8, pub start_us: u64, pub finger: Finger }
```

On the wire a `Finger` is a bare number: `3`. `0`, `6`, `"3"` and `null` all
fail to deserialise as a `Finger`.

### A2 — on the note (`crates/core/src/timeline.rs`)

- `Note` gains `pub finger: Option<Finger>` (`None` = no number shown).
- Timeline helpers, same shape as the hand ones:
  - `set_finger(&mut self, id: NoteId, finger: Option<Finger>) -> bool` —
    returns whether the id existed.
  - `fingers(&self) -> Vec<NoteFinger>` — every note with a finger, sorted by
    `(start_us, pitch)`.
  - `apply_fingers(&mut self, fingers: &[NoteFinger])` — exact
    `(pitch, start_us)` match via `find_starting_at`; entries with no matching
    note are silently ignored.
- What happens to the finger on each edit — **the same as `hand` today**:

  | Operation | Finger |
  | --- | --- |
  | move, resize, quantise/snap, retempo, transpose (in place, same `NoteId`) | kept |
  | `adjust_velocity` (remove + re-insert with `..note`), `insert_shifted` | kept |
  | yank + paste (`ClipNote` gains `finger`) | kept |
  | `slice_segment` (`crates/core/src/segment.rs`) | kept |
  | `from_events` | `None` (re-applied from `meta.json`) |
  | `add_note`, `insert_run`, chord commit, step-record, live-record | `None` |
  | `chart_to_timeline` (`crates/import/src/parser.rs`) | `None` (M27-D fills it in) |

  Every other `Note { .. }` literal (tests included) gets `finger: None`.

### A3 — persistence (`crates/core/src/song.rs`)

```rust
// on RecordingMeta:
#[serde(default)] pub fingers: Vec<NoteFinger>,
```

- `new_midi_only` and every other `RecordingMeta { .. }` literal set it (empty
  for recordings and tests).
- **Lenient load:** a `meta.json` whose `fingers` list contains an entry with an
  out-of-range finger must still load — drop the bad entry, keep the rest. Use a
  `deserialize_with` helper on the field for this; do not let one bad entry
  fail the whole bundle.
- Save and load, in each frontend's host layer, right next to the hand lines:
  - TUI save `crates/tui/src/edit.rs` (`write_bundle`), TUI load
    `crates/tui/src/app.rs` (extend `load_meta_hands` or add a sibling).
  - Desktop save `tauri-app/src-tauri/src/state.rs` (`save_bundle_into` /
    `write_bundle`), desktop load `load_bundle`.
  - Split: `write_part_bundle` (`crates/import/src/writer.rs`) writes
    `fingers: sliced.timeline.fingers()`.
- **Dirty tracking.** The desktop's `timeline_fingerprint` /
  `timeline_fingerprint_snapshot` (`state.rs`) must include the finger, the
  same way `hand_code` does — otherwise a finger edit sends the notes-stripped
  `"meta"` event and the canvas never redraws. The TUI's
  `EditScreen::timeline_fingerprint` (`crates/tui/src/edit.rs`) currently
  hashes neither hand nor finger; add **both**, so a hand or finger edit marks
  the piece dirty there too.

### A4 — actions (`crates/core/src/action.rs`, dispatch in `composer.rs`)

| action | params | effect |
| --- | --- | --- |
| `set_note_finger` | `finger: Finger` | set the finger on the target |
| `clear_note_finger` | — | remove the finger from the target |

- **Target** is the same rule as `set_note_hand`: the selection when one is
  active, else the note under the cursor (reuse `hand_target_ids`, renamed if
  that reads better). With neither, the action is a no-op, never an error.
- One history checkpoint per action, so it undoes as one step. Do not
  checkpoint when there is no target.
- Add both variants to `Action`, `name()`, `action_names()`, `ACTION_HELP`
  (param type string `"Finger"`), `all_variants()` and the sample table in
  `action_help_every_param_set_dispatches` (`"Finger" => json!(1)`).
- `set_note_finger` with a missing or out-of-range `finger` is rejected by
  `action_from_name` (serde error), like a bad `hand`.
- `NoteView` gains `#[serde(default)] pub finger: Option<Finger>`; mirror it in
  `tauri-app/src/ipc/types.ts` as `finger?: number | null`, and add the two
  names to the `ActionName` union there.

### A5 — docs

- `docs/DEMO-SCENARIO.md`: add a beat that sets and clears a finger.
- `specs/M14-E-per-note-hand-override.md` says "Do not add finger numbers" in
  its scope list; leave it, it describes that task.

## Tests

- `Finger::new`: 1 and 5 → `Some`, 0 and 6 → `None`; serde round-trip as a bare
  number; `0`, `6`, `"3"` rejected. `NoteFinger` round-trip.
- Timeline: `set_finger` sets/clears and reports id-existed; `fingers()` lists
  only fingered notes, sorted; `apply_fingers` matches by `(pitch, start_us)`
  and ignores a stale entry; `insert_shifted` and `slice_segment` carry the
  finger.
- `RecordingMeta`: round-trip with fingers; a legacy `meta.json` (no `fingers`)
  parses to empty; a `fingers` list with one entry `"finger": 9` loads with that
  entry dropped and the others kept.
- Composer: both actions are no-ops with no target; set on the cursor note; set
  on a whole selection; clear; undo restores in one step; the finger rides a
  move and a transpose; yank + paste keeps it; a note re-entered with `add_note`
  in the same cell has none; `NoteView.finger` is in the snapshot.
- `action.rs`: parity tests pass; `finger_actions_round_trip_via_name` (mirrors
  `hand_actions_round_trip_via_name`), including rejection of a missing param
  and of `6`.
- Save → load round-trip in **both** frontends (mirror
  `hand_split_and_overrides_round_trip_through_a_saved_bundle` in `state.rs`;
  add the equivalent for the TUI): set fingers, save, reload, same notes carry
  the same fingers; a note moved then saved is re-matched at its new position.
- Split: a part bundle written by `write_part_bundle` keeps the fingers of the
  notes inside it, re-based to the part's time.
- Dirty: setting a finger marks the piece dirty in both frontends; in the TUI,
  `cycle_note_hand` now does too.
- A bundle with no fingers saves, loads, plays and scores exactly as today.

## Scope boundaries (do NOT)

- No rendering of the number anywhere (M27-B, M27-C) and no key bindings
  (M27-B).
- Do not touch `song.mid` encoding, `events_to_smf_bytes` or
  `smf_bytes_to_events`.
- Do not let the finger influence scoring, wait mode, practice-hand filtering
  or audio.
- No automatic fingering suggestion.
- No finger substitution (two fingers on one note) — one optional number only.
- No third-party dependencies.
- Keep `core` pure: no I/O in `finger.rs` / `timeline.rs`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `npm run check` (tsc + vitest) green in `tauri-app/`
- [ ] PR opened against `main` from the branch above, `Closes #370`
