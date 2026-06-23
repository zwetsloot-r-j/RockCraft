// EditScreen.tsx — the composer edit screen: a status bar over a canvas
// piano-roll grid. A faithful port of `crates/tui/src/edit.rs`'s view + keymap:
// it renders the latest `ComposerSnapshot` and dispatches every key through
// `run_action` (no edit logic here). Snapshots arrive on the bridge's `snapshot`
// event (primed by `queryState` on mount); while playing, a RAF interpolates the
// playhead between events for a smooth sweep.
//
// #165 additions:
//   - ChordSelectorPanel overlay when `chord_preview` is non-null.
//   - HelpOverlay toggled by `?`.
//   - DirtyExitPrompt shown when Esc is pressed with unsaved changes.
//   - SaveAsPrompt for `S` (save to library).
//   - `save_flash` one-shot confirmation toast after a successful save.
//   - `load_bundle` on mount when the router payload contains `dir`.
//   - Dirty tracking via the `dirty` field returned in `ActionReply`.

import {
  createEffect,
  createMemo,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from "solid-js";
import { createStore } from "solid-js/store";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  alignmentLoad,
  alignmentSave,
  editClearBacking,
  editClearVideo,
  editQueryBacking,
  editQueryVideo,
  editSetBacking,
  editSetVideo,
  editSetVideoOffset,
  loadBundle,
  onSnapshot,
  openBackingFilePicker,
  openVideoFilePicker,
  queryDirty,
  queryState,
  runAction,
  saveBundle,
  splitBundle,
  transcriptionLoad,
} from "../../ipc/bridge";
import type { AlignmentDto } from "../../ipc/bridge";
import type { ComposerSnapshot } from "../../ipc/types";
import { onMidiEvent } from "../../ipc/midi";
import { EditCanvas } from "./EditCanvas";
import { StatusBar } from "./StatusBar";
import { RecordControls } from "./RecordControls";
import { resolveKey } from "./keymap";
import { stepUs } from "./viewport";
import {
  keptSegmentSpecs,
  pieceLengthUs,
  reconcileMeta,
  segmentsFromSplits,
  type Segment,
  type SegmentMeta,
} from "./split";
import { noteName } from "../highway/utils";
import { useRouter } from "../../shell/Router";

// ── Video backdrop (M7-tauri-N) ────────────────────────────────────────────

/** Fine backdrop-offset nudge (10 ms), bound to `,` / `.` while attached. */
const BACKDROP_NUDGE_FINE_US = 10_000;
/** Coarse backdrop-offset nudge (250 ms), bound to `;` / `'` while attached. */
const BACKDROP_NUDGE_COARSE_US = 250_000;
/** Opacity the backdrop `<video>` renders at (dimmed under the grid). */
const BACKDROP_OPACITY = 0.5;

// ── Backdrop calibration (M-align) ─────────────────────────────────────────
// The edit grid is the authoritative note-space (fixed: 88 keys across the
// width, hit line ⅓ up, 16 s of vertical zoom). To let the user *register* an
// imported movie under that grid, we apply a per-video affine transform to the
// `<video>` element: screenPos = off + scale · containerPos (origin top-left),
// independently on X (keyboard alignment) and Y (note density + hit-line). Time
// alignment reuses the existing backdrop `offset_us`; review playback scrubs the
// overlay+video at a chosen rate. All of this is a Tauri *view* concern — it
// never touches `core` — and persists per video path in localStorage.
/** µs spanned across the canvas height at default zoom (mirrors EditCanvas). */
const DEFAULT_SPAN_US = 16_000_000;
interface VideoCal {
  /** Vertical zoom: µs across the canvas height (smaller = zoomed in). */
  spanUs: number;
  /** Hit-line position: fraction of the span kept below the now-line. */
  hitFrac: number;
  /** Horizontal keyboard zoom (1 = full 88 across the width). */
  xScale: number;
  /** Horizontal keyboard pan in px. */
  xOffset: number;
  /** Time alignment (µs); mirrors the backdrop `offset_us`. */
  offsetUs: number;
}
const DEFAULT_CAL: VideoCal = {
  spanUs: DEFAULT_SPAN_US,
  hitFrac: 1 / 3,
  xScale: 1,
  xOffset: 0,
  offsetUs: 0,
};
const CAL_STORE_PREFIX = "rockcraft.videocal:";
function loadVideoCal(path: string): VideoCal | null {
  try {
    const j = localStorage.getItem(CAL_STORE_PREFIX + path);
    if (j) return { ...DEFAULT_CAL, ...(JSON.parse(j) as Partial<VideoCal>) };
  } catch {
    /* ignore malformed / unavailable storage */
  }
  return null;
}
function saveVideoCal(path: string, c: VideoCal): void {
  try {
    localStorage.setItem(CAL_STORE_PREFIX + path, JSON.stringify(c));
  } catch {
    /* ignore quota / unavailable storage */
  }
}
/** Review-playback speeds (frontend-driven scrub of overlay over the movie). */
const REVIEW_RATES = [0.1, 0.25, 0.5, 1, 2];
/** Grid-calibration nudge steps. */
const GRID_ZOOM_STEP = 1.06; // multiplicative vertical zoom per press
const GRID_SPAN_MIN = 1_000_000;
const GRID_SPAN_MAX = 64_000_000;
const HITFRAC_STEP = 0.02;
const XSCALE_STEP = 0.03;
const XOFF_STEP_PX = 6;
/** Together-scrub steps for ↑/↓ (fine) and Shift+↑/↓ (coarse). */
const SCRUB_STEP_US = 500_000;
const SCRUB_STEP_COARSE_US = 5_000_000;
/** Coarse movie-only offset step for [ / ] (find the first note quickly). */
const OFFSET_COARSE_S_US = 1_000_000;

// ── Overlay state ─────────────────────────────────────────────────────────

/** What top-level overlay is currently active (at most one at a time). */
type Overlay =
  | "none"
  | "help"
  /** "Save / Discard / Cancel" shown on dirty Esc. */
  | "exit-prompt"
  /** Text input for the save-to-library name. */
  | "save-as"
  /** Numeric input for the absolute set-BPM (tempo) value. */
  | "set-bpm"
  /** Video-backdrop alignment: keyboard X, density/hit-line Y, time, speed. */
  | "calibrate";

// ── Chord selector helpers ────────────────────────────────────────────────

/** MIDI pitch → note name (C, C#, …). */
function pitchToName(p: number): string {
  return noteName(p);
}

// ── Component ─────────────────────────────────────────────────────────────

interface Props {
  /** Bundle directory to load on mount (from the router payload). */
  dir?: string;
  /**
   * Open already armed for recording (StepRecord) — the "New piece" entry
   * point (M9-A). Arms once on mount via the same `toggle_record_arm` action
   * the `R` key dispatches.
   */
  armed?: boolean;
}

