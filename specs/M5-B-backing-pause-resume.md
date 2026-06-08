# M5-B — audio: BackingHandle pause / resume / seek

> Milestone: M5 — Play-along & Backing Sync · Issue: #107 · Suggested tier: sonnet
> Branch: `claude/m5-backing-pause-resume`

## Goal

Extend [`BackingHandle`](../crates/audio/src/lib.rs) so a playing backing track
can be **paused, resumed, and re-seeked** without tearing down and rebuilding
the output stream — the audio side of "pause the music while waiting for the
right notes" (M5-C) and of scrubbing the track while editing (M5-D/E).

## Context

- Crate: `crates/audio` only.
- `BackingHandle` today wraps a `rodio::Sink` and exposes only `stop()`. The
  sink already supports `pause()`, `play()`, and `try_seek()` — this task is a
  thin, documented, non-blocking API over them. `play_file_at` already shows the
  best-effort seek pattern (`let _ = sink.try_seek(..)`).
- All *decisions* about when to pause/seek live in core (M5-A) and the
  frontends; this crate just pokes the sink. Never block the audio thread.
- `loc:local`: actual audibility is verified on the host.

## What to do

Add to `impl BackingHandle`:

```rust
/// Pause playback in place; the stream stays open and resumes from here.
/// Idempotent.
pub fn pause(&self);

/// Resume after `pause`. Idempotent.
pub fn resume(&self);

/// Pause or resume to match `paused`.
pub fn set_paused(&self, paused: bool);

/// Whether playback is currently paused.
pub fn is_paused(&self) -> bool;

/// Best-effort seek to `pos` within the track (mirrors `play_file_at`’s
/// fallback: ignore decoders that can’t seek). Does not change paused state.
pub fn seek(&self, pos: std::time::Duration);
```

Map them onto `rodio::Sink`: `pause()` → `sink.pause()`, `resume()` →
`sink.play()`, `is_paused()` → `sink.is_paused()`, `seek()` →
`let _ = self.sink.try_seek(pos);`. Document that `seek` while paused stays
paused and resumes at the new position.

## Tests

- Unit-level: constructing a handle (via `play_file`/`play_file_at` on a tiny
  bundled or temp-generated WAV, behind the existing test gating) then
  `pause()`/`resume()`/`set_paused()` flips `is_paused()` accordingly;
  operations are idempotent. If the existing audio tests already avoid opening a
  real device in CI, follow that same gating — do **not** add a CI step that
  needs a sound card.
- Manual host check (record in the PR): pause silences without a click/gap and
  the stream stays open; resume continues in place; seek jumps to the new
  position.

## Scope boundaries (do NOT)

- Do not change `play_file` / `play_file_at` signatures or the synth.
- Do not add a polling thread or any blocking call; methods are immediate sink
  pokes.
- No new third-party deps (rodio already provides everything).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Manual host verification noted in the PR
- [ ] PR against `main` from `claude/m5-backing-pause-resume`, `Closes #107`
