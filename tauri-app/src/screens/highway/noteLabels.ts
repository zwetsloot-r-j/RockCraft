/**
 * What text a highway note carries (M27-C): its pitch name and, when the piece
 * gives it one, its finger number. Pure, so the placement rules are unit
 * tested; HighwayCanvas draws whatever this returns.
 *
 * Offsets are px measured up from the note's onset edge (the edge nearest the
 * hit line) to the text's vertical centre.
 */

/** Notes taller than this carry the pitch name (the pre-M27 rule). */
export const NAME_MIN_H = 16;
/** Notes taller than this have room for the finger *and* the name. */
export const BOTH_MIN_H = 32;
/** Notes shorter than this cannot hold a digit; it goes just past the tail. */
export const FINGER_INSIDE_MIN_H = 12;

export interface NoteLabels {
  /** Offset of the pitch name, or null when it is not drawn. */
  name: number | null;
  /** Offset of the finger digit, or null when the note has no finger. */
  finger: number | null;
  /** The digit sits outside the note (past its tail), not on it. */
  fingerOutside: boolean;
}

/**
 * Lay out a note's labels.
 *
 * @param heightPx  the note body's on-screen height
 * @param labels    whether pitch names are switched on
 * @param finger    the note's finger number, if any
 */
export function noteLabels(
  heightPx: number,
  labels: boolean,
  finger: number | null | undefined,
): NoteLabels {
  const nameFits = labels && heightPx > NAME_MIN_H;
  if (finger == null) {
    // No finger: exactly the old behaviour.
    return { name: nameFits ? 9 : null, finger: null, fingerOutside: false };
  }
  if (heightPx > BOTH_MIN_H) {
    // Room for both: the finger takes the onset edge, the name moves up a line.
    return { name: labels ? 23 : null, finger: 9, fingerOutside: false };
  }
  if (heightPx >= FINGER_INSIDE_MIN_H) {
    // One line only: the finger wins over the name.
    return { name: null, finger: Math.min(9, heightPx / 2), fingerOutside: false };
  }
  // Too short to hold a digit: just beyond the tail, so it still shows.
  return { name: null, finger: heightPx + 8, fingerOutside: true };
}
