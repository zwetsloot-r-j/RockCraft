// fullscreen.ts — F11 / Alt+Enter full screen, remembered across launches.
//
// WebView2 doesn't map those keys to full screen as a browser does, so the
// shell catches them and asks the backend (`toggle_fullscreen`, also reachable
// over the control socket). The backend announces every change, whichever way
// it came, and the shell stores it here to restore on the next launch.

/** `localStorage` key for the last full-screen state. */
export const FULLSCREEN_KEY = "rc.app.fullscreen";

/** Is this key press the full-screen toggle: F11, or Alt+Enter? */
export function isFullscreenKey(
  e: Pick<KeyboardEvent, "key" | "altKey" | "ctrlKey" | "metaKey" | "shiftKey">,
): boolean {
  if (e.ctrlKey || e.metaKey || e.shiftKey) return false;
  return e.key === "F11" ? !e.altKey : e.key === "Enter" && e.altKey;
}

/** Was the app full screen when it last closed? */
export function readFullscreen(): boolean {
  try {
    return localStorage.getItem(FULLSCREEN_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remember the full-screen state for the next launch. */
export function writeFullscreen(on: boolean): void {
  try {
    localStorage.setItem(FULLSCREEN_KEY, on ? "1" : "0");
  } catch {
    /* storage disabled — just don't remember */
  }
}
