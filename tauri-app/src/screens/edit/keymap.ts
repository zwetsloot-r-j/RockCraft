// keymap.ts — the normal-mode keymap for the composer edit screen.
//
// This is a *declarative mirror* of `crates/tui/src/edit.rs::key_to_action()`
// (and `chord_key_to_action()`), kept as a table — not a switch — so review
// against the TUI is line-by-line. The frontend holds **zero** edit logic: each
// binding names a `core::Action` (snake_case `ActionName`) plus its params, and
// the screen dispatches it over the IPC bridge's `run_action`. The composer
// interprets mode-sensitive keys (e.g. `hjkl` while grabbing) per its own state,
// exactly as the TUI does.
//
// The few VEL/nudge step constants mirror the TUI's `edit.rs` consts so the
// magnitudes stay identical (`VEL_STEP = 8`, fine 10 ms, coarse 250 ms).

import type { ActionName, ActionParams } from "../../ipc/types";

/** Velocity step for `+`/`-` (mirrors `edit.rs::VEL_STEP`). */
const VEL_STEP = 8;
/** Fine backing-alignment nudge (10 ms), bound to `,` / `.`. */
const BACKING_NUDGE_FINE_US = 10_000;
/** Coarse backing-alignment nudge (250 ms), bound to `;` / `'`. */
const BACKING_NUDGE_COARSE_US = 250_000;
/** Tempo nudge in BPM, bound to `(` / `)` (mirrors `edit.rs::BPM_NUDGE`). */
const BPM_NUDGE = 5;

// ── background-image layout (M14-D) ─────────────────────────────────────────
// Pan/zoom/rotate steps for background mode, in the integer units the actions
// take (permille of a surface width/height, permille of scale, millidegrees).
/** Pan step: 2.5% of the surface per press. */
const BG_PAN_PERMILLE = 25;
/** Zoom step: 5% of the contained fit per press. */
const BG_SCALE_PERMILLE = 50;
/** Rotation step: 2.5° per press. */
const BG_ROTATE_MILLIDEG = 2_500;

/** A resolved action dispatch: an `ActionName` and its (optional) params. */
export interface ActionDispatch {
  name: ActionName;
  params?: ActionParams;
}

/**
 * One binding row: the key(s) it fires on and the action it dispatches.
 *
 * A binding matches a `KeyboardEvent` when `event.key` equals one of `keys`.
 * `event.key` carries the produced character ("h", "H", "[", " " for Space) and
 * the named non-printables ("ArrowLeft", "Escape", "Enter"), which is exactly
 * the granularity `key_to_action` switches on (shift is encoded in the case of
 * the character, so we never inspect modifier flags here).
 */
export interface Binding {
  keys: string[];
  action: ActionDispatch;
}

/**
 * Normal-mode keymap — a row-for-row port of `edit.rs::key_to_action()`.
 *
 * The order/comments follow `edit.rs` so the two tables read in parallel.
 */
