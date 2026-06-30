// utils.test.ts — pure unit tests for the per-note key treatment. No canvas or
// DOM: keyNoteStyle is a pure function of the MIDI pitch, so the black/white
// distinction rule can be verified headlessly (M11-B, shared with the edit view).
import { describe, expect, it } from "vitest";
import { isBlack, keyNoteStyle } from "./utils";

describe("keyNoteStyle", () => {
  it("gives a black-key pitch a distinct treatment from a white-key pitch", () => {
    const white = keyNoteStyle(60); // C4 — natural
    const black = keyNoteStyle(61); // C#4 — accidental

    // Naturals stay plain; accidentals get all three redundant cues.
    expect(white).toEqual({ inset: 0, stroke: null, shadeMul: 0 });
    expect(black.inset).toBeGreaterThan(white.inset); // slimmer pill
    expect(black.stroke).not.toBeNull(); // outline naturals don't get
    expect(black.shadeMul).toBeLessThan(0); // darker fill
  });

  it("agrees with isBlack across a full octave", () => {
    for (let n = 60; n < 72; n++) {
      const style = keyNoteStyle(n);
      if (isBlack(n)) {
        expect(style.inset).toBeGreaterThan(0);
        expect(style.stroke).not.toBeNull();
        expect(style.shadeMul).toBeLessThan(0);
      } else {
        expect(style).toEqual({ inset: 0, stroke: null, shadeMul: 0 });
      }
    }
  });

  it("is octave-invariant (treatment tracks pitch class, not register)", () => {
    expect(keyNoteStyle(61)).toEqual(keyNoteStyle(73)); // C#4 == C#5
    expect(keyNoteStyle(60)).toEqual(keyNoteStyle(72)); // C4 == C5
  });
});
