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

import type { Screen } from "../shell/screens";
import type {
  ActionInfo,
  ActionName,
  ActionParams,
  ActionReply,
  BackgroundKeyframe,
  BackgroundTransform,
  ComposerSnapshot,
  Effect,
  LibraryEntryDto,
  PlayInfo,
  PlayStateEvent,
  PlaySummary,
  SaveBundleResult,
  SaveDest,
  SegmentSpec,
} from "./types";

/** Event name the backend emits a fresh {@link ComposerSnapshot} on. */
const EVENT_SNAPSHOT = "snapshot";
/** Notes-stripped snapshot for note-invariant actions (see {@link onMeta}). */
const EVENT_META = "meta";
/** Event name the backend emits a batch of {@link Effect}s on. */
const EVENT_EFFECTS = "effects";
/** Event name the backend emits a lightweight {@link PlayheadEvent} on during
 * playback (just the moving position + flag, no note list). */
const EVENT_PLAYHEAD = "playhead";
/** Event name the backend emits a live {@link PlayStateEvent} on (#168). */
const EVENT_PLAY_STATE = "play_state";
/**
 * Event name the backend emits a {@link Screen} on when an agent-driven control
 * request changes the active context ("auto-follow"). The shell {@link Router}
 * subscribes and navigates so a remote session is watchable.
 */
const EVENT_NAVIGATE = "navigate";

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
 * Subscribe to `meta` events: a snapshot with an **empty `notes` array**,
 * emitted for note-invariant actions (cursor moves, transport, grid tweaks)
 * instead of the heavy full snapshot. Merge every field except `notes` and
 * keep the existing note store — a dense chart is never re-serialised or
 * re-diffed per keystroke. Returns the Tauri unlisten function.
 */
