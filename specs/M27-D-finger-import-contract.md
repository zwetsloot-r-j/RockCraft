# M27-D — import: finger numbers in the chart contract, read from MusicXML

> Milestone: M27 — Finger numbers · Issue: #373 · Suggested tier: sonnet
> Branch: `claude/m27-finger-import` · Depends on: M27-A (#370)

## Goal

When an imported source clearly states a finger for a note, the imported bundle
carries it. This spec adds an optional `finger` to the extractor contract
(`ExtractedChart`), has the Rust importer write it into the bundle, and teaches
the score importer to read MusicXML `<fingering>`. A note with no clear finger
gets none — that is the default and the common case.

## Context

- Contract: `crates/import/src/schema.rs` — `ExtractedNote { pitch, start_us,
  dur_us, hand, velocity, confidence }`. There is no schema version; new
  optional fields are added with `#[serde(default)]` and unknown fields are
  ignored (see how `velocity` and `notation` were added).
- Two Python mirrors that deliberately do not import each other:
  `tools/synthesia-extract/synthesia_extract/schema.py` and
  `tools/score-import/score_import/schema.py`.
- Bundle writing: `crates/import/src/writer.rs` `write_chart_bundle_full`;
  `crates/import/src/parser.rs` (`chart_to_timeline`, `hand_assignment`).
- MusicXML is parsed by the Python sidecar `tools/score-import` with music21
  (`score_import/convert.py`): `_strip_ties` merges tied notes, `_part_notes`
  emits one `ExtractedNote` per pitch. OMR scans (M13-B) go through the same
  function. music21 exposes `<technical><fingering>` as
  `articulations.Fingering` (`.fingerNumber`) on the note or chord's
  `articulations`; `score_import/dynamics.py` `_articulation_boost` already
  walks that list.
- M27-A added `RecordingMeta.fingers: Vec<NoteFinger>` and `Finger` (1–5).

## What to do

### D1 — contract (Rust + both Python mirrors)

```rust
// crates/import/src/schema.rs, on ExtractedNote
#[serde(default, skip_serializing_if = "Option::is_none")]
pub finger: Option<u8>,   // 1..=5; anything else is treated as absent
```

- Kept as a raw `u8` in the contract so one odd value from a sidecar cannot
  fail a whole import; it is validated when the bundle is written (D2).
- Both Python `ExtractedNote`s gain `finger: int | None = None`, omitted from
  `to_dict()` when `None`.
- **Carry it through every place that rebuilds a note field by field**, or it
  is silently dropped: `synthesia_extract/audio.py` (`shift_notes` and the two
  other constructions), `rsplus_audio.py` (`_moved`), `rsplus.py`
  (`series_to_notes`), `pipeline.py`, `score_import/convert.py`. Where a stage
  merges or splits notes, the result keeps the finger of the note whose onset
  it keeps.
- Update `docs/IMPORT.md` where it documents the chart fields.

### D2 — bundle writer (`crates/import/src/`)

- `parser.rs`: `pub fn finger_assignment(chart: &ExtractedChart) ->
  Vec<NoteFinger>` — one entry per note whose `finger` is `Finger::new`-valid,
  sorted by `(start_us, pitch)`. If two chart notes share `(pitch, start_us)`
  and disagree, that position gets none.
- `writer.rs` `write_chart_bundle_full`: `meta.fingers = finger_assignment(chart)`.
- `chart_to_timeline` keeps building notes with `finger: None`, like hand: the
  bundle's `meta.json` is the carrier.

### D3 — MusicXML `<fingering>` (`tools/score-import/score_import/convert.py`)

In `_part_notes`, give each emitted note the finger the score states for it:

- Single note with exactly one fingering whose number is 1–5 → that finger.
- Chord: music21 lists the chord's fingerings in the order of its notes. Assign
  them one to one **only when** the chord has exactly as many fingerings as
  notes; otherwise the whole chord gets none. (Verify the ordering against
  music21 with the fixture below; if it is not reliable, read the fingering
  from each chord member's own articulations instead.)
- Anything unclear → none: more than one fingering on a single note (finger
  substitution such as `2-1`), a number outside 1–5, alternate/substitution
  fingerings (`alternate="yes"`, `substitution="yes"`), a fingering on a grace
  note (grace notes are dropped anyway).
- Tied notes: the merged note takes the fingering of its **first** segment;
  check `_strip_ties` keeps it, and fix up if it does not.
- Repeats: every expanded pass carries the fingering.
- OMR input goes through the same code. Nothing extra to do, but do not relax
  the rules for it.
- New CLI flag `--no-fingering` on the sidecar (default: read fingerings),
  alongside `--no-dynamics`.

### D4 — fixtures

Add `fixtures/score/fingering.musicxml`, obviously synthetic (a scale and a
couple of chords written for this test), covering: fingered single notes, a
fully fingered chord, a partly fingered chord, a tied note fingered on its
first segment, a substitution fingering, an unfingered note.

## Tests

- Rust: a chart JSON without `finger` still parses (the committed
  `synthetic_chart.json` fixture is untouched); `finger: 3` round-trips;
  `finger_assignment` keeps 1–5, drops 0 and 9, drops a conflicting duplicate
  position, sorts; a written bundle's `meta.json` has the fingers and
  `song.mid` + `apply_fingers` puts them back on the right notes; a chart with
  no fingers writes `fingers: []`.
- Python (score-import): one test per bullet of D3 against the fixture;
  `--no-fingering` yields no fingers; existing fixtures still yield no fingers
  and otherwise identical output.
- Python (synthesia-extract): `ExtractedNote.to_dict()` omits `finger` when
  `None` and writes it when set; a finger survives `shift_notes` and the
  Rocksmith+ `_moved` step.

## Scope boundaries (do NOT)

- No reading of finger numbers from video — that is M27-E.
- No in-app import option; the sidecar flag is enough for now.
- No guessing: never infer a finger that the source does not state.
- Do not change hand detection or `hand_assignment`.
- No new dependencies (Rust or Python).
- Content hygiene (`specs/M6-B-content-hygiene.md`): the fixture must be
  synthetic; no real score.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `pytest` green in `tools/score-import` and `tools/synthesia-extract`
- [ ] `scripts/check-no-media.sh` passes
- [ ] PR opened against `main` from the branch above, `Closes #373`
