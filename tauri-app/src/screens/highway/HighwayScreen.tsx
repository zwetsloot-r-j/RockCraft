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
  playSetRate,
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
/**
 * How much of the backdrop movie to show behind the highway.
 *
 * An imported piece's movie is a note tutorial, so its own falling notes sit
 * right where ours do and compete with them. `dim` keeps the scene readable
 * (artwork, the performer) while pushing its notes back; `off` removes it.
 */
type Backdrop = "on" | "dim" | "off";
const BACKDROP_KEY = "rc.play.backdrop";
/** Opacity the backdrop `<video>` renders at in each mode. */
const BACKDROP_OPACITY: Record<Backdrop, number> = {
  on: 1,
  dim: 0.22,
  off: 0,
};

function readBackdrop(): Backdrop {
  try {
    const v = localStorage.getItem(BACKDROP_KEY);
    return v === "dim" || v === "off" ? v : "on";
  } catch {
    return "on";
  }
}
/** Practice speed presets in permille, cycled with `-` / `=`. */
const RATE_STEPS = [500, 625, 750, 875, 1000] as const;
const RATE_KEY = "rc.play.rate";
function readRate(): number {
  try {
    const v = Number(localStorage.getItem(RATE_KEY));
    return RATE_STEPS.includes(v as (typeof RATE_STEPS)[number]) ? v : 1000;
  } catch {
    return 1000;
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
  // Backdrop servo state (see driveVideo): wall-clock of the last seek, so seeks
  // stay throttled, and the previous target so a backward jump is detectable.
  let lastVideoSeekMs = 0;
  let prevVideoWant = -1;
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
  // How much of the backdrop movie to show, persisted across takes/sessions.
  const [backdrop, setBackdrop] = createSignal<Backdrop>(readBackdrop());
  // Practice speed (permille), persisted across takes/sessions.
  const [rate, setRate] = createSignal(readRate());
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
      case "-":
      case "_":
        // Slow the take down one step.
        e.preventDefault();
        nudgeRate(-1);
        break;
      case "=":
      case "+":
        // Speed the take back up one step (never past 1x).
        e.preventDefault();
        nudgeRate(1);
        break;
      case "v":
      case "V":
        // Cycle the backdrop movie: on → dim → off.
        e.preventDefault();
        cycleBackdrop();
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

  /**
   * Drive the backdrop `<video>` from the song clock. videoTime =
   * (songTime - shift) + offset, clamped at 0, in seconds.
   *
   * This used to set `currentTime` on every `play_state` tick whenever the frame
   * drifted more than 50 ms — which, since the clock advances continuously, meant
   * a *seek per tick* for the whole take. WebView2 cannot decode and repaint a
   * `currentTime` set 30-60x/second, so the element spent playback in a
   * permanent seek-stall: the stutter this replaces. `EditScreen.driveVideo`
   * already solved the same problem; this mirrors it.
   *
   * Playing: let the clip run natively on its own clock and correct drift by
   * nudging `playbackRate` — a phase-lock with no seeks at all. A muted backdrop
   * briefly running at up to 2x to close a gap is invisible, and the decoder
   * stays fed the whole time.
   *
   * Frozen (wait-mode or paused): the element is idle, so seek it exactly —
   * throttled, and only while the gap is real, so a held note cannot become a
   * per-tick reseek loop that stalls the decoder black.
   */
  function driveVideo(timeUs: number, frozen: boolean): void {
    const v = videoEl;
    const meta = video();
    // Hidden backdrop: skip the work entirely. Without this, `v` stopped the
    // element compositing but left it seeking every tick — which is why turning
    // the movie off did nothing for the stutter.
    if (!v || meta === null || backdrop() === "off") return;
    const dur = v.duration;
    if (!Number.isFinite(dur) || dur <= 0) return; // metadata not ready yet
    const want = Math.min(
      Math.max((timeUs - shiftUs + meta.offset_us) / 1e6, 0),
      dur,
    );

    if (frozen || !started()) {
      if (!v.paused) v.pause();
      if (v.playbackRate !== 1) v.playbackRate = 1;
      prevVideoWant = -1; // resuming re-seats without a false back-jump
      const now = performance.now();
      if (Math.abs(want - v.currentTime) > 0.1 && !v.seeking && now - lastVideoSeekMs >= 250) {
        lastVideoSeekMs = now;
        v.currentTime = want;
      }
      return;
    }

    // A backward jump (replay, or scrubbing back) must SEEK: `want` dropped, so
    // the clip is now far ahead and slowing it would never converge.
    const backJump = prevVideoWant >= 0 && want < prevVideoWant - 0.2;
    prevVideoWant = want;
    if (backJump) {
      v.currentTime = want;
      v.playbackRate = 1;
      lastVideoSeekMs = performance.now();
      return;
    }

    // Prefer native playback. If muted autoplay is blocked — common in WebView2
    // — the element stays paused; fall back to a *throttled* seek (~12 fps)
    // rather than one per tick.
    if (v.paused) void v.play().catch(() => {});
    if (v.paused) {
      const now = performance.now();
      if (now - lastVideoSeekMs >= 80) {
        lastVideoSeekMs = now;
        v.currentTime = want;
      }
      return;
    }

    const drift = want - v.currentTime; // +: video behind, must speed up
    if (Math.abs(drift) > 3) {
      // Too far to servo in reasonable time; one throttled hard seek.
      const now = performance.now();
      if (now - lastVideoSeekMs >= 1000) {
        lastVideoSeekMs = now;
        v.currentTime = want;
        v.playbackRate = 1;
      }
      return;
    }
    v.playbackRate = Math.max(0.5, Math.min(2, 1 + drift * 0.8));
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
    driveVideo(s.time_us, s.frozen);
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
        // The piece's authored split (meta.hand_split) is authoritative: seed
        // the play-mode split from it so moving the splitter in edit mode
        // takes effect here, instead of a machine-global localStorage value
        // clobbering it (which made right-hand practice include left notes).
        setSplit(info.split_pitch);
        const sd = songFromInfo(info);
        setSong(sd);
        const engine = new HighwayCanvas(
          canvasEl,
          { ...cfgFusion, lead: info.lead_us / 1000 },
          sd,
        );
        engine.setLive(true);
        engine.setPractice(practice(), split());
        // Background video backdrop (M9-G): when the piece carries one, draw the
        // highway over a translucent fill so the <video> behind shows through.
        setVideo(info.video ?? null);
        // Background images (M14-D) count as a backdrop too: the highway draws
        // over a translucent fill so whatever sits behind it shows through.
        const layers = info.backgrounds ?? [];
        setBgLayers(layers);
        setBgTransforms({});
        engine.setBackdrop(
          (info.video != null && backdrop() !== "off") || layers.length > 0,
        );
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
        // Re-apply the persisted practice speed to the fresh session.
        if (rate() !== 1000) void playSetRate(rate());
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

  /** Step the practice speed through RATE_STEPS. Slowing mutes the backing
   * recording (it cannot follow without resampling); the synth carries on. */
  function nudgeRate(delta: number): void {
    const i = RATE_STEPS.indexOf(rate() as (typeof RATE_STEPS)[number]);
    const at = i === -1 ? RATE_STEPS.length - 1 : i;
    const next = RATE_STEPS[Math.max(0, Math.min(RATE_STEPS.length - 1, at + delta))];
    if (next === rate()) return;
    setRate(next);
    writeLS(RATE_KEY, String(next));
    void playSetRate(next).then((v) => setRate(v));
  }

  /** Cycle the backdrop movie: on → dim → off. View-only — the play session is
   * untouched, so this never affects timing, scoring or the wait gate. */
  function cycleBackdrop(): void {
    const next: Backdrop =
      backdrop() === "on" ? "dim" : backdrop() === "dim" ? "off" : "on";
    setBackdrop(next);
    writeLS(BACKDROP_KEY, next);
    // The highway paints an opaque bg when nothing shows behind it, so a hidden
    // movie must not leave the canvas translucent over black.
    eng()?.setBackdrop(hasVisibleBackdrop(next));
  }

  /** Whether anything is actually visible behind the highway canvas. */
  function hasVisibleBackdrop(mode: Backdrop = backdrop()): boolean {
    const movieShows = video() !== null && mode !== "off";
    return movieShows || bgLayers().length > 0;
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
    eng()?.setPractice(next, split());
    void playSetPractice(practiceArg(next)).then((v) => setPractice(v as Practice));
  }

  /** Move the left/right split pitch by `delta` semitones (clamped to 21..108). */
  function nudgeSplit(delta: number): void {
    const next = Math.max(21, Math.min(108, split() + delta));
    setSplit(next);
    writeLS(SPLIT_KEY, String(next));
    eng()?.setPractice(practice(), next);
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
          backdrop={backdrop}
          rate={rate}
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
              // `off` is display:none rather than opacity:0 so the element stops
              // compositing entirely — the movie's notes must not merely fade,
              // they must cost nothing.
              display:
                video() === null || backdrop() === "off" ? "none" : "block",
              opacity: BACKDROP_OPACITY[backdrop()],
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
