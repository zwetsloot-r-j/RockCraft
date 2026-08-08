// utils.ts — 88-key geometry + small drawing/color helpers, ported from the
// design prototype's song.js (RC globals) and highway.js helpers. Mirrors
// crates/core (events.rs) and crates/tui (keyboard.rs) so the screen uses the
// exact same model the Rust engine produces.

import type { FeedbackLevel, FeedbackTiming } from "../../ipc/types";
import type { KeyInfo, KeyLayout } from "./types";

export const LOWEST = 21; // A0
export const HIGHEST = 108; // C8
const NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

export const isBlack = (n: number): boolean => [1, 3, 6, 8, 10].includes(n % 12);
export const pitchClass = (n: number): number => n % 12;
export const noteName = (n: number): string =>
  NAMES[n % 12] + (Math.floor(n / 12) - 1);

// 12-hue spectrum by pitch class (Spectrum prototype). Even wheel, fixed L/C.
export const spectrumHue = (n: number): number => (pitchClass(n) * 30 + 8) % 360;

export const lerp = (a: number, b: number, t: number): number => a + (b - a) * t;
export const clamp = (v: number, a: number, b: number): number =>
  Math.max(a, Math.min(b, v));

// Column geometry for a keyboard `boardW` px wide. Mirrors keyboard.rs:
// 52 white keys evenly fill the width; black keys straddle the boundary.
export function keyLayout(boardW: number): KeyLayout {
  const whiteW = boardW / 52;
  const whites: KeyInfo[] = [];
  let wi = 0;
  for (let n = LOWEST; n <= HIGHEST; n++) {
    if (!isBlack(n)) {
      const x = wi * whiteW;
      whites.push({ note: n, x, w: whiteW, cx: x + whiteW / 2, black: false, wi });
      wi++;
    }
  }
  const blackW = whiteW * 0.6;
  const blacks: KeyInfo[] = [];
  for (let n = LOWEST; n <= HIGHEST; n++) {
    if (isBlack(n)) {
      // black's n-1 is always white
      const below = whites.find((k) => k.note === n - 1)!;
      const x = below.x + below.w - blackW / 2;
      blacks.push({ note: n, x, w: blackW, cx: x + blackW / 2, black: true });
    }
  }
  const byNote: Record<number, KeyInfo> = {};
  whites.forEach((k) => (byNote[k.note] = k));
  blacks.forEach((k) => (byNote[k.note] = k));
  return { whites, blacks, byNote };
}

type Corner = number | [number, number, number, number];

export function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: Corner,
): void {
  const [tl, tr, br, bl] = typeof r === "number" ? [r, r, r, r] : r;
  ctx.beginPath();
  ctx.moveTo(x + tl, y);
  ctx.lineTo(x + w - tr, y);
  ctx.arcTo(x + w, y, x + w, y + tr, tr);
  ctx.lineTo(x + w, y + h - br);
  ctx.arcTo(x + w, y + h, x + w - br, y + h, br);
  ctx.lineTo(x + bl, y + h);
  ctx.arcTo(x, y + h, x, y + h - bl, bl);
  ctx.lineTo(x, y + tl);
  ctx.arcTo(x, y, x + tl, y, tl);
  ctx.closePath();
}

export function withAlpha(col: string, a: number): string {
  if (col.startsWith("oklch")) return col.replace(")", ` / ${a})`);
  if (col.startsWith("#")) {
    const n = col.slice(1);
    const v =
      n.length === 3
        ? n
            .split("")
            .map((c) => c + c)
            .join("")
        : n;
    const r = parseInt(v.slice(0, 2), 16);
    const g = parseInt(v.slice(2, 4), 16);
    const b = parseInt(v.slice(4, 6), 16);
    return `rgba(${r},${g},${b},${a})`;
  }
  return col;
}

// ── Per-note key treatment ────────────────────────────────────────────────
// Mode-independent visual cues that mark a black-key (accidental) note block as
// distinct from a white-key (natural) one, regardless of which `colorMode` the
// fill came from. Shared with the edit view (EditCanvas imports this), so the
// rule lives in exactly one place and stays unit-testable. Matches the Claude
// Design prototype's accidental look: slimmer pill + darker fill + an outline.
//
//   inset    extra fraction of the lane width trimmed on top of the base
//            noteGap, so accidentals read as a thinner pill. Base gap 0.16 +
//            0.20 ≈ the prototype's ~0.36 gap; naturals add nothing.
//   stroke   outline colour the fill is traced with (naturals get none), so the
//            boundary reads even when fills are similar (e.g. accent mode).
//   shadeMul amount passed to `shade()` to darken the fill the same way in every
//            colour mode (the prototype's `shade(..., -0.18)`); 0 leaves it.
export interface KeyNoteStyle {
  inset: number;
  stroke: string | null;
  shadeMul: number;
}

const NATURAL_STYLE: KeyNoteStyle = { inset: 0, stroke: null, shadeMul: 0 };
const ACCIDENTAL_STYLE: KeyNoteStyle = {
  inset: 0.2,
  stroke: "rgba(255,255,255,0.55)",
  shadeMul: -0.18,
};

