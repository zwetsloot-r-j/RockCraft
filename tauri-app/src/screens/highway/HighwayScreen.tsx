// HighwayScreen.tsx — the Spectrum Live (Design D) play screen.
//
// Always live (#187): the screen is opened from the library / "Play last
// recording" with a bundle `dir`. It loads the bundle (`play_load`), drives the
// HighwayCanvas off the backend `play_state` event (real `core::PlayClock` +
// scoring, never the render loop), and shows an end-of-take summary. Keys:
// m hear-song, w wait mode, Enter replay (on summary), Esc → menu (handled by
// the shell router).
//
// Opening Play without a `dir` is just a guard (reaching it requires a bundle):
// it shows a centered empty state instead of any canned fixture.

import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  onPlayState,
  playFinish,
  playLoad,
  playSetPractice,
  playSetSplit,
  playSetWait,
  playToggleHearSong,
  playToggleMonitor,
  playTogglePause,
} from "../../ipc/bridge";
import type {
  BackgroundLayerView,
  BackgroundTransform,
  BackgroundVideoView,
  PlayStateEvent,
  PlaySummary,
} from "../../ipc/types";
import {
  midiRescan,
  midiStatus,
  onMidiEvent,
  type MidiStatus,
} from "../../ipc/midi";
import { useRouter } from "../../shell/Router";
import { IDENTITY_TRANSFORM, layerStyle } from "./backgrounds";
import { HighwayCanvas } from "./HighwayCanvas";
import { HighwayHeader } from "./HighwayHeader";
import { MixerPanel } from "./MixerPanel";
import { PlaySummaryPanel } from "./PlaySummaryPanel";
import { songFromInfo } from "./liveSong";
import type { HighwayConfig, SongData } from "./types";

/** Empty song used as the initial signal value before a bundle loads. */
const EMPTY_SONG: SongData = {
  title: "",
  artist: "",
  key: "",
  timeSig: "4/4",
  tempoBpm: 120,
  notes: [],
  chords: [],
  LOOP: 0,
  BEAT: 500,
  BAR: 2000,
};

const cfgFusion: HighwayConfig = {
  colorMode: "spectrum",
  perspective: 0,
  glow: 0.32,
  gridlines: "soft",
  keyboard: "flat",
  labels: true,
  pitchRuler: true,
  kbRatio: 0.2,
  lead: 3000,
  bg: "#0f1016",
  hitLine: "#aab2d0",
  noteGap: 0.2,
  radius: 3,
  scoring: true,
  laneTint: "rgba(255,255,255,0.012)",
};

/** localStorage key for the persisted wait-mode preference. */
const WAIT_PREF_KEY = "rc.play.waitMode";

/** Read the saved wait-mode preference; defaults to **on** when never set. */
function readWaitPref(): boolean {
  try {
    const v = localStorage.getItem(WAIT_PREF_KEY);
    return v === null ? true : v === "1";
  } catch {
    return true;
  }
}

/** Persist the wait-mode preference so it is remembered across takes/sessions. */
function writeWaitPref(on: boolean): void {
  try {
    localStorage.setItem(WAIT_PREF_KEY, on ? "1" : "0");
  } catch {
    /* private mode / storage disabled — non-fatal, just don't persist */
  }
}

/** Hand-practice mode: which hand the player practices ("both" = whole piece). */
type Practice = "both" | "left" | "right";
const PRACTICE_KEY = "rc.play.practice";
const SPLIT_KEY = "rc.play.split";

function readPractice(): Practice {
  try {
    const v = localStorage.getItem(PRACTICE_KEY);
    return v === "left" || v === "right" ? v : "both";
  } catch {
    return "both";
  }
}
function readSplit(): number {
  try {
    const v = Number(localStorage.getItem(SPLIT_KEY));
    return Number.isInteger(v) && v >= 21 && v <= 108 ? v : 60;
  } catch {
    return 60;
  }
}
function writeLS(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* non-fatal */
  }
}

