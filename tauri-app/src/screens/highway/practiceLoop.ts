// practiceLoop.ts — pure helpers for the play screen's practice loop (M17-A):
// which keys map to which loop command, the header badge text, and which notes
// the highway dims. No DOM, no IPC — the screen does the invoking.

import type { LoopPhaseName, LoopView } from "../../ipc/types";

/** A practice-loop request a key maps to; the screen turns it into an IPC call. */
export type LoopCommand =
  | { kind: "seek"; delta: number }
  | { kind: "mark"; edge: "start" | "end" }
  | { kind: "set"; firstBar: number; lastBar: number }
  | { kind: "clear" };

/**
 * Map a key to its loop command, or `null` when the key is not a loop key (or
 * does nothing right now).
 *
 * - `←` / `→`: pause and jump a bar — outside a running loop only.
 * - `[` / `]`: mark the loop start / end at the bar under the playhead.
 * - `l`: stop a running loop; otherwise start one over the marks, or over the
 *   bar under the playhead when nothing is marked (the backend reports a lone
 *   mark as a one-bar range).
 */
export function loopKeyCommand(
  key: string,
  bar: number,
  loop: LoopView | null,
): LoopCommand | null {
  const running = loop?.running ?? false;
  switch (key) {
    case "ArrowLeft":
      return running ? null : { kind: "seek", delta: -1 };
    case "ArrowRight":
      return running ? null : { kind: "seek", delta: 1 };
    case "[":
      return { kind: "mark", edge: "start" };
    case "]":
      return { kind: "mark", edge: "end" };
    case "l":
    case "L":
      if (running) return { kind: "clear" };
      if (loop) return { kind: "set", firstBar: loop.first_bar, lastBar: loop.last_bar };
      return { kind: "set", firstBar: bar, lastBar: bar };
    default:
      return null;
  }
}

const PHASE_LABEL: Record<LoopPhaseName, string> = {
  count_in: "COUNT-IN",
  demo: "DEMO",
  your_turn: "YOUR TURN",
};

/** "Loop 5–8" (1-based bars; one bar reads "Loop 5"). */
export function loopRangeText(loop: LoopView): string {
  const a = loop.first_bar + 1;
  const b = loop.last_bar + 1;
  return a === b ? `Loop ${a}` : `Loop ${a}–${b}`;
}

/** Accuracy in basis points → a whole percentage ("83%"). */
export function accuracyText(bp: number): string {
  return `${Math.round(bp / 100)}%`;
}

/**
 * The header loop badge: "Loop 5–8 · YOUR TURN · pass 3 · last 83%", or just
 * the range while only marked. `null` when nothing is marked.
 */
export function loopBadgeText(loop: LoopView | null): string | null {
  if (!loop) return null;
  const parts = [loopRangeText(loop)];
  if (loop.running && loop.phase) {
    parts.push(PHASE_LABEL[loop.phase], `pass ${loop.pass}`);
  }
  if (loop.last_pass) parts.push(`last ${accuracyText(loop.last_pass.accuracy_bp)}`);
  return parts.join(" · ");
}

/** The loop band the highway draws, in engine milliseconds. */
export interface LoopBand {
  startMs: number;
  endMs: number;
  running: boolean;
}

/** Project the backend loop view onto the highway's ms clock. */
export function loopBand(loop: LoopView | null): LoopBand | null {
  if (!loop) return null;
  return { startMs: loop.start_us / 1000, endMs: loop.end_us / 1000, running: loop.running };
}

/** Whether a note starting at `startMs` is dimmed: outside a running loop. */
export function dimmedByLoop(startMs: number, band: LoopBand | null): boolean {
  if (!band || !band.running) return false;
  return startMs < band.startMs || startMs >= band.endMs;
}
