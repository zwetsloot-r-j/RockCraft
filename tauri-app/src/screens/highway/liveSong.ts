// liveSong.ts — convert a backend `PlayInfo` bundle into the engine's `SongData`
// (#168). The highway engine is ms-based with hand info; a core bundle is
// pitch+us with no hand, so we map at this boundary (the engine stays the
// prototype's exact projection — see types.ts). Hand is derived by a simple
// pitch split (middle C) purely for coloring in "hands" mode; the active config
// colors by pitch (spectrum), so the value is cosmetic.

import type { PlayInfo } from "../../ipc/types";
import type { NoteSpan, SongData } from "./types";

/** Middle C — the pitch split used to derive a nominal hand for coloring. */
const SPLIT = 60;

/** A reasonable default grid for a bundle without tempo metadata: 120 BPM 4/4. */
const BEAT = 500; // ms
const BAR = BEAT * 4;

/**
 * Build the engine `SongData` from a loaded bundle.
 *
 * Spans arrive already shifted by the pre-roll (in ms). `LOOP` is set past the
 * end of the song plus the lead window so the engine's loop-copy draw never
 * wraps a visible second copy — live bundles play once, not on a loop.
 */
export function songFromInfo(info: PlayInfo): SongData {
  const notes: NoteSpan[] = info.notes.map((n) => ({
    note: n.note,
    start: n.start,
    end: n.end,
    hand: n.note < SPLIT ? "L" : "R",
  }));

  const leadMs = info.lead_us / 1000;
  const durMs = info.duration_us / 1000;
  // One-shot: keep LOOP comfortably beyond the last note + lead so wraps stay
  // off-screen for the whole take.
  const loop = Math.max(durMs + leadMs * 2, BAR);

  return {
    title: info.title,
    artist: "",
    key: "",
    timeSig: "4/4",
    tempoBpm: 120,
    notes,
    chords: [],
    LOOP: loop,
    BEAT,
    BAR,
  };
}
