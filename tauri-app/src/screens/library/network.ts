/**
 * Pure helpers for the library's Network panel (M26-D): settings that
 * persist, address validation, and how transfers and remote songs read.
 */
import type { ShareLocal, ShareTransfer } from "../../ipc/types";

const ENABLED_KEY = "rockcraft.share.enabled";
const SERVERS_KEY = "rockcraft.share.servers";

/** Whether sharing was left on (read at app start). False when unknown. */
export function loadShareEnabled(): boolean {
  try {
    return localStorage.getItem(ENABLED_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remember the sharing toggle. Best-effort. */
export function saveShareEnabled(on: boolean): void {
  try {
    localStorage.setItem(ENABLED_KEY, on ? "1" : "0");
  } catch {
    // Storage unavailable: the toggle just isn't remembered.
  }
}

/** Manually added server addresses, in the order added. */
export function loadServers(): string[] {
  try {
    const raw = localStorage.getItem(SERVERS_KEY);
    const parsed: unknown = raw === null ? [] : JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((s): s is string => typeof s === "string" && parseServer(s) !== null)
      : [];
  } catch {
    return [];
  }
}

/** Remember the manual server addresses. Best-effort. */
export function saveServers(servers: string[]): void {
  try {
    localStorage.setItem(SERVERS_KEY, JSON.stringify(servers));
  } catch {
    // Storage unavailable: the list lasts for this session only.
  }
}

/**
 * Normalise a typed server address, or null when it isn't `host:port` with a
 * port of 1–65535. A bare host gets the default share port 47800. IPv6 must
 * be bracketed: `[::1]:47800`.
 */
export function parseServer(input: string): string | null {
  const text = input.trim();
  if (text.length === 0 || /\s/.test(text)) return null;
  let host: string;
  let port: string | undefined;
  if (text.startsWith("[")) {
    const close = text.indexOf("]");
    if (close < 2) return null;
    host = text.slice(0, close + 1);
    const rest = text.slice(close + 1);
    if (rest.length > 0) {
      if (!rest.startsWith(":")) return null;
      port = rest.slice(1);
    }
  } else {
    const parts = text.split(":");
    if (parts.length > 2) return null;
    host = parts[0];
    port = parts[1];
    if (!/^[A-Za-z0-9.-]+$/.test(host)) return null;
  }
  const p = port === undefined ? 47800 : Number(port);
  if (!Number.isInteger(p) || p < 1 || p > 65535 || (port !== undefined && !/^\d+$/.test(port))) {
    return null;
  }
  return `${host}:${p}`;
}

/** Bytes as megabytes, one decimal under 10 MB: `"3.4 MB"`, `"146 MB"`. */
export function fmtMB(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

/** Progress as a 0–100 integer (0 while the size is unknown). */
export function transferPercent(t: Pick<ShareTransfer, "bytes_done" | "bytes_total">): number {
  if (t.bytes_total <= 0) return 0;
  return Math.min(100, Math.floor((t.bytes_done / t.bytes_total) * 100));
}

/** One-line transfer summary: `↓ frog-song  12.0 / 48 MB  25%`. */
export function transferLine(t: ShareTransfer): string {
  const arrow = t.direction === "down" ? "↓" : "↑";
  const size = `${fmtMB(t.bytes_done)} / ${fmtMB(t.bytes_total)}`;
  switch (t.state) {
    case "running":
      return `${arrow} ${t.name}  ${size}  ${transferPercent(t)}%`;
    case "done":
      return `${arrow} ${t.name}  done`;
    case "failed":
      return `${arrow} ${t.name}  failed: ${t.detail}`;
    case "exists":
      return `${arrow} ${t.name}  name taken (${t.slug})`;
  }
}

/** The badge a remote song shows, or null for none. */
export function localBadge(local: ShareLocal): string | null {
  switch (local) {
    case "same":
      return "in library";
    case "different":
      return "different version";
    case "absent":
      return null;
  }
}