export function onMeta(
  cb: (meta: ComposerSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<ComposerSnapshot>(EVENT_META, (e) => cb(e.payload));
}

/**
 * Subscribe to backend `navigate` events (auto-follow). Returns the Tauri
 * unlisten function; call it in `onCleanup`.
 */
export function onNavigate(cb: (screen: Screen) => void): Promise<UnlistenFn> {
  return listen<Screen>(EVENT_NAVIGATE, (e) => cb(e.payload));
}

/**
 * Subscribe to `effects` events. Returns the Tauri unlisten function; call it
 * in `onCleanup`.
 */
export function onEffects(cb: (effects: Effect[]) => void): Promise<UnlistenFn> {
  return listen<Effect[]>(EVENT_EFFECTS, (e) => cb(e.payload));
}

/** Lightweight playback push: the moving playhead + playing flag, without the
 * note list. Emitted ~30×/s during playback so the highway scrolls cheaply; the
 * full {@link ComposerSnapshot} is re-sent only when the notes change. */
export interface PlayheadEvent {
  playhead_us: number;
  playing: boolean;
}

/**
 * Subscribe to lightweight `playhead` events (playback scroll). Returns the
 * Tauri unlisten function; call it in `onCleanup`.
 */
export function onPlayhead(
  cb: (p: PlayheadEvent) => void,
): Promise<UnlistenFn> {
  return listen<PlayheadEvent>(EVENT_PLAYHEAD, (e) => cb(e.payload));
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

/**
 * Slice the loaded piece into the given kept parts (M10-C), writing each as a
 * new standalone library bundle via the shared `SplitBundle` write path (M10-B).
 * Each {@link SegmentSpec} is the half-open range `[start_us, end_us)` plus a
 * name; discarded parts are simply omitted by the caller (= trimming).
 *
 * Resolves to the created bundle directory paths, or rejects with a string
 * error. The source piece is left untouched (non-destructive — does not clear
 * the dirty flag).
 */
export function splitBundle(segments: SegmentSpec[]): Promise<string[]> {
  return invoke<string[]>("split_bundle", { segments });
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

// ── Sound selection + mixer (M14-C) ────────────────────────────────────────

/**
 * A synth voice: the notes you play, or the notes the song plays. Each has its
 * own instrument and level. Mirror of `core::SynthBus`.
 */
export type SynthBus = "player" | "song";

/**
 * Anything with a fader: the two synth voices plus the backing audio track
 * (which has a level but no instrument). Mirror of `core::MixerBus`.
 */
export type MixerBus = SynthBus | "backing";

/** One selectable sound. Mirror of `core::Instrument`. */
export interface Instrument {
  /** Stable id — what {@link setInstrument} takes and what we persist. */
  id: string;
  /** Display name for the dropdown. */
  name: string;
  /** General MIDI program number. */
  program: number;
}

/** One synth bus's settings. Mirror of `core::BusMix`. */
export interface BusMix {
  instrument: Instrument;
  /** Linear level, 0.0–1.0. */
  gain: number;
}

/**
 * The current mix plus the instrument catalog — the reply to every mixer call,
 * so one round trip renders the whole panel. Mirror of `core::MixerReport`.
 */
export interface MixerReport {
  player: BusMix;
  song: BusMix;
  /** The backing track's level, 0.0–1.0. */
  backing_gain: number;
  /** Every selectable instrument, in menu order — never hardcode this list. */
  instruments: Instrument[];
}

/** Read the current mix and the selectable-instrument catalog. */
export function mixerStatus(): Promise<MixerReport> {
  return invoke<MixerReport>("mixer_status");
}

/**
 * Point one synth bus at a catalog instrument by id. Rejects with an error
 * string for an unknown id. Returns the new mix.
 */
export function setInstrument(
  bus: SynthBus,
  instrument: string,
): Promise<MixerReport> {
  return invoke<MixerReport>("set_instrument", { bus, instrument });
}

/**
 * Set one bus's level. Values outside 0.0–1.0 are clamped by the backend.
 * Returns the new mix.
 */
export function setBusGain(bus: MixerBus, gain: number): Promise<MixerReport> {
  return invoke<MixerReport>("set_bus_gain", { bus, gain });
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

/** Toggle manual pause of the active take (Start / play-pause). Returns the new
 * paused state (`false` when no session is active). */
export function playTogglePause(): Promise<boolean> {
  return invoke<boolean>("play_toggle_pause");
}

/** Toggle input-monitor: synthesise the player's own key presses (`n`). Returns
 * the new state. */
export function playToggleMonitor(): Promise<boolean> {
  return invoke<boolean>("play_toggle_monitor");
}

/** Set the practiced hand: `"left"`, `"right"`, or `null` = both. Only that hand
 * is waited-on/scored; the other hand auto-plays. Returns the applied value. */
export function playSetPractice(
  hand: "left" | "right" | null,
): Promise<string> {
  return invoke<string>("play_set_practice", { hand });
}

/** Set the play-session practice speed in permille (1000 = 1x), clamped
 * 0.25x..=2x. The highway, wait gate and scoring stretch together; the backing
 * recording mutes off-tempo. Returns the applied value. */
export function playSetRate(ratePermille: number): Promise<number> {
  return invoke<number>("play_set_rate", { ratePermille });
}

/** Set the pitch dividing left/right hands (0..=127). Returns the applied value. */
export function playSetSplit(pitch: number): Promise<number> {
  return invoke<number>("play_set_split", { pitch });
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
  | { kind: "Url"; value: string }
  /**
   * A local score file (MusicXML and friends) — the M13-A path — or a scan/PDF,
   * which the same sidecar transcribes with an OMR engine first (M13-B). There is
   * deliberately no third kind: the sidecar decides which of the two an input is.
   */
  | { kind: "Score"; value: string };

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

// ── Grid alignment sidecar (M-align) ─────────────────────────────────────────

/**
 * Grid-calibration knobs persisted alongside the bundle as `alignment.json` so a
 * user's overlay alignment travels with the track (and survives Save-As). View
 * geometry only — the video path + time offset live in `meta.json`.
 */
export interface AlignmentDto {
  /** µs across the canvas height (vertical zoom). */
  span_us: number;
  /** Fraction of the span kept below the hit/now line. */
  hit_frac: number;
  /** Horizontal keyboard zoom (1 = full 88 across the width). */
  x_scale: number;
  /** Horizontal keyboard pan in px. */
  x_offset: number;
}

/**
 * Write the alignment sidecar (`alignment.json`) into the bundle `dir`. Called
 * after `save_bundle` resolves so the calibration is saved with the track.
 */
export function alignmentSave(dir: string, dto: AlignmentDto): Promise<void> {
  return invoke<void>("alignment_save", { dir, dto });
}

/**
 * Read the alignment sidecar for the bundle at `dir`, or `null` when none
 * exists. Called on `load_bundle` to restore the calibration.
 */
export function alignmentLoad(dir: string): Promise<AlignmentDto | null> {
  return invoke<AlignmentDto | null>("alignment_load", { dir });
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

// ── Background images, persisted in the bundle (M14-D) ───────────────────────

/**
 * A background image layer on the live editor — mirror of
 * `state::BackgroundRef`. `path` is absolute; wrap it with `convertFileSrc`
 * before assigning to an `<img>`. `transform` is evaluated by `core` at the
 * playhead — the webview never interpolates.
 */
export interface BackgroundRef {
  index: number;
  id: string;
  file: string;
  path: string;
  selected: boolean;
  transform: BackgroundTransform;
  keyframes: BackgroundKeyframe[];
}

/**
 * Open a native file-picker dialog filtered to image files.
 * Returns the selected path, or `null` if the user cancelled.
 */
export async function openImageFilePicker(): Promise<string | null> {
  const selected = await dialogOpen({
    multiple: false,
    filters: [
      {
        name: "Image files",
        extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif"],
      },
    ],
  });
  if (selected === null || selected === undefined) return null;
  return typeof selected === "string" ? selected : null;
}

/**
 * Attach a background image as the front-most layer and select it (M14-D).
 * Persisted into the bundle's `meta.json` on the next `save_bundle`.
 */
export function editAttachBackground(path: string): Promise<BackgroundRef[]> {
  return invoke<BackgroundRef[]>("edit_attach_background", { path });
}

/** Detach the background image layer with this id (M14-D). */
export function editDetachBackground(id: string): Promise<BackgroundRef[]> {
  return invoke<BackgroundRef[]>("edit_detach_background", { id });
}

/** Every background image layer with its transform at the playhead (M14-D). */
export function editQueryBackgrounds(): Promise<BackgroundRef[]> {
  return invoke<BackgroundRef[]>("edit_query_backgrounds");
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
