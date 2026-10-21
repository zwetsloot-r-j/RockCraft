// liveClock.ts — the highway's view of the backend play clock.
//
// The backend owns song time (`core::PlayClock`) and sends it in `play_state`
// snapshots, ~60 Hz but with jitter. Between snapshots the highway extrapolates
// with the wall clock so the scroll stays smooth at the render rate. Done
// naively (time + wall delta) that extrapolation overshoots and the highway
// jumps back when the next snapshot lands:
//
// - at a wait-mode freeze, which the webview only learns of after the clock
//   has stopped;
// - below 100% practice speed, where song time moves slower than the wall;
// - whenever a snapshot is late.
//
// So this clock scales the wall delta by the practice speed, never runs past
// the next point the backend will stop at (`stop_us`), and never moves back.
// When it is ahead of a fresh snapshot it holds still until the backend
// catches up, instead of snapping back. Only a real jump (a seek, a loop
// wrap, a restart) moves it back. A frozen snapshot is authoritative: the
// awaited note sits exactly on the hit line.
//
// Pure (the wall time is passed in), so the tests drive it with plain numbers.

/** A backward step this large (ms) is a jump (seek / loop / restart), not jitter. */
export const JUMP_BACK_MS = 300;

/** Never extrapolate further than this (ms) past the last snapshot: if the
 * backend stalls, the highway waits for it instead of running away. */
export const MAX_EXTRAPOLATE_MS = 250;

export class LiveClock {
  /** Song time (ms) of the last snapshot. */
  private baseMs = 0;
  /** Wall time (ms) the last snapshot arrived. */
  private anchorMs = 0;
  private frozen = false;
  /** Song ms per wall ms. */
  private rate = 1;
  /** Song time (ms) the clock will stop at or jump from next, if any. */
  private stopMs: number | null = null;
  /** The last time shown, so the scroll never steps back. */
  private shownMs = 0;

  /** Take a backend snapshot that arrived at wall time `wallMs`. */
  update(
    timeMs: number,
    frozen: boolean,
    wallMs: number,
    ratePermille = 1000,
    stopMs: number | null = null,
  ): void {
    const shown = this.now(wallMs);
    if (frozen || timeMs < shown - JUMP_BACK_MS) {
      // Authoritative: frozen on the awaited note, or a real jump.
      this.shownMs = timeMs;
    }
    this.baseMs = timeMs;
    this.anchorMs = wallMs;
    this.frozen = frozen;
    this.rate = ratePermille > 0 ? ratePermille / 1000 : 1;
    this.stopMs = stopMs !== null && stopMs >= timeMs ? stopMs : null;
  }

  /** The song time (ms) to draw at wall time `wallMs`. */
  now(wallMs: number): number {
    let t = this.baseMs;
    if (!this.frozen) {
      const dt = Math.min(Math.max(0, wallMs - this.anchorMs), MAX_EXTRAPOLATE_MS);
      t += dt * this.rate;
      if (this.stopMs !== null) t = Math.min(t, this.stopMs);
    }
    // Monotonic: when ahead of the backend, hold until it catches up.
    if (t > this.shownMs) this.shownMs = t;
    return this.shownMs;
  }
}
