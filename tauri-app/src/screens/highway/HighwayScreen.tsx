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

import { createSignal, onCleanup, onMount, Show } from "solid-js";
import {
  onPlayState,
  playFinish,
  playLoad,
  playSetWait,
  playToggleHearSong,
} from "../../ipc/bridge";
import type { PlayStateEvent, PlaySummary } from "../../ipc/types";
import { useRouter } from "../../shell/Router";
import { HighwayCanvas } from "./HighwayCanvas";
import { HighwayHeader } from "./HighwayHeader";
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

export function HighwayScreen() {
  const { screen, navigate } = useRouter();
  const scr = screen();
  const dir = scr.kind === "play" ? scr.dir : undefined;
  const live = dir !== undefined;

  let canvasEl!: HTMLCanvasElement;
  const [eng, setEng] = createSignal<HighwayCanvas | null>(null);
  // ~9 fps throttle so the header re-reads without a per-frame render.
  const [frame, setFrame] = createSignal(0);
  const [playState, setPlayState] = createSignal<PlayStateEvent | null>(null);
  const [summary, setSummary] = createSignal<PlaySummary | null>(null);
  const [song, setSong] = createSignal<SongData>(EMPTY_SONG);
  const [loadErr, setLoadErr] = createSignal<string | null>(null);
  const [hearSong, setHearSong] = createSignal(false);
  const [waitMode, setWaitMode] = createSignal(false);

  let finished = false;

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
    switch (e.key) {
      case "m":
      case "M":
        e.preventDefault();
        void playToggleHearSong().then(setHearSong);
        break;
      case "w":
      case "W":
        e.preventDefault();
        void playSetWait(!waitMode()).then(setWaitMode);
        break;
      default:
        break;
    }
  }

  function applyState(s: PlayStateEvent): void {
    setPlayState(s);
    const e = eng();
    // Engine time is ms; backend time is µs.
    if (e) e.setLiveState(s.time_us / 1000, s.frozen, s.held, s.awaiting);
    if (s.finished && !finished) {
      finished = true;
      void playFinish().then(setSummary);
    }
  }

  function startLive(bundleDir: string): void {
    playLoad(bundleDir)
      .then((info) => {
        setHearSong(info.hear_song);
        const sd = songFromInfo(info);
        setSong(sd);
        const engine = new HighwayCanvas(
          canvasEl,
          { ...cfgFusion, lead: info.lead_us / 1000 },
          sd,
        );
        engine.setLive(true);
        engine.start();
        setEng(engine);
      })
      .catch((err) => setLoadErr(String(err)));
  }

  function replay(): void {
    if (dir === undefined) return;
    finished = false;
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
    startLive(dir);
    window.addEventListener("keydown", onKeydown);

    const id = setInterval(() => setFrame((f) => f + 1), 110);
    onCleanup(() => {
      clearInterval(id);
      if (unlisten) unlisten();
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
        />
        <div style={{ flex: "1 1 auto", "min-height": 0, position: "relative" }}>
          <canvas ref={canvasEl} style={{ width: "100%", height: "100%", display: "block" }} />
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
