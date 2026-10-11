import { describe, expect, it } from "vitest";
import { bpmInRange, formatBpm, parseBpm, typeBpmKey } from "./bpm";
import { quarterUs } from "./viewport";

describe("formatBpm", () => {
  it("trims trailing zeros like core's Bpm Display", () => {
    expect(formatBpm(120)).toBe("120");
    expect(formatBpm(120.5)).toBe("120.5");
    expect(formatBpm(97.25)).toBe("97.25");
    expect(formatBpm(120.001)).toBe("120.001");
    expect(formatBpm(119.99997)).toBe("120");
    expect(formatBpm(-0.1)).toBe("-0.1");
  });
});

describe("parseBpm", () => {
  it("accepts dot and comma decimals, rounding to 0.001", () => {
    expect(parseBpm("120")).toBe(120);
    expect(parseBpm(" 120.5 ")).toBe(120.5);
    expect(parseBpm("97,25")).toBe(97.25);
    expect(parseBpm("120.")).toBe(120);
    expect(parseBpm("97.2504")).toBe(97.25);
  });

  it("rejects non-numbers", () => {
    expect(parseBpm("")).toBeNull();
    expect(parseBpm(".")).toBeNull();
    expect(parseBpm("-3")).toBeNull();
    expect(parseBpm("1e3")).toBeNull();
    expect(parseBpm("1.2.3")).toBeNull();
    expect(parseBpm("abc")).toBeNull();
  });
});

describe("typeBpmKey", () => {
  it("builds a decimal with one separator and at most 3+3 digits", () => {
    let t = "";
    for (const k of ["1", "2", "0", "5", ",", "2", "5", "0", "1", ".", "x"]) {
      t = typeBpmKey(t, k);
    }
    expect(t).toBe("120.250");
  });
});

describe("bpmInRange", () => {
  it("is 20..=300 inclusive", () => {
    expect(bpmInRange(20)).toBe(true);
    expect(bpmInRange(300)).toBe(true);
    expect(bpmInRange(19.999)).toBe(false);
    expect(bpmInRange(300.001)).toBe(false);
  });
});

describe("quarterUs", () => {
  it("mirrors Grid::quarter_us (floored, in milli-BPM)", () => {
    expect(quarterUs(120)).toBe(500_000);
    expect(quarterUs(70)).toBe(857_142);
    expect(quarterUs(120.5)).toBe(497_925);
    // 60 s / 153.6 is exactly 390_625 µs — float division must not land below.
    expect(quarterUs(153.6)).toBe(390_625);
  });
});
