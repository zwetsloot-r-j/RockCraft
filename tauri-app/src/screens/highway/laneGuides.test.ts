import { describe, expect, it } from "vitest";
import { nextPerLane } from "./laneGuides";
import type { NoteSpan } from "./types";

const n = (note: number, start: number, hand: "L" | "R" = "R"): NoteSpan => ({
  note,
  start,
  end: start + 200,
  hand,
});

describe("nextPerLane", () => {
  it("keeps only the closest note in each lane", () => {
    const notes = [n(60, 100), n(64, 150), n(60, 400), n(64, 500), n(67, 900)];
    const got = nextPerLane(notes, 0, notes.length, 0, 3000);
    expect(got).toEqual([n(60, 100), n(64, 150), n(67, 900)]);
  });

  it("hands a lane to the next note once the current one reaches the keys", () => {
    const notes = [n(60, 100), n(60, 400)];
    expect(nextPerLane(notes, 0, 2, 250, 3000)).toEqual([n(60, 400)]);
  });

  it("guides a note sitting exactly on the hit line (a wait-mode freeze)", () => {
    const notes = [n(60, 1000), n(60, 1500)];
    expect(nextPerLane(notes, 0, 2, 1000, 4000)).toEqual([n(60, 1000)]);
  });

  it("ignores notes beyond the horizon and outside [lo, hi)", () => {
    const notes = [n(60, 100), n(62, 200), n(64, 5000)];
    expect(nextPerLane(notes, 1, 3, 0, 3000)).toEqual([n(62, 200)]);
  });

  it("lets a skipped note pass its lane on to the next one", () => {
    const notes = [n(60, 100, "L"), n(60, 300, "R"), n(62, 200, "L")];
    const got = nextPerLane(notes, 0, 3, 0, 3000, (x) => x.hand === "L");
    expect(got).toEqual([n(60, 300, "R")]);
  });
});
