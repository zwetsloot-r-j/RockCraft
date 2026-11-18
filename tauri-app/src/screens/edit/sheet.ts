/**
 * Sheet view (M22-C): the keys the overlay answers and its zoom steps. Pure
 * helpers; SheetView.tsx draws, EditScreen routes the keys.
 */

/** What a key does while the sheet view is open; null = swallowed, no effect. */
export type SheetKey = "close" | "zoom-in" | "zoom-out" | "export" | "print";

export function sheetKey(key: string): SheetKey | null {
  switch (key) {
    case "Escape":
    case "N":
      return "close";
    case "+":
    case "=":
      return "zoom-in";
    case "-":
      return "zoom-out";
    case "e":
      return "export";
    case "p":
      return "print";
    default:
      return null;
  }
}

export const SHEET_ZOOM_MIN = 0.4;
export const SHEET_ZOOM_MAX = 1.6;
export const SHEET_ZOOM_DEFAULT = 0.8;
const SHEET_ZOOM_STEP = 0.1;

/** The zoom one step in (`+1`) or out (`-1`) from `zoom`, kept in range. */
export function nextSheetZoom(zoom: number, direction: 1 | -1): number {
  const stepped = Math.round((zoom + direction * SHEET_ZOOM_STEP) * 10) / 10;
  return Math.min(SHEET_ZOOM_MAX, Math.max(SHEET_ZOOM_MIN, stepped));
}
