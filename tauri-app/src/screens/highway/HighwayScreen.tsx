// HighwayScreen.tsx — top-level Spectrum Live (Design D) screen. Mounts the
// FusionProto header + the HighwayCanvas engine on a full-viewport layout.
//
// Design D config (cfgFusion) from prototypes.jsx: spectrum pitch-class colors,
// flat keyboard with note-name labels, pitch ruler, soft beat grid, scoring.

import { createSignal, onCleanup, onMount } from "solid-js";
import { HighwayCanvas } from "./HighwayCanvas";
import { HighwayHeader } from "./HighwayHeader";
import { SONG } from "./song";
import type { HighwayConfig } from "./types";

const cfgFusion: HighwayConfig = {
  colorMode: "spectrum",
  perspective: 0,
  glow: 0.32,
  gridlines: "soft",
  keyboard: "flat",
  labels: true,
  pitchRuler: true,
  kbRatio: 0.2,
  lead: 3000,
  bg: "#0f1016",
  hitLine: "#aab2d0",
  noteGap: 0.2,
  radius: 3,
  scoring: true,
  laneTint: "rgba(255,255,255,0.012)",
};

export function HighwayScreen() {
  let canvasEl!: HTMLCanvasElement;
  const [eng, setEng] = createSignal<HighwayCanvas | null>(null);
  // ~9 fps throttle so the header re-reads the engine without a per-frame render.
  const [frame, setFrame] = createSignal(0);

  onMount(() => {
    const engine = new HighwayCanvas(canvasEl, cfgFusion, SONG);
    engine.start();
    setEng(engine);
    const id = setInterval(() => setFrame((f) => f + 1), 110);
    onCleanup(() => {
      clearInterval(id);
      engine.stop();
    });
  });

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        "flex-direction": "column",
        background: "#0f1016",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      <HighwayHeader eng={eng} frame={frame} song={SONG} />
      <div style={{ flex: "1 1 auto", "min-height": 0, position: "relative" }}>
        <canvas ref={canvasEl} style={{ width: "100%", height: "100%", display: "block" }} />
      </div>
    </div>
  );
}
