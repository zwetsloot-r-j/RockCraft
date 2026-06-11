// RecordHeader.tsx — the 56 px top status bar (Design E, from B · Console).
// Reads live clock/level off the engine; re-evaluates whenever the throttled
// `frame` signal bumps (the engine itself is framework-free, per CONVENTIONS).

import { type Accessor, createMemo, type JSX } from "solid-js";
import type { RecordCanvas } from "./RecordCanvas";
import type { Song } from "./types";
import { tc } from "./format";
import { Chip } from "./ui/Chip";
import { Dot } from "./ui/Dot";
import { Icon } from "./ui/Icon";
import { Meter } from "./ui/Meter";
import { RecDot } from "./ui/RecDot";
import { MONO, OK } from "./ui/theme";
import { Toggle } from "./ui/Toggle";
import { VRule } from "./ui/VRule";

const DEVICE = "Casio PX-S3100";

export interface RecordHeaderProps {
  eng: Accessor<RecordCanvas | null>;
  frame: Accessor<number>;
  song: Song;
  metro: Accessor<boolean>;
  onMetro: (v: boolean) => void;
  count: Accessor<boolean>;
  onCount: (v: boolean) => void;
}

export function RecordHeader(props: RecordHeaderProps): JSX.Element {
  const clock = createMemo(() => {
    props.frame(); // track the throttled bump
    const e = props.eng();
    const now = e ? e.now : 0;
    const bar = Math.floor(now / props.song.BAR);
    const beat = Math.floor((now % props.song.BAR) / props.song.BEAT);
    return {
      now,
      bar,
      beat,
      chord: props.song.chords[bar] ?? "—",
      level: e ? e.level : 0,
    };
  });

  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        gap: "12px",
        padding: "0 16px",
        height: "56px",
        background: "#181a24",
        "border-bottom": "1px solid rgba(255,255,255,0.06)",
        flex: "0 0 auto",
      }}
    >
      {/* logo badge — two spectrum-colored bars */}
      <div
        style={{
          width: "26px",
          height: "26px",
          "border-radius": "6px",
          background: "linear-gradient(150deg,#2a2c36,#15161d)",
          border: "1px solid rgba(255,255,255,0.08)",
          display: "flex",
          "align-items": "center",
          "justify-content": "center",
          gap: "2px",
          flex: "0 0 auto",
        }}
      >
        <span
          style={{ width: "3px", height: "8px", background: "oklch(0.72 0.16 150)", "border-radius": "2px" }}
        />
        <span
          style={{ width: "3px", height: "13px", background: "oklch(0.72 0.16 30)", "border-radius": "2px" }}
        />
      </div>
      <div>
        <div style={{ "font-size": "14px", "font-weight": 600, "letter-spacing": "-0.2px" }}>
          {props.song.title} <span style={{ color: "#7c8094", "font-weight": 400 }}>· Take 03</span>
        </div>
        <div
          style={{ "font-size": "10px", color: "#7c8094", "font-family": MONO, "margin-top": "1px" }}
        >
          {props.song.key} · {props.song.tempoBpm} BPM · {props.song.timeSig}
        </div>
      </div>
      <VRule />
      <RecDot />
      <div
        style={{
          "font-family": MONO,
          "font-size": "16px",
          "font-weight": 500,
          "font-variant-numeric": "tabular-nums",
        }}
      >
        {tc(clock().now)}
      </div>
      <Chip mono>
        BAR {clock().bar + 1}.{clock().beat + 1}
      </Chip>
      <Chip c="oklch(0.82 0.13 150)">{clock().chord}</Chip>
      <div style={{ flex: 1 }} />
      <Toggle active={props.metro()} onClick={() => props.onMetro(!props.metro())} title="Metronome">
        <Icon d="metro" size={13} stroke={props.metro() ? OK : "#6e7282"} /> {props.song.tempoBpm}
      </Toggle>
      <Toggle active={props.count()} onClick={() => props.onCount(!props.count())} title="Count-in">
        COUNT 1
      </Toggle>
      <Chip c="#cfd3df" bg="rgba(0,0,0,0.24)">
        <Dot c={OK} s={6} /> MIDI · {DEVICE}
      </Chip>
      <Meter level={clock().level} />
    </div>
  );
}
