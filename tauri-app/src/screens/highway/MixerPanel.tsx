// MixerPanel.tsx — sound selection + the three faders (M14-C).
//
// Sits over the top-right of the highway, collapsed to a "Mix" button until
// clicked. Inside: an instrument dropdown per synth voice (You / Song) and a
// level slider for each of the three sources — You, Song, Backing.
//
// The backend owns the mix (`core::Mixer`, applied by `crates/audio`); this
// panel is a view over it. Every control calls straight through and re-renders
// from the returned {@link MixerReport}, so the panel can never drift from what
// is actually sounding. The choice is also written to `localStorage` and
// re-applied on mount, since the backend's mix is per-run (see `mixerPrefs`).
//
// The instrument list comes from the backend catalog — nothing here hardcodes
// a sound. Whether a program is audible depends on the loaded SoundFont: a
// single-preset piano bank falls back to its one preset
// (`crates/audio/assets/NOTICE.md`).

import { createSignal, For, onMount, Show } from "solid-js";
import {
  mixerStatus,
  setBusGain,
  setInstrument,
  type MixerBus,
  type MixerReport,
  type SynthBus,
} from "../../ipc/bridge";
import {
  applicablePrefs,
  MIXER_PREFS_KEY,
  parseMixerPrefs,
  prefsFromReport,
  serializeMixerPrefs,
} from "./mixerPrefs";

const monoFont = "'IBM Plex Mono', monospace";

/** The three faders, in signal order: your notes, the song, the recording. */
const FADERS: { bus: MixerBus; label: string; hint: string }[] = [
  { bus: "player", label: "You", hint: "the notes you play" },
  { bus: "song", label: "Song", hint: "the auto-played chart" },
  { bus: "backing", label: "Backing", hint: "the backing track" },
];

/** The two voices that carry an instrument. */
const VOICES: { bus: SynthBus; label: string }[] = [
  { bus: "player", label: "You" },
  { bus: "song", label: "Song" },
];

