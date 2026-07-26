# score-import — score file or scan → chart bundle (M13-A / M13-B)

A Python **sidecar** that turns a *local* score file (MusicXML and the other
formats [`music21`](https://www.music21.org/) reads) **or a scanned page** into
the M6-A `ExtractedChart` JSON consumed by the Rust `rockcraft-import` crate.

Two tiers, one output format and one entry point:

- **M13-A — score files.** Not an inference problem. Pitch, duration, tempo,
  metre, key and staff→hand are all stated explicitly by the source, so the
  conversion is a **deterministic transform**: no computer vision, no ML, no LLM,
  no heuristics beyond the documented staff→hand rule. Its tests assert onsets and
  durations exact to the microsecond, and every note comes out at
  `confidence: 1.0`.
- **M13-B — scans and PDFs.** `convert.py` spots an image or PDF input and routes
  it through `omr.py`, which drives an external optical music recognition engine
  to produce MusicXML and then hands that to the same transform. This tier **is**
  lossy, so its notes carry a derived `confidence` and the run tells you how much
  of the result to distrust. See [OMR](#omr-scans-and-pdfs-m13-b) below.

The Rust side sees **one contract either way** — same `--in`/`--out`, same JSON on
stdout — so nothing outside this directory has to know whether OMR ran.

This is **not** a workspace crate. It runs as a standalone process, invoked by
the import pipeline as a subprocess (`crates/import/src/pipeline.rs`).

## Setup

```bash
cd tools/score-import
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
```

`music21` is the only runtime dependency. It is an **optional** system
dependency of RockCraft as a whole: the Rust build never links it, and every
other import path works without it. Without the venv, a score import fails with
an actionable `SidecarMissing` error naming this file.

## Usage

```bash
python convert.py --in <score-file|scan> --out <chart.json>|-   # '-' = stdout
                  [--title "Name"] [--hand-map 0=right,1=left]
```

**stdout carries only the JSON.** Warnings, dropped-element counts, OMR engine
output and the summary lines all go to stderr, because the pipeline's
`run_sidecar` parses stdout.

From the app: the TUI's *"Import score or scan…"* menu entry, or the
`import_score` host command over the agent-control socket
(`{"command":"import_score","path":"…"}`). Both accept either kind — the picker
lists scans alongside score files.

## Supported formats

**Score files** — the documented and tested set is **`.musicxml`, `.xml`, `.mxl`,
`.abc`, `.krn`**. `music21` reads more than that, and `convert.py` will happily
try anything it accepts — but only these are covered by the pipeline's file
filters and this project's tests.

**Scans** — `.pdf`, `.png`, `.jpg`, `.jpeg`, `.tif`, `.tiff`, `.bmp`. Routed
through OMR first; needs an engine installed (next section).

## Conversion rules

Each rule has a test in `tests/test_convert.py`.

1. **Parse** with `music21.converter.parse`.
2. **Repeats are expanded** into a linear score — repeats, voltas, D.C., D.S.
   and coda jumps all unroll, so a repeated bar yields twice the notes.
   Malformed repeat structures are common in real files: if expansion fails, the
   score is imported *as written* (repeated sections appear once) with a warning
   on stderr, rather than failing the import.
3. **Ties are merged**: a tied pair becomes one note of the summed duration, not
   two. Adjacent *untied* notes of the same pitch stay separate.
4. **Chords** become one note per pitch, sharing start and duration.
5. **Timing** is integrated over a tempo map built from the score's metronome
   marks, so a tempo change moves every later note correctly:
   `start_us = round(seconds × 1e6)`, `dur_us = max(1, …)`.
6. **Hand** comes from the staff/part: upper staff (or first part) → `Right`,
   lower staff (or second part) → `Left`. A single staff, or 3+ parts, →
   `Unknown` — one staff simply carries no hand information, and guessing would
   be worse than saying so. `--hand-map` overrides both.
7. **Velocity** is `null` unless the source states one explicitly, letting the
   Rust parser's `DEFAULT_VELOCITY` apply. Dynamics markings are *not* turned
   into velocities: a synthesized number would be indistinguishable from a real
   one downstream.
8. **Dropped by design** — counted on stderr, never silently swallowed: grace
   notes, ornament realizations (the principal note is kept), unpitched and
   percussion notes, pedal marks, articulations, lyrics.
9. **`confidence: 1.0`** on every note. This transform is exact. The OMR path is
   the one exception — see below.
10. **`notation`** carries the tempo, time signature and key as *notated*, which
    is what gives an imported piece a grid its bars actually snap to.

## OMR: scans and PDFs (M13-B)

`omr.py` **selects and drives an existing engine**. There is no model here, no
training, no vision code and deliberately no LLM: a hallucinated bar is worse than
an OMR error, because it is plausible rather than obviously broken.

### Installing an engine

Nothing is bundled or downloaded. Install **any one** of these; resolution order
is exactly this list:

```bash
# 1. Bring your own — invoked as `<cmd> <scan-file> <output.musicxml>`.
export ROCKCRAFT_OMR_CMD="my-omr --whatever-flags"

# 2. Audiveris — strongest, needs a JVM, reads PDFs whole.
#    https://github.com/Audiveris/audiveris — install, then put `audiveris` on PATH.

# 3. oemer — pure Python, no JVM, weaker on dense scores, single-image only.
pip install oemer
sudo apt-get install poppler-utils   # for `pdftoppm`, to rasterize PDFs for it
```

With **none** installed, a scan import exits 2 with a message naming all three and
how to install them. It never falls back silently, never crashes, and never
affects the build — the Rust side links none of this, and score-file imports,
video imports and everything else keep working. **CI never runs OMR.**

### Accuracy — what to expect

| Source | Expect |
|--------|--------|
| Clean engraved score, ≥300 DPI scan | good; most bars correct |
| Dense piano writing, many voices per staff | frequent errors, especially in inner voices |
| Handwritten manuscript | poor; treat the output as a rough skeleton |
| Phone photo, skewed or low-resolution | poor; rescan flat and higher-resolution first |

**OMR output is meant to be reviewed before it is trusted.** That is not a
disclaimer, it is the design: the run tells you how many notes it doubts and which
measures they are in, so you know to go and look.

### Confidence rules

Derived structurally — engines generally report none — and each rule has a test in
`tests/test_confidence.py`:

| Check | Confidence |
|-------|------------|
| A measure whose notes and rests don't **cover** its time signature (short, gapped, or overfull) | `0.5` on every note in it |
| A pitch outside A0..C8 — a misread ledger line, clef or octave mark | `0.25` |
| Confidence the engine reported, if it reports one (`editorial.confidence`) | taken as a further minimum |
| Everything else | `0.9` — **never `1.0`** on an OMR path |

The measure check honours two real-world shapes rather than crying wolf on them: a
note tied over the barline (which rule 3 merges into one long note) does not make
either bar look wrong, and a bar whose score *declares* it partial (`paddingLeft`,
a written pickup) is left alone. An **undeclared** short bar is still flagged —
which is what an engine emits for a pickup it failed to recognise, and the safe
direction to err on a path whose whole point is "review this".

Every note's confidence is decided by object identity, not by time ranges, so a
doubtful bar in the left staff flags the left staff only.

### What a run reports

```
using OMR engine audiveris: /usr/bin/audiveris
OMR is an inference step, not a transform: … Review the result before trusting it.
suspect measures (2): 12, 40
converted 412 notes -> - (bpm=90)
omr: imported 412 notes, 37 flagged — review in the editor
```

That last line's `omr: ` prefix is a small protocol: both frontends lift it out of
the import log and put it on their status line. It is mirrored in Rust as
`rockcraft_import::OMR_SUMMARY_PREFIX` and in TypeScript as
`tauri-app/src/ipc/omr.ts` — change all three or none.

### Debugging a bad scan import

`omr.py` is also a standalone CLI, so you can look at what the engine actually
produced before blaming the conversion:

```bash
python omr.py --in ~/scans/page.pdf --out /tmp/page.musicxml
python convert.py --in /tmp/page.musicxml --out -      # convert it as a score file
```

If `/tmp/page.musicxml` is already wrong, it is the engine's fault; if it is right
and the chart is wrong, it is this project's.

### Multi-page PDFs

Audiveris (and any `$ROCKCRAFT_OMR_CMD`) is handed the PDF whole, which is always
preferable. Only a single-image engine forces the fallback: pages are rasterized
at 300 DPI, transcribed one at a time, and concatenated by part index in page
order — with a warning on stderr, because page-boundary joins are exactly where
measures go missing and a page that disagrees about how many staves there are
cannot be stitched silently.

## Known limitations

- **One tempo per piece.** Note times are absolute microseconds and stay correct
  across a tempo change, but `core::Grid` holds a single BPM, so the editor's bar
  lines drift after the first change. A score with more than one tempo is
  imported with a loud warning; adding a tempo map to `Grid` is a separate task.
- **No tempo mark** → 120 BPM is assumed, warned about, and written into
  `notation.bpm` so the grid at least matches the note times.
- **A key signature with no mode is omitted, not guessed.** The same accidentals
  fit a major key and its relative minor. Modes beyond major and natural minor
  are omitted for the same reason — `core::Key` models only those two.
- **Hand is dropped downstream.** The chart carries it, but
  `chart_to_timeline` does not yet thread it into `core`'s `Timeline`.
- **Confidence is reported, not persisted.** The counts reach the import status
  line, and the per-note values are on the `ExtractedChart` JSON where M6-E's
  review step will read them — but `chart_to_timeline` drops `confidence` exactly
  as it drops `hand`, so the written bundle does not carry it. Highlighting
  low-confidence notes inside the editor is M6-E's job.

## Test fixtures

The fixtures live in `fixtures/score/` at the repository root, reusing the
existing `fixtures/` carve-out in the content-policy guard
(`scripts/check-no-media.sh`). They are hand-written and obviously synthetic — a
C-major scale, a fabricated two-hand chord — never a real piece.

**Never commit a real score, and never commit a scan.** `.pdf`, `.mxl`, `.mscz`,
`.sib` and Guitar Pro files are rejected anywhere in the tree by the guard; `.mxl`
in particular is a zip container and therefore unreviewable in a diff, so plain
`.musicxml` is the only committable score format. Scan *images* can't be banned by
extension — the repo tracks `.png` design mockups and app icons — so that one is
on you. Nothing here needs a committed scan: the OMR tests use a **stub engine**,
and the manual end-to-end check reads a scan from your own machine. See
`docs/IMPORT.md`.

## Tests

```bash
python3 -m pytest        # from tools/score-import/
```

No OMR engine runs, by design — that is why this suite is safe in CI. Engine
resolution is tested against a stubbed `PATH`, the confidence heuristics against
hand-built `music21` scores, and one end-to-end run goes through the real
`convert.py` with a stub `$ROCKCRAFT_OMR_CMD` that "transcribes" a fixture.

The single test that *does* need an engine is marked `omr` and deselected by
default. Run it locally against your own material:

```bash
ROCKCRAFT_OMR_SCAN=~/scans/my-page.pdf python3 -m pytest -m omr
```

It asserts that the pipeline produces a reviewable chart — not a correct one.
