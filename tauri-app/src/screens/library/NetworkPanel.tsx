// NetworkPanel.tsx — the library's Network panel (M26-D): share this library,
// browse other RockCrafts on the home network, download their songs and send
// ours. `useNetwork` holds the state and actions (called by LibraryScreen,
// which routes the keys); `NetworkPanel` draws it; `TransferStrip` shows
// progress under the lists.

import { createSignal, For, onCleanup, Show, type JSX } from "solid-js";
import {
  shareDiscover,
  shareDownload,
  shareList,
  shareStart,
  shareStatus,
  shareStop,
  shareTransfers,
  shareUpload,
} from "../../ipc/bridge";
import type { ListedSong, SharePeer, ShareStatus, ShareTransfer } from "../../ipc/types";
import {
  fmtMB,
  loadServers,
  localBadge,
  parseServer,
  saveServers,
  saveShareEnabled,
  transferLine,
  transferPercent,
} from "./network";
import { baseName, freeSlug } from "./songFiles";

/** A name clash waiting for the user: Replace / Keep both / Cancel. */
export interface Conflict {
  slug: string;
  /** Where the clash is, for the prompt: "the library" / "<server>". */
  where: string;
  replace: () => void;
  keepBoth: () => void;
}

/** What the panel needs from the library screen. */
export interface NetworkDeps {
  /** Names of the local songs (for "Keep both" on download). */
  localNames: () => string[];
  flash: (msg: string) => void;
  /** A download finished into `dir`: rescan and select it. */
  onDownloaded: (dir: string) => void;
  askConflict: (c: Conflict) => void;
}

/** Which network list has the keyboard. */
export type NetFocus = "servers" | "songs";

/** One row of the servers list. */
export interface ServerRow {
  label: string;
  addr: string;
  manual: boolean;
}

/** A transfer this panel started and still waits on. */
type Pending =
  | { kind: "down"; server: string; id: string; name: string }
  | { kind: "up"; server: string; bundle: string; name: string };

const POLL_MS = 500;

