# M27-C — play: show finger numbers on the highway (desktop + TUI)

> Milestone: M27 — Finger numbers · Issue: #372 · Suggested tier: sonnet
> Branch: `claude/m27-finger-play` · Depends on: M27-A (#370)

## Goal

While playing a piece, each note that has a finger number shows it on the
highway, so the player can read the fingering as the note approaches. Notes
without one look exactly as they do today. Display only: scoring, wait mode and
practice are untouched.

## Context

- M27-A persists fingers in `meta.json` as `fingers: Vec<NoteFinger>` keyed by
  `(pitch, start_us)`.
- There is no shared play crate; each frontend loads `meta.json` itself and
  matches hand overrides against the span's start **before** its own pre-roll
  shift. Fingers follow the same path:
  - Desktop: `tauri-app/src-tauri/src/play.rs` — `NoteSpan`, `with_hands`,
    `load_session_from_dir`, `SpanView`, `PlayInfo`; webview
    `tauri-app/src/screens/highway/` — `liveSong.ts` (`songFromInfo`),
    `types.ts` (`NoteSpan`), `HighwayCanvas.ts` (`drawNote`).
  - TUI: `crates/tui/src/play.rs` — `PlayScreen` keeps a parallel
    `hands: Vec<Hand>` beside `highway::NoteSpan`; loaded in
    `load_play_screen` (`crates/tui/src/app.rs`).
- On the desktop, play always runs with `labels: true`: `drawNote` writes the
  pitch-class name near the onset edge of notes taller than 16 px. The finger
  digit has to share that space.
- In the TUI, a note is drawn as block glyphs (`cells_of_extent`: `Full`,
  `Bottom(n)`, `Top(n)`); notes carry no text today.

## What to do

### C1 — desktop backend (`tauri-app/src-tauri/src/play.rs`)

- `NoteSpan` gains `finger: Option<Finger>` (`None` at every `build_spans`
  site).
- `with_fingers(mut self, fingers: &[NoteFinger]) -> Self`, matching exactly as
  `with_hands` does (`span.start_us.saturating_sub(shift_us)`), called from
  `load_session_from_dir` right after `with_hands`.
- `SpanView` gains `#[serde(skip_serializing_if = "Option::is_none")] finger:
  Option<Finger>`; `info()` fills it.

### C2 — desktop highway (`tauri-app/src/`)

- `ipc/types.ts`: `PlaySpan.finger?: number`. `highway/types.ts`:
  `NoteSpan.finger?: number`. `songFromInfo` passes it through.
- Decide what text a note shows in a pure helper (new `noteLabels.ts`), unit
  tested, and have `drawNote` draw what it returns:

  | note height | finger set | drawn |
  | --- | --- | --- |
  | any | no | exactly today's behaviour (name if `labels` and height > 16 px) |
  | > 32 px | yes | finger at the onset edge, name one line further from it |
  | 12–32 px | yes | finger only, at the onset edge |
  | < 12 px | yes | finger just outside the note beyond its tail, in the note's colour |

  The finger is drawn bolder and slightly larger than the name so the two are
  not confused. Thresholds are a starting point — tune by eye, keep the helper
  and its tests in step.
- The finger shows regardless of `labels` and of the colour mode.
- A note greyed out because the other hand is being practised, or faded because
  it lies outside the practice loop, shows its digit with the same dimming.

### C3 — TUI (`crates/tui/src/play.rs`, `app.rs`)

- `PlayScreen` gains a parallel `fingers: Vec<Option<Finger>>` (all `None` from
  `from_smf_bytes`) and `with_fingers(mut self, fingers: &[NoteFinger]) ->
  Self`, matching on the pre-shift start like `with_hands`. `load_play_screen`
  calls it.
- In `draw_highway`, for a fingered note draw the digit in the cell at the
  note's **onset** end (the end that reaches the keyboard first), in the centre
  column of the lane: dark foreground on the note's colour as background. If
  the onset cell is a partial block, draw the digit there anyway (full-cell
  background) — a readable digit matters more than the sub-cell edge.
- Dimming for other-hand and outside-loop notes applies to the digit's
  background the same way it applies to the note.

## Tests

- Desktop Rust (mirror the hand tests in `play.rs`):
  `with_fingers_matches_the_pre_shift_position`;
  `play_info_carries_the_finger`; a span with no finger serialises without a
  `finger` key; a bundle with no fingers produces a `PlayInfo` identical to
  today's.
- Scoring is unaffected: an existing scoring test, re-run on a session with
  fingers applied, gives the same judgments.
- Vitest: `songFromInfo` passes the finger through and leaves it undefined when
  absent; the label helper for each row of the table above.
- TUI: `with_fingers` applies fingers at the file (pre-shift) position; a
  render test where a fingered note shows its digit at the onset cell and an
  unfingered note in the next lane shows only blocks; a one-column black-key
  lane still shows the digit.
- Loading a bundle through `load_play_screen` with fingers in `meta.json` gives
  a screen whose `fingers` are set (no test goes through that function with
  overrides today — add one that covers hands and fingers).

## Scope boundaries (do NOT)

- No setting or key to hide finger numbers; a note with none already shows
  none. (A toggle can come later if the numbers prove distracting.)
- Do not change scoring, wait mode, practice-hand logic, triggers or audio.
- Do not show the finger on the keyboard strip — highway notes only.
- Do not add a shared play crate or move the existing play logic.
- No third-party dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `npm run check` (tsc + vitest) green in `tauri-app/`
- [ ] PR opened against `main` from the branch above, `Closes #372`
