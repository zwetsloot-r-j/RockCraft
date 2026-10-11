# M22-B — Export sheet music as MusicXML

> Milestone: M22 — Sheet-music export · Issue: #330 · Suggested tier: sonnet
> Branch: `claude/m22-musicxml-export` · Depends on: M22-A (#329)

## Goal

Let the user turn the piece open in the editor into printable sheet music.
RockCraft writes a **MusicXML** file from the M22-A notation model, and the
user opens it in MuseScore (free) or any other notation program to adjust the
layout, add fingering and print or save a PDF. We don't build an engraver:
MusicXML is the format every notation program reads, and the M13 importer
already reads it back.

## Context

- **M22-A** (`specs/M22-A-notation-model.md`): `core::notation::build_score(notes,
  grid, bar_starts, key, hand_split, opts) -> Score`. That spec defines the
  `Score` / `Measure` / `Event` / `WrittenNote` types this writer consumes.
- **File formats stay out of `core`** (CLAUDE.md), so the writer goes in a new
  crate. It depends only on `rockcraft-core`.
- **Host commands:** `crates/control/src/host.rs` (enum, `name()`, the names
  list, `HostCommandInfo` catalog, parity tests). Each frontend has one
  exhaustive `match`: `tauri-app/src-tauri/src/control.rs` and
  `crates/tui/src/app.rs`. Every Tauri IPC command pairs with a host command
  (precedent: `save_bundle` ↔ `HostCommand::SaveBundle`, `edit_query_video` ↔
  `HostCommand::QueryVideo`).
- **Where the piece's data lives at save time.** `state::save_bundle_into`
  (`tauri-app/src-tauri/src/state.rs`) already collects the grid, key, hand
  split, tempo map and notes to write `meta.json` + `song.mid`. Gather the
  `build_score` inputs from the same place. The TUI's save path does the same
  in `crates/tui`. `AppState::current_dir` is the loaded bundle's directory.
- **Desktop dialog:** `tauri-plugin-dialog` is already a dependency;
  `bridge.ts` imports `open` from `@tauri-apps/plugin-dialog`. Use its `save`.
- Reference MusicXML to imitate: `fixtures/score/two_staff_chords.musicxml`,
  `tied_note.musicxml`.

## What to do

### 1. New crate `crates/export` (`rockcraft-export`)

Add it to the workspace. Its only dependency is `rockcraft-core`; it needs no
XML library, since the output is small and hand-written with escaping.

```rust
// crates/export/src/musicxml.rs (re-exported from lib.rs)
/// A complete MusicXML 4.0 `score-partwise` document for one grand-staff
/// piano part. Pure: returns the text, never touches the disk.
pub fn to_musicxml(score: &Score, title: &str) -> String;
```

Output requirements:

- The XML declaration, the MusicXML 4.0 partwise DOCTYPE, and
  `<work><work-title>` = `title` (XML-escaped: `& < > " '`).
- `<identification><encoding><software>RockCraft</software>`.
- One `<part-list>` entry, `P1` named "Piano", and one `<part id="P1">`.
- **Measure 1** `<attributes>`: `<divisions>` = `notation::DIVISIONS`,
  `<key><fifths>` + `<mode>major|minor`, `<time><beats>/<beat-type>`,
  `<staves>2</staves>`, clef 1 = G line 2, clef 2 = F line 4.
- **Tempo:** for each measure with `tempo: Some(bpm)`, at the start:
  `<direction placement="above"><direction-type><metronome>` (beat-unit
  quarter, per-minute `bpm`) `</direction-type><sound tempo="bpm"/></direction>`.
- **Staff 1, then `<backup>` the full bar, then staff 2,** each event in order:
  - Chord: one `<note>` per pitch; every note after the first gets `<chord/>`.
    Each has `<pitch><step>`, `<alter>` (omit when 0), `<octave>`;
    `<duration>` = value ticks; `<voice>` 1 for staff 1 and 5 for staff 2;
    `<type>` (whole / half / quarter / eighth / 16th / 32nd); `<dot/>` when
    dotted; `<accidental>` (sharp / flat / natural) when set; `<staff>`.
    Ties need both `<tie type="start|stop"/>` (sound) and, under
    `<notations>`, `<tied type="start|stop"/>` (display).
  - Rest: `<note><rest/>` with duration, voice, type, dot and staff.
  - BarRest: `<note><rest measure="yes"/>` with `<duration>` = `bar_ticks`
    and no `<type>`.
- Measures with `number` = `Measure::number`. Finish with `</part></score-partwise>`.

### 2. Host command `ExportScore { path: String }`

- In `host.rs`: the variant, `name()` → `"export_score"`, the names list, and
  a catalog entry: params `path: String`, with a description saying that it
  writes the open piece as MusicXML sheet music (right hand treble, left hand
  bass, quantised to sixteenths) to `path`, returns the written path, and that
  `.musicxml` is appended when the path has no extension. Add it to the
  parity test lists.
- Behaviour, identical in both frontends:
  1. Build the `Score` from the open piece with `NotationOptions::default()`.
  2. No notes → error `"nothing to export"`.
  3. Title = the loaded bundle's directory name, else `"Untitled"`.
  4. Write `to_musicxml(..)` to `path` (append `.musicxml` if there is no
     extension) and return the absolute path written. I/O errors map to the
     frontend's usual host error.
- **Tauri:** `state::export_score(state, path)`, a `#[tauri::command] fn
  edit_export_score(path: String) -> Result<String, String>` registered in
  `generate_handler!`, and the `HostCommand::ExportScore` arm in `control.rs`.
- **TUI:** implement the arm the same way in `crates/tui/src/app.rs` (the TUI
  has the composer and disk access). No TUI key or menu in this spec.

### 3. Desktop editor key

- `bridge.ts`: `editExportScore(path: string): Promise<string>`.
- `EditScreen.tsx`: **`Y`** (Shift+y) opens the dialog plugin's `save` with a
  `MusicXML` filter (`musicxml`, `xml`) and a default name of
  `<bundle name>.musicxml`. On confirm, call `editExportScore`, then flash
  `exported → <file name>` or `export failed: <error>`. Cancel does nothing.
  First confirm `Y` is unbound in `keymap.ts` and the `onKeydown` blocks; if
  it isn't, pick a free key and say so in the PR.
- Add a help-overlay row: `Y  export sheet music (MusicXML)`.

### 4. Docs

- `docs/AGENT-CONTROL.md`: an `export_score` row in the host command table.
- `docs/IMPORT.md` or a short new section in the README: "Printing sheet
  music: press `Y` in the editor, open the `.musicxml` in MuseScore, then
  File → Print / Export PDF." Mention the v1 limits (sixteenth quantisation,
  one voice per hand, no triplets/pedal/dynamics).

