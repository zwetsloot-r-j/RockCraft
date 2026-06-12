import {
  createSignal,
  type JSX,
  Match,
  onCleanup,
  onMount,
  Switch,
} from "solid-js";
import { createStore } from "solid-js/store";
import { HighwayScreen } from "./screens/highway/HighwayScreen";
import { RecordScreen } from "./screens/record/RecordScreen";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { onSnapshot, queryState, runAction } from "./ipc/bridge";
import type { ComposerSnapshot } from "./ipc/types";

type Screen = "highway" | "record" | "debug";

// Minimal screen switcher (no routing infra yet — the app shell/router is #162).
// Both the note highway (#23) and this record screen are reachable from the tab
// bar; the router will replace this hand-rolled switcher once #162 lands. The
// "Debug" tab is the temporary IPC-bridge strip from #161 — it proves the
// run_action → snapshot-event round trip and goes away once the edit screen
// lands.
export default function App(): JSX.Element {
  const [screen, setScreen] = createSignal<Screen>("debug");

  const TabBtn = (p: { id: Screen; label: string }): JSX.Element => (
    <button
      onClick={() => setScreen(p.id)}
      style={{
        border: "none",
        cursor: "pointer",
        padding: "6px 14px",
        "border-radius": "7px",
        "font-family": "system-ui, sans-serif",
        "font-size": "12px",
        "font-weight": 600,
        background: screen() === p.id ? "rgba(255,255,255,0.14)" : "transparent",
        color: screen() === p.id ? "#fff" : "#8a8e9c",
      }}
    >
      {p.label}
    </button>
  );

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        "flex-direction": "column",
        background: "#0f1016",
        color: "#e7e8ef",
      }}
    >
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "4px",
          padding: "6px 10px",
          background: "#15161d",
          "border-bottom": "1px solid rgba(255,255,255,0.06)",
          flex: "0 0 auto",
        }}
      >
        <TabBtn id="highway" label="Highway" />
        <TabBtn id="record" label="Record" />
        <TabBtn id="debug" label="Debug" />
      </div>
      <div style={{ flex: "1 1 auto", "min-height": 0 }}>
        <Switch>
          <Match when={screen() === "record"}>
            <RecordScreen />
          </Match>
          <Match when={screen() === "highway"}>
            <HighwayScreen />
          </Match>
          <Match when={screen() === "debug"}>
            <DebugStrip />
          </Match>
        </Switch>
      </div>
    </div>
  );
}

// Temporary IPC-bridge proof (#161). The snapshot mirror is a `createStore`
// updated from the `snapshot` event the backend pushes after every action and
// each transport tick. Clicking the buttons runs the matching core action; the
// rendered snapshot updates purely via the event (we never read run_action's
// return value here), proving the round trip in `npm run dev`.
function DebugStrip(): JSX.Element {
  const [snap, setSnap] = createStore<{ value: ComposerSnapshot | null }>({
    value: null,
  });

  onMount(() => {
    let unlisten: UnlistenFn | undefined;
    onSnapshot((s) => setSnap("value", s)).then((fn) => {
      unlisten = fn;
    });
    // Prime with the current state so the strip isn't blank before the first
    // action or tick.
    queryState()
      .then((s) => setSnap("value", s))
      .catch(() => {
        /* backend not up yet (e.g. plain vite without tauri) — ignore */
      });
    onCleanup(() => unlisten?.());
  });

  const Btn = (p: { label: string; onClick: () => void }): JSX.Element => (
    <button
      onClick={p.onClick}
      style={{
        border: "1px solid rgba(255,255,255,0.18)",
        background: "rgba(255,255,255,0.06)",
        color: "#e7e8ef",
        cursor: "pointer",
        padding: "6px 12px",
        "border-radius": "6px",
        "font-family": "system-ui, sans-serif",
        "font-size": "12px",
      }}
    >
      {p.label}
    </button>
  );

  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        "flex-direction": "column",
        gap: "10px",
        padding: "14px",
        "font-family": "system-ui, sans-serif",
        "box-sizing": "border-box",
      }}
    >
      <div style={{ display: "flex", gap: "8px" }}>
        <Btn label="add_note" onClick={() => void runAction("add_note")} />
        <Btn
          label="cursor_right"
          onClick={() => void runAction("cursor_right")}
        />
        <Btn label="undo" onClick={() => void runAction("undo")} />
      </div>
      <pre
        style={{
          flex: "1 1 auto",
          margin: 0,
          padding: "12px",
          overflow: "auto",
          background: "#15161d",
          "border-radius": "8px",
          "font-size": "11px",
          "line-height": 1.5,
          color: "#aeb2c4",
        }}
      >
        {snap.value
          ? JSON.stringify(snap.value, null, 2)
          : "(waiting for snapshot…)"}
      </pre>
    </div>
  );
}
