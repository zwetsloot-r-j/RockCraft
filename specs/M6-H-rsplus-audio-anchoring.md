# M6-H — Rocksmith+ import: anchor notes to the audio, keep detected hands

> Milestone: M6 — Video Import · Issue: #366 · Suggested tier: opus
> Branch: `claude/m6-rsplus-audio`

## Goal

Make a Rocksmith+ (M6-G) import match what the song actually plays. On real
recordings the visual chart has three systematic errors, all visible in fast
passages:

- **MISS text read as the note.** When the player doesn't play, Rocksmith+ draws
  white "MISS" text, tinted in the lane colour, over the hit line for ~0.4 s. A
  short block is on the hit line for only 2–3 frames, so the extractor often
  reads the text instead: the onset comes out ~0.37 s late, and the duration is
  the text's. When it catches both, one note becomes two.
- **Split held notes.** One held bar comes out as two notes.
- **Ghost notes.** Short glows at the highway edge read as notes that are not in
  the chart.

The recording's audio is Rocksmith+ playing the chart itself (MIDI playback,
independent of the player — a missed note still sounds), so it is the reference
for when each note starts.

Also: the importer drops the hand each note was detected with. Keep it.

## What to do

### Sidecar: `synthesia_extract/rsplus_audio.py`

`fuse_rsplus_chart(chart, samples, sample_rate) -> ExtractedChart`, used by
`extract.py --audio-fusion` when the layout is `rsplus` (the Synthesia path
keeps `audio.fuse_chart`).

1. Unsuitable audio (`audio.assess_suitability`): fall back to `fuse_chart`.
2. Clock: `audio.estimate_clock_offset`; shift the notes, stamp
   `source.clock_offset_us`.
3. Onsets: broadband spectral-flux peaks (~46 ms window, ~5 ms hop, ≥ 40 ms
   apart).
4. Attacks per onset: the positive change of the square-root magnitude spectrum
   across the onset (a ~93 ms window just after it minus one just before it, up
   to 5 kHz), decomposed by non-negative least squares on harmonic templates
   (harmonics 1–11, weight 1/h^0.7) for every pitch from the chart's lowest − 12
   to highest + 12. A pitch is attacked when its coefficient is ≥ 0.2 × the
   onset's largest.
5. Per pitch, in time order, each visual note is:
   - **anchored** to the nearest unclaimed same-pitch attack within ±0.1 s;
   - else **re-anchored** to an unclaimed same-pitch attack 0.2–0.5 s earlier
     (MISS text read in place of the block);
   - else **dropped** if the previous kept note on the pitch is short and began
     0.2–0.6 s earlier (that note's MISS text);
   - else **merged** into the previous kept note on the pitch when it ended
     ≤ 0.35 s earlier with no attack in between (a split held note);
   - else **dropped** (no attack behind it: a ghost).
6. Durations: a short note (visual duration ≤ 0.15 s, or re-anchored) lasts a
   fixed sixteenth at the editor's default 120 bpm (125 ms); a longer one keeps
   its visual end. No note runs into the next note on its pitch.
7. Velocity from attack strength (log scale around the song's median, 20–127).
8. Safety: when fewer than half the visual notes find an attack, the audio is
   not the chart (the player played along, or it is a different mix): fall back
   to `fuse_chart`.
9. `source.audio_fusion` records the counts of each outcome.

### Importer: keep detected hands (`crates/import`)

When a chart has notes with a known hand (`Left`/`Right`), the bundle's
`meta.json` gets the split line that agrees with the most of them
(`hand_split`), plus a `hand_overrides` entry for each known-hand note the split
gets wrong. `Unknown` notes follow the split. A chart with no known hands keeps
`hand_split` unset, exactly as before. The author can still change any of it in
the editor (M14-E).

## Tests (synthetic only)

`tests/test_rsplus_audio.py`, rendered audio plus hand-written visual charts:
anchoring tightens onsets to the audio; a MISS-late note moves back to its
attack; a MISS fragment after a found short note is dropped; a split held note
is merged; a note with no attack is dropped; short notes get the fixed
sixteenth and never overlap the next same-pitch note; velocity follows
loudness; hands are kept; an unrelated chart falls back; a CLI run on a
Rocksmith+ frames directory uses the Rocksmith+ fusion.

`crates/import`: the best split and overrides for a two-hand chart, a crossover
note becomes an override, all-`Unknown` sets nothing; the written bundle
carries them and they survive a load.

## Scope boundaries (do NOT)

- Do not commit any real capture, frame, extracted chart or audio.
- Do not change the `ExtractedChart` schema or the M6-G visual extraction.
- Do not change the Synthesia path's audio fusion.
- No finger numbers (a possible follow-up).
- No new Python dependencies.

## Acceptance

- [ ] `pytest` in `tools/synthesia-extract` green
- [ ] `cargo fmt --all --check` / `cargo clippy --workspace --all-targets` /
      `cargo test --workspace` clean
- [ ] PR opened against `main` from the branch above, `Closes #366`
