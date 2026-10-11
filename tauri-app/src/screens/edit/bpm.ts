// bpm.ts — tempo text helpers for the edit screen.
//
// `core` stores a tempo as fixed-point milli-BPM (`core::Bpm`, 0.001 BPM
// precision) and sends it as a plain JSON number. These helpers format it for
// display (trailing zeros trimmed, mirroring `Bpm`'s `Display`), and parse /
// sanitise what the user types into the set-BPM prompt.

/** Lowest / highest tempo the prompt accepts (mirrors `Grid::MIN_BPM/MAX_BPM`). */
export const MIN_BPM = 20;
export const MAX_BPM = 300;

/** Max digits after the decimal separator (0.001 BPM precision). */
const MAX_DECIMALS = 3;
/** Max digits before it (the max tempo, 300, has three). */
const MAX_WHOLE_DIGITS = 3;

/**
 * Format a tempo rounded to 0.001 BPM with trailing zeros trimmed:
 * `120`, `120.5`, `97.25`, `120.001`.
 */
export function formatBpm(bpm: number): string {
  if (!Number.isFinite(bpm)) return "0";
  const milli = Math.round(bpm * 1000);
  const sign = milli < 0 ? "-" : "";
  const abs = Math.abs(milli);
  const whole = Math.floor(abs / 1000);
  const frac = abs % 1000;
  if (frac === 0) return `${sign}${whole}`;
  return `${sign}${whole}.${String(frac).padStart(3, "0").replace(/0+$/, "")}`;
}

/**
 * Append one typed key to the set-BPM prompt text, or return the text unchanged
 * if the key isn't accepted. Digits (at most 3 whole and 3 decimal) and a single
 * decimal separator — `.` or `,` (normalised to `.`) — are accepted.
 */
export function typeBpmKey(text: string, key: string): string {
  const sep = text.indexOf(".");
  if (key === "." || key === ",") {
    return sep === -1 ? text + "." : text;
  }
  if (!/^[0-9]$/.test(key)) return text;
  const room = sep === -1 ? text.length < MAX_WHOLE_DIGITS : text.length - sep - 1 < MAX_DECIMALS;
  return room ? text + key : text;
}

/**
 * Parse prompt text (`120`, `120.5`, `97,25`) to a tempo rounded to 0.001 BPM.
 * Returns `null` for anything that isn't a plain non-negative decimal.
 */
export function parseBpm(text: string): number | null {
  const t = text.trim().replace(",", ".");
  if (!/^(\d+\.?\d*|\.\d+)$/.test(t)) return null;
  const v = Number(t);
  if (!Number.isFinite(v)) return null;
  return Math.round(v * 1000) / 1000;
}

/** Whether `bpm` is inside the editable range. */
export function bpmInRange(bpm: number): boolean {
  return bpm >= MIN_BPM && bpm <= MAX_BPM;
}
