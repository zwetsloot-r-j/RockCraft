// RecordHeader.tsx — the 56 px top status bar (Design E, from B · Console).
// Reads live clock/level off the engine; re-evaluates whenever the throttled
// `frame` signal bumps (the engine itself is framework-free, per CONVENTIONS).
// #169: shows real MIDI device status, session timecode, and backing chip.

import { type Accessor, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";
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
import { midiStatus, type MidiStatus } from "../../ipc/midi";

export interface RecordHeaderProps {
  eng: Accessor<RecordCanvas | null>;
  frame: Accessor<number>;
  song: Song;
  metro: Accessor<boolean>;
  onMetro: (v: boolean) => void;
  count: Accessor<boolean>;
  onCount: (v: boolean) => void;
  /** Whether a recording session is currently active. */
  recording: Accessor<boolean>;
  /** Wall-clock ms when the session started (for timecode display). */
  sessionStart: Accessor<number>;
  /** Path to the currently attached backing file, or null. */
  backingPath: Accessor<string | null>;
  /** Called when the user clicks "Choose backing track". */
  onChooseBacking: () => void;
}

export function RecordHeader(props: RecordHeaderProps): JSX.Element {
  // Poll MIDI status once and refresh on frame ticks.
  const [midi, setMidi] = createSignal<MidiStatus>({ kind: "mock", port: null });

  onMount(() => {
    // Initial fetch.
    midiStatus().then(setMidi).catch(() => {});
    // Refresh ~every 2s (midi device can be plugged in at any time).
    const id = window.setInterval(() => {
      midiStatus().then(setMidi).catch(() => {});
    }, 2000);
    onCleanup(() => window.clearInterval(id));
  });

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

  // Session elapsed timecode: when recording, compute from sessionStart.
  const sessionTc = createMemo(() => {
    props.frame(); // re-run on frame tick
    if (!props.recording()) return null;
    const elapsed = Date.now() - props.sessionStart();
    return elapsed > 0 ? elapsed : 0;
  });

  const backingName = createMemo(() => {
    const p = props.backingPath();
    if (!p) return null;
    return p.split(/[/\\]/).pop() ?? p;
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
          {props.song.title}{" "}
          <span style={{ color: "#7c8094", "font-weight": 400 }}>
            · {props.recording() ? "Recording…" : "Take ready"}
          </span>
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
        {/* Show session elapsed when recording, otherwise canvas clock */}
        {sessionTc() != null ? tc(sessionTc()!) : tc(clock().now)}
      </div>
      <Chip mono>
        BAR {clock().bar + 1}.{clock().beat + 1}
      </Chip>
      <Chip c="oklch(0.82 0.13 150)">{clock().chord}</Chip>
      <div style={{ flex: 1 }} />

      {/* Backing chip / picker */}
      {backingName() ? (
        <Chip c={OK} bg="rgba(91,231,196,0.08)">
          <Dot c={OK} s={6} />
          {backingName()}
        </Chip>
      ) : null}
      <button
        onClick={props.onChooseBacking}
        title="Choose backing track"
        style={{
          "font-size": "10px",
          "font-family": MONO,
          padding: "4px 10px",
          "border-radius": "6px",
          border: "1px solid rgba(255,255,255,0.12)",
          background: "rgba(255,255,255,0.05)",
          color: "#aeb2c2",
          cursor: "pointer",
          "white-space": "nowrap",
          flex: "0 0 auto",
        }}
      >
        {backingName() ? "Change backing" : "Choose backing track"}
      </button>

      <Toggle active={props.metro()} onClick={() => props.onMetro(!props.metro())} title="Metronome">
        <Icon d="metro" size={13} stroke={props.metro() ? OK : "#6e7282"} /> {props.song.tempoBpm}
      </Toggle>
      <Toggle active={props.count()} onClick={() => props.onCount(!props.count())} title="Count-in">
        COUNT 1
      </Toggle>

      {/* MIDI device chip: green + port when live, amber MOCK otherwise */}
      <Chip
        c={midi().kind === "live" ? OK : "#f0c060"}
        bg={midi().kind === "live" ? "rgba(91,231,196,0.08)" : "rgba(240,192,96,0.10)"}
      >
        <Dot c={midi().kind === "live" ? OK : "#f0c060"} s={6} />
        {midi().kind === "live" ? `MIDI · ${midi().port ?? "unknown"}` : "MOCK"}
      </Chip>

      <Meter level={clock().level} />
    </div>
  );
}
