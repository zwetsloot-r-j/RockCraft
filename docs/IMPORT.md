# Content Policy — Import (video and sheet music)

## The rule in plain language

**The tool is shared. The songs are not.**

RockCraft is open-source. Anyone can clone it, build it, and run it against
their own media. What they may not do — and what this repository must never
contain — is the actual video or audio files, the sheet music, or the `.mid`
charts extracted from them. Those are covered by copyright and must stay on your
local machine. **Sheet music is copyrighted exactly like a recording** — an
engraved edition is a protected work in its own right, so a `.pdf` or `.mxl`
score is no more committable than an `.mp4`.

## What is and isn't allowed in git

| Allowed | Not allowed |
|---------|-------------|
| Rust source, scripts, CI config, docs | Video files (`*.mp4`, `*.mkv`, `*.webm`, `*.mov`, `*.m4a`) |
| Curated test fixtures under `fixtures/` | Audio files (`*.mp3`, `*.wav`, `*.ogg`, `*.flac`) outside `fixtures/` |
| `fixtures/**/*.mid` (hand-crafted test MIDIs) | Extracted `.mid` charts anywhere outside `fixtures/` |
| `fixtures/score/**` — synthetic plain-text scores (`*.musicxml`, `*.xml`, `*.abc`, `*.krn`) | Those same score formats anywhere outside `fixtures/` |
| Everything in `.gitignore` is already untracked | Published or opaque scores **anywhere**: `*.pdf`, `*.mxl`, `*.mscz`, `*.sib`, `*.gp3/4/5`, `*.gpx` |
| | Anything in `/import-out/` or `/import-cache/` |

`.mxl` is a zip container and the rest are binary or published formats — none of
them can be reviewed in a diff, so there is no `fixtures/` carve-out for them.
Plain `.musicxml` is the only committable score format, and only for obviously
synthetic test material (a scale, a fabricated chord) — never a real piece.

## How the guardrails work

### 1. `.gitignore`

The following paths and extensions are gitignored at the repo root so that
`git add .` will never accidentally stage them:

- `/import-out/` — where M6 writes extracted chart bundles
- `/import-cache/` — where M6's downloader caches source media
- `/scripts/local/` — your private fetch wrapper (e.g. a `yt-dlp` invocation)
- `*.mp4`, `*.mkv`, `*.webm`, `*.mov`, `*.m4a` — video containers
- Audio globs (`*.mp3`, `*.wav`, etc.) are **not** globally gitignored because
  `fixtures/` may contain small curated `.wav` / SoundFont test assets. The CI
  guard (see below) enforces the boundary instead.

### 2. CI guard (`scripts/check-no-media.sh`)

Every CI run executes this script before the Rust build. It scans all tracked
and staged files and **fails** if it finds:

- Any media extension (video or audio) anywhere in the tree.
- Any published/opaque score format (`.pdf`, `.mxl`, `.mscz`, `.sib`, `.gp*`)
  anywhere in the tree.
- Any `.mid` / `.midi` file **outside** `fixtures/`.
- Any `.musicxml` / `.abc` / `.krn` file **outside** `fixtures/`. A generic
  `.xml` is judged by *content* rather than by name — the repo legitimately
  tracks unrelated XML (Android launcher icons), so only a file whose root
  element is `<score-partwise>` / `<score-timewise>` counts as a score.

A clean tree passes silently. The curated `fixtures/` directory is the only
allowed home for tracked `.mid` or audio test assets.

### 3. Local pre-commit hook (optional)

Run once after cloning to catch violations before they reach CI:

```sh
bash scripts/install-hooks.sh
```

This installs `.git/hooks/pre-commit` that runs the same guard on every
`git commit`.

## Backing audio (the source video's sound)

