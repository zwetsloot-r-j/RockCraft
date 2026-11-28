// hoverGate.ts — keep a resting mouse from stealing the keyboard's selection.
//
// When ↑/↓ scroll the list, rows slide under a mouse that hasn't moved, and
// the webview reports that as the pointer entering (or moving over) a new row.
// Selecting on those would yank the highlight to whatever row sits under the
// cursor — the middle of the screen — instead of the next row. So hover only
// selects once the pointer has really moved: its screen position changed
// since the last event we saw.

export class HoverGate {
  private last: { x: number; y: number } | null = null;
  /** True while the keyboard owns the selection (also on screen open). */
  private keyboard = true;

  /** A keyboard move: the next hover selects only after real movement. */
  keyMoved(): void {
    this.keyboard = true;
  }

  /** A pointer event over a row: should it select that row? */
  shouldSelect(e: Pick<MouseEvent, "screenX" | "screenY">): boolean {
    const prev = this.last;
    this.last = { x: e.screenX, y: e.screenY };
    // The first event only sets the baseline: it can't tell a moving mouse
    // from rows appearing under a still one.
    if (prev === null) return false;
    if (prev.x !== e.screenX || prev.y !== e.screenY) this.keyboard = false;
    return !this.keyboard;
  }
}
