// UrlInput.tsx — centered modal URL input for "Import from URL…"
//
// Uses a real focusable <input> so the OS/webview handle paste (Ctrl/Cmd+V),
// caret, selection, and mid-string editing natively.
//
// TUI parity (`crates/tui/src/import_screen.rs` UrlInput):
//   - Enter submits (non-empty URL only)
//   - Esc cancels back to menu

import {
  createSignal,
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

  let inputRef: HTMLInputElement | undefined;

  function onInput(e: InputEvent & { currentTarget: HTMLInputElement }): void {
    setUrl(e.currentTarget.value);
    setError("");
  }

  function onKeydown(e: KeyboardEvent): void {
    // Keep typing/paste inside the field; never let it reach the shell's
    // global mock-MIDI / router keydown handler.
    e.stopPropagation();

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
    }
  }

  onMount(() => {
    inputRef?.focus();
  });

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

        {/* URL input */}
        <input
          ref={inputRef}
          type="text"
          value={url()}
          onInput={onInput}
          onKeyDown={onKeydown}
          placeholder="paste or type a URL…"
          spellcheck={false}
          autocapitalize="off"
          autocomplete="off"
          autocorrect="off"
          style={{
            background: "rgba(0,0,0,0.35)",
            border: `1px solid ${error() ? "#e06c75" : "rgba(255,255,255,0.15)"}`,
            "border-radius": "6px",
            padding: "10px 14px",
            "font-size": "14px",
            color: FG,
            "min-height": "40px",
            "font-family": "monospace",
            outline: "none",
            "caret-color": ACCENT,
            width: "100%",
            "box-sizing": "border-box",
          }}
        />

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
    </div>
  );
}
