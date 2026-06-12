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
  createSignal,
  onCleanup,
  onMount,
  Show,
  type JSX,
} from "solid-js";
import { createStore } from "solid-js/store";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  loadBundle,
  onSnapshot,
  openVideoFilePicker,
  queryDirty,
  queryState,
  runAction,
  saveBundle,
  transcriptionLoad,
  transcriptionSave,
} from "../../ipc/bridge";
import type { ComposerSnapshot } from "../../ipc/types";
import { EditCanvas } from "./EditCanvas";
import { StatusBar } from "./StatusBar";
import { resolveKey } from "./keymap";
import { stepUs } from "./viewport";
import { noteName } from "../highway/utils";
import { useRouter } from "../../shell/Router";

// ── Video backdrop (M7-tauri-N) ────────────────────────────────────────────

/** Fine backdrop-offset nudge (10 ms), bound to `,` / `.` while attached. */
const BACKDROP_NUDGE_FINE_US = 10_000;
/** Coarse backdrop-offset nudge (250 ms), bound to `;` / `'` while attached. */
const BACKDROP_NUDGE_COARSE_US = 250_000;
/** Opacity the backdrop `<video>` renders at (dimmed under the grid). */
const BACKDROP_OPACITY = 0.5;

// ── Overlay state ─────────────────────────────────────────────────────────

/** What top-level overlay is currently active (at most one at a time). */
type Overlay =
  | "none"
  | "help"
  /** "Save / Discard / Cancel" shown on dirty Esc. */
  | "exit-prompt"
  /** Text input for the save-to-library name. */
  | "save-as";

// ── Chord selector helpers ────────────────────────────────────────────────

/** MIDI pitch → note name (C, C#, …). */
function pitchToName(p: number): string {
  return noteName(p);
}

// ── Component ─────────────────────────────────────────────────────────────

interface Props {
  /** Bundle directory to load on mount (from the router payload). */
  dir?: string;
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

  // The snapshot mirror.
  const [store, setStore] = createStore<{ snap: ComposerSnapshot | null }>({
    snap: null,
  });
  const [, setRev] = createSignal(0);

  // Dirty flag — mirrors the backend's AppState::dirty.
  const [dirty, setDirty] = createSignal(false);

  // Overlay state.
  const [overlay, setOverlay] = createSignal<Overlay>("none");

  // Save-as prompt: name typed so far.
  const [saveName, setSaveName] = createSignal("");

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

  // ── Save / load actions ─────────────────────────────────────────────────

  function showFlash(msg: string): void {
    setSaveFlash(msg);
    clearTimeout(flashTimeout);
    flashTimeout = window.setTimeout(() => setSaveFlash(null), 2500);
  }

  function doQuickSave(): void {
    saveBundle({ kind: "quick_save" })
      .then((dir) => {
        setDirty(false);
        persistBackdrop(dir);
        showFlash(`saved → ${dir}`);
      })
      .catch((e: unknown) => {
        showFlash(`save failed: ${String(e)}`);
      });
  }

  function doSaveAs(name: string): void {
    saveBundle({ kind: "library", name })
      .then((dir) => {
        setDirty(false);
        persistBackdrop(dir);
        showFlash(`saved to library → ${dir}`);
      })
      .catch((e: unknown) => {
        showFlash(`save failed: ${String(e)}`);
      });
  }

  // ── Video backdrop (M7-tauri-N) ─────────────────────────────────────────

  /** After a save resolves, write the backdrop sidecar if one is attached. */
  function persistBackdrop(dir: string): void {
    const v = videoPath();
    if (v === null) return;
    void transcriptionSave(dir, { video: v, offset_us: offsetUs() }).catch(
      () => {
        /* sidecar is best-effort; the bundle itself already saved */
      },
    );
  }

  /** Open the native picker and attach the chosen video as the backdrop. */
  function attachBackdrop(): void {
    void openVideoFilePicker()
      .then((path) => {
        if (path === null) return; // cancelled
        setVideoPath(path);
        setOffsetUs(0);
      })
      .catch(() => {
        showFlash("could not open video picker");
      });
  }

  /** Clear the backdrop and reset its offset. */
  function detachBackdrop(): void {
    setVideoPath(null);
    setOffsetUs(0);
  }

  /** `V` toggles the backdrop: pick when none, detach when attached. */
  function toggleBackdrop(): void {
    if (videoPath() === null) attachBackdrop();
    else detachBackdrop();
  }

  /** Nudge the alignment offset and re-sync the frame immediately. */
  function nudgeOffset(deltaUs: number): void {
    setOffsetUs((o) => o + deltaUs);
    const s = store.snap;
    if (s) syncVideo(anchorUsOf(s));
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
      } else {
        videoEl.removeAttribute("src");
        videoEl.load();
      }
    }
    render();
  });

  // ── Key routing ─────────────────────────────────────────────────────────

  function onKeydown(e: KeyboardEvent): void {
    const s = store.snap;
    if (e.ctrlKey || e.metaKey || e.altKey) return;

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

    // ── Normal / chord mode ─────────────────────────────────────────────
    if (!s) return;

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
    const init = props.dir
      ? loadBundle(props.dir).then((snap) => {
          applySnapshot(snap);
          setDirty(false);
          // Re-attach a previously saved video backdrop, if any (M7-tauri-N).
          void transcriptionLoad(props.dir!)
            .then((dto) => {
              if (dto) {
                setVideoPath(dto.video);
                setOffsetUs(dto.offset_us);
              }
            })
            .catch(() => {
              /* no sidecar / backend down — leave the backdrop detached */
            });
        })
      : queryState().then(applySnapshot);

    init.catch(() => {
      /* backend not up (plain vite without tauri) — render stays blank */
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
      if (s?.playing) render();
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);

    window.addEventListener("keydown", onKeydown);
    onCleanup(() => {
      window.removeEventListener("keydown", onKeydown);
      cancelAnimationFrame(raf);
      clearTimeout(flashTimeout);
      unlisten?.();
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
            "object-fit": "contain",
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

        {/* Chord selector panel — shown while chord_preview is non-null */}
        <Show when={store.snap?.chord_preview !== null && store.snap?.chord_preview !== undefined}>
          <ChordSelectorPanel snap={store.snap!} />
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
        ,/. ±10 ms · ;/' ±250 ms · V detach
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
        "Space         Play / stop from cursor",
        "P             Play from start",
      ],
    },
    {
      title: "Backing alignment",
      rows: [
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
      title: "Loop / Metronome / Count-in",
      rows: [
        "o             Toggle loop",
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
