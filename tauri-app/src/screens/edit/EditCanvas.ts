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
  tailGapPx,
  withAlpha,
} from "../highway/utils";
import { gridTiming, Viewport } from "./viewport";

/** Euclidean modulo (non-negative result), for classifying step indices that
 * may be negative when the viewport bottom edge sits before the grid origin. */
function mod(a: number, b: number): number {
  return ((a % b) + b) % b;
}

const BG = "#0f1016";
/** Translucent dim used over a video backdrop (lets the frame read through). */
const BG_BACKDROP = "rgba(15,16,22,0.45)";
/**
 * How far (s) the backdrop element's `currentTime` may sit from the frame we
 * asked for and still count as showing it. Comfortably wider than a frame at
 * any sane rate, and far narrower than the gap to the intro frame a decoding
 * WebView2 substitutes — which is what this distinguishes.
 */
const FRAME_TOL_S = 0.2;
/**
 * How long (ms) the cached frame may be held while the element refuses to reach
 * its target before we give up and show whatever it has.
 *
 * The on-target test suppresses the intro frame WebView2 substitutes mid-decode,
 * but a seek that never lands — the element was not primed, the clip is still
 * loading — would otherwise freeze the backdrop permanently. Showing a slightly
 * wrong frame beats showing a stale one forever.
 */
