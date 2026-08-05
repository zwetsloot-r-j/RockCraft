# M13-A — import: score files (MusicXML) → chart bundle, with notated tempo/key

> Milestone: M13 — Sheet Music Import · Issue: #245 · Suggested tier: opus
> Branch: `claude/m13-score-file-import`

## Goal

A second import source alongside the M6 video path: a **digital score file**
(MusicXML and friends) → the existing M6-A `ExtractedChart` JSON → a playable
chart bundle. Unlike video extraction this is a **deterministic transform** —
pitch, duration, tempo, key, time signature and staff→hand are all explicit in
the source, so there is no CV, no ML and no inference anywhere in this task.

Also closes the gap the video path left: `ExtractedChart` carries no notated
musical context, so every imported piece currently lands in the editor on a
default 120 BPM grid with nothing snapping to bars.

## Context

- The M6-A seam (`specs/M6-A-import-contract.md`) is already source-agnostic:
  anything that emits `ExtractedChart` JSON gets a bundle for free via
  `parser::chart_to_timeline` → `writer::write_chart_bundle_full`. **Do not
  duplicate or bypass that path** — this task adds a producer, not a pipeline.
- The sidecar convention is set by M6-C: a standalone Python project under
  `tools/`, its own `requirements.txt`, invoked as a subprocess emitting JSON on
  stdout (`crates/import/src/pipeline.rs::run_sidecar`). Mirror
  `tools/synthesia-extract/` in layout, CLI shape and test style.
- Relevant existing code:
  - `crates/import/src/schema.rs` — `ExtractedChart` / `ExtractedNote` / `Hand` / `SourceMeta`
  - `crates/import/src/writer.rs:70` — `write_chart_bundle_full` (writes `meta.json`)
  - `crates/import/src/pipeline.rs` — `ImportInput`, `run_pipeline`, `find_sidecar`
  - `crates/core/src/grid.rs` — `Grid` (`bpm`, `time_sig`, `subdivision`), `TimeSig`
  - `crates/core/src/chord.rs` — `Key` (`root_pc`, `scale`), `Scale`
  - `crates/control/src/host.rs:128` — `HostCommand::ImportStart` (the pattern to copy)
- Cloud-testable end to end: fixtures are hand-written synthetic MusicXML, no
  hardware, no network, no copyrighted input.

## What to do

### 1. Schema — notated context (`crates/import/src/schema.rs`)

```rust
pub struct ExtractedChart {
    pub notes: Vec<ExtractedNote>,
    pub source: SourceMeta,
    /// Notated musical context, when the source carries it. Score files do;
    /// video does not. Drives `meta.grid` / `meta.key` in the written bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notation: Option<NotationMeta>,
}

/// Tempo / metre / key as *notated* in the source score.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct NotationMeta {
    /// Tempo at the start of the piece, in BPM. The writer clamps it to
    /// `Grid::MIN_BPM..=Grid::MAX_BPM`; out-of-range is not an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_sig: Option<rockcraft_core::TimeSig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<rockcraft_core::Key>,
}
```

**Backward compatibility is a hard requirement**: `notation` is `#[serde(default)]`
so charts emitted by the existing Synthesia extractor — and the committed
`crates/import/tests/fixtures/synthetic_chart.json` — still deserialize
unchanged. Do not modify that fixture.

### 2. Writer — populate `meta.grid` / `meta.key`

In `write_chart_bundle_full`, after `RecordingMeta::new_midi_only`:

- `chart.notation == None` → leave `grid` / `key` as `None`. The video path's
  output must be **byte-identical** to today.
- `chart.notation == Some(n)` → start from `Grid::default_120()`, then:
  - `n.bpm` → `grid.set_bpm(bpm)` (clamps; `5` → `20`, `5000` → `300`)
  - `n.time_sig` → `grid.time_sig`
  - `subdivision` stays `Subdivision::Sixteenth` — deliberately not derived from
    the score's smallest note value; the user re-picks it in the editor.
  - `meta.key = n.key`

### 3. Sidecar — `tools/score-import/`

Layout mirroring `tools/synthesia-extract/`:

```
tools/score-import/
  convert.py            # CLI entry point
  score_import/         # package: conversion logic, schema emission
  requirements.txt      # music21 (only)
  README.md             # venv setup, supported formats, known limitations
  pytest.ini
  tests/
```

