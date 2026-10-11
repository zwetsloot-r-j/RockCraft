// waveform.ts — pure helpers for the backing-track waveform overlay (M21-B).
//
// The backend analyses the backing audio into two 0..255 curves per 10 ms
// bucket (M21-A): a loudness envelope and an onset-strength curve. Buckets are
// indexed by *backing-file* position, so song time `t` lives in bucket
// `floor((t + backingOffsetUs) / bucketUs)` and nudging the offset moves the
// strips without a refetch. Everything here is DOM-free and unit-tested; the
// canvas only loops over rows and calls `bucketRange` / `maxOver`.

/** Which curves the edit grid shows. `O` cycles them in this order. */
export type WaveMode = "both" | "env" | "onset" | "off";

const CYCLE: WaveMode[] = ["both", "env", "onset", "off"];

/** The ready curves, as `editBackingWaveform()` returns them. */
export interface WaveformData {
  bucket_us: number;
  envelope: number[];
  onsets: number[];
}

/** The mode after `m` in the `O` cycle: both → env → onset → off → both. */
export function nextWaveMode(m: WaveMode): WaveMode {
  return CYCLE[(CYCLE.indexOf(m) + 1) % CYCLE.length];
}

export const showsEnvelope = (m: WaveMode): boolean => m === "both" || m === "env";
export const showsOnsets = (m: WaveMode): boolean => m === "both" || m === "onset";

const STORE_KEY = "rockcraft.waveform";

/** The persisted mode, or `both` when unset, invalid, or storage throws. */
export function loadWaveMode(storage: () => Pick<Storage, "getItem">): WaveMode {
  try {
    const v = storage().getItem(STORE_KEY);
    return CYCLE.includes(v as WaveMode) ? (v as WaveMode) : "both";
  } catch {
    return "both";
  }
}

/** Persist the mode; a throwing storage (private mode, blocked) is ignored. */
export function saveWaveMode(storage: () => Pick<Storage, "setItem">, m: WaveMode): void {
  try {
    storage().setItem(STORE_KEY, m);
  } catch {
    /* per-viewer convenience only */
  }
}

/**
 * Inclusive bucket index range covering song time `[t0Us, t1Us]` (either
 * order), clipped to `[0, len)`. `null` when the span lies entirely before file
 * position 0 or past the data.
 */
export function bucketRange(
  t0Us: number,
  t1Us: number,
  offsetUs: number,
  bucketUs: number,
  len: number,
): [number, number] | null {
  if (bucketUs <= 0 || len <= 0) return null;
  const a = Math.floor((Math.min(t0Us, t1Us) + offsetUs) / bucketUs);
  const b = Math.floor((Math.max(t0Us, t1Us) + offsetUs) / bucketUs);
  if (b < 0 || a >= len) return null;
  return [Math.max(0, a), Math.min(len - 1, b)];
}

/** Max of `arr[lo..=hi]` — rows max-pool their buckets so spikes never vanish. */
export function maxOver(arr: number[], lo: number, hi: number): number {
  let m = 0;
  for (let i = lo; i <= hi; i++) if (arr[i] > m) m = arr[i];
  return m;
}