After the chart is extracted, the pipeline derives an audio track from the
source video so an imported song plays with its real recording behind it by
default. It shells out to **ffmpeg** (`ffmpeg -i <video> -vn backing.wav`),
writing `backing.wav` next to `song.mid` inside the bundle dir under
`import-out/` (gitignored, like the rest of the bundle), and records it in the
bundle's `meta.json` as `"backing"`. Play and Edit then pick it up automatically.

ffmpeg is an **optional** system dependency:

- If ffmpeg is installed (on `PATH`, or pointed at by `ROCKCRAFT_FFMPEG`), the
  import attaches the extracted audio as the backing track.
- If ffmpeg is absent — or the source has no audio stream — the import still
  succeeds; the bundle is simply MIDI-only (`"backing": null`).

The extracted `backing.wav` lives only under `import-out/` and is never tracked
(same boundary the CI guard enforces above).

## Score import (M13-A)

A score file is the second import source, alongside video. It is a
**deterministic transform**, not an extraction: pitch, duration, tempo, metre,
key and staff→hand are all stated explicitly by the source, so there is no CV, no
ML and no inference on this path.

- **Sidecar**: `tools/score-import/` (`convert.py`), invoked as a subprocess by
  the same pipeline as the video extractor and emitting the same
  `ExtractedChart` JSON on stdout.
- **Formats**: `.musicxml`, `.xml`, `.mxl`, `.abc`, `.krn` (whatever `music21`
  reads; those five are the documented and tested set). Scanned PDFs and images
  go through the OMR tier below, which feeds this same converter.
- **Optional dependency**: `music21` is the only requirement, and it is optional
  for RockCraft as a whole. The Rust build never links it; without the venv a
  score import fails with an actionable error naming
  `tools/score-import/requirements.txt`. Every other import path is unaffected.
- **Notated context**: unlike video, a score states its tempo, time signature and
  key, so the written bundle's `meta.grid` / `meta.key` are populated and the
  imported piece opens in the editor already snapping to its own bars.
- **Velocity from the notated dynamics**: levels (`ppp`…`fff`), hairpin ramps and
  accent/marcato, resolved *per staff* so a `p` left hand under an `f` right hand
  stays two curves. This matters more here than on the video path, which fills
  velocity from the source audio and ships a real `backing.wav` — a score import
  has neither, so the synth is the only sound. `mf` maps to the parser's
  `DEFAULT_VELOCITY`, and a passage with no dynamic in effect stays `null`, so a
  score with no markings sounds exactly as it did before the pass existed.
- **Single-tempo limitation**: note times are absolute microseconds and stay
  correct across a tempo change, but `core::Grid` holds one BPM, so the editor's
  bar lines drift after the first change. Such a score imports with a loud
  warning on stderr rather than being rejected or silently mangled.

Entry points: the TUI's *"Import score or scan…"* menu item, or the `import_score`
host command over the agent-control socket. Full conversion rules and known
limitations: `tools/score-import/README.md`.

## Scanned sheet music (OMR, M13-B)

A **scan or photograph** of a page — `.pdf`, `.png`, `.jpg`, `.jpeg`, `.tif`,
`.tiff`, `.bmp` — is the third import source. It is the same entry point as a
score file (`import_score`, the same TUI menu item, the same
`ImportInput::Score`): the sidecar looks at the extension and decides for itself
whether the input needs optical music recognition first. Nothing on the Rust side
knows or cares.

**This tier is inference, not transformation.** M13-A converts what a file
*states*; OMR *guesses* what a picture shows. Clean engraved scores transcribe
well. Handwritten scores, dense piano writing, skewed phone photos and
low-resolution scans produce real errors. So the result is presented as something
to review, never as fact.

### Engines (all optional, none bundled)

Resolved in this order, mirroring how the fetch hook and ffmpeg are treated:

1. **`$ROCKCRAFT_OMR_CMD`** — any engine or wrapper you already have, invoked as
   `<cmd> <scan-file> <output.musicxml>`. It must write MusicXML to that second
   path. Assumed to handle whatever it is given, PDFs included.
