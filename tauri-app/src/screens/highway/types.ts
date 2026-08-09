// types.ts — domain types for the Spectrum Live note-highway screen.
//
// These mirror the design prototype's shapes (ms-based, with hand info).
// Mapping to core for the live-wiring follow-up (#168):
//   core::NoteSpan is (pitch: u8, start_us, end_us) in MICROSECONDS, carrying
//   the note's effective hand (M14-E). This screen's NoteSpan is
//   { note, start, end, hand } in MILLISECONDS. `liveSong.ts` converts at the
//   boundary (us → ms, "left"/"right" → "L"/"R") instead of editing the engine,
//   so the engine stays the prototype's exact projection.

export interface NoteSpan {
  note: number;
  start: number;
  end: number;
  hand: "L" | "R";
}

export interface SongData {
  title: string;
  artist: string;
  key: string;
  timeSig: string;
  tempoBpm: number;
  notes: NoteSpan[];
  chords: string[];
  LOOP: number;
  BEAT: number;
  BAR: number;
}

export interface HighwayConfig {
  lead?: number;
  bg?: string;
  kbRatio?: number;
  noteGap?: number;
  radius?: number;
  colorMode?: "hands" | "spectrum" | "accent";
  handColors?: { L: string; R: string };
  perspective?: number;
  glow?: number;
  labels?: boolean;
  gridlines?: "none" | "soft";
  keyboard?: "realistic" | "flat" | "strip";
  hitLine?: string;
  scoring?: boolean;
  pitchRuler?: boolean;
  laneTint?: string;
}

export interface KeyInfo {
  note: number;
  x: number;
  w: number;
  cx: number;
  black: boolean;
  wi?: number;
}

export interface KeyLayout {
  whites: KeyInfo[];
  blacks: KeyInfo[];
  byNote: Record<number, KeyInfo>;
}
