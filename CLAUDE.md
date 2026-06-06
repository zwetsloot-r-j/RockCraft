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
- **System deps (one-time):** the `audio`/`tui` crates link ALSA via `alsa-sys`
  (rodio/midir). Without the dev headers, `cargo build`/`test` aborts in
  `alsa-sys`'s build script with `pkg-config exited with status code 1 … Package
  alsa was not found` — this is a missing system library, not a code bug. Fix it
  once with `./scripts/setup-dev.sh` (or `sudo apt-get install -y libasound2-dev
  pkg-config`); CI runs the same step. Pure `core` work needs none of this —
  scope to `cargo test -p rockcraft-core`.
- One crate per task where possible — the workspace makes crates independent so
  parallel agents don't collide.
- Branch naming: humans `feat/*` etc.; Claude agents `claude/*`; Vibe agents
  `vibe/*`. Keep PRs scoped to one branch.
- Prefer expanding the typed `core` model over stringly-typed shortcuts. The
  compiler is the first reviewer.
- Task tracking: GitHub Issues are the control plane; detailed specs live in
  `specs/*.md` and travel in the clone. A PR closes its issue (`Closes #N`).
  See `docs/WORKFLOW.md`. Work to the spec; don't exceed its scope boundaries.

## Picking up work (delegation protocol)

You may be told either **a specific issue** ("work on issue #6") or **a queue**
("work on issues labeled `tier:cheap`" / "`agent:vibe`"). In both cases:

**1. Resolve the task.**
- Specific issue: read that issue.
- Queue: list open issues with the given label, e.g.
  `gh issue list --state open --label "<label>" --json number,title,labels`.
  **Exclude** any issue labeled `loc:local` (needs the physical piano — you
  cannot do these in a sandbox) or `status:in-progress` (already claimed).
  From what remains, pick the **lowest-numbered** issue and do that one.
  If nothing remains, stop and report "no available issues in `<label>`".

**2. Read before coding.** The issue body links a spec at `specs/<file>.md` —
read it, and read this guide. The spec is the source of truth; the issue is just
the pointer. If the issue has no spec link, implement from the issue body but
keep scope minimal and say so in the PR.

**3. Claim it** so parallel agents don't collide: add the `status:in-progress`
label and a brief comment, e.g.
`gh issue edit <N> --add-label status:in-progress` then
`gh issue comment <N> --body "Starting — <your agent name>, branch <branch>"`.

**4. Do the work.**
- **If the issue names an existing branch** (e.g. "Branch: `vibe/foo` (seeded)"),
  check it OUT — do not create a new one: `git fetch origin && git checkout <branch>`.
  It may already contain **seeded acceptance tests that fail to compile/pass** —
  that's intentional. Implement until the gate is green. **Do NOT modify, weaken,
  or delete any test files already present on the branch**; only add the
  implementation (and your own *additional* tests if useful).
- **Otherwise** create a branch with the correct prefix (Claude `claude/*`,
  Vibe `vibe/*`).
- Stay within the spec's scope boundaries either way.

**5. Finish.** Ensure the gate passes locally, open ONE PR against `main` whose
description contains `Closes #N`. Do not self-merge — a human reviews.

Take **one** issue per run unless explicitly told to batch.

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
