// format.ts — timecode + bar:beat formatting, ported from record-ui.jsx (`tc`)
// and record-protos.jsx (`bbOf`).

import type { Song } from "./types";

// MM:SS.cc style timecode (seconds + centiseconds), e.g. "7:54".
export const tc = (ms: number): string => {
  const s = Math.floor(ms / 1000);
  const cs = Math.floor((ms % 1000) / 10);
  return `${s}:${String(cs).padStart(2, "0")}`;
};

// 1-based bar.beat label for a position in ms, e.g. "1.1".
export const bbOf = (ms: number, song: Song): string =>
  `${Math.floor(ms / song.BAR) + 1}.${Math.floor((ms % song.BAR) / song.BEAT) + 1}`;
