import { describe, expect, it } from "vitest";
import type { NoteView } from "../../ipc/types";
import { countAfterKey } from "./count";
import { cursorNoteFinger, fingerKey, fingerPlacement } from "./finger";

describe("fingerKey", () => {
  it("sets fingers 1 to 5", () => {
    for (let n = 1; n <= 5; n++) {
      expect(fingerKey(String(n))).toEqual({ kind: "set", finger: n });
    }
  });

  it("clears on 0, x, Backspace and Delete", () => {
    for (const key of ["0", "x", "Backspace", "Delete"]) {
      expect(fingerKey(key)).toEqual({ kind: "clear" });
    }
  });

  it("cancels on anything else, including digits past 5", () => {
    for (const key of ["6", "7", "9", "Escape", "j", "a", "Enter", " "]) {
      expect(fingerKey(key)).toEqual({ kind: "cancel" });
    }
  });

  it("covers every key the count prefix would otherwise take", () => {
    // The digit after `f` must never start a count: fingerKey resolves all of
    // 0-9 itself, so EditScreen returns before the count prefix sees them.
    for (let d = 0; d <= 9; d++) {
      const key = String(d);
      expect(["set", "clear", "cancel"]).toContain(fingerKey(key).kind);
      if (d >= 1) expect(countAfterKey(null, key)).not.toBeNull();
    }
  });
});

describe("fingerPlacement", () => {
  it("puts the digit inside a note tall enough to hold it", () => {
    expect(fingerPlacement(16, 40)).toEqual({ kind: "inside", fontPx: 12 });
  });

  it("puts it outside a note too short for the glyph", () => {
    expect(fingerPlacement(16, 6)).toEqual({ kind: "outside", fontPx: 12 });
  });

  it("shrinks the glyph with the lane", () => {
    expect(fingerPlacement(10, 40)).toEqual({ kind: "inside", fontPx: 8 });
  });

  it("drops the digit when the lane is too narrow to read", () => {
    expect(fingerPlacement(6, 40)).toEqual({ kind: "none" });
  });
});

describe("cursorNoteFinger", () => {
  const note = (pitch: number, start_us: number, finger?: number): NoteView => ({
    id: start_us,
    pitch,
    start_us,
    dur_us: 400_000,
    velocity: 80,
    finger,
  });

  it("reads the note starting in the cursor cell", () => {
    const notes = [note(60, 0, 2), note(60, 500_000, 4), note(62, 0, 5)];
    expect(cursorNoteFinger(notes, 60, 500_000, 125_000)).toBe(4);
    expect(cursorNoteFinger(notes, 62, 0, 125_000)).toBe(5);
  });

  it("falls back to a note sounding across the cursor", () => {
    expect(cursorNoteFinger([note(60, 0, 3)], 60, 250_000, 125_000)).toBe(3);
  });

  it("is null with no note, or a note without a finger", () => {
    expect(cursorNoteFinger([], 60, 0, 125_000)).toBeNull();
    expect(cursorNoteFinger([note(60, 0)], 60, 0, 125_000)).toBeNull();
    expect(cursorNoteFinger([note(60, 0, 1)], 61, 0, 125_000)).toBeNull();
  });
});
