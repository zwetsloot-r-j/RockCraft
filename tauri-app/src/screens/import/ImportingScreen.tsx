// ImportingScreen.tsx — live import progress display.
//
// TUI parity (`crates/tui/src/import_screen.rs` ImportingScreen):
//   - Stage label (fetching / extracting / writing / done / failed)
//   - Progress bar (0–100 %; only meaningful for "extracting"; indeterminate for others)
//   - Scrolling log pane (last 200 lines)
//   - OMR confidence summary on its own status line (M13-B), when the import was
//     a scan and the sidecar reported one
//   - No Esc-cancel while running (TUI parity; cancellation is not implemented)
//   - "done"   → navigate to library (with new bundle highlighted)
//   - "failed" → error panel; Esc → menu

import {
  createEffect,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from "solid-js";
import { onImportProgress, type ImportProgressEvent } from "../../ipc/bridge";
import { omrSummary } from "../../ipc/omr";
import { useRouter } from "../../shell/Router";

// Maximum log lines retained (TUI parity).
const MAX_LOG_LINES = 200;

const BG = "#0f1016";
const FG = "#e7e8ef";
const DIM = "#8a8e9c";
const ACCENT = "#4fa3e3";
const WARN = "#e5c07b";
const FONT = "'Space Grotesk', system-ui, sans-serif";

interface ImportingScreenProps {
  /** Bundle directory to highlight in the library on success. */
  bundleDir?: string;
}

type Stage =
  | "fetching"
  | "extracting"
  | "writing"
  | "done"
  | "failed"
  | "idle";

function stageLabel(stage: Stage): string {
  switch (stage) {
    case "fetching":
      return "Fetching…";
    case "extracting":
      return "Extracting chart…";
    case "writing":
      return "Writing bundle…";
    case "done":
      return "Import complete";
    case "failed":
      return "Import failed";
    default:
      return "Starting…";
  }
}

export function ImportingScreen(_props: ImportingScreenProps): JSX.Element {
  const { navigate } = useRouter();

  const [stage, setStage] = createSignal<Stage>("idle");
  const [progress, setProgress] = createSignal<number | null>(null);
  const [logLines, setLogLines] = createSignal<string[]>([]);
  const [summary, setSummary] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [_bundleDir, setBundleDir] = createSignal<string | null>(null);
  const [failed, setFailed] = createSignal(false);

  let logContainerEl: HTMLDivElement | undefined;

  function addLog(line: string): void {
    setLogLines((prev) => {
      const next = [...prev, line];
      return next.length > MAX_LOG_LINES ? next.slice(-MAX_LOG_LINES) : next;
    });
  }

  function handleEvent(ev: ImportProgressEvent): void {
    setStage(ev.stage as Stage);
    if (ev.progress !== undefined) {
      setProgress(ev.progress);
    }
    if (ev.log) {
      addLog(ev.log);
      // A scan import's confidence counts arrive on the log stream; promote them
      // out of the pane so they are readable without scrolling.
      const found = omrSummary(ev.log);
      if (found !== null) setSummary(found);
    }
    if (ev.stage === "done" && ev.bundle_dir) {
      setBundleDir(ev.bundle_dir);
      // Short delay so the "Import complete" label is visible.
      setTimeout(() => {
        navigate({ kind: "library" });
      }, 800);
    }
    if (ev.stage === "failed") {
      setFailed(true);
      setError(ev.error ?? "Unknown error");
    }
  }

  // Scroll log to bottom on each new line.
  createEffect(() => {
    logLines(); // reactive dependency
    if (logContainerEl) {
      logContainerEl.scrollTop = logContainerEl.scrollHeight;
    }
  });

  onMount(async () => {
    const unlisten = await onImportProgress(handleEvent);
    onCleanup(unlisten);
  });

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === "Escape" && failed()) {
      e.preventDefault();
      navigate({ kind: "menu" });
    }
    // No Esc-cancel while running — TUI parity.
  }

  onMount(() => window.addEventListener("keydown", onKeydown));
  onCleanup(() => window.removeEventListener("keydown", onKeydown));

  // ── progress bar width ───────────────────────────────────────────────────
  // "extracting" stage shows real progress; all others show an animated fill.
  function barWidth(): string {
    const s = stage();
    if (s === "done") return "100%";
    if (s === "failed") return "0%";
    if (s === "extracting") {
      const p = progress();
      return p !== null ? `${Math.round(p * 100)}%` : "0%";
    }
    // Indeterminate: animated wide bar.
    return "60%";
  }

  function isIndeterminate(): boolean {
    const s = stage();
    return s !== "extracting" && s !== "done" && s !== "failed" && s !== "idle";
  }

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        "flex-direction": "column",
        background: BG,
        color: FG,
        "font-family": FONT,
        "box-sizing": "border-box",
        padding: "24px 28px",
        gap: "16px",
      }}
    >
      {/* Header */}
      <div style={{ "font-size": "20px", "font-weight": 700, color: "#fff" }}>
        {stageLabel(stage())}
      </div>

      {/* Progress bar */}
      <div
        style={{
          height: "8px",
          background: "rgba(255,255,255,0.08)",
          "border-radius": "4px",
          overflow: "hidden",
        }}
      >
        <div
          class={isIndeterminate() ? "indeterminate-bar" : ""}
          style={{
            height: "100%",
            width: barWidth(),
            background: failed() ? "#e06c75" : ACCENT,
            "border-radius": "4px",
            transition: isIndeterminate() ? "none" : "width 0.3s ease",
          }}
        />
      </div>

      {/* OMR confidence summary — a scan import's own status line (M13-B). Shown
          as soon as the sidecar reports it and kept for the rest of the screen,
          because a chart read off a scan is meant to be reviewed. */}
      <Show when={summary() !== null}>
        <div style={{ "font-size": "13px", color: WARN }}>{summary()}</div>
      </Show>

      {/* Stage label for extracting */}
      <Show when={stage() === "extracting"}>
        <div style={{ "font-size": "13px", color: DIM }}>
          {progress() !== null
            ? `${Math.round((progress() ?? 0) * 100)} %`
            : "0 %"}
        </div>
      </Show>

      {/* Log pane */}
      <div
        ref={logContainerEl}
        style={{
          flex: "1 1 auto",
          overflow: "hidden auto",
          background: "rgba(0,0,0,0.35)",
          border: "1px solid rgba(255,255,255,0.08)",
          "border-radius": "6px",
          padding: "10px 12px",
          "font-family": "monospace",
          "font-size": "12px",
          color: DIM,
          "line-height": "1.5",
        }}
      >
        <For each={logLines()}>
          {(line) => (
            <div style={{ "white-space": "pre-wrap", "word-break": "break-all" }}>
              {line}
            </div>
          )}
        </For>
      </div>

      {/* Error panel */}
      <Show when={failed()}>
        <div
          style={{
            background: "rgba(224,108,117,0.12)",
            border: "1px solid rgba(224,108,117,0.5)",
            "border-radius": "8px",
            padding: "14px 16px",
            "font-size": "13px",
            color: "#e06c75",
          }}
        >
          <div style={{ "font-weight": 700, "margin-bottom": "6px" }}>
            Import failed
          </div>
          <div style={{ "word-break": "break-word" }}>{error()}</div>
          <div style={{ "margin-top": "12px", "font-size": "12px", color: DIM }}>
            Esc — back to menu (log retained above)
          </div>
        </div>
      </Show>

      {/* Footer note: no cancel */}
      <Show when={!failed() && stage() !== "done"}>
        <div style={{ "font-size": "11px", color: "#4a4d5a" }}>
          Import cannot be cancelled — wait for the pipeline to finish.
        </div>
      </Show>

      {/* Indeterminate animation */}
      <style>
        {`
          @keyframes slide {
            0%   { transform: translateX(-100%) }
            100% { transform: translateX(200%) }
          }
          .indeterminate-bar {
            animation: slide 1.4s ease-in-out infinite;
          }
        `}
      </style>
    </div>
  );
}
