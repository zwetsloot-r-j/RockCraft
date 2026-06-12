// EditCanvas.ts — the composer edit grid renderer.
//
// A horizontal piano-roll on a single <canvas>, drawn *entirely* from the latest
// `ComposerSnapshot` (no edit state of its own). One `draw(snapshot, playheadUs)`
// call paints a frame; the screen calls it on every `snapshot` event and, while
// playing, on each RAF with an interpolated playhead.
//
// Visual language matches the highway screen (Spectrum palette helpers from
// screens/highway/utils.ts, `#0f1016` bg, IBM Plex Mono for numbers), but the
// axes are flipped: time → right, pitch → up — the `edit.rs` (step × pitch)
// model rather than the falling highway.

import type { ComposerSnapshot } from "../../ipc/types";
import {
  isBlack,
  noteName,
  pitchClass,
  roundRect,
  spectrumHue,
  withAlpha,
} from "../highway/utils";
import { gridTiming, Viewport } from "./viewport";

const BG = "#0f1016";
const FONT_MONO = "'IBM Plex Mono', ui-monospace, monospace";

/** µs spanned across the canvas width (~8 bars at 120 4/4): the horizontal zoom. */
const DEFAULT_SPAN_US = 16_000_000;

export class EditCanvas {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private ro: ResizeObserver;

