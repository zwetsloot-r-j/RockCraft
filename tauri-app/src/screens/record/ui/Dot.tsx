// Dot.tsx — a small status dot (e.g. the green MIDI-connected indicator).

import type { JSX } from "solid-js";

export interface DotProps {
  c: string;
  s?: number;
  glow?: boolean;
}

export function Dot(props: DotProps): JSX.Element {
  const s = (): number => props.s ?? 9;
  const glow = (): boolean => props.glow ?? true;
  return (
    <span
      style={{
        width: `${s()}px`,
        height: `${s()}px`,
        "border-radius": "99px",
        background: props.c,
        display: "inline-block",
        "box-shadow": glow() ? `0 0 8px ${props.c}aa` : "none",
      }}
    />
  );
}
