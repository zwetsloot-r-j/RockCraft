// RecordToolbar.tsx — the 58 px bottom transport + edit bar (Design E, from
// B · Console with C's notation-view pickers). Transport + Trim/Delete/Quantize/
// Punch-in/Undo/Redo + CLEF/SPELL/SNAP segmented controls. The edit tools and
// pickers are visual-only here; #169 wires the ones with a core action and
// disables the rest. SNAP options live in one constant so #169 can extend the
// list (core's Subdivision also has Quarter + triplets) without layout surgery.

import type { Accessor, JSX } from "solid-js";
import { RoundBtn } from "./ui/RoundBtn";
import { Seg } from "./ui/Seg";
import { REC } from "./ui/theme";
import { ToolBtn } from "./ui/ToolBtn";
import { VRule } from "./ui/VRule";

export const SNAP_OPTIONS = ["1/8", "1/16", "1/32"] as const;
export const CLEF_OPTIONS = ["Grand", "Treble"] as const;
export const SPELL_OPTIONS = ["♯", "♭"] as const;

export type Snap = (typeof SNAP_OPTIONS)[number];
export type Clef = (typeof CLEF_OPTIONS)[number];
export type Spelling = (typeof SPELL_OPTIONS)[number];

export interface RecordToolbarProps {
  snap: Accessor<Snap>;
  onSnap: (v: Snap) => void;
  clef: Accessor<Clef>;
  onClef: (v: Clef) => void;
  spelling: Accessor<Spelling>;
  onSpelling: (v: Spelling) => void;
}

export function RecordToolbar(props: RecordToolbarProps): JSX.Element {
  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        gap: "10px",
        padding: "0 14px",
        height: "58px",
        background: "#181a24",
        "border-top": "1px solid rgba(255,255,255,0.06)",
        flex: "0 0 auto",
      }}
    >
      <div style={{ display: "flex", "align-items": "center", gap: "6px" }}>
        <RoundBtn icon="rewind" fill="fill" size={32} title="Rewind" />
        <RoundBtn
          icon="stop"
          fill="fill"
          color={REC}
          bg="rgba(255,77,87,0.16)"
          glow
          size={38}
          title="Stop"
        />
        <RoundBtn icon="play" fill="fill" size={32} title="Play take" />
        <RoundBtn icon="loop" size={32} title="Loop" />
      </div>
      <VRule />
      <div style={{ display: "flex", "align-items": "center", gap: "2px" }}>
        <ToolBtn icon="scissors" label="Trim" />
        <ToolBtn icon="trash" label="Delete" danger />
        <ToolBtn icon="magnet" label="Quantize" />
        <ToolBtn icon="target" label="Punch-in" />
        <ToolBtn icon="undo" label="Undo" />
        <ToolBtn icon="redo" label="Redo" />
      </div>
      <div style={{ flex: 1 }} />
      <Seg label="CLEF" options={CLEF_OPTIONS} value={props.clef()} onChange={props.onClef} />
      <Seg
        label="SPELL"
        options={SPELL_OPTIONS}
        value={props.spelling()}
        onChange={props.onSpelling}
      />
      <VRule />
      <Seg label="SNAP" options={SNAP_OPTIONS} value={props.snap()} onChange={props.onSnap} />
    </div>
  );
}
