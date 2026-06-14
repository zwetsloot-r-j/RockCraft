// RecordControls.tsx — the unified screen's Record ⇄ Edit toggle (M9-A).
//
// The capture+edit screen is one surface with two modes (the core `InputMode`):
// editing = DirectEdit, recording = StepRecord / LiveRecord. This floating
// control makes the mode switch discoverable with the mouse and surfaces the
// recording-mode feedback the standalone record screen used to provide: a red
// REC indicator and a live input-level meter. It dispatches no edit logic — the
// buttons call back into `toggle_record_arm` / `toggle_record_flavour`, exactly
// what the `R` / `t` keys do.

import { For, Show, type JSX } from "solid-js";
import type { InputMode } from "../../ipc/types";

const FONT_MONO = "'IBM Plex Mono', ui-monospace, monospace";

export interface RecordControlsProps {
  /** Current input mode (DirectEdit = editing; Step/Live = recording). */
  inputMode: InputMode;
  /** Live input activity level (0..1) for the meter. */
  level: number;
  /** Toggle armed ⇄ disarmed (`R`). */
  onToggleArm: () => void;
  /** Toggle step ⇄ live record (`t`); inert while disarmed. */
  onToggleFlavour: () => void;
}

/** A small vertical-bar level meter, lit proportionally to `level` (0..1). */
function LevelMeter(props: { level: number }): JSX.Element {
  const bars = 12;
  return (
    <div
      style={{
        display: "flex",
        "align-items": "flex-end",
        gap: "1.5px",
        height: "16px",
        width: "48px",
      }}
      title="Input level"
    >
      <For each={Array.from({ length: bars }, (_, i) => i)}>
        {(i) => {
          const on = (): boolean => props.level * bars > i;
          const c = i / bars > 0.78 ? "#ff5d6c" : i / bars > 0.6 ? "#ffd166" : "#5be7c4";
          return (
            <span
              style={{
                flex: 1,
                height: `${30 + (i / bars) * 70}%`,
                "border-radius": "1px",
                background: on() ? c : "rgba(255,255,255,0.09)",
                transition: "background .08s",
              }}
            />
          );
        }}
      </For>
    </div>
  );
}

export function RecordControls(props: RecordControlsProps): JSX.Element {
  const recording = (): boolean => props.inputMode !== "DirectEdit";
  const flavour = (): string =>
    props.inputMode === "LiveRecord" ? "LIVE" : "STEP";

  return (
    <div
      style={{
        position: "absolute",
        top: "12px",
        right: "16px",
        display: "flex",
        "align-items": "center",
        gap: "10px",
        background: "rgba(26,27,36,0.9)",
        border: `1px solid ${recording() ? "#ff5d6c" : "rgba(255,255,255,0.14)"}`,
        "border-radius": "8px",
        padding: "6px 10px",
        "z-index": 90,
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      {/* Record ⇄ Edit toggle button — the primary, discoverable control. */}
      <button
        onClick={props.onToggleArm}
        title={
          recording()
            ? "Switch to editing (R)"
            : "Switch to recording (R)"
        }
        style={{
          display: "flex",
          "align-items": "center",
          gap: "6px",
          padding: "4px 10px",
          "border-radius": "6px",
          border: "none",
          cursor: "pointer",
          background: recording() ? "#ff5d6c" : "#d96bff",
          color: recording() ? "#fff" : "#0f1016",
          "font-size": "11px",
          "font-weight": 700,
          "letter-spacing": "0.5px",
        }}
      >
        {/* Record dot — solid red while armed, hollow while editing. */}
        <span
          style={{
            width: "8px",
            height: "8px",
            "border-radius": "50%",
            background: recording() ? "#fff" : "transparent",
            border: recording() ? "none" : "2px solid #0f1016",
            "box-shadow": recording() ? "0 0 6px #fff" : "none",
          }}
        />
        {recording() ? "REC" : "EDIT"}
      </button>

      {/* Step / Live flavour toggle — only meaningful while armed. */}
      <Show when={recording()}>
        <button
          onClick={props.onToggleFlavour}
          title="Toggle step / live record (t)"
          style={{
            padding: "4px 8px",
            "border-radius": "6px",
            border: "1px solid rgba(255,255,255,0.16)",
            background: "rgba(255,255,255,0.05)",
            color: "#e7e8ef",
            cursor: "pointer",
            "font-family": FONT_MONO,
            "font-size": "11px",
          }}
        >
          {flavour()}
        </button>
        <LevelMeter level={props.level} />
      </Show>
    </div>
  );
}