  // CSS-pixel size of the drawing region (DPR handled in resize()).
  private w = 1;
  private h = 1;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(canvas);
    this.resize();
  }

  /** Detach the resize observer. Call from `onCleanup`. */
  dispose(): void {
    this.ro.disconnect();
  }

  private resize(): void {
    const r = this.canvas.getBoundingClientRect();
    this.w = Math.max(1, Math.round(r.width));
    this.h = Math.max(1, Math.round(r.height));
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.canvas.width = this.w * dpr;
    this.canvas.height = this.h * dpr;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  /**
   * Draw one frame from `snapshot`. `playheadUs` overrides the snapshot's own
   * `playhead_us` so the screen can pass a wall-clock-interpolated value between
   * real events for a smooth sweep; it snaps back to `snapshot.playhead_us` on
   * each event.
   */
  draw(snapshot: ComposerSnapshot, playheadUs: number): void {
    const ctx = this.ctx;
    ctx.clearRect(0, 0, this.w, this.h);
    ctx.fillStyle = BG;
    ctx.fillRect(0, 0, this.w, this.h);

    const g = gridTiming(
      snapshot.bpm,
      snapshot.time_sig,
      snapshot.subdivision,
    );
    // Anchor the scroll to the playhead while playing, else the cursor step.
    const cursorUs = snapshot.cursor.step * g.stepUs;
    const anchorUs = snapshot.playing ? playheadUs : cursorUs;
    const vp = new Viewport({
      width: this.w,
      height: this.h,
      spanUs: DEFAULT_SPAN_US,
      anchorUs,
      anchorPitch: snapshot.cursor.pitch,
    });

    this.drawLanes(vp);
    this.drawLoopRegion(snapshot, vp);
    this.drawGridlines(g, vp);
    this.drawNotes(snapshot, vp);
    this.drawSelection(snapshot, vp);
    this.drawChordPreview(snapshot, vp, cursorUs, g.stepUs);
    this.drawCursor(snapshot, vp, cursorUs, g.stepUs);
    this.drawPlayhead(snapshot, vp, playheadUs);
    this.drawLaneLabels(vp);
  }

  // ── horizontal pitch lanes ──────────────────────────────────────────────

  private drawLanes(vp: Viewport): void {
    const ctx = this.ctx;
    for (let p = vp.pitchLo; p <= vp.pitchHi; p++) {
      const y = vp.yOf(p);
      // Black-key lanes tinted darker than white-key lanes.
      ctx.fillStyle = isBlack(p) ? "rgba(0,0,0,0.28)" : "rgba(255,255,255,0.012)";
      ctx.fillRect(0, y, this.w, vp.laneH);
      // Lane separator.
      ctx.strokeStyle = "rgba(255,255,255,0.04)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, y + 0.5);
      ctx.lineTo(this.w, y + 0.5);
      ctx.stroke();
    }
  }

  /** C-lane labels (C3…) down the left gutter, on top of notes. */
  private drawLaneLabels(vp: Viewport): void {
    const ctx = this.ctx;
    ctx.save();
    ctx.font = `500 10px ${FONT_MONO}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    for (let p = vp.pitchLo; p <= vp.pitchHi; p++) {
      if (pitchClass(p) !== 0) continue; // C lanes only
      const y = vp.yOf(p) + vp.laneH / 2;
      ctx.fillStyle = "rgba(255,255,255,0.06)";
      ctx.fillRect(0, vp.yOf(p), this.w, vp.laneH); // faint C-row stripe
      ctx.fillStyle = "rgba(255,255,255,0.4)";
      ctx.fillText(noteName(p), 4, y);
    }
    ctx.restore();
  }

  // ── vertical gridlines (per step / beat / bar) ──────────────────────────

  private drawGridlines(
    g: { stepUs: number; beatUs: number; barUs: number },
    vp: Viewport,
  ): void {
    const ctx = this.ctx;
    const endUs = vp.originUs + this.w / vp.pxPerUs;
    // Start at the first step at/after the left edge.
    const first = Math.floor(vp.originUs / g.stepUs) * g.stepUs;
    for (let t = first; t <= endUs; t += g.stepUs) {
      const x = vp.xOf(t);
      if (x < 0 || x > this.w) continue;
      const onBar = t % g.barUs === 0;
      const onBeat = t % g.beatUs === 0;
      // Heaviest per bar, heavier per beat, faint per subdivision step.
      ctx.strokeStyle = onBar
        ? "rgba(255,255,255,0.16)"
        : onBeat
          ? "rgba(255,255,255,0.08)"
          : "rgba(255,255,255,0.035)";
      ctx.lineWidth = onBar ? 1.4 : 1;
      ctx.beginPath();
      ctx.moveTo(x + 0.5, 0);
      ctx.lineTo(x + 0.5, this.h);
      ctx.stroke();
    }
  }

  // ── notes ───────────────────────────────────────────────────────────────

  private drawNotes(snapshot: ComposerSnapshot, vp: Viewport): void {
    const ctx = this.ctx;
    for (const n of snapshot.notes) {
      if (!vp.pitchVisible(n.pitch)) continue;
      const x = vp.xOf(n.start_us);
      const w = Math.max(2, vp.wOf(n.dur_us));
      if (x + w < 0 || x > this.w) continue;
      const y = vp.yOf(n.pitch);
      const pad = Math.min(2, vp.laneH * 0.15);
      const hue = spectrumHue(n.pitch);
      // Velocity → alpha (0..127 mapped onto 0.4..1.0).
      const alpha = 0.4 + (n.velocity / 127) * 0.6;
      const fill = `oklch(0.72 0.16 ${hue})`;
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.fillStyle = fill;
      roundRect(ctx, x, y + pad, w, vp.laneH - pad * 2, 3);
      ctx.fill();
      // Bright left edge (the onset).
      ctx.globalAlpha = Math.min(1, alpha + 0.15);
      ctx.strokeStyle = withAlpha("#ffffff", 0.5);
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x + 0.75, y + pad);
      ctx.lineTo(x + 0.75, y + vp.laneH - pad);
      ctx.stroke();
      ctx.restore();
    }
  }

  // ── selection rectangle ─────────────────────────────────────────────────

  private drawSelection(snapshot: ComposerSnapshot, vp: Viewport): void {
    const sel = snapshot.selection;
    if (!sel) return;
    const ctx = this.ctx;
    const x = vp.xOf(sel.us_lo);
    const w = Math.max(2, vp.wOf(sel.us_hi - sel.us_lo));
    // pitch_hi is the top lane; include its full height.
    const yTop = vp.yOf(sel.pitch_hi);
    const yBot = vp.yOf(sel.pitch_lo) + vp.laneH;
    ctx.save();
    ctx.fillStyle = "rgba(126,224,138,0.14)";
    ctx.fillRect(x, yTop, w, yBot - yTop);
    ctx.strokeStyle = "rgba(126,224,138,0.55)";
    ctx.lineWidth = 1;
    ctx.strokeRect(x + 0.5, yTop + 0.5, w, yBot - yTop);
    ctx.restore();
  }

  // ── chord preview (ghost notes on the cursor step) ──────────────────────

  private drawChordPreview(
    snapshot: ComposerSnapshot,
    vp: Viewport,
    cursorUs: number,
    stepUs: number,
  ): void {
    const pitches = snapshot.chord_preview;
    if (!pitches) return;
    const ctx = this.ctx;
    const x = vp.xOf(cursorUs);
    const w = Math.max(2, vp.wOf(stepUs));
    const pad = Math.min(2, vp.laneH * 0.15);
    ctx.save();
    ctx.setLineDash([4, 3]);
    for (const p of pitches) {
      if (!vp.pitchVisible(p)) continue;
      const y = vp.yOf(p);
      const hue = spectrumHue(p);
      ctx.globalAlpha = 0.5;
      ctx.fillStyle = `oklch(0.72 0.16 ${hue})`;
      roundRect(ctx, x, y + pad, w, vp.laneH - pad * 2, 3);
      ctx.fill();
      ctx.globalAlpha = 0.9;
      ctx.strokeStyle = `oklch(0.85 0.16 ${hue})`;
      ctx.lineWidth = 1;
      roundRect(ctx, x + 0.5, y + pad + 0.5, w - 1, vp.laneH - pad * 2 - 1, 3);
      ctx.stroke();
    }
    ctx.restore();
  }

  // ── cursor cell ─────────────────────────────────────────────────────────

  private drawCursor(
    snapshot: ComposerSnapshot,
    vp: Viewport,
    cursorUs: number,
    stepUs: number,
  ): void {
    const { pitch } = snapshot.cursor;
    if (!vp.pitchVisible(pitch)) return;
    const ctx = this.ctx;
    const x = vp.xOf(cursorUs);
    const w = Math.max(3, vp.wOf(stepUs));
    const y = vp.yOf(pitch);
    ctx.save();
    ctx.strokeStyle = "#d96bff"; // magenta, matching the TUI cursor
    ctx.lineWidth = 2;
    ctx.strokeRect(x + 1, y + 1, w - 2, vp.laneH - 2);
    ctx.fillStyle = "rgba(217,107,255,0.12)";
    ctx.fillRect(x + 1, y + 1, w - 2, vp.laneH - 2);
    ctx.restore();
  }

  // ── playhead + loop region ──────────────────────────────────────────────

  private drawPlayhead(
    snapshot: ComposerSnapshot,
    vp: Viewport,
    playheadUs: number,
  ): void {
    if (!snapshot.playing) return;
    const ctx = this.ctx;
    const x = vp.xOf(playheadUs);
    if (x < 0 || x > this.w) return;
    ctx.save();
    ctx.strokeStyle = "#5be7c4";
    ctx.lineWidth = 2;
    ctx.shadowColor = "#5be7c4";
    ctx.shadowBlur = 8;
    ctx.beginPath();
    ctx.moveTo(x + 0.5, 0);
    ctx.lineTo(x + 0.5, this.h);
    ctx.stroke();
    ctx.restore();
  }

  private drawLoopRegion(snapshot: ComposerSnapshot, vp: Viewport): void {
    if (!snapshot.looping) return;
    const ctx = this.ctx;
    const x0 = vp.xOf(snapshot.loop_start_us);
    const x1 = vp.xOf(snapshot.loop_end_us);
    ctx.save();
    ctx.fillStyle = "rgba(91,231,196,0.06)";
    ctx.fillRect(x0, 0, x1 - x0, this.h);
    ctx.strokeStyle = "rgba(91,231,196,0.3)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(x0 + 0.5, 0);
    ctx.lineTo(x0 + 0.5, this.h);
    ctx.moveTo(x1 + 0.5, 0);
    ctx.lineTo(x1 + 0.5, this.h);
    ctx.stroke();
    ctx.restore();
  }
}
