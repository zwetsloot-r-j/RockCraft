// Meter.tsx — vertical level meter (input loudness, driven by engine.level).

import { For, type JSX } from "solid-js";
import { OK, REC } from "./theme";

export interface MeterProps {
  level: number;
  bars?: number;
  w?: number;
}

export function Meter(props: MeterProps): JSX.Element {
  const bars = (): number => props.bars ?? 14;
  const w = (): number => props.w ?? 56;
  return (
    <div
      style={{
        display: "flex",
        "align-items": "flex-end",
        gap: "1.5px",
        height: "18px",
        width: `${w()}px`,
      }}
    >
      <For each={Array.from({ length: bars() }, (_, i) => i)}>
        {(i) => {
          const on = (): boolean => props.level * bars() > i;
          const c = i / bars() > 0.78 ? REC : i / bars() > 0.6 ? "#ffd166" : OK;
          return (
            <span
              style={{
                flex: 1,
                height: `${30 + (i / bars()) * 70}%`,
                "border-radius": "1px",
                background: on() ? c : "rgba(255,255,255,0.09)",
                "box-shadow": on() ? `0 0 5px ${c}88` : "none",
                transition: "background .08s",
              }}
            />
          );
        }}
      </For>
    </div>
  );
}
