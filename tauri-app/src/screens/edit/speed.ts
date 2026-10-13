// speed.ts — the editor's transport-speed keys (M24-C), as pure helpers.
//
// Mirrors `crates/core/src/speed.rs` (`RATE_STEPS`, `nudge_rate_permille`) so
// `!` / `@` here step exactly like the TUI's — and like Play's `-` / `=`.

/** The speeds `!` / `@` walk through, in permille (1000 = 1×). */
export const RATE_STEPS: readonly number[] = [500, 625, 750, 875, 1000];

/** The piece's own tempo, in permille. */
export const RATE_UNITY = 1000;

/**
 * The next speed (permille) from `current`, one step slower or faster. An
 * off-step value snaps to the next step in that direction (800 → 750 slower,
 * 875 faster); past either end of the steps it stays put (500 slower → 500,
 * 1000 faster → 1000).
 */
export function nudgeRatePermille(current: number, faster: boolean): number {
  const next = faster
    ? RATE_STEPS.find((s) => s > current)
    : [...RATE_STEPS].reverse().find((s) => s < current);
  return next ?? current;
}

/** A playback-rate multiplier (the snapshot's `playback_rate`) as permille. */
export function rateToPermille(rate: number): number {
  return Math.round(rate * 1000);
}

/**
 * Which way a key steps the speed: `false` slower (`!`), `true` faster (`@`),
 * `null` for any other key. Matched on the produced character (`e.key`), so
 * `!` is Shift+1 on any layout and `@` is its own key on a JIS keyboard.
 */
export function speedKey(key: string): boolean | null {
  if (key === "!") return false;
  if (key === "@") return true;
  return null;
}

/** The status-bar badge text — `"0.875×"` — or `""` at 1×. */
export function speedLabel(rate: number): string {
  const permille = rateToPermille(rate);
  if (permille === RATE_UNITY) return "";
  const x = (permille / 1000).toFixed(3).replace(/0+$/, "").replace(/\.$/, "");
  return `${x}×`;
}
