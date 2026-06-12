// NoteInspector.tsx — the floating selected-note panel (Design E, from
// C · Score). Appears top-right of the canvas whenever the engine has a
// selected note. Nudge / Snap have no backing core action yet (#188), so they
// render disabled rather than silently no-op-ing.

import { type Accessor, createMemo, For, type JSX, Show } from "solid-js";
import type { RecordCanvas } from "./RecordCanvas";
import { bbOf } from "./format";
import { EMPTY_TAKE } from "./emptyTake";
import { noteName, spectrumHue } from "./utils";
import { DISP, MONO, OK } from "./ui/theme";

export interface NoteInspectorProps {
  eng: Accessor<RecordCanvas | null>;
  frame: Accessor<number>;
}

export function NoteInspector(props: NoteInspectorProps): JSX.Element {
  const sel = createMemo(() => {
    props.frame(); // track the throttled bump so the panel appears once mounted
    return props.eng()?.sel ?? null;
  });

  return (
    <Show when={sel()}>
      {(note) => {
        const hue = (): number => spectrumHue(note().note);
        const rows = (): [string, string | number][] => [
          ["Beat", bbOf(note().start, EMPTY_TAKE)],
          ["Length", "♩ quarter"],
          ["Velocity", note().vel],
        ];
        return (
          <div
            style={{
              position: "absolute",
              top: "12px",
              right: "12px",
              width: "184px",
              padding: "11px 13px",
              "border-radius": "11px",
              background: "rgba(20,22,31,0.9)",
              "backdrop-filter": "blur(12px)",
              border: "1px solid rgba(255,255,255,0.09)",
              "box-shadow": "0 10px 30px rgba(0,0,0,0.4)",
            }}
          >
            <div
              style={{
                "font-size": "9px",
                "letter-spacing": "2px",
                color: "#7c8094",
                "font-family": MONO,
                "margin-bottom": "8px",
              }}
            >
              SELECTED NOTE
            </div>
            <div
              style={{
                display: "flex",
                "align-items": "center",
                gap: "9px",
                "margin-bottom": "9px",
              }}
            >
              <span
                style={{
                  width: "16px",
                  height: "12px",
                  "border-radius": "99px",
                  background: `oklch(0.74 0.16 ${hue()})`,
                  "box-shadow": `0 0 10px oklch(0.74 0.16 ${hue()})`,
                  transform: "rotate(-20deg)",
                }}
              />
              <span style={{ "font-size": "22px", "font-weight": 700, "font-family": DISP }}>
                {noteName(note().note)}
              </span>
            </div>
            <For each={rows()}>
              {([k, v]) => (
                <div
                  style={{
                    display: "flex",
                    "justify-content": "space-between",
                    "font-size": "11.5px",
                    padding: "3px 0",
                    color: "#aeb2c2",
                  }}
                >
                  <span style={{ color: "#7c8094" }}>{k}</span>
                  <span style={{ "font-family": MONO }}>{v}</span>
                </div>
              )}
            </For>
            <div style={{ display: "flex", gap: "6px", "margin-top": "10px" }}>
              <button
                disabled
                title="Nudge — not yet wired"
                style={{
                  flex: 1,
                  border: "1px solid rgba(255,255,255,0.12)",
                  background: "rgba(255,255,255,0.05)",
                  color: "#dfe2ea",
                  "border-radius": "7px",
                  padding: "6px 0",
                  "font-size": "11px",
                  "font-family": DISP,
                  cursor: "default",
                  opacity: "0.4",
                }}
              >
                Nudge
              </button>
              <button
                disabled
                title="Snap — not yet wired"
                style={{
                  flex: 1,
                  border: `1px solid ${OK}44`,
                  background: `${OK}1a`,
                  color: OK,
                  "border-radius": "7px",
                  padding: "6px 0",
                  "font-size": "11px",
                  "font-family": DISP,
                  cursor: "default",
                  opacity: "0.4",
                }}
              >
                Snap
              </button>
            </div>
          </div>
        );
      }}
    </Show>
  );
}