/** Network state and actions; call once inside the library screen. */
export function useNetwork(deps: NetworkDeps) {
  const [status, setStatus] = createSignal<ShareStatus | null>(null);
  /** Why the last attempt to start sharing failed; stays until the next try. */
  const [shareError, setShareError] = createSignal<string | null>(null);
  const [peers, setPeers] = createSignal<SharePeer[]>([]);
  const [manual, setManual] = createSignal<string[]>(loadServers());
  const [discovering, setDiscovering] = createSignal(false);
  const [serverIdx, setServerIdx] = createSignal(0);
  const [songs, setSongs] = createSignal<ListedSong[]>([]);
  const [songIdx, setSongIdx] = createSignal(0);
  const [songsError, setSongsError] = createSignal<string | null>(null);
  const [loadingSongs, setLoadingSongs] = createSignal(false);
  const [transfers, setTransfers] = createSignal<ShareTransfer[]>([]);
  /** The "add address" input, while open. */
  const [adding, setAdding] = createSignal(false);
  const pending = new Map<number, Pending>();
  let pollTimer: number | undefined;
  /** Guards against a slow list reply for a server no longer selected. */
  let listGen = 0;

  /** Discovered peers then manual addresses, de-duplicated by address. */
  function servers(): ServerRow[] {
    const rows: ServerRow[] = peers().map((p) => ({ label: p.name, addr: p.addr, manual: false }));
    for (const addr of manual()) {
      if (!rows.some((r) => r.addr === addr)) rows.push({ label: addr, addr, manual: true });
    }
    return rows;
  }

  function selectedServer(): ServerRow | undefined {
    return servers()[serverIdx()];
  }

  function refreshStatus(): void {
    shareStatus()
      .then(setStatus)
      .catch(() => setStatus(null));
  }

  async function toggleSharing(): Promise<void> {
    setShareError(null);
    try {
      if (status()?.running) {
        await shareStop();
        saveShareEnabled(false);
        deps.flash("sharing off");
      } else {
        const s = await shareStart();
        saveShareEnabled(true);
        deps.flash(`sharing at ${s.addresses[0] ?? `port ${s.port}`}`);
      }
    } catch (err) {
      setShareError(String(err));
      deps.flash("sharing failed");
    }
    refreshStatus();
  }

  async function discover(): Promise<void> {
    setDiscovering(true);
    try {
      setPeers(await shareDiscover());
    } catch {
      setPeers([]);
    }
    setDiscovering(false);
    if (serverIdx() >= servers().length) setServerIdx(0);
    await refreshSongs();
  }

  async function refreshSongs(): Promise<void> {
    const server = selectedServer();
    const gen = ++listGen;
    if (!server) {
      setSongs([]);
      setSongsError(null);
      return;
    }
    setLoadingSongs(true);
    try {
      const list = await shareList(server.addr);
      if (gen !== listGen) return;
      setSongs(list);
      setSongsError(null);
      if (songIdx() >= list.length) setSongIdx(0);
    } catch (err) {
      if (gen !== listGen) return;
      setSongs([]);
      setSongsError(`can't reach ${server.addr}: ${String(err)}`);
    } finally {
      if (gen === listGen) setLoadingSongs(false);
    }
  }

  function moveServer(delta: number): void {
    const n = servers().length;
    if (n === 0) return;
    setServerIdx((((serverIdx() + delta) % n) + n) % n);
    setSongIdx(0);
    void refreshSongs();
  }

  function selectServer(i: number): void {
    if (i === serverIdx()) return;
    setServerIdx(i);
    setSongIdx(0);
    void refreshSongs();
  }

  function moveSong(delta: number): void {
    const n = songs().length;
    if (n === 0) return;
    setSongIdx((((songIdx() + delta) % n) + n) % n);
  }

  function addServer(text: string): boolean {
    const addr = parseServer(text);
    if (addr === null) {
      deps.flash("enter an address like 192.168.1.5:27800");
      return false;
    }
    if (!manual().includes(addr)) {
      const next = [...manual(), addr];
      setManual(next);
      saveServers(next);
    }
    setAdding(false);
    const idx = servers().findIndex((r) => r.addr === addr);
    if (idx >= 0) selectServer(idx);
    return true;
  }

  function removeSelectedServer(): void {
    const s = selectedServer();
    if (!s?.manual) return;
    const next = manual().filter((a) => a !== s.addr);
    setManual(next);
    saveServers(next);
    setServerIdx(0);
    void refreshSongs();
  }

  // ── transfers ───────────────────────────────────────────────────────

  function track(id: number, p: Pending): void {
    pending.set(id, p);
    if (pollTimer === undefined) pollTimer = window.setInterval(() => void poll(), POLL_MS);
    void poll();
  }

  async function poll(): Promise<void> {
    let rows: ShareTransfer[];
    try {
      rows = await shareTransfers();
    } catch {
      return;
    }
    setTransfers(rows);
    for (const row of rows) {
      const p = pending.get(row.id);
      if (!p || row.state === "running") continue;
      pending.delete(row.id);
      settle(row, p);
    }
    if (pending.size === 0 && pollTimer !== undefined) {
      window.clearInterval(pollTimer);
      pollTimer = undefined;
    }
  }

  /** React to a transfer that just finished. */
  function settle(row: ShareTransfer, p: Pending): void {
    switch (row.state) {
      case "done":
        if (p.kind === "down" && row.dir) {
          deps.onDownloaded(row.dir);
          deps.flash(`downloaded ${baseName(row.dir)}`);
        } else {
          deps.flash(`sent ${p.name} to ${p.server}`);
        }
        void refreshSongs();
        break;
      case "failed":
        deps.flash(`${p.kind === "down" ? "download" : "send"} failed: ${row.detail}`);
        break;
      case "exists":
        if (p.kind === "down") {
          deps.askConflict({
            slug: row.slug,
            where: "the library",
            replace: () => void startDownload(p.server, p.id, p.name, true, null),
            keepBoth: () =>
              void startDownload(p.server, p.id, p.name, false, freeSlug(row.slug, deps.localNames())),
          });
        } else {
          deps.askConflict({
            slug: row.slug,
            where: p.server,
            replace: () => void startUpload(p.server, p.bundle, p.name, true, null),
            keepBoth: () =>
              void startUpload(
                p.server,
                p.bundle,
                p.name,
                false,
                freeSlug(row.slug, songs().map((s) => s.name)),
              ),
          });
        }
        break;
      case "running":
        break;
    }
  }

  async function startDownload(
    server: string,
    id: string,
    name: string,
    overwrite: boolean,
    renameTo: string | null,
  ): Promise<void> {
    try {
      const tid = await shareDownload(server, id, overwrite, renameTo);
      track(tid, { kind: "down", server, id, name });
    } catch (err) {
      deps.flash(`download failed: ${String(err)}`);
    }
  }

  async function startUpload(
    server: string,
    bundle: string,
    name: string,
    overwrite: boolean,
    renameTo: string | null,
  ): Promise<void> {
    try {
      const tid = await shareUpload(server, bundle, overwrite, renameTo);
      track(tid, { kind: "up", server, bundle, name });
    } catch (err) {
      deps.flash(`send failed: ${String(err)}`);
    }
  }

  /** Download remote song `i` (default: the selected one). */
  function downloadSong(i: number = songIdx()): void {
    const server = selectedServer();
    const song = songs()[i];
    if (!server || !song) return;
    void startDownload(server.addr, song.id, song.name, false, null);
  }

  /** Send the local bundle at `dir` to the selected server. */
  function uploadBundle(dir: string): void {
    const server = selectedServer();
    if (!server) {
      deps.flash("pick a server in the Network panel first");
      return;
    }
    void startUpload(server.addr, dir, baseName(dir), false, null);
  }

  /** Panel opened: fresh status, peers, songs and transfers. */
  function open(): void {
    refreshStatus();
    void discover();
    void poll();
  }

  onCleanup(() => {
    if (pollTimer !== undefined) window.clearInterval(pollTimer);
  });

  return {
    status,
    shareError,
    servers,
    serverIdx,
    songs,
    songIdx,
    songsError,
    loadingSongs,
    discovering,
    transfers,
    adding,
    setAdding,
    open,
    toggleSharing,
    discover,
    moveServer,
    selectServer,
    moveSong,
    setSongIdx,
    addServer,
    removeSelectedServer,
    downloadSong,
    uploadBundle,
    selectedServer,
  };
}

