// mixerPrefs.ts — persistence for the play-screen mixer (M14-C).
//
// The mix itself lives in the backend (`core::Mixer`, applied by
// `crates/audio`); it is per-run state, so a fresh launch would open back on
// grand piano at unity. These helpers remember the last choice on *this
// machine* in `localStorage` and hand it back on mount — the same
// machine-local-scratch pattern the edit screen uses for video calibration.
//
// Everything here is pure and storage-agnostic: parsing takes a raw string,
// serialising returns one. The screen owns the `localStorage` calls, the tests
// own plain strings.

import type { Instrument, MixerReport } from "../../ipc/bridge";

/** `localStorage` key the play screen reads/writes. */
export const MIXER_PREFS_KEY = "rockcraft.mixer";

/** One synth bus's remembered settings. */
export interface BusPrefs {
  /**
   * Instrument id, or `null` for "whatever the backend defaults to". Null is
   * the honest value for a first run: the catalog is the backend's, so the
   * webview never hardcodes a default id of its own.
   */
  instrument: string | null;
  /** Linear level, 0.0–1.0. */
  gain: number;
}

/** The whole remembered mix. */
export interface MixerPrefs {
  player: BusPrefs;
  song: BusPrefs;
  backingGain: number;
}

/** A first run: no instrument opinion, every fader open. */
export const DEFAULT_MIXER_PREFS: MixerPrefs = {
  player: { instrument: null, gain: 1 },
  song: { instrument: null, gain: 1 },
  backingGain: 1,
};

/** Clamp a stored number into 0.0–1.0, falling back for anything non-finite. */
function gainOr(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.min(1, Math.max(0, value))
    : fallback;
}

/** Accept a stored instrument id only if it is a non-empty string. */
function instrumentOr(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function busOr(value: unknown, fallback: BusPrefs): BusPrefs {
  if (typeof value !== "object" || value === null) return fallback;
  const bus = value as Record<string, unknown>;
  return {
    instrument: instrumentOr(bus.instrument),
    gain: gainOr(bus.gain, fallback.gain),
  };
}

/**
 * Parse stored prefs, tolerating anything: absent, malformed JSON, missing
 * fields, wrong types, out-of-range gains. A corrupt entry must never keep the
 * play screen from opening, so every bad part falls back to its default.
 */
export function parseMixerPrefs(raw: string | null): MixerPrefs {
  if (raw === null) return DEFAULT_MIXER_PREFS;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return DEFAULT_MIXER_PREFS;
  }
  if (typeof parsed !== "object" || parsed === null) return DEFAULT_MIXER_PREFS;
  const obj = parsed as Record<string, unknown>;
  return {
    player: busOr(obj.player, DEFAULT_MIXER_PREFS.player),
    song: busOr(obj.song, DEFAULT_MIXER_PREFS.song),
    backingGain: gainOr(obj.backingGain, DEFAULT_MIXER_PREFS.backingGain),
  };
}

/** Serialise prefs for storage. */
export function serializeMixerPrefs(prefs: MixerPrefs): string {
  return JSON.stringify(prefs);
}

/** Snapshot the backend's mix as prefs to store. */
export function prefsFromReport(report: MixerReport): MixerPrefs {
  return {
    player: { instrument: report.player.instrument.id, gain: report.player.gain },
    song: { instrument: report.song.instrument.id, gain: report.song.gain },
    backingGain: report.backing_gain,
  };
}

/**
 * Drop instrument ids the live catalog no longer offers, so a curated-list
 * change (or a downgrade) can't leave the panel pointing at a sound the
 * backend would reject. The gains always survive — they are just numbers.
 */
export function applicablePrefs(
  prefs: MixerPrefs,
  catalog: Instrument[],
): MixerPrefs {
  const known = (id: string | null): string | null =>
    id !== null && catalog.some((i) => i.id === id) ? id : null;
  return {
    player: { instrument: known(prefs.player.instrument), gain: prefs.player.gain },
    song: { instrument: known(prefs.song.instrument), gain: prefs.song.gain },
    backingGain: prefs.backingGain,
  };
}
