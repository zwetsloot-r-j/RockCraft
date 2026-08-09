# M14-E — edit/play: per-note hand assignment (split line + manual exceptions)

> Milestone: M14 — Play-screen polish · Issue: #266 · Suggested tier: opus
> Branch: `claude/note-hand-override`

## Goal

Let a piece decide which hand plays each note. The **default** is the existing
pitch **split line** (notes below it are the left hand, at/above are the right);
on top of that, the author can **mark individual notes as exceptions** in edit
mode, and those overrides persist in the bundle. Everything that already keys off
"hand" — hand-practice grey-out, one-hand scoring, colouring — reads the
*effective* hand (override, else the split), so a crossover note is finally
practised and scored on the correct hand.

`core` owns the model and the effective-hand rule (pure, headless-testable); both
frontends render/consume it and the webview edits it through pure `Action`s.

## Context

- Today hand is **inferred from pitch only**, at play time, by
  `hand_of(pitch, split)` in `tauri-app/src-tauri/src/play.rs` (the `Hand` enum
  at `play.rs:246`, `DEFAULT_SPLIT: u8 = 60` at `:253`). It is not persisted and
  there is no per-note memory — `play.rs:244` says so outright. This spec makes
  hand a first-class, persisted note property and **promotes the `Hand` type and
  the split rule into `core`** so edit, play, and both frontends share one
  definition.
- The editable note is `core::timeline::Note` (`crates/core/src/timeline.rs:36`);
  notes carry a stable in-session `NoteId` (`timeline.rs:24`) but **that id is
  reassigned on reload** (`from_events`, `timeline.rs:222`) and `song.mid`
  carries no per-note metadata. So overrides persist in `meta.json`
  (`RecordingMeta`, `crates/core/src/song.rs:71`) **keyed by musical position**
  `(pitch, start_us)`, which *is* stable — it is exactly the key `from_events`
  sorts by, and `(pitch, start_us)` is unique.
- Edit is cursor-driven: actions act on `note_under_cursor()`
  (`composer.rs:844`) or on the current region selection (`selection_ids()`,
  `composer.rs:874`). New edit ops are pure `core::Action`s and auto-wire to
  every frontend and the control socket via `action_from_name` / `action_help`
  (parity tests in `action.rs` enforce the catalog).
- `RecordingMeta` grows two `#[serde(default)]` fields, the established pattern
  for new persisted data (see M14-D `backgrounds`) so every legacy bundle still
  parses.
- We are **not** encoding hand as a MIDI channel/track. External piano MIDI uses
  that convention only loosely (it is a soft two-*track* convention, not a
  standard; multi-instrument files use channels for instruments), so it cannot be
  trusted on import and would be a much larger change. Overrides-on-split is the
  primary model; a channel/track import heuristic can be a later, separate spec.

## What to do

### E1 — `core` model + effective-hand rule (`crates/core/src/hand.rs`, new)

```rust
pub enum Hand { Left, Right }                 // Copy, Eq, serde
pub enum HandSetting { Auto, Left, Right }    // Copy, Eq, serde — the edit param

/// Default split: pitch < split → Left, pitch >= split → Right. Middle C.
pub const DEFAULT_SPLIT: u8 = 60;

pub fn hand_of(pitch: MidiNote, split: u8) -> Hand;   // the split rule, one place
```

- `HandSetting::override_value() -> Option<Hand>`: `Auto → None`, `Left/Right →
  Some(_)`. `Hand::setting()` is the inverse for round-tripping the snapshot.

On the note (`crates/core/src/timeline.rs`):

```rust
pub struct Note { pub pitch, pub start_us, pub dur_us, pub velocity,
                  pub hand: Option<Hand> }   // None = follow the split line
```

- `Note::effective_hand(&self, split: u8) -> Hand =
  self.hand.unwrap_or_else(|| hand_of(self.pitch, split))`.
- Every existing `Note` construction defaults `hand` to `None` (the
  `Timeline::insert` path, `from_events`, any struct literal / test builder).
  `hand` is **not** written to `song.mid` (events drop it) — it round-trips only
  through `meta.json` (E1b).

Timeline helpers (`timeline.rs`):

- `set_hand(&mut self, id: NoteId, hand: Option<Hand>) -> bool` — set/clear the
  override on one note; returns whether the id existed.
- `hand_overrides(&self) -> Vec<HandOverride>` — every note with
  `hand.is_some()`, as `{ pitch, start_us, hand }`, sorted by `(start_us,
  pitch)`.
- `apply_hand_overrides(&mut self, &[HandOverride])` — for each entry, find the
  note at exactly `(pitch, start_us)` (reuse `find_at`-style lookup) and set its
  hand; silently ignore entries with no matching note (a note deleted since the
  save).

### E1b — persistence (`crates/core/src/song.rs`)

```rust
pub struct HandOverride { pub pitch: u8, pub start_us: u64, pub hand: Hand }

// on RecordingMeta:
#[serde(default)] pub hand_split: Option<u8>,        // None → DEFAULT_SPLIT
#[serde(default)] pub hand_overrides: Vec<HandOverride>,
```

- `RecordingMeta::split_or_default() -> u8 = self.hand_split.unwrap_or(DEFAULT_SPLIT)`.
- Save path (frontend host layer): populate `hand_split` from the composer's
  split and `hand_overrides` from `timeline.hand_overrides()`.
- Load path: after building the timeline from events, call
  `timeline.apply_hand_overrides(&meta.hand_overrides)` and set the composer's
  split from `meta.split_or_default()`.

### E2 — edit-mode actions (`core::Action`, pure)

The split line becomes an authored, persisted property of the piece; overrides
are cursor/selection edits.

