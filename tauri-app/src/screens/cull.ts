// cull.ts — viewport culling for the note-highway canvases (edit + play).
//
// A dense chart (e.g. an imported song of thousands of notes) is drawn on a
// canvas that only ever shows a few bars. Scanning the whole note list every
// frame is O(total); windowing it to the visible time span makes each frame
// O(visible + log N). Notes are kept **sorted ascending by start time**, so the
// visible slice is a contiguous index range found by binary search.
//
// Pure geometry/index math — no canvas, no DOM — so it is unit-tested in
// isolation (cull.test.ts) and shared by EditCanvas and HighwayCanvas.

/**
 * First index `i` with `key(items[i]) >= target` (lower bound), or `items.length`
 * if none. `items` must be sorted ascending by `key`.
 */
export function lowerBoundBy<T>(
  items: T[],
  target: number,
  key: (t: T) => number,
): number {
  let lo = 0;
  let hi = items.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (key(items[mid]) < target) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * First index `i` with `key(items[i]) > target` (upper bound), or `items.length`
 * if none. `items` must be sorted ascending by `key`.
 */
export function upperBoundBy<T>(
  items: T[],
  target: number,
  key: (t: T) => number,
): number {
  let lo = 0;
  let hi = items.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (key(items[mid]) <= target) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * The half-open index range `[lo, hi)` of notes whose time span can overlap the
 * visible window `[tMin, tMax]`, for `items` sorted ascending by `startKey`.
 *
 * A note starting before `tMin` can still be visible if it is long enough to
 * extend into the window, so the lower edge is padded by `maxExtent` — the
 * longest note's duration (in the same time unit as the keys). The upper edge is
 * the first note starting after `tMax` (any later note is entirely above the
 * window). Callers still apply their exact per-note clip; this only bounds the
 * scan, never changes what is considered on-screen.
 */
export function visibleRange<T>(
  items: T[],
  tMin: number,
  tMax: number,
  maxExtent: number,
  startKey: (t: T) => number,
): [number, number] {
  const lo = lowerBoundBy(items, tMin - maxExtent, startKey);
  const hi = upperBoundBy(items, tMax, startKey);
  return [lo, Math.max(lo, hi)];
}
