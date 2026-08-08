import { describe, expect, it } from "vitest";
import type { BackgroundTransform } from "../../ipc/types";
import {
  IDENTITY_TRANSFORM,
  cssOpacity,
  cssTransform,
  layerStyle,
} from "./backgrounds";

function t(over: Partial<BackgroundTransform> = {}): BackgroundTransform {
  return { ...IDENTITY_TRANSFORM, ...over };
}

describe("cssTransform", () => {
  it("renders the identity as a no-op transform", () => {
    expect(cssTransform(IDENTITY_TRANSFORM)).toBe(
      "translate(0%, 0%) rotate(0deg) scale(1)",
    );
  });

  it("maps surface fractions onto percentage translation", () => {
    // x/y are fractions of the surface and the element fills the surface, so
    // 0.25 is a quarter of the width — no pixel measurement involved.
    expect(cssTransform(t({ x: 0.25, y: -0.5 }))).toBe(
      "translate(25%, -50%) rotate(0deg) scale(1)",
    );
  });

  it("carries rotation and scale through in order", () => {
    expect(cssTransform(t({ rotation_deg: -30, scale: 1.75 }))).toBe(
      "translate(0%, 0%) rotate(-30deg) scale(1.75)",
    );
  });

  it("falls back to the identity for non-finite components", () => {
    const bad = {
      x: NaN,
      y: Infinity,
      scale: NaN,
      rotation_deg: -Infinity,
      opacity: NaN,
    };
    expect(cssTransform(bad)).toBe("translate(0%, 0%) rotate(0deg) scale(1)");
    expect(cssOpacity(bad)).toBe(1);
  });
});

describe("cssOpacity", () => {
  it("passes an in-range value through", () => {
    expect(cssOpacity(t({ opacity: 0.4 }))).toBeCloseTo(0.4);
  });

  it("clamps out-of-range values", () => {
    expect(cssOpacity(t({ opacity: 2 }))).toBe(1);
    expect(cssOpacity(t({ opacity: -1 }))).toBe(0);
  });
});

describe("layerStyle", () => {
  it("fills and contains the surface without stealing pointer events", () => {
    const style = layerStyle(IDENTITY_TRANSFORM, 0);
    expect(style.position).toBe("absolute");
    expect(style.inset).toBe("0");
    expect(style["object-fit"]).toBe("contain");
    expect(style["pointer-events"]).toBe("none");
    expect(style["transform-origin"]).toBe("center center");
  });

  it("stacks layers back-to-front by index", () => {
    expect(layerStyle(IDENTITY_TRANSFORM, 0)["z-index"]).toBe("0");
    expect(layerStyle(IDENTITY_TRANSFORM, 2)["z-index"]).toBe("2");
  });

  it("carries the live transform and opacity", () => {
    const style = layerStyle(t({ x: 0.1, scale: 2, opacity: 0.5 }), 1);
    expect(style.transform).toBe(
      "translate(10%, 0%) rotate(0deg) scale(2)",
    );
    expect(style.opacity).toBe("0.5");
  });
});
