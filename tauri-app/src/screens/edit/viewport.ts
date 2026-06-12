// viewport.ts — pure step/pitch ↔ pixel mapping for the edit grid.
//
// The edit grid is a *horizontal* piano-roll: time runs left → right (µs → x),
// pitch runs bottom → up (MIDI → y), matching the `edit.rs` mental model of
// (step × pitch). All the grid maths is `snapshot`-relative and mirrors
// `core::grid` (`crates/core/src/grid.rs`): step / beat / bar sizing is derived
// from `bpm`, `time_sig` and `subdivision` exactly as the Rust `Grid` does, so
// the rendered gridlines line up with the cursor steps the composer produces.
//
// This module is pure geometry — no canvas, no DOM. The canvas renderer
// (EditCanvas) and the screen consume it; keeping it separate keeps the mapping
// reviewable in isolation.

import type { Subdivision, TimeSig } from "../../ipc/types";

/** MIDI range of an 88-key board (A0..C8), mirroring `keyboard.rs`. */
export const LOWEST_MIDI = 21;
export const HIGHEST_MIDI = 108;

/** Pitch lanes shown above/below the cursor before scrolling kicks in. */
const VISIBLE_SEMITONES_HALF = 24; // ±2 octaves around the cursor

/** Grid timing derived from a snapshot — the µs durations `core::Grid` exposes. */
export interface GridTiming {
  /** Microseconds per quarter note. */
  quarterUs: number;
  /** Microseconds per subdivision step (the snap unit). */
  stepUs: number;
  /** Microseconds per beat (time-sig beat unit). */
  beatUs: number;
  /** Microseconds per bar. */
  barUs: number;
}

/** µs of one quarter note at `bpm` — mirrors `Grid::quarter_us`. */
export function quarterUs(bpm: number): number {
  return Math.floor(60_000_000 / bpm);
}

/** µs of one subdivision step — mirrors `Grid::step_us`. */
export function stepUs(bpm: number, sub: Subdivision): number {
  const q = quarterUs(bpm);
  switch (sub) {
    case "Quarter":
      return q;
    case "Eighth":
      return Math.floor(q / 2);
    case "Sixteenth":
      return Math.floor(q / 4);
    case "ThirtySecond":
      return Math.floor(q / 8);
    case "EighthTriplet":
      return Math.floor(q / 3);
    case "SixteenthTriplet":
      return Math.floor(q / 6);
  }
}

/** µs of one beat (beat unit) — mirrors `Grid::beat_us`. */
export function beatUs(bpm: number, ts: TimeSig): number {
  return Math.floor((quarterUs(bpm) * 4) / ts.beat_unit);
}

/** µs of one bar — mirrors `Grid::bar_us`. */
export function barUs(bpm: number, ts: TimeSig): number {
  return Math.floor((ts.beats_per_bar * quarterUs(bpm) * 4) / ts.beat_unit);
}

/** Bundle the four grid durations for a `(bpm, time_sig, subdivision)`. */
export function gridTiming(
  bpm: number,
  ts: TimeSig,
  sub: Subdivision,
): GridTiming {
  return {
    quarterUs: quarterUs(bpm),
    stepUs: Math.max(1, stepUs(bpm, sub)),
    beatUs: Math.max(1, beatUs(bpm, ts)),
    barUs: Math.max(1, barUs(bpm, ts)),
  };
}

/** `(bar, beat)` for a µs position — mirrors `Grid::bar_beat_of` (0-indexed). */
export function barBeatOf(
  us: number,
  bpm: number,
  ts: TimeSig,
): { bar: number; beat: number } {
  const bar = barUs(bpm, ts);
  const beat = beatUs(bpm, ts);
  return { bar: Math.floor(us / bar), beat: Math.floor((us % bar) / beat) };
}

const clamp = (v: number, lo: number, hi: number): number =>
  Math.max(lo, Math.min(hi, v));

/**
 * A scroll/zoom viewport over the grid: maps song time ↔ x and MIDI pitch ↔ y
 * within a `width × height` px canvas region. Built each frame from the current
 * snapshot so cursor-follow scrolling tracks the cursor (or playhead, while
 * playing) without any persistent state.
 */
export class Viewport {
  readonly width: number;
  readonly height: number;
  /** Pixels per microsecond on the time (x) axis. */
  readonly pxPerUs: number;
  /** Pixel height of one pitch lane. */
  readonly laneH: number;
  /** Song µs at the left edge (x = 0). */
  readonly originUs: number;
  /** Lowest visible MIDI pitch (the bottom lane). */
  readonly pitchLo: number;
  /** Highest visible MIDI pitch (the top lane). */
  readonly pitchHi: number;

  constructor(opts: {
    width: number;
    height: number;
    /** µs spanned across the full canvas width (the horizontal zoom). */
    spanUs: number;
    /** µs to keep anchored: cursor step time (or playhead while playing). */
    anchorUs: number;
    /** MIDI pitch of the cursor — the vertical scroll anchor. */
    anchorPitch: number;
  }) {
    this.width = Math.max(1, opts.width);
    this.height = Math.max(1, opts.height);
    this.pxPerUs = this.width / Math.max(1, opts.spanUs);

    // Anchor the time axis a third of the way in so there's history to the left
    // and lookahead to the right of the cursor/playhead.
    this.originUs = Math.max(0, opts.anchorUs - opts.spanUs / 3);

    // Vertical window: ±2 octaves around the cursor, clamped to the 88-key
    // range, then sized so each lane gets an integer-ish height.
    let lo = opts.anchorPitch - VISIBLE_SEMITONES_HALF;
    let hi = opts.anchorPitch + VISIBLE_SEMITONES_HALF;
    if (lo < LOWEST_MIDI) {
      hi += LOWEST_MIDI - lo;
      lo = LOWEST_MIDI;
    }
    if (hi > HIGHEST_MIDI) {
      lo -= hi - HIGHEST_MIDI;
      hi = HIGHEST_MIDI;
    }
    this.pitchLo = clamp(lo, LOWEST_MIDI, HIGHEST_MIDI);
    this.pitchHi = clamp(hi, LOWEST_MIDI, HIGHEST_MIDI);
    const lanes = Math.max(1, this.pitchHi - this.pitchLo + 1);
    this.laneH = this.height / lanes;
  }

  /** Song µs → canvas x (px). */
  xOf(us: number): number {
    return (us - this.originUs) * this.pxPerUs;
  }

  /** µs duration → px width. */
  wOf(durUs: number): number {
    return durUs * this.pxPerUs;
  }

  /** MIDI pitch → top y (px) of that pitch's lane. Higher pitch → smaller y. */
  yOf(pitch: number): number {
    return (this.pitchHi - pitch) * this.laneH;
  }

  /** Whether a pitch is within the visible vertical window. */
  pitchVisible(pitch: number): boolean {
    return pitch >= this.pitchLo && pitch <= this.pitchHi;
  }
}