## Tests

- `crates/export`, from small `Score`s built by hand or via `build_score`:
  1. One quarter C4 + rests → the document contains `<divisions>8</divisions>`,
     `<fifths>0</fifths>`, `<beats>4</beats>`, `<staves>2</staves>`, both
     clefs, a note with `<step>C</step><octave>4</octave>`,
     `<duration>8</duration>`, `<type>quarter</type>`, and staff 2's
     `<rest measure="yes"/>`.
  2. Chord → `<chord/>` on the 2nd and 3rd notes only.
  3. Tie → `<tie type="start"/>` + `<tied type="start"/>` in bar 1 and the
     stop pair in bar 2.
  4. Accidentals and alter: C♯ has `<alter>1</alter>` + `<accidental>sharp`;
     B♭ in F major has `<alter>-1</alter>` and no `<accidental>`.
  5. A dotted value has `<dot/>`; the tempo mark has `<sound tempo="160"/>`.
  6. A title with `&` and `<` is escaped.
  7. **Well-formedness:** a minimal tag-balance checker in the test (a stack
     over `<x …>` / `</x>` / `<x/>`) passes for a multi-bar two-staff score.
  8. Every measure's staff-1 durations and staff-2 durations each sum to
     `bar_ticks`, and the `<backup>` equals `bar_ticks`.
- `host.rs`: the parity tests include `export_score`.
- Tauri crate: `export_score` writes a file into a temp dir for a 2-note
  piece, returns the path with `.musicxml` appended, and reports `nothing to
  export` for an empty piece.
- Frontend: `tsc` and vitest stay green.

**Manual check (local, not CI):** export a real piece, open it in MuseScore 4,
and check that both staves, ties, key, time signature and tempo marks show
without import warnings. Also re-import the file through `import_score` and
check that the notes come back on the same beats.

## Scope boundaries (do NOT)

- Don't change the M22-A model's rules. If the writer needs something the
  model doesn't provide, raise it in the PR.
- No PDF/SVG rendering, in-app score preview, LilyPond or ABC output (possible
  follow-ups). No compressed `.mxl`.
- No new dependencies except the new crate's dependency on `rockcraft-core`.
- No TUI key/menu binding (host command only).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Tauri crate tests and frontend `tsc`/vitest/build green locally
- [ ] PR opened against `main` from the branch above, `Closes #330`
