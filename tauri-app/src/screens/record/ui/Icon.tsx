// Icon.tsx — the inline SVG icon set, ported from record-ui.jsx (`P` + `Icon`).
// `d` may be a known icon key or a raw SVG path string.

import type { JSX } from "solid-js";

export const P: Record<string, string> = {
  rewind: "M11 5 4 12l7 7zM20 5l-7 7 7 7z",
  forward: "M13 5l7 7-7 7zM4 5l7 7-7 7z",
  play: "M7 4l13 8-13 8z",
  stop: "M6 6h12v12H6z",
  loop: "M17 1l4 4-4 4M3 11V9a4 4 0 014-4h14M7 23l-4-4 4-4M21 13v2a4 4 0 01-4 4H3",
  undo: "M3 8h11a6 6 0 010 12h-3M3 8l4-4M3 8l4 4",
  redo: "M21 8H10a6 6 0 000 12h3M21 8l-4-4M21 8l-4 4",
  scissors:
    "M6 6l12 12M8.5 8.5L18 6M6 18l4-4M9 6a3 3 0 11-6 0 3 3 0 016 0zM9 18a3 3 0 11-6 0 3 3 0 016 0z",
  trash: "M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14M10 10v6M14 10v6",
  grid: "M4 4h16v16H4zM4 9.3h16M4 14.6h16M9.3 4v16M14.6 4v16",
  magnet: "M6 3v8a6 6 0 0012 0V3h-4v8a2 2 0 01-4 0V3zM6 3H2m12 0h4",
  metro: "M12 3l5.5 17H6.5zM6.5 20h11M12 19l4.5-9",
  target: "M12 2a10 10 0 100 20 10 10 0 000-20zM12 7a5 5 0 100 10 5 5 0 000-10zM12 11a1 1 0 100 2 1 1 0 000-2z",
  pencil: "M4 20l1-4L16 5l3 3L8 19zM14 7l3 3",
  midi: "M5 9a7 7 0 0114 0v3H5zM9 9V6m3 3V5m3 4V6M12 19v3",
  chevDown: "M6 9l6 6 6-6",
  chevUp: "M6 15l6-6 6 6",
  mic: "M12 3a3 3 0 00-3 3v6a3 3 0 006 0V6a3 3 0 00-3-3zM5 11a7 7 0 0014 0M12 18v3",
  cursor: "M5 3l14 7-6 2-2 6z",
  plus: "M12 5v14M5 12h14",
  minus: "M5 12h14",
};

export interface IconProps {
  d: string;
  size?: number;
  fill?: string;
  stroke?: string;
  w?: number;
  style?: JSX.CSSProperties;
}

export function Icon(props: IconProps): JSX.Element {
  const size = (): number => props.size ?? 16;
  const fill = (): string => props.fill ?? "none";
  const stroke = (): string => props.stroke ?? "currentColor";
  const w = (): number => props.w ?? 1.8;
  return (
    <svg
      width={size()}
      height={size()}
      viewBox="0 0 24 24"
      fill={fill()}
      stroke={fill() === "none" ? stroke() : "none"}
      stroke-width={w()}
      stroke-linejoin="round"
      stroke-linecap="round"
      style={{ display: "block", flex: "0 0 auto", ...props.style }}
    >
      <path d={P[props.d] ?? props.d} />
    </svg>
  );
}
