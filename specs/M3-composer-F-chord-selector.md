# M3-F — tui: chord selector (key-aware multi-lane insert)

> Milestone: M3 — Composer · Issue: #54 · Suggested tier: opus
> Branch: `claude/m3-chord-selector`

## Goal

A convenience layer that places **3+ notes at once** — a chord that fits the
piece's key — across pitch lanes at the cursor's time, and lets the user cycle
through the diatonic chords that fit. Builds on the chord theory in #51 and
the editor in #52/E.

## Context

- Crate: `crates/tui`, extends `edit.rs`. Uses `core::{Key, ChordKind}` (#51)
  and `Timeline::insert` (#49).
- `EditScreen` gains a `key: Key` field (default C major) — later persisted by
  #56 and editable in UI by a follow-up. Chord pitches are voiced from the
  cursor's current pitch as the chord root octave.

## What to do

Add a chord-insert flow (append to the #52 keymap):

| Key            | Action                                                          |
|----------------|-----------------------------------------------------------------|
| `c`            | enter chord mode (status shows current degree + quality)        |
| `1`..`7`       | choose scale degree; place that diatonic chord at the cursor    |
| `[` / `]`      | (in chord mode) cycle degree down/up through the 7 fitting chords|
| `7` qualifier  | `s` toggles Triad↔Seventh for the placed chord                  |
| `Enter`/`Esc`  | commit / cancel chord mode                                      |

- Placing a chord inserts each pitch from `key.diatonic_chord(degree, kind,
  root)` as a one-step note at the cursor time (root = cursor pitch).
- Cycling degrees should **preview** (audition through synth, and show a ghost)
  before commit, replacing the previously-previewed chord so cycling doesn't
  pile up notes. Commit makes it permanent; cancel removes the preview.
- Expose `previewed_chord()` / `last_committed_pitches()` for tests.

## Tests (headless)

- In C major, degree `1` places {C,E,G} as three notes sharing the cursor's
  start and one-step duration; degree `5` places {G,B,D}.
- Toggling to Seventh on degree `5` places four notes {G,B,D,F}.
- Cycling degree with `[`/`]` replaces the preview (note count stays at the chord
  size, not accumulating); commit keeps it, cancel removes it.
- Chord voiced near the top of the keyboard drops out-of-range pitches (per #51).

## Scope boundaries (do NOT)

- Do not implement scales/qualities beyond what #51 provides.
- No inversions/voicings UI; root-position only.
- No key-change UI here (just a default + the field #56 persists).
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-chord-selector`, `Closes #54`
