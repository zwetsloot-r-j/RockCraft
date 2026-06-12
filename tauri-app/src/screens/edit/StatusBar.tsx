// StatusBar.tsx — the edit screen's top status strip.
//
// A live mirror of `edit.rs::draw_status`: input-mode / visual / chord badge,
// then BPM, time-sig, subdivision, bar:beat, loop + metronome state, and the
// clipboard count. Read-only — it derives everything from the current
// `ComposerSnapshot`; it dispatches no actions.

import { Show, type JSX } from "solid-js";
import type { ComposerSnapshot, Subdivision } from "../../ipc/types";
import { barBeatOf, stepUs } from "./viewport";
import { noteName } from "../highway/utils";

const FONT_MONO = "'IBM Plex Mono', ui-monospace, monospace";

const SUBDIVISION_LABEL: Record<Subdivision, string> = {
  Quarter: "1/4",
  Eighth: "1/8",
  Sixteenth: "1/16",
  ThirtySecond: "1/32",
  EighthTriplet: "1/8T",
  SixteenthTriplet: "1/16T",
};

interface BadgeProps {
  text: string;
  bg: string;
  fg?: string;
}

function Badge(props: BadgeProps): JSX.Element {
  return (
    <span
      style={{
        background: props.bg,
        color: props.fg ?? "#0f1016",
        padding: "2px 8px",
        "border-radius": "4px",
        "font-weight": 600,
        "font-size": "11px",
        "letter-spacing": "0.5px",
      }}
    >
      {props.text}
    </span>
  );
}

function Field(props: { label: string; value: string }): JSX.Element {
  return (
    <span style={{ color: "#aeb2c4", "font-family": FONT_MONO, "font-size": "12px" }}>
      <span style={{ color: "#6a6e7e" }}>{props.label} </span>
      {props.value}
    </span>
  );
}

interface Props {
  snapshot: ComposerSnapshot;
  /** Whether the timeline has unsaved changes (shows a dirty indicator). */
  dirty?: boolean;
}

export function StatusBar(props: Props): JSX.Element {
  // Mode badge priority mirrors `edit.rs`: chord > visual > input mode.
  const mode = (): BadgeProps => {
    const s = props.snapshot;
    if (s.chord_preview) return { text: "CHORD", bg: "#3fd0d6" };
    if (s.selection) return { text: "VISUAL", bg: "#7ee08a" };
    switch (s.input_mode) {
      case "DirectEdit":
        return { text: "EDIT", bg: "#d96bff" };
      case "StepRecord":
        return { text: "STEP-REC", bg: "#ff5d6c", fg: "#fff" };
      case "LiveRecord":
        return { text: "LIVE-REC", bg: "#ff5d6c", fg: "#fff" };
    }
  };

  const barBeat = (): string => {
    const s = props.snapshot;
    const cursorUs = s.cursor.step * stepUs(s.bpm, s.subdivision);
    const { bar, beat } = barBeatOf(cursorUs, s.bpm, s.time_sig);
    return `${bar + 1}:${beat + 1}`;
  };

  return (
    <div
      style={{
        display: "flex",
        "align-items": "center",
        gap: "14px",
        padding: "0 14px",
        height: "34px",
        background: "#15161d",
        "border-bottom": "1px solid rgba(255,255,255,0.06)",
        "flex-wrap": "nowrap",
        overflow: "hidden",
      }}
    >
      <Badge {...mode()} />

      <Show when={props.snapshot.looping}>
        <Badge text="LOOP" bg="#5be7c4" />
      </Show>
      <Show when={props.snapshot.metronome}>
        <Badge text="METRO" bg="#7aa2ff" />
      </Show>

      <Field label="bpm" value={String(props.snapshot.bpm)} />
      <Field
        label="sig"
        value={`${props.snapshot.time_sig.beats_per_bar}/${props.snapshot.time_sig.beat_unit}`}
      />
      <Field label="snap" value={SUBDIVISION_LABEL[props.snapshot.subdivision]} />
      <Field label="bar" value={barBeat()} />
      <Field label="♪" value={noteNameSafe(props.snapshot.cursor.pitch)} />
      <Field label="clip" value={String(props.snapshot.clipboard_len)} />

      <Show when={props.snapshot.backing_offset_us !== 0}>
        <Field
          label="backing"
          value={`${signedMs(props.snapshot.backing_offset_us)}ms`}
        />
      </Show>

      <Show when={props.dirty}>
        <span
          style={{
            color: "#f5c542",
            "font-size": "11px",
            "font-weight": 600,
            "flex-shrink": 0,
          }}
          title="Unsaved changes"
        >
          ●
        </span>
      </Show>

      <span
        style={{
          "margin-left": "auto",
          color: "#4a4d5a",
          "font-size": "11px",
          "white-space": "nowrap",
        }}
      >
        a/x add·del · [/] size · +/- vel · m grab · c chord · v·y·p·D select ·
        u/U undo · Space play · o loop · s save · ? help
      </span>
    </div>
  );
}

function noteNameSafe(pitch: number): string {
  return noteName(pitch);
}

function signedMs(us: number): string {
  const ms = Math.round(us / 1000);
  return ms >= 0 ? `+${ms}` : String(ms);
}