export const NORMAL_BINDINGS: Binding[] = [
  // ── navigation ────────────────────────────────────────────────────────
  // Visual layout (vertical, highway-aligned): horizontal = pitch (keyboard,
  // low→high left→right), vertical = time (timeline, earlier→later bottom→top).
  // h/l move pitch left/right; j/k move step down/up (k = ArrowUp = later in
  // time). The action names are identical to the TUI's; only the on-screen axes
  // differ, so `h`→CursorDown (pitch −1), `k`→CursorRight (step +1), etc.
  { keys: ["h", "ArrowLeft"], action: { name: "cursor_down" } }, // pitch −1 (left)
  { keys: ["l", "ArrowRight"], action: { name: "cursor_up" } }, // pitch +1 (right)
  { keys: ["j", "ArrowDown"], action: { name: "cursor_left" } }, // step −1 (down/earlier)
  { keys: ["k", "ArrowUp"], action: { name: "cursor_right" } }, // step +1 (up/later)
  { keys: ["H"], action: { name: "cursor_bar_left" } }, // one bar earlier (down)
  { keys: ["L"], action: { name: "cursor_bar_right" } }, // one bar later (up)
  { keys: ["w"], action: { name: "cursor_octave_up" } }, // octave +
  { keys: ["b"], action: { name: "cursor_octave_down" } }, // octave −
  { keys: ["J"], action: { name: "cursor_octave_down" } }, // alias for b
  { keys: ["K"], action: { name: "cursor_octave_up" } }, // alias for w
  { keys: ["g"], action: { name: "cursor_to_start" } }, // timeline start
  { keys: ["G"], action: { name: "cursor_to_end" } }, // timeline end
  { keys: ["0"], action: { name: "cursor_to_pitch_min" } }, // A0 (MIDI 21)
  { keys: ["$"], action: { name: "cursor_to_pitch_max" } }, // C8 (MIDI 108)
  { keys: [">"], action: { name: "subdivision_finer" } },
  { keys: ["<"], action: { name: "subdivision_coarser" } },

  // ── edit ──────────────────────────────────────────────────────────────
  { keys: ["a", "i"], action: { name: "add_note" } },
  { keys: ["x", "d"], action: { name: "delete_note" } },
  // Ripple bar edits: A inserts an empty bar at the cursor ("add a bar"), Z cuts
  // the cursor's bar and closes the gap. (Uppercase = the bigger, structural op.)
  { keys: ["A"], action: { name: "insert_bar" } },
  { keys: ["Z"], action: { name: "remove_bar" } },
  // Per-bar length by one grid STEP — moves the bar LINES only, no notes/time
  // (Q shorter / W longer; use </> to change the step size for finer/coarser).
  // Per-bar tempo re-times the notes inside the bar (e faster / r slower) — the
  // four bar ops sit on the QWER row. (F/V are owned by the split editor and the
  // video backdrop in EditScreen, so tempo uses e/r instead of the TUI's F/V.)
  { keys: ["Q"], action: { name: "nudge_bar_length", params: { delta_steps: -1 } } },
  { keys: ["W"], action: { name: "nudge_bar_length", params: { delta_steps: 1 } } },
  { keys: ["e"], action: { name: "nudge_bar_tempo", params: { delta: -1 } } },
  { keys: ["r"], action: { name: "nudge_bar_tempo", params: { delta: 1 } } },
  { keys: ["]"], action: { name: "resize_note", params: { delta_steps: 1 } } },
  { keys: ["["], action: { name: "resize_note", params: { delta_steps: -1 } } },
  {
    keys: ["+", "="],
    action: { name: "adjust_velocity", params: { delta: VEL_STEP } },
  },
  {
    keys: ["-"],
    action: { name: "adjust_velocity", params: { delta: -VEL_STEP } },
  },
  { keys: ["m"], action: { name: "toggle_grab" } },
  { keys: ["c"], action: { name: "enter_chord_mode" } },

  // ── tempo (BPM) ─────────────────────────────────────────────────────────
  // `(` / `)` nudge tempo down/up. `T` opens an absolute set-BPM prompt, handled
  // in EditScreen (text entry), not as a binding here — mirrors `edit.rs`.
  { keys: ["("], action: { name: "adjust_bpm", params: { delta: -BPM_NUDGE } } },
  { keys: [")"], action: { name: "adjust_bpm", params: { delta: BPM_NUDGE } } },

  // ── input mode ────────────────────────────────────────────────────────
  { keys: ["R"], action: { name: "toggle_record_arm" } },
  { keys: ["t"], action: { name: "toggle_record_flavour" } },

  // ── transport ─────────────────────────────────────────────────────────
  { keys: [" "], action: { name: "toggle_play_cursor" } },
  { keys: ["P"], action: { name: "play_from_start" } },

  // ── backing alignment ─────────────────────────────────────────────────
  {
    keys: [","],
    action: {
      name: "nudge_backing_offset",
      params: { delta_us: -BACKING_NUDGE_FINE_US },
    },
  },
  {
    keys: ["."],
    action: {
      name: "nudge_backing_offset",
      params: { delta_us: BACKING_NUDGE_FINE_US },
    },
  },
  {
    keys: [";"],
    action: {
      name: "nudge_backing_offset",
      params: { delta_us: -BACKING_NUDGE_COARSE_US },
    },
  },
  {
    keys: ["'"],
    action: {
      name: "nudge_backing_offset",
      params: { delta_us: BACKING_NUDGE_COARSE_US },
    },
  },

  // ── loop / metronome / count-in ───────────────────────────────────────
  { keys: ["o"], action: { name: "toggle_loop" } },
  { keys: ["{"], action: { name: "set_loop_start" } }, // loop-in at cursor
  { keys: ["}"], action: { name: "set_loop_end" } }, // loop-out at cursor
  { keys: ["M"], action: { name: "toggle_metronome" } },
  { keys: ["C"], action: { name: "start_count_in_record" } },

  // ── selection / clipboard ─────────────────────────────────────────────
  { keys: ["v"], action: { name: "start_selection" } },
  { keys: ["y"], action: { name: "yank_selection" } },
  { keys: ["p"], action: { name: "paste_clipboard" } },
  { keys: ["D"], action: { name: "delete_selection" } },
  // Esc → clear_selection is handled specially (see resolveKey) so the router's
  // global Esc still fires when there's nothing to clear.

  // ── hand assignment (M14-E) ───────────────────────────────────────────
  // `n` cycles the hand of the cursor note (or the whole selection) through
  // auto → left → right → auto. The split line itself is nudged with
  // Shift+Left/Right, handled in EditScreen (it needs the current split from
  // the snapshot to compute the absolute target pitch).
  { keys: ["n"], action: { name: "cycle_note_hand" } },

  // ── history ───────────────────────────────────────────────────────────
  { keys: ["u"], action: { name: "undo" } },
  { keys: ["U"], action: { name: "redo" } },
];