export function MixerPanel() {
  const [open, setOpen] = createSignal(false);
  const [mix, setMix] = createSignal<MixerReport | null>(null);
  const [err, setErr] = createSignal<string | null>(null);

  /** Remember the current mix on this machine. */
  function persist(report: MixerReport): void {
    try {
      localStorage.setItem(
        MIXER_PREFS_KEY,
        serializeMixerPrefs(prefsFromReport(report)),
      );
    } catch {
      // A full / disabled localStorage must not break the mixer itself.
    }
  }

  /** Adopt a fresh report from the backend and remember it. */
  function adopt(report: MixerReport): void {
    setMix(report);
    setErr(null);
    persist(report);
  }

  function pickInstrument(bus: SynthBus, id: string): void {
    void setInstrument(bus, id).then(adopt).catch((e) => setErr(String(e)));
  }

  function moveFader(bus: MixerBus, gain: number): void {
    void setBusGain(bus, gain).then(adopt).catch((e) => setErr(String(e)));
  }

  onMount(() => {
    // Read the live mix first: its catalog is what validates the stored ids,
    // and it is the fallback if nothing was ever stored.
    void mixerStatus()
      .then(async (report) => {
        let raw: string | null = null;
        try {
          raw = localStorage.getItem(MIXER_PREFS_KEY);
        } catch {
          // Unavailable storage → just run with the backend's mix.
        }
        const prefs = applicablePrefs(parseMixerPrefs(raw), report.instruments);
        // Re-apply the remembered choice one control at a time, keeping the
        // last reply as the truth. Sequential on purpose: each call returns the
        // whole mix, so ordering them keeps the panel from flickering between
        // partially-applied snapshots.
        let latest = report;
        for (const { bus } of VOICES) {
          const id = prefs[bus].instrument;
          if (id !== null && id !== latest[bus].instrument.id) {
            latest = await setInstrument(bus, id);
          }
        }
        for (const { bus } of FADERS) {
          const gain =
            bus === "backing" ? prefs.backingGain : prefs[bus].gain;
          const current =
            bus === "backing" ? latest.backing_gain : latest[bus].gain;
          if (gain !== current) latest = await setBusGain(bus, gain);
        }
        setMix(latest);
      })
      .catch((e) => setErr(String(e)));
  });

  const gainOf = (report: MixerReport, bus: MixerBus): number =>
    bus === "backing" ? report.backing_gain : report[bus].gain;

  return (
    <div
      style={{
        position: "absolute",
        top: "10px",
        right: "10px",
        "z-index": 3,
        "font-family": "'Space Grotesk', system-ui, sans-serif",
        color: "#e7e8ef",
      }}
    >
      <button
        type="button"
        onClick={() => setOpen(!open())}
        aria-expanded={open()}
        title="Sound + levels"
        style={{
          "font-family": monoFont,
          "font-size": "11px",
          "letter-spacing": "0.08em",
          padding: "5px 10px",
          color: "#aab2d0",
          background: "rgba(24,25,34,0.88)",
          border: "1px solid rgba(255,255,255,0.12)",
          "border-radius": "6px",
          cursor: "pointer",
        }}
      >
        MIX
      </button>

      <Show when={open()}>
        <div
          style={{
            "margin-top": "6px",
            width: "230px",
            padding: "12px",
            background: "rgba(24,25,34,0.96)",
            border: "1px solid rgba(255,255,255,0.1)",
            "border-radius": "8px",
            display: "flex",
            "flex-direction": "column",
            gap: "12px",
          }}
        >
          <Show
            when={mix()}
            fallback={
              <div style={{ "font-size": "12px", color: "#7c8094" }}>
                {err() ?? "loading…"}
              </div>
            }
          >
            <For each={VOICES}>
              {(voice) => (
                <label
                  style={{
                    display: "flex",
                    "flex-direction": "column",
                    gap: "4px",
                  }}
                >
                  <span
                    style={{
                      "font-family": monoFont,
                      "font-size": "10px",
                      "letter-spacing": "0.08em",
                      color: "#7c8094",
                    }}
                  >
                    {voice.label.toUpperCase()} SOUND
                  </span>
                  <select
                    value={mix()![voice.bus].instrument.id}
                    onChange={(e) =>
                      pickInstrument(voice.bus, e.currentTarget.value)
                    }
                    style={{
                      "font-family": "inherit",
                      "font-size": "12px",
                      padding: "4px 6px",
                      color: "#e7e8ef",
                      background: "#12131a",
                      border: "1px solid rgba(255,255,255,0.12)",
                      "border-radius": "5px",
                    }}
                  >
                    <For each={mix()!.instruments}>
                      {(inst) => <option value={inst.id}>{inst.name}</option>}
                    </For>
                  </select>
                </label>
              )}
            </For>

            <For each={FADERS}>
              {(fader) => (
                <label
                  style={{
                    display: "flex",
                    "flex-direction": "column",
                    gap: "2px",
                  }}
                  title={fader.hint}
                >
                  <span
                    style={{
                      display: "flex",
                      "justify-content": "space-between",
                      "font-family": monoFont,
                      "font-size": "10px",
                      "letter-spacing": "0.08em",
                      color: "#7c8094",
                    }}
                  >
                    <span>{fader.label.toUpperCase()}</span>
                    <span>{Math.round(gainOf(mix()!, fader.bus) * 100)}%</span>
                  </span>
                  <input
                    type="range"
                    min="0"
                    max="100"
                    step="1"
                    value={Math.round(gainOf(mix()!, fader.bus) * 100)}
                    onInput={(e) =>
                      moveFader(fader.bus, Number(e.currentTarget.value) / 100)
                    }
                    style={{ width: "100%", "accent-color": "#aab2d0" }}
                  />
                </label>
              )}
            </For>

            <Show when={err()}>
              <div style={{ "font-size": "11px", color: "#ff8089" }}>
                {err()}
              </div>
            </Show>
          </Show>
        </div>
      </Show>
    </div>
  );
}
