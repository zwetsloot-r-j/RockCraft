// viewport.ts — pure step/pitch ↔ pixel mapping for the edit grid.
//
// The edit grid is a *vertical*, highway-aligned piano-roll matching the Record
// and Play screens: time runs **bottom → top** (song start at the bottom, later
// time higher; µs → y) and pitch runs **left → right, low → high** (MIDI → x).
// Advancing in time moves the cursor *up*. All the grid maths is
// `snapshot`-relative and mirrors `core::grid` (`crates/core/src/grid.rs`): step
// / beat / bar sizing is derived from `bpm`, `time_sig` and `subdivision`
// exactly as the Rust `Grid` does, so the rendered gridlines line up with the
// cursor steps the composer produces.
//
// This module is pure geometry — no canvas, no DOM. The canvas renderer
// (EditCanvas) and the screen consume it; keeping it separate keeps the mapping
// reviewable in isolation.
//
// Geometry invariants (the axis swap, in one place):
//   yOf(0) ≈ height            — song start sits at the bottom
//   yOf(later) < yOf(earlier)  — later time is higher up
//   xOf(pitchLo) < xOf(pitchHi)— lowest key left, highest key right

import type { Subdivision, TimeSig } from "../../ipc/types";

/** MIDI range of an 88-key board (A0..C8), mirroring `keyboard.rs`. */
export const LOWEST_MIDI = 21;
export const HIGHEST_MIDI = 108;

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

/**
 * A scroll/zoom viewport over the grid: maps MIDI pitch ↔ x and song time ↔ y
 * within a `width × height` px canvas region. Built each frame from the current
 * snapshot so cursor-follow scrolling tracks the cursor (or playhead, while
 * playing) without any persistent state.
 *
 * The pitch axis is *fit-88*: the whole A0..C8 keyboard always spans the width
 * (lowest left, highest right), so the cursor pitch is never off-screen and no
 * horizontal scrolling is needed. The time axis scrolls vertically.
 */
export class Viewport {
  readonly width: number;
  readonly height: number;
  /** Pixels per microsecond on the time (y) axis — the vertical zoom. */
  readonly pxPerUs: number;
  /** Pixel width of one pitch lane (full 88-key range across the width). */
  readonly laneW: number;
  /** Song µs at the bottom edge (y = height). */
  readonly originUs: number;
  /** Lowest visible MIDI pitch (the leftmost lane). */
  readonly pitchLo: number;
  /** Highest visible MIDI pitch (the rightmost lane). */
  readonly pitchHi: number;

  /** Horizontal pan (px) added to every lane x — backdrop keyboard alignment. */
  readonly xOffset: number;

  constructor(opts: {
    width: number;
    height: number;
    /** µs spanned across the full canvas height (the vertical zoom). */
    spanUs: number;
    /** µs to keep anchored: cursor step time (or playhead while playing). */
    anchorUs: number;
    /**
     * Fraction of the span kept *below* the anchor, i.e. the hit/now line sits
     * `(1 − hitLineFrac)` of the way down the canvas. Default ⅓ (line at ⅔
     * down). Raising it moves the hit line toward the bottom — used to register
     * the grid's now-line on a backdrop movie's drawn keyboard.
     */
    hitLineFrac?: number;
    /** Extra horizontal zoom on the keyboard (1 = full 88 across width). */
    xScale?: number;
    /** Horizontal pan in px (keyboard left-edge alignment). */
    xOffset?: number;
  }) {
    this.width = Math.max(1, opts.width);
    this.height = Math.max(1, opts.height);
    this.pxPerUs = this.height / Math.max(1, opts.spanUs);

    // Anchor the time axis `hitLineFrac` up from the bottom so there's history
    // below and lookahead above the cursor/playhead.
    const hitFrac = opts.hitLineFrac ?? 1 / 3;
    this.originUs = Math.max(0, opts.anchorUs - opts.spanUs * hitFrac);

    // Fit the full 88-key range across the width (lowest key left, highest
    // right, one lane per semitone), then apply the optional horizontal zoom.
    this.pitchLo = LOWEST_MIDI;
    this.pitchHi = HIGHEST_MIDI;
    const lanes = this.pitchHi - this.pitchLo + 1;
    this.laneW = ((opts.xScale ?? 1) * this.width) / lanes;
    this.xOffset = opts.xOffset ?? 0;
  }

  /** MIDI pitch → left x (px) of that pitch's lane. Higher pitch → larger x. */
  xOf(pitch: number): number {
    return this.xOffset + (pitch - this.pitchLo) * this.laneW;
  }

  /** Song µs → canvas y (px). Earlier time → larger y (lower on screen). */
  yOf(us: number): number {
    return this.height - (us - this.originUs) * this.pxPerUs;
  }

  /** µs duration → px height. */
  hOf(durUs: number): number {
    return durUs * this.pxPerUs;
  }

  /** Whether a pitch is within the visible horizontal window. */
  pitchVisible(pitch: number): boolean {
    return pitch >= this.pitchLo && pitch <= this.pitchHi;
  }
}
