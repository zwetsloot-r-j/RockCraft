// RoundBtn.tsx — round transport button (rewind / stop / play / loop).

import type { JSX } from "solid-js";
import { Icon } from "./Icon";

export interface RoundBtnProps {
  icon: string;
  fill?: "none" | "fill";
  bg?: string;
  color?: string;
  size?: number;
  glow?: boolean;
  active?: boolean;
  /** When true the button is rendered dimmed and non-interactive. */
  disabled?: boolean;
  onClick?: () => void;
  title?: string;
}

export function RoundBtn(props: RoundBtnProps): JSX.Element {
  const color = (): string => props.color ?? "#dfe2ea";
  const bg = (): string => props.bg ?? "rgba(255,255,255,0.06)";
  const size = (): number => props.size ?? 34;
  const fill = (): string => props.fill ?? "none";
  return (
    <button
      title={props.title}
      disabled={props.disabled}
      onClick={() => !props.disabled && props.onClick?.()}
      style={{
        width: `${size()}px`,
        height: `${size()}px`,
        "border-radius": "99px",
        border: props.active ? `1px solid ${color()}66` : "1px solid rgba(255,255,255,0.08)",
        background: bg(),
        color: color(),
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        cursor: props.disabled ? "default" : "pointer",
        "box-shadow": props.glow ? `0 2px 14px ${color()}55` : "none",
        flex: "0 0 auto",
        opacity: props.disabled ? "0.38" : "1",
        transition: "transform .1s",
      }}
    >
      <Icon
        d={props.icon}
        fill={fill() === "fill" ? color() : "none"}
        stroke={color()}
        size={size() * 0.46}
      />
    </button>
  );
}
