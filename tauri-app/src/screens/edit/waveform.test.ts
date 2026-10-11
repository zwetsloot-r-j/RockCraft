import { describe, expect, it } from "vitest";
import {
  bucketRange,
  loadWaveMode,
  maxOver,
  nextWaveMode,
  saveWaveMode,
  showsEnvelope,
  showsOnsets,
} from "./waveform";

describe("bucketRange", () => {
  it("maps song time to buckets with no offset", () => {
    expect(bucketRange(0, 9_999, 0, 10_000, 100)).toEqual([0, 0]);
    expect(bucketRange(25_000, 41_000, 0, 10_000, 100)).toEqual([2, 4]);
    // Either order.
    expect(bucketRange(41_000, 25_000, 0, 10_000, 100)).toEqual([2, 4]);
  });

  it("shifts by a positive backing offset (file runs ahead of the song)", () => {
    // Song 0 is file 30 ms → bucket 3.
    expect(bucketRange(0, 0, 30_000, 10_000, 100)).toEqual([3, 3]);
  });

  it("shifts by a negative backing offset and clips before file start", () => {
    // Song 0..15 ms is file −20..−5 ms: entirely before the audio.
    expect(bucketRange(0, 15_000, -20_000, 10_000, 100)).toBeNull();
    // Song 15..35 ms is file −5..15 ms: clipped to buckets 0..1.
    expect(bucketRange(15_000, 35_000, -20_000, 10_000, 100)).toEqual([0, 1]);
  });

  it("clips past the end and returns null beyond the data", () => {
    expect(bucketRange(95_000, 200_000, 0, 10_000, 10)).toEqual([9, 9]);
    expect(bucketRange(100_000, 200_000, 0, 10_000, 10)).toBeNull();
    expect(bucketRange(0, 1, 0, 10_000, 0)).toBeNull();
  });
});

describe("maxOver", () => {
  it("max-pools several buckets so one spike survives a zoomed-out row", () => {
    expect(maxOver([0, 3, 250, 4, 0], 1, 3)).toBe(250);
    expect(maxOver([0, 3, 250, 4, 0], 3, 4)).toBe(4);
    expect(maxOver([7], 0, 0)).toBe(7);
  });
});

describe("wave mode", () => {
  it("cycles both → env → onset → off → both", () => {
    expect(nextWaveMode("both")).toBe("env");
    expect(nextWaveMode("env")).toBe("onset");
    expect(nextWaveMode("onset")).toBe("off");
    expect(nextWaveMode("off")).toBe("both");
  });

  it("says which strips each mode shows", () => {
    expect([showsEnvelope("both"), showsOnsets("both")]).toEqual([true, true]);
    expect([showsEnvelope("env"), showsOnsets("env")]).toEqual([true, false]);
    expect([showsEnvelope("onset"), showsOnsets("onset")]).toEqual([false, true]);
    expect([showsEnvelope("off"), showsOnsets("off")]).toEqual([false, false]);
  });

  it("persists and restores, defaulting to both", () => {
    const m = new Map<string, string>();
    const store = {
      getItem: (k: string) => m.get(k) ?? null,
      setItem: (k: string, v: string) => void m.set(k, v),
    };
    expect(loadWaveMode(() => store)).toBe("both");
    saveWaveMode(() => store, "onset");
    expect(loadWaveMode(() => store)).toBe("onset");
    m.set("rockcraft.waveform", "garbage");
    expect(loadWaveMode(() => store)).toBe("both");
  });

  it("falls back to both when storage throws", () => {
    const boom = () => {
      throw new Error("blocked");
    };
    expect(loadWaveMode(boom)).toBe("both");
    expect(() => saveWaveMode(boom, "off")).not.toThrow();
  });
});