const MAX_HOLD_MS = 700;
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

  // When a video backdrop is attached the grid must show the frame underneath.
  // Rather than let the <video> element show *through* a translucent canvas —
  // which flickers to black in WebView2 every time we seek its currentTime — we
  // *sample* the element's current frame straight onto this canvas with
  // drawImage (canvas sampling never triggers the element's seek-to-black), then
  // dim it. The <video> stays behind, decoding, fully covered by the opaque
  // canvas. `backdropVideo` is that source element (or null when unset).
  private backdrop = false;
  private backdropVideo: HTMLVideoElement | null = null;

  // Last frame successfully sampled from `backdropVideo`, kept at the video's
  // native resolution so it can be re-stretched to any canvas size. Painted
  // whenever the element has no trustworthy frame of its own — chiefly while a
  // seek is in flight, which is most of the time when scrubbing the cursor.
  // Null until the first frame decodes.
  private lastFrame: HTMLCanvasElement | null = null;

  // Song time (s) the backdrop frame is *supposed* to show, or null to accept
  // whatever the element presents (during playback, where the element runs on
  // its own clock and drifts from the target by design).
  //
  // `seeking` alone is not a usable signal: WebView2 presents the clip's INTRO
  // frame while it decodes a seek, with `seeking` already back to false — so a
  // frame can look live, be wrong, and poison the cache. Comparing currentTime
  // against the target catches that, because the intro frame is seconds away
  // from where we asked to be.
  private backdropWantS: number | null = null;

  // When the cached frame started being held in place of a live-but-off-target
  // one, so the hold can time out (see MAX_HOLD_MS). 0 = not currently holding.
  private holdSinceMs = 0;

  // Live set of MIDI pitches held on the piano, shared by reference from the
  // screen (updated on every `midi_event`). The keyboard strip lights these keys
  // next to the wait-mode target key, so a note can be verified against the
  // physical piano at a glance. Empty until `setHeldNotes` is called.
  private heldNotes: ReadonlySet<number> = new Set();

  // M10-C split editor: the derived segments (with keep/discard flags). When set,
  // `draw` paints a marker line at each interior boundary and dims discarded
  // segments, so the user can see where they are cutting over the video backdrop.
  // `null` (the default) means the split editor is closed — nothing is drawn.
  private splits: { start_us: number; end_us: number; keep: boolean }[] | null =
    null;

  // Grid calibration (M-align): vertical zoom (spanUs), hit-line position
  // (hitLineFrac), and horizontal zoom/pan (xScale/xOffset) so the edit grid can
  // be resized to register on a backdrop movie (and zoom in generally). Defaults
  // reproduce the un-calibrated grid exactly.
  private gridCal = {
    spanUs: DEFAULT_SPAN_US,
    hitLineFrac: 1 / 3,
    xScale: 1,
    xOffset: 0,
  };

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
   * Register the `<video>` element whose current frame is composited onto the
   * canvas while a backdrop is attached. Sampling the frame with drawImage —
   * instead of showing the element through a translucent canvas — sidesteps the
   * WebView2 quirk where every `currentTime` seek blacks the element out (the
   * "backdrop flickers between visible and hidden" bug). Pass null to clear.
   */
  setBackdropVideo(el: HTMLVideoElement | null): void {
    if (el !== this.backdropVideo) this.lastFrame = null; // stale for a new clip
    this.backdropVideo = el;
  }

  /**
   * Declare the song time (s) the backdrop frame should show, so `draw` can tell
   * a real frame from the intro frame WebView2 substitutes while decoding a
   * seek. Pass null while playing — the element then runs on its own clock and
   * is *expected* to differ from any single target.
   */
  setBackdropWant(seconds: number | null): void {
    this.backdropWantS = seconds;
  }

  /**
   * Copy `vid`'s current frame into the offscreen cache, returning whether it
   * is now safe to paint from the element this tick.
   *
   * `drawImage` from a media element throws if the frame isn't paintable, so a
   * successful copy here is also the proof that drawing the element will
   * succeed. On failure the previous cached frame is left intact — a bad tick
   * must never blank a backdrop we already had.
   */
  private cacheFrame(vid: HTMLVideoElement): boolean {
    const vw = vid.videoWidth;
    const vh = vid.videoHeight;
    if (vw <= 0 || vh <= 0) return false;
    let cache = this.lastFrame;
    if (!cache || cache.width !== vw || cache.height !== vh) {
      cache = document.createElement("canvas");
      cache.width = vw;
      cache.height = vh;
    }
    const cctx = cache.getContext("2d");
    if (!cctx) return false;
    try {
      cctx.drawImage(vid, 0, 0, vw, vh);
    } catch {
      return false; // frame not paintable — keep whatever we cached before
    }
    this.lastFrame = cache;
    return true;
  }

  /**
   * Register the live held-note set for the keyboard strip. Held by reference:
   * the screen mutates the same `Set` on each `midi_event`, and the next `draw`
   * reads it — no per-event setter churn.
   */
  setHeldNotes(held: ReadonlySet<number>): void {
    this.heldNotes = held;
  }

  /**
   * Set the grid calibration (M-align). `spanUs` is the vertical zoom (µs across
   * the height — smaller = zoomed in), `hitLineFrac` the now-line position,
   * `xScale`/`xOffset` the horizontal zoom/pan of the keyboard. The next `draw`
   * picks it up.
   */
  setGridCal(cal: {
    spanUs: number;
    hitLineFrac: number;
    xScale: number;
    xOffset: number;
  }): void {
    this.gridCal = cal;
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
  draw(
    snapshot: ComposerSnapshot,
    playheadUs: number,
    scrollAnchorUs?: number,
  ): void {
    const ctx = this.ctx;
    ctx.clearRect(0, 0, this.w, this.h);
    // With a backdrop attached, sample the video's current frame straight onto
    // this canvas (opaque) and dim it, rather than showing the <video> element
    // through a translucent canvas — the latter flickers to black on every
    // WebView2 seek. Stretch the frame to the canvas box (matches the element's
    // object-fit:fill; the *grid* is then calibrated onto it). If the frame
    // isn't decodable yet, fall back to the opaque app bg so the covered
    // element never peeks through. Otherwise (no backdrop) paint the app bg.
    const vid = this.backdropVideo;
    if (this.backdrop && vid) {
      // A frame is only trustworthy when the element is decoded AND not mid-seek:
      // while `seeking` is true WebView2 may present an unrelated frame (or drop
      // readyState and present none), which is what made every cursor move flash
      // a wrong backdrop for the length of the seek. So cache each good frame and
      // re-show the cached one for the duration of the seek — the backdrop then
      // holds steady on the last real frame and cuts straight to the new one when
      // the seek lands.
      const want = this.backdropWantS;
      const onTarget =
        want === null || Math.abs(vid.currentTime - want) <= FRAME_TOL_S;
      // Hold the cache while the element is off-target — but only for so long,
      // so a seek that never lands can't freeze the backdrop for good.
      const now = performance.now();
      if (onTarget) {
        this.holdSinceMs = 0;
      } else if (this.holdSinceMs === 0) {
        this.holdSinceMs = now;
      }
      const heldTooLong =
        this.holdSinceMs !== 0 && now - this.holdSinceMs >= MAX_HOLD_MS;
      const decoded = vid.readyState >= 2 && !vid.seeking;
      const live = decoded && (onTarget || heldTooLong || !this.lastFrame);
      if (live && this.cacheFrame(vid)) {
        ctx.drawImage(vid, 0, 0, this.w, this.h);
        ctx.fillStyle = BG_BACKDROP; // dim for note/grid legibility
      } else if (this.lastFrame) {
        ctx.drawImage(this.lastFrame, 0, 0, this.w, this.h);
        ctx.fillStyle = BG_BACKDROP;
      } else {
        ctx.fillStyle = BG; // nothing decoded yet — opaque bg
      }
    } else {
      ctx.fillStyle = BG;
    }
    ctx.fillRect(0, 0, this.w, this.h);

    const g = gridTiming(
      snapshot.bpm,
      snapshot.time_sig,
      snapshot.subdivision,
    );
    // Anchor the scroll to an explicit review head when given, else to the
    // playhead while playing, else the cursor step. The cursor's µs position is
    // phased by the grid origin (step 0 sits at the origin, not song time 0).
    const gridOriginUs = snapshot.grid_origin_us ?? 0;
    const cursorUs = gridOriginUs + snapshot.cursor.step * g.stepUs;
    const reviewing = scrollAnchorUs !== undefined;
    const anchorUs = reviewing
      ? scrollAnchorUs
      : snapshot.playing
        ? playheadUs
        : cursorUs;
    const vp = new Viewport({
      width: this.w,
      height: this.h,
      spanUs: this.gridCal.spanUs,
      anchorUs,
      hitLineFrac: this.gridCal.hitLineFrac,
      xScale: this.gridCal.xScale,
      xOffset: this.gridCal.xOffset,
      // Keep the earliest notes above the keyboard strip (+margin) so notes
      // placed near the start aren't hidden behind it.
      bottomReservePx: this.keyboardBandH() + 12,
    });

    this.drawLanes(vp);
    this.drawLoopRegion(snapshot, vp);
    this.drawGridlines(g, vp, gridOriginUs);
    // Crosshair guides sit under the notes so a note on the cursor's column /
    // row stays fully legible, but over the gridlines so the selected timeslot
    // reads at a glance even on a sparse grid.
    this.drawCrosshair(snapshot, vp, cursorUs, g.stepUs);
    this.drawNotes(snapshot, vp);
    this.drawSelection(snapshot, vp);
    this.drawSplits(vp);
    this.drawChordPreview(snapshot, vp, cursorUs, g.stepUs);
    this.drawCursor(snapshot, vp, cursorUs, g.stepUs);
    this.drawPlayhead(snapshot, vp, reviewing ? scrollAnchorUs : playheadUs, reviewing);
    this.drawLaneLabels(vp);
    // The piano keyboard strip sits on top of everything at the bottom edge,
    // aligned 1:1 with the pitch lanes so a falling note descends into its key.
    this.drawKeyboard(snapshot, vp);
  }

  // ── keyboard strip (note-by-note verification) ──────────────────────────

  /**
   * A piano-key strip along the bottom edge, one key per visible pitch lane
   * (aligned with the highway columns, so a note falls into its key). Keys are
   * lit for the wait-mode target (the note the player must strike to advance),
   * for the keys currently held on the piano, and — brightest — when a held key
   * matches the target. Naturals read light, accidentals dark (a shorter cap),
   * so the octave shape is legible; C keys carry a name label.
   */
  /** Height (px) of the bottom keyboard strip — shared by the keyboard draw and
   * the viewport's bottom reserve so the strip never occludes the earliest notes. */
  private keyboardBandH(): number {
    return Math.max(30, Math.min(64, this.h * 0.16));
  }

  private drawKeyboard(snapshot: ComposerSnapshot, vp: Viewport): void {
    const ctx = this.ctx;
    const bandH = this.keyboardBandH();
    const top = this.h - bandH;
    const awaitingArr = snapshot.awaiting ?? [];
    const awaiting = new Set<number>(awaitingArr);
    // A wait step (a single note OR a whole chord) is only "satisfied" once EVERY
    // required note is held together. Until then the target keys stay amber — a
    // held-but-still-pending target keeps its amber (with a green "registered" tick)
    // instead of turning green early, so a chord reads as done only when complete.
    const chordSatisfied =
      awaitingArr.length > 0 && awaitingArr.every((p) => this.heldNotes.has(p));

    ctx.save();
    // Opaque base so the keyboard reads clearly over the note field / backdrop.
    ctx.fillStyle = "rgba(8,9,13,0.92)";
    ctx.fillRect(0, top, this.w, bandH);

    for (let p = vp.pitchLo; p <= vp.pitchHi; p++) {
      const x = vp.xOf(p);
      const w = Math.max(1, vp.laneW - 1);
      const black = isBlack(p);
      const held = this.heldNotes.has(p);
      const target = awaiting.has(p);
      // A target key that is held but whose chord isn't complete yet: stays amber,
      // gets a green tick so you can see it's registered while you find the rest.
      const targetHeldPending = target && held && !chordSatisfied;

      let fill: string;
      let keyH = bandH;
      if (target && chordSatisfied)
        fill = "#7ee08a"; // all required notes held — the step is satisfied
      else if (target)
        fill = "#f5a742"; // play this — a note still needed for the (chord) step
      else if (held)
        fill = "#5b9be7"; // you're playing this (not part of the target)
      else if (black) {
        fill = "#23262e"; // black key — a shorter, darker cap
        keyH = bandH * 0.62;
      } else fill = "#c9cdd6"; // white key

      ctx.fillStyle = fill;
      ctx.fillRect(x + 0.5, top, w, keyH);
      ctx.strokeStyle = "rgba(0,0,0,0.55)";
      ctx.lineWidth = 1;
      ctx.strokeRect(x + 0.5, top + 0.5, w, bandH - 1);

      // Green "registered" tick along the top of a held-but-pending target key.
      if (targetHeldPending) {
        ctx.fillStyle = "#7ee08a";
        ctx.fillRect(x + 0.5, top, w, Math.max(3, bandH * 0.14));
      }

      if (pitchClass(p) === 0 && vp.laneW > 10) {
        ctx.fillStyle = "rgba(0,0,0,0.6)";
        ctx.font = `600 9px ${FONT_MONO}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "bottom";
        ctx.fillText(noteName(p), x + vp.laneW / 2, this.h - 2);
      }
    }

    // Bright top edge so the strip reads as a deliberate keyboard.
    ctx.strokeStyle = "rgba(255,255,255,0.2)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, top + 0.5);
    ctx.lineTo(this.w, top + 0.5);
    ctx.stroke();
    ctx.restore();
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
    gridOriginUs: number,
  ): void {
    const ctx = this.ctx;
    // The top edge is the latest visible time (originUs is the bottom edge).
    const endUs = vp.originUs + this.h / vp.pxPerUs;
    // Grid lines fall at `gridOriginUs + i·stepUs`. Classify bar/beat lines by the
    // INTEGER step index `i` (steps-per-bar / -beat) rather than a float modulo of
    // the time — `stepUs` is fractional (e.g. 60e6/172/4), so a `t % barUs` would
    // accumulate error and miss downbeats. Start at the first index at/after the
    // bottom (earliest) edge.
    const stepsPerBar = Math.max(1, Math.round(g.barUs / g.stepUs));
    const stepsPerBeat = Math.max(1, Math.round(g.beatUs / g.stepUs));
    let i = Math.floor((vp.originUs - gridOriginUs) / g.stepUs);
    for (; ; i++) {
      const t = gridOriginUs + i * g.stepUs;
      if (t > endUs) break;
      const y = vp.yOf(t);
      if (y < 0 || y > this.h) continue;
      const onBar = mod(i, stepsPerBar) === 0;
      const onBeat = mod(i, stepsPerBeat) === 0;
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
      const full = Math.max(2, vp.hOf(n.dur_us));
      // Trim the trailing (later-time) edge — the top — so back-to-back
      // same-pitch notes read as two blocks instead of one bar. The onset edge
      // (y + h) stays put, so a note still starts exactly where its data says.
      const gap = tailGapPx(full);
      const y = vp.yOf(n.start_us + n.dur_us) + gap;
      const h = full - gap;
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
    // No shadowBlur: too costly to repaint every frame; the filled cell + bright
    // outline stay unmistakable without the glow.
    ctx.lineWidth = 2;
    ctx.strokeRect(x + 1, y + 1, vp.laneW - 2, h - 2);
    ctx.restore();
  }

  // ── playhead + loop region ──────────────────────────────────────────────

  private drawPlayhead(
    snapshot: ComposerSnapshot,
    vp: Viewport,
    playheadUs: number,
    force = false,
  ): void {
    if (!snapshot.playing && !force) return;
    const ctx = this.ctx;
    const y = vp.yOf(playheadUs);
    if (y < 0 || y > this.h) return;
    ctx.save();
    // No shadowBlur here: canvas-2D shadows are very expensive per frame and
    // this line is redrawn every RAF during playback. A crisp 2px line reads
    // fine without the glow.
    ctx.strokeStyle = "#5be7c4";
    ctx.lineWidth = 2;
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
