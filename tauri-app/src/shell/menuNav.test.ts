import { describe, expect, it } from "vitest";
import { isTyping, menuNavStep } from "./menuNav";

const key = (...args: Parameters<typeof menuNavStep>) => menuNavStep(...args).key;

describe("menuNavStep", () => {
  it("maps C / D / E to Enter / ↑ / ↓ on the main menu", () => {
    expect(key("menu", "confirm", false)).toBe("Enter");
    expect(key("menu", "up", false)).toBe("ArrowUp");
    expect(key("menu", "down", false)).toBe("ArrowDown");
  });

  it("leaves C# inert on the main menu", () => {
    expect(key("menu", "cancel", false)).toBeNull();
  });

  it("maps all four keys on the library and importing screens", () => {
    for (const s of ["library", "importing"] as const) {
      expect(key(s, "confirm", false)).toBe("Enter");
      expect(key(s, "cancel", false)).toBe("Escape");
      expect(key(s, "up", false)).toBe("ArrowUp");
      expect(key(s, "down", false)).toBe("ArrowDown");
    }
  });

  it("never navigates where keys are music or text", () => {
    for (const s of ["play", "edit", "url-input", "video-picker"] as const) {
      for (const n of ["confirm", "cancel", "up", "down"] as const) {
        expect(key(s, n, false)).toBeNull();
      }
    }
  });

  it("ignores notes that are not menu keys", () => {
    expect(menuNavStep("menu", null, false)).toEqual({ key: null, freePlay: false });
    expect(menuNavStep("menu", null, true)).toEqual({ key: null, freePlay: true });
  });

  it("A0 toggles free play on menu screens", () => {
    expect(menuNavStep("menu", "free_play", false)).toEqual({ key: null, freePlay: true });
    expect(menuNavStep("library", "free_play", true)).toEqual({ key: null, freePlay: false });
  });

  it("free play swallows navigation", () => {
    for (const n of ["confirm", "cancel", "up", "down"] as const) {
      expect(menuNavStep("library", n, true)).toEqual({ key: null, freePlay: true });
    }
  });

  it("A0 is just a note on play and edit", () => {
    for (const s of ["play", "edit"] as const) {
      expect(menuNavStep(s, "free_play", false)).toEqual({ key: null, freePlay: false });
    }
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
