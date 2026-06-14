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
  PlayInfo,
  PlayStateEvent,
  PlaySummary,
  SaveBundleResult,
  SaveDest,
} from "./types";

/** Event name the backend emits a fresh {@link ComposerSnapshot} on. */
const EVENT_SNAPSHOT = "snapshot";
/** Event name the backend emits a batch of {@link Effect}s on. */
const EVENT_EFFECTS = "effects";
/** Event name the backend emits a live {@link PlayStateEvent} on (#168). */
const EVENT_PLAY_STATE = "play_state";

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

/**
 * Absolute directory of the newest bundle across the default scan roots, or
 * `null` when no bundles exist. Backs the menu's "Play/Edit last recording".
 * Mirrors `latest_recording` (returns Rust `Option<String>`).
 */
export function latestRecording(): Promise<string | null> {
  return invoke<string | null>("latest_recording");
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

// ── Edit-screen backing track, persisted in the bundle (M9-E) ───────────────

/**
 * A backing-track reference held on the live editor — mirror of
 * `state::BackingRef`. `path` is absolute; `name` is the bare file name for
 * display.
 */
export interface BackingRef {
  path: string;
  name: string;
}

/**
 * Open a native file-picker dialog filtered to audio files.
 * Returns the selected path, or `null` if the user cancelled.
 *
 * Audio extensions mirror the backing-track formats the playback path supports
 * (mp3 wav ogg flac m4a aac).
 */
export async function openBackingFilePicker(): Promise<string | null> {
  const selected = await dialogOpen({
    multiple: false,
    filters: [
      {
        name: "Audio files",
        extensions: ["mp3", "wav", "ogg", "flac", "m4a", "aac"],
      },
    ],
  });
  if (selected === null || selected === undefined) return null;
  return typeof selected === "string" ? selected : null;
}

/**
 * Attach (or replace) the edit-screen backing track (M9-E). Stored on the live
 * editor so the next `save_bundle` persists it into the bundle's
 * `meta.backing`, and handed to the playback thread so it plays under the
 * transport. Rejects if the file is missing.
 */
export function editSetBacking(path: string): Promise<void> {
  return invoke<void>("edit_set_backing", { path });
}

/** Detach the edit-screen backing track (M9-E). */
export function editClearBacking(): Promise<void> {
  return invoke<void>("edit_clear_backing");
}

/** Return the currently attached backing track, or `null` (M9-E). */
export function editQueryBacking(): Promise<BackingRef | null> {
  return invoke<BackingRef | null>("edit_query_backing");
}

// ── Record commands ───────────────────────────────────────────────────────────

/** Status of the live-recording session. */
export interface RecordStatus {
  /** Whether a session is currently active. */
  active: boolean;
  /** Elapsed time in microseconds (0 when inactive). */
  elapsed_us: number;
  /** Number of MIDI events captured so far (0 when inactive). */
  event_count: number;
}

/**
 * Start a new live-recording session.
 *
 * Pass `backing` (absolute path) to start backing audio immediately and
 * anchor the MIDI origin to the backing start time.
 *
 * Rejects if a session is already active or the backing file is missing.
 */
export function recordStart(backing?: string): Promise<void> {
  return invoke<void>("record_start", { backing: backing ?? null });
}

/**
 * Stop the current recording session without saving.
 *
 * The captured buffer is discarded. No-op when inactive.
 */
export function recordStop(): Promise<void> {
  return invoke<void>("record_stop");
}

/**
 * Save the current session as a take bundle.
 *
 * Returns the bundle directory path (e.g. `recordings/take-1234567/`).
 * Rejects if no session is active or the buffer is empty.
 */
export function recordSave(): Promise<string> {
  return invoke<string>("record_save");
}

/** Poll the current recording status. */
export function recordStatus(): Promise<RecordStatus> {
  return invoke<RecordStatus>("record_status");
}

// ── Play commands (#168) ──────────────────────────────────────────────────

/**
 * Load a bundle directory into a fresh play session and return its static info
 * (title, shifted spans, lead-in, backing presence). Rejects if `song.mid` is
 * missing or unparseable. Mirrors `play_load` on the Rust backend.
 */
export function playLoad(dir: string): Promise<PlayInfo> {
  return invoke<PlayInfo>("play_load", { dir });
}

/** Arm / disarm wait mode (`w` key). Returns the new armed state. */
export function playSetWait(on: boolean): Promise<boolean> {
  return invoke<boolean>("play_set_wait", { on });
}

/** Toggle "hear the song" audition (`m` key). Returns the new state. */
export function playToggleHearSong(): Promise<boolean> {
  return invoke<boolean>("play_toggle_hear_song");
}

/**
 * Finish the take: tear the session down (stop backing, silence the synth) and
 * return the end-of-take summary. Idempotent.
 */
export function playFinish(): Promise<PlaySummary> {
  return invoke<PlaySummary>("play_finish");
}

/**
 * Subscribe to `play_state` events (the ~60 Hz live highway snapshot). Returns
 * the Tauri unlisten function; call it in `onCleanup`.
 */
export function onPlayState(
  cb: (state: PlayStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<PlayStateEvent>(EVENT_PLAY_STATE, (e) => cb(e.payload));
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

// ── Transcription backdrop sidecar (M7-tauri-N) ─────────────────────────────

/**
 * The persisted video-backdrop reference — mirrors `transcription::TranscriptionDto`
 * on the Rust side. Stored next to a bundle as `transcription.json`, deliberately
 * outside the `core` bundle schema.
 */
export interface TranscriptionDto {
  /** Path to the backdrop video file (absolute). */
  video: string;
  /** Alignment offset in microseconds (`videoTime = songTime + offset`). */
  offset_us: number;
}

/**
 * Write the backdrop sidecar (`transcription.json`) into the bundle `dir`.
 * Called after `save_bundle` resolves when a backdrop is attached.
 */
export function transcriptionSave(
  dir: string,
  dto: TranscriptionDto,
): Promise<void> {
  return invoke<void>("transcription_save", { dir, dto });
}

/**
 * Read the backdrop sidecar for the bundle at `dir`, or `null` when none
 * exists. Called on `load_bundle` to re-attach the backdrop.
 */
export function transcriptionLoad(dir: string): Promise<TranscriptionDto | null> {
  return invoke<TranscriptionDto | null>("transcription_load", { dir });
}

// ── Background video, persisted in the bundle (M9-G) ─────────────────────────

/**
 * A background-video reference held on the live editor — mirror of
 * `state::VideoRef`. `path` is absolute; wrap it with `convertFileSrc` before
 * assigning to a `<video>` element.
 */
export interface VideoRef {
  path: string;
  offset_us: number;
}

/**
 * Attach (or replace) the edit-screen background video. Persisted into the
 * bundle's `meta.json` on the next `save_bundle` (M9-G).
 */
export function editSetVideo(path: string, offsetUs: number): Promise<void> {
  return invoke<void>("edit_set_video", { path, offsetUs });
}

/** Update only the alignment offset of the attached background video (M9-G). */
export function editSetVideoOffset(offsetUs: number): Promise<void> {
  return invoke<void>("edit_set_video_offset", { offsetUs });
}

/** Detach the edit-screen background video (M9-G). */
export function editClearVideo(): Promise<void> {
  return invoke<void>("edit_clear_video");
}

/** Return the currently attached background video, or `null` (M9-G). */
export function editQueryVideo(): Promise<VideoRef | null> {
  return invoke<VideoRef | null>("edit_query_video");
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
