import { describe, expect, it } from "vitest";
import { nudgeRatePermille, rateToPermille, speedKey, speedLabel } from "./speed";

describe("nudgeRatePermille", () => {
  it("walks down the steps and clamps at half speed", () => {
    const seen: number[] = [];
    let r = 1000;
    for (let i = 0; i < 6; i++) {
      r = nudgeRatePermille(r, false);
      seen.push(r);
    }
    expect(seen).toEqual([875, 750, 625, 500, 500, 500]);
  });

  it("walks up and never passes 1x", () => {
    expect(nudgeRatePermille(500, true)).toBe(625);
    expect(nudgeRatePermille(875, true)).toBe(1000);
    expect(nudgeRatePermille(1000, true)).toBe(1000);
  });

  it("snaps an off-step value in the asked direction", () => {
    expect(nudgeRatePermille(800, false)).toBe(750);
    expect(nudgeRatePermille(800, true)).toBe(875);
    expect(nudgeRatePermille(250, true)).toBe(500);
    expect(nudgeRatePermille(1500, false)).toBe(1000);
  });
});

describe("speed helpers", () => {
  it("maps the speed keys", () => {
    expect(speedKey("!")).toBe(false);
    expect(speedKey("@")).toBe(true);
    expect(speedKey("1")).toBeNull();
  });

  it("converts and labels rates", () => {
    expect(rateToPermille(0.875)).toBe(875);
    expect(speedLabel(1)).toBe("");
    expect(speedLabel(0.875)).toBe("0.875×");
    expect(speedLabel(0.5)).toBe("0.5×");
    expect(speedLabel(0.625)).toBe("0.625×");
  });
});
