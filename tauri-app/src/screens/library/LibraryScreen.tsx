// LibraryScreen.tsx — Library browser: list saved/imported/recorded bundles
// and open the selected one in Play or Edit. Ported from the TUI
// `crates/tui/src/library_screen.rs`.
//
// Keys (TUI parity):
//   ArrowUp/k   — move selection up
//   ArrowDown/j — move selection down
//   Enter / p   — open in Play
//   e           — open in Edit
//   Esc / q     — back to menu
//
// Mouse: click selects, double-click plays.

import {
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from "solid-js";
import { scanLibrary } from "../../ipc/bridge";
import type { LibraryEntryDto } from "../../ipc/types";
import { useRouter } from "../../shell/Router";

// ── helpers ───────────────────────────────────────────────────────────────

/** Format microseconds as `M:SS`. */
function fmtDuration(us: number): string {
  const secs = Math.floor(us / 1_000_000);
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** Map an origin label to a short badge string. */
function originBadge(origin: string | null): string {
  switch (origin) {
    case "recorded":
      return "Recorded";
    case "composed":
      return "Composed";
    case "edited":
      return "Edited";
    case "imported":
      return "Imported";
    default:
      return "?";
  }
}

// ── component ─────────────────────────────────────────────────────────────

export function LibraryScreen(): JSX.Element {
  const { navigate } = useRouter();
  const [entries, setEntries] = createSignal<LibraryEntryDto[]>([]);
  const [selected, setSelected] = createSignal(0);
  const [loading, setLoading] = createSignal(true);

  // Load bundles on mount.
  onMount(() => {
    scanLibrary()
      .then((list) => {
        setEntries(list);
        setLoading(false);
      })
      .catch(() => {
        // Backend not available (e.g. plain Vite without Tauri) — show empty.
        setLoading(false);
      });
  });

  function clampSel(idx: number): number {
    const n = entries().length;
    if (n === 0) return 0;
    return ((idx % n) + n) % n;
  }

  function moveSelection(delta: number): void {
    setSelected((s) => clampSel(s + delta));
  }

  function openPlay(idx: number): void {
    const e = entries()[idx];
    if (!e) return;
    navigate({ kind: "play", dir: e.dir });
  }

  function openEdit(idx: number): void {
    const e = entries()[idx];
    if (!e) return;
    navigate({ kind: "edit", dir: e.dir });
  }

  function onKeydown(ev: KeyboardEvent): void {
    switch (ev.key) {
      case "ArrowUp":
      case "k":
        ev.preventDefault();
        moveSelection(-1);
        break;
      case "ArrowDown":
      case "j":
        ev.preventDefault();
        moveSelection(1);
        break;
      case "Enter":
      case "p":
        ev.preventDefault();
        openPlay(selected());
        break;
      case "e":
        ev.preventDefault();
        openEdit(selected());
        break;
      case "Escape":
      case "q":
        ev.preventDefault();
        navigate({ kind: "menu" });
        break;
      default:
        break;
    }
  }

  onMount(() => window.addEventListener("keydown", onKeydown));
  onCleanup(() => window.removeEventListener("keydown", onKeydown));

  // ── styles ────────────────────────────────────────────────────────────

  const BG = "#0f1016";
  const FG = "#e7e8ef";
  const DIM = "#8a8e9c";
  const ACCENT = "#4fa3e3";
  const FONT = "'Space Grotesk', system-ui, sans-serif";

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
        padding: "20px 24px",
        gap: "12px",
      }}
    >
      {/* Header */}
      <div
        style={{
          display: "flex",
          "align-items": "center",
          "justify-content": "space-between",
        }}
      >
        <div style={{ "font-size": "20px", "font-weight": 700, color: "#fff" }}>
          Library
        </div>
        <div style={{ "font-size": "12px", color: DIM }}>
          ↑/↓/j/k move · Enter/p Play · e Edit · Esc back
        </div>
      </div>

      {/* List panel */}
      <div
        style={{
          flex: "1 1 auto",
          border: "1px solid rgba(255,255,255,0.1)",
          "border-radius": "8px",
          overflow: "hidden auto",
          display: "flex",
          "flex-direction": "column",
        }}
      >
        <Show when={!loading()} fallback={
          <div
            style={{
              flex: "1 1 auto",
              display: "flex",
              "align-items": "center",
              "justify-content": "center",
              color: DIM,
              "font-size": "14px",
            }}
          >
            Loading…
          </div>
        }>
          <Show
            when={entries().length > 0}
            fallback={
              <div
                style={{
                  flex: "1 1 auto",
                  display: "flex",
                  "align-items": "center",
                  "justify-content": "center",
                  color: DIM,
                  "font-size": "14px",
                  padding: "32px",
                  "text-align": "center",
                }}
              >
                no recordings yet — Record or Compose from the menu
              </div>
            }
          >
            <For each={entries()}>
              {(entry, idx) => {
                const isSel = () => selected() === idx();
                return (
                  <div
                    style={{
                      display: "flex",
                      "align-items": "center",
                      gap: "12px",
                      padding: "10px 14px",
                      cursor: "pointer",
                      background: isSel()
                        ? "rgba(79,163,227,0.15)"
                        : "transparent",
                      "border-left": isSel()
                        ? `3px solid ${ACCENT}`
                        : "3px solid transparent",
                      "border-bottom": "1px solid rgba(255,255,255,0.05)",
                      transition: "background 0.1s",
                      "user-select": "none",
                    }}
                    onMouseEnter={() => setSelected(idx())}
                    onClick={() => setSelected(idx())}
                    onDblClick={() => openPlay(idx())}
                  >
                    {/* Name */}
                    <div
                      style={{
                        "flex": "1 1 auto",
                        "font-size": "14px",
                        "font-weight": isSel() ? 600 : 400,
                        color: isSel() ? "#fff" : FG,
                        "white-space": "nowrap",
                        overflow: "hidden",
                        "text-overflow": "ellipsis",
                      }}
                    >
                      {entry.name}
                    </div>

                    {/* Origin badge */}
                    <div
                      style={{
                        "font-size": "11px",
                        "font-weight": 500,
                        color: DIM,
                        background: "rgba(255,255,255,0.06)",
                        padding: "2px 8px",
                        "border-radius": "10px",
                        "white-space": "nowrap",
                        "flex-shrink": 0,
                      }}
                    >
                      {originBadge(entry.origin)}
                    </div>

                    {/* Note count */}
                    <div
                      style={{
                        "font-size": "12px",
                        color: DIM,
                        "white-space": "nowrap",
                        "flex-shrink": 0,
                        width: "80px",
                        "text-align": "right",
                      }}
                    >
                      {entry.note_count} notes
                    </div>

                    {/* Duration */}
                    <div
                      style={{
                        "font-size": "12px",
                        color: DIM,
                        "white-space": "nowrap",
                        "flex-shrink": 0,
                        width: "48px",
                        "text-align": "right",
                        "font-variant-numeric": "tabular-nums",
                      }}
                    >
                      {fmtDuration(entry.duration_us)}
                    </div>

                    {/* Backing dot */}
                    <div
                      style={{
                        width: "8px",
                        height: "8px",
                        "border-radius": "50%",
                        background: entry.has_backing ? ACCENT : "transparent",
                        border: entry.has_backing
                          ? "none"
                          : "1px solid rgba(255,255,255,0.15)",
                        "flex-shrink": 0,
                      }}
                      title={entry.has_backing ? "has backing track" : "no backing track"}
                    />
                  </div>
                );
              }}
            </For>
          </Show>
        </Show>
      </div>

      {/* Footer */}
      <div style={{ "font-size": "12px", color: DIM }}>
        {entries().length} track{entries().length !== 1 ? "s" : ""}
      </div>
    </div>
  );
}
