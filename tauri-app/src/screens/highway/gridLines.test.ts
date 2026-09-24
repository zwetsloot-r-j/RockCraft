import { describe, expect, it } from "vitest";
import { mappedGridLines, uniformGridLines } from "./gridLines";

describe("uniformGridLines", () => {
  it("spaces beats evenly and marks bar lines", () => {
    const lines = uniformGridLines(500, 2000, 0, 2000);
    expect(lines.map((l) => l.t)).toEqual([0, 500, 1000, 1500, 2000]);
    expect(lines.filter((l) => l.bar).map((l) => l.t)).toEqual([0, 2000]);
  });
});

describe("mappedGridLines", () => {
  // Bar 0: 2000 ms (500 ms beats), bar 1: 4000 ms (1000 ms beats).
  const bars = [1000, 3000, 7000];

  it("follows each bar's own length", () => {
    const lines = mappedGridLines(bars, 4, 0, 7000);
    expect(lines.map((l) => l.t)).toEqual([
      1000, 1500, 2000, 2500, 3000, 4000, 5000, 6000, 7000,
    ]);
    expect(lines.filter((l) => l.bar).map((l) => l.t)).toEqual([1000, 3000, 7000]);
  });

  it("clips to the window and draws nothing before the map", () => {
    const lines = mappedGridLines(bars, 4, 3500, 5500);
    expect(lines.map((l) => l.t)).toEqual([4000, 5000]);
    expect(mappedGridLines(bars, 4, 0, 900)).toEqual([]);
  });

  it("repeats the last bar's length past the map end", () => {
    const lines = mappedGridLines(bars, 4, 11000, 15000);
    // Last bar is 4000 ms: extrapolated downbeats at 11000 and 15000.
    expect(lines.filter((l) => l.bar).map((l) => l.t)).toEqual([11000, 15000]);
    expect(lines.map((l) => l.t)).toEqual([11000, 12000, 13000, 14000, 15000]);
  });
});
