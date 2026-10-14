import { describe, expect, it } from "vitest";
import { baseName, existsSlug, freeSlug } from "./songFiles";

describe("existsSlug", () => {
  it("extracts the slug from an exists error", () => {
    expect(existsSlug("exists:my-tune")).toBe("my-tune");
    expect(existsSlug(new Error("exists:a-b"))).toBe("a-b");
  });

  it("is null for other errors", () => {
    expect(existsSlug("missing file: song.mid")).toBeNull();
    expect(existsSlug("exists:")).toBeNull();
  });
});

describe("freeSlug", () => {
  it("picks the first free -N suffix", () => {
    expect(freeSlug("tune", ["tune"])).toBe("tune-2");
    expect(freeSlug("tune", ["tune", "tune-2", "tune-3"])).toBe("tune-4");
  });
});

describe("baseName", () => {
  it("handles both separators", () => {
    expect(baseName("C:\\songs\\tune.rcsong")).toBe("tune.rcsong");
    expect(baseName("/home/me/tune.rcsong")).toBe("tune.rcsong");
  });
});
