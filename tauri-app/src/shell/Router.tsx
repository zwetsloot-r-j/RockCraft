import {
  createContext,
  createSignal,
  onCleanup,
  onMount,
  Show,
  useContext,
  type JSX,
} from "solid-js";
import type { Screen } from "./screens";
import {
  midiStatus,
  mockKey,
  MOCK_KEY_SET,
  type MidiStatus,
} from "../ipc/midi";

// ── Context ────────────────────────────────────────────────────────────────

interface RouterContext {
  screen: () => Screen;
  navigate: (s: Screen) => void;
}

const RouterCtx = createContext<RouterContext>();

export function useRouter(): RouterContext {
  const ctx = useContext(RouterCtx);
  if (!ctx) throw new Error("useRouter must be used inside <Router>");
  return ctx;
}

// ── Screens that accept instrument (MIDI note) input ──────────────────────

function screenWantsInstrumentInput(s: Screen): boolean {
  // StepRecord / LiveRecord input modes are active on the edit screen; the
  // play screen (#168) scores live mock-key strikes against the chart.
  return s.kind === "edit" || s.kind === "play";
}

// ── Router component ───────────────────────────────────────────────────────

interface RouterProps {
  children: JSX.Element;
}

export function Router(props: RouterProps): JSX.Element {
  const [screen, setScreen] = createSignal<Screen>({ kind: "menu" });
  const [midi, setMidi] = createSignal<MidiStatus | null>(null);

  function navigate(s: Screen): void {
    setScreen(s);
  }

  // Fetch initial MIDI status; refresh whenever it could change.
  function refreshMidiStatus(): void {
    midiStatus()
      .then((s) => setMidi(s))
      .catch(() => {
        /* backend not up yet — ignore */
      });
  }

  // Global Escape: return to menu from any non-menu screen.
  // The importing screen manages its own Escape (only after failure) — skip it
  // here so the pipeline is never aborted mid-run from this global handler.
  function onKeydown(e: KeyboardEvent): void {
    if (
      e.key === "Escape" &&
      screen().kind !== "menu" &&
      screen().kind !== "importing"
    ) {
      navigate({ kind: "menu" });
      return;
    }

    // Forward mapped number-row keys to mock MIDI when:
    //   1. source is mock (live device present → no injection)
    //   2. the focused screen wants instrument input
    //   3. not an auto-repeat event
    const status = midi();
    if (
      status?.kind === "mock" &&
      screenWantsInstrumentInput(screen()) &&
      !e.repeat &&
      MOCK_KEY_SET.has(e.key)
    ) {
      e.preventDefault();
      void mockKey(e.key, true);
    }
  }

  function onKeyup(e: KeyboardEvent): void {
    const status = midi();
    if (
      status?.kind === "mock" &&
      screenWantsInstrumentInput(screen()) &&
      MOCK_KEY_SET.has(e.key)
    ) {
      void mockKey(e.key, false);
    }
  }

  onMount(() => {
    window.addEventListener("keydown", onKeydown);
    window.addEventListener("keyup", onKeyup);
    refreshMidiStatus();
    onCleanup(() => {
      window.removeEventListener("keydown", onKeydown);
      window.removeEventListener("keyup", onKeyup);
    });
  });

  return (
    <RouterCtx.Provider value={{ screen, navigate }}>
      {props.children}
      <MidiStatusChip status={midi()} />
    </RouterCtx.Provider>
  );
}

// ── MidiStatusChip ─────────────────────────────────────────────────────────

/**
 * Shell-level MIDI status indicator shown in the bottom-right corner on every
 * screen. Green dot + port name when a live device is connected; `MOCK` badge
 * when running on the computer-keyboard mock. Hidden while the status is
 * loading.
 */
function MidiStatusChip(props: { status: MidiStatus | null }): JSX.Element {
  return (
    <Show when={props.status !== null}>
      <div
        style={{
          position: "fixed",
          bottom: "10px",
          right: "12px",
          display: "flex",
          "align-items": "center",
          gap: "5px",
          padding: "3px 8px",
          background: "rgba(0,0,0,0.55)",
          "border-radius": "10px",
          "font-family": "system-ui, sans-serif",
          "font-size": "11px",
          color: "#aeb2c4",
          "pointer-events": "none",
          "z-index": "9999",
          "user-select": "none",
        }}
      >
        <Show
          when={props.status?.kind === "live"}
          fallback={
            <span
              style={{
                background: "#3a7bd5",
                color: "#fff",
                "border-radius": "4px",
                padding: "0 5px",
                "font-size": "10px",
                "font-weight": "600",
                "letter-spacing": "0.05em",
              }}
            >
              MOCK
            </span>
          }
        >
          {/* Live device: green dot + port name */}
          <span
            style={{
              width: "7px",
              height: "7px",
              "border-radius": "50%",
              background: "#4caf50",
              "flex-shrink": "0",
            }}
          />
          <span>{props.status?.port ?? "MIDI"}</span>
        </Show>
      </div>
    </Show>
  );
}
