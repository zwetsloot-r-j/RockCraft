// RecordScreen.tsx — Design E "Studio (full + notation)": a live recording
// session. Notes rise from the keyboard and crystallise into a grand staff
// above; full transport/edit toolbar below; selected-note inspector floats
// top-right.
//
// Live MIDI wiring (#169):
// - onMidiEvent feeds RecordCanvas; recording session managed via record_start /
//   record_stop / record_save Tauri commands.
// - The screen starts idle on an empty take (#187 retired the demo fixture);
//   live MIDI fills it in.
// - "s" saves; Esc with unsaved input opens a Save/Discard/Cancel prompt (#188),
//   else the shell navigates to the menu.
// - "Choose backing track" opens a native file dialog and attaches audio.

import { createSignal, type JSX, onCleanup, onMount, Show } from "solid-js";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import "./record.css";
import { RecordCanvas } from "./RecordCanvas";
import { RecordHeader } from "./RecordHeader";
import { RecordToolbar, type Snap } from "./RecordToolbar";
import { NoteInspector } from "./NoteInspector";
import { EMPTY_TAKE } from "./emptyTake";
import { nextExit, type ExitPhase } from "./exit";
import { DISP } from "./ui/theme";
import { useRouter } from "../../shell/Router";
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
  const { navigate } = useRouter();
  let canvasEl!: HTMLCanvasElement;
  const [eng, setEng] = createSignal<RecordCanvas | null>(null);
  // Bumped ~9 fps so the header/toolbar re-read engine clock/level/selection
  // without coupling chrome updates to the canvas rAF rate.
  const [frame, setFrame] = createSignal(0);
  const [snap, setSnap] = createSignal<Snap>("1/16");

  // Session state
  const [recording, setRecording] = createSignal(false);
  const [dirty, setDirty] = createSignal(false);
  const [toast, setToast] = createSignal<string | null>(null);
  const [backingPath, setBackingPath] = createSignal<string | null>(null);
  const [sessionStart, setSessionStart] = createSignal<number>(0);
  // Whether the dirty-exit Save/Discard/Cancel prompt is shown (#188).
  const [exitPrompt, setExitPrompt] = createSignal(false);

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

  /** Save the take; returns whether it succeeded (or there was nothing to do). */
  async function trySave(): Promise<boolean> {
    if (!recording() && !dirty()) return true;
    try {
      const dir = await recordSave();
      setDirty(false);
      showToast(`Saved → ${dir}`);
      return true;
    } catch (err) {
      showToast(`Save failed: ${err}`);
      return false;
    }
  }

  async function saveRecording(): Promise<void> {
    await trySave();
  }

  // ── Dirty-exit prompt (#188) ─────────────────────────────────────────────────

  /** Save → stop → menu. Stays on the screen (with an error toast) if save fails. */
  async function exitWithSave(): Promise<void> {
    if (await trySave()) {
      await stopRecording();
      navigate({ kind: "menu" });
    }
  }

  /** Discard the take: stop the session and return to the menu. */
  async function exitDiscard(): Promise<void> {
    await stopRecording();
    navigate({ kind: "menu" });
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

  // Registered in the capture phase (see onMount) so the dirty-exit prompt can
  // intercept Esc *before* the shell's global Esc→menu handler runs.
  function handleKeyDown(e: KeyboardEvent): void {
    const phase: ExitPhase = exitPrompt() ? "open" : "closed";

    // Esc, and every key while the prompt is open, go through the pure helper.
    if (e.key === "Escape" || phase === "open") {
      const d = nextExit(phase, dirty(), e.key);
      setExitPrompt(d.phase === "open");
      if (d.intercept) {
        e.preventDefault();
        e.stopImmediatePropagation();
      }
      if (d.effect === "save") void exitWithSave();
      else if (d.effect === "discard") void exitDiscard();
      // effect "none" with intercept=false (clean-take Esc): the event is left
      // to propagate to the shell, which navigates to the menu; onCleanup then
      // stops the session if it is still recording.
      return;
    }

    // Normal shortcuts (prompt closed).
    if (e.key === "s" || e.key === "S") {
      e.preventDefault();
      void saveRecording();
    } else if (e.key === "r" || e.key === "R") {
      e.preventDefault();
      if (recording()) void stopRecording();
      else void startRecording();
    }
  }

  onMount(() => {
    const e = new RecordCanvas(
      canvasEl,
      { viz: "ribbons+staff", glow: 0.5, window: 2200, kbRatio: 0.14 },
      EMPTY_TAKE,
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

    // Capture phase: intercept Esc before the shell's global Esc→menu handler.
    window.addEventListener("keydown", handleKeyDown, true);

    onCleanup(() => {
      window.clearInterval(frameId);
      e.stop();
      unlistenPromise.then((unlisten) => unlisten());
      window.removeEventListener("keydown", handleKeyDown, true);
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
        song={EMPTY_TAKE}
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
        recording={recording}
        onRecord={startRecording}
        onStop={stopRecording}
        onSave={saveRecording}
      />

      {/* Dirty-exit confirm prompt (#188): Esc with unsaved input lands here. */}
      <Show when={exitPrompt()}>
        <div
          style={{
            position: "fixed",
            inset: 0,
            background: "rgba(8,9,14,0.66)",
            display: "flex",
            "align-items": "center",
            "justify-content": "center",
            "z-index": 2000,
          }}
          onClick={() => setExitPrompt(false)}
        >
          <div
            onClick={(ev) => ev.stopPropagation()}
            style={{
              "min-width": "320px",
              padding: "22px 24px",
              "border-radius": "14px",
              background: "#1b1d28",
              border: "1px solid rgba(255,255,255,0.12)",
              "box-shadow": "0 20px 60px rgba(0,0,0,0.5)",
              "text-align": "center",
            }}
          >
            <div style={{ "font-size": "15px", "font-weight": 600, "margin-bottom": "6px" }}>
              Unsaved take
            </div>
            <div style={{ "font-size": "12px", color: "#aeb2c2", "margin-bottom": "18px" }}>
              Save this recording before leaving?
            </div>
            <div style={{ display: "flex", gap: "10px", "justify-content": "center" }}>
              <button
                onClick={() => void exitWithSave()}
                style={{
                  padding: "8px 16px",
                  "border-radius": "8px",
                  border: "1px solid rgba(91,231,196,0.4)",
                  background: "rgba(91,231,196,0.14)",
                  color: "#5be7c4",
                  "font-family": DISP,
                  "font-size": "12px",
                  cursor: "pointer",
                }}
              >
                Save (s)
              </button>
              <button
                onClick={() => void exitDiscard()}
                style={{
                  padding: "8px 16px",
                  "border-radius": "8px",
                  border: "1px solid rgba(255,77,87,0.4)",
                  background: "rgba(255,77,87,0.12)",
                  color: "#ff7079",
                  "font-family": DISP,
                  "font-size": "12px",
                  cursor: "pointer",
                }}
              >
                Discard (d)
              </button>
              <button
                onClick={() => setExitPrompt(false)}
                style={{
                  padding: "8px 16px",
                  "border-radius": "8px",
                  border: "1px solid rgba(255,255,255,0.14)",
                  background: "rgba(255,255,255,0.05)",
                  color: "#dfe2ea",
                  "font-family": DISP,
                  "font-size": "12px",
                  cursor: "pointer",
                }}
              >
                Cancel (Esc)
              </button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}
