import { createSignal, type JSX, Match, Switch } from "solid-js";
import { HighwayScreen } from "./screens/highway/HighwayScreen";
import { RecordScreen } from "./screens/record/RecordScreen";

type Screen = "highway" | "record";

// Minimal screen switcher (no routing infra yet — the app shell/router is #162).
// Both the note highway (#23) and this record screen are reachable from the tab
// bar; the router will replace this hand-rolled switcher once #162 lands.
export default function App(): JSX.Element {
  const [screen, setScreen] = createSignal<Screen>("record");

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
      </div>
      <div style={{ flex: "1 1 auto", "min-height": 0 }}>
        <Switch>
          <Match when={screen() === "record"}>
            <RecordScreen />
          </Match>
          <Match when={screen() === "highway"}>
            <HighwayScreen />
          </Match>
        </Switch>
      </div>
    </div>
  );
}
