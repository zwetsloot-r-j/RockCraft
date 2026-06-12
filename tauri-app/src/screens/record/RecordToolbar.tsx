// RecordToolbar.tsx — the 58 px bottom transport + edit bar (Design E, from
// B · Console with C's notation-view pickers). Transport + Trim/Delete/Quantize/
// Punch-in/Undo/Redo + SNAP segmented control.
//
// Controls audit (#188): only the controls backed by real behaviour are
// interactive; everything without a core action is rendered disabled with a
// "not yet wired" tooltip rather than silently no-op-ing.
// - Record (●) / Stop (■) start/stop the live session (real).
// - Rewind / Play take / Loop have no take-playback path yet → disabled.
// - Trim / Delete / Quantize / Punch-in / Undo / Redo have no core action → disabled.
// - SNAP is disabled until quantize exists (it drives no quantization today).
// - The redundant CLEF / SPELL pickers were removed: they changed no render.

import { type Accessor, type JSX } from "solid-js";
import { RoundBtn } from "./ui/RoundBtn";
import { Seg } from "./ui/Seg";
import { REC } from "./ui/theme";
import { ToolBtn } from "./ui/ToolBtn";
import { VRule } from "./ui/VRule";

export const SNAP_OPTIONS = ["1/8", "1/16", "1/32"] as const;

export type Snap = (typeof SNAP_OPTIONS)[number];

export interface RecordToolbarProps {
  snap: Accessor<Snap>;
  onSnap: (v: Snap) => void;
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
        {/* Disabled: no take-playback transport yet */}
        <RoundBtn icon="rewind" fill="fill" size={32} disabled title="Rewind — not yet wired" />
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
        {/* Disabled: no take-playback transport yet */}
        <RoundBtn icon="play" fill="fill" size={32} disabled title="Play take — not yet wired" />
        <RoundBtn icon="loop" size={32} disabled title="Loop — not yet wired" />
      </div>
      <VRule />
      <div style={{ display: "flex", "align-items": "center", gap: "2px" }}>
        {/* All edit ops are disabled: no backing core action yet (#188). */}
        <ToolBtn icon="scissors" label="Trim" disabled title="Trim — not yet wired" />
        <ToolBtn icon="trash" label="Delete" danger disabled title="Delete — not yet wired" />
        <ToolBtn icon="magnet" label="Quantize" disabled title="Quantize — not yet wired" />
        <ToolBtn icon="target" label="Punch-in" disabled title="Punch-in — not yet wired" />
        <ToolBtn icon="undo" label="Undo" disabled title="Undo — not yet wired" />
        <ToolBtn icon="redo" label="Redo" disabled title="Redo — not yet wired" />
      </div>
      <div style={{ flex: 1 }} />
      {/* Disabled until a quantize action exists (snap drives nothing today). */}
      <Seg
        label="SNAP"
        options={SNAP_OPTIONS}
        value={props.snap()}
        onChange={props.onSnap}
        disabled
        title="Snap — not yet wired (needs quantize)"
      />
    </div>
  );
}
