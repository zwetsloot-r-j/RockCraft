# M7-tauri-G-midi-input — MIDI input service: live device + mock keyboard

> Milestone: M7 · Issue: #167 · Suggested tier: sonnet
> Branch: `claude/tauri-midi-input`
> Depends on: M7-tauri-A-ipc-bridge (#161)

## Goal

A backend MIDI input service: a real USB-MIDI device when present, the
computer-keyboard mock otherwise, streaming `NoteEvent`s to the webview and
into the composer — so StepRecord/LiveRecord and (later) play/record screens
work identically to the TUI, and everything is drivable in a sandbox.

## Context

- `crates/midi` — `LiveInput::connect(name_filter)` (midir),
  `LiveInput::events()` (drained iterator), `port_name()`;
  `mock::MockKeyboard` + `mock::key_map()` (computer key → MIDI note, the
  TUI dev mapping); `mock::ScriptedSource` for tests; `NoteSource` trait.
- `Composer::ingest(NoteEvent) -> Vec<Effect>` — how MIDI reaches the
  composer (input-mode behaviour lives in core; the service just feeds it).
- `CLAUDE.md` invariant: never block the MIDI callback thread — `LiveInput`
  already buffers via channel; keep it that way.
- Scoring rule: judgments use `NoteEvent::timestamp_us`, never UI time.

## What to do

### Backend `tauri-app/src-tauri/src/midi.rs`

```rust
pub enum InputSource { Live(LiveInput), Mock(MockKeyboard), None }
pub struct MidiState { source: Mutex<InputSource>, /* … */ }
```

- At setup, try `LiveInput::connect("")` (any port); on failure fall back to
  `InputSource::Mock`. Expose:

```rust
#[tauri::command]
fn midi_status(…) -> MidiStatus;   // { kind: "live"|"mock", port: Option<String> }
#[tauri::command]
fn mock_key(…, key: char, down: bool) -> Result<(), String>;
// down=true → MockKeyboard::press → NoteEvent::On; release → Off.
// Err when source is Live (mock keys must not fake events on real input).
```

- Drain loop: on the existing #161 tick thread, drain pending events each
  tick; for each event (1) emit webview event `"midi_event"` with the
  serialized `NoteEvent`, (2) `composer.ingest(event)` and route the
  returned effects like any others (sound via #166 if merged; emit
  `"effects"` regardless).

### Frontend

- `src/ipc/midi.ts`: `onMidiEvent(cb)`, `mockKey(key, down)`,
  `midiStatus()`.
- Global capture in the shell (#162 router): when the source is mock and the
  focused screen wants instrument input (edit in Step/LiveRecord — later
  play/record), forward mapped keydown/keyup to `mock_key`. Use
  `mock::key_map()`'s keys (duplicate the char list in TS with a comment
  pointing at `crates/midi/src/mock.rs::key_map` as source of truth).
  Suppress auto-repeat (`event.repeat`).
- A small status chip (shell-level, bottom corner): green dot + port name
  when live, `MOCK` badge when mock — the record screen header (#24) shows
  its own chip; this one is for every other screen.

## Tests

- Rust: service-level test with `ScriptedSource`/`MockKeyboard` — feed
  press/release, assert a StepRecord composer received the notes (snapshot
  gains notes at the cursor) and `midi_event` payloads serialize with
  `timestamp_us` intact.
- `mock_key` on a Live source returns `Err`.
- No test may require hardware; real-device verification is local
  (note it in the PR).

## Scope boundaries (do NOT)

- Do not implement play/record screen behaviour (#168/#169) — only the
  service, ingest routing, and the status chip.
- Do not add hot-replug/device-picker UI (TUI doesn't have one either).
- Do not block or sleep in the MIDI callback path.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] `npm run dev` (no device): status chip shows `MOCK`; in the edit
      screen with `R` (StepRecord), mapped computer keys place notes
- [ ] `midi_event`s observable in the webview console (documented snippet)
- [ ] PR opened against `main` from `claude/tauri-midi-input`, `Closes #167`
