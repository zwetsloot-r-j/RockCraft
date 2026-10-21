import { describe, expect, it } from "vitest";
import { isFullscreenKey } from "./fullscreen";

const key = (key: string, mods: Partial<KeyboardEvent> = {}) => ({
  key,
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  ...mods,
});

describe("isFullscreenKey", () => {
  it("is F11 and Alt+Enter", () => {
    expect(isFullscreenKey(key("F11"))).toBe(true);
    expect(isFullscreenKey(key("Enter", { altKey: true }))).toBe(true);
  });

  it("leaves plain Enter and other combinations alone", () => {
    expect(isFullscreenKey(key("Enter"))).toBe(false);
    expect(isFullscreenKey(key("Enter", { altKey: true, ctrlKey: true }))).toBe(false);
    expect(isFullscreenKey(key("F11", { shiftKey: true }))).toBe(false);
    expect(isFullscreenKey(key("F11", { altKey: true }))).toBe(false);
    expect(isFullscreenKey(key("F10"))).toBe(false);
  });
});