export function EditScreen(props: Props): JSX.Element {
  const { navigate } = useRouter();
  let canvasEl!: HTMLCanvasElement;
  let videoEl: HTMLVideoElement | undefined;
  let engine: EditCanvas | null = null;
  let raf = 0;

  // ── Video backdrop state (M7-tauri-N) ───────────────────────────────────
  // The attached backdrop path (null = none) and its alignment offset, applied
  // as videoTime = songTime + offset. State lives here (frontend-only); it is
  // persisted via the `transcription.json` sidecar, never the bundle schema.
  const [videoPath, setVideoPath] = createSignal<string | null>(null);
  const [offsetUs, setOffsetUs] = createSignal(0);

  // ── Grid calibration + review playback (M-align) ────────────────────────
  // The edit grid (note-space) is *resized* to register an imported movie under
  // it — vertical zoom (spanUs) + hit-line position (hitFrac), horizontal
  // keyboard zoom/pan (xScale/xOffset). This doubles as a general zoom. Review
  // playback sweeps a frontend head at `reviewRate` so drift is visible without
  // touching the core transport. The movie stays put; only the grid moves.
  const [gridSpanUs, setGridSpanUs] = createSignal(DEFAULT_SPAN_US);
  const [gridHitFrac, setGridHitFrac] = createSignal(1 / 3);
  const [gridXScale, setGridXScale] = createSignal(1);
  const [gridXOffset, setGridXOffset] = createSignal(0);
  /** Review inspection head (µs); null = follow cursor/playhead normally. */
  const [reviewUs, setReviewUs] = createSignal<number | null>(null);
  /** Whether the review head auto-advances (▶) vs sits paused at a position. */
  const [reviewPlaying, setReviewPlaying] = createSignal(false);
  const [reviewRate, setReviewRate] = createSignal(1);
  // Wall-clock anchor for the review sweep (re-seated on play / rate change).
  let reviewBaseUs = 0;
  let reviewBaseWall = 0;

  // ── Backing audio track (M9-E; swap M10-E) ────────────────────────────────
  // The attached backing file name (null = none). The relocation of the former
  // top-level "Choose backing track" menu item into this screen: `B` opens the
  // native audio picker to attach or **replace** the file, `Backspace` detaches,
  // and the choice is persisted into the loaded piece's meta.backing by
  // `save_bundle` (the backend holds the path). The backing and the background
  // video are independent: swapping/detaching one never disturbs the other.
  const [backingName, setBackingName] = createSignal<string | null>(null);

  // The snapshot mirror.
  const [store, setStore] = createStore<{ snap: ComposerSnapshot | null }>({
    snap: null,
  });
  const [, setRev] = createSignal(0);

  // Dirty flag — mirrors the backend's AppState::dirty.
  const [dirty, setDirty] = createSignal(false);

  // ── Split / trim editor (M10-C) ──────────────────────────────────────────
  // When open, the user drops split markers along the timeline (at the playhead/
  // cursor), dividing the piece into consecutive segments; each is kept (named)
  // or discarded, and "Save parts" writes the kept ones as standalone bundles
  // via the `SplitBundle` host command. All ephemeral UI state — no persistence.
  const [splitMode, setSplitMode] = createSignal(false);
  // Interior split points in song-time µs (0 < m < total), kept sorted + unique.
  const [markers, setMarkers] = createSignal<number[]>([]);
  // Per-segment keep/name, parallel to `segments()` and reconciled by index.
  const [meta, setMeta] = createStore<{ list: SegmentMeta[] }>({ list: [] });

  /** Total piece length in µs — the span the segments cover. */
  const totalUs = createMemo(() => pieceLengthUs(store.snap?.notes ?? []));
  /** Consecutive segments the markers imply — mirrors `segments_from_splits`. */
  const segments = createMemo(() => segmentsFromSplits(markers(), totalUs()));

  // Keep `meta.list` the same length as the derived segments, preserving each
  // keep/name choice by index when a marker is added or removed. The same-length
  // case returns the existing array unchanged so editing notes mid-playback does
  // not churn the keep/name state.
  createEffect(() => {
    const n = segments().length;
    setMeta("list", (prev) => (prev.length === n ? prev : reconcileMeta(prev, n)));
  });

  // ── Live-capture feedback (M9-A) ─────────────────────────────────────────
  // Input activity level (0..1), driven by played note-on velocity and decayed
  // each animation frame — the record-mode level meter folded in from the
  // standalone RecordScreen so the recording mode keeps its activity feedback.
  const [inputLevel, setInputLevel] = createSignal(0);

  // Overlay state.
  const [overlay, setOverlay] = createSignal<Overlay>("none");

  // Save-as prompt: name typed so far.
  const [saveName, setSaveName] = createSignal("");

  // Set-BPM prompt: digits typed so far (seeded with the current tempo on open).
  const [bpmText, setBpmText] = createSignal("");

  // One-shot save-confirmation toast, cleared after 2.5 s.
  const [saveFlash, setSaveFlash] = createSignal<string | null>(null);
  let flashTimeout = 0;

  // Wall-clock anchor for playhead interpolation.
  let basePlayheadUs = 0;
  let baseWallMs = 0;

  function interpolatedPlayheadUs(s: ComposerSnapshot): number {
    if (!s.playing) return s.playhead_us;
    const elapsedUs = (performance.now() - baseWallMs) * 1000;
    return basePlayheadUs + elapsedUs;
  }

  /**
   * The song-time line the viewport scrolls around — the playhead while
   * playing, else the cursor step. Mirrors `EditCanvas`'s own anchor so the
   * backdrop frame lines up with the on-screen grid.
   */
  function anchorUsOf(s: ComposerSnapshot): number {
    if (s.playing) return interpolatedPlayheadUs(s);
    return s.cursor.step * stepUs(s.bpm, s.subdivision);
  }

  /**
   * Scrub the backdrop `<video>` to the song time at the reference line. We
   * *seek* rather than `play()` so the frame stays in lockstep with the
   * playhead (no audio, no drift); clamped to the clip's duration.
   */
  /**
   * Activate the backdrop `<video>` once after (re)attaching a source. WebView2
   * (and some other engines) will not actually *seek* — decode and repaint a new
   * frame — for a media element that has been `load()`-ed but never played: the
   * `currentTime` setter is silently ignored and the element stays pinned on its
   * first decoded frame. A single muted `play()`→`pause()` activates the media
   * pipeline so subsequent scrubs take effect. Muted, so autoplay is permitted;
   * paused again immediately so we still drive the frame purely by seeking.
   */
  function primeBackdrop(): void {
    const v = videoEl;
    if (!v) return;
    const activate = (): void => {
      const p = v.play();
      if (p && typeof p.then === "function") {
        p.then(() => {
          v.pause();
          syncVideo(currentHeadUs()); // re-seat the frame the play() nudged off
        }).catch(() => {
          /* autoplay blocked — seeking may still work; nothing else to do */
        });
      } else {
        v.pause();
        syncVideo(currentHeadUs());
      }
    };
    if (v.readyState >= 2) {
      activate();
    } else {
      v.addEventListener("loadeddata", activate, { once: true });
    }
  }

  function syncVideo(anchorUs: number): void {
    const v = videoEl;
    if (!v || videoPath() === null) return;
    const dur = v.duration;
    if (!Number.isFinite(dur) || dur <= 0) return; // metadata not ready yet
    const t = (anchorUs + offsetUs()) / 1e6;
    v.currentTime = Math.min(Math.max(t, 0), dur);
  }

  function render(): void {
    const s = store.snap;
    if (!engine || !s) return;
    engine.setBackdrop(videoPath() !== null);
    // Overlay the split markers + discarded shading while the editor is open so
    // the cuts are readable over the grid and video backdrop.
    engine.setSplits(
      splitMode()
        ? segments().map((seg, i) => ({
            ...seg,
            keep: meta.list[i]?.keep ?? true,
          }))
        : null,
    );
    // While reviewing, a frontend-only head drives both the grid scroll and the
    // video scrub so the overlay and movie advance together at `reviewRate`.
    const head = reviewUs();
    if (head !== null) {
      engine.draw(s, head, head);
      syncVideo(head);
      return;
    }
    const playhead = interpolatedPlayheadUs(s);
    engine.draw(s, playhead);
    syncVideo(anchorUsOf(s));
  }

  function applySnapshot(s: ComposerSnapshot): void {
    basePlayheadUs = s.playhead_us;
    baseWallMs = performance.now();
    setStore("snap", s);
    setRev((r) => r + 1);
    render();
  }

  // ── Grid calibration + review playback (M-align) ────────────────────────

  /** Last note's end time (µs), for bounding the review sweep. */
  function songEndUs(): number {
    const notes = store.snap?.notes ?? [];
    let end = 0;
    for (const n of notes) end = Math.max(end, n.start_us + n.dur_us);
    return end;
  }

  /** Re-seat the review wall-clock anchor to the current head (avoids a jump on
   * play / rate change). */
  function reseatReview(fromUs: number): void {
    reviewBaseUs = fromUs;
    reviewBaseWall = performance.now();
  }

  /** The current inspection time: review head if set, else the cursor step. */
  function currentHeadUs(): number {
    const r = reviewUs();
    if (r !== null) return r;
    const s = store.snap;
    return s ? s.cursor.step * stepUs(s.bpm, s.subdivision) : 0;
  }

  /** Open the ALIGN overlay, seeding a shared inspection head so ↑/↓ scrub from
   * a stable position (the first note, where alignment matters) instead of
   * teleporting the movie back to the cursor / first frame. */
  function openCalibrate(): void {
    if (reviewUs() === null) {
      const notes = store.snap?.notes ?? [];
      let seed = 0;
      if (notes.length > 0) {
        seed = notes.reduce((m, n) => Math.min(m, n.start_us), Infinity);
      } else {
        const s = store.snap;
        seed = s ? s.cursor.step * stepUs(s.bpm, s.subdivision) : 0;
      }
      setReviewUs(seed);
    }
    setOverlay("calibrate");
    render();
  }

  /** Play/pause the together-sweep (movie + overlay advance in lock-step). */
  function toggleReviewPlay(): void {
    if (reviewPlaying()) {
      setReviewPlaying(false);
      render();
      return;
    }
    const from = currentHeadUs();
    const seed = from >= songEndUs() ? 0 : from;
    setReviewUs(seed);
    reseatReview(seed);
    setReviewPlaying(true);
  }

  /** Manual together-scrub by `deltaUs` (movie + overlay move together). */
  function scrubTogether(deltaUs: number): void {
    const next = Math.max(0, currentHeadUs() + deltaUs);
    setReviewUs(next);
    if (reviewPlaying()) reseatReview(next);
    render();
  }

  /** Leave review mode entirely → grid follows the cursor/playhead again. */
  function clearReview(): void {
    setReviewPlaying(false);
    setReviewUs(null);
    render();
  }

  /** Step the review rate up/down through REVIEW_RATES. */
  function changeReviewRate(dir: number): void {
    const cur = reviewRate();
    let idx = REVIEW_RATES.indexOf(cur);
    if (idx < 0) idx = REVIEW_RATES.indexOf(1);
    idx = Math.max(0, Math.min(REVIEW_RATES.length - 1, idx + dir));
    setReviewRate(REVIEW_RATES[idx]);
    // Re-seat so the new rate takes effect from the current head, no jump.
    if (reviewPlaying()) reseatReview(reviewUs() ?? 0);
  }

  /** Multiplicatively zoom the vertical span (density), clamped. */
  function zoomGrid(factor: number): void {
    setGridSpanUs((v) =>
      Math.max(GRID_SPAN_MIN, Math.min(GRID_SPAN_MAX, v * factor)),
    );
  }

  /** Reset the grid calibration + time offset to defaults. */
  function resetCalibration(): void {
    setGridSpanUs(DEFAULT_SPAN_US);
    setGridHitFrac(1 / 3);
    setGridXScale(1);
    setGridXOffset(0);
    setOffsetUs(0);
    void editSetVideoOffset(0).catch(() => {});
    render();
  }

  // Push grid calibration into the canvas and repaint whenever a knob changes.
  createEffect(() => {
    const cal = {
      spanUs: gridSpanUs(),
      hitLineFrac: gridHitFrac(),
      xScale: gridXScale(),
      xOffset: gridXOffset(),
    };
    if (engine) {
      engine.setGridCal(cal);
      render();
    }
  });

  // Persist calibration per video path whenever any knob changes.
  createEffect(() => {
    const p = videoPath();
    const cal: VideoCal = {
      spanUs: gridSpanUs(),
      hitFrac: gridHitFrac(),
      xScale: gridXScale(),
      xOffset: gridXOffset(),
      offsetUs: offsetUs(),
    };
    if (p) saveVideoCal(p, cal);
  });

  /** Attach a backdrop video and restore its saved calibration (falling back to
   * the backend's persisted time offset when no local calibration exists). */
  function applyVideoRef(
    path: string,
    backendOffsetUs: number,
    sidecar: AlignmentDto | null,
  ): void {
    // Grid-calibration precedence: bundle sidecar (travels with the track) wins,
    // else this machine's localStorage scratch, else defaults.
    if (sidecar) {
      setGridSpanUs(sidecar.span_us);
      setGridHitFrac(sidecar.hit_frac);
      setGridXScale(sidecar.x_scale);
      setGridXOffset(sidecar.x_offset);
    } else {
      const local = loadVideoCal(path);
      setGridSpanUs(local?.spanUs ?? DEFAULT_SPAN_US);
      setGridHitFrac(local?.hitFrac ?? 1 / 3);
      setGridXScale(local?.xScale ?? 1);
      setGridXOffset(local?.xOffset ?? 0);
    }
    // The time offset is authoritative in meta.json (the backend already
    // restored it on load_bundle); the sidecar carries grid geometry only.
    setOffsetUs(backendOffsetUs);
    setVideoPath(path); // last: the src-load effect fires with cal already set
  }

  /** The current grid calibration as a sidecar DTO (for save). */
  function currentAlignmentDto(): AlignmentDto {
    return {
      span_us: gridSpanUs(),
      hit_frac: gridHitFrac(),
      x_scale: gridXScale(),
      x_offset: gridXOffset(),
    };
  }

  /** Re-attach the backend's current backdrop video + backing track to the view.
   * Runs on *every* entry to the edit screen (load-by-dir AND socket/queryState
   * load), so a bundle loaded over the control socket still shows its movie. The
   * grid calibration comes from the bundle's `alignment.json` sidecar when the
   * dir is known (GUI load); the socket path falls back to localStorage. */
  function reattachBackdropAndBacking(dir: string | undefined): void {
    void editQueryBacking()
      .then((ref) => setBackingName(ref ? ref.name : null))
      .catch(() => {
        /* backend down — leave the indicator hidden */
      });
    const loadAlign: Promise<AlignmentDto | null> = dir
      ? alignmentLoad(dir).catch(() => null)
      : Promise.resolve(null);
    void loadAlign
      .then((sidecar) =>
        editQueryVideo().then((ref) => {
          if (ref) {
            applyVideoRef(ref.path, ref.offset_us, sidecar);
            return;
          }
          // Legacy transcription.json sidecar fallback (needs the bundle dir).
          if (!dir) return;
          return transcriptionLoad(dir).then((dto) => {
            if (dto) {
              applyVideoRef(dto.video, dto.offset_us, sidecar);
              void editSetVideo(dto.video, dto.offset_us).catch(() => {});
            }
          });
        }),
      )
      .catch(() => {
        /* backend down — leave the backdrop detached */
      });
  }

  // ── Save / load actions ─────────────────────────────────────────────────

  function showFlash(msg: string): void {
    setSaveFlash(msg);
    clearTimeout(flashTimeout);
    flashTimeout = window.setTimeout(() => setSaveFlash(null), 2500);
  }

  function doQuickSave(): void {
    // The background video (if any) is persisted into the bundle's meta.json by
    // `save_bundle` itself now (M9-G) — the backend holds the reference, set via
    // editSetVideo/editSetVideoOffset/editClearVideo.
    saveBundle({ kind: "quick_save" })
      .then((dir) => {
        setDirty(false);
        showFlash(`saved → ${dir}`);
        persistAlignment(dir);
      })
      .catch((e: unknown) => {
        showFlash(`save failed: ${String(e)}`);
      });
  }

  function doSaveAs(name: string): void {
    saveBundle({ kind: "library", name })
      .then((dir) => {
        setDirty(false);
        showFlash(`saved to library → ${dir}`);
        persistAlignment(dir);
      })
      .catch((e: unknown) => {
        showFlash(`save failed: ${String(e)}`);
      });
  }

  /** Write the grid calibration into the just-saved bundle so the alignment
   * travels with the track (the time offset is already in meta.json). */
  function persistAlignment(dir: string): void {
    if (videoPath() === null) return;
    void alignmentSave(dir, currentAlignmentDto()).catch(() => {
      /* non-fatal: the bundle still saved, alignment just isn't persisted */
    });
  }

  // ── Record arm / flavour (M9-A) ──────────────────────────────────────────
  // The unified screen toggles between editing (DirectEdit) and recording
  // (StepRecord / LiveRecord) via the existing core actions — no new action.
  // These wrap `run_action` so the on-screen Record⇄Edit control and the `R` /
  // `t` keys drive the exact same path.

  /** Dispatch `toggle_record_arm` (arm ⇄ disarm), then refresh dirty state. */
  function toggleRecordArm(): void {
    void runAction("toggle_record_arm").then((reply) => setDirty(reply.dirty));
  }

  /** Dispatch `toggle_record_flavour` (step ⇄ live; no-op while disarmed). */
  function toggleRecordFlavour(): void {
    void runAction("toggle_record_flavour").then((reply) =>
      setDirty(reply.dirty),
    );
  }

  // ── Video backdrop (M7-tauri-N) ─────────────────────────────────────────

  /** Open the native picker and attach the chosen video as the backdrop.
   * The reference is pushed to the backend so `save_bundle` persists it into the
   * bundle's `meta.json` (M9-G — promotes N's sidecar to a first-class field). */
  function attachBackdrop(): void {
    void openVideoFilePicker()
      .then((path) => {
        if (path === null) return; // cancelled
        setVideoPath(path);
        setOffsetUs(0);
        void editSetVideo(path, 0).catch(() => {
          /* backend down (plain vite) — UI state still reflects the pick */
        });
      })
      .catch(() => {
        showFlash("could not open video picker");
      });
  }

  /** Clear the backdrop and reset its offset (both locally and on the backend). */
  function detachBackdrop(): void {
    setVideoPath(null);
    setOffsetUs(0);
    void editClearVideo().catch(() => {
      /* backend down — UI state already cleared */
    });
  }

  /** `V` toggles the backdrop: pick when none, detach when attached. */
  function toggleBackdrop(): void {
    if (videoPath() === null) attachBackdrop();
    else detachBackdrop();
  }

  // ── Backing audio track (M9-E) ──────────────────────────────────────────

  /** Open the native audio picker and attach (or **replace**) the backing track
   * for the loaded piece (M9-E; swap added in M10-E). The path is pushed to the
   * backend so `save_bundle` persists it into the bundle's meta.backing and the
   * playback thread plays it under the transport (reusing attach_backing). When
   * a track is already attached this swaps the file in place; the attached
   * background video is independent state, so it stays visible/playing
   * throughout. Marks dirty. */
  function attachBacking(): void {
    void openBackingFilePicker()
      .then((path) => {
        if (path === null) return; // cancelled
        editSetBacking(path)
          .then(() => {
            const name = path.split(/[\\/]/).pop() ?? path;
            setBackingName(name);
            setDirty(true);
            showFlash(`backing → ${name}`);
          })
          .catch((e: unknown) => {
            showFlash(`backing failed: ${String(e)}`);
          });
      })
      .catch(() => {
        showFlash("could not open audio picker");
      });
  }

  /** Detach the backing track (stops playback, drops it from the next save).
   * Clears only the audio — the background video, if any, is untouched (M10-E). */
  function detachBacking(): void {
    setBackingName(null);
    setDirty(true);
    void editClearBacking().catch(() => {
      /* backend down — UI state already cleared */
    });
  }

  /** Nudge the alignment offset and re-sync the frame immediately. The new
   * offset is pushed to the backend so a later save persists it (M9-G). */
  function nudgeOffset(deltaUs: number): void {
    const next = offsetUs() + deltaUs;
    setOffsetUs(next);
    const s = store.snap;
    if (s) syncVideo(anchorUsOf(s));
    if (videoPath() !== null) {
      void editSetVideoOffset(next).catch(() => {
        /* backend down — UI state still reflects the offset */
      });
    }
  }

  // Bind the `<video>` src to the attached path (via the asset protocol) and
  // repaint so the grid dim toggles with it.
  createEffect(() => {
    const p = videoPath();
    engine?.setBackdrop(p !== null);
    if (videoEl) {
      if (p !== null) {
        videoEl.src = convertFileSrc(p);
        videoEl.load();
        primeBackdrop();
      } else {
        videoEl.removeAttribute("src");
        videoEl.load();
      }
    }
    render();
  });

  // Repaint the split overlay whenever its state changes (markers added/removed,
  // a segment kept/discarded, or the editor opened/closed). Reads the reactive
  // sources explicitly so the dependency tracking is obvious.
  createEffect(() => {
    splitMode();
    segments();
    meta.list.forEach((m) => m.keep);
    render();
  });

  // ── Split / trim editor (M10-C) ─────────────────────────────────────────

  /** Open/close the split editor. Closing discards the ephemeral marker state. */
  function toggleSplitMode(): void {
    if (splitMode()) exitSplitMode();
    else setSplitMode(true);
  }

  /** Close the editor and clear the markers/segment meta. */
  function exitSplitMode(): void {
    setSplitMode(false);
    setMarkers([]);
    setMeta("list", []);
  }

  /** Drop a split marker at the current playhead/cursor song-time. */
  function dropMarker(): void {
    const s = store.snap;
    if (!s) return;
    const total = totalUs();
    const a = Math.round(anchorUsOf(s));
    if (total <= 0) {
      showFlash("add some notes before splitting");
      return;
    }
    if (a <= 0 || a >= total) {
      showFlash("marker must sit inside the piece");
      return;
    }
    setMarkers((ms) => {
      if (ms.some((m) => Math.abs(m - a) < 1)) return ms; // already a marker here
      return [...ms, a].sort((x, y) => x - y);
    });
  }

  /** Remove the marker nearest the playhead/cursor song-time. */
  function removeNearestMarker(): void {
    const s = store.snap;
    if (!s) return;
    const a = anchorUsOf(s);
    setMarkers((ms) => {
      if (ms.length === 0) return ms;
      let best = 0;
      let bestDist = Infinity;
      ms.forEach((m, i) => {
        const d = Math.abs(m - a);
        if (d < bestDist) {
          bestDist = d;
          best = i;
        }
      });
      return ms.filter((_, i) => i !== best);
    });
  }

  /** Clear every split marker (back to one whole-piece segment). */
  function clearMarkers(): void {
    setMarkers([]);
  }

  /** Toggle keep/discard for the segment at `i`. */
  function toggleKeep(i: number): void {
    setMeta("list", i, "keep", (k) => !k);
  }

  /** Rename the segment at `i`. */
  function renameSegment(i: number, name: string): void {
    setMeta("list", i, "name", name);
  }

  /** Gather the kept segments and write them via `SplitBundle` (M10-B). */
  function doSaveParts(): void {
    const specs = keptSegmentSpecs(segments(), meta.list);
    if (specs.length === 0) {
      showFlash("keep at least one part to save");
      return;
    }
    if (specs.some((sp) => sp.name.length === 0)) {
      showFlash("every kept part needs a name");
      return;
    }
    splitBundle(specs)
      .then((dirs) => {
        showFlash(
          `saved ${dirs.length} part${dirs.length === 1 ? "" : "s"} → library`,
        );
        exitSplitMode();
      })
      .catch((e: unknown) => {
        showFlash(`split failed: ${String(e)}`);
      });
  }

  // ── Key routing ─────────────────────────────────────────────────────────

  function onKeydown(e: KeyboardEvent): void {
    const s = store.snap;
    if (e.ctrlKey || e.metaKey || e.altKey) return;

    // Let native text editing own the keyboard while a real text field is
    // focused (the split editor's segment-name inputs). Without this the editor
    // keymap would steal letters like `n`/`x`/`c` mid-type.
    const target = e.target as HTMLElement | null;
    if (
      target &&
      (target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable)
    ) {
      return;
    }

    const ov = overlay();

    // ── Save-as overlay ─────────────────────────────────────────────────
    if (ov === "save-as") {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setOverlay("none");
        setSaveName("");
      } else if (e.key === "Enter") {
        const name = saveName().trim();
        if (name.length > 0) {
          setOverlay("none");
          doSaveAs(name);
          setSaveName("");
        }
        // Empty name: stay in prompt (reject inline by doing nothing).
      } else if (e.key === "Backspace") {
        setSaveName((n) => n.slice(0, -1));
      } else if (e.key.length === 1) {
        setSaveName((n) => n + e.key);
      }
      return;
    }

    // ── Set-BPM overlay ─────────────────────────────────────────────────
    if (ov === "set-bpm") {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setOverlay("none");
        setBpmText("");
      } else if (e.key === "Enter") {
        const bpm = parseInt(bpmText().trim(), 10);
        setOverlay("none");
        setBpmText("");
        if (Number.isFinite(bpm)) {
          // core clamps to 20..=300; we just forward the typed value.
          void runAction("set_bpm", { bpm }).then((reply) => {
            setDirty(reply.dirty);
          });
        }
      } else if (e.key === "Backspace") {
        setBpmText((n) => n.slice(0, -1));
      } else if (/^[0-9]$/.test(e.key)) {
        // Cap at 3 digits (max BPM is 300).
        setBpmText((n) => (n.length < 3 ? n + e.key : n));
      }
      return;
    }

    // ── Exit-prompt overlay ─────────────────────────────────────────────
    if (ov === "exit-prompt") {
      e.preventDefault();
      e.stopPropagation();
      switch (e.key) {
        case "s":
        case "S":
          setOverlay("none");
          doQuickSave();
          navigate({ kind: "menu" });
          break;
        case "d":
        case "D":
          setOverlay("none");
          navigate({ kind: "menu" });
          break;
        default:
          // Any other key cancels (stays in editor).
          setOverlay("none");
          break;
      }
      return;
    }

    // ── Help overlay ────────────────────────────────────────────────────
    if (ov === "help") {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape" || e.key === "?") {
        setOverlay("none");
      }
      return;
    }

    // ── Backdrop calibration overlay (M-align) ──────────────────────────
    // Captures the whole keyboard while open (like save-as) so the alignment
    // keys never collide with the editor keymap. See CalibrationPanel for the
    // on-screen legend.
    if (ov === "calibrate") {
      e.preventDefault();
      e.stopPropagation();
      switch (e.key) {
        case "Escape":
        case "`":
          clearReview();
          setOverlay("none");
          break;
        // Keyboard X — pan (a/d) and zoom (A/D) the grid horizontally.
        case "a":
          setGridXOffset((v) => v - XOFF_STEP_PX);
          break;
        case "d":
          setGridXOffset((v) => v + XOFF_STEP_PX);
          break;
        case "A":
          setGridXScale((v) => Math.max(0.1, v - XSCALE_STEP));
          break;
        case "D":
          setGridXScale((v) => v + XSCALE_STEP);
          break;
        // Vertical — hit-line position (w/s) and zoom/density (W/S).
        case "w":
          setGridHitFrac((v) => Math.min(0.95, v + HITFRAC_STEP)); // line up
          break;
        case "s":
          setGridHitFrac((v) => Math.max(0.05, v - HITFRAC_STEP)); // line down
          break;
        case "W":
          zoomGrid(1 / GRID_ZOOM_STEP); // zoom in (denser, faster notes)
          break;
        case "S":
          zoomGrid(GRID_ZOOM_STEP); // zoom out
          break;
        // Together scrub — movie + overlay advance in lock-step. Shift = coarse.
        case "ArrowUp":
          scrubTogether(e.shiftKey ? SCRUB_STEP_COARSE_US : SCRUB_STEP_US);
          break;
        case "ArrowDown":
          scrubTogether(e.shiftKey ? -SCRUB_STEP_COARSE_US : -SCRUB_STEP_US);
          break;
        case " ":
          toggleReviewPlay();
          break;
        case "-":
        case "_":
          changeReviewRate(-1);
          break;
        case "=":
        case "+":
          changeReviewRate(1);
          break;
        // Movie-only time offset (find the first note): [ / ] coarse ±1 s,
        // ; / ' mid ±250 ms, , / . fine ±10 ms.
        case "[":
          nudgeOffset(-OFFSET_COARSE_S_US);
          break;
        case "]":
          nudgeOffset(OFFSET_COARSE_S_US);
          break;
        case ";":
          nudgeOffset(-BACKDROP_NUDGE_COARSE_US);
          break;
        case "'":
          nudgeOffset(BACKDROP_NUDGE_COARSE_US);
          break;
        case ",":
          nudgeOffset(-BACKDROP_NUDGE_FINE_US);
          break;
        case ".":
          nudgeOffset(BACKDROP_NUDGE_FINE_US);
          break;
        case "0":
          resetCalibration();
          break;
        default:
          break;
      }
      // Reflect offset/scrub nudges immediately even while paused.
      render();
      return;
    }

    // ── Normal / chord mode ─────────────────────────────────────────────
    if (!s) return;

    // ── Split editor (M10-C) — owns a few keys while open ────────────────
    // Only the split-specific keys are intercepted; navigation/transport keys
    // fall through to the normal keymap so the user can move the cursor/playhead
    // to position the next marker.
    if (splitMode() && s.chord_preview === null) {
      switch (e.key) {
        case "Escape":
          e.preventDefault();
          e.stopPropagation();
          exitSplitMode();
          return;
        case "n":
          e.preventDefault();
          dropMarker();
          return;
        case "x":
          e.preventDefault();
          removeNearestMarker();
          return;
        case "c":
          e.preventDefault();
          clearMarkers();
          return;
        case "Enter":
          e.preventDefault();
          doSaveParts();
          return;
      }
    }

    // `?` toggles help regardless of mode.
    if (e.key === "?" && s.chord_preview === null) {
      e.preventDefault();
      setOverlay("help");
      return;
    }

    // `s` = quick save, `S` = save-as (only in non-chord mode).
    if (s.chord_preview === null) {
      if (e.key === "s") {
        e.preventDefault();
        doQuickSave();
        return;
      }
      if (e.key === "S") {
        e.preventDefault();
        setSaveName("");
        setOverlay("save-as");
        return;
      }
      // `T` opens the absolute set-BPM prompt, seeded with the current tempo.
      if (e.key === "T") {
        e.preventDefault();
        setBpmText(String(Math.round(s.bpm)));
        setOverlay("set-bpm");
        return;
      }
      // `F` opens / closes the split & trim editor (M10-C).
      if (e.key === "F") {
        e.preventDefault();
        toggleSplitMode();
        return;
      }
    }

    // ── Backing audio track (M9-E; swap M10-E), non-chord mode only ──────
    if (s.chord_preview === null) {
      // `B` opens the picker to attach a backing track, or to **replace** the
      // current one in place (M10-E) — the background video stays visible.
      // Relocated here from the former top-level "Choose backing track" item.
      if (e.key === "B") {
        e.preventDefault();
        attachBacking();
        return;
      }
      // Backspace / Delete detaches the backing (audio only — the video, if
      // any, remains). Only intercepted while a track is attached so the keys
      // still bubble otherwise.
      if (
        (e.key === "Backspace" || e.key === "Delete") &&
        backingName() !== null
      ) {
        e.preventDefault();
        detachBacking();
        return;
      }
    }

    // ── Video backdrop (M7-tauri-N), non-chord mode only ────────────────
    if (s.chord_preview === null) {
      // `V` attaches a video (or detaches the current one).
      if (e.key === "V") {
        e.preventDefault();
        toggleBackdrop();
        return;
      }
      // While a backdrop is attached, the backing-align nudge keys realign the
      // *video* offset instead (they don't collide with backing-offset because
      // they only divert here when a video is present).
      if (videoPath() !== null) {
        // Backtick opens the alignment overlay (keyboard X, density/hit-line Y,
        // time, review speed) for registering the movie under the grid.
        if (e.key === "`") {
          e.preventDefault();
          openCalibrate();
          return;
        }
        switch (e.key) {
          case ",":
            e.preventDefault();
            nudgeOffset(-BACKDROP_NUDGE_FINE_US);
            return;
          case ".":
            e.preventDefault();
            nudgeOffset(BACKDROP_NUDGE_FINE_US);
            return;
          case ";":
            e.preventDefault();
            nudgeOffset(-BACKDROP_NUDGE_COARSE_US);
            return;
          case "'":
            e.preventDefault();
            nudgeOffset(BACKDROP_NUDGE_COARSE_US);
            return;
        }
      }
    }

    // Esc in non-chord, no-selection mode → dirty exit prompt or menu nav.
    if (e.key === "Escape" && s.chord_preview === null && s.selection === null) {
      if (dirty()) {
        e.preventDefault();
        e.stopPropagation();
        setOverlay("exit-prompt");
      }
      // If not dirty, let the event bubble to the Router's global Esc handler.
      return;
    }

    // Route through the keymap (normal / chord).
    const res = resolveKey(e.key, s.chord_preview !== null, s.selection !== null);
    switch (res.kind) {
      case "action":
        e.preventDefault();
        void runAction(res.dispatch.name, res.dispatch.params).then((reply) => {
          setDirty(reply.dirty);
        });
        break;
      case "action-swallow":
        e.preventDefault();
        e.stopPropagation();
        void runAction(res.dispatch.name, res.dispatch.params).then((reply) => {
          setDirty(reply.dirty);
        });
        break;
      case "swallow":
        e.preventDefault();
        e.stopPropagation();
        break;
      case "ignore":
        break;
    }
  }

  onMount(() => {
    engine = new EditCanvas(canvasEl);

    let unlisten: UnlistenFn | undefined;
    onSnapshot(applySnapshot).then((fn) => {
      unlisten = fn;
    });

    // If a bundle dir was passed in the route payload, load it first;
    // otherwise fetch the current state.
    const init = (
      props.dir
        ? loadBundle(props.dir).then((snap) => {
            applySnapshot(snap);
            setDirty(false);
          })
        : queryState().then(applySnapshot)
    ).then(() => {
      // Re-attach the backend's backdrop video + backing on BOTH paths so a
      // bundle loaded over the control socket (no route dir) shows its movie.
      reattachBackdropAndBacking(props.dir);
    });

    init
      .then(() => {
        // "New piece" opens armed for recording (M9-A): arm step-record once,
        // only if the loaded state is still in DirectEdit (never disarm).
        if (props.armed && store.snap?.input_mode === "DirectEdit") {
          toggleRecordArm();
        }
      })
      .catch(() => {
        /* backend not up (plain vite without tauri) — render stays blank */
      });

    // Drive the live input-level meter from played note-ons (M9-A). The level
    // decays in the render loop; a strike snaps it up to the note velocity.
    let unlistenMidi: UnlistenFn | undefined;
    onMidiEvent((ev) => {
      if (ev.on) setInputLevel(Math.min(1, ev.velocity / 127));
    }).then((fn) => {
      unlistenMidi = fn;
    });

    // Fetch initial dirty state (covers the case where a bundle was pre-loaded
    // by another path before this component mounted).
    queryDirty()
      .then(setDirty)
      .catch(() => {
        /* ignore if backend not up */
      });

    const loop = (): void => {
      const s = store.snap;
      if (reviewPlaying()) {
        // Advance the together-sweep head by real elapsed time × rate, then
        // pause a couple of seconds past the last note (head stays at the end).
        const t =
          reviewBaseUs +
          (performance.now() - reviewBaseWall) * 1000 * reviewRate();
        if (t >= songEndUs() + 2_000_000) {
          setReviewPlaying(false);
          setReviewUs(songEndUs());
          render();
        } else {
          setReviewUs(t);
          render();
        }
      } else if (s?.playing) render();
      // Decay the input-level meter toward zero between strikes (M9-A).
      const lvl = inputLevel();
      if (lvl > 0.001) setInputLevel(lvl * 0.9);
      else if (lvl !== 0) setInputLevel(0);
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);

    window.addEventListener("keydown", onKeydown);
    onCleanup(() => {
      window.removeEventListener("keydown", onKeydown);
      cancelAnimationFrame(raf);
      clearTimeout(flashTimeout);
      unlisten?.();
      unlistenMidi?.();
      engine?.dispose();
      engine = null;
    });
  });

  // ── Render ───────────────────────────────────────────────────────────────

  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        "flex-direction": "column",
        background: "#0f1016",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
        position: "relative",
      }}
    >
      {/* Status bar / save-flash */}
      <Show
        when={saveFlash()}
        fallback={
          <Show
            when={store.snap}
            fallback={
              <div
                style={{
                  height: "34px",
                  display: "flex",
                  "align-items": "center",
                  padding: "0 14px",
                  color: "#6a6e7e",
                  "font-size": "12px",
                  background: "#15161d",
                }}
              >
                waiting for snapshot…
              </div>
            }
          >
            {(snap) => <StatusBar snapshot={snap()} dirty={dirty()} />}
          </Show>
        }
      >
        {(flash) => (
          <div
            style={{
              height: "34px",
              display: "flex",
              "align-items": "center",
              padding: "0 14px",
              background: "#15161d",
              "border-bottom": "1px solid rgba(255,255,255,0.06)",
              gap: "12px",
            }}
          >
            <span
              style={{
                background: "#4caf50",
                color: "#0f1016",
                padding: "2px 10px",
                "border-radius": "4px",
                "font-size": "11px",
                "font-weight": 600,
              }}
            >
              {flash()}
            </span>
            <span style={{ color: "#4a4d5a", "font-size": "11px" }}>
              s save · S library · Esc menu
            </span>
          </div>
        )}
      </Show>

      {/* Canvas area */}
      <div style={{ flex: "1 1 auto", "min-height": 0, position: "relative" }}>
        {/* Video backdrop (M7-tauri-N) — sits *behind* the canvas (lower
            z-index). Paused/muted; we scrub `currentTime`, never `play()`.
            Hidden until a backdrop is attached. */}
        <video
          ref={videoEl}
          muted
          playsinline
          preload="auto"
          style={{
            position: "absolute",
            inset: 0,
            width: "100%",
            height: "100%",
            // `fill` stretches the frame to the canvas box so the movie spans a
            // known rectangle; the *grid* (not the movie) is then resized to
            // register on it — see the M-align calibration in EditCanvas.
            "object-fit": "fill",
            "z-index": 0,
            opacity: BACKDROP_OPACITY,
            "pointer-events": "none",
            display: videoPath() === null ? "none" : "block",
            background: "#000",
          }}
        />

        <canvas
          ref={canvasEl}
          style={{
            position: "relative",
            "z-index": 1,
            width: "100%",
            height: "100%",
            display: "block",
          }}
        />

        {/* Backdrop HUD — current alignment offset + key hints */}
        <Show when={videoPath() !== null}>
          <BackdropHud offsetUs={offsetUs()} />
        </Show>

        {/* Calibration overlay — alignment legend + live values (M-align) */}
        <Show when={overlay() === "calibrate"}>
          <CalibrationPanel
            spanUs={gridSpanUs()}
            hitFrac={gridHitFrac()}
            xScale={gridXScale()}
            xOffset={gridXOffset()}
            offsetUs={offsetUs()}
            rate={reviewRate()}
            playing={reviewPlaying()}
            headUs={reviewUs()}
          />
        </Show>

        {/* Backing-track indicator (M9-E) — shows the attached audio file and
            the attach/detach key. Sits above the backdrop HUD when both show. */}
        <BackingHud name={backingName()} />

        {/* Chord selector panel — shown while chord_preview is non-null */}
        <Show when={store.snap?.chord_preview !== null && store.snap?.chord_preview !== undefined}>
          <ChordSelectorPanel snap={store.snap!} />
        </Show>

        {/* Record ⇄ Edit controls (M9-A): a discoverable on-screen toggle, a
            record indicator, and the live input-level meter folded in from the
            standalone record screen. Reuses the existing toggle actions. */}
        <Show when={store.snap}>
          <RecordControls
            inputMode={store.snap!.input_mode}
            level={inputLevel()}
            onToggleArm={toggleRecordArm}
            onToggleFlavour={toggleRecordFlavour}
          />
        </Show>

        {/* Split & trim strip (M10-C) — markers + per-segment keep/discard/name,
            docked at the bottom so the grid and video backdrop stay visible. */}
        <Show when={splitMode()}>
          <SplitStrip
            segments={segments()}
            meta={meta.list}
            total={totalUs()}
            onDrop={dropMarker}
            onRemove={removeNearestMarker}
            onClear={clearMarkers}
            onToggleKeep={toggleKeep}
            onRename={renameSegment}
            onSave={doSaveParts}
            onExit={exitSplitMode}
          />
        </Show>
      </div>

      {/* Help overlay */}
      <Show when={overlay() === "help"}>
        <HelpOverlay onClose={() => setOverlay("none")} />
      </Show>

      {/* Exit-prompt overlay */}
      <Show when={overlay() === "exit-prompt"}>
        <ExitPrompt />
      </Show>

      {/* Save-as overlay */}
      <Show when={overlay() === "save-as"}>
        <SaveAsPrompt name={saveName()} />
      </Show>

      {/* Set-BPM overlay */}
      <Show when={overlay() === "set-bpm"}>
        <SetBpmPrompt text={bpmText()} />
      </Show>
    </div>
  );
}

