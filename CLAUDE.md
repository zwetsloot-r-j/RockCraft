<!--
  CANONICAL AGENT INSTRUCTIONS.
  Keep CLAUDE.md and AGENTS.md byte-for-byte identical — Claude reads CLAUDE.md,
  Mistral Vibe reads AGENTS.md. If you edit one, mirror it to the other in the
  same commit.
-->

# RockCraft — agent guide

RockCraft helps people learn songs on a USB-MIDI digital piano: a scrolling
"note highway" (Synthesia-style) plus Rocksmith-style scoring and practice.
Input is clean USB-MIDI (note-on/off, pitch, velocity, timing) — not audio — so
the core loop is precise and deterministic.

## Architecture (the contract — do not violate)

Cargo workspace; crates depend **inward only**:

```
crates/core   ← pure domain: events, song timeline, timing clock, scoring.
              ← NO I/O. No MIDI device, no audio, no terminal. Headless-testable.
crates/midi   ← live input (midir) + file parse/record (midly). Depends on core.
crates/audio  ← playback/metronome/synth (cpal/rodio/rustysynth). Depends on core.
crates/tui    ← ratatui frontend (MVP). Depends on core/midi/audio.
```

Invariants:
- **`core` stays pure.** No device, file-format, or terminal code in `core`.
  It must compile and test with zero hardware. This is what lets remote/cloud
  agents work on it against recorded fixtures.
- **Frontends are swappable.** TUI now; Tauri and Godot later consume the *same*
  `core`. Never leak view concerns into `core`.
- **Decouple rendering from timing.** Scoring judgments run off precise MIDI
  timestamps (`NoteEvent::timestamp_us`), never off frame/render rate.
- **Never block the real-time MIDI/audio thread.** Hand events to the engine via
  a lock-free channel/ring buffer; do rendering and disk I/O elsewhere.

## How to work

- The merge gate is `.github/workflows/ci.yml`: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets` (warnings are errors), and
  `cargo test --workspace`. Run all three locally before opening a PR.
- One crate per task where possible — the workspace makes crates independent so
  parallel agents don't collide.
- Branch naming: humans `feat/*` etc.; Claude agents `claude/*`; Vibe agents
  `vibe/*`. Keep PRs scoped to one branch.
- Prefer expanding the typed `core` model over stringly-typed shortcuts. The
  compiler is the first reviewer.
- Task tracking: GitHub Issues are the control plane; detailed specs live in
  `specs/*.md` and travel in the clone. A PR closes its issue (`Closes #N`).
  See `docs/WORKFLOW.md`. Work to the spec; don't exceed its scope boundaries.

## What is local-only (cannot run in a cloud sandbox)

Anything needing the physical piano: live MIDI capture, latency/feel, audio
output. Cloud/remote agents work on `core`, file parsing, scoring (verified
against committed MIDI fixtures), and refactors. To enable that, record real
MIDI sessions locally and commit them as fixtures.

## Current status

Step 0 complete: workspace scaffold, CI gate, this guide. Builds offline (no
third-party deps yet). Next: **M0 "Echo"** — `midir` reads the piano, a
`ratatui` view lists note events, and the session records to a `.mid`
(re-playable via `midly`). Third-party deps are added then, not before.