2. **[Audiveris](https://github.com/Audiveris/audiveris)** on `PATH` — the
   strongest open engine. Needs a JVM; reads multi-page PDFs whole and exports
   compressed `.mxl`, which the sidecar unpacks to plain XML.
3. **[`oemer`](https://github.com/BreezeWhite/oemer)** — `pip install oemer`.
   Pure Python, no JVM, weaker on dense scores, and **single-image only**: a PDF
   is rasterized page by page with `pdftoppm` (poppler-utils, overridable with
   `$ROCKCRAFT_PDFTOPPM`) and the pages are concatenated in order. Page-boundary
   joins are where measures go missing, so this path says so loudly on stderr.
4. **None installed** → the import fails with a message naming all three and how
   to install them. Never a silent fallback, never a crash, and never a build
   failure: the Rust build links none of this, and every other import path is
   unaffected. **CI never runs OMR** — no engine, no model weights, no scans.

Which engine ran is recorded in the bundle's `extractor_version`, e.g.
`omr-audiveris-5.3+score-import-0.1`, so a chart that reads wrong is traceable to
the engine that produced it.

### Confidence — why the import tells you to review it

OMR engines generally report no per-note confidence, so it is derived
*structurally* from what the transcription says about itself:

| Check | Confidence |
|-------|------------|
| A measure whose notes and rests don't cover its time signature — the highest-yield OMR error there is | `0.5` for every note in it |
| A pitch outside a piano's range (below A0, above C8) — a misread ledger line, clef or octave mark | `0.25` |
| Confidence the engine itself reported, when it reports one | taken as a further minimum |
| Everything else | `0.9` — **never `1.0`**; that value means "the source stated this", which only M13-A can claim |

Both checks are pure functions over the parsed score, so they are fully tested in
CI without an engine ever running (`tools/score-import/tests/test_confidence.py`).

The import reports the outcome on its status line in both frontends:

```
imported 412 notes, 37 flagged — review in the editor
```

with the suspect measure numbers in the log pane below it. **That message is the
whole review affordance today.** The per-note values live on the
`ExtractedChart` JSON, which is where M6-E's review step will read them, but
`chart_to_timeline` does not yet thread them into `core`'s `Timeline` — so what
survives into the written bundle is the counts you were told, not a per-note
marking. Highlighting flagged notes inside the editor is M6-E's job and a separate
issue.

### Content policy for scans

Same rule, no exceptions: **the tool is shared, the songs are not.** A `.pdf` is
rejected anywhere in the tree by `scripts/check-no-media.sh` — it is a published
work and unreviewable in a diff. Scan **images** cannot be banned by extension,
because the repo legitimately tracks `.png` design mockups and app icons; the
rule there is human, not mechanical: never commit a scan. Keep your scans outside
the repo (or under the gitignored `import-cache/`), and note that no fixture,
test or CI step here needs one — the manual end-to-end check reads a scan from
`$ROCKCRAFT_OMR_SCAN` on your own machine.

There is also, deliberately, **no LLM or vision model on this path.** It was
considered and rejected: VLMs lose ledger lines, mis-scope accidentals and drift
on vertical alignment in chords, and a hallucinated bar is worse than an OMR error
because it is plausible rather than obviously broken. If a model ever enters this
pipeline it belongs *after* OMR, as a validator over structured output — never as
the transcriber.

## Where downloading lives

The actual media download step is intentionally **not** part of this repo.
It lives in `scripts/local/` on your machine (gitignored), where you can
place a private wrapper around `yt-dlp`, `ffmpeg`, or whatever tool you use
to fetch and prepare source files. M6-D documents the expected interface
between that wrapper and the import pipeline.

## Summary

- Clone, build, contribute to the tool freely.
- Keep your video, audio, sheet music, and extracted charts on your own machine.
- `import-out/` and `import-cache/` are always gitignored.
- The CI guard is the last line of defense — it will reject any PR that
  accidentally contains disallowed content.
