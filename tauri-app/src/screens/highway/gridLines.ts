// Beat/bar line positions for the play highway: uniform, or following a
// per-bar tempo map (M16-A). Pure, so it is unit-tested apart from the canvas.

/** A grid line at song time `t` (ms); `bar` marks a downbeat. */
export type GridLine = { t: number; bar: boolean };

/** Uniform beat/bar lines in `[from, to]` (ms). */
export function uniformGridLines(beat: number, bar: number, from: number, to: number): GridLine[] {
  const out: GridLine[] = [];
  for (let t = Math.floor(from / beat) * beat; t <= to; t += beat) {
    out.push({ t, bar: Math.round(t) % bar === 0 });
  }
  return out;
}

/**
 * Tempo-mapped lines in `[from, to]` (ms): a bar line on every downbeat in
 * `bars` and `beatsPerBar - 1` evenly spaced beat lines inside each bar. Past the
 * map's end the last bar's length repeats (mirror of the editor's extrapolation).
 */
export function mappedGridLines(
  bars: number[],
  beatsPerBar: number,
  from: number,
  to: number,
): GridLine[] {
  const n = bars.length;
  const last = Math.max(1, bars[n - 1] - bars[n - 2]);
  const start = (i: number) => (i < n ? bars[i] : bars[n - 1] + (i - (n - 1)) * last);
  // First bar whose end reaches `from` (bars before the map start are skipped).
  let i = 0;
  while (i < n - 1 && bars[i + 1] < from) i++;
  if (from > bars[n - 1]) i = n - 1 + Math.floor((from - bars[n - 1]) / last);
  const out: GridLine[] = [];
  for (; start(i) <= to; i++) {
    const s = start(i);
    const d = start(i + 1) - s;
    for (let k = 0; k < beatsPerBar; k++) {
      const t = s + (k * d) / beatsPerBar;
      if (t >= from && t <= to) out.push({ t, bar: k === 0 });
    }
  }
  return out;
}
