import { describe, expect, it } from "vitest";
import { noteLabels } from "./noteLabels";

describe("noteLabels", () => {
  it("without a finger behaves exactly as before", () => {
    expect(noteLabels(40, true, undefined)).toEqual({
      name: 9,
      finger: null,
      fingerOutside: false,
    });
    expect(noteLabels(16, true, null).name).toBeNull(); // not tall enough
    expect(noteLabels(40, false, undefined).name).toBeNull(); // names off
  });

  it("a tall note shows the finger at the onset and the name above it", () => {
    expect(noteLabels(60, true, 3)).toEqual({
      name: 23,
      finger: 9,
      fingerOutside: false,
    });
  });

  it("shows the finger even when names are off", () => {
    expect(noteLabels(60, false, 3)).toEqual({
      name: null,
      finger: 9,
      fingerOutside: false,
    });
  });

  it("a medium note shows the finger only", () => {
    expect(noteLabels(24, true, 2)).toEqual({
      name: null,
      finger: 9,
      fingerOutside: false,
    });
    // Centred when the note is shorter than the usual offset allows.
    expect(noteLabels(12, true, 2).finger).toBe(6);
  });

  it("a very short note carries the finger just past its tail", () => {
    expect(noteLabels(5, true, 5)).toEqual({
      name: null,
      finger: 13,
      fingerOutside: true,
    });
  });
});
