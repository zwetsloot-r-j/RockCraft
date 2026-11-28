import { describe, expect, it } from "vitest";
import { HoverGate } from "./hoverGate";

const at = (x: number, y: number) => ({ screenX: x, screenY: y });

describe("HoverGate", () => {
  it("ignores a still mouse when the screen opens", () => {
    const g = new HoverGate();
    expect(g.shouldSelect(at(500, 400))).toBe(false);
    expect(g.shouldSelect(at(500, 400))).toBe(false);
  });

  it("selects once the mouse really moves", () => {
    const g = new HoverGate();
    g.shouldSelect(at(500, 400));
    expect(g.shouldSelect(at(502, 400))).toBe(true);
    expect(g.shouldSelect(at(502, 400))).toBe(true);
  });

  it("ignores rows scrolling under a still mouse after a key move", () => {
    const g = new HoverGate();
    g.shouldSelect(at(500, 400));
    g.shouldSelect(at(501, 400));
    g.keyMoved();
    expect(g.shouldSelect(at(501, 400))).toBe(false);
    expect(g.shouldSelect(at(501, 400))).toBe(false);
    expect(g.shouldSelect(at(501, 410))).toBe(true);
  });
});
