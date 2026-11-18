/**
 * Finger numbers in the editor (M27-B): the `f` + digit entry keys and where a
 * note's digit is drawn. Pure helpers; EditScreen holds the pending state and
 * EditCanvas does the drawing.
 */
import type { NoteView } from "../../ipc/types";

/** What the key pressed after `f` does. */
export type FingerKey =
  | { kind: "set"; finger: number }
  | { kind: "clear" }
  | { kind: "cancel" };

/**
 * Resolve the key that follows `f`: `1`–`5` set that finger, `0` / `x` /
 * Backspace / Delete clear it, and anything else cancels the entry (the key is
 * swallowed either way, so a stray key never edits).
 */
export function fingerKey(key: string): FingerKey {
  if (key.length === 1 && key >= "1" && key <= "5") {
    return { kind: "set", finger: Number(key) };
  }
  if (key === "0" || key === "x" || key === "Backspace" || key === "Delete") {
    return { kind: "clear" };
  }
  return { kind: "cancel" };
}

/** Digit glyph height as drawn, in px, for a note body `w` px wide. */
export function fingerFontPx(w: number): number {
  return Math.max(8, Math.min(12, Math.round(w * 0.8)));
}

/** Narrowest note body that still gets a readable digit. */
export const FINGER_MIN_W = 7;

/** Where a note's finger digit goes. */
export type FingerPlacement =
  /** Inside the note, sitting at the onset (lower) end. */
  | { kind: "inside"; fontPx: number }
  /** The note is too short to hold the glyph: just beyond its tail (above). */
  | { kind: "outside"; fontPx: number }
  /** The lane is too narrow for a readable digit. */
  | { kind: "none" };

/**
 * Decide where the digit goes for a note body `w` × `h` px. A short note still
 * shows its number — outside, past the tail — so zooming out never hides the
 * fingering; only a lane too narrow to read drops it.
 */
export function fingerPlacement(w: number, h: number): FingerPlacement {
  if (w < FINGER_MIN_W) return { kind: "none" };
  const fontPx = fingerFontPx(w);
  return h >= fontPx + 3 ? { kind: "inside", fontPx } : { kind: "outside", fontPx };
}

/**
 * The finger of the note under the cursor, or null. Mirrors the composer's
 * `note_under_cursor`: a note of the cursor's pitch starting inside the cursor
 * cell, else one sounding across the cursor's time.
 */
export function cursorNoteFinger(
  notes: readonly NoteView[],
  pitch: number,
  cursorUs: number,
  stepUs: number,
): number | null {
  let covering: NoteView | null = null;
  for (const n of notes) {
    if (n.pitch !== pitch) continue;
    if (n.start_us >= cursorUs && n.start_us < cursorUs + stepUs) {
      return n.finger ?? null;
    }
    if (n.start_us <= cursorUs && cursorUs < n.start_us + n.dur_us) covering = n;
  }
  return covering?.finger ?? null;
}
