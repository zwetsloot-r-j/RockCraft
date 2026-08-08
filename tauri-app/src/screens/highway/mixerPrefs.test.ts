// mixerPrefs.test.ts — the play-screen mixer's persistence rules (M14-C). Pure
// string-in / object-out helpers, so no DOM or storage is involved: a corrupt
// stored entry must degrade to defaults rather than keep the screen from
// opening, and an instrument the catalog no longer offers must be dropped.
import { describe, expect, it } from "vitest";
import type { Instrument, MixerReport } from "../../ipc/bridge";
import {
  applicablePrefs,
  DEFAULT_MIXER_PREFS,
  parseMixerPrefs,
  prefsFromReport,
  serializeMixerPrefs,
} from "./mixerPrefs";

const CATALOG: Instrument[] = [
  { id: "grand_piano", name: "Grand Piano", program: 0 },
  { id: "marimba", name: "Marimba", program: 12 },
];

function report(): MixerReport {
  return {
    player: { instrument: CATALOG[1], gain: 0.75 },
    song: { instrument: CATALOG[0], gain: 0.25 },
    backing_gain: 0.5,
    instruments: CATALOG,
  };
}

describe("parseMixerPrefs", () => {
  it("defaults when nothing is stored", () => {
    expect(parseMixerPrefs(null)).toEqual(DEFAULT_MIXER_PREFS);
  });

  it("defaults on malformed JSON rather than throwing", () => {
    expect(parseMixerPrefs("{not json")).toEqual(DEFAULT_MIXER_PREFS);
    expect(parseMixerPrefs("[1,2,3]")).toEqual(DEFAULT_MIXER_PREFS);
    expect(parseMixerPrefs("null")).toEqual(DEFAULT_MIXER_PREFS);
  });

  it("keeps the fields it recognises and defaults the rest", () => {
    const prefs = parseMixerPrefs(
      JSON.stringify({ song: { instrument: "marimba", gain: 0.4 } }),
    );
    expect(prefs.song).toEqual({ instrument: "marimba", gain: 0.4 });
    expect(prefs.player).toEqual(DEFAULT_MIXER_PREFS.player);
    expect(prefs.backingGain).toBe(1);
  });

  it("clamps stored gains and rejects non-numeric ones", () => {
    const prefs = parseMixerPrefs(
      JSON.stringify({
        player: { instrument: 7, gain: 4 },
        song: { gain: -2 },
        backingGain: "loud",
      }),
    );
    expect(prefs.player).toEqual({ instrument: null, gain: 1 });
    expect(prefs.song.gain).toBe(0);
    expect(prefs.backingGain).toBe(1);
  });

  it("round-trips what it serialised", () => {
    const prefs = prefsFromReport(report());
    expect(parseMixerPrefs(serializeMixerPrefs(prefs))).toEqual(prefs);
  });
});

describe("prefsFromReport", () => {
  it("snapshots instrument ids and every fader", () => {
    expect(prefsFromReport(report())).toEqual({
      player: { instrument: "marimba", gain: 0.75 },
      song: { instrument: "grand_piano", gain: 0.25 },
      backingGain: 0.5,
    });
  });
});

describe("applicablePrefs", () => {
  it("drops an instrument the catalog no longer offers, keeping the gains", () => {
    const stored = {
      player: { instrument: "kazoo", gain: 0.3 },
      song: { instrument: "marimba", gain: 0.6 },
      backingGain: 0.2,
    };
    expect(applicablePrefs(stored, CATALOG)).toEqual({
      player: { instrument: null, gain: 0.3 },
      song: { instrument: "marimba", gain: 0.6 },
      backingGain: 0.2,
    });
  });

  it("leaves a fully-known mix untouched", () => {
    const prefs = prefsFromReport(report());
    expect(applicablePrefs(prefs, CATALOG)).toEqual(prefs);
  });
});
