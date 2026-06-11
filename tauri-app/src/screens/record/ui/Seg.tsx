// Seg.tsx — segmented control (CLEF / SPELL / SNAP). Generic over the option
// union so callers stay fully typed.

import { For, type JSX, Show } from "solid-js";
import { MONO } from "./theme";

export interface SegProps<T extends string> {
  options: readonly T[];
  value: T;
  onChange: (value: T) => void;
  label?: string;
}

export function Seg<T extends string>(props: SegProps<T>): JSX.Element {
  return (
    <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
      <Show when={props.label}>
        <span
          style={{
            "font-size": "9px",
            "letter-spacing": "1.5px",
            color: "#7c8094",
            "font-family": MONO,
          }}
        >
          {props.label}
        </span>
      </Show>
      <div
        style={{
          display: "flex",
          background: "rgba(0,0,0,0.3)",
          "border-radius": "7px",
          padding: "2px",
          gap: "2px",
        }}
      >
        <For each={props.options}>
          {(o) => (
            <button
              onClick={() => props.onChange(o)}
              style={{
                border: "none",
                cursor: "pointer",
                padding: "3px 9px",
                "border-radius": "5px",
                "font-family": MONO,
                "font-size": "11px",
                background: props.value === o ? "rgba(255,255,255,0.12)" : "transparent",
                color: props.value === o ? "#fff" : "#888c9c",
              }}
            >
              {o}
            </button>
          )}
        </For>
      </div>
    </div>
  );
}
