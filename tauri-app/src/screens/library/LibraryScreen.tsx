// LibraryScreen.tsx — Library browser: list saved/imported/recorded bundles
// and open the selected one in Play or Edit. Ported from the TUI
// `crates/tui/src/library_screen.rs`.
//
// Keys (TUI parity):
//   ArrowUp/k   — move selection up
//   ArrowDown/j — move selection down
//   Enter / p   — open in Play
//   e           — open in Edit
//   x           — export the selected song as a .rcsong file (M26-A)
//   i           — import a .rcsong file into the library (M26-A)
//   N           — open/close the Network panel (M26-D)
//   Esc / q     — back to menu (Esc closes the Network panel first)
//
// With the Network panel open (M26-D), Tab moves the keyboard between the
// local list, the servers and the remote songs:
//   u           — (local list) send the selected song to the selected server
//   S           — share this library on/off
//   r / + / Del — (servers) search again / add an address / remove an added one
//   Enter / d   — (remote songs) download
//
// Mouse: click selects, double-click plays (a remote song: downloads).

import {
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from "solid-js";
import {
  libraryExportSong,
  libraryImportSong,
  openSongFilePicker,
  saveSongFilePicker,
  scanLibrary,
} from "../../ipc/bridge";
import { baseName, existsSlug, freeSlug } from "./songFiles";
import {
  NetworkPanel,
  TransferStrip,
  useNetwork,
  type Conflict,
  type NetFocus,
} from "./NetworkPanel";
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
  /** One-line result of the last export/import, shown in the footer. */
  const [flash, setFlash] = createSignal<string | null>(null);
  /** A name clash (import, download or send) awaiting Replace / Keep both. */
  const [conflict, setConflict] = createSignal<Conflict | null>(null);
  /** The Network panel (M26-D), and which list has the keyboard. */
  const [netOpen, setNetOpen] = createSignal(false);
  const [focus, setFocus] = createSignal<"local" | NetFocus>("local");
  /** The scrollable list panel; its element children are the rows, in order. */
  let listEl: HTMLDivElement | undefined;
  let flashTimer: number | undefined;

  function showFlash(msg: string): void {
    setFlash(msg);
    window.clearTimeout(flashTimer);
    flashTimer = window.setTimeout(() => setFlash(null), 4000);
  }
  onCleanup(() => window.clearTimeout(flashTimer));

  /** Re-read the library; select the bundle at `dir` when given. */
  function rescan(dir?: string): Promise<void> {
    return scanLibrary()
      .then((list) => {
        setEntries(list);
        setLoading(false);
        if (dir !== undefined) {
          const norm = (p: string) => p.replace(/\\/g, "/");
          const idx = list.findIndex(
            (e) => e.name === baseName(dir) && norm(e.dir).endsWith(norm(dir)),
          );
          if (idx >= 0) {
            setSelected(idx);
            listEl?.children[idx]?.scrollIntoView({ block: "nearest" });
          }
        }
      })
      .catch(() => {
        // Backend not available (e.g. plain Vite without Tauri) — show empty.
        setLoading(false);
      });
  }

  // Load bundles on mount.
  onMount(() => void rescan());

  /** `x`: write the selected song to a .rcsong file the user picks. */
  async function exportSelected(): Promise<void> {
    const e = entries()[selected()];
    if (!e) return;
    const path = await saveSongFilePicker(e.name);
    if (path === null) return;
    try {
      const written = await libraryExportSong(e.dir, path);
      showFlash(`exported → ${baseName(written)}`);
    } catch (err) {
      showFlash(`export failed: ${String(err)}`);
    }
  }

  /** Import `path`; a taken name opens the Replace / Keep both prompt. */
  async function importFile(path: string, overwrite: boolean, name: string | null): Promise<void> {
    try {
      const dir = await libraryImportSong(path, overwrite, name);
      await rescan(dir);
      showFlash(`imported ${baseName(dir)}`);
    } catch (err) {
      const slug = existsSlug(err);
      if (slug !== null && !overwrite) {
        setConflict({
          slug,
          where: "the library",
          replace: () => void importFile(path, true, null),
          keepBoth: () =>
            void importFile(path, false, freeSlug(slug, entries().map((e) => e.name))),
        });
      } else {
        showFlash(`import failed: ${String(err)}`);
      }
    }
  }

  /** `i`: pick a .rcsong file and import it. */
  async function importPicked(): Promise<void> {
    const path = await openSongFilePicker();
    if (path !== null) await importFile(path, false, null);
  }

  const net = useNetwork({
    localNames: () => entries().map((e) => e.name),
    flash: showFlash,
    onDownloaded: (dir) => void rescan(dir),
    askConflict: setConflict,
  });

  function toggleNetwork(): void {
    if (netOpen()) {
      setNetOpen(false);
      setFocus("local");
    } else {
      setNetOpen(true);
      setFocus("servers");
      net.open();
    }
  }

  /** Tab order: local → servers → songs → local. */
  function cycleFocus(): void {
    const order: Array<"local" | NetFocus> = ["local", "servers", "songs"];
    setFocus(order[(order.indexOf(focus()) + 1) % order.length]);
  }

  /** Keys while the name-conflict prompt is open: it owns the keyboard. */
  function onConflictKey(ev: KeyboardEvent, c: Conflict): void {
    ev.preventDefault();
    ev.stopPropagation();
    switch (ev.key) {
      case "r":
      case "R":
        setConflict(null);
        c.replace();
        break;
      case "k":
      case "K":
        setConflict(null);
        c.keepBoth();
        break;
      case "Escape":
      case "c":
      case "C":
        setConflict(null);
        break;
      default:
        break;
    }
  }

  function clampSel(idx: number): number {
    const n = entries().length;
    if (n === 0) return 0;
    return ((idx % n) + n) % n;
  }

  /** Keyboard move: select, then scroll the row into view. ↑/↓ suppress the
   * panel's native scrolling, so without this the highlight walks off-screen.
   * (Hover/click selection never scrolls — the row is already under the mouse.) */
  function moveSelection(delta: number): void {
    const next = clampSel(selected() + delta);
    setSelected(next);
    listEl?.children[next]?.scrollIntoView({ block: "nearest" });
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
    const c = conflict();
    if (c !== null) {
      onConflictKey(ev, c);
      return;
    }
    // A text field (the "add address" input) owns its keys.
    const target = ev.target as HTMLElement | null;
    if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) return;

    if (ev.key === "N") {
      ev.preventDefault();
      toggleNetwork();
      return;
    }
    if (netOpen()) {
      if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        toggleNetwork();
        return;
      }
      if (ev.key === "Tab") {
        ev.preventDefault();
        cycleFocus();
        return;
      }
      if (ev.key === "S") {
        ev.preventDefault();
        void net.toggleSharing();
        return;
      }
      if (focus() === "servers") {
        onServersKey(ev);
        return;
      }
      if (focus() === "songs") {
        onSongsKey(ev);
        return;
      }
      if (ev.key === "u") {
        ev.preventDefault();
        const e = entries()[selected()];
        if (e) net.uploadBundle(e.dir);
        return;
      }
    }
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
      case "x":
        ev.preventDefault();
        void exportSelected();
        break;
      case "i":
        ev.preventDefault();
        void importPicked();
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

  /** Keys while the servers list has focus. */
  function onServersKey(ev: KeyboardEvent): void {
    switch (ev.key) {
      case "ArrowUp":
      case "k":
        ev.preventDefault();
        net.moveServer(-1);
        break;
      case "ArrowDown":
      case "j":
        ev.preventDefault();
        net.moveServer(1);
        break;
      case "Enter":
        ev.preventDefault();
        setFocus("songs");
        break;
      case "r":
        ev.preventDefault();
        void net.discover();
        break;
      case "+":
        ev.preventDefault();
        net.setAdding(true);
        break;
      case "Delete":
      case "Backspace":
        ev.preventDefault();
        net.removeSelectedServer();
        break;
      default:
        break;
    }
  }

  /** Keys while the remote songs list has focus. */
  function onSongsKey(ev: KeyboardEvent): void {
    switch (ev.key) {
      case "ArrowUp":
      case "k":
        ev.preventDefault();
        net.moveSong(-1);
        break;
      case "ArrowDown":
      case "j":
        ev.preventDefault();
        net.moveSong(1);
        break;
      case "Enter":
      case "d":
        ev.preventDefault();
        net.downloadSong();
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
          {netOpen()
            ? "Tab switch list · u send · S share on/off · N close · Esc back"
            : "↑/↓/j/k move · Enter/p Play · e Edit · x export · i import · N network · Esc back"}
        </div>
      </div>

      <div style={{ flex: "1 1 auto", display: "flex", gap: "12px", "min-height": 0 }}>
      {/* List panel */}
      <div
        ref={listEl}
        style={{
          flex: "1 1 0",
          "min-width": 0,
          border:
            netOpen() && focus() === "local"
              ? `1px solid ${ACCENT}`
              : "1px solid rgba(255,255,255,0.1)",
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
                    onClick={() => {
                      setSelected(idx());
                      setFocus("local");
                    }}
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
      <Show when={netOpen()}>
        <NetworkPanel
          net={net}
          focus={focus() === "local" ? null : (focus() as NetFocus)}
          onFocus={setFocus}
        />
      </Show>
      </div>

      <TransferStrip net={net} />

      {/* Name-conflict prompt (import / download / send) */}
      <Show when={conflict()}>
        {(c) => (
          <div
            style={{
              border: `1px solid ${ACCENT}`,
              "border-radius": "8px",
              padding: "10px 14px",
              "font-size": "13px",
            }}
          >
            A song named <b>{c().slug}</b> is already in {c().where}. R replace ·
            K keep both · Esc cancel
          </div>
        )}
      </Show>

      {/* Footer */}
      <div style={{ "font-size": "12px", color: DIM, display: "flex", gap: "16px" }}>
        <span>
          {entries().length} track{entries().length !== 1 ? "s" : ""}
        </span>
        <Show when={flash()}>{(m) => <span style={{ color: FG }}>{m()}</span>}</Show>
      </div>
    </div>
  );
}
