// EditScreen.tsx — the composer edit screen: a status bar over a canvas
// piano-roll grid. A faithful port of `crates/tui/src/edit.rs`'s view + keymap:
// it renders the latest `ComposerSnapshot` and dispatches every key through
// `run_action` (no edit logic here). Snapshots arrive on the bridge's `snapshot`
// event (primed by `queryState` on mount); while playing, a RAF interpolates the
// playhead between events for a smooth sweep.

import { createSignal, onCleanup, onMount, Show, type JSX } from "solid-js";
import { createStore } from "solid-js/store";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { onSnapshot, queryState, runAction } from "../../ipc/bridge";
import type { ComposerSnapshot } from "../../ipc/types";
import { EditCanvas } from "./EditCanvas";
import { StatusBar } from "./StatusBar";
import { resolveKey } from "./keymap";

export function EditScreen(): JSX.Element {
  let canvasEl!: HTMLCanvasElement;
  let engine: EditCanvas | null = null;
  let raf = 0;

  // The snapshot mirror. `createStore` so SolidJS tracks field reads in the
  // StatusBar without re-creating the object identity each event.
  const [store, setStore] = createStore<{ snap: ComposerSnapshot | null }>({
    snap: null,
  });
  // Re-render trigger for the StatusBar; bumped on each snapshot.
  const [, setRev] = createSignal(0);

  // Wall-clock anchor for playhead interpolation: the snapshot's playhead_us at
  // the moment the snapshot arrived, plus the perf-clock reading then. Between
  // events we add the elapsed wall time; each real event re-seats both.
  let basePlayheadUs = 0;
  let baseWallMs = 0;

  /** Current playhead µs: the snapshot value while stopped, interpolated while playing. */
  function interpolatedPlayheadUs(s: ComposerSnapshot): number {
    if (!s.playing) return s.playhead_us;
    const elapsedUs = (performance.now() - baseWallMs) * 1000;
    return basePlayheadUs + elapsedUs;
  }

  function render(): void {
    const s = store.snap;
    if (!engine || !s) return;
    engine.draw(s, interpolatedPlayheadUs(s));
  }

  function applySnapshot(s: ComposerSnapshot): void {
    basePlayheadUs = s.playhead_us;
    baseWallMs = performance.now();
    setStore("snap", s);
    setRev((r) => r + 1);
    render();
  }

  function onKeydown(e: KeyboardEvent): void {
    const s = store.snap;
    if (!s) return;
    // Ignore modifier-chorded keys (Ctrl/Cmd/Alt) so browser/OS shortcuts pass
    // through; the editor keymap is plain keys + shift only, like the TUI.
    if (e.ctrlKey || e.metaKey || e.altKey) return;

    const res = resolveKey(e.key, s.chord_preview !== null, s.selection !== null);
    switch (res.kind) {
      case "action":
        e.preventDefault();
        void runAction(res.dispatch.name, res.dispatch.params);
        break;
      case "action-swallow":
        e.preventDefault();
        e.stopPropagation();
        void runAction(res.dispatch.name, res.dispatch.params);
        break;
      case "swallow":
        e.preventDefault();
        e.stopPropagation();
        break;
      case "ignore":
        break;
    }
  }

  onMount(() => {
    engine = new EditCanvas(canvasEl);

    let unlisten: UnlistenFn | undefined;
    onSnapshot(applySnapshot).then((fn) => {
      unlisten = fn;
    });
    queryState()
      .then(applySnapshot)
      .catch(() => {
        /* backend not up (plain vite without tauri) — render stays blank */
      });

    // The smooth-playhead loop: redraw every frame while playing so the
    // interpolated playhead sweeps between snapshot events; idle otherwise.
    const loop = (): void => {
      const s = store.snap;
      if (s?.playing) render();
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);

    window.addEventListener("keydown", onKeydown);
    onCleanup(() => {
      window.removeEventListener("keydown", onKeydown);
      cancelAnimationFrame(raf);
      unlisten?.();
      engine?.dispose();
      engine = null;
    });
  });

  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        "flex-direction": "column",
        background: "#0f1016",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      <Show
        when={store.snap}
        fallback={
          <div
            style={{
              height: "34px",
              display: "flex",
              "align-items": "center",
              padding: "0 14px",
              color: "#6a6e7e",
              "font-size": "12px",
              background: "#15161d",
            }}
          >
            waiting for snapshot…
          </div>
        }
      >
        {(snap) => <StatusBar snapshot={snap()} />}
      </Show>
      <div style={{ flex: "1 1 auto", "min-height": 0, position: "relative" }}>
        <canvas
          ref={canvasEl}
          style={{ width: "100%", height: "100%", display: "block" }}
        />
      </div>
    </div>
  );
}
