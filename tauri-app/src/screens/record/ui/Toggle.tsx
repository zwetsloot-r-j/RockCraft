// Toggle.tsx — dark toggle pill (metronome / count-in). Light readable text on
// a dark fill; green only as an accent (border + indicator) when active.

import type { JSX } from "solid-js";
import { MONO } from "./theme";

export interface ToggleProps {
  active?: boolean;
  onClick?: () => void;
  title?: string;
  children?: JSX.Element;
}

export function Toggle(props: ToggleProps): JSX.Element {
  return (
    <button
      onClick={() => props.onClick?.()}
      title={props.title}
      style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "6px",
        cursor: "pointer",
        "font-family": MONO,
        "font-size": "10.5px",
        padding: "4px 9px",
        "border-radius": "6px",
        background: "rgba(0,0,0,0.24)",
        border: `1px solid ${props.active ? "rgba(91,231,196,0.5)" : "rgba(255,255,255,0.09)"}`,
        color: props.active ? "#e7e8ef" : "#6e7282",
        transition: "border-color .12s, color .12s",
      }}
    >
      {props.children}
    </button>
  );
}
