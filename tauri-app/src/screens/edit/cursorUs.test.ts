import { describe, expect, it } from "vitest";
import type { ComposerSnapshot } from "../../ipc/types";
import { cursorUsOf } from "./EditCanvas";

// Only the fields cursorUsOf reads; the rest of the snapshot is irrelevant.
function snap(step: number, bar_starts: number[]): ComposerSnapshot {
  return {
    bpm: 120,
    time_sig: { beats_per_bar: 4, beat_unit: 4 },
    subdivision: "Quarter",
    grid_origin_us: 1_000_000,
    cursor: { pitch: 60, step },
    bar_starts,
  } as unknown as ComposerSnapshot;
}

describe("cursorUsOf", () => {
  it("uses the uniform grid without a tempo map", () => {
    // 120 BPM quarters = 0.5 s, phased from the 1 s origin.
    expect(cursorUsOf(snap(6, []))).toBe(4_000_000);
  });

  it("follows the tempo map's own bar lengths", () => {
    // Bar 0 is 2 s (0.5 s beats), bar 1 is 4 s (1 s beats). Step 6 = bar 1,
    // beat 3: 3 s + 2 × 1 s — a uniform grid would say 4 s, the backdrop drift.
    expect(cursorUsOf(snap(6, [1_000_000, 3_000_000, 7_000_000]))).toBe(5_000_000);
  });
});