const NOTE_NAMES = [
  "C",
  "C#",
  "D",
  "D#",
  "E",
  "F",
  "F#",
  "G",
  "G#",
  "A",
  "A#",
  "B",
];
/** MIDI pitch → short name (e.g. 60 → "C4"). */
function noteName(pitch: number): string {
  return `${NOTE_NAMES[pitch % 12]}${Math.floor(pitch / 12) - 1}`;
}
/** The bridge `hand` argument for a practice mode ("both" → null). */
function practiceArg(p: Practice): "left" | "right" | null {
  return p === "both" ? null : p;
}

export function HighwayScreen() {
  const { screen, navigate } = useRouter();
  const scr = screen();
  const dir = scr.kind === "play" ? scr.dir : undefined;
  const live = dir !== undefined;

  let canvasEl!: HTMLCanvasElement;
  // ── Background video backdrop (M9-G) ───────────────────────────────────────
  // The piece's persisted background video (from `meta.video`), rendered behind
  // the highway canvas and scrubbed to song time with `offset_us`. `null` when
  // the piece has no video. `videoEl` is paused/muted; we set `currentTime`
  // rather than calling `play()`, exactly like the edit-grid backdrop (N).
  let videoEl: HTMLVideoElement | undefined;
  const [video, setVideo] = createSignal<BackgroundVideoView | null>(null);
  // Whole-song shift in µs; the video aligns to song *content*, which begins
  // after the pre-roll shift. videoTime = (songTime - shift) + offset.
  let shiftUs = 0;
  // ── Background image layers (M14-D) ────────────────────────────────────────
  // The piece's keyframed backdrops, back-to-front behind the highway canvas.
  // `layers` is the static half (id + file) from `play_load`; `transforms` is
  // the moving half, re-evaluated by `core` on every `play_state` tick — the
  // webview never interpolates, it only applies what it is handed.
  const [bgLayers, setBgLayers] = createSignal<BackgroundLayerView[]>([]);
  const [bgTransforms, setBgTransforms] = createSignal<
    Record<string, BackgroundTransform>
  >({});
  const [eng, setEng] = createSignal<HighwayCanvas | null>(null);
  // ~9 fps throttle so the header re-reads without a per-frame render.
  const [frame, setFrame] = createSignal(0);
  const [playState, setPlayState] = createSignal<PlayStateEvent | null>(null);
  const [summary, setSummary] = createSignal<PlaySummary | null>(null);
  const [song, setSong] = createSignal<SongData>(EMPTY_SONG);
  const [loadErr, setLoadErr] = createSignal<string | null>(null);
  const [hearSong, setHearSong] = createSignal(false);
  // Input monitor: synthesise the player's own key presses (`n`), off by default.
  const [monitor, setMonitor] = createSignal(false);
  // Wait-mode preference persists across takes/sessions (defaults on).
  const [waitMode, setWaitMode] = createSignal(readWaitPref());
  // Hand-practice mode + the left/right split pitch, both persisted.
  const [practice, setPractice] = createSignal<Practice>(readPractice());
  const [split, setSplit] = createSignal(readSplit());
  // Live MIDI device status, shown on the Start overlay so the player can tell
  // whether their piano is connected (and reconnect if it was powered on late).
  const [midiInfo, setMidiInfo] = createSignal<MidiStatus | null>(null);
  const [rescanning, setRescanning] = createSignal(false);
  // The take loads paused (backend `start_paused`); it does not advance until the
  // player hits Start. This gates auto-run before the window is focused and makes
  // Replay re-enter the same ready-to-start state.
  const [started, setStarted] = createSignal(false);

  let finished = false;
  // Wall-clock (performance.now) when the result summary appeared, so a piano
  // key only "continues" after a short grace — keys still held/released from the
  // final note must not dismiss the result instantly.
  let summaryShownAt = 0;

  // ── live key handling ──────────────────────────────────────────────────
  function onKeydown(e: KeyboardEvent): void {
    if (!live) return;
    // On the summary panel: Enter replays the same bundle. Esc → menu is the
    // shell router's job.
    if (summary()) {
      if (e.key === "Enter") {
        e.preventDefault();
        replay();
      }
      return;
    }
    // Before the take is started, Space/Enter begins it (the Start prompt).
    if (!started()) {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        start();
      }
      return;
    }
    switch (e.key) {
      case " ":
        // Play/pause toggle once running.
        e.preventDefault();
        void playTogglePause();
        break;
      case "m":
      case "M":
        e.preventDefault();
        void playToggleHearSong().then(setHearSong);
        break;
      case "w":
      case "W":
        e.preventDefault();
        void playSetWait(!waitMode()).then((on) => {
          setWaitMode(on);
          writeWaitPref(on);
        });
        break;
      case "n":
      case "N":
        // Input monitor: hear your own key presses through the synth.
        e.preventDefault();
        void playToggleMonitor().then(setMonitor);
        break;
      case "h":
      case "H":
        // Cycle hand-practice: both → right → left → both.
        e.preventDefault();
        cyclePractice();
        break;
      case ",":
        // Move the left/right split down a semitone.
        e.preventDefault();
        nudgeSplit(-1);
        break;
      case ".":
        // Move the left/right split up a semitone.
        e.preventDefault();
        nudgeSplit(1);
        break;
      default:
        break;
    }
  }

  /** Scrub the backdrop `<video>` to the current song time. videoTime =
   * (songTime - shift) + offset, clamped at 0, in seconds. We set `currentTime`
   * and never `play()` so the frame tracks the (pausable) song clock exactly. */
  function syncVideo(timeUs: number): void {
    const v = videoEl;
    const meta = video();
    if (!v || meta === null) return;
    const tUs = timeUs - shiftUs + meta.offset_us;
    const t = Math.max(0, tUs) / 1e6;
    if (Number.isFinite(t) && Math.abs(v.currentTime - t) > 0.05) {
      v.currentTime = t;
    }
  }

  function applyState(s: PlayStateEvent): void {
    setPlayState(s);
    const e = eng();
    // Engine time is ms; backend time is µs.
    if (e) {
      e.setLiveState(s.time_us / 1000, s.frozen, s.held, s.awaiting);
      // Per-note hit/near/miss effects (M14-B). The backend sends each judged
      // note once, so pushing whatever arrived spawns exactly one effect each.
      if (s.judgments.length > 0) e.pushJudgments(s.judgments);
    }
    syncVideo(s.time_us);
    if (s.backgrounds.length > 0) {
      const next: Record<string, BackgroundTransform> = {};
      for (const b of s.backgrounds) next[b.id] = b.transform;
      setBgTransforms(next);
    }
    if (s.finished && !finished) {
      finished = true;
      // Halt the render loop so the highway holds behind the summary panel
      // instead of interpolating forward (which read as a "restart" under the
      // dialog once no more play_state updates arrive).
      eng()?.stop();
      void playFinish().then((sm) => {
        summaryShownAt = performance.now();
        setSummary(sm);
      });
    }
  }

  function startLive(bundleDir: string): void {
    playLoad(bundleDir)
      .then((info) => {
        setHearSong(info.hear_song);
        shiftUs = info.shift_us;
        const sd = songFromInfo(info);
        setSong(sd);
        const engine = new HighwayCanvas(
          canvasEl,
          { ...cfgFusion, lead: info.lead_us / 1000 },
          sd,
        );
        engine.setLive(true);
        // Background video backdrop (M9-G): when the piece carries one, draw the
        // highway over a translucent fill so the <video> behind shows through.
        setVideo(info.video ?? null);
        // Background images (M14-D) count as a backdrop too: the highway draws
        // over a translucent fill so whatever sits behind it shows through.
        const layers = info.backgrounds ?? [];
        setBgLayers(layers);
        setBgTransforms({});
        engine.setBackdrop(info.video != null || layers.length > 0);
        if (videoEl) {
          if (info.video) {
            videoEl.src = convertFileSrc(info.video.path);
            videoEl.load();
            videoEl.currentTime = 0;
          } else {
            videoEl.removeAttribute("src");
            videoEl.load();
          }
        }
        engine.start();
        setEng(engine);
        // The backend session loaded paused; wait for Start before advancing.
        setStarted(false);
        // A fresh session is disarmed; apply the persisted wait-mode preference
        // so it is remembered across takes/sessions and Replay.
        void playSetWait(waitMode()).then(setWaitMode);
        // A fresh session has monitor off; re-enable it if the player had it on
        // (keeps the input-monitor setting across Replay/re-entry).
        if (monitor()) void playToggleMonitor().then(setMonitor);
        // Apply the persisted hand-practice mode + split to the fresh session.
        void playSetSplit(split());
        void playSetPractice(practiceArg(practice()));
      })
      .catch((err) => setLoadErr(String(err)));
  }

  /** Begin (or resume) the paused take — the Start button / Space / a piano key. */
  function start(): void {
    void playTogglePause().then(() => setStarted(true));
  }

  /** Refresh the shown MIDI device status. */
  function refreshMidi(): void {
    void midiStatus().then(setMidiInfo);
  }

  /** Re-scan for a piano powered on after launch and adopt it if found. */
  function rescan(): void {
    setRescanning(true);
    void midiRescan()
      .then(setMidiInfo)
      .finally(() => setRescanning(false));
  }

  /** Cycle the practiced hand: both → right → left → both. */
  function cyclePractice(): void {
    const next: Practice =
      practice() === "both"
        ? "right"
        : practice() === "right"
          ? "left"
          : "both";
    setPractice(next);
    writeLS(PRACTICE_KEY, next);
    void playSetPractice(practiceArg(next)).then((v) => setPractice(v as Practice));
  }

  /** Move the left/right split pitch by `delta` semitones (clamped to 21..108). */
  function nudgeSplit(delta: number): void {
    const next = Math.max(21, Math.min(108, split() + delta));
    setSplit(next);
    writeLS(SPLIT_KEY, String(next));
    void playSetSplit(next).then(setSplit);
  }

  function replay(): void {
    if (dir === undefined) return;
    finished = false;
    setStarted(false);
    setSummary(null);
    setPlayState(null);
    eng()?.stop();
    setEng(null);
    startLive(dir);
  }

  onMount(() => {
    // No bundle → empty-state guard; nothing to drive (Esc → menu via shell).
    if (!live || dir === undefined) return;

    let unlisten: (() => void) | undefined;
    void onPlayState((s) => applyState(s)).then((u) => (unlisten = u));

    // A piano key-press also clears the Start prompt (#2): while the take has not
    // begun, any note-on starts it — the player can just start playing. (This
    // also confirms a live device, so refresh the shown status.)
    let unlistenMidi: (() => void) | undefined;
    void onMidiEvent((ev) => {
      if (!(ev.on && ev.velocity > 0)) return;
      if (summary()) {
        // Continue (replay) on a piano key — but only after a ~1s grace so keys
        // still held/released from the final note don't dismiss it immediately.
        if (performance.now() - summaryShownAt >= 1000) replay();
        return;
      }
      // Before the take begins, any piano key starts it (and confirms the device).
      if (!started()) {
        refreshMidi();
        start();
      }
    }).then((u) => (unlistenMidi = u));

    refreshMidi();
    startLive(dir);
    window.addEventListener("keydown", onKeydown);

    const id = setInterval(() => setFrame((f) => f + 1), 110);
    onCleanup(() => {
      clearInterval(id);
      if (unlisten) unlisten();
      if (unlistenMidi) unlistenMidi();
      window.removeEventListener("keydown", onKeydown);
      eng()?.stop();
      // Always tear the backend session down when leaving (Esc / unmount).
      void playFinish().catch(() => {});
    });
  });

  return (
    <div
      style={{
        height: "100%",
        display: "flex",
        "flex-direction": "column",
        background: "#0f1016",
        "font-family": "'Space Grotesk', system-ui, sans-serif",
      }}
    >
      <Show
        when={live}
        fallback={
          <div
            style={{
              flex: "1 1 auto",
              display: "flex",
              "align-items": "center",
              "justify-content": "center",
              color: "#7c8094",
              "font-size": "14px",
              "text-align": "center",
              padding: "0 24px",
            }}
          >
            Nothing to play — open a bundle from the Library.
          </div>
        }
      >
        <HighwayHeader
          eng={eng}
          frame={frame}
          song={song()}
          live={live}
          playState={playState}
          hearSong={hearSong}
          waitMode={waitMode}
          monitor={monitor}
          practice={practice}
          splitName={() => noteName(split())}
        />
        <div style={{ flex: "1 1 auto", "min-height": 0, position: "relative" }}>
          {/* Background video backdrop (M9-G) — sits *behind* the canvas (lower
              z-index). Paused/muted; we scrub `currentTime`, never `play()`.
              Hidden until the piece carries a video. */}
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
              "pointer-events": "none",
              display: video() === null ? "none" : "block",
              background: "#000",
            }}
          />
          {/* Background image layers (M14-D) — one <img> per layer, stacked
              back-to-front inside their own z-index:0 stacking context. That
              places them above the movie backdrop (so a translucent layer can
              sit over it) and below the highway canvas. Each transform was
              evaluated by `core` against the play clock; here we only turn it
              into CSS. */}
          <Show when={bgLayers().length > 0}>
            <div
              style={{
                position: "absolute",
                inset: 0,
                "z-index": 0,
                "pointer-events": "none",
                overflow: "hidden",
              }}
            >
              <For each={bgLayers()}>
                {(layer, i) => (
                  <img
                    src={convertFileSrc(layer.path)}
                    alt=""
                    draggable={false}
                    style={layerStyle(
                      bgTransforms()[layer.id] ?? IDENTITY_TRANSFORM,
                      i(),
                    )}
                  />
                )}
              </For>
            </div>
          </Show>
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
          {/* Sound + levels (M14-C): instrument per voice and a fader each for
              you / the song / the backing. Collapsed to a button until opened;
              sits above the canvas, below the summary panel. */}
          <MixerPanel />
          <Show when={loadErr()}>
            <div
              style={{
                position: "absolute",
                inset: "0",
                display: "flex",
                "align-items": "center",
                "justify-content": "center",
                color: "#ff8089",
                "font-size": "14px",
              }}
            >
              failed to load bundle: {loadErr()}
            </div>
          </Show>
          <Show when={!started() && !summary() && !loadErr()}>
            <div
              style={{
                position: "absolute",
                inset: "0",
                display: "flex",
                "flex-direction": "column",
                "align-items": "center",
                "justify-content": "center",
                gap: "16px",
                "z-index": 2,
                background: "rgba(15,16,22,0.55)",
              }}
            >
              <button
                onClick={() => start()}
                style={{
                  padding: "14px 40px",
                  "font-size": "18px",
                  "font-weight": "600",
                  color: "#0f1016",
                  background: "#8fb6ff",
                  border: "none",
                  "border-radius": "10px",
                  cursor: "pointer",
                  "font-family": "'Space Grotesk', system-ui, sans-serif",
                }}
              >
                ▶ Start
              </button>
              <div
                style={{
                  color: "#9aa0b4",
                  "font-size": "13px",
                  "text-align": "center",
                }}
              >
                Press Space — or play any key — to start
                {waitMode() ? " · wait mode on (pauses on each note)" : ""}
              </div>
              {/* MIDI device status + reconnect (a piano powered on after launch
                  is otherwise missed; rescan adopts it). */}
              <div
                style={{
                  display: "flex",
                  "align-items": "center",
                  gap: "10px",
                  "font-size": "12px",
                  color: "#7c8094",
                }}
              >
                <span>
                  {midiInfo()?.kind === "live"
                    ? `🎹 ${midiInfo()?.port ?? "connected"}`
                    : "no piano detected"}
                </span>
                <Show when={midiInfo()?.kind !== "live"}>
                  <button
                    onClick={() => rescan()}
                    disabled={rescanning()}
                    style={{
                      padding: "5px 12px",
                      "font-size": "12px",
                      color: "#c8cce0",
                      background: "transparent",
                      border: "1px solid #3a3f52",
                      "border-radius": "6px",
                      cursor: rescanning() ? "default" : "pointer",
                      "font-family": "'Space Grotesk', system-ui, sans-serif",
                    }}
                  >
                    {rescanning() ? "searching…" : "🔍 Reconnect piano"}
                  </button>
                </Show>
              </div>
            </div>
          </Show>
          <Show when={summary()}>
            <PlaySummaryPanel
              summary={summary()!}
              onReplay={replay}
              onMenu={() => navigate({ kind: "menu" })}
            />
          </Show>
        </div>
      </Show>
    </div>
  );
}
