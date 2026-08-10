import { describe, expect, it } from "vitest";
import {
  DEFAULT_SPLIT,
  HAND_COLORS,
  effectiveHand,
  handTint,
  isOverridden,
} from "./hand";

describe("effectiveHand", () => {
  it("follows the split line when the note carries no override", () => {
    expect(effectiveHand(59, null, DEFAULT_SPLIT)).toBe("left");
    // At the split belongs to the RIGHT hand — mirrors core::hand::hand_of.
    expect(effectiveHand(60, null, DEFAULT_SPLIT)).toBe("right");
    expect(effectiveHand(61, undefined, DEFAULT_SPLIT)).toBe("right");
  });

  it("moves with a custom split", () => {
    expect(effectiveHand(60, null, 53)).toBe("right");
    expect(effectiveHand(60, null, 72)).toBe("left");
    expect(effectiveHand(53, null, 53)).toBe("right");
  });

  it("prefers the override over the split", () => {
    // A low crossover the author pinned to the right hand.
    expect(effectiveHand(48, "right", DEFAULT_SPLIT)).toBe("right");
    // ...and a high note pinned to the left.
    expect(effectiveHand(84, "left", DEFAULT_SPLIT)).toBe("left");
    // The override wins wherever the split sits.
    expect(effectiveHand(48, "right", 21)).toBe("right");
    expect(effectiveHand(48, "right", 127)).toBe("right");
  });
});

describe("isOverridden", () => {
  it("is true only for an explicit hand", () => {
    expect(isOverridden(null)).toBe(false);
    expect(isOverridden(undefined)).toBe(false);
    expect(isOverridden("left")).toBe(true);
    expect(isOverridden("right")).toBe(true);
  });
});

describe("handTint", () => {
  it("picks the colour from the effective hand", () => {
    expect(handTint(48, null, DEFAULT_SPLIT)).toEqual({
      color: HAND_COLORS.left,
      overridden: false,
    });
    expect(handTint(72, null, DEFAULT_SPLIT)).toEqual({
      color: HAND_COLORS.right,
      overridden: false,
    });
  });

  it("flags an overridden note so the canvas can mark it", () => {
    // Same pitch, same colour as a split-derived left-hand note — the flag is
    // what distinguishes "left by the split" from "left because it was set".
    expect(handTint(72, "left", DEFAULT_SPLIT)).toEqual({
      color: HAND_COLORS.left,
      overridden: true,
    });
  });

  it("gives the two hands distinct colours", () => {
    expect(HAND_COLORS.left).not.toBe(HAND_COLORS.right);
  });
});
