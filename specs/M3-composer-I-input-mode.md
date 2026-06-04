# M3-I — tui: input-mode toggle (direct-edit ↔ live / step-record)

> Milestone: M3 — Composer · Issue: #57 · Suggested tier: opus
> Branch: `claude/m3-input-mode`

## Goal

Let the user swap how notes get into the editor: **direct edit** (cursor +
keys, #53) versus **playing them in** from the input source (piano or mock
keyboard). Two play-in flavours: **step-record** (each played note lands at the
cursor and the cursor advances one step — no transport needed) and, building on
the transport (#59), **real-time record** into the timeline at the playhead.

## Context

- Crate: `crates/tui`, extends `edit.rs` and `app.rs` routing.
- The input seam already exists: `Shell` owns `Box<dyn NoteSource>` and drains
  `events()` each frame (`app.rs` run loop). Today those events feed Record/Play;
  this task feeds them to `EditScreen::ingest(ev)` when the editor is in a
  record mode.
- `MockKeyboard::forward_key` already turns typed letters into `NoteEvent`s, so
  this is fully testable headlessly (type letters → notes land in the timeline).

## What to do

- Add an input-mode enum to `EditScreen`: `DirectEdit | StepRecord | LiveRecord`,
  shown in the status line. A key cycles it (e.g. `R` = toggle record arm; while
  armed, `Tab`-free key like `t` toggles step vs live). Keep navigation keys
  working in all modes; document the mode-specific key precedence in a comment
  (mirror the precedence comment in `app.rs`).
- `EditScreen::ingest(&mut self, ev: NoteEvent)`:
  - **StepRecord:** on a note-on, insert a note at the cursor pitch?/played
    pitch at the cursor step (default one-step duration) and advance the cursor
    one step. (Use the *played* pitch, not the cursor pitch, so a piano plays the
    melody in.) Note-offs may set the just-inserted note's duration if held
    across steps — keep v1 simple: fixed one-step length, document it.
  - **LiveRecord:** while the transport (#59) is running, write incoming
    on/off into the timeline at the current playhead µs (snapped to grid),
    pairing on→off into a `Note` like `Timeline::from_events`.
- Wire `app.rs`: in the run-loop event drain, route events to
  `Screen::Edit(edit) => edit.ingest(ev)` (and sound them via the synth, as
  Record does).

## Tests (headless, `MockKeyboard` / synthetic events)

- In StepRecord, forwarding three mapped note keys inserts three notes at
  consecutive steps with the played pitches; the cursor advanced three steps.
- DirectEdit ignores `ingest` note input for placement (cursor keys still edit).
- Mode cycling updates the reported mode; navigation keys unaffected by mode.
- (LiveRecord timing test may depend on #59; gate that case behind it or use a
  manually-advanced playhead seam.)

## Scope boundaries (do NOT)

- Do not build the transport here (that is #59) — LiveRecord may use a playhead
  seam/stub if #59 isn't merged; StepRecord must work standalone.
- Do not add metronome/count-in (that is #64).
- Do not change `NoteSource`/`MockKeyboard`; consume them.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-input-mode`, `Closes #57`
