/**
 * Vim-style count prefix for the edit keymap (M25-A): type a number, then a
 * move key, and the move repeats that many times (`9k` = nine steps later,
 * `4L` = four bars later). Pure helpers; EditScreen holds the pending count.
 */
import type { ActionName } from "../../ipc/types";

/** Largest count the prefix accumulates; further digits are ignored. */
export const COUNT_MAX = 999;

/** The cursor moves a count repeats. Every other action consumes the count. */
export const COUNTABLE_ACTIONS: ReadonlySet<ActionName> = new Set<ActionName>([
  "cursor_left",
  "cursor_right",
  "cursor_up",
  "cursor_down",
  "cursor_bar_left",
  "cursor_bar_right",
  "cursor_octave_up",
  "cursor_octave_down",
]);

/** Modifier keydowns (Shift for `H`/`L`, …) leave a pending count alone. */
const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock"]);

/**
 * The pending count after `key`, or `null` when `key` isn't part of a count.
 * `1`–`9` start or extend a count; `0` only extends one, so a bare `0` keeps
 * its own binding (lowest pitch), as in vim.
 */
export function countAfterKey(pending: number | null, key: string): number | null {
  if (key.length !== 1 || key < "0" || key > "9") return null;
  if (pending === null) return key === "0" ? null : Number(key);
  return Math.min(pending * 10 + Number(key), COUNT_MAX);
}

/** Whether `key` should leave a pending count in place (a bare modifier). */
export function keepsCount(key: string): boolean {
  return MODIFIER_KEYS.has(key);
}

/** How many times to run `action` given the pending count. */
export function repeatsFor(action: ActionName, pending: number | null): number {
  return pending !== null && COUNTABLE_ACTIONS.has(action) ? pending : 1;
}
