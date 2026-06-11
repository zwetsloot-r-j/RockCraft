// Chip.tsx — small status chip (bar:beat, chord, MIDI device, take number).

import type { JSX } from "solid-js";
import { DISP, MONO } from "./theme";

export interface ChipProps {
  c?: string;
  bg?: string;
  mono?: boolean;
  children?: JSX.Element;
}

export function Chip(props: ChipProps): JSX.Element {
  const c = (): string => props.c ?? "#aeb2c2";
  const bg = (): string => props.bg ?? "rgba(255,255,255,0.06)";
  const mono = (): boolean => props.mono ?? true;
  return (
    <span
      style={{
        "font-size": "10.5px",
        "font-family": mono() ? MONO : DISP,
        padding: "3px 8px",
        "border-radius": "6px",
        background: bg(),
        color: c(),
        display: "inline-flex",
        "align-items": "center",
        gap: "6px",
        "white-space": "nowrap",
      }}
    >
      {props.children}
    </span>
  );
}
