// liveSong.ts — convert a backend `PlayInfo` bundle into the engine's `SongData`
// (#168). The highway engine is ms-based; a core bundle is µs, so we map at this
// boundary (the engine stays the prototype's exact projection — see types.ts).
//
// Hand comes straight from the backend now (M14-E): `core` resolves each note's
// *effective* hand — the piece's per-note override when it has one, else its
// split line — so a crossover colours on the hand its author marked and this
// layer never re-derives the rule.

import type { PlayInfo } from "../../ipc/types";
import type { NoteSpan, SongData } from "./types";

/**
 * Build the engine `SongData` from a loaded bundle.
 *
 * Spans arrive already shifted by the pre-roll (in ms). `LOOP` is set past the
 * end of the song plus the lead window so the engine's loop-copy draw never
 * wraps a visible second copy — live bundles play once, not on a loop.
 */
export function songFromInfo(info: PlayInfo): SongData {
  const notes: NoteSpan[] = info.notes
    .map((n) => ({
      note: n.note,
      start: n.start,
      end: n.end,
      hand: (n.hand === "left" ? "L" : "R") as "L" | "R",
    }))
    // Sort by start so the highway engine can binary-search the visible
    // window (viewport culling for dense charts). The engine also sorts
    // defensively, but doing it here keeps the boundary output canonical.
    .sort((a, b) => a.start - b.start);

  // Bar/beat grid at the piece's real tempo (the backend bar-aligns the pre-roll
  // shift, so timeline bars land on these play-clock bar lines).
  const bpm = info.bpm > 0 ? info.bpm : 120;
  const beatsPerBar = info.beats_per_bar > 0 ? info.beats_per_bar : 4;
  const BEAT = 60000 / bpm; // ms per beat
  const BAR = BEAT * beatsPerBar;

  const leadMs = info.lead_us / 1000;
  const durMs = info.duration_us / 1000;
  // One-shot: keep LOOP comfortably beyond the last note + lead so wraps stay
  // off-screen for the whole take.
  const loop = Math.max(durMs + leadMs * 2, BAR);

  return {
    title: info.title,
    artist: "",
    key: "",
    timeSig: `${beatsPerBar}/4`,
    tempoBpm: bpm,
    notes,
    chords: [],
    LOOP: loop,
    BEAT,
    BAR,
  };
}