CLI: `python convert.py --in <score-file> --out <chart.json>|-`

Conversion rules — each one is testable, so spell them out in the README too:

1. **Parse** with `music21.converter.parse`. Accept whatever it reads natively;
   the documented/tested set is `.musicxml`, `.xml`, `.mxl`, `.abc`, `.krn`.
2. **Expand repeats** into a linear score (repeats, voltas, D.C., D.S., coda) —
   `music21.repeat.Expander`. If expansion raises (malformed repeat structure is
   common in real files), fall back to the unexpanded score and warn on stderr.
3. **Merge ties** so a tied pair becomes one note of the summed duration, not two.
4. **Chords** → one `ExtractedNote` per pitch, sharing start and duration.
5. **Timing** comes from music21's realized seconds (which honours tempo
   changes), not from a single BPM multiplication:
   `start_us = round(seconds * 1_000_000)`, `dur_us = max(1, round(...))`.
   - No tempo mark in the score → assume 120 BPM, warn on stderr, and still set
     `notation.bpm = 120`.
   - **More than one distinct tempo → warn loudly.** Note times stay correct
     (they are absolute µs), but `Grid` holds a single BPM, so the editor's bar
     lines will drift after the change. This is a known, documented limitation,
     not a bug to work around here.
6. **Hand** from staff/part: upper staff (or first part) → `Right`, lower staff
   (or second part) → `Left`; a single staff, or 3+ parts, → `Unknown`. Provide a
   `--hand-map` override for scores the heuristic can't split. (music21 exposes
   this via `PartStaff` for split piano parts; pick whichever API is reliable.)
7. **Velocity** → `null` unless the source carries an explicit velocity, letting
   `parser::DEFAULT_VELOCITY` apply. Do not synthesize velocities from dynamics
   markings in this task.
8. **Dropped by design** (document, don't silently swallow — count them on
   stderr): grace notes, ornament realizations (keep the principal note only),
   unpitched/percussion notes, pedal marks, articulations, lyrics.
9. **`confidence: 1.0`** on every note — this transform is exact.
10. **`SourceMeta`**: `title` from the score metadata, `extractor_version:
    "score-import-0.1"`, all the video-geometry fields `None`.
11. **`notation`** from the first metronome mark / time signature / key
    signature. MusicXML mode `major` → `Scale::Major`, `minor` →
    `Scale::NaturalMinor`; any other mode → omit the key rather than guess.
12. **stdout carries only the JSON.** All warnings and counts go to stderr —
    `run_sidecar` parses stdout.

### 4. Pipeline wiring (`crates/import/src/pipeline.rs`)

- Add `ImportInput::Score(PathBuf)`.
- **Rename `import_video` → `import_source`** (it is no longer video-only) and
  update the 4 call sites: `lib.rs` re-export, `examples/run_import.rs`,
  `crates/tui`, `tauri-app/src-tauri/src/import.rs`. This is the one existing
  public signature this spec authorizes changing.
- In `run_pipeline`, branch early on `Score`: run the score sidecar, write the
  bundle, done. **No fetch hook, no ffmpeg, no retained video, no
  `alignment.json`** — a score import is MIDI-only and reports
  `Progress::Extracting` → `Writing` → `Done`.
- Add `find_score_sidecar` alongside `find_sidecar`, pointing at
  `tools/score-import/convert.py`, with the same actionable `SidecarMissing`
  message shape (naming the right `requirements.txt`).
- Output dir: `import-out/<slug_stamp>` exactly as today.
- Extend `examples/run_import.rs` to route score extensions to
  `ImportInput::Score`.

### 5. Frontend entry points

- **`HostCommand::ImportScore { path: String }`** in `crates/control/src/host.rs`
  — name `"import_score"`, added to the catalog, the help text and
  `all_variants()` in the parity tests. It does disk I/O, so it is a
  `HostCommand` and never a `core::Action`.
- **TUI**: a "Import score file…" menu item reusing the existing file-browse and
  `Importing` progress screens. The `HostCommand` arm returns
  `HostError::Unsupported("import_score")`, matching the existing `ImportStart`
  precedent at `crates/tui/src/app.rs:754` (the TUI drives import through its
  interactive screens, not the socket).
