# M13-B — import: scanned sheet music (PDF/image) via OMR

> Milestone: M13 — Sheet Music Import · Issue: #246 · Suggested tier: opus
> Branch: `claude/m13-omr-scan-import`

## Goal

Extend score import to **scanned or photographed** sheet music (PDF / PNG / JPG)
by wrapping an off-the-shelf Optical Music Recognition engine. The engine emits
MusicXML, so this task delegates everything downstream to M13-A: pick an engine,
run it, hand over the MusicXML, and be honest about how good the result is.

## Context

- **Depends on #245 (M13-A)**, which defines the MusicXML→`ExtractedChart`
  converter, the `notation` schema block and the `tools/score-import/` sidecar
  this extends. Do not start until that has merged.
- Unlike M13-A, this **is** a lossy inference step. Clean engraved scores
  transcribe well; handwritten or dense piano scores produce errors. The
  deliverable is therefore not just the conversion — it is the confidence
  plumbing that routes a doubtful import into the editor for review instead of
  presenting a wrong chart as fact. Per-note `confidence` already exists in the
  M6-A schema for exactly this (`specs/M6-E-menu-integration.md`'s review step).
- The optional-system-dependency posture is already established twice in this
  repo — copy it rather than inventing a third shape:
  - ffmpeg on the video path (`pipeline.rs::extract_backing`): absent → degrade,
    never fail the build.
  - the fetch hook (`env_fetch_cmd`): `ROCKCRAFT_FETCH_CMD` env override, then a
    conventional path, then a clear actionable error.
- **CI never runs OMR.** No engine, model weights or scans in the repo or the
  workflow.

## What to do

### 1. OMR stage — `tools/score-import/omr.py`

Lives in the M13-A tool dir because it is the same pipeline stage; it produces
MusicXML and hands off to the existing converter.

- `python omr.py --in <scan.pdf|png|jpg> --out <score.musicxml>` for standalone
  use and debugging.
- `convert.py` detects an image/PDF input and routes through `omr.py`
  internally, so **the Rust side keeps exactly one sidecar contract** — same
  `--in`/`--out -`, same `ExtractedChart` JSON on stdout. The only Rust-side
  change should be accepting more file extensions.

### 2. Engine resolution

In priority order, mirroring `env_fetch_cmd`:

1. `ROCKCRAFT_OMR_CMD` — an explicit command, for engines or wrappers we don't
   know about.
2. **Audiveris** on `PATH` (batch export to MusicXML) — the strongest open
   engine, but a JVM dependency.
3. **`oemer`** — pure-Python fallback, `pip`-installable, weaker on dense scores.
4. None found → fail with a message naming all three options and how to install
   them. Never a silent fallback, never a crash, never a build failure.

Record which engine ran in `SourceMeta.extractor_version` (e.g.
`"omr-audiveris-5.3+score-import-0.1"`) so a bad import is traceable to its engine.

Multi-page PDFs: prefer handing the PDF to the engine whole. Only if the chosen
engine is single-image do you rasterize pages and concatenate the resulting parts
in page order — and if you do, say so on stderr, because page-boundary joins are
where measures get lost.

### 3. Confidence — the part that matters

OMR engines generally do not report per-note confidence, so derive it
structurally. Implement as a **pure function over the parsed score** so it is
testable without ever running an engine:

- **Measure-duration check**: a measure whose note durations don't sum to its
  time signature is suspect → every note in it gets `confidence: 0.5`. This is
  the single highest-yield OMR error detector.
- **Range check**: pitches below A0 or above C8 → `confidence: 0.25` (a
  misread ledger line or clef).
- **Engine-reported confidence**, when available → take the minimum with the
  above.
- Everything else → `confidence: 0.9`. Never `1.0` on an OMR path; M13-A's exact
  transform owns that value and the distinction should stay visible.
- Emit a stderr summary: total notes, flagged notes, flagged measure numbers.

### 4. Surfacing the result

- The bundle is written unchanged by M13-A's path (`TrackOrigin::Imported`).
- The import status line in both frontends reports the counts, e.g.
  `imported 412 notes, 37 flagged — review in the editor`. Thread the summary
  through the existing `Progress::Log` events rather than adding a new channel.
- That status message is the whole review affordance in this task. **Do not
  build a review UI** — flagged-note highlighting in the editor is M6-E's
  territory and a separate issue.

### 5. Content policy + docs

- `.pdf` and image scans are already never-committable under M13-A's extended
  `scripts/check-no-media.sh`. Verify the guard rejects a staged `.pdf`; add the
  case if M13-A's version missed it.
- `docs/IMPORT.md`: extend the score-import section with the OMR tier — engine
  options and install notes, the optional-dependency behaviour, the accuracy
  expectation (clean engraved good, handwritten poor), and the fact that OMR
  output is meant to be reviewed before it is trusted.
- `tools/score-import/README.md`: same, from the operator's angle.

## Tests

No OMR engine runs in CI. Test everything around it:

- **Engine resolution**: `ROCKCRAFT_OMR_CMD` wins when set; falls through to
  Audiveris then `oemer` when on a stubbed `PATH`; with nothing available, the
  error names all three options. Use a fake executable on a temp `PATH`.
- **Confidence heuristics** as a pure function over hand-built scores:
  - a 4/4 measure holding 3 beats → all its notes flagged at `0.5`
  - a clean score → all notes `0.9`, nothing flagged
  - a pitch above C8 → `0.25`
  - engine confidence `0.4` on an otherwise clean note → `0.4`, not `0.9`
- **Routing**: `convert.py` given a `.pdf` calls the OMR stage; given a
  `.musicxml` it does not (assert with a stubbed OMR entry point).
- **Summary counts** on stderr match the flagged notes, and stdout stays
  pure JSON.
- A genuine end-to-end run against a real scan is a **manual local check**,
  documented in the README and gated behind a `@pytest.mark.omr` marker that is
  deselected by default.

## Scope boundaries (do NOT)

- Do not vendor, commit or auto-download an OMR engine or model weights.
- Do not commit any scan, PDF, or engine output.
- Do not run OMR in CI or add it to `scripts/setup-dev.sh` as a required dep.
- Do not train, fine-tune or bundle a model. This task selects and wraps.
- **Do not use an LLM or vision model to read the score.** This was considered
  and rejected: VLMs lose ledger lines, mis-scope accidentals and drift on
  vertical alignment in chords, and a hallucinated bar is worse than an OMR
  error because it is plausible rather than obviously broken. If an LLM enters
  this pipeline later it belongs *after* OMR as a validator over structured
  output, never as the transcriber — and that is a separate, evidenced issue.
- Do not modify M13-A's MusicXML converter semantics; this task feeds it.
- Do not build editor review UI (M6-E).
- No new Rust dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `pytest` green in `tools/score-import/` with the `omr` marker deselected
- [ ] `scripts/check-no-media.sh` rejects a staged `.pdf`
- [ ] With no engine installed, a `.pdf` import fails with a message naming
      all three engine options — and the build is unaffected
- [ ] PR against `main` from `claude/m13-omr-scan-import`, `Closes #246`
