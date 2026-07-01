// EditCanvas.ts — the composer edit grid renderer.
//
// A *vertical*, highway-aligned piano-roll on a single <canvas>, drawn *entirely*
// from the latest `ComposerSnapshot` (no edit state of its own). One
// `draw(snapshot, playheadUs)` call paints a frame; the screen calls it on every
// `snapshot` event and, while playing, on each RAF with an interpolated playhead.
//
// Visual language matches the highway/record screens (Spectrum palette helpers
// from screens/highway/utils.ts, `#0f1016` bg, IBM Plex Mono for numbers) and
// shares their orientation: time runs **bottom → top** (song start at the
// bottom, later time higher) and pitch runs **left → right, low → high**. A note
// starting later sits higher; the playhead sweeps upward.
//
// The background fill is kept as its own step so the M7-tauri-N video backdrop
// can later draw a frame *behind* the grid (make the fill translucent / skip it).

import type { ComposerSnapshot } from "../../ipc/types";
import {
  isBlack,
  keyNoteStyle,
  noteName,
  pitchClass,
  roundRect,
  shade,
  spectrumHue,
  withAlpha,
} from "../highway/utils";
import { gridTiming, Viewport } from "./viewport";

const BG = "#0f1016";
/** Translucent dim used over a video backdrop (lets the frame read through). */
const BG_BACKDROP = "rgba(15,16,22,0.45)";
const FONT_MONO = "'IBM Plex Mono', ui-monospace, monospace";

/** µs spanned across the canvas height (~8 bars at 120 4/4): the vertical zoom. */
const DEFAULT_SPAN_US = 16_000_000;

export class EditCanvas {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private ro: ResizeObserver;

  // CSS-pixel size of the drawing region (DPR handled in resize()).
  private w = 1;
  private h = 1;

  // When a video backdrop is attached the grid must show the frame underneath:
  // the opaque background fill is replaced by a translucent dim so the <video>
  // behind the canvas reads through while keeping notes/grid legible.
  private backdrop = false;

  // M10-C split editor: the derived segments (with keep/discard flags). When set,
  // `draw` paints a marker line at each interior boundary and dims discarded
  // segments, so the user can see where they are cutting over the video backdrop.
  // `null` (the default) means the split editor is closed — nothing is drawn.
  private splits: { start_us: number; end_us: number; keep: boolean }[] | null =
    null;

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

  /**
   * Toggle backdrop mode. When `on`, `draw` dims the grid with a translucent
   * fill instead of the opaque `BG`, so the `<video>` behind the canvas shows
   * through. The next `draw` call picks it up.
   */
  setBackdrop(on: boolean): void {
    this.backdrop = on;
  }

