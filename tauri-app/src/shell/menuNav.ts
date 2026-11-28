// Piano menu navigation: C confirm · C# cancel · D up · E down (any octave),
// and A0 — the lowest key of an 88-key piano — toggles free play, during which
// those keys are just notes.
//
// The backend tags each `midi_event` key-down with its `menu_nav`
// (`rockcraft_core::MenuNav`). This module decides, per screen, which keyboard
// key that press stands for; the Router then dispatches it as a synthetic
// `keydown`, so every screen's existing key handling serves the piano too.
// Mirrors the TUI's `Shell::on_menu_nav` (crates/tui/src/app.rs) and
// `rockcraft_core::MenuNavigator`.

import type { MenuNav } from "../ipc/midi";
import type { Screen } from "./screens";

type NavKey = Exclude<MenuNav, "free_play">;

const KEY_FOR: Record<NavKey, string> = {
  confirm: "Enter",
  cancel: "Escape",
  up: "ArrowUp",
  down: "ArrowDown",
};

/**
 * Whether piano menu keys act on `screen` at all. On play and edit every key
 * is music (A0 included); the URL box is text entry, left to the keyboard.
 */
export function takesMenuKeys(screen: Screen["kind"]): boolean {
  return screen === "menu" || screen === "library" || screen === "importing";
}

/** What one piano press does: the key to dispatch (if any) and the new mode. */
export interface MenuNavStep {
  key: string | null;
  freePlay: boolean;
}

/**
 * Apply one piano press on `screen`, given whether free play is on.
 *
 * - A0 (`"free_play"`) flips free play on a menu screen, dispatching nothing.
 * - In free play, C / C# / D / E navigate nothing.
 * - Cancel is inert on the main menu, matching the TUI (where Esc quits).
 */
export function menuNavStep(
  screen: Screen["kind"],
  nav: MenuNav | null,
  freePlay: boolean,
): MenuNavStep {
  if (nav === null || !takesMenuKeys(screen)) return { key: null, freePlay };
  if (nav === "free_play") return { key: null, freePlay: !freePlay };
  if (freePlay) return { key: null, freePlay };
  if (screen === "menu" && nav === "cancel") return { key: null, freePlay };
  return { key: KEY_FOR[nav], freePlay };
}

/** True while a text field has focus — piano keys must not steer around it. */
export function isTyping(el: Element | null): boolean {
  if (!el) return false;
  const tag = el.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    (el as HTMLElement).isContentEditable === true
  );
}
