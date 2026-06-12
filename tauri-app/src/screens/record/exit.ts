// exit.ts — pure decision logic for the record screen's dirty-exit prompt
// (#188). Kept side-effect-free so it is obviously correct without a JS test
// framework (per the M7 convention); RecordScreen performs the side effects.
//
// Mirrors the TUI record flow: pressing Esc on a take with unsaved input opens
// a Save / Discard / Cancel prompt instead of dropping the take. On a clean
// (saved) take, Esc is left to the shell's global Esc→menu handler.

/** Whether the Save/Discard/Cancel prompt is currently shown. */
export type ExitPhase = "open" | "closed";

/** Side effect the screen should perform after a key is handled. */
export type ExitEffect = "none" | "save" | "discard";

export interface ExitDecision {
  /** The prompt phase after handling the key. */
  phase: ExitPhase;
  /** Side effect for the screen to run. */
  effect: ExitEffect;
  /**
   * When true the key must be swallowed (preventDefault +
   * stopImmediatePropagation) so the shell's global Esc→menu handler does not
   * also fire. When false the event is left to propagate — used for Esc on a
   * clean take, which the shell turns into a navigation to the menu.
   */
  intercept: boolean;
}

/**
 * Decide the next exit state.
 *
 * Closed phase:
 *  - `Escape` on a dirty take → open the prompt (intercept the shell nav).
 *  - `Escape` on a clean take → let the shell navigate to the menu.
 *  - any other key → no exit handling.
 *
 * Open phase (modal; all keys are swallowed so nothing leaks to the canvas):
 *  - `s` → save, `d` → discard, `Escape` → cancel (close), else stay open.
 */
export function nextExit(phase: ExitPhase, dirty: boolean, key: string): ExitDecision {
  if (phase === "open") {
    if (key === "s" || key === "S") return { phase: "closed", effect: "save", intercept: true };
    if (key === "d" || key === "D") return { phase: "closed", effect: "discard", intercept: true };
    if (key === "Escape") return { phase: "closed", effect: "none", intercept: true };
    // Swallow every other key while the modal prompt is open.
    return { phase: "open", effect: "none", intercept: true };
  }
  if (key === "Escape") {
    return dirty
      ? { phase: "open", effect: "none", intercept: true }
      : { phase: "closed", effect: "none", intercept: false };
  }
  return { phase: "closed", effect: "none", intercept: false };
}
