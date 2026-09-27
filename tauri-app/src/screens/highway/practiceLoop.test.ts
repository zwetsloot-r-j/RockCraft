import { describe, expect, it } from "vitest";
import type { LoopView } from "../../ipc/types";
import {
  dimmedByLoop,
  loopBadgeText,
  loopBand,
  loopKeyCommand,
} from "./practiceLoop";

function view(over: Partial<LoopView> = {}): LoopView {
  return {
    first_bar: 4,
    last_bar: 7,
    start_us: 8_000_000,
    end_us: 16_000_000,
    running: false,
    phase: null,
    pass: 0,
    last_pass: null,
    ...over,
  };
}

describe("loopKeyCommand", () => {
  it("steps bars with the arrows outside a loop", () => {
    expect(loopKeyCommand("ArrowLeft", 3, null)).toEqual({ kind: "seek", delta: -1 });
    expect(loopKeyCommand("ArrowRight", 3, view())).toEqual({ kind: "seek", delta: 1 });
  });

  it("ignores the arrows while the loop runs", () => {
    const running = view({ running: true, phase: "demo", pass: 1 });
    expect(loopKeyCommand("ArrowLeft", 3, running)).toBeNull();
    expect(loopKeyCommand("ArrowRight", 3, running)).toBeNull();
  });

  it("marks with the brackets, running or not", () => {
    expect(loopKeyCommand("[", 3, null)).toEqual({ kind: "mark", edge: "start" });
    expect(loopKeyCommand("]", 3, view({ running: true }))).toEqual({
      kind: "mark",
      edge: "end",
    });
  });

  it("l loops the marks, or the bar under the playhead when none", () => {
    expect(loopKeyCommand("l", 3, view())).toEqual({ kind: "set", firstBar: 4, lastBar: 7 });
    expect(loopKeyCommand("L", 9, null)).toEqual({ kind: "set", firstBar: 9, lastBar: 9 });
  });

  it("l stops a running loop", () => {
    expect(loopKeyCommand("l", 3, view({ running: true, phase: "count_in", pass: 1 }))).toEqual({
      kind: "clear",
    });
  });

  it("other keys are not loop keys", () => {
    expect(loopKeyCommand("w", 0, null)).toBeNull();
    expect(loopKeyCommand(" ", 0, view())).toBeNull();
  });
});

describe("loopBadgeText", () => {
  it("is null with nothing marked", () => {
    expect(loopBadgeText(null)).toBeNull();
  });

  it("names the marked range 1-based", () => {
    expect(loopBadgeText(view())).toBe("Loop 5–8");
    expect(loopBadgeText(view({ first_bar: 2, last_bar: 2 }))).toBe("Loop 3");
  });

  it("adds the phase and pass while running", () => {
    expect(loopBadgeText(view({ running: true, phase: "count_in", pass: 1 }))).toBe(
      "Loop 5–8 · COUNT-IN · pass 1",
    );
    expect(loopBadgeText(view({ running: true, phase: "demo", pass: 2 }))).toBe(
      "Loop 5–8 · DEMO · pass 2",
    );
    expect(
      loopBadgeText(
        view({
          running: true,
          phase: "your_turn",
          pass: 3,
          last_pass: { pass: 2, hits: 5, misses: 1, accuracy_bp: 8333 },
        }),
      ),
    ).toBe("Loop 5–8 · YOUR TURN · pass 3 · last 83%");
  });
});

describe("loop band", () => {
  it("projects µs onto the engine's ms", () => {
    expect(loopBand(view())).toEqual({ startMs: 8000, endMs: 16000, running: false });
    expect(loopBand(null)).toBeNull();
  });

  it("dims notes outside a running loop only", () => {
    const marked = loopBand(view());
    expect(dimmedByLoop(1000, marked)).toBe(false);
    const running = loopBand(view({ running: true, phase: "demo", pass: 1 }));
    expect(dimmedByLoop(7999, running)).toBe(true);
    expect(dimmedByLoop(8000, running)).toBe(false);
    expect(dimmedByLoop(15999, running)).toBe(false);
    expect(dimmedByLoop(16000, running)).toBe(true);
    expect(dimmedByLoop(1000, null)).toBe(false);
  });
});
