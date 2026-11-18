# M27-F — sheet-music export writes finger numbers

> Milestone: M27 — Finger numbers · Issue: #375 · Suggested tier: sonnet
> Branch: `claude/m27-finger-export` · Depends on: M27-A (#370), M22-A (#329),
> M22-B (#330)

## Goal

The MusicXML written by `export_score` includes each note's finger number as a
standard `<fingering>`, so it prints in MuseScore and friends and comes back
on re-import (M27-D).

## Context

- **Blocked until M22 lands.** The notation model (`specs/M22-A-notation-model.md`,
  planned `crates/core/src/notation.rs`) and the exporter
  (`specs/M22-B-musicxml-export.md`, planned `crates/export`) are specced but
  not implemented. Do not start this before both are merged; names below are
  from those specs — follow the code as built.
- M22-A: `build_score(notes: &[Note], ..) -> Score`; a `WrittenNote` has
  `tie_start` / `tie_stop`, and a note split across bar lines or into several
  printable values becomes several tied `WrittenNote`s. Same-onset notes form a
  `Chord(Vec<WrittenNote>)`; duplicate pitches collapse.
- M22-B: each `<note>` already gets a `<notations>` element when it carries
  `<tied>`.
- The other export, the song archive (`.rcsong`, `crates/share`), packs
  `meta.json` verbatim and already carries fingers since M27-A.

## What to do

1. `WrittenNote` gains `pub finger: Option<Finger>`. `build_score` copies it
   from the source `Note` onto the **first** written piece only (the one with
   `tie_stop == false`); later tied pieces get `None`. When duplicate pitches
   collapse into one chord tone, keep the finger if exactly one of them has
   one or they agree; otherwise none.
2. `to_musicxml`: for a `WrittenNote` with a finger, write

   ```xml
   <notations>
     <tied type="start"/>            <!-- when present, as today -->
     <technical><fingering>3</fingering></technical>
   </notations>
   ```

   One `<notations>` per note, `<tied>` before `<technical>` (schema order).
   Chord tones each carry their own.
3. `specs/M22-B-musicxml-export.md` says the user opens the file elsewhere to
   "add fingering"; reword that sentence, since fingering now comes along.
4. Mention fingering in the export paragraph of whichever doc M22-B added.

## Tests

- `build_score`: a fingered note inside one bar keeps its finger; a fingered
  note tied across a bar line has it on the first piece only; an unfingered
  note has none; duplicate-pitch collapse follows the rule above.
- `to_musicxml`: a fingered note emits exactly one `<fingering>`; a tied
  fingered note emits `<tied>` then `<technical>` in one `<notations>`, and its
  continuation emits no `<fingering>`; a piece with no fingers produces output
  byte-identical to before this change.
- Round trip, if M27-D is merged: export a small fingered piece, run it through
  the score importer, and the same notes carry the same fingers. If M27-D is
  not merged yet, leave a `#[ignore]`d test with a note saying why.

## Scope boundaries (do NOT)

- No placement attributes (`placement`, `default-x`…) — notation programs lay
  fingering out themselves.
- No other technical marks or articulations.
- Do not change quantisation, staff assignment or anything else in
  `build_score`.
- No third-party dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #375`
