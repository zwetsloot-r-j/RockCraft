// hand.ts — deriving a note's hand for the edit grid (M14-E).
//
// The *rule* lives in `core` (`crates/core/src/hand.rs`): a note follows the
// piece's pitch **split line** unless it carries an explicit override. The
// snapshot ships both halves separately — `NoteView.hand` (the raw override,
// usually null) and `ComposerSnapshot.hand_split` — so the canvas resolves them
// here, in one pure place, rather than at every draw call.
//
// Colours are the highway's `handColors` palette so a note reads as the same
// hand in the editor and on the play screen.

import type { Hand } from "../../ipc/types";

/** Middle C — the split `core` falls back to (mirror of `DEFAULT_SPLIT`). */
export const DEFAULT_SPLIT = 60;

/**
 * Per-hand note tint. Same values as `HighwayCanvas`'s `handColors`, so the
 * editor and the highway agree on which colour means which hand.
 */
export const HAND_COLORS: Record<Hand, string> = {
  left: "#5ad1c7",
  right: "#e6a14b",
};

/**
 * Which hand plays a note: the override when it has one, else the split rule
 * (`pitch < split` → left, at/above → right). Mirror of
 * `core::timeline::Note::effective_hand`.
 */
export function effectiveHand(
  pitch: number,
  hand: Hand | null | undefined,
  split: number,
): Hand {
  if (hand === "left" || hand === "right") return hand;
  return pitch < split ? "left" : "right";
}

/** Whether a note carries an authored exception to the split line. */
export function isOverridden(hand: Hand | null | undefined): boolean {
  return hand === "left" || hand === "right";
}

/** The colour a note is drawn in, plus whether it is an authored exception. */
export function handTint(
  pitch: number,
  hand: Hand | null | undefined,
  split: number,
): { color: string; overridden: boolean } {
  return {
    color: HAND_COLORS[effectiveHand(pitch, hand, split)],
    overridden: isOverridden(hand),
  };
}