// ── SplitStrip (M10-C) ──────────────────────────────────────────────────────

interface SplitStripProps {
  /** Derived consecutive segments (mirrors `segments_from_splits`). */
  segments: Segment[];
  /** Per-segment keep/name, parallel to `segments`. */
  meta: SegmentMeta[];
  /** Total piece length in µs (0 ⇒ no notes yet). */
  total: number;
  onDrop: () => void;
  onRemove: () => void;
  onClear: () => void;
  onToggleKeep: (i: number) => void;
  onRename: (i: number, name: string) => void;
  onSave: () => void;
  onExit: () => void;
}

/** µs → "1.2s" for the segment range labels. */
function fmtSec(us: number): string {
  return `${(us / 1e6).toFixed(1)}s`;
}

/**
 * The split & trim control strip docked at the bottom of the edit screen
 * (M10-C). Lists the segments the markers imply, each with a keep/discard toggle
 * and an editable name, plus the marker + save actions. Kept deliberately above
 * the grid but leaving the video backdrop visible (the point — trim by watching).
 */
function SplitStrip(props: SplitStripProps): JSX.Element {
  const keptCount = (): number =>
    props.segments.reduce(
      (n, _seg, i) => n + ((props.meta[i]?.keep ?? true) ? 1 : 0),
      0,
    );
  const hasPiece = (): boolean => props.total > 0;

  // Buttons stay enabled; the handlers validate (and flash) and the strip shows
  // an explicit empty/all-discarded message, so there is no stale `disabled`
  // state to keep reactive (components run once in Solid).
  const btn = (
    label: string,
    onClick: () => void,
    primary?: boolean,
  ): JSX.Element => (
    <button
      onClick={onClick}
      style={{
        background: primary ? "#f5a742" : "rgba(255,255,255,0.06)",
        color: primary ? "#0f1016" : "#e7e8ef",
        border: "1px solid rgba(255,255,255,0.12)",
        "border-radius": "5px",
        padding: "4px 10px",
        "font-size": "11px",
        "font-weight": primary ? 700 : 500,
        cursor: "pointer",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      {label}
    </button>
  );

  return (
    <div
      style={{
        position: "absolute",
        left: 0,
        right: 0,
        bottom: 0,
        background: "rgba(15,16,22,0.94)",
        "border-top": "1px solid #f5a742",
        padding: "10px 14px",
        "z-index": 120,
        "backdrop-filter": "blur(3px)",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
        "max-height": "42%",
        display: "flex",
        "flex-direction": "column",
        gap: "8px",
      }}
    >
      {/* Header: badge + counts + actions */}
      <div style={{ display: "flex", "align-items": "center", gap: "10px" }}>
        <span
          style={{
            background: "#f5a742",
            color: "#0f1016",
            padding: "2px 8px",
            "border-radius": "4px",
            "font-size": "11px",
            "font-weight": 700,
            "letter-spacing": "0.5px",
          }}
        >
          SPLIT / TRIM
        </span>
        <span style={{ color: "#aeb2c4", "font-size": "11px" }}>
          {props.segments.length} segment
          {props.segments.length === 1 ? "" : "s"} · {keptCount()} kept
        </span>
        <span style={{ flex: 1 }} />
        {btn("＋ marker (n)", props.onDrop)}
        {btn("− nearest (x)", props.onRemove)}
        {btn("clear (c)", props.onClear)}
        {btn("save parts", props.onSave, true)}
        {btn("exit (Esc)", props.onExit)}
      </div>

      {/* Segment chips, or guidance when there is nothing to split */}
      <Show
        when={hasPiece()}
        fallback={
          <div style={{ color: "#6a6e7e", "font-size": "12px" }}>
            Add some notes before splitting.
          </div>
        }
      >
        <div
          style={{
            display: "flex",
            gap: "8px",
            "overflow-x": "auto",
            "padding-bottom": "2px",
          }}
        >
          <For each={props.segments}>
            {(seg, i) => {
              const m = (): SegmentMeta | undefined => props.meta[i()];
              const keep = (): boolean => m()?.keep ?? true;
              return (
                <div
                  style={{
                    "min-width": "150px",
                    background: keep()
                      ? "rgba(245,167,66,0.12)"
                      : "rgba(255,90,90,0.08)",
                    border: `1px solid ${keep() ? "rgba(245,167,66,0.5)" : "rgba(255,90,90,0.35)"}`,
                    "border-radius": "6px",
                    padding: "6px 8px",
                    opacity: keep() ? 1 : 0.55,
                    display: "flex",
                    "flex-direction": "column",
                    gap: "5px",
                  }}
                >
                  <div
                    style={{
                      display: "flex",
                      "align-items": "center",
                      "justify-content": "space-between",
                      gap: "6px",
                    }}
                  >
                    <span
                      style={{
                        color: "#e7e8ef",
                        "font-size": "11px",
                        "font-weight": 600,
                      }}
                    >
                      part {i() + 1}
                    </span>
                    <button
                      onClick={() => props.onToggleKeep(i())}
                      style={{
                        background: keep() ? "#f5a742" : "rgba(255,90,90,0.25)",
                        color: keep() ? "#0f1016" : "#ff9a9a",
                        border: "none",
                        "border-radius": "4px",
                        padding: "2px 8px",
                        "font-size": "10px",
                        "font-weight": 700,
                        cursor: "pointer",
                        "font-family": "'Space Grotesk', system-ui, sans-serif",
                      }}
                    >
                      {keep() ? "KEEP" : "CUT"}
                    </button>
                  </div>
                  <span
                    style={{
                      color: "#6a6e7e",
                      "font-size": "10px",
                      "font-family": "'IBM Plex Mono', ui-monospace, monospace",
                    }}
                  >
                    {fmtSec(seg.start_us)}–{fmtSec(seg.end_us)}
                  </span>
                  <input
                    value={m()?.name ?? ""}
                    disabled={!keep()}
                    placeholder="name"
                    onInput={(e) =>
                      props.onRename(i(), e.currentTarget.value)
                    }
                    style={{
                      background: "#0f1016",
                      border: "1px solid rgba(255,255,255,0.12)",
                      "border-radius": "4px",
                      padding: "3px 6px",
                      color: "#e7e8ef",
                      "font-size": "11px",
                      "font-family": "'IBM Plex Mono', ui-monospace, monospace",
                      width: "100%",
                      "box-sizing": "border-box",
                    }}
                  />
                </div>
              );
            }}
          </For>
        </div>
      </Show>

      <Show when={hasPiece() && keptCount() === 0}>
        <div style={{ color: "#ff9a9a", "font-size": "11px" }}>
          All parts discarded — keep at least one to save.
        </div>
      </Show>
    </div>
  );
}

// ── BackdropHud ───────────────────────────────────────────────────────────

/**
 * Small floating HUD shown while a video backdrop is attached: the current
 * alignment offset (videoTime = songTime + offset) and the realign/detach keys.
 */
function BackdropHud(props: { offsetUs: number }): JSX.Element {
  const offsetMs = (): string => {
    const ms = Math.round(props.offsetUs / 1000);
    return ms >= 0 ? `+${ms}` : String(ms);
  };

  return (
    <div
      style={{
        position: "absolute",
        bottom: "12px",
        left: "16px",
        background: "rgba(26,27,36,0.85)",
        border: "1px solid #f5c542",
        "border-radius": "8px",
        padding: "8px 12px",
        "z-index": 100,
        "backdrop-filter": "blur(3px)",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "8px",
          "margin-bottom": "4px",
        }}
      >
        <span
          style={{
            background: "#f5c542",
            color: "#0f1016",
            padding: "2px 8px",
            "border-radius": "4px",
            "font-size": "11px",
            "font-weight": 700,
            "letter-spacing": "0.5px",
          }}
        >
          VIDEO
        </span>
        <span
          style={{
            color: "#e7e8ef",
            "font-size": "12px",
            "font-family": "'IBM Plex Mono', ui-monospace, monospace",
          }}
        >
          offset {offsetMs()} ms
        </span>
      </div>
      <div style={{ color: "#6a6e7e", "font-size": "11px" }}>
        ,/. ±10 ms · ;/' ±250 ms · ` align · V detach
      </div>
    </div>
  );
}

// ── CalibrationPanel ────────────────────────────────────────────────────────

/**
 * Alignment overlay for registering an imported movie under the fixed edit
 * grid. Shows the live affine values (keyboard X scale/offset, density/hit-line
 * Y scale/offset), the time offset, the review speed, and the key legend. Keys
 * are handled in `onKeydown`'s `"calibrate"` branch.
 */
function CalibrationPanel(props: {
  spanUs: number;
  hitFrac: number;
  xScale: number;
  xOffset: number;
  offsetUs: number;
  rate: number;
  playing: boolean;
  headUs: number | null;
}): JSX.Element {
  const num = (n: number): string => n.toFixed(2);
  const ms = (): string => {
    const v = Math.round(props.offsetUs / 1000);
    return v >= 0 ? `+${v}` : String(v);
  };
  const row = (label: string, value: string, keys: string): JSX.Element => (
    <div
      style={{
        display: "grid",
        "grid-template-columns": "120px 64px 1fr",
        gap: "8px",
        "align-items": "baseline",
        "font-size": "12px",
        margin: "2px 0",
      }}
    >
      <span style={{ color: "#9a9eb0" }}>{label}</span>
      <span
        style={{
          color: "#e7e8ef",
          "font-family": "'IBM Plex Mono', ui-monospace, monospace",
          "text-align": "right",
        }}
      >
        {value}
      </span>
      <span style={{ color: "#6a6e7e", "font-size": "11px" }}>{keys}</span>
    </div>
  );

  return (
    <div
      style={{
        position: "absolute",
        top: "16px",
        right: "16px",
        background: "rgba(26,27,36,0.92)",
        border: "1px solid #5be7c4",
        "border-radius": "8px",
        padding: "10px 14px",
        "z-index": 120,
        "backdrop-filter": "blur(3px)",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
        "min-width": "320px",
      }}
    >
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "8px",
          "margin-bottom": "8px",
        }}
      >
        <span
          style={{
            background: "#5be7c4",
            color: "#0f1016",
            padding: "2px 8px",
            "border-radius": "4px",
            "font-size": "11px",
            "font-weight": 700,
            "letter-spacing": "0.5px",
          }}
        >
          ALIGN
        </span>
        <span style={{ color: "#9a9eb0", "font-size": "11px" }}>
          resize the grid to register on the movie
        </span>
      </div>
      {row("Keyboard pan", `${props.xOffset}px`, "a / d")}
      {row("Keyboard zoom", num(props.xScale), "A / D")}
      {row("Hit-line", `${Math.round((1 - props.hitFrac) * 100)}%`, "w / s")}
      {row("Vertical zoom", `${(props.spanUs / 1e6).toFixed(1)} s`, "W / S")}
      {row("Time offset", `${ms()} ms`, "[ / ] ±1s · ;/' · ,/.")}
      {row(
        "Together / speed",
        `${props.rate}×${props.playing ? " ▶" : ""}`,
        "↑/↓ ·⇧coarse· Space ·-/=",
      )}
      <div
        style={{
          color: "#5be7c4",
          "font-size": "11px",
          "font-family": "'IBM Plex Mono', ui-monospace, monospace",
          "margin-top": "8px",
          "border-top": "1px solid #2a2c38",
          "padding-top": "6px",
        }}
      >
        head{" "}
        {props.headUs === null ? "—" : `${(props.headUs / 1e6).toFixed(2)}s`} →
        vid{" "}
        {props.headUs === null
          ? "—"
          : `${((props.headUs + props.offsetUs) / 1e6).toFixed(2)}s`}
      </div>
      <div
        style={{
          color: "#6a6e7e",
          "font-size": "11px",
          "margin-top": "4px",
        }}
      >
        0 reset · ` or Esc close
      </div>
    </div>
  );
}

// ── BackingHud ──────────────────────────────────────────────────────────────

/**
 * Small floating indicator for the backing audio track (M9-E). When a track is
 * attached it names the file and offers `B` to detach; when none is attached it
 * shows the `B`-to-attach affordance so the relocated entry point is
 * discoverable on the edit screen itself. Aligns audio with `,/. ;/'`.
 */
function BackingHud(props: { name: string | null }): JSX.Element {
  return (
    <div
      style={{
        position: "absolute",
        top: "12px",
        left: "16px",
        background: "rgba(26,27,36,0.85)",
        border: `1px solid ${props.name ? "#3fd0d6" : "rgba(255,255,255,0.12)"}`,
        "border-radius": "8px",
        padding: "8px 12px",
        "z-index": 100,
        "backdrop-filter": "blur(3px)",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
        <span
          style={{
            background: props.name ? "#3fd0d6" : "transparent",
            color: props.name ? "#0f1016" : "#6a6e7e",
            border: props.name ? "none" : "1px solid #3a3d4a",
            padding: "2px 8px",
            "border-radius": "4px",
            "font-size": "11px",
            "font-weight": 700,
            "letter-spacing": "0.5px",
          }}
        >
          BACKING
        </span>
        <span
          style={{
            color: props.name ? "#e7e8ef" : "#6a6e7e",
            "font-size": "12px",
            "font-family": "'IBM Plex Mono', ui-monospace, monospace",
          }}
        >
          {props.name ?? "none"}
        </span>
        <span style={{ color: "#6a6e7e", "font-size": "11px" }}>
          {props.name ? "· B detach" : "· B choose"}
        </span>
      </div>
    </div>
  );
}

// ── ChordSelectorPanel ────────────────────────────────────────────────────

/**
 * Floating panel shown while a chord preview is open. Displays the current
 * degree (roman numeral), kind (Triad / Seventh), and preview pitch names,
 * plus the key hints.
 */
function ChordSelectorPanel(props: { snap: ComposerSnapshot }): JSX.Element {
  const preview = (): number[] => props.snap.chord_preview ?? [];

  // Infer current degree from chord size (triad=3, seventh=4) — the
  // backend owns degree/kind state; we just show the pitches.
  const kind = (): string => (preview().length >= 4 ? "Seventh" : "Triad");

  const pitchNames = (): string =>
    preview()
      .map(pitchToName)
      .join("  ");

  return (
    <div
      style={{
        position: "absolute",
        top: "12px",
        right: "16px",
        background: "#1a1b24",
        border: "1px solid #3fd0d6",
        "border-radius": "8px",
        padding: "10px 14px",
        "min-width": "240px",
        "z-index": 100,
        "box-shadow": "0 4px 24px rgba(0,0,0,0.5)",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      {/* Header row: CHORD badge + kind */}
      <div
        style={{
          display: "flex",
          "align-items": "center",
          gap: "8px",
          "margin-bottom": "8px",
        }}
      >
        <span
          style={{
            background: "#3fd0d6",
            color: "#0f1016",
            padding: "2px 8px",
            "border-radius": "4px",
            "font-size": "11px",
            "font-weight": 700,
            "letter-spacing": "0.5px",
          }}
        >
          CHORD
        </span>
        <span style={{ color: "#3fd0d6", "font-size": "12px", "font-weight": 500 }}>
          {kind()}
        </span>
      </div>

      {/* Pitch names */}
      <div
        style={{
          color: "#e7e8ef",
          "font-size": "15px",
          "font-weight": 600,
          "letter-spacing": "1px",
          "margin-bottom": "8px",
          "font-family": "'IBM Plex Mono', ui-monospace, monospace",
        }}
      >
        {pitchNames()}
      </div>

      {/* Key hints */}
      <div style={{ color: "#6a6e7e", "font-size": "11px", "line-height": "1.6" }}>
        <div>1–7 degree · [/] cycle · s toggle 7th</div>
        <div>Enter commit · Esc cancel</div>
      </div>
    </div>
  );
}

// ── HelpOverlay ───────────────────────────────────────────────────────────

/**
 * Scrollable help overlay listing every binding. Generated from the same
 * keymap tables as the StatusBar hints — no second copy.
 *
 * Esc or `?` closes (handled in the parent onKeydown, not here).
 */
function HelpOverlay(props: { onClose: () => void }): JSX.Element {
  const SECTIONS: Array<{ title: string; rows: string[] }> = [
    {
      title: "Navigation",
      rows: [
        "h / ←         Pitch −1 (left on keyboard)",
        "l / →         Pitch +1 (right on keyboard)",
        "j / ↓         Step −1 (earlier in timeline)",
        "k / ↑         Step +1 (later in timeline)",
        "H             One bar earlier",
        "L             One bar later",
        "w             Octave +12",
        "b             Octave −12",
        "g             Timeline start",
        "G             Timeline end (last note)",
        "0             Lowest pitch (A0)",
        "$             Highest pitch (C8)",
        ">             Finer subdivision",
        "<             Coarser subdivision",
      ],
    },
    {
      title: "Edit",
      rows: [
        "a / i         Add note",
        "x / d         Delete note",
        "]             Lengthen note (+1 step)",
        "[             Shorten note (−1 step)",
        "+ / =         Velocity +8",
        "-             Velocity −8",
        "( / )         Tempo −/+ 5 BPM",
        "T             Set BPM (type a value, Enter to apply)",
        "m             Toggle grab (move note with h/j/k/l)",
      ],
    },
    {
      title: "Chord selector",
      rows: [
        "c             Enter chord mode",
        "1–7           Set degree",
        "[ / ]         Cycle degree ±1",
        "s             Toggle Triad ↔ Seventh",
        "Enter         Commit chord",
        "Esc           Cancel chord",
      ],
    },
    {
      title: "Selection / Clipboard",
      rows: [
        "v             Start visual selection",
        "y             Yank selection",
        "p             Paste clipboard",
        "D             Delete selection",
        "Esc           Clear selection",
      ],
    },
    {
      title: "Transport",
      rows: [
        "Space         Play / stop from cursor (toggles — press again to stop)",
        "P             Play from start",
        "              While playing, a PLAYING badge shows in the status bar",
      ],
    },
    {
      title: "Backing track",
      rows: [
        "B             Choose / replace the backing audio track",
        "Backspace     Detach the backing (keeps the video backdrop)",
        ", / .         Nudge −/+ 10 ms",
        "; / '         Nudge −/+ 250 ms",
      ],
    },
    {
      title: "Video backdrop",
      rows: [
        "V             Attach / detach a video backdrop",
        ", / .         Realign −/+ 10 ms (while attached)",
        "; / '         Realign −/+ 250 ms (while attached)",
      ],
    },
    {
      title: "Split & trim into pieces",
      rows: [
        "F             Open / close the split editor",
        "n             Drop a split marker at the cursor / playhead",
        "x             Remove the nearest split marker",
        "c             Clear all split markers",
        "              Toggle keep/discard and name each part in the strip",
        "Enter         Save the kept parts as new library pieces",
        "Esc           Close the split editor",
      ],
    },
    {
      title: "Loop / Metronome / Count-in",
      rows: [
        "o             Toggle loop (defaults to the bar under the cursor)",
        "{             Set loop start (loop-in) at the cursor",
        "}             Set loop end (loop-out) at the cursor",
        "M             Toggle metronome",
        "C             Count-in record",
      ],
    },
    {
      title: "Input mode",
      rows: [
        "R             Toggle record arm",
        "t             Toggle step / live record",
      ],
    },
    {
      title: "Undo / Redo",
      rows: [
        "u             Undo",
        "U             Redo",
      ],
    },
    {
      title: "Save / Exit",
      rows: [
        "s             Quick save (recordings/)",
        "S             Save to library (named)",
        "?             Toggle help",
        "Esc           Back to menu (save prompt if dirty)",
      ],
    },
  ];

  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        background: "rgba(0,0,0,0.72)",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        "z-index": 200,
      }}
      onClick={props.onClose}
    >
      <div
        style={{
          background: "#1a1b24",
          border: "1px solid rgba(255,255,255,0.12)",
          "border-radius": "10px",
          padding: "20px 24px",
          "max-width": "560px",
          "max-height": "80vh",
          "overflow-y": "auto",
          "box-shadow": "0 8px 40px rgba(0,0,0,0.6)",
          "font-family": "'Space Grotesk', system-ui, sans-serif",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Title bar */}
        <div
          style={{
            display: "flex",
            "align-items": "center",
            "justify-content": "space-between",
            "margin-bottom": "16px",
          }}
        >
          <span style={{ color: "#fff", "font-size": "16px", "font-weight": 700 }}>
            Keyboard shortcuts
          </span>
          <button
            style={{
              background: "none",
              border: "none",
              color: "#6a6e7e",
              cursor: "pointer",
              "font-size": "18px",
              padding: "0",
              "line-height": "1",
            }}
            onClick={props.onClose}
          >
            ×
          </button>
        </div>

        {/* Sections */}
        {SECTIONS.map((sec) => (
          <div style={{ "margin-bottom": "14px" }}>
            <div
              style={{
                color: "#f5c542",
                "font-size": "11px",
                "font-weight": 700,
                "text-transform": "uppercase",
                "letter-spacing": "0.08em",
                "margin-bottom": "4px",
              }}
            >
              {sec.title}
            </div>
            {sec.rows.map((row) => {
              const tabIdx = row.indexOf("  ");
              const key = tabIdx >= 0 ? row.slice(0, tabIdx).trim() : row;
              const desc = tabIdx >= 0 ? row.slice(tabIdx).trim() : "";
              return (
                <div
                  style={{
                    display: "flex",
                    gap: "12px",
                    "font-size": "12px",
                    "line-height": "1.7",
                  }}
                >
                  <span
                    style={{
                      color: "#aeb2c4",
                      "font-family": "'IBM Plex Mono', ui-monospace, monospace",
                      "min-width": "110px",
                    }}
                  >
                    {key}
                  </span>
                  <span style={{ color: "#6a6e7e" }}>{desc}</span>
                </div>
              );
            })}
          </div>
        ))}

        <div
          style={{
            "margin-top": "12px",
            color: "#4a4d5a",
            "font-size": "11px",
            "text-align": "center",
          }}
        >
          Press ? or Esc to close
        </div>
      </div>
    </div>
  );
}

// ── ExitPrompt ────────────────────────────────────────────────────────────

/**
 * Centered overlay: "Save / Discard / Cancel" — mirrors `draw_exit_prompt`.
 * Key routing is handled in the parent `onKeydown`.
 */
function ExitPrompt(): JSX.Element {
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        background: "rgba(0,0,0,0.6)",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        "z-index": 200,
      }}
    >
      <div
        style={{
          background: "#1a1b24",
          border: "1px solid #f5c542",
          "border-radius": "8px",
          padding: "16px 24px",
          "text-align": "center",
          "font-family": "'Space Grotesk', system-ui, sans-serif",
          "box-shadow": "0 4px 24px rgba(0,0,0,0.5)",
        }}
      >
        <div style={{ color: "#f5c542", "font-size": "14px", "margin-bottom": "8px" }}>
          Unsaved changes
        </div>
        <div style={{ color: "#aeb2c4", "font-size": "12px" }}>
          <span style={{ color: "#f5c542" }}>[s]</span> Save and leave ·{" "}
          <span style={{ color: "#f5c542" }}>[d]</span> Discard ·{" "}
          <span style={{ color: "#f5c542" }}>[any other]</span> Cancel
        </div>
      </div>
    </div>
  );
}

// ── SaveAsPrompt ──────────────────────────────────────────────────────────

/**
 * Single-line text input overlay for "Save to library". Mirrors
 * `draw_name_prompt` from the TUI. Key routing is in the parent `onKeydown`.
 */
function SaveAsPrompt(props: { name: string }): JSX.Element {
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        background: "rgba(0,0,0,0.6)",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        "z-index": 200,
      }}
    >
      <div
        style={{
          background: "#1a1b24",
          border: "1px solid #3fd0d6",
          "border-radius": "8px",
          padding: "16px 24px",
          "min-width": "320px",
          "font-family": "'Space Grotesk', system-ui, sans-serif",
          "box-shadow": "0 4px 24px rgba(0,0,0,0.5)",
        }}
      >
        <div style={{ color: "#3fd0d6", "font-size": "13px", "margin-bottom": "10px" }}>
          Save to library
        </div>
        <div
          style={{
            background: "#0f1016",
            border: "1px solid rgba(255,255,255,0.12)",
            "border-radius": "4px",
            padding: "6px 10px",
            "font-family": "'IBM Plex Mono', ui-monospace, monospace",
            "font-size": "14px",
            color: "#e7e8ef",
            "margin-bottom": "8px",
            "min-height": "28px",
          }}
        >
          {props.name}
          <span style={{ color: "#3fd0d6" }}>█</span>
        </div>
        <div style={{ color: "#4a4d5a", "font-size": "11px" }}>
          Enter to save · Esc to cancel · empty name is rejected
        </div>
      </div>
    </div>
  );
}

