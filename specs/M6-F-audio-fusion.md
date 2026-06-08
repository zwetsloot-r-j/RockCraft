# M6-F — sidecar (phase 2): audio-fusion (velocity + onset refinement)

> Milestone: M6 — Video Import · Issue: #118 · Suggested tier: opus
> Branch: `claude/m6-audio-fusion`

## Goal

Enrich the vision-extracted chart using the video's **audio when it is clean
piano** (a MIDI render or solo-piano recording): run piano automatic
transcription, align onsets to the visual notes, and fill in **velocity** plus
sub-frame onset/offset refinement. For full-mix original audio, safely no-op and
leave the visual notes authoritative.

## Context

- Extends the M6-C (#115) sidecar in `tools/synthesia-extract/`; emits the same
  M6-A (#113) JSON, now with `velocity` populated and tighter timing.
- Vision gives pitch/timing/hand but **no dynamics**; clean-piano audio recovers
  velocity and tightens onsets. Full-band audio cannot be cleanly transcribed to
  the piano part, so detection-and-skip matters.
- Use an existing piano-transcription approach (e.g. an onsets/frames-style
  model or `basic-pitch`); document the dependency. Keep it optional behind a
  flag so M6-C alone still works.

## What to do

- `extract.py --in <video> --audio-fusion` (or a separate `fuse.py`):
  1. Demux/transcribe the audio to candidate (pitch, onset, offset, velocity).
  2. **Suitability check:** if the audio looks like a full mix (vocals/drums/
     broadband energy) rather than isolated piano, **skip fusion** and return the
     visual notes unchanged (set a `SourceMeta`/log flag explaining why).
  3. **Fuse:** match each visual note to a transcription event within a
     tolerance window (pitch equal, onset within the visual's frame
     uncertainty). On a match, copy `velocity` and nudge `start_us`/`dur_us`
     toward the audio estimate. Unmatched visual notes keep visual timing and a
     default velocity. Visual **pitch stays ground truth** (never overridden by
     a noisy transcription).
- Emit M6-A JSON with `velocity: Some(..)` and updated `confidence`.

## Tests (synthetic only)

- Build on M6-C's synthetic clip: also render a **clean-piano** audio track with
  known per-note velocities aligned to the bars. Assert the fused chart has the
  correct velocities and onsets at least as tight as visual-only.
- A synthetic **full-mix decoy** (piano + noise/other tones) triggers the
  suitability skip: fused output equals visual-only (no corruption).
- No real songs/audio committed — generate synthetic audio in the test.

## Scope boundaries (do NOT)

- Do not let audio override visual pitch or invent notes the visual didn't show.
- Do not make audio-fusion mandatory — M6-C must still produce a chart without
  it.
- No downloading; no real copyrighted audio in the repo (M6-B).

## Acceptance

- [ ] Synthetic clean-piano fusion: correct velocities, tighter onsets
- [ ] Synthetic full-mix decoy: safe no-op (visual notes unchanged)
- [ ] Documented optional dependency + flag; M6-C path unaffected when off
- [ ] PR against `main` from `claude/m6-audio-fusion`, `Closes #118`
