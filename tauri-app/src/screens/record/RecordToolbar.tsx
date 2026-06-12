// RecordToolbar.tsx — the 58 px bottom transport + edit bar (Design E, from
// B · Console with C's notation-view pickers). Transport + Trim/Delete/Quantize/
// Punch-in/Undo/Redo + CLEF/SPELL/SNAP segmented controls.
//
// #169 wiring:
// - Record (●) starts/stops the recording session.
// - Stop (■) stops recording and discards (same as record toggle).
// - Save shortcut is "s" in RecordScreen; no toolbar button yet.
// - Trim / Quantize / Punch-in render disabled (no core action yet).
// - Delete / Undo / Redo remain visual-only here.

import { type Accessor, type JSX } from "solid-js";
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
  /** Whether a recording session is currently active. */
  recording: Accessor<boolean>;
  /** Called when the record button is pressed (starts session). */
  onRecord: () => void;
  /** Called when the stop button is pressed (ends session). */
  onStop: () => void;
  /** Called when the save button is pressed. */
  onSave: () => void;
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
          title="Stop recording"
          onClick={props.onStop}
        />
        {/* Record toggle: lit red when active */}
        <RoundBtn
          icon="record"
          fill="fill"
          color={props.recording() ? REC : "#dfe2ea"}
          bg={props.recording() ? "rgba(255,77,87,0.20)" : "rgba(255,255,255,0.06)"}
          glow={props.recording()}
          size={38}
          title={props.recording() ? "Stop recording" : "Start recording"}
          onClick={props.recording() ? props.onStop : props.onRecord}
          active={props.recording()}
        />
        <RoundBtn icon="play" fill="fill" size={32} title="Play take" />
        <RoundBtn icon="loop" size={32} title="Loop" />
      </div>
      <VRule />
      <div style={{ display: "flex", "align-items": "center", gap: "2px" }}>
        {/* Disabled: no core action yet */}
        <ToolBtn icon="scissors" label="Trim" disabled title="Trim — not yet wired" />
        <ToolBtn icon="trash" label="Delete" danger />
        {/* Disabled: no core action yet */}
        <ToolBtn icon="magnet" label="Quantize" disabled title="Quantize — not yet wired" />
        {/* Disabled: no core action yet */}
        <ToolBtn icon="target" label="Punch-in" disabled title="Punch-in — not yet wired" />
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
