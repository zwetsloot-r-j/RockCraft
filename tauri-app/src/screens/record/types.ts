// types.ts — record-screen domain types: the take fixture, engine config, and
// engine-readable state. Mirrors the shapes the design prototype's RecordCanvas
// produced (design/record_screen/rockcraft-proto/record.js + song.js).

export type Hand = "L" | "R";

export interface TakeNote {
  note: number;
  start: number; // ms from loop origin
  end: number; // ms from loop origin
  hand: Hand;
  vel: number; // 0..127
}

export interface Song {
  title: string;
  artist: string;
  key: string;
  tempoBpm: number;
  timeSig: string;
  notes: TakeNote[];
  BEAT: number;
  BAR: number;
  BARS: number;
  LOOP: number;
  chords: string[];
}

export type Viz = "ribbons" | "staff" | "ribbons+staff" | "roll";

// Caller-facing config (every field optional; defaults applied in the engine).
export interface RecordConfig {
  viz?: Viz;
  bg?: string;
  kbRatio?: number;
  window?: number; // ms of capture shown rising above the keys
  labels?: boolean;
  glow?: number; // 0..1 glow strength
  radius?: number; // note corner radius
  noteGap?: number;
  showKeyboard?: boolean;
  recording?: boolean;
  paused?: boolean;
  selectNote?: boolean;
  accent?: string;
  fontMono?: string;
  fontDisp?: string;
  fontMusic?: string;
}

export type ResolvedConfig = Required<RecordConfig>;
