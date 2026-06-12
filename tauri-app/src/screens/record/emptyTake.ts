// emptyTake.ts — an empty recording "take" used to start the Record screen
// idle (#187). It carries the standard 4/4 · 120 BPM tempo scaffolding (so the
// staff/grid renders a bare, non-degenerate timeline) but no notes — the Ember
// Lantern fixture is retired, and live MIDI fills the take from here.

import type { Song } from "./types";

const BEAT = 500; // ms (120 BPM)
const BAR = BEAT * 4; // 2000 ms
const BARS = 4;
const LOOP = BAR * BARS; // 8000 ms

export const EMPTY_TAKE: Song = {
  title: "",
  artist: "",
  key: "—",
  tempoBpm: 120,
  timeSig: "4/4",
  notes: [],
  BEAT,
  BAR,
  BARS,
  LOOP,
  chords: [],
};
