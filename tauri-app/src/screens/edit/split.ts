// split.ts — pure segment-derivation helpers for the M10-C split editor.
//
// The frontend never slices a timeline or writes a bundle (that is M10-A/B,
// reached through the `split_bundle` host command). All it does is *derive* the
// consecutive segments a set of split markers implies, then gather the kept ones
// into `SegmentSpec`s for the backend. This module is the pure half of that:
// no DOM, no IPC — math only, kept obviously correct without a JS test framework
// (the M7 front-end convention, see docs/TAURI-CONTROLS.md).
//
// `segmentsFromSplits` is a line-for-line port of
// `core::segment::segments_from_splits` (crates/core/src/segment.rs) so the
// boundaries the editor shows match exactly what the backend slices.

import type { NoteView, SegmentSpec } from "../../ipc/types";

/** A consecutive slice in song-time microseconds; half-open `[start_us, end_us)`. */
export interface Segment {
  start_us: number;
  end_us: number;
}

/**
 * Turn split points into consecutive segments covering `[0, total_us)`. Points
 * are clamped to `[0, total_us]`, deduplicated and sorted; empty/zero-width gaps
 * are dropped. No splits => one segment `[0, total_us)`. `total_us === 0` => `[]`.
 *
 * A faithful mirror of `core::segment::segments_from_splits`.
 */
export function segmentsFromSplits(splits: number[], totalUs: number): Segment[] {
  if (totalUs <= 0) return [];

  // Boundaries are 0, the clamped split points, and total_us — sorted and
  // deduped, so consecutive boundaries form non-overlapping segments.
  const bounds = [0, totalUs];
  for (const s of splits) {
    bounds.push(Math.min(Math.max(0, s), totalUs));
  }
  bounds.sort((a, b) => a - b);

  const segs: Segment[] = [];
  for (let i = 1; i < bounds.length; i++) {
    const start = bounds[i - 1];
    const end = bounds[i];
    if (end > start) segs.push({ start_us: start, end_us: end }); // drop zero-width (dedupe)
  }
  return segs;
}

/**
 * Total piece length in song-time microseconds: the latest note end across the
 * snapshot's notes (max of `start_us + dur_us`), or 0 when there are no notes.
 * This is the `total_us` the segments cover.
 */
export function pieceLengthUs(notes: NoteView[]): number {
  let end = 0;
  for (const n of notes) {
    const e = n.start_us + n.dur_us;
    if (e > end) end = e;
  }
  return end;
}

/** The default name for the segment at index `i` (0-based): `part-1`, `part-2`, … */
export function defaultSegmentName(i: number): string {
  return `part-${i + 1}`;
}

/** Per-segment keep/discard flag plus its editable name, parallel to the segments. */
export interface SegmentMeta {
  keep: boolean;
  name: string;
}

/**
 * Reconcile a per-segment meta array to a new segment count, preserving existing
 * entries by index and filling new slots with `{ keep: true, name: part-N }`.
 * Used after markers change so keep/name choices survive an added/removed marker.
 */
export function reconcileMeta(prev: SegmentMeta[], count: number): SegmentMeta[] {
  const next: SegmentMeta[] = [];
  for (let i = 0; i < count; i++) {
    next.push(prev[i] ?? { keep: true, name: defaultSegmentName(i) });
  }
  return next;
}

/**
 * Gather the kept segments into the `SegmentSpec`s handed to `split_bundle`,
 * pairing each derived {@link Segment} with its {@link SegmentMeta} (same index)
 * and dropping the discarded ones. Names are trimmed.
 */
export function keptSegmentSpecs(
  segments: Segment[],
  meta: SegmentMeta[],
): SegmentSpec[] {
  const specs: SegmentSpec[] = [];
  segments.forEach((seg, i) => {
    const m = meta[i];
    if (!m || !m.keep) return;
    specs.push({
      start_us: seg.start_us,
      end_us: seg.end_us,
      name: m.name.trim(),
    });
  });
  return specs;
}
