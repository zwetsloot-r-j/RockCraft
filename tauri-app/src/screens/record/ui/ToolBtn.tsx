// ToolBtn.tsx — square icon + label toolbar button (Trim / Delete / …).

import { type JSX, Show } from "solid-js";
import { Icon } from "./Icon";
import { MONO, OK, REC } from "./theme";

export interface ToolBtnProps {
  icon: string;
  label?: string;
  active?: boolean;
  danger?: boolean;
  onClick?: () => void;
}

export function ToolBtn(props: ToolBtnProps): JSX.Element {
  const col = (): string => (props.danger ? REC : props.active ? OK : "#c7cad6");
  return (
    <button
      onClick={() => props.onClick?.()}
      title={props.label}
      style={{
        display: "flex",
        "flex-direction": "column",
        "align-items": "center",
        gap: "3px",
        padding: "6px 9px",
        border: props.active ? `1px solid ${col()}55` : "1px solid transparent",
        "border-radius": "8px",
        background: props.active ? `${col()}1a` : "transparent",
        color: col(),
        cursor: "pointer",
        "min-width": "46px",
      }}
    >
      <Icon d={props.icon} size={17} stroke={col()} />
      <Show when={props.label}>
        <span
          style={{
            "font-size": "9px",
            "letter-spacing": "0.3px",
            "font-family": MONO,
            color: props.active ? col() : "#8a8e9c",
          }}
        >
          {props.label}
        </span>
      </Show>
    </button>
  );
}
