import type { JSX } from "solid-js";
import type { Screen } from "./screens";

interface Props {
  screen: Screen;
}

export function Placeholder(props: Props): JSX.Element {
  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        "flex-direction": "column",
        "align-items": "center",
        "justify-content": "center",
        background: "#0f1016",
        color: "#8a8e9c",
        gap: "12px",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      <div
        style={{
          "font-size": "20px",
          "font-weight": 600,
          color: "#e7e8ef",
          "text-transform": "capitalize",
        }}
      >
        {props.screen.kind}
      </div>
      <div style={{ "font-size": "14px" }}>not ported yet</div>
      <div
        style={{
          "font-size": "12px",
          "margin-top": "8px",
          padding: "4px 10px",
          border: "1px solid rgba(255,255,255,0.1)",
          "border-radius": "6px",
        }}
      >
        Esc to menu
      </div>
    </div>
  );
}
