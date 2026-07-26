// omr.test.ts — the summary marker must behave exactly like its Rust twin
// (`rockcraft_import::pipeline::omr_summary`), or the two frontends disagree
// about whether an import needs reviewing.
import { describe, expect, it } from "vitest";
import { omrSummary, OMR_SUMMARY_PREFIX } from "./omr";

describe("omrSummary", () => {
  it("lifts the summary out of a marked line", () => {
    expect(omrSummary("omr: imported 412 notes, 37 flagged — review in the editor")).toBe(
      "imported 412 notes, 37 flagged — review in the editor",
    );
  });

  it("tolerates padding around the line", () => {
    expect(omrSummary("  omr: imported 8 notes, 0 flagged  ")).toBe(
      "imported 8 notes, 0 flagged",
    );
  });

  it("ignores every other line the sidecar emits", () => {
    for (const other of [
      "using OMR engine oemer: /usr/bin/oemer",
      "suspect measures (2): 12, 40",
      "warning: no tempo mark in the score",
      "converted 412 notes -> - (bpm=90)",
      "",
    ]) {
      expect(omrSummary(other)).toBeNull();
    }
  });

  it("matches the marker as a prefix, not a substring", () => {
    expect(omrSummary("engine said omr: something")).toBeNull();
    expect(OMR_SUMMARY_PREFIX).toBe("omr: ");
  });
});