export type Network = ReturnType<typeof useNetwork>;

const FG = "#e7e8ef";
const DIM = "#8a8e9c";
const ACCENT = "#4fa3e3";

function rowStyle(selected: boolean, focused: boolean): JSX.CSSProperties {
  return {
    display: "flex",
    "align-items": "center",
    gap: "10px",
    padding: "7px 12px",
    cursor: "pointer",
    "font-size": "13px",
    background: selected ? (focused ? "rgba(79,163,227,0.15)" : "rgba(255,255,255,0.05)") : "transparent",
    "border-left": selected && focused ? `3px solid ${ACCENT}` : "3px solid transparent",
    "border-bottom": "1px solid rgba(255,255,255,0.05)",
    "user-select": "none",
  };
}

const boxStyle: JSX.CSSProperties = {
  border: "1px solid rgba(255,255,255,0.1)",
  "border-radius": "8px",
  overflow: "hidden auto",
  display: "flex",
  "flex-direction": "column",
};

const titleStyle: JSX.CSSProperties = {
  "font-size": "11px",
  "text-transform": "uppercase",
  "letter-spacing": "0.06em",
  color: DIM,
  padding: "2px 2px 4px",
};

/** The right-hand Network panel. */
export function NetworkPanel(props: {
  net: Network;
  focus: NetFocus | null;
  onFocus: (f: NetFocus) => void;
}): JSX.Element {
  const net = props.net;
  let addInput: HTMLInputElement | undefined;

  return (
    <div style={{ flex: "1 1 0", display: "flex", "flex-direction": "column", gap: "10px", "min-width": 0 }}>
      {/* Sharing toggle */}
      <div
        style={{
          ...boxStyle,
          "flex-direction": "row",
          "align-items": "center",
          gap: "12px",
          padding: "10px 12px",
          overflow: "hidden",
        }}
      >
        <button
          onClick={() => void net.toggleSharing()}
          style={{
            background: net.status()?.running ? ACCENT : "rgba(255,255,255,0.08)",
            color: net.status()?.running ? "#06111b" : FG,
            border: "none",
            "border-radius": "12px",
            padding: "4px 12px",
            "font-weight": 600,
            cursor: "pointer",
            "flex-shrink": 0,
          }}
          title="Shift+S toggles sharing"
        >
          {net.status()?.running ? "Sharing on" : "Sharing off"}
        </button>
        <div style={{ "font-size": "12px", color: DIM, "min-width": 0 }}>
          <Show
            when={net.status()?.running}
            fallback={
              <>
                Share my library on the network (Shift+S)
                <Show when={net.shareError()}>
                  {(err) => <div style={{ color: "#e3a14f" }}>Couldn't start sharing: {err()}</div>}
                </Show>
              </>
            }
          >
            Sharing as <b style={{ color: FG }}>{net.status()?.name}</b> at{" "}
            <span style={{ color: FG }}>{net.status()?.addresses.join(", ") || `port ${net.status()?.port}`}</span>
            <Show when={!net.status()?.discoverable}>
              <div>Other devices may need your address to connect</div>
            </Show>
          </Show>
        </div>
      </div>

      {/* Servers */}
      <div style={titleStyle}>
        Servers {net.discovering() ? "· searching…" : "· r refresh · + add · Del remove"}
      </div>
      <div style={{ ...boxStyle, "flex": "0 0 auto", "max-height": "28vh", "min-height": "64px" }}>
        <Show
          when={net.servers().length > 0}
          fallback={
            <div style={{ padding: "12px", color: DIM, "font-size": "13px" }}>
              {net.discovering()
                ? "Looking for other RockCrafts…"
                : "No other RockCraft found. Press + to add an address."}
            </div>
          }
        >
          <For each={net.servers()}>
            {(row, i) => (
              <div
                style={rowStyle(net.serverIdx() === i(), props.focus === "servers")}
                onClick={() => {
                  props.onFocus("servers");
                  net.selectServer(i());
                }}
              >
                <div style={{ flex: "1 1 auto", color: FG }}>{row.label}</div>
                <div style={{ color: DIM, "font-size": "12px" }}>
                  {row.manual ? "added" : row.addr}
                </div>
              </div>
            )}
          </For>
        </Show>
        <Show when={net.adding()}>
          <div style={{ display: "flex", gap: "8px", padding: "8px 12px" }}>
            <input
              ref={(el) => {
                addInput = el;
                queueMicrotask(() => el.focus());
              }}
              placeholder="192.168.1.5:27800"
              style={{
                flex: "1 1 auto",
                background: "#1a1c26",
                color: FG,
                border: `1px solid ${ACCENT}`,
                "border-radius": "6px",
                padding: "5px 8px",
              }}
              onKeyDown={(ev) => {
                if (ev.key === "Enter") {
                  ev.preventDefault();
                  net.addServer(addInput?.value ?? "");
                } else if (ev.key === "Escape") {
                  ev.preventDefault();
                  ev.stopPropagation();
                  net.setAdding(false);
                }
              }}
            />
          </div>
        </Show>
      </div>

      {/* Remote songs */}
      <div style={titleStyle}>
        {net.selectedServer() ? `Songs on ${net.selectedServer()?.label}` : "Songs"} · Enter download
      </div>
      <div style={{ ...boxStyle, flex: "1 1 auto" }}>
        <Show when={net.songsError()}>
          {(err) => <div style={{ padding: "12px", color: "#e3a14f", "font-size": "13px" }}>{err()}</div>}
        </Show>
        <Show when={!net.songsError() && net.songs().length === 0}>
          <div style={{ padding: "12px", color: DIM, "font-size": "13px" }}>
            {net.loadingSongs() ? "Loading…" : net.selectedServer() ? "No songs shared" : "Pick a server"}
          </div>
        </Show>
        <For each={net.songs()}>
          {(song, i) => (
            <div
              style={rowStyle(net.songIdx() === i(), props.focus === "songs")}
              onClick={() => {
                props.onFocus("songs");
                net.setSongIdx(i());
              }}
              onDblClick={() => net.downloadSong(i())}
            >
              <div
                style={{
                  flex: "1 1 auto",
                  color: FG,
                  "white-space": "nowrap",
                  overflow: "hidden",
                  "text-overflow": "ellipsis",
                }}
              >
                {song.name}
                {song.has_video ? " 🎬" : ""}
              </div>
              <Show when={localBadge(song.local)}>
                {(b) => (
                  <div
                    style={{
                      "font-size": "11px",
                      color: song.local === "same" ? ACCENT : "#e3a14f",
                      background: "rgba(255,255,255,0.06)",
                      padding: "2px 8px",
                      "border-radius": "10px",
                      "white-space": "nowrap",
                    }}
                  >
                    {b()}
                  </div>
                )}
              </Show>
              <div style={{ color: DIM, "font-size": "12px", "white-space": "nowrap" }}>
                {song.note_count} notes · {fmtMB(song.size_bytes)}
              </div>
            </div>
          )}
        </For>
      </div>
    </div>
  );
}

/** Progress of recent transfers, under the lists. */
export function TransferStrip(props: { net: Network }): JSX.Element {
  const recent = () => props.net.transfers().slice(-4).reverse();
  return (
    <Show when={recent().length > 0}>
      <div style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
        <For each={recent()}>
          {(t) => (
            <div style={{ display: "flex", "align-items": "center", gap: "10px", "font-size": "12px" }}>
              <div
                style={{
                  width: "160px",
                  height: "6px",
                  background: "rgba(255,255,255,0.08)",
                  "border-radius": "3px",
                  overflow: "hidden",
                  "flex-shrink": 0,
                }}
              >
                <div
                  style={{
                    width: `${t.state === "done" ? 100 : transferPercent(t)}%`,
                    height: "100%",
                    background: t.state === "failed" ? "#e35f4f" : ACCENT,
                  }}
                />
              </div>
              <div style={{ color: t.state === "running" ? FG : DIM }}>{transferLine(t)}</div>
            </div>
          )}
        </For>
      </div>
    </Show>
  );
}
