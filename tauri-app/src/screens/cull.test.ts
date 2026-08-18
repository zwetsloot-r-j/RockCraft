import { describe, expect, it } from "vitest";
import { lowerBoundBy, upperBoundBy, visibleRange } from "./cull";

interface N {
  start: number;
  dur: number;
}
const K = (n: N): number => n.start;
// Sorted-by-start notes; the note at start=10 is long (dur 100) so it reaches
// well past its own start.
const notes: N[] = [
  { start: 0, dur: 5 },
  { start: 10, dur: 100 },
  { start: 20, dur: 5 },
  { start: 30, dur: 5 },
  { start: 30, dur: 5 }, // duplicate start
  { start: 50, dur: 5 },
];

describe("lowerBoundBy", () => {
  it("finds the first index >= target", () => {
    expect(lowerBoundBy(notes, 0, K)).toBe(0);
    expect(lowerBoundBy(notes, 15, K)).toBe(2);
    expect(lowerBoundBy(notes, 30, K)).toBe(3); // first of the duplicates
    expect(lowerBoundBy(notes, 51, K)).toBe(6); // past the end
  });
  it("handles empty", () => {
    expect(lowerBoundBy([], 5, K)).toBe(0);
  });
});

describe("upperBoundBy", () => {
  it("finds the first index > target", () => {
    expect(upperBoundBy(notes, 30, K)).toBe(5); // past both duplicates
    expect(upperBoundBy(notes, 0, K)).toBe(1);
    expect(upperBoundBy(notes, 100, K)).toBe(6);
  });
});

describe("visibleRange", () => {
  const maxExtent = 100; // the long note's duration
  it("bounds the scan to the visible window", () => {
    // Window [25, 35]: notes at 30,30 are inside; 20 (+dur 5 ends at 25) touches
    // the edge; the long note at 10 (ends at 110) overlaps too. maxExtent pads
    // the lower edge to include it.
    const [lo, hi] = visibleRange(notes, 25, 35, maxExtent, K);
    // lower edge padded to 25-100 = -75 → index 0; upper first start > 35 → 5.
    expect(lo).toBe(0);
    expect(hi).toBe(5);
  });
  it("returns an empty range past the end", () => {
    const [lo, hi] = visibleRange(notes, 1000, 2000, maxExtent, K);
    expect(lo).toBe(hi); // nothing visible
  });
  it("never returns hi < lo", () => {
    const [lo, hi] = visibleRange(notes, 5, 5, 0, K);
    expect(hi).toBeGreaterThanOrEqual(lo);
  });
});