| action | params | effect |
| --- | --- | --- |
| `set_hand_split` | `pitch: u8` | set the piece's split line |
| `set_note_hand` | `hand: HandSetting` | set the override on the **selection** if one is active, else the note under the cursor |
| `cycle_note_hand` | — | cycle that same target `Auto → Left → Right → Auto` (a one-key convenience; uses the target note's current setting) |

- `set_note_hand` / `cycle_note_hand` are **no-ops (never errors)** when there
  is no selection and no note under the cursor. On a selection they set every
  selected note to the same setting.
- Add each variant to `Action`, `name()`, `action_names()`, `action_help()`
  (parity tests enforce all three) and dispatch in `Composer::apply`.
- `ComposerSnapshot` gains `hand_split: u8`; `NoteView` gains
  `hand: Option<Hand>` (the raw override — the frontend derives effective from
  `pitch` + `hand_split`). Mirror both in `tauri-app/src/ipc/types.ts`.

### E3 — play-side consumption (`tauri-app/src-tauri/src/play.rs`)

- Replace the local `Hand` / `hand_of` / `DEFAULT_SPLIT` with the `core` ones.
- The play span gains `hand: Option<Hand>`. `load_session_from_dir` already
  reads `meta`; thread `meta.hand_overrides` and `meta.split_or_default()` into
  the session so each span's `hand` is set (match overrides by the note's
  **original** `(pitch, start_us)`, before the pre-roll shift), and the session's
  `split_pitch` defaults from the meta split.
- Everywhere hand is currently computed as `hand_of(span.note, self.split_pitch)`
  (the practice grey-out at `play.rs:704`/`:899`, `spans_for` at `:343`,
  `expected_*` helpers) use `span.effective_hand(self.split_pitch)` instead.
- `play_set_split` still lets the player retune the split live; `PlayInfo` notes
  carry the **effective** hand so the highway can colour without re-deriving.

### E4 — rendering (both frontends)

**Tauri edit** (`EditScreen.tsx` / `EditCanvas.ts`):

- Draw the split line across the grid at `snapshot.hand_split`.
- Tint each note by its **effective** hand (override ?? split) and give
  overridden notes a distinct mark (e.g. a small badge / outline) so exceptions
  are visible at a glance.
- Keys (keep clear of existing bindings; document in the help overlay): a key to
  `cycle_note_hand` on the cursor note / selection, and `Shift+`those to nudge
  `set_hand_split` up/down a semitone. Exact keys the implementer's choice.

**Tauri play** (`liveSong.ts`): use the effective hand from `PlayInfo`
(`songFromInfo` currently hardcodes a middle-C split at `liveSong.ts:12` — drop
that and read the note's hand). Hand-practice grey-out already consumes hand and
now follows overrides for free.

**TUI**: the new actions are pure and auto-wire, so they are callable in the TUI
already. Rendering the hand in the TUI grid is **optional/minimal** — at least do
not regress; a colour/marker is a nice-to-have, not required.

## Tests

- `hand_of`: below split → Left, at split → Right, above → Right; a custom split.
- `HandSetting` ↔ `Option<Hand>` round-trip; `Note::effective_hand` returns the
  override when set, else the split-derived hand.
- Timeline: `set_hand` sets/clears and returns id-existed; `hand_overrides`
  lists only overridden notes, sorted; `apply_hand_overrides` matches by
  `(pitch, start_us)` and ignores entries with no matching note.
- `RecordingMeta` round-trips with and without the new fields; a legacy
  `meta.json` (no `hand_split`, no `hand_overrides`) parses to `None` / empty,
  and `split_or_default()` yields `DEFAULT_SPLIT`.
- Composer: `set_note_hand` with no selection + no cursor note is a no-op; with
  a note under the cursor it sets that note's override; with an active selection
  it sets all selected notes; `cycle_note_hand` walks `Auto→Left→Right→Auto`;
  `set_hand_split` updates the snapshot; the snapshot exposes `hand_split` and
  `NoteView.hand`.
- Save→load round-trip: mark exceptions + a non-default split, save, reload,
  and the same notes carry the same overrides and the split is preserved;
  a note moved after being overridden either keeps its override in-session
  (rides the `NoteId`) — assert that — and, once saved at its new position and
  reloaded, is re-matched by the new `(pitch, start_us)`.
- Play: a span whose override crosses the split is practised/scored on the
  overridden hand — with `practice = Some(overridden hand)` it is expected and
  scored, and with the other hand it is auto-played (not scored); a MIDI-only
  bundle with no overrides behaves exactly as today.
- `action.rs` parity tests cover the three new variants automatically.
- Vitest: `songFromInfo` maps `PlayInfo` hand through; the edit note tint/badge
  chosen from `(hand, split)`.

## Scope boundaries (do NOT)

- Do not encode hand as a MIDI channel or track, and do not change
  `events_to_smf_bytes` / `smf_bytes_to_events` or the import writer — hand
  never touches `song.mid`.
- Do not add an import-time hand heuristic (sniffing external files' tracks) —
  that is a separate, later spec.
- Do not add third-party dependencies (Rust or npm).
- Do not add finger numbers, per-hand instruments/voicing, or more than two
  hands.
- Do not break the existing pitch-only default: a bundle with no overrides and
  no `hand_split` must play, score, and colour exactly as it does today.
- Keep `core` pure — no I/O in `hand.rs` / `timeline.rs`; save/load wiring lives
  in the frontend host layer using the core helpers.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `npm run check` (tsc + vitest) green in `tauri-app/`
- [ ] PR opened against `main` from the branch above, `Closes #266`
