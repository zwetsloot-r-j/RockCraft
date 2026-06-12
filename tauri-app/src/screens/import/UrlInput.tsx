// UrlInput.tsx — centered modal URL input for "Import from URL…"
//
// TUI parity (`crates/tui/src/import_screen.rs` UrlInput):
//   - Type characters to build the URL string
//   - Backspace to delete last character
//   - Enter submits (non-empty URL only)
//   - Esc cancels back to menu

import {
  createSignal,
  onCleanup,
  onMount,
  type JSX,
} from "solid-js";
import { useRouter } from "../../shell/Router";

interface UrlInputProps {
  onSubmit: (url: string) => void;
}

const BG = "#0f1016";
const FG = "#e7e8ef";
const DIM = "#8a8e9c";
const ACCENT = "#4fa3e3";
const FONT = "'Space Grotesk', system-ui, sans-serif";

export function UrlInput(props: UrlInputProps): JSX.Element {
  const { navigate } = useRouter();
  const [url, setUrl] = createSignal("");
  const [error, setError] = createSignal("");

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === "Enter") {
      e.preventDefault();
      const val = url().trim();
      if (!val) {
        setError("URL must not be empty");
        return;
      }
      props.onSubmit(val);
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      navigate({ kind: "menu" });
      return;
    }
    if (e.key === "Backspace") {
      e.preventDefault();
      setUrl((s) => s.slice(0, -1));
      setError("");
      return;
    }
    // Accept printable characters (single char, no modifier except Shift).
    if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault();
      setUrl((s) => s + e.key);
      setError("");
    }
  }

  onMount(() => window.addEventListener("keydown", onKeydown));
  onCleanup(() => window.removeEventListener("keydown", onKeydown));

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        "flex-direction": "column",
        "align-items": "center",
        "justify-content": "center",
        background: BG,
        color: FG,
        "font-family": FONT,
      }}
    >
      <div
        style={{
          width: "520px",
          padding: "32px",
          background: "rgba(255,255,255,0.04)",
          border: "1px solid rgba(255,255,255,0.1)",
          "border-radius": "12px",
          display: "flex",
          "flex-direction": "column",
          gap: "16px",
        }}
      >
        {/* Title */}
        <div style={{ "font-size": "18px", "font-weight": 700, color: "#fff" }}>
          Import from URL
        </div>

        {/* URL display */}
        <div
          style={{
            background: "rgba(0,0,0,0.35)",
            border: `1px solid ${error() ? "#e06c75" : "rgba(255,255,255,0.15)"}`,
            "border-radius": "6px",
            padding: "10px 14px",
            "font-size": "14px",
            color: url() ? FG : DIM,
            "min-height": "40px",
            "word-break": "break-all",
            "font-family": "monospace",
          }}
        >
          {url() || "paste or type a URL…"}
          {/* blinking cursor */}
          <span
            style={{
              display: "inline-block",
              width: "1px",
              height: "1em",
              background: ACCENT,
              "margin-left": "2px",
              "vertical-align": "text-bottom",
              animation: "blink 1s step-end infinite",
            }}
          />
        </div>

        {/* Error message */}
        <div
          style={{
            "font-size": "12px",
            color: "#e06c75",
            "min-height": "16px",
          }}
        >
          {error()}
        </div>

        {/* Key hints */}
        <div style={{ "font-size": "12px", color: DIM }}>
          Enter — submit · Esc — cancel
        </div>
      </div>

      {/* CSS for blinking cursor */}
      <style>
        {`@keyframes blink { 0%,100% { opacity:1 } 50% { opacity:0 } }`}
      </style>
    </div>
  );
}
