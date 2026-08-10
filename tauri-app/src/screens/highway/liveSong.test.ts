import { describe, expect, it } from "vitest";
import type { PlayInfo, PlaySpan } from "../../ipc/types";
import { songFromInfo } from "./liveSong";

function info(notes: PlaySpan[], over: Partial<PlayInfo> = {}): PlayInfo {
  return {
    title: "test",
    notes,
    shift_us: 3_500_000,
    duration_us: 4_000_000,
    lead_us: 2_000_000,
    has_backing: false,
    video: null,
    backgrounds: [],
    hear_song: true,
    bpm: 120,
    beats_per_bar: 4,
    ...over,
  };
}

describe("songFromInfo", () => {
  it("maps the backend's effective hand through to the engine", () => {
    const song = songFromInfo(
      info([
        { note: 48, start: 0, end: 200, hand: "left" },
        { note: 72, start: 200, end: 400, hand: "right" },
      ]),
    );
    expect(song.notes.map((n) => n.hand)).toEqual(["L", "R"]);
  });

  it("takes the hand from the payload, never re-deriving it from pitch", () => {
    // A crossover: a low note the piece assigns to the RIGHT hand, and a high
    // note it assigns to the LEFT. A pitch-split guess would flip both.
    const song = songFromInfo(
      info([
        { note: 40, start: 0, end: 100, hand: "right" },
        { note: 90, start: 100, end: 200, hand: "left" },
      ]),
    );
    expect(song.notes.map((n) => n.hand)).toEqual(["R", "L"]);
  });

  it("keeps the ms bounds and pitch untouched", () => {
    const song = songFromInfo(
      info([{ note: 60, start: 3500, end: 3700, hand: "right" }]),
    );
    expect(song.notes[0]).toEqual({
      note: 60,
      start: 3500,
      end: 3700,
      hand: "R",
    });
  });

  it("derives the bar/beat grid from the piece tempo", () => {
    const song = songFromInfo(info([], { bpm: 90, beats_per_bar: 3 }));
    expect(song.tempoBpm).toBe(90);
    expect(song.BEAT).toBeCloseTo(60000 / 90);
    expect(song.BAR).toBeCloseTo((60000 / 90) * 3);
    expect(song.timeSig).toBe("3/4");
  });

  it("falls back to 120/4 when the bundle declares no grid", () => {
    const song = songFromInfo(info([], { bpm: 0, beats_per_bar: 0 }));
    expect(song.tempoBpm).toBe(120);
    expect(song.timeSig).toBe("4/4");
  });
});
