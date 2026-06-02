# M2-A — synth foundation: hear the piano you play

> Milestone: M2 — Audio · Issue: #27 · Suggested tier: opus
> Branch: `claude/audio-synth`

## Goal

Turn live MIDI into piano sound. Add the first real audio dependencies to
`crates/audio`, build a polyphonic SoundFont synth fed by a lock-free command
queue, play it through the default output device, and wire the Record screen so
every key you press on the piano is heard. This is the foundation every later
audio task (B, D, E) builds on.

## Context

- Crate: `crates/audio` (currently an empty stub that re-exports `core`).
- Sound source decision: **rustysynth** (pure-Rust SoundFont synthesizer).
- Output/decoding decision: **rodio** (wraps `cpal`).
- The event stream already exists: `app.rs` drains `LiveInput::events()` once per
  frame and routes each `NoteEvent` to the active screen. The synth taps the
  *same* `NoteEvent`s — no new MIDI path.
- Read `CLAUDE.md`: **never block the real-time audio thread** and **never leak
  view concerns into core**. The MIDI/app thread only *enqueues* commands; all
  rustysynth rendering happens inside the rodio audio callback thread.

## What to do

Add to `crates/audio/Cargo.toml`:

```toml
rodio = "0.20"        # or latest compatible 0.x
rustysynth = "1.3"    # or latest compatible
```

### SoundFont asset

Commit a **small, piano-only, permissively-licensed** `.sf2` (target < ~8 MB)
at `crates/audio/assets/piano.sf2`, plus `crates/audio/assets/NOTICE.md`
recording its source, author, and license (the license MUST permit
redistribution). Prefer one with multiple velocity layers for dynamics.

### `crates/audio/src/synth.rs`

```rust
/// Commands handed from the app thread to the audio thread. Cheap, Copy.
enum SynthCommand { NoteOn { note: MidiNote, velocity: Velocity }, NoteOff { note: MidiNote }, AllOff }

/// Cheap, cloneable handle used from the MIDI/app thread. Only enqueues.
pub struct SynthHandle { /* Sender<SynthCommand> */ }
impl SynthHandle {
    pub fn note_on(&self, note: MidiNote, velocity: Velocity);
    pub fn note_off(&self, note: MidiNote);
    pub fn all_off(&self);
    /// Convenience: route a NoteEvent straight to the synth.
    pub fn apply(&self, ev: &NoteEvent);
}

/// The rodio Source that owns the rustysynth Synthesizer. Lives on the audio
/// thread. `next()` drains pending commands, renders a block, yields
/// interleaved stereo f32 samples.
pub struct SynthSource { /* Synthesizer, Receiver<SynthCommand>, render bufs, sample rate */ }

/// Build the synth from SoundFont bytes (asset-source-agnostic so `core`-style
/// callers and tests don't depend on a file path). Returns the audio-thread
/// Source and the app-thread handle.
pub fn synth_from_sf2_bytes(bytes: &[u8], sample_rate: u32) -> Result<(SynthSource, SynthHandle), SynthError>;
```

Implementation notes:
- A single producer (app thread) → single consumer (audio thread): a
  `std::sync::mpsc` channel is acceptable for the MVP (no extra dep). The
  callback drains with `try_iter()` — never blocks.
- rustysynth renders into separate L/R `f32` buffers; interleave them for rodio.
  Buffer leftover samples between `next()` calls so block size ≠ rodio's request
  size is handled.
- Map velocity 0..=127 straight through; use MIDI channel 0.

### `crates/audio/src/lib.rs` — `AudioOut`

```rust
pub struct AudioOut { /* OutputStream + stream handle (kept alive) + SynthHandle */ }
impl AudioOut {
    /// Open the default output device, embed the piano SoundFont, start playing.
    pub fn new() -> Result<Self, AudioError>;     // uses include_bytes!("../assets/piano.sf2")
    pub fn synth(&self) -> SynthHandle;
}
```

Drop closes the stream. Keep the `OutputStream` alive in the struct (rodio stops
on drop).

### Wire into the TUI

In `crates/tui`: construct `AudioOut` once at startup (alongside `LiveInput`),
pass the `SynthHandle` into the `Shell`. In the Record screen's `ingest` (or in
`app.rs` where events are routed), call `handle.apply(&ev)` so live keys sound.
If `AudioOut::new()` fails (no output device), log a warning and continue
silently — audio is not required to run.

## Tests

Audible behaviour is **local/manual** (needs hardware). CI must still cover the
pure logic — put these in `synth.rs` behind `#[cfg(test)]`, no device:
- A `NoteOn` then `NoteOff` enqueued via `SynthHandle` are received in order by
  the consumer side of the channel.
- `SynthHandle::apply` maps `NoteEvent::on/off` to the matching command (incl.
  velocity-0 note-on → `NoteOff`, mirroring MIDI convention).
- Interleaving helper: given L=[1,2] R=[3,4] produces [1,3,2,4].
- `synth_from_sf2_bytes` returns `Ok` for the committed asset bytes and yields a
  Source whose `channels()==2` and `sample_rate()` matches the request.

## Scope boundaries (do NOT)

- Do not touch `crates/core` or `crates/midi`. The synth consumes existing
  `NoteEvent` types only.
- Do not add audio to Play mode here — that is Task B (#28).
- Do not add backing-track / file playback here — Tasks D/E.
- Do not block the audio callback (no locks held across rendering, no I/O, no
  allocation storms).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green (the pure-logic tests above)
- [ ] Manual (local): pressing keys on the piano produces piano sound with
      velocity dynamics and correct note-off release
- [ ] PR against `main` from `claude/audio-synth`, `Closes #27`
