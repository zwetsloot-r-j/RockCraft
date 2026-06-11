# M7-tauri-I-record-live — Record screen live wiring: takes, backing, save bundles

> Milestone: M7 · Issue: #169 · Suggested tier: sonnet
> Branch: `claude/tauri-record-live`
> Depends on: #24 (record screen), #161 (IPC bridge), #166 (audio), #167 (MIDI input)

## Goal

Wire the Record screen (Design E, #24) to real input with TUI parity
(`crates/tui/src/record.rs`): live/mock MIDI feeds the rising ribbons and an
event buffer, takes save as standard bundles, and a chosen backing track
plays during recording with the origin offset preserved. This supersedes the
mock-only scope of `M2-tauri-record-screen.md`.

## Context

- TUI reference: `crates/tui/src/record.rs` — `RecordScreen` (held buffer,
  `origin_us` anchoring, backing start wall-clock, save path
  `recordings/take-<timestamp>/`).
- Core/midi: `EventBuffer` (core), `events_to_smf_bytes`, `RecordingMeta`
  (`origin: TrackOrigin::Recorded`, `backing: Option<BackingTrack>`).
- Input: `midi_event` stream + mock keyboard from #167. Audio: backing via
  #166 (`attach_backing`-style session, but started at record start).
- Frontend: `screens/record/` from #24 — `RecordCanvas` currently replays
  the Ember Lantern take fixture; its public surface (`now`, `level`, `sel`)
  stays, the fixture feed is replaced by live events.

## What to do

### Backend `tauri-app/src-tauri/src/record.rs`

```rust
pub struct RecordSession {
    buffer: EventBuffer,
    origin_us: Option<u64>,        // first event timestamp, or anchored to
    backing: Option<RecordBacking> // backing start as in record.rs
}
#[tauri::command] fn record_start(…, backing: Option<String>) -> Result<(), String>;
#[tauri::command] fn record_stop(…);
#[tauri::command] fn record_save(…) -> Result<String, String>; // bundle dir
```

- While a session is active, the #167 drain loop also appends events to the
  session buffer (in addition to emitting `midi_event`) — recording must not
  depend on the webview being awake.
- Backing: `record_start` with a path starts playback immediately and
  anchors the origin exactly as the TUI does (`event.timestamp_us −
  elapsed_since_backing_start`), so the saved MIDI aligns with the audio.
- `record_save` writes `song.mid` + `meta.json`
  (`origin: Recorded`, backing reference + copy) to
  `recordings/take-<unix_ts>/` — byte-compatible with the TUI's bundles.

### Frontend changes (`screens/record/`)

- Feed `RecordCanvas` from `onMidiEvent` (note-on starts a ribbon, note-off
  closes it) instead of the fixture; keep the fixture as the no-session demo
  fallback so #24's standalone mode still works.
- Header: timecode from session elapsed; MIDI chip from `midi_status()`
  (#167) — green + port when live, `MOCK` otherwise; level meter from recent
  velocities (frontend-computed is fine, it's cosmetic).
- Transport: record (start session, with the currently chosen backing if
  any), stop, `s` save → toast with bundle dir, Esc → confirm-if-unsaved
  then menu.
- "Choose backing track" (menu item from #162, and a button in the record
  header): native file dialog (`tauri-plugin-dialog` from #166, same
  audio-extension filters as `crates/tui/src/backing.rs`), selection shown
  as a chip.
- Toolbar buttons with no core behaviour yet (Trim / Quantize / Punch-in)
  render disabled with a tooltip "not yet wired"; metronome & count-in
  toggles stay visual unless trivially wired through existing actions.

## Tests

- Rust: scripted session — feed events through the session path, save to a
  temp dir, re-parse: same notes/timestamps; `meta.json` has
  `origin: Recorded`.
- Backing-anchored origin: with a simulated backing started Δ before the
  first event, saved timestamps are shifted by Δ (pure helper, unit-tested).
- Save with empty buffer → `Err` (TUI refuses empty saves — verify in
  `record.rs` and match).

## Scope boundaries (do NOT)

- Do not implement editing of the take in this screen (the bundle opens in
  the edit screen for that).
- Do not implement Trim/Quantize/Punch-in (no core actions exist).
- Do not change the bundle schema.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] Mock-keyboard take: ribbons rise live, save produces a bundle that the
      library lists and both editors (TUI & Tauri) open
- [ ] Recording with backing stores reference + offset in `meta.json`
- [ ] PR opened against `main` from `claude/tauri-record-live`, `Closes #169`
