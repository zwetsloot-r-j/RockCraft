// RecDot.tsx — the animated recording orb + REC / PAUSED label.

import type { JSX } from "solid-js";
import { MONO, REC } from "./theme";

export interface RecDotProps {
  on?: boolean;
  paused?: boolean;
}

export function RecDot(props: RecDotProps): JSX.Element {
  const on = (): boolean => props.on ?? true;
  const tint = (): string => (props.paused ? "#ffb020" : REC);
  return (
    <span style={{ display: "inline-flex", "align-items": "center", gap: "7px" }}>
      <span style={{ position: "relative", width: "11px", height: "11px" }}>
        <span
          style={{
            position: "absolute",
            inset: 0,
            "border-radius": "99px",
            background: tint(),
            animation: on() && !props.paused ? "recpulse 1.1s ease-in-out infinite" : "none",
            "box-shadow": `0 0 10px ${tint()}`,
          }}
        />
      </span>
      <span
        style={{
          "font-size": "11px",
          "font-weight": 700,
          "letter-spacing": "2px",
          color: tint(),
          "font-family": MONO,
        }}
      >
        {props.paused ? "PAUSED" : "REC"}
      </span>
    </span>
  );
}
