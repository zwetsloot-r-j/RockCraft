// Piano menu navigation: C confirm · C# cancel · D up · E down (any octave).
//
// The backend tags each `midi_event` key-down with its `menu_nav`
// (`rockcraft_core::MenuNav`). This module decides, per screen, which keyboard
// key that press stands for; the Router then dispatches it as a synthetic
// `keydown`, so every screen's existing key handling serves the piano too.
// Mirrors the TUI's `Shell::menu_nav_key` (crates/tui/src/app.rs).

import type { MenuNav } from "../ipc/midi";
import type { Screen } from "./screens";

const KEY_FOR: Record<MenuNav, string> = {
  confirm: "Enter",
  cancel: "Escape",
  up: "ArrowUp",
  down: "ArrowDown",
};

/**
 * The `KeyboardEvent.key` a piano menu key stands for on `screen`, or `null`
 * when it should navigate nothing there.
 *
 * - Only browsing screens navigate: on play and edit every key is music.
 * - The URL box is text entry — left to the computer keyboard.
 * - Cancel is inert on the main menu, matching the TUI (where Esc quits).
 */
export function menuNavKey(screen: Screen["kind"], nav: MenuNav | null): string | null {
  if (nav === null) return null;
  switch (screen) {
    case "menu":
      return nav === "cancel" ? null : KEY_FOR[nav];
    case "library":
    case "importing":
      return KEY_FOR[nav];
    default:
      return null;
  }
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
