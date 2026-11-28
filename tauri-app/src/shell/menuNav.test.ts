import { describe, expect, it } from "vitest";
import { isTyping, menuNavKey } from "./menuNav";

describe("menuNavKey", () => {
  it("maps C / D / E to Enter / ↑ / ↓ on the main menu", () => {
    expect(menuNavKey("menu", "confirm")).toBe("Enter");
    expect(menuNavKey("menu", "up")).toBe("ArrowUp");
    expect(menuNavKey("menu", "down")).toBe("ArrowDown");
  });

  it("leaves C# inert on the main menu", () => {
    expect(menuNavKey("menu", "cancel")).toBeNull();
  });

  it("maps all four keys on the library and importing screens", () => {
    for (const s of ["library", "importing"] as const) {
      expect(menuNavKey(s, "confirm")).toBe("Enter");
      expect(menuNavKey(s, "cancel")).toBe("Escape");
      expect(menuNavKey(s, "up")).toBe("ArrowUp");
      expect(menuNavKey(s, "down")).toBe("ArrowDown");
    }
  });

  it("never navigates where keys are music or text", () => {
    for (const s of ["play", "edit", "url-input", "video-picker"] as const) {
      for (const n of ["confirm", "cancel", "up", "down"] as const) {
        expect(menuNavKey(s, n)).toBeNull();
      }
    }
  });

  it("ignores notes that are not menu keys", () => {
    expect(menuNavKey("menu", null)).toBeNull();
  });
});

describe("isTyping", () => {
  const el = (tagName: string, isContentEditable = false) =>
    ({ tagName, isContentEditable }) as unknown as Element;

  it("is true for text fields", () => {
    expect(isTyping(el("INPUT"))).toBe(true);
    expect(isTyping(el("TEXTAREA"))).toBe(true);
    expect(isTyping(el("DIV", true))).toBe(true);
  });

  it("is false otherwise", () => {
    expect(isTyping(null)).toBe(false);
    expect(isTyping(el("BODY"))).toBe(false);
  });
});
