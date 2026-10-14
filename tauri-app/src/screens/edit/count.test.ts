import { describe, expect, it } from "vitest";
import { COUNT_MAX, countAfterKey, keepsCount, repeatsFor } from "./count";

describe("countAfterKey", () => {
  it("starts a count on 1-9 but not on a bare 0", () => {
    expect(countAfterKey(null, "9")).toBe(9);
    expect(countAfterKey(null, "1")).toBe(1);
    expect(countAfterKey(null, "0")).toBeNull();
  });

  it("extends a pending count, including with 0", () => {
    expect(countAfterKey(1, "2")).toBe(12);
    expect(countAfterKey(1, "0")).toBe(10);
  });

  it("caps the count", () => {
    expect(countAfterKey(999, "9")).toBe(COUNT_MAX);
  });

  it("ignores non-digits", () => {
    expect(countAfterKey(null, "k")).toBeNull();
    expect(countAfterKey(3, "L")).toBeNull();
    expect(countAfterKey(3, "Shift")).toBeNull();
  });
});

describe("repeatsFor", () => {
  it("repeats cursor moves by the count", () => {
    expect(repeatsFor("cursor_right", 9)).toBe(9);
    expect(repeatsFor("cursor_bar_right", 4)).toBe(4);
  });

  it("runs everything else once", () => {
    expect(repeatsFor("add_note", 9)).toBe(1);
    expect(repeatsFor("cursor_right", null)).toBe(1);
  });
});

describe("keepsCount", () => {
  it("keeps the count across a bare Shift", () => {
    expect(keepsCount("Shift")).toBe(true);
    expect(keepsCount("k")).toBe(false);
  });
});
