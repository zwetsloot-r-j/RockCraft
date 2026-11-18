import { describe, expect, it } from "vitest";
import {
  SHEET_ZOOM_DEFAULT,
  SHEET_ZOOM_MAX,
  SHEET_ZOOM_MIN,
  nextSheetZoom,
  sheetKey,
} from "./sheet";

describe("sheetKey", () => {
  it("closes on Esc and on the key that opened it", () => {
    expect(sheetKey("Escape")).toBe("close");
    expect(sheetKey("N")).toBe("close");
  });

  it("zooms, exports and prints", () => {
    expect(sheetKey("+")).toBe("zoom-in");
    expect(sheetKey("=")).toBe("zoom-in");
    expect(sheetKey("-")).toBe("zoom-out");
    expect(sheetKey("e")).toBe("export");
    expect(sheetKey("p")).toBe("print");
  });

  it("gives every editor key no effect", () => {
    for (const key of ["a", "x", "d", "s", "S", " ", "Enter", "h", "j", "1", "f", "n"]) {
      expect(sheetKey(key)).toBeNull();
    }
  });
});

describe("nextSheetZoom", () => {
  it("steps by a tenth without float drift", () => {
    expect(nextSheetZoom(SHEET_ZOOM_DEFAULT, 1)).toBe(0.9);
    expect(nextSheetZoom(SHEET_ZOOM_DEFAULT, -1)).toBe(0.7);
    let z = SHEET_ZOOM_DEFAULT;
    for (let i = 0; i < 3; i++) z = nextSheetZoom(z, 1);
    expect(z).toBe(1.1);
  });

  it("stays in range", () => {
    expect(nextSheetZoom(SHEET_ZOOM_MAX, 1)).toBe(SHEET_ZOOM_MAX);
    expect(nextSheetZoom(SHEET_ZOOM_MIN, -1)).toBe(SHEET_ZOOM_MIN);
  });
});
