// Thin typed wrappers over the Tauri IPC bridge.
//
// These mirror the WebSocket control protocol's verbs (`run_action`,
// `query state`, `query help`) and the two push events (`snapshot`, `effects`)
// emitted by the backend tick thread. See `tauri-app/src-tauri/src/lib.rs` for
// the command/event definitions, and `crates/core/src/action.rs` for the
// authoritative action catalog.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  ActionInfo,
  ActionName,
  ActionParams,
  ActionReply,
  ComposerSnapshot,
  Effect,
  LibraryEntryDto,
} from "./types";

/** Event name the backend emits a fresh {@link ComposerSnapshot} on. */
const EVENT_SNAPSHOT = "snapshot";
/** Event name the backend emits a batch of {@link Effect}s on. */
const EVENT_EFFECTS = "effects";

/**
 * Apply a named action, returning its effects and the new snapshot.
 *
 * The backend also pushes a `snapshot` (and `effects`) event after applying, so
 * a UI listening via {@link onSnapshot} stays in sync even if it ignores this
 * return value.
 */
export function runAction(
  name: ActionName,
  params?: ActionParams,
): Promise<ActionReply> {
  return invoke<ActionReply>("run_action", { name, params: params ?? {} });
}

/** Fetch the current composer snapshot — mirrors `query state`. */
export function queryState(): Promise<ComposerSnapshot> {
  return invoke<ComposerSnapshot>("query_state");
}

/** Fetch the self-describing action catalog — mirrors `query help`. */
export function queryHelp(): Promise<ActionInfo[]> {
  return invoke<ActionInfo[]>("query_help");
}

/**
 * Subscribe to `snapshot` events. Returns the Tauri unlisten function; call it
 * in `onCleanup` per the SolidJS conventions.
 */
export function onSnapshot(
  cb: (snapshot: ComposerSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<ComposerSnapshot>(EVENT_SNAPSHOT, (e) => cb(e.payload));
}

/**
 * Subscribe to `effects` events. Returns the Tauri unlisten function; call it
 * in `onCleanup`.
 */
export function onEffects(cb: (effects: Effect[]) => void): Promise<UnlistenFn> {
  return listen<Effect[]>(EVENT_EFFECTS, (e) => cb(e.payload));
}

/**
 * Scan the default library roots and return a list of bundle DTOs, sorted by
 * name. Missing roots are silently skipped. Mirrors `scan_library` on the
 * Rust backend.
 */
export function scanLibrary(): Promise<LibraryEntryDto[]> {
  return invoke<LibraryEntryDto[]>("scan_library");
}

// ── Audio commands ────────────────────────────────────────────────────────

/** Audio status returned by {@link audioStatus}. */
export interface AudioStatus {
  /** Whether an audio output device is available on this machine. */
  device: boolean;
  /** The backing file name (no path) if one is attached; null otherwise. */
  backing: string | null;
}

/**
 * Attach a backing-track audio file (mp3/wav/ogg/flac).
 *
 * The file must already exist on disk (use the native file dialog before
 * calling this). Rejects with an error string if the file is missing.
 */
export function attachBacking(path: string): Promise<void> {
  return invoke<void>("attach_backing", { path });
}

/** Detach the current backing track (stops playback). */
export function detachBacking(): Promise<void> {
  return invoke<void>("detach_backing");
}

/** Poll current audio status (device availability + backing file name). */
export function audioStatus(): Promise<AudioStatus> {
  return invoke<AudioStatus>("audio_status");
}