  /**
   * Set (or clear) the split-editor segments to overlay (M10-C). Pass the
   * derived segments with their keep/discard flags to draw marker lines at the
   * interior boundaries and dim the discarded ranges; pass `null` to draw
   * nothing (editor closed). The next `draw` call picks it up.
   */
  setSplits(
    segs: { start_us: number; end_us: number; keep: boolean }[] | null,
  ): void {
    this.splits = segs;
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
    // With a backdrop attached, leave the fill translucent so the <video>
    // underneath reads through (clearRect already made it transparent); the dim
    // keeps notes/grid legible on top. Otherwise paint the opaque app bg.
    ctx.fillStyle = this.backdrop ? BG_BACKDROP : BG;
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
    });

    this.drawLanes(vp);
    this.drawLoopRegion(snapshot, vp);
    this.drawGridlines(g, vp);
    // Crosshair guides sit under the notes so a note on the cursor's column /
    // row stays fully legible, but over the gridlines so the selected timeslot
    // reads at a glance even on a sparse grid.
    this.drawCrosshair(snapshot, vp, cursorUs, g.stepUs);
    this.drawNotes(snapshot, vp);
    this.drawSelection(snapshot, vp);
    this.drawSplits(vp);
    this.drawChordPreview(snapshot, vp, cursorUs, g.stepUs);
    this.drawCursor(snapshot, vp, cursorUs, g.stepUs);
    this.drawPlayhead(snapshot, vp, playheadUs);
    this.drawLaneLabels(vp);
  }

  // ── vertical pitch lanes ────────────────────────────────────────────────

  private drawLanes(vp: Viewport): void {
    const ctx = this.ctx;
    for (let p = vp.pitchLo; p <= vp.pitchHi; p++) {
      const x = vp.xOf(p);
      // Black-key lanes tinted darker than white-key lanes.
      ctx.fillStyle = isBlack(p) ? "rgba(0,0,0,0.28)" : "rgba(255,255,255,0.012)";
      ctx.fillRect(x, 0, vp.laneW, this.h);
      // Lane separator.
      ctx.strokeStyle = "rgba(255,255,255,0.04)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x + 0.5, 0);
      ctx.lineTo(x + 0.5, this.h);
      ctx.stroke();
    }
  }

  /** C-lane labels (C3…) along the bottom gutter, on top of notes. */
  private drawLaneLabels(vp: Viewport): void {
    const ctx = this.ctx;
    ctx.save();
    ctx.font = `500 10px ${FONT_MONO}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "bottom";
    for (let p = vp.pitchLo; p <= vp.pitchHi; p++) {
      if (pitchClass(p) !== 0) continue; // C lanes only
      const x = vp.xOf(p);
      ctx.fillStyle = "rgba(255,255,255,0.06)";
      ctx.fillRect(x, 0, vp.laneW, this.h); // faint C-column stripe
      ctx.fillStyle = "rgba(255,255,255,0.4)";
      ctx.fillText(noteName(p), x + vp.laneW / 2, this.h - 3);
    }
    ctx.restore();
  }

  // ── horizontal gridlines (per step / beat / bar) ────────────────────────

  private drawGridlines(
    g: { stepUs: number; beatUs: number; barUs: number },
    vp: Viewport,
  ): void {
    const ctx = this.ctx;
    // The top edge is the latest visible time (originUs is the bottom edge).
    const endUs = vp.originUs + this.h / vp.pxPerUs;
    // Start at the first step at/after the bottom (earliest) edge.
    const first = Math.floor(vp.originUs / g.stepUs) * g.stepUs;
    for (let t = first; t <= endUs; t += g.stepUs) {
      const y = vp.yOf(t);
      if (y < 0 || y > this.h) continue;
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
      ctx.moveTo(0, y + 0.5);
      ctx.lineTo(this.w, y + 0.5);
      ctx.stroke();
    }
  }

  // ── notes ───────────────────────────────────────────────────────────────

  private drawNotes(snapshot: ComposerSnapshot, vp: Viewport): void {
    const ctx = this.ctx;
    for (const n of snapshot.notes) {
      if (!vp.pitchVisible(n.pitch)) continue;
      const x = vp.xOf(n.pitch);
      // The onset is the lower edge; the note grows upward by its duration.
      const y = vp.yOf(n.start_us + n.dur_us);
      const h = Math.max(2, vp.hOf(n.dur_us));
      if (y + h < 0 || y > this.h) continue;
      // Per-note key treatment (slim/darker/outline for accidentals). Single
      // source of truth shared with the highway; see utils.ts::keyNoteStyle. The
      // edit view omits the highway's diagonal rear cutoff (a scroll-motion cue).
      const ksty = keyNoteStyle(n.pitch);
      // `inset` trims an extra fraction of the lane width per side on top of the
      // base pad, so accidentals read as a thinner pill.
      const pad = Math.min(2, vp.laneW * 0.15) + (ksty.inset * vp.laneW) / 2;
      const w = vp.laneW - pad * 2;
      const hue = spectrumHue(n.pitch);
      // Velocity → alpha (0..127 mapped onto 0.4..1.0).
      const alpha = 0.4 + (n.velocity / 127) * 0.6;
      const base = `oklch(0.72 0.16 ${hue})`;
      // Darken accidentals the same way the highway does (helper's shadeMul).
      const fill = ksty.shadeMul ? shade(base, ksty.shadeMul) : base;
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.fillStyle = fill;
      roundRect(ctx, x + pad, y, w, h, 3);
      ctx.fill();
      // Redundant outline so accidentals read even when fills are similar;
      // naturals get none.
      if (ksty.stroke) {
        ctx.globalAlpha = Math.min(1, alpha + 0.2);
        ctx.strokeStyle = ksty.stroke;
        ctx.lineWidth = 1;
        roundRect(ctx, x + pad + 0.5, y + 0.5, w - 1, h - 1, 3);
        ctx.stroke();
      }
      // Bright bottom edge (the onset).
      ctx.globalAlpha = Math.min(1, alpha + 0.15);
      ctx.strokeStyle = withAlpha("#ffffff", 0.5);
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x + pad, y + h - 0.75);
      ctx.lineTo(x + pad + w, y + h - 0.75);
      ctx.stroke();
      ctx.restore();
    }
  }

  // ── selection rectangle ─────────────────────────────────────────────────

  private drawSelection(snapshot: ComposerSnapshot, vp: Viewport): void {
    const sel = snapshot.selection;
    if (!sel) return;
    const ctx = this.ctx;
    // pitch_lo is the leftmost lane; include pitch_hi's full lane width.
    const xLeft = vp.xOf(sel.pitch_lo);
    const xRight = vp.xOf(sel.pitch_hi) + vp.laneW;
    // us_hi is later (higher); us_lo is earlier (lower).
    const yTop = vp.yOf(sel.us_hi);
    const yBot = vp.yOf(sel.us_lo);
    ctx.save();
    ctx.fillStyle = "rgba(126,224,138,0.14)";
    ctx.fillRect(xLeft, yTop, xRight - xLeft, yBot - yTop);
    ctx.strokeStyle = "rgba(126,224,138,0.55)";
    ctx.lineWidth = 1;
    ctx.strokeRect(xLeft + 0.5, yTop + 0.5, xRight - xLeft, yBot - yTop);
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
    const y = vp.yOf(cursorUs + stepUs);
    const h = Math.max(2, vp.hOf(stepUs));
    const pad = Math.min(2, vp.laneW * 0.15);
    ctx.save();
    ctx.setLineDash([4, 3]);
    for (const p of pitches) {
      if (!vp.pitchVisible(p)) continue;
      const x = vp.xOf(p);
      const hue = spectrumHue(p);
      ctx.globalAlpha = 0.5;
      ctx.fillStyle = `oklch(0.72 0.16 ${hue})`;
      roundRect(ctx, x + pad, y, vp.laneW - pad * 2, h, 3);
      ctx.fill();
      ctx.globalAlpha = 0.9;
      ctx.strokeStyle = `oklch(0.85 0.16 ${hue})`;
      ctx.lineWidth = 1;
      roundRect(ctx, x + pad + 0.5, y + 0.5, vp.laneW - pad * 2 - 1, h - 1, 3);
      ctx.stroke();
    }
    ctx.restore();
  }

  // ── cursor crosshair + cell ─────────────────────────────────────────────

  /**
   * Light crosshair guides through the cursor: the cursor's pitch column
   * (full canvas height) and step row (full canvas width) are tinted magenta so
   * the selected timeslot is locatable at a glance even on a sparse grid. Drawn
   * faintly *under* the notes; the bright cursor cell ({@link drawCursor}) sits
   * on top.
   */
  private drawCrosshair(
    snapshot: ComposerSnapshot,
    vp: Viewport,
    cursorUs: number,
    stepUs: number,
  ): void {
    const { pitch } = snapshot.cursor;
    const ctx = this.ctx;
    ctx.save();
    // Pitch column guide (full height), only when the lane is on-screen.
    if (vp.pitchVisible(pitch)) {
      const x = vp.xOf(pitch);
      ctx.fillStyle = "rgba(217,107,255,0.08)";
      ctx.fillRect(x, 0, vp.laneW, this.h);
    }
    // Step row guide (full width). The cursor is kept in view by the viewport,
    // so the row is normally on-screen; clamp-skip if it ever isn't.
    const yTop = vp.yOf(cursorUs + stepUs);
    const h = Math.max(3, vp.hOf(stepUs));
    if (yTop + h >= 0 && yTop <= this.h) {
      ctx.fillStyle = "rgba(217,107,255,0.08)";
      ctx.fillRect(0, yTop, this.w, h);
    }
    ctx.restore();
  }

  private drawCursor(
    snapshot: ComposerSnapshot,
    vp: Viewport,
    cursorUs: number,
    stepUs: number,
  ): void {
    const { pitch } = snapshot.cursor;
    if (!vp.pitchVisible(pitch)) return;
    const ctx = this.ctx;
    const x = vp.xOf(pitch);
    const y = vp.yOf(cursorUs + stepUs);
    const h = Math.max(3, vp.hOf(stepUs));
    ctx.save();
    // Unmistakable filled cell with a bright outline, distinct from notes
    // (rounded, hue-tinted) and the playhead (a thin green line).
    ctx.fillStyle = "rgba(217,107,255,0.28)";
    ctx.fillRect(x + 1, y + 1, vp.laneW - 2, h - 2);
    ctx.strokeStyle = "#d96bff"; // magenta, matching the TUI cursor
    ctx.lineWidth = 2;
    ctx.shadowColor = "#d96bff";
    ctx.shadowBlur = 6;
    ctx.strokeRect(x + 1, y + 1, vp.laneW - 2, h - 2);
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
    const y = vp.yOf(playheadUs);
    if (y < 0 || y > this.h) return;
    ctx.save();
    ctx.strokeStyle = "#5be7c4";
    ctx.lineWidth = 2;
    ctx.shadowColor = "#5be7c4";
    ctx.shadowBlur = 8;
    ctx.beginPath();
    ctx.moveTo(0, y + 0.5);
    ctx.lineTo(this.w, y + 0.5);
    ctx.stroke();
    ctx.restore();
  }

  private drawLoopRegion(snapshot: ComposerSnapshot, vp: Viewport): void {
    if (!snapshot.looping) return;
    if (snapshot.loop_end_us <= snapshot.loop_start_us) return;
    const ctx = this.ctx;
    // y0 is the earlier (lower / loop-in) bound, y1 the later (higher /
    // loop-out) bound. The band is bright enough to read as a deliberate region
    // (not just a tint) so the `{`/`}` loop-in/out keys have a visible target.
    const y0 = vp.yOf(snapshot.loop_start_us);
    const y1 = vp.yOf(snapshot.loop_end_us);
    ctx.save();
    ctx.fillStyle = "rgba(91,231,196,0.10)";
    ctx.fillRect(0, y1, this.w, y0 - y1);
    // Solid bracket lines at both bounds.
    ctx.strokeStyle = "rgba(91,231,196,0.6)";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(0, y0 + 0.5);
    ctx.lineTo(this.w, y0 + 0.5);
    ctx.moveTo(0, y1 + 0.5);
    ctx.lineTo(this.w, y1 + 0.5);
    ctx.stroke();
    // Edge labels so the region is unmistakably the loop, and which edge is
    // which (the in is the lower line, the out the higher one).
    ctx.font = `600 10px ${FONT_MONO}`;
    ctx.textAlign = "left";
    ctx.fillStyle = "rgba(91,231,196,0.85)";
    if (y0 >= 0 && y0 <= this.h) {
      ctx.textBaseline = "bottom";
      ctx.fillText("LOOP IN", 6, y0 - 2);
    }
    if (y1 >= 0 && y1 <= this.h) {
      ctx.textBaseline = "top";
      ctx.fillText("LOOP OUT", 6, y1 + 2);
    }
    ctx.restore();
  }

  // ── split markers + discarded shading (M10-C) ────────────────────────────

  /**
   * Overlay the split editor: a translucent red wash over every discarded
   * segment (so trimmed-away ranges read as "gone") and a bright amber marker
   * line at each interior boundary, labelled with the part number. Time runs
   * bottom→top here, so a "split along the timeline" is a horizontal line across
   * the width. Drawn over the notes but under the cursor/playhead.
   */
  private drawSplits(vp: Viewport): void {
    if (!this.splits) return;
    const ctx = this.ctx;
    ctx.save();

    // Dim discarded segments.
    for (const seg of this.splits) {
      if (seg.keep) continue;
      const yTop = vp.yOf(seg.end_us);
      const yBot = vp.yOf(seg.start_us);
      const y = Math.max(0, Math.min(yTop, yBot));
      const hh = Math.min(this.h, Math.max(yTop, yBot)) - y;
      if (hh <= 0) continue;
      ctx.fillStyle = "rgba(255,90,90,0.16)";
      ctx.fillRect(0, y, this.w, hh);
    }

    // Marker line + part label at every interior boundary (segment starts > 0).
    ctx.font = `700 10px ${FONT_MONO}`;
    ctx.textAlign = "left";
    this.splits.forEach((seg, i) => {
      if (i > 0) {
        const y = vp.yOf(seg.start_us);
        if (y >= 0 && y <= this.h) {
          ctx.strokeStyle = "#f5a742";
          ctx.lineWidth = 2;
          ctx.setLineDash([6, 4]);
          ctx.beginPath();
          ctx.moveTo(0, y + 0.5);
          ctx.lineTo(this.w, y + 0.5);
          ctx.stroke();
          ctx.setLineDash([]);
        }
      }
      // Part-number tag sat just inside the lower edge of the segment.
      const yLabel = vp.yOf(seg.start_us);
      if (yLabel >= 8 && yLabel <= this.h) {
        ctx.fillStyle = seg.keep
          ? "rgba(245,167,66,0.9)"
          : "rgba(255,120,120,0.85)";
        ctx.textBaseline = "bottom";
        ctx.fillText(`part ${i + 1}${seg.keep ? "" : " (cut)"}`, 6, yLabel - 3);
      }
    });

    ctx.restore();
  }
}
