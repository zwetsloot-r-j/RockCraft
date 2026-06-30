// utils.ts — 88-key geometry + small drawing/color helpers, ported from the
// design prototype's song.js (RC globals) and highway.js helpers. Mirrors
// crates/core (events.rs) and crates/tui (keyboard.rs) so the screen uses the
// exact same model the Rust engine produces.

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
