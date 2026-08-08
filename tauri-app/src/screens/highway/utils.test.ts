// utils.test.ts — pure unit tests for the per-note render helpers. No canvas or
// DOM: these are pure functions of the MIDI pitch / note length, so the rules
// they encode can be verified headlessly. Both are shared by the highway and
// the edit view: keyNoteStyle (M11-B) and tailGapPx (M14-A).
import { describe, expect, it } from "vitest";
import { isBlack, keyNoteStyle, NOTE_TAIL_GAP_PX, tailGapPx } from "./utils";

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

describe("tailGapPx", () => {
  it("gives a comfortably long note the full gap", () => {
    expect(tailGapPx(120)).toBe(NOTE_TAIL_GAP_PX);
    expect(tailGapPx(40)).toBe(NOTE_TAIL_GAP_PX);
  });

  it("separates two back-to-back notes of the same pitch", () => {
    // Two 40 px notes stacked edge to edge: the earlier one ends where the
    // later one begins. Trimming the trailing edge leaves visible space.
    const gap = tailGapPx(40);
    expect(gap).toBeGreaterThan(0);
    expect(40 - gap).toBeLessThan(40); // the block really is drawn shorter
  });

  it("never eats more than a quarter of a short note", () => {
    for (const len of [4, 6, 8, 10]) {
      expect(tailGapPx(len)).toBeLessThanOrEqual(len * 0.25);
    }
  });

  it("keeps the shortest blocks visible", () => {
    for (const len of [0.5, 1, 2, 3]) {
      const body = len - tailGapPx(len);
      expect(body).toBeGreaterThan(0);
      expect(body).toBeLessThanOrEqual(len);
    }
    expect(tailGapPx(2)).toBe(0); // already at the minimum body — no trim
  });

  it("is non-negative and monotonic in the note length", () => {
    let prev = -1;
    for (let len = 0; len <= 60; len += 0.5) {
      const gap = tailGapPx(len);
      expect(gap).toBeGreaterThanOrEqual(0);
      expect(gap).toBeGreaterThanOrEqual(prev);
      prev = gap;
    }
  });

  it("treats a zero/negative length as no gap", () => {
    expect(tailGapPx(0)).toBe(0);
    expect(tailGapPx(-5)).toBe(0);
    expect(tailGapPx(NaN)).toBe(0);
  });
});