/**
 * Chord-mode keymap — a port of `edit.rs::chord_key_to_action()`. Live while a
 * chord preview is open; the selector overlay (ChordSelectorPanel) shows the
 * current degree, kind, and preview pitches (#165).
 */
export const CHORD_BINDINGS: Binding[] = [
  { keys: ["1"], action: { name: "set_chord_degree", params: { degree: 1 } } },
  { keys: ["2"], action: { name: "set_chord_degree", params: { degree: 2 } } },
  { keys: ["3"], action: { name: "set_chord_degree", params: { degree: 3 } } },
  { keys: ["4"], action: { name: "set_chord_degree", params: { degree: 4 } } },
  { keys: ["5"], action: { name: "set_chord_degree", params: { degree: 5 } } },
  { keys: ["6"], action: { name: "set_chord_degree", params: { degree: 6 } } },
  { keys: ["7"], action: { name: "set_chord_degree", params: { degree: 7 } } },
  { keys: ["]"], action: { name: "cycle_chord_degree", params: { delta: 1 } } },
  { keys: ["["], action: { name: "cycle_chord_degree", params: { delta: -1 } } },
  { keys: ["s"], action: { name: "toggle_chord_kind" } },
  { keys: ["Enter"], action: { name: "commit_chord" } },
  // Esc → cancel_chord, handled in resolveKey.
];

/**
 * Background-mode keymap (M14-D) — live while the edit screen is laying out a
 * background image layer. A modal map, like {@link CHORD_BINDINGS}, so the
 * layout keys can be plain letters instead of modifier chords.
 *
 * Every binding is a `core::Action`: the frontend holds no layout logic, and
 * each nudge auto-keyframes at the playhead in `core`.
 */
export const BACKGROUND_BINDINGS: Binding[] = [
  // ── pan ───────────────────────────────────────────────────────────────
  // Screen-natural: the arrows/hjkl move the *image*, not a cursor.
  {
    keys: ["h", "ArrowLeft"],
    action: {
      name: "nudge_background_pos",
      params: { dx_permille: -BG_PAN_PERMILLE, dy_permille: 0 },
    },
  },
  {
    keys: ["l", "ArrowRight"],
    action: {
      name: "nudge_background_pos",
      params: { dx_permille: BG_PAN_PERMILLE, dy_permille: 0 },
    },
  },
  {
    keys: ["k", "ArrowUp"],
    action: {
      name: "nudge_background_pos",
      params: { dx_permille: 0, dy_permille: -BG_PAN_PERMILLE },
    },
  },
  {
    keys: ["j", "ArrowDown"],
    action: {
      name: "nudge_background_pos",
      params: { dx_permille: 0, dy_permille: BG_PAN_PERMILLE },
    },
  },

  // ── zoom / rotate ─────────────────────────────────────────────────────
  {
    keys: ["+", "="],
    action: {
      name: "nudge_background_scale",
      params: { delta_permille: BG_SCALE_PERMILLE },
    },
  },
  {
    keys: ["-"],
    action: {
      name: "nudge_background_scale",
      params: { delta_permille: -BG_SCALE_PERMILLE },
    },
  },
  {
    keys: ["]"],
    action: {
      name: "nudge_background_rotation",
      params: { delta_millideg: BG_ROTATE_MILLIDEG },
    },
  },
  {
    keys: ["["],
    action: {
      name: "nudge_background_rotation",
      params: { delta_millideg: -BG_ROTATE_MILLIDEG },
    },
  },

  // ── opacity: the number row sets it directly (0 = clear, 9 = 90%) ─────
  ...Array.from<unknown, Binding>({ length: 10 }, (_, n) => ({
    keys: [String(n)],
    action: {
      name: "set_background_opacity",
      params: { permille: n * 100 },
    },
  })),
  { keys: ["o"], action: { name: "set_background_opacity", params: { permille: 1000 } } },

  // ── keyframes ─────────────────────────────────────────────────────────
  { keys: ["f"], action: { name: "add_background_keyframe" } },
  { keys: ["x", "d"], action: { name: "delete_background_keyframe" } },
  {
    keys: ["e"],
    action: { name: "set_background_easing", params: { easing: "ease_in_out" } },
  },
  {
    keys: ["E"],
    action: { name: "set_background_easing", params: { easing: "linear" } },
  },
  {
    keys: ["c"],
    action: { name: "set_background_easing", params: { easing: "hold" } },
  },

  // ── layer selection ───────────────────────────────────────────────────
  { keys: ["n", "Tab"], action: { name: "cycle_background", params: { delta: 1 } } },
  { keys: ["N"], action: { name: "cycle_background", params: { delta: -1 } } },
];