export function keyNoteStyle(note: number): KeyNoteStyle {
  return isBlack(note) ? ACCIDENTAL_STYLE : NATURAL_STYLE;
}

// ── Trailing-edge gap ─────────────────────────────────────────────────────
// Two notes of the same pitch played back-to-back share an edge: the first
// one's end is the second one's onset, so their blocks touch and read as a
// single long bar. To keep them apart, every note is drawn slightly short at
// its **trailing** (later-time) edge — the end, i.e. the edge *away* from the
// hit line, since time runs bottom → top in both the highway and the edit grid.
// The onset edge is never moved, so the note still lands exactly on time.
//
// Render-only: `start_us`/`dur_us` are untouched; this trims pixels, not time.
// Shared by the highway and the edit view so both read the same way.
//
//   NOTE_TAIL_GAP_PX   the gap a comfortably long note gets.
//   TAIL_GAP_MAX_FRAC  ceiling as a fraction of the note's own length, so a
//                      short note loses a proportional sliver, not most of it.
//   TAIL_GAP_MIN_BODY  px of body a note always keeps, so the shortest blocks
//                      stay visible rather than being gapped out of existence.
export const NOTE_TAIL_GAP_PX = 3;
const TAIL_GAP_MAX_FRAC = 0.25;
const TAIL_GAP_MIN_BODY = 2;

/** Pixels to trim from the trailing edge of a note `lengthPx` long. */
export function tailGapPx(lengthPx: number): number {
  if (!(lengthPx > 0)) return 0;
  return Math.min(
    NOTE_TAIL_GAP_PX,
    lengthPx * TAIL_GAP_MAX_FRAC,
    Math.max(0, lengthPx - TAIL_GAP_MIN_BODY),
  );
}

// ── Hit / near / miss feedback (M14-B) ────────────────────────────────────
// The backend judges each note against `core::scoring` and sends a
// `Feedback` *level* (see `core::scoring::Feedback`): clear for a well-timed
// hit, near for an off-time one, subtle for a miss. That's the whole judgment
// rule — it lives in core so no frontend re-derives it. What lives *here* is
// only the pixel budget each level gets, kept pure so the "clear reads stronger
// than near reads stronger than subtle" ordering is unit-testable rather than
// buried in canvas calls.
//
//   sparks      particles thrown up from the lane. A miss throws none — a note
//               you didn't play shouldn't celebrate.
//   flashAlpha  peak opacity of the lane flash at the hit line.
//   flashSpread how far that flash grows, as a multiple of the lane width.
//   flashMs     how long it takes to decay away.
//   label       the central readout text, and `labelMs` its lifetime.
export interface FeedbackFx {
  sparks: number;
  flashAlpha: number;
  flashSpread: number;
  flashMs: number;
  label: string;
  labelMs: number;
  color: string;
}

/** Level → effect budget. Same palette as the design prototype's judgments. */
const FEEDBACK_FX: Record<FeedbackLevel, FeedbackFx> = {
  clear: {
    sparks: 7,
    flashAlpha: 0.8,
    flashSpread: 1.6,
    flashMs: 420,
    label: "PERFECT",
    labelMs: 520,
    color: "#5be7c4",
  },
  near: {
    sparks: 3,
    flashAlpha: 0.42,
    flashSpread: 1.0,
    flashMs: 300,
    label: "GOOD",
    labelMs: 420,
    color: "#ffd166",
  },
  subtle: {
    sparks: 0,
    flashAlpha: 0.18,
    flashSpread: 0.5,
    flashMs: 220,
    label: "MISS",
    labelMs: 340,
    color: "#ff5d6c",
  },
};

/** The effect budget for a judged note's feedback level. */
export function feedbackFx(level: FeedbackLevel): FeedbackFx {
  return FEEDBACK_FX[level] ?? FEEDBACK_FX.subtle;
}

/**
 * Readout text for a judgment: the level's label, except that an off-time hit
 * says *which* way it was off — the one thing a player can act on. Pure so the
 * wording is testable alongside the budgets.
 */
export function feedbackLabel(level: FeedbackLevel, timing: FeedbackTiming): string {
  if (level === "near") return timing === "early" ? "EARLY" : "LATE";
  return feedbackFx(level).label;
}

// Shade a hex color lighter (+) or darker (-); passthrough for oklch.
export function shade(col: string, amt: number): string {
  if (!col.startsWith("#")) {
    if (col.startsWith("oklch")) {
      return col.replace(
        /oklch\(([\d.]+)/,
        (_m, l: string) => `oklch(${clamp(parseFloat(l) + amt, 0, 1)}`,
      );
    }
    return col;
  }
  const n = col.slice(1);
  const v =
    n.length === 3
      ? n
          .split("")
          .map((c) => c + c)
          .join("")
      : n;
  const r = parseInt(v.slice(0, 2), 16);
  const g = parseInt(v.slice(2, 4), 16);
  const b = parseInt(v.slice(4, 6), 16);
  const f = (x: number) => clamp(Math.round(x + 255 * amt), 0, 255);
  return `rgb(${f(r)},${f(g)},${f(b)})`;
}
