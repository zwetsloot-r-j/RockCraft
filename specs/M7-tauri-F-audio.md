# M7-tauri-F-audio — Synth audition, backing playback, metronome clicks

> Milestone: M7 · Issue: #166 · Suggested tier: sonnet
> Branch: `claude/tauri-audio`
> Depends on: M7-tauri-A-ipc-bridge (#161)

## Goal

Make the Tauri app sound like the TUI: `Effect`s from the composer drive the
synth, bundles' backing tracks follow the transport with the alignment
offset, and the metronome clicks. All through `crates/audio` in the backend —
the webview never touches audio.

## Context

- `crates/audio` — `AudioOut::new()` (may fail: no device in CI/sandbox),
  `AudioOut::synth() -> SynthHandle` (`note_on/note_off/all_off`),
  `play_file_at(path, pos) -> BackingHandle`
  (`pause/resume/set_paused/seek/stop`).
- Effects (`core::action::Effect`): `AuditionNote { pitch, velocity }`,
  `AuditionChord { pitches }`, `AllOff`. Metronome clicks already arrive as
  `AuditionNote { pitch: 36, velocity: 80 }` from `Composer::advance()` —
  sounding effects is sufficient, there is no separate metronome path.
- TUI reference for transport-coupled backing: `crates/tui/src/edit.rs`
  (`poll_backing`, nudge handling) — backing position = playhead +
  `snapshot.backing_offset_us`.
- `CLAUDE.md` invariant: never block the real-time audio path; the tick
  thread hands work to audio handles, no disk I/O on it.

## What to do

### Backend `tauri-app/src-tauri/src/audio.rs`

```rust
pub struct AudioState {
    out: Option<AudioOut>,        // None when no device — every op a no-op
    synth: Option<SynthHandle>,
    backing: Mutex<Option<BackingSession>>,
}
struct BackingSession {
    path: PathBuf,
    handle: Option<BackingHandle>, // started lazily on play
}
```

- Initialise once at setup; `AudioOut::new()` failure logs a warning and
  leaves `out: None` (the app must be fully usable silent — CI, sandboxes).
- Route every `Effect` (from `run_action` replies *and* the #161 tick loop)
  through one function: `apply_effects(&AudioState, &[Effect])` — chord =
  `all_off` then `note_on` each pitch, like the TUI synth wiring.
- Sustained audition: `Composer::advance` emits the note-offs; no
  frontend-side timers.

### Backing transport coupling

In the tick loop (and on transport-affecting `run_action` replies):

- play started → `play_file_at(path, playhead + backing_offset_us)`
- pause → `set_paused(true)`; resume → `set_paused(false)`
- seek (`play_from_start`, `play {from_us}`, `set_playhead`) and
  `nudge_backing_offset` → `seek(playhead + offset)`

Commands for the frontend:

```rust
#[tauri::command]
fn attach_backing(state: …, path: String) -> Result<(), String>; // file exists?
#[tauri::command]
fn detach_backing(state: …);
#[tauri::command]
fn audio_status(state: …) -> AudioStatus; // { device: bool, backing: Option<String> }
```

The edit screen's "Choose backing track" menu item opens the native file
dialog (add `tauri-plugin-dialog`, filters `mp3/wav/ogg/flac` — the same
extensions as `crates/tui/src/backing.rs`) and calls `attach_backing`. The
status bar shows the backing file name and offset when attached.

## Tests

Headless, no device required (all must pass in CI):

- `apply_effects` with `out: None` is a no-op (doesn't panic).
- Backing position maths: pure helper
  `fn backing_pos(playhead_us: u64, offset_us: u64) -> Duration` unit-tested
  (0 + 0; nudges accumulating; large values).
- `attach_backing` rejects a missing path.

(Audible behaviour is verified locally — note this in the PR.)

## Scope boundaries (do NOT)

- Do not play audio from the webview (no HTML5 audio).
- Do not add new audio dependencies beyond `tauri-plugin-dialog`.
- Do not implement record-screen backing (#169) or play-screen pre-roll
  start (#168) — only the edit-transport coupling here.
- Do not regress headless: `cargo test --workspace` must pass with no
  sound device.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
      (in a machine with no audio device too)
- [ ] `npx tsc --noEmit` passes
- [ ] Local run: cursor moves audition notes, chord preview sounds, `M`
      clicks on beats, backing follows Space/`P` and `,`/`.` nudges
- [ ] PR opened against `main` from `claude/tauri-audio`, `Closes #166`