// ── SetBpmPrompt ──────────────────────────────────────────────────────────

/**
 * Single-line numeric input overlay for the absolute tempo (BPM). Mirrors the
 * TUI's `draw_bpm_prompt`; key routing is in the parent `onKeydown`. The typed
 * value is forwarded to `set_bpm`, which clamps to 20..=300 in `core`.
 */
function SetBpmPrompt(props: { text: string }): JSX.Element {
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        background: "rgba(0,0,0,0.6)",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        "z-index": 200,
      }}
    >
      <div
        style={{
          background: "#1a1b24",
          border: "1px solid #f5c542",
          "border-radius": "8px",
          padding: "16px 24px",
          "min-width": "320px",
          "font-family": "'Space Grotesk', system-ui, sans-serif",
          "box-shadow": "0 4px 24px rgba(0,0,0,0.5)",
        }}
      >
        <div style={{ color: "#f5c542", "font-size": "13px", "margin-bottom": "10px" }}>
          Set tempo (BPM)
        </div>
        <div
          style={{
            background: "#0f1016",
            border: "1px solid rgba(255,255,255,0.12)",
            "border-radius": "4px",
            padding: "6px 10px",
            "font-family": "'IBM Plex Mono', ui-monospace, monospace",
            "font-size": "14px",
            color: "#e7e8ef",
            "margin-bottom": "8px",
            "min-height": "28px",
          }}
        >
          {props.text}
          <span style={{ color: "#f5c542" }}>█</span>
        </div>
        <div style={{ color: "#4a4d5a", "font-size": "11px" }}>
          Enter to apply · Esc to cancel · range 20–300
        </div>
      </div>
    </div>
  );
}
