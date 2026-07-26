// omr.ts — the OMR confidence summary carried on the import log stream (M13-B).
//
// A scan import is a lossy inference step, so the score sidecar reports how much
// of the result it doubts ("imported 412 notes, 37 flagged — review in the
// editor"). That summary rides the existing `import_progress` log events rather
// than a channel of its own, and this module is how the import screen picks it
// out of the surrounding engine chatter to show as its status line.
//
// Kept free of any Tauri import so it stays a pure, headlessly testable function.

/**
 * Marker the sidecar puts on its summary line. Mirrors
 * `rockcraft_import::OMR_SUMMARY_PREFIX` on the Rust side and
 * `score_import.confidence.SUMMARY_PREFIX` in the sidecar — change all three or
 * none.
 */
export const OMR_SUMMARY_PREFIX = "omr: ";

/**
 * The OMR summary a log line carries, or `null` for any other line.
 * Called on every log line; the caller keeps the last match.
 */
export function omrSummary(line: string): string | null {
  const trimmed = line.trim();
  if (!trimmed.startsWith(OMR_SUMMARY_PREFIX)) return null;
  return trimmed.slice(OMR_SUMMARY_PREFIX.length).trim();
}
