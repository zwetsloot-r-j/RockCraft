// HighwayHeader.tsx — the 64 px Spectrum Live header bar. Faithful port of the
// FusionProto header from design/note_highway/rockcraft-proto/prototypes.jsx.
//
// Reads the engine accessor for score/combo each time frame() bumps (the ~9 fps
// throttle signal drives the updates); derives bar/beat/chord from
// performance.now() - eng.t0 modulo the loaded song's LOOP.

import { For, Show } from "solid-js";
import type { HighwayCanvas } from "./HighwayCanvas";
import type { PlayStateEvent } from "../../ipc/types";
import type { SongData } from "./types";

interface HighwayHeaderProps {
  eng: () => HighwayCanvas | null;
  frame: () => number;
  song: SongData;
  /** Live mode (#168): read score/combo/clock from `playState`, not the eng. */
  live?: boolean;
  playState?: () => PlayStateEvent | null;
  hearSong?: () => boolean;
  waitMode?: () => boolean;
  monitor?: () => boolean;
}

// 12-dot pitch-class color wheel.
const WHEEL = Array.from(
  { length: 12 },
  (_, i) => `oklch(0.72 0.16 ${(i * 30 + 8) % 360})`,
);

const monoFont = "'IBM Plex Mono', monospace";

export function HighwayHeader(props: HighwayHeaderProps) {
  // Derived reads — each touches props.frame() so the throttle signal drives them.
  // In live mode (#168) the song time and score/combo come from the backend
  // `play_state` (the real clock + scoring), not the demo engine's mock fields.
  const nowMs = (): number => {
    if (props.live) return (props.playState?.()?.time_us ?? 0) / 1000;
    const eng = props.eng();
    const elapsed = eng ? performance.now() - eng.t0 : 0;
    return elapsed % props.song.LOOP;
  };
  const clock = () => {
    props.frame();
    const now = nowMs();
    const bar = Math.floor(now / props.song.BAR);
    const beat = Math.floor((now % props.song.BAR) / props.song.BEAT);
    return { bar, beat, chord: props.song.chords[bar] };
  };
  const combo = () => {
    props.frame();
    if (props.live) return props.playState?.()?.combo ?? 0;
    return props.eng()?.combo ?? 0;
  };
  const score = () => {
    props.frame();
    if (props.live) return props.playState?.()?.score ?? 0;
    return props.eng()?.score ?? 0;
  };
  const mult = () => 1 + Math.floor(combo() / 8);

  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        gap: "18px",
        padding: "0 18px",
        height: "64px",
        background: "#181922",
        "border-bottom": "1px solid rgba(255,255,255,0.07)",
        color: "#e7e8ef",
        flex: "0 0 auto",
      }}
    >
      <div style={{ display: "flex", "align-items": "center", gap: "10px" }}>
        <div style={{ "font-size": "15px", "font-weight": 600 }}>{props.song.title}</div>
        <span
          style={{
            "font-size": "11px",
            "font-family": monoFont,
            padding: "2px 8px",
            "border-radius": "6px",
            background: "rgba(255,255,255,0.07)",
            color: "#b9bccb",
          }}
        >
          {props.song.key}
        </span>
        <div style={{ display: "flex", gap: "2px", "margin-left": "2px" }}>
          <For each={WHEEL}>
            {(cc) => (
              <span
                style={{
                  width: "7px",
                  height: "7px",
                  "border-radius": "2px",
                  background: cc,
                }}
              />
            )}
          </For>
        </div>
      </div>

      <Show when={props.live}>
        <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
          <span
            style={{
              "font-size": "11px",
              "font-family": monoFont,
              padding: "2px 8px",
              "border-radius": "6px",
              background: props.hearSong?.()
                ? "rgba(103,227,196,0.2)"
                : "rgba(255,255,255,0.05)",
              color: props.hearSong?.() ? "#67e3c4" : "#7c7f8e",
            }}
            title="m — hear the song"
          >
            ♪ m
          </span>
          <span
            style={{
              "font-size": "11px",
              "font-family": monoFont,
              padding: "2px 8px",
              "border-radius": "6px",
              background: props.waitMode?.()
                ? "rgba(255,209,102,0.2)"
                : "rgba(255,255,255,0.05)",
              color: props.waitMode?.() ? "#ffd166" : "#7c7f8e",
            }}
            title="w — wait mode"
          >
            ⏸ w
          </span>
          <span
            style={{
              "font-size": "11px",
              "font-family": monoFont,
              padding: "2px 8px",
              "border-radius": "6px",
              background: props.monitor?.()
                ? "rgba(143,182,255,0.2)"
                : "rgba(255,255,255,0.05)",
              color: props.monitor?.() ? "#8fb6ff" : "#7c7f8e",
            }}
            title="n — play my notes (input monitor)"
          >
            🎹 n
          </span>
        </div>
      </Show>

      <div style={{ flex: 1 }} />

      <div style={{ "text-align": "center" }}>
        <div
          style={{
            "font-size": "22px",
            "font-weight": 600,
            "font-family": monoFont,
            "line-height": 1,
            "font-variant-numeric": "tabular-nums",
          }}
        >
          {clock().bar + 1}
          <span style={{ color: "#6f7280", opacity: 0.5 }}>:</span>
          {clock().beat + 1}
        </div>
        <div
          style={{
            "font-size": "8.5px",
            "letter-spacing": "2px",
            color: "#7c7f8e",
            "margin-top": "3px",
          }}
        >
          BAR · BEAT
        </div>
      </div>

      <div style={{ "text-align": "center", "min-width": "50px" }}>
        <div
          style={{
            "font-size": "22px",
            "font-weight": 700,
            "line-height": 1,
            color: "oklch(0.8 0.13 150)",
          }}
        >
          {clock().chord}
        </div>
        <div
          style={{
            "font-size": "8.5px",
            "letter-spacing": "2px",
            color: "#7c7f8e",
            "margin-top": "3px",
          }}
        >
          CHORD
        </div>
      </div>

      <div style={{ width: "1px", height: "30px", background: "rgba(255,255,255,0.08)" }} />

      <div style={{ "text-align": "center" }}>
        <div
          style={{
            "font-size": "20px",
            "font-weight": 700,
            "line-height": 1,
            color: "#67e3c4",
            "text-shadow": "0 0 12px #67e3c455",
          }}
        >
          ×{mult()}
        </div>
        <div
          style={{
            "font-size": "8.5px",
            "letter-spacing": "2px",
            color: "#5f9b8c",
            "margin-top": "3px",
          }}
        >
          {combo()} COMBO
        </div>
      </div>

      <div style={{ "text-align": "right", "min-width": "96px" }}>
        <div
          style={{
            "font-size": "24px",
            "font-weight": 700,
            "line-height": 1,
            "font-family": monoFont,
            color: "#fff",
            "font-variant-numeric": "tabular-nums",
          }}
        >
          {String(score()).padStart(6, "0")}
        </div>
        <div
          style={{
            "font-size": "8.5px",
            "letter-spacing": "2px",
            color: "#7c7f8e",
            "margin-top": "2px",
          }}
        >
          SCORE
        </div>
      </div>
    </div>
  );
}
