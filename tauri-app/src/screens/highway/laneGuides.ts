// laneGuides.ts — which notes get a lane guide: a faint column in the note's
// colour running from its onset down to the keyboard, so where it will land
// reads at a glance. One guide per lane — the next note due in it.

import type { NoteSpan } from "./types";

/**
 * The next note due in each lane: among `notes[lo, hi)` (sorted by start), for
 * every pitch the earliest note starting in `[now, horizon]`, skipping any
 * `skip` rejects. A note already sounding (start < now) has reached the
 * keyboard, so it gets no guide; the note after it in that lane does. Returned
 * in start order.
 */
export function nextPerLane(
  notes: readonly NoteSpan[],
  lo: number,
  hi: number,
  now: number,
  horizon: number,
  skip: (n: NoteSpan) => boolean = () => false,
): NoteSpan[] {
  const taken = new Set<number>();
  const out: NoteSpan[] = [];
  for (let i = lo; i < hi; i++) {
    const n = notes[i];
    if (n.start < now || n.start > horizon) continue;
    if (taken.has(n.note) || skip(n)) continue;
    taken.add(n.note);
    out.push(n);
  }
  return out;
}