- **Tauri**: add `ImportInputDto::Score(String)` and the dispatch arm in
  `control.rs` calling the existing `import::import_start`. Backend only — no
  new Tauri UI affordance in this task.

### 6. Content policy (`scripts/check-no-media.sh`, `docs/IMPORT.md`)

Scores are copyrighted exactly like recordings, and the guard doesn't know about
them yet. Extend it:

- **Never allowed anywhere**: `.pdf`, `.mxl`, `.mscz`, `.sib`, `.gp3/.gp4/.gp5/.gpx`.
  `.mxl` is a zip container — unreviewable in a diff, so plain `.musicxml` is the
  only committable score format.
- **Allowed only under `fixtures/`**: `.musicxml`, `.xml`, `.abc`, `.krn` —
  reusing the existing `^fixtures/` carve-out rather than inventing a second one.
  Put the synthetic test scores in `fixtures/score/` and have the Python tests
  read them from there.
- Update the allowed/not-allowed table in `docs/IMPORT.md` and add a short
  "Score import" section covering the sidecar, the optional-dependency posture
  and the single-tempo limitation.

## Tests

**Rust** (`crates/import`):
- `notation` round-trips through `to_json` / `from_json`.
- A chart JSON with **no** `notation` key still deserializes — assert against the
  existing `tests/fixtures/synthetic_chart.json`.
- Writer: `notation { bpm: 90, time_sig: 3/4, key: G major }` → `meta.json` has
  `grid.bpm == 90`, `grid.time_sig == 3/4`, `grid.subdivision == Sixteenth`,
  `key == G major`.
- Writer: `notation: None` → `meta.grid` and `meta.key` are both `None`
  (video path unregressed).
- Writer clamping: `bpm: 5` → `20`; `bpm: 5000` → `300`.
- `host.rs` parity tests cover `ImportScore` once it is in `all_variants()`.

**Python** (`tools/score-import/tests/`), against hand-written synthetic scores in
`fixtures/score/` — obviously fabricated (a C-major scale, a two-hand chord),
never a real piece:
- Pitches exact; onsets and durations exact **to the microsecond** (this
  transform is deterministic — no tolerance windows, unlike M6-C).
- Hands: two-staff fixture → upper `Right`, lower `Left`; single-staff → `Unknown`.
- `notation` block matches the fixture's 3/4 / 90 BPM / G major.
- A fixture with a repeated bar unrolls to double the notes.
- A tied-note fixture yields one note of the summed duration, not two.
- A grace-note fixture drops the grace note and reports the count on stderr.
- A score with no tempo mark → 120 BPM, warning on stderr, valid output.
- A score with two tempo marks → note times reflect both; a warning is emitted.
- Empty score and single-note score do not crash.
- Emitted JSON validates against the M6-A schema (deserializes in the Rust test
  suite, or assert the key set — don't hand-maintain a duplicate schema).

## Scope boundaries (do NOT)

- Do not add a `hand` field to `core`'s `Note`/`Timeline`. `chart_to_timeline`
  keeps dropping it (`parser.rs:26`) — `events_to_smf_bytes` is single-track with
  no channel, so carrying hand through would ripple into scoring, the editor and
  both frontends. Separate task.
- Do not add a tempo map to `Grid`. One `bpm` per piece stands; warn instead.
- Do not touch the Synthesia extractor, the video pipeline's behaviour, or
  `chart_to_timeline`'s existing semantics.
- Do not build any OMR / PDF / image handling — that is #246.
- Do not use an LLM or vision model anywhere in this path. This transform is
  exact; inference would only make it worse.
- No new Rust dependencies. Python: `music21` only.
- Do not commit a real score, PDF, or `.mxl` — synthetic `.musicxml` under
  `fixtures/score/` only.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `pytest` green in `tools/score-import/`
- [ ] `scripts/check-no-media.sh` passes with the new extensions in place
- [ ] A synthetic MusicXML runs end to end via
      `cargo run -p rockcraft-import --example run_import -- fixtures/score/<f>.musicxml`
      and produces a bundle whose `meta.json` carries the notated grid and key
- [ ] PR against `main` from `claude/m13-score-file-import`, `Closes #245`
