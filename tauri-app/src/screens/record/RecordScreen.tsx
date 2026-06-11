// RecordScreen.tsx — Design E "Studio (full + notation)": a live recording
// session. Notes rise from the keyboard and crystallise into a grand staff
// above; full transport/edit toolbar below; selected-note inspector floats
// top-right. All data is mocked from the Ember Lantern take fixture — live MIDI
// hookup is #169. See specs/M2-tauri-record-screen.md.

import { createSignal, type JSX, onCleanup, onMount } from "solid-js";
import "./record.css";
import { RecordCanvas } from "./RecordCanvas";
import { RecordHeader } from "./RecordHeader";
import { RecordToolbar, type Clef, type Snap, type Spelling } from "./RecordToolbar";
import { NoteInspector } from "./NoteInspector";
import { RSONG } from "./song";
import { DISP } from "./ui/theme";

export function RecordScreen(): JSX.Element {
  let canvasEl!: HTMLCanvasElement;
  const [eng, setEng] = createSignal<RecordCanvas | null>(null);
  // Bumped ~9 fps so the header/toolbar re-read engine clock/level/selection
  // without coupling chrome updates to the canvas rAF rate.
  const [frame, setFrame] = createSignal(0);
  const [metro, setMetro] = createSignal(true);
  const [count, setCount] = createSignal(true);
  const [snap, setSnap] = createSignal<Snap>("1/16");
  const [clef, setClef] = createSignal<Clef>("Grand");
  const [spelling, setSpelling] = createSignal<Spelling>("♯");

  onMount(() => {
    const e = new RecordCanvas(
      canvasEl,
      { viz: "ribbons+staff", glow: 0.5, window: 2200, kbRatio: 0.14 },
      RSONG,
    );
    e.start();
    setEng(e);
    const id = window.setInterval(() => setFrame((f) => f + 1), 110);
    onCleanup(() => {
      window.clearInterval(id);
      e.stop();
    });
  });

  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        "flex-direction": "column",
        background: "#101119",
        "font-family": DISP,
        color: "#e7e8ef",
      }}
    >
      <RecordHeader
        eng={eng}
        frame={frame}
        song={RSONG}
        metro={metro}
        onMetro={setMetro}
        count={count}
        onCount={setCount}
      />
      <div style={{ flex: "1 1 auto", "min-height": 0, position: "relative" }}>
        <canvas ref={canvasEl} style={{ width: "100%", height: "100%", display: "block" }} />
        <NoteInspector eng={eng} frame={frame} />
      </div>
      <RecordToolbar
        snap={snap}
        onSnap={setSnap}
        clef={clef}
        onClef={setClef}
        spelling={spelling}
        onSpelling={setSpelling}
      />
    </div>
  );
}
