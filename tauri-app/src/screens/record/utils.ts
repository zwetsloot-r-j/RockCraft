// utils.ts — record-screen geometry/colour helpers. The note-highway screen
// (#23) is merged, so per specs/M2-tauri-record-screen.md the shared key-layout
// / colour helpers are re-used from screens/highway/utils.ts rather than
// duplicated. Only the record-specific bits (spectrum fill, staff diatonic
// mapping) live here.

import { isBlack, spectrumHue } from "../highway/utils";

export {
  clamp,
  HIGHEST,
  isBlack,
  keyLayout,
  lerp,
  LOWEST,
  noteName,
  pitchClass,
  roundRect,
  shade,
  spectrumHue,
  withAlpha,
} from "../highway/utils";
export type { KeyInfo, KeyLayout } from "../highway/types";

// hue by pitch class (alias kept for parity with the design prototype).
export const hueOf = spectrumHue;
// spectrum fill: an oklch colour for a note, with adjustable lightness/chroma.
export const spec = (n: number, l = 0.72, c = 0.16): string =>
  `oklch(${l} ${c} ${spectrumHue(n)})`;

// letter-name (diatonic) index from C0 — used to lay notes on a staff.
const WHITE_STEP: Record<number, number> = { 0: 0, 2: 1, 4: 2, 5: 3, 7: 4, 9: 5, 11: 6 };
export function diatonic(n: number): number {
  const pc = n % 12;
  const oct = Math.floor(n / 12) - 1;
  // black keys borrow the white key just below (sharp spelling)
  const base = isBlack(n) ? (n - 1) % 12 : pc;
  return oct * 7 + WHITE_STEP[base];
}
