# M13-D — import: velocity from notated dynamics in score files

> Milestone: M13 — Sheet Music Import · Issue: #248 · Suggested tier: sonnet
> Branch: `claude/m13-score-dynamics`

## Goal

Make an imported score *sound* like music when the synth plays it. M13-A emits
`velocity: null` for every note, so `DEFAULT_VELOCITY` (80) applies uniformly and
the whole piece auditions at one flat dynamic. The source already carries the
answer — read the notated dynamics and emit real velocities.

## Context

- **Depends on #245 (M13-A)**, which builds `tools/score-import/` and the
  MusicXML→`ExtractedChart` conversion this extends. M13-A explicitly deferred
  this ("Do not synthesize velocities from dynamics markings in this task").
- This is sidecar-only: `ExtractedNote.velocity` is already an
  `Option<u8>` in the M6-A schema and `parser::chart_to_timeline` already honours
  it (`crates/import/src/parser.rs:44`). **No Rust change is expected.**
- Why it matters more for scores than for video: the video path fills velocity
  from the source audio in M6-F, and imported videos also carry a real
  `backing.wav`. A score import has neither — the synth is the *only* sound, so
  flat velocity is the whole listening experience.
- Dynamics are **per-staff**: a left hand marked `p` under a right hand marked
  `f` is normal piano writing and must not be flattened to one curve.

## What to do

In the M13-A sidecar, resolve an explicit velocity per note and emit it instead
of `null`.

### Mapping

Base velocity from the dynamic in effect at the note's offset, in its own part/staff:

| dynamic | ppp | pp | p  | mp | mf | f  | ff  | fff |
|---------|-----|----|----|----|----|----|-----|-----|
| velocity| 16  | 33 | 49 | 64 | 80 | 96 | 112 | 126 |

`mf` maps to **80 — the same value as `parser::DEFAULT_VELOCITY`** — so a score
whose only dynamic is `mf` sounds exactly as it does today. Keep that property.

- **Hairpins** (`cresc.` / `dim.` wedges): ramp linearly between the dynamic
  before the wedge and the one after it, by the note's offset within the wedge.
  A wedge with no resolving dynamic ramps one level and warns on stderr.
- **Articulations**: accent `+15`, marcato `+25`, both clamped to 127. These
  stack on the base dynamic.
- **No dynamic in effect** (before the first marking, or a score with none at
  all): emit `velocity: null`, **not** 80. The null preserves the honest signal
  "the source didn't say", and `DEFAULT_VELOCITY` still applies downstream. A
  score with zero dynamics markings must produce output identical to M13-A's.
- `--no-dynamics` CLI flag restores M13-A's behaviour wholesale, for A/B
  comparison and as an escape hatch for scores the resolution mis-reads.

Report on stderr how many notes got an explicit velocity vs. how many fell
through to null — the same counting convention M13-A uses for dropped ornaments.

## Tests

Python, against hand-written synthetic MusicXML in `fixtures/score/` (the
directory and the `^fixtures/` carve-out come from M13-A):

- A fixture marked `p` → all notes velocity 49; `ff` → 112.
- A `cresc.` wedge from `p` to `f` → velocities increase monotonically across the
  wedge, starting at 49 and ending at 96.
- An accented note under `mf` → 95; a marcato note under `fff` → clamped to 127.
- **Per-staff independence**: left staff `p`, right staff `f` in the same
  measure → 49 and 96 respectively, not a single blended value.
- A fixture with **no** dynamics → every note emits `velocity: null`, and the
  output is byte-identical to the same fixture run under `--no-dynamics`.
- `--no-dynamics` on a fully-marked score → all `null`.
- The stderr counts match the emitted notes.

## Scope boundaries (do NOT)

- **No sustain pedal.** MusicXML has `<pedal>` marks, but there is nowhere to put
  them: `SynthCommand` handles only `NoteOn`/`NoteOff`/`AllOff`
  (`crates/audio/src/synth.rs:29`), `NoteEvent` has no control-change concept,
  and `events_to_smf_bytes` writes note on/off only. Wiring CC64 through means a
  new control-event model in `core` that ripples into the MIDI writer, both
  frontends and the editor. Worth doing, but as its own issue — not smuggled in
  here.
- **No duration changes.** Staccato, tenuto and fermata alter how long a note
  sounds, which would move the highway blocks and the scoring windows, not just
  the volume. Velocity only.
- No tempo rubato, no humanized timing jitter. Timing stays exactly as notated —
  this is a learning tool and the highway must stay honest.
- Do not change `ExtractedNote`, the parser, the writer, or anything in
  `crates/`. Sidecar-only.
- Do not add a Python dependency beyond M13-A's `music21`.

## Acceptance

- [ ] `pytest` green in `tools/score-import/`
- [ ] `cargo fmt --all --check` / `clippy` / `test --workspace` still green
      (nothing in `crates/` should have changed)
- [ ] Manual (local): an imported score with dynamic markings audibly swells and
      softens under hear-the-song, and a score without them is unchanged
- [ ] PR against `main` from `claude/m13-score-dynamics`, `Closes #248`
