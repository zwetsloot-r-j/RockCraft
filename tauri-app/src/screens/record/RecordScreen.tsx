// RecordScreen.tsx — Design E "Studio (full + notation)": a live recording
// session. Notes rise from the keyboard and crystallise into a grand staff
// above; full transport/edit toolbar below; selected-note inspector floats
// top-right.
//
// Live MIDI wiring (#169):
// - onMidiEvent feeds RecordCanvas; recording session managed via record_start /
//   record_stop / record_save Tauri commands.
// - The Ember Lantern fixture is kept as the no-session demo fallback.
// - "s" saves, Esc confirms-if-unsaved then navigates to menu.
// - "Choose backing track" opens a native file dialog and attaches audio.

import { createSignal, type JSX, onCleanup, onMount } from "solid-js";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import "./record.css";
import { RecordCanvas } from "./RecordCanvas";
import { RecordHeader } from "./RecordHeader";
import { RecordToolbar, type Clef, type Snap, type Spelling } from "./RecordToolbar";
import { NoteInspector } from "./NoteInspector";
import { RSONG } from "./song";
import { DISP } from "./ui/theme";
import { onMidiEvent } from "../../ipc/midi";
import {
  recordStart,
  recordStop,
  recordSave,
} from "../../ipc/bridge";

// Audio extensions accepted by the backing-track dialog (mirrors
// crates/tui/src/backing.rs and crates/audio/src/lib.rs).
const AUDIO_EXTENSIONS = ["mp3", "wav", "ogg", "flac", "m4a"];

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

  // Session state
  const [recording, setRecording] = createSignal(false);
  const [dirty, setDirty] = createSignal(false);
  const [toast, setToast] = createSignal<string | null>(null);
  const [backingPath, setBackingPath] = createSignal<string | null>(null);
  const [sessionStart, setSessionStart] = createSignal<number>(0);

  function showToast(msg: string): void {
    setToast(msg);
    window.setTimeout(() => setToast(null), 3000);
  }

  // ── Transport ────────────────────────────────────────────────────────────────

  async function startRecording(): Promise<void> {
    const e = eng();
    if (recording() || !e) return;
    try {
      await recordStart(backingPath() ?? undefined);
      e.startSession();
      setRecording(true);
      setDirty(false);
      setSessionStart(Date.now());
    } catch (err) {
      showToast(`Start failed: ${err}`);
    }
  }

  async function stopRecording(): Promise<void> {
    const e = eng();
    if (!recording()) return;
    try {
      await recordStop();
    } catch (_) {
      // ignore — session may already be inactive
    }
    e?.stopSession();
    setRecording(false);
  }

  async function saveRecording(): Promise<void> {
    if (!recording() && !dirty()) return;
    try {
      const dir = await recordSave();
      setDirty(false);
      showToast(`Saved → ${dir}`);
    } catch (err) {
      showToast(`Save failed: ${err}`);
    }
  }

  // ── Backing file dialog ──────────────────────────────────────────────────────

  async function chooseBacking(): Promise<void> {
    try {
      const selected = await openDialog({
        title: "Choose backing track",
        filters: [{ name: "Audio", extensions: AUDIO_EXTENSIONS }],
        multiple: false,
        directory: false,
      });
      if (selected && typeof selected === "string") {
        setBackingPath(selected);
        showToast(`Backing: ${selected.split(/[/\\]/).pop()}`);
      }
    } catch (err) {
      showToast(`Dialog error: ${err}`);
    }
  }

  // ── Keyboard shortcuts ────────────────────────────────────────────────────────

  async function handleKeyDown(e: KeyboardEvent): Promise<void> {
    if (e.key === "s" || e.key === "S") {
      e.preventDefault();
      await saveRecording();
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (dirty()) {
        // For now just stop; a confirm dialog is a future enhancement.
        await stopRecording();
      } else {
        await stopRecording();
      }
    } else if (e.key === "r" || e.key === "R") {
      e.preventDefault();
      if (recording()) {
        await stopRecording();
      } else {
        await startRecording();
      }
    }
  }

  onMount(() => {
    const e = new RecordCanvas(
      canvasEl,
      { viz: "ribbons+staff", glow: 0.5, window: 2200, kbRatio: 0.14 },
      RSONG,
    );
    e.start();
    setEng(e);
    const frameId = window.setInterval(() => setFrame((f) => f + 1), 110);

    // Subscribe to MIDI events and feed them to the canvas (+ mark dirty).
    const unlistenPromise = onMidiEvent((ev) => {
      const canvas = eng();
      if (!canvas) return;
      if (ev.on) {
        canvas.noteOn(ev.note, ev.velocity);
        if (recording()) setDirty(true);
      } else {
        canvas.noteOff(ev.note);
      }
    });

    window.addEventListener("keydown", handleKeyDown);

    onCleanup(() => {
      window.clearInterval(frameId);
      e.stop();
      unlistenPromise.then((unlisten) => unlisten());
      window.removeEventListener("keydown", handleKeyDown);
      // Stop the session if the screen is unmounted while recording.
      if (recording()) {
        recordStop().catch(() => {});
      }
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
      {/* Toast notification */}
      {toast() && (
        <div
          style={{
            position: "fixed",
            top: "16px",
            right: "16px",
            background: "rgba(30,32,44,0.95)",
            border: "1px solid rgba(255,255,255,0.12)",
            "border-radius": "8px",
            padding: "10px 16px",
            "font-size": "12px",
            "z-index": 1000,
            "pointer-events": "none",
          }}
        >
          {toast()}
        </div>
      )}
      <RecordHeader
        eng={eng}
        frame={frame}
        song={RSONG}
        metro={metro}
        onMetro={setMetro}
        count={count}
        onCount={setCount}
        recording={recording}
        sessionStart={sessionStart}
        backingPath={backingPath}
        onChooseBacking={chooseBacking}
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
        recording={recording}
        onRecord={startRecording}
        onStop={stopRecording}
        onSave={saveRecording}
      />
    </div>
  );
}
