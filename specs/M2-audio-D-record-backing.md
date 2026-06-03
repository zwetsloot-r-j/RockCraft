# M2-D — backing track while recording: play, sync, save bundle

> Milestone: M2 — Audio · Issue: #30 · Suggested tier: sonnet
> Branch: `claude/record-backing`

## Goal

Let the player attach a backing audio file when starting a recording. The track
plays through the speakers while they record, and the take is saved as a
**bundle directory** that pins the audio and its sync offset, so playback (Task
E) can line everything up.

## Context

- Depends on **Task C (#29)** for `RecordingMeta` / `BackingTrack` / sync math,
  and **Task A (#27)** for rodio audio output.
- Crate: `crates/tui` (`record.rs`, `app.rs`) + `crates/audio` (file playback).
- Today `RecordScreen` rebases timestamps so the **first note** is `t=0`. With a
  backing track that is wrong — the MIDI must be timed against the **audio
  start** instead, or it won't realign on playback.
- rodio decodes wav/mp3/ogg/flac via `Decoder`. The actual `std::fs` and dir
  creation live here (the tui/io layer), not in `core`.

## What to do

### Audio file playback (`crates/audio`)
Add a thin helper to play a decoded file through the existing output (mix it
alongside the synth — rodio's `Sink`/mixer or a second `Sink` on the same
`OutputStream`):

```rust
pub fn play_file(path: &Path) -> Result<BackingHandle, AudioError>; // starts immediately
pub struct BackingHandle { /* Sink */ }
impl BackingHandle { pub fn stop(&self); }
```

### Record flow (`crates/tui`)
1. Entering Record, let the player choose to attach an audio file (e.g. pass a
   path via a CLI arg / env / a simple prompt — keep the UI minimal; a path
   argument is acceptable for the MVP). No file = today's behaviour, unchanged.
2. When a backing track is attached, on Record start: begin audio playback and
   set the recording **origin to that moment**. Every `NoteEvent` is rebased to
   `now − audio_start_instant` (microseconds), so note times are relative to the
   audio start, **not** the first note. With no backing track, keep the existing
   first-note-is-zero rebase.
3. Show in the status line that a backing track is playing.
4. **Save as a bundle** instead of a loose `.mid`:
   - Create `recordings/take-<stamp>/`.
   - Write the MIDI to `song.mid` (reuse `events_to_smf_bytes`).
   - Copy the audio file in as `backing.<ext>` (keep the original extension).
   - Write `meta.json` from a `RecordingMeta` (Task C): `midi_file="song.mid"`,
     `backing = Some(BackingTrack { file:"backing.<ext>", audio_start_us })`.
     `audio_start_us` is 0 for the MVP (audio and recording start together).
   - With no backing track, still write a bundle dir with `song.mid` +
     midi-only `meta.json` (so Task E has one discovery path). (If keeping the
     old loose-`.mid` save is simpler for back-compat, note it in the PR — but
     prefer the bundle.)

## Tests

Audible/hardware behaviour is local/manual. Cover the pure pieces in CI:
- Timestamp rebasing against an audio-start origin (feed events with a known
  origin, assert rebased times) — factor into a pure helper, no device.
- Manifest written for the bundle deserializes back to the expected
  `RecordingMeta` (round-trip via Task C).
- Filenames/extensions chosen correctly for a few input paths.

## Scope boundaries (do NOT)

- Do not modify `crates/core` (consume Task C). If C lacks something, prefer
  adding it to C, not working around it in tui.
- Do not implement playback-side sync — that is Task E.
- Do not block the audio thread; file decode/playback runs off the rodio thread.
- Keep the file-picking UI minimal; a fancy file browser is out of scope.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` green
- [ ] Manual (local): recording with a backing track plays the audio, and the
      saved bundle contains `song.mid`, `backing.<ext>`, and a correct
      `meta.json`
- [ ] PR against `main` from `claude/record-backing`, `Closes #30`
