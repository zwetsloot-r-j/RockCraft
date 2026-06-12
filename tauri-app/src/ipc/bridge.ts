// Thin typed wrappers over the Tauri IPC bridge.
//
// These mirror the WebSocket control protocol's verbs (`run_action`,
// `query state`, `query help`) and the two push events (`snapshot`, `effects`)
// emitted by the backend tick thread. See `tauri-app/src-tauri/src/lib.rs` for
// the command/event definitions, and `crates/core/src/action.rs` for the
// authoritative action catalog.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open as dialogOpen } from "@tauri-apps/plugin-dialog";

import type {
  ActionInfo,
  ActionName,
  ActionParams,
  ActionReply,
  ComposerSnapshot,
  Effect,
  LibraryEntryDto,
  SaveBundleResult,
  SaveDest,
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

// ── Bundle save / load ───────────────────────────────────────────────────

/**
 * Save the current composer timeline to a bundle.
 *
 * `dest` selects the target:
 * - `{ kind: "quick_save" }` → `recordings/take-<stamp>/`
 * - `{ kind: "library", name: "..." }` → `<library_root>/<slug>/`
 *
 * Resolves to the bundle directory path on success, or rejects with a string
 * error. Clears the backend's dirty flag on success.
 */
export function saveBundle(dest: SaveDest): Promise<SaveBundleResult> {
  return invoke<SaveBundleResult>("save_bundle", { dest });
}

/**
 * Load a bundle from `dir` into the composer, replacing its current timeline.
 *
 * Reads `song.mid` (required) and `meta.json` (optional). Resolves to the new
 * composer snapshot so the UI can refresh immediately. Clears the dirty flag.
 */
export function loadBundle(dir: string): Promise<ComposerSnapshot> {
  return invoke<ComposerSnapshot>("load_bundle", { dir });
}

/**
 * Query whether the current timeline has unsaved changes.
 *
 * The `dirty` flag is also returned in every `ActionReply`; this function is
 * mainly useful for an initial query on screen mount.
 */
export function queryDirty(): Promise<boolean> {
  return invoke<boolean>("query_dirty");
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

// ── Import commands ────────────────────────────────────────────────────────

/**
 * Import input DTO — mirrors `import::ImportInputDto` on the Rust side.
 * The tag is `"kind"` and the content is `"value"`.
 */
export type ImportInputDto =
  | { kind: "File"; value: string }
  | { kind: "Url"; value: string };

/**
 * Progress event payload emitted by the backend during an import.
 * Mirrors `import::ImportProgressEvent`.
 */
export interface ImportProgressEvent {
  stage: "fetching" | "extracting" | "writing" | "done" | "failed";
  /** Fraction 0–1; only meaningful for `"extracting"`. */
  progress?: number;
  /** A single log line from the pipeline. */
  log?: string;
  /** Absolute path to the bundle directory; only present on `"done"`. */
  bundle_dir?: string;
  /** Human-readable error; only present on `"failed"`. */
  error?: string;
}

/**
 * Open a native file-picker dialog filtered to video files.
 * Returns the selected path, or `null` if the user cancelled.
 *
 * Video extensions: mp4 mkv avi mov webm flv wmv m4v (TUI parity).
 */
export async function openVideoFilePicker(): Promise<string | null> {
  const selected = await dialogOpen({
    multiple: false,
    filters: [
      {
        name: "Video files",
        extensions: ["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v"],
      },
    ],
  });
  if (selected === null || selected === undefined) return null;
  // `open()` returns a string when `multiple: false`.
  return typeof selected === "string" ? selected : null;
}

/** Returns `true` when a fetch command is configured for URL imports. */
export function importUrlAvailable(): Promise<boolean> {
  return invoke<boolean>("import_url_available");
}

/**
 * Start an import. Returns `Err("import already running")` if a concurrent
 * import is in progress; otherwise spawns the pipeline thread and resolves
 * immediately.
 */
export function importStart(input: ImportInputDto): Promise<void> {
  return invoke<void>("import_start", { input });
}

/**
 * Subscribe to `import_progress` events. Returns the unlisten function; call
 * it in `onCleanup`.
 */
export function onImportProgress(
  cb: (ev: ImportProgressEvent) => void,
): Promise<import("@tauri-apps/api/event").UnlistenFn> {
  return listen<ImportProgressEvent>("import_progress", (e) => cb(e.payload));
}
