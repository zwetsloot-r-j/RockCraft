import { describe, expect, it } from "vitest";
import { JUMP_BACK_MS, LiveClock, MAX_EXTRAPOLATE_MS } from "./liveClock";

describe("LiveClock", () => {
  it("extrapolates with the wall clock between snapshots", () => {
    const c = new LiveClock();
    c.update(1000, false, 0);
    expect(c.now(10)).toBe(1010);
  });

  it("scales extrapolation by the practice speed", () => {
    const c = new LiveClock();
    c.update(1000, false, 0, 500);
    expect(c.now(16)).toBe(1008);
  });

  it("never runs past the next stop", () => {
    const c = new LiveClock();
    c.update(1000, false, 0, 1000, 1005);
    expect(c.now(16)).toBe(1005);
    // The freeze arrives: exactly on the stop, no snap back.
    c.update(1005, true, 20);
    expect(c.now(40)).toBe(1005);
  });

  it("holds instead of stepping back when ahead of a late snapshot", () => {
    const c = new LiveClock();
    c.update(1000, false, 0);
    expect(c.now(30)).toBe(1030);
    // The backend only reached 1020 (a slow tick): hold at 1030…
    c.update(1020, false, 30);
    expect(c.now(30)).toBe(1030);
    expect(c.now(40)).toBe(1030);
    // …and move on once it has caught up (1020 + 15 ms).
    expect(c.now(45)).toBe(1035);
  });

  it("follows a real jump back (seek, loop, restart)", () => {
    const c = new LiveClock();
    c.update(10_000, false, 0);
    c.now(5);
    c.update(10_000 - JUMP_BACK_MS - 100, false, 5);
    expect(c.now(5)).toBe(10_000 - JUMP_BACK_MS - 100);
  });

  it("a frozen snapshot is authoritative even if slightly behind", () => {
    const c = new LiveClock();
    c.update(1000, false, 0);
    expect(c.now(16)).toBe(1016);
    c.update(1012, true, 16);
    expect(c.now(50)).toBe(1012);
  });

  it("stops extrapolating when snapshots stop coming", () => {
    const c = new LiveClock();
    c.update(0, false, 0);
    expect(c.now(10_000)).toBe(MAX_EXTRAPOLATE_MS);
  });

  it("ignores a stop already behind the snapshot", () => {
    const c = new LiveClock();
    c.update(1000, false, 0, 1000, 900);
    expect(c.now(10)).toBe(1010);
  });
});
