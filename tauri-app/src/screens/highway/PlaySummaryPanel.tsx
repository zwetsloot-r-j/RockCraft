// PlaySummaryPanel.tsx — end-of-take summary overlay (#168).
//
// Shows the totals from `core::stats` (hits / misses / extras / accuracy /
// best combo / score / timing breakdown). Enter replays the same bundle; Esc
// returns to the menu (handled by the shell router, but the button is offered
// for the mouse).

import type { JSX } from "solid-js";
import type { PlaySummary } from "../../ipc/types";

interface Props {
  summary: PlaySummary;
  onReplay: () => void;
  onMenu: () => void;
}

const FONT = "'Space Grotesk', system-ui, sans-serif";
const MONO = "'IBM Plex Mono', monospace";

function Stat(props: { label: string; value: string; color?: string }): JSX.Element {
  return (
    <div style={{ "text-align": "center", "min-width": "84px" }}>
      <div
        style={{
          "font-size": "26px",
          "font-weight": 700,
          "font-family": MONO,
          "line-height": 1,
          color: props.color ?? "#fff",
          "font-variant-numeric": "tabular-nums",
        }}
      >
        {props.value}
      </div>
      <div
        style={{
          "font-size": "9px",
          "letter-spacing": "2px",
          color: "#7c7f8e",
          "margin-top": "5px",
        }}
      >
        {props.label}
      </div>
    </div>
  );
}

export function PlaySummaryPanel(props: Props): JSX.Element {
  const accuracyPct = (): string => (props.summary.accuracy_bp / 100).toFixed(1);

  return (
    <div
      style={{
        position: "absolute",
        inset: "0",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        background: "rgba(8,9,13,0.82)",
        "backdrop-filter": "blur(3px)",
        "font-family": FONT,
        "z-index": "20",
      }}
    >
      <div
        style={{
          background: "#181922",
          border: "1px solid rgba(255,255,255,0.1)",
          "border-radius": "14px",
          padding: "28px 36px",
          display: "flex",
          "flex-direction": "column",
          gap: "22px",
          "box-shadow": "0 20px 60px rgba(0,0,0,0.5)",
          "min-width": "520px",
        }}
      >
        <div style={{ "text-align": "center" }}>
          <div style={{ "font-size": "22px", "font-weight": 700, color: "#fff" }}>
            Take complete
          </div>
          <div
            style={{
              "font-size": "48px",
              "font-weight": 700,
              "font-family": MONO,
              color: "#67e3c4",
              "margin-top": "8px",
              "font-variant-numeric": "tabular-nums",
            }}
          >
            {accuracyPct()}%
          </div>
          <div style={{ "font-size": "10px", "letter-spacing": "2px", color: "#5f9b8c" }}>
            ACCURACY
          </div>
        </div>

        <div
          style={{
            display: "flex",
            "justify-content": "center",
            gap: "10px",
            "flex-wrap": "wrap",
          }}
        >
          <Stat label="HITS" value={String(props.summary.hits)} color="#7ee08a" />
          <Stat label="MISSES" value={String(props.summary.misses)} color="#ff8089" />
          <Stat label="EXTRAS" value={String(props.summary.extras)} color="#b9bccb" />
          <Stat label="PERFECT" value={String(props.summary.perfect)} color="#5be7c4" />
          <Stat label="EARLY" value={String(props.summary.early)} color="#9fc3ff" />
          <Stat label="LATE" value={String(props.summary.late)} color="#ffd166" />
        </div>

        <div
          style={{
            display: "flex",
            "justify-content": "center",
            gap: "32px",
          }}
        >
          <Stat
            label="BEST COMBO"
            value={`×${props.summary.best_combo}`}
            color="#67e3c4"
          />
          <Stat
            label="SCORE"
            value={String(props.summary.score).padStart(6, "0")}
          />
        </div>

        <div
          style={{
            display: "flex",
            "justify-content": "center",
            gap: "12px",
            "margin-top": "4px",
          }}
        >
          <button
            type="button"
            onClick={() => props.onReplay()}
            style={{
              padding: "10px 22px",
              "border-radius": "8px",
              border: "none",
              background: "#4fa3e3",
              color: "#fff",
              "font-size": "14px",
              "font-weight": 600,
              cursor: "pointer",
              "font-family": FONT,
            }}
          >
            Replay (Enter)
          </button>
          <button
            type="button"
            onClick={() => props.onMenu()}
            style={{
              padding: "10px 22px",
              "border-radius": "8px",
              border: "1px solid rgba(255,255,255,0.15)",
              background: "transparent",
              color: "#e7e8ef",
              "font-size": "14px",
              "font-weight": 600,
              cursor: "pointer",
              "font-family": FONT,
            }}
          >
            Menu (Esc)
          </button>
        </div>
      </div>
    </div>
  );
}
