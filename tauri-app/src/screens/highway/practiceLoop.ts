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
 * - `PageUp` / `PageDown`, or `shift` + `←` / `→`: the same, four bars (M24-A).
 * - `[` / `]`: mark the loop start / end at the bar under the playhead.
 * - `l`: stop a running loop; otherwise start one over the marks, or over the
 *   bar under the playhead when nothing is marked (the backend reports a lone
 *   mark as a one-bar range).
 */
/** Bars per big skip (PageUp/PageDown, Shift+←/→). */
export const BIG_SKIP_BARS = 4;

/**
 * The seek bar (M24-A): a click at `fraction` (0–1) of the bar → the **song**
 * time (µs) to hand to `play_seek_us`, which snaps it to that bar's start. The
 * bar spans the whole play clock (`duration_us`, count-in included); the
 * count-in maps to song time 0.
 */
export function seekBarSongUs(fraction: number, durationUs: number, shiftUs: number): number {
  const f = Math.min(1, Math.max(0, fraction));
  return Math.max(0, Math.round(f * durationUs - shiftUs));
}

export function loopKeyCommand(
  key: string,
  bar: number,
  loop: LoopView | null,
  shift = false,
): LoopCommand | null {
  const running = loop?.running ?? false;
  const step = shift ? BIG_SKIP_BARS : 1;
  switch (key) {
    case "ArrowLeft":
      return running ? null : { kind: "seek", delta: -step };
    case "ArrowRight":
      return running ? null : { kind: "seek", delta: step };
    case "PageUp":
      return running ? null : { kind: "seek", delta: -BIG_SKIP_BARS };
    case "PageDown":
      return running ? null : { kind: "seek", delta: BIG_SKIP_BARS };
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

/** "Loop 5–8" (1-based bars; one bar reads "Loop 5"). A lone mark shows
 * which edge is still open: "Loop 5–?" / "Loop ?–8". */
export function loopRangeText(loop: LoopView): string {
  if (!loop.running) {
    if (loop.start_mark !== null && loop.end_mark === null) return `Loop ${loop.start_mark + 1}–?`;
    if (loop.start_mark === null && loop.end_mark !== null) return `Loop ?–${loop.end_mark + 1}`;
  }
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
  /** The marked edges' labels — "[ 5" on the start, "8 ]" on the end — or
   * `null` for an edge that is not marked (a lone mark has one edge). */
  startLabel: string | null;
  endLabel: string | null;
}

/** Project the backend loop view onto the highway's ms clock. */
export function loopBand(loop: LoopView | null): LoopBand | null {
  if (!loop) return null;
  const both = loop.running || (loop.start_mark !== null && loop.end_mark !== null);
  return {
    startMs: loop.start_us / 1000,
    endMs: loop.end_us / 1000,
    running: loop.running,
    startLabel: both || loop.start_mark !== null ? `[ ${loop.first_bar + 1}` : null,
    endLabel: both || loop.end_mark !== null ? `${loop.last_bar + 1} ]` : null,
  };
}

/** The marked range on the seek bar: its left edge and width as fractions
 * (0–1) of the play clock, and which of its edges are marked. `null` with
 * nothing marked or an empty clock. */
export interface SeekBarLoop {
  left: number;
  width: number;
  running: boolean;
  startMarked: boolean;
  endMarked: boolean;
}

export function seekBarLoop(loop: LoopView | null, durationUs: number): SeekBarLoop | null {
  const band = loopBand(loop);
  if (!band || durationUs <= 0) return null;
  const clamp = (f: number) => Math.min(1, Math.max(0, f));
  const left = clamp((band.startMs * 1000) / durationUs);
  const right = clamp((band.endMs * 1000) / durationUs);
  return {
    left,
    width: Math.max(0, right - left),
    running: band.running,
    startMarked: band.startLabel !== null,
    endMarked: band.endLabel !== null,
  };
}

/** The header's demo chip: shown while a loop is marked, or whenever the demo
 * is off (so a switched-off demo never surprises). */
export function demoChipText(demo: boolean, loop: LoopView | null): string | null {
  if (!demo) return "no demo";
  return loop ? "demo" : null;
}

/** Whether a note starting at `startMs` is dimmed: outside a running loop. */
export function dimmedByLoop(startMs: number, band: LoopBand | null): boolean {
  if (!band || !band.running) return false;
  return startMs < band.startMs || startMs >= band.endMs;
}