/** What `resolveKey` decided to do with a `KeyboardEvent`. */
export type KeyResolution =
  | { kind: "action"; dispatch: ActionDispatch }
  /**
   * Chord key or a special modal key: dispatch and swallow (prevent bubbling so
   * the router's global Esc or other shell handlers don't fire).
   */
  | { kind: "action-swallow"; dispatch: ActionDispatch }
  /** Swallow the event but dispatch nothing (chord mode owns the keyboard). */
  | { kind: "swallow" }
  /** Nothing bound — let the event bubble (e.g. router's global Esc). */
  | { kind: "ignore" };

/**
 * Resolve a key while background-layout mode owns the keyboard (M14-D).
 *
 * Every key is swallowed so nothing leaks to the normal keymap or the router
 * while laying a backdrop out; `Escape` leaves the mode and is reported as
 * `{ kind: "swallow" }` so the caller can exit without dispatching an action.
 */
export function resolveBackgroundKey(key: string): KeyResolution {
  const dispatch = lookup(BACKGROUND_BINDINGS, key);
  return dispatch ? { kind: "action-swallow", dispatch } : { kind: "swallow" };
}

function lookup(bindings: Binding[], key: string): ActionDispatch | undefined {
  for (const b of bindings) {
    if (b.keys.includes(key)) return b.action;
  }
  return undefined;
}

/**
 * Resolve a key to an action, mirroring `edit.rs::on_key`'s routing:
 *
 * - While a chord preview is open the chord keymap owns the keyboard; any other
 *   key is swallowed (the ChordSelectorPanel holds focus). Esc cancels the chord.
 * - Otherwise the normal keymap applies. Esc clears an active selection (and is
 *   swallowed); with no selection it is passed through so the EditScreen can
 *   show the dirty-exit prompt or let the router return to the menu.
 *
 * @param key             `KeyboardEvent.key`.
 * @param chordActive     whether `snapshot.chord_preview` is non-null.
 * @param selectionActive whether `snapshot.selection` is non-null.
 */
export function resolveKey(
  key: string,
  chordActive: boolean,
  selectionActive: boolean,
): KeyResolution {
  if (chordActive) {
    if (key === "Escape") {
      return { kind: "action-swallow", dispatch: { name: "cancel_chord" } };
    }
    const dispatch = lookup(CHORD_BINDINGS, key);
    // Chord mode swallows every key (the selector owns the keyboard) so nothing
    // leaks to the normal map or the router while a preview is open.
    return dispatch ? { kind: "action-swallow", dispatch } : { kind: "swallow" };
  }

  if (key === "Escape") {
    return selectionActive
      ? { kind: "action-swallow", dispatch: { name: "clear_selection" } }
      : { kind: "ignore" };
  }

  const dispatch = lookup(NORMAL_BINDINGS, key);
  return dispatch ? { kind: "action", dispatch } : { kind: "ignore" };
}
