// HighwayCanvas.ts — a configurable falling-note highway + 88-key board, drawn
// on a single <canvas>. One class, driven by config. Faithful to highway.rs
// projection: time t maps to a vertical position where `now` is the keyboard
// hit-line at the bottom and `now + lead` is the top.
//
// Ported from design/note_highway/rockcraft-proto/highway.js. The `window.RC.*`
// globals are now constructor parameters (config + song) and imported helpers.
//
// Seams for the live-wiring follow-up (#168):
//   - The internal clock is read through `elapsed()` only; #168 swaps the
//     `performance.now() - t0` source for an external `core::PlayClock`.
//   - `runScoring` is demo-only; #168 replaces it with real `core::scoring`
//     judgments (perfect 50 ms / good 150 ms) off MIDI timestamps.

import type { HighwayConfig, KeyInfo, KeyLayout, NoteSpan, SongData } from "./types";
import {
  clamp,
  keyLayout,
  keyNoteStyle,
  lerp,
  noteName,
  roundRect,
  shade,
  spectrumHue,
  withAlpha,
} from "./utils";

// Fully-resolved config: DEFAULTS merged with the passed HighwayConfig.
interface FullConfig {
  lead: number;
  bg: string;
  laneTint: string;
  boardInset: number;
  kbRatio: number;
  noteGap: number;
  radius: number;
  colorMode: "hands" | "spectrum" | "accent";
  handColors: { L: string; R: string };
  accent: string;
  perspective: number;
  glow: number;
  labels: boolean;
  gridlines: "none" | "soft";
  keyboard: "realistic" | "flat" | "strip";
  hitLine: string;
  scoring: boolean;
  pitchRuler: boolean;
  fontMono: string;
  fontDisp: string;
}

const DEFAULTS: FullConfig = {
  lead: 2600, // ms of look-ahead shown top→hit-line
  bg: "#0e0f13",
  laneTint: "rgba(255,255,255,0.018)",
  boardInset: 0, // px inset on each side of the keyboard
  kbRatio: 0.18, // keyboard height as fraction of canvas height
  noteGap: 0.16, // fraction of lane width trimmed (white notes)
  radius: 4, // note corner radius
  colorMode: "hands",
  handColors: { L: "#5ad1c7", R: "#e6a14b" },
  accent: "#7aa2ff",
  perspective: 0, // 0 = flat; else top horizontal scale (e.g. 0.5)
  glow: 0, // 0..1 glow strength on notes
  labels: false, // note-name labels on notes
  gridlines: "soft",
  keyboard: "realistic",
  hitLine: "#ffffff",
  scoring: false, // run a simulated player + score HUD
  pitchRuler: false, // left-edge octave ruler
  fontMono: "'IBM Plex Mono', ui-monospace, monospace",
  fontDisp: "'Space Grotesk', system-ui, sans-serif",
};

interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  color: string;
  born: number;
  life: number;
}
interface Flash {
  cx: number;
  w: number;
  color: string;
  born: number;
}
interface Judge {
  text: string;
  color: string;
  born: number;
}

export class HighwayCanvas {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private cfg: FullConfig;
  private song: SongData;

  t0: number;
  private lastNow = 0;

  // ── live wiring (#168) ───────────────────────────────────────────────────
  // When `live` is set the engine stops using its internal `performance.now()`
  // clock and `runScoring` sim; instead the song time, freeze flag, and held
  // notes come from the backend `play_state` event (driven by `core::PlayClock`
  // off MIDI timestamps). The header reads score/combo from `play_state`, not
  // these fields. `liveTimeMs` is interpolated between events with the wall
  // clock so the scroll stays smooth at the render rate without owning timing.
  private live = false;
  private liveTimeMs = 0;
  /** Wall-clock anchor (performance.now) for the last `setLiveState` call. */
  private liveAnchor = 0;
  private liveFrozen = false;
  private liveHeld = new Set<number>();
  private liveAwaiting = new Set<number>();

  /** Enable live mode: the engine reads time/held from the backend, not its
   * own clock or scoring sim. */
  setLive(on: boolean): void {
    this.live = on;
  }

  // ── Background video backdrop (M9-G) ──────────────────────────────────────
  // When a piece carries a background video, the highway draws over it: the
  // opaque background fill is swapped for a translucent dim so the <video>
  // element behind the canvas reads through. Mirrors EditCanvas.setBackdrop.
  private backdrop = false;

  /** Toggle backdrop mode. When `on`, the background fill is translucent so the
   * `<video>` behind the canvas shows through. */
  setBackdrop(on: boolean): void {
    this.backdrop = on;
  }

  /** Push the latest backend `play_state`. `timeMs` is the authoritative song
   * time; render interpolates forward from here using the wall clock unless
   * `frozen`. `held` highlights the player's keys; `awaiting` pulses the notes
   * the player must hold to un-freeze. */
  setLiveState(
    timeMs: number,
    frozen: boolean,
    held: number[],
    awaiting: number[] = [],
  ): void {
    this.liveTimeMs = timeMs;
    this.liveAnchor = performance.now();
    this.liveFrozen = frozen;
    this.liveHeld = new Set(held);
    this.liveAwaiting = new Set(awaiting);
  }

  // scoring sim state (public reads for the header chrome)
  score = 0;
  combo = 0;
  bestCombo = 0;
  hits = 0;
  total = 0;
  private judged = new Set<NoteSpan>();
  private hitNotes = new Set<NoteSpan>();
  private flashes: Flash[] = [];
  private particles: Particle[] = [];
  private judge: Judge | null = null;

  private raf: number | null = null;
  private ro: ResizeObserver;
  private err: string | null = null;

  // layout (set by resize)
  private w = 1;
  private h = 1;
  private boardW = 1;
  private boardX = 0;
  private kl!: KeyLayout;
  private kbH = 0;
  private hitY = 0;
  private centerX = 0;

  constructor(canvas: HTMLCanvasElement, cfg: HighwayConfig, song: SongData) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
    this.cfg = { ...DEFAULTS, ...cfg };
    this.song = song;
    this.t0 = performance.now();
    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(canvas);
    this.resize();
  }

  resize(): void {
    const r = this.canvas.getBoundingClientRect();
    this.w = Math.max(1, Math.round(r.width));
    this.h = Math.max(1, Math.round(r.height));
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.canvas.width = this.w * dpr;
    this.canvas.height = this.h * dpr;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.boardW = this.w - this.cfg.boardInset * 2;
    this.boardX = this.cfg.boardInset;
    this.kl = keyLayout(this.boardW);
    this.kbH = Math.round(this.h * this.cfg.kbRatio);
    this.hitY = this.h - this.kbH;
    this.centerX = this.boardX + this.boardW / 2;
  }

  start(): void {
    const loop = () => {
      try {
        this.frame();
      } catch (e) {
        if (!this.err) {
          this.err = String((e instanceof Error && e.stack) || e);
          console.error("frame error", e);
        }
      }
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  stop(): void {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.ro.disconnect();
  }

  // The one clock accessor. In live mode (#168) this is the backend song time
  // interpolated forward from the last `play_state` with the wall clock (frozen
  // holds it steady); otherwise the demo-only internal clock.
  private elapsed(): number {
    if (this.live) {
      if (this.liveFrozen) return this.liveTimeMs;
      return this.liveTimeMs + (performance.now() - this.liveAnchor);
    }
    return performance.now() - this.t0;
  }

  // Horizontal perspective scale at highway y (0 top .. hitY bottom).
  private sAt(y: number): number {
    if (!this.cfg.perspective) return 1;
    const d = clamp(y / this.hitY, 0, 1); // 0 top, 1 hit-line
    return lerp(this.cfg.perspective, 1, d);
  }
  private xP(x: number, s: number): number {
    return this.centerX + (x - this.centerX) * s;
  }
  // time -> highway y (now at hitY, now+lead at 0)
  private yOf(t: number, now: number): number {
    return this.hitY * (1 - (t - now) / this.cfg.lead);
  }

  private activeTargets(now: number): Map<number, "L" | "R"> {
    const set = new Map<number, "L" | "R">();
    for (const nt of this.song.notes) {
      if (nt.start <= now && now < nt.end) set.set(nt.note, nt.hand);
    }
    return set;
  }

  private noteColor(nt: NoteSpan, light = 0): string {
    const c = this.cfg;
    if (c.colorMode === "spectrum") {
      const h = spectrumHue(nt.note);
      return `oklch(${0.7 + light * 0.12} 0.16 ${h})`;
    }
    if (c.colorMode === "accent") return c.accent;
    return c.handColors[nt.hand] || c.accent;
  }

  private frame(): void {
    const c = this.cfg;
    // Live bundles play once (no loop wrap); the demo mock loops on LOOP.
    const now = this.live ? this.elapsed() : this.elapsed() % this.song.LOOP;
    // The internal scoring sim is demo-only; live scoring is the backend's.
    if (c.scoring && !this.live) this.runScoring(now);

    this.ctx.clearRect(0, 0, this.w, this.h);
    this.drawBackground();
    this.drawGrid(now);
    this.drawNotes(now);
    this.drawHitLine();
    this.drawKeyboard(now);
    if (c.pitchRuler) this.drawRuler();
    if (c.scoring) this.drawScoreFx();
    this.lastNow = now;
  }

  /** Translucent dim used over a video backdrop (lets the frame read through),
   * mirroring EditCanvas. */
  private static readonly BG_BACKDROP = "rgba(15,16,22,0.45)";

  private drawBackground(): void {
    const ctx = this.ctx,
      c = this.cfg;
    // With a backdrop attached, leave the fill translucent so the <video> behind
    // the canvas shows through; otherwise paint the opaque highway background.
    ctx.fillStyle = this.backdrop ? HighwayCanvas.BG_BACKDROP : c.bg;
    ctx.fillRect(0, 0, this.w, this.h);
    // faint lane tint columns (white keys) on the highway
    if (c.laneTint) {
      ctx.fillStyle = c.laneTint;
      for (const k of this.kl.whites) {
        if ((k.wi ?? 0) % 2) continue;
        // taper with perspective by sampling top & bottom
        const sT = this.sAt(0),
          sB = this.sAt(this.hitY);
        ctx.beginPath();
        ctx.moveTo(this.xP(k.x, sT), 0);
        ctx.lineTo(this.xP(k.x + k.w, sT), 0);
        ctx.lineTo(this.xP(k.x + k.w, sB), this.hitY);
        ctx.lineTo(this.xP(k.x, sB), this.hitY);
        ctx.closePath();
        ctx.fill();
      }
    }
    if (c.perspective) {
      // depth fog at the top (vanishing horizon)
      const g = ctx.createLinearGradient(0, 0, 0, this.hitY);
      g.addColorStop(0, c.bg);
      g.addColorStop(0.18, "rgba(0,0,0,0)");
      ctx.fillStyle = g;
      ctx.fillRect(0, 0, this.w, this.hitY);
    }
  }

  private drawGrid(now: number): void {
    const ctx = this.ctx,
      c = this.cfg;
    if (c.gridlines === "none") return;
    const { BEAT, BAR } = this.song;
    // draw beat & bar lines within the visible window
    const firstBeat = Math.floor(now / BEAT) * BEAT;
    for (let t = firstBeat; t <= now + c.lead; t += BEAT) {
      const y = this.yOf(t, now);
      if (y < 0 || y > this.hitY) continue;
      const bar = Math.round(t) % BAR === 0;
      const s = this.sAt(y);
      const half = (this.boardW / 2) * s;
      ctx.beginPath();
      ctx.moveTo(this.centerX - half, y);
      ctx.lineTo(this.centerX + half, y);
      ctx.lineWidth = bar ? 1.4 : 1;
      ctx.strokeStyle = bar ? "rgba(255,255,255,0.13)" : "rgba(255,255,255,0.05)";
      ctx.stroke();
    }
  }

  private drawNotes(now: number): void {
    const c = this.cfg;
    const lead = c.lead;
    // draw across loop copies so the scroll is seamless
    for (const nt of this.song.notes) {
      const lane = this.kl.byNote[nt.note];
      if (!lane) continue;
      for (let k = -1; k <= 2; k++) {
        const st = nt.start + k * this.song.LOOP;
        const en = nt.end + k * this.song.LOOP;
        if (en < now || st > now + lead) continue;
        this.drawNote(nt, lane, st, en, now);
      }
    }
  }

  private drawNote(
    nt: NoteSpan,
    lane: KeyInfo,
    st: number,
    en: number,
    now: number,
  ): void {
    const ctx = this.ctx,
      c = this.cfg;
    const yTop = clamp(this.yOf(en, now), -40, this.hitY);
    let yBot = clamp(this.yOf(st, now), -40, this.hitY);
    if (yBot - yTop < 3) yBot = yTop + 3; // min visible height
    // Per-note key treatment (slim/darker/outline for accidentals). Single
    // source of truth shared with the edit view; see utils.ts::keyNoteStyle.
    const ksty = keyNoteStyle(nt.note);
    const gap = clamp(c.noteGap + ksty.inset, 0, 0.92); // accidentals = slim pill
    const halfW = (lane.w * (1 - gap)) / 2;
    const L = lane.cx - halfW,
      R = lane.cx + halfW;
    const sT = this.sAt(yTop),
      sB = this.sAt(yBot);
    const depth = 1 - clamp(yBot / this.hitY, 0, 1); // 0 far, 1 near hit
    const light = depth * 0.5;
    // Darken accidentals the same way in every colour mode (replaces the old
    // ad-hoc black-key alpha tweak) so the dimmer read is mode-independent.
    const baseCol = this.noteColor(nt, light);
    const col = ksty.shadeMul ? shade(baseCol, ksty.shadeMul) : baseCol;
    // Diagonal rear (top) cutoff: a highway-only motion cue for accidentals when
    // perspective is flat. Not part of the shared keyNoteStyle (the edit view
    // omits it); proportional to height so short notes stay legible.
    const cut = ksty.stroke && !c.perspective ? Math.min(7, (yBot - yTop) * 0.5) : 0;

    ctx.save();
    // glow
    if (c.glow) {
      ctx.shadowColor = col;
      ctx.shadowBlur = (8 + depth * 22) * c.glow;
    }
    ctx.beginPath();
    if (!c.perspective) {
      if (cut > 0) {
        const xL = this.xP(L, 1),
          xR = this.xP(R, 1);
        ctx.moveTo(xL, yTop + cut);
        ctx.lineTo(xR, yTop);
        ctx.lineTo(xR, yBot);
        ctx.lineTo(xL, yBot);
        ctx.closePath();
      } else {
        roundRect(ctx, this.xP(L, 1), yTop, halfW * 2, yBot - yTop, c.radius);
      }
    } else {
      ctx.moveTo(this.xP(L, sT), yTop);
      ctx.lineTo(this.xP(R, sT), yTop);
      ctx.lineTo(this.xP(R, sB), yBot);
      ctx.lineTo(this.xP(L, sB), yBot);
      ctx.closePath();
    }
    // vertical gradient body for depth
    const g = ctx.createLinearGradient(0, yTop, 0, yBot);
    g.addColorStop(0, withAlpha(col, c.glow ? 0.72 : 0.86));
    g.addColorStop(1, col);
    ctx.fillStyle = g;
    ctx.fill();
    // Redundant outline so accidentals read even when fills are similar; the
    // path is still current, so trace it before restore. Naturals get none.
    if (ksty.stroke) {
      ctx.lineWidth = 1.25;
      ctx.strokeStyle = ksty.stroke;
      ctx.stroke();
    }
    ctx.restore();

    // bright leading edge (nearest the hit line)
    ctx.save();
    ctx.beginPath();
    const eL = this.xP(L, sB),
      eR = this.xP(R, sB);
    ctx.moveTo(eL, yBot);
    ctx.lineTo(eR, yBot);
    ctx.lineWidth = 2;
    ctx.strokeStyle = withAlpha("#ffffff", c.glow ? 0.85 : 0.4);
    ctx.stroke();
    ctx.restore();

    // label
    if (c.labels && yBot - yTop > 16) {
      ctx.save();
      ctx.fillStyle = "rgba(10,10,12,0.9)";
      ctx.font = `600 ${Math.min(12, lane.w * 0.7)}px ${c.fontMono}`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(noteName(nt.note).replace(/-?\d+$/, ""), lane.cx, yBot - 9);
      ctx.restore();
    }
  }

  private drawHitLine(): void {
    const ctx = this.ctx,
      c = this.cfg;
    const y = this.hitY;
    const g = ctx.createLinearGradient(0, y - 16, 0, y);
    g.addColorStop(0, "rgba(255,255,255,0)");
    g.addColorStop(1, withAlpha(c.hitLine, 0.1));
    ctx.fillStyle = g;
    ctx.fillRect(0, y - 16, this.w, 16);
    ctx.beginPath();
    ctx.moveTo(0, y + 0.5);
    ctx.lineTo(this.w, y + 0.5);
    ctx.lineWidth = 2;
    ctx.strokeStyle = withAlpha(c.hitLine, 0.5);
    ctx.stroke();
  }

  private drawKeyboard(now: number): void {
    const ctx = this.ctx,
      c = this.cfg;
    const x0 = this.boardX,
      top = this.hitY,
      h = this.kbH;
    const targets = this.activeTargets(now);
    const matched = this.matchedNotes(now);
    const style = c.keyboard;

    // white keys
    for (const k of this.kl.whites) {
      const tHand = targets.get(k.note);
      const isMatch = matched.has(k.note);
      let fill = "#f3f1ea",
        topF = "#ffffff";
      if (tHand) {
        const col = this.keyTint(k.note, tHand);
        fill = col;
        topF = withAlpha("#ffffff", 0.6);
      }
      // Live: a held key that is not (yet) a target reads as "you" (blue).
      if (this.live && !tHand && this.liveHeld.has(k.note)) {
        fill = "#6fa8ff";
        topF = "#cfe0ff";
      }
      if (isMatch) {
        fill = "#7ee08a";
        topF = "#d8ffdd";
      }
      // Live wait-mode: pulse the awaited keys so the player knows what to hold.
      if (this.live && this.liveAwaiting.has(k.note)) {
        const p = 0.5 + 0.5 * Math.sin(performance.now() / 140);
        fill = withAlpha("#ffd166", 0.55 + 0.45 * p);
        topF = "#fff0c4";
      }
      if (style === "realistic") {
        const g = ctx.createLinearGradient(0, top, 0, top + h);
        g.addColorStop(0, topF);
        g.addColorStop(0.12, fill);
        g.addColorStop(1, shade(fill, -0.14));
        ctx.fillStyle = g;
      } else {
        ctx.fillStyle = fill;
      }
      ctx.fillRect(x0 + k.x + 0.5, top, k.w - 1, h);
      // key separators
      ctx.strokeStyle = "rgba(0,0,0,0.22)";
      ctx.lineWidth = 1;
      ctx.strokeRect(x0 + k.x + 0.5, top, k.w - 1, h);
      if (style === "realistic") {
        ctx.fillStyle = "rgba(0,0,0,0.05)";
        ctx.fillRect(x0 + k.x + 0.5, top + h - 4, k.w - 1, 4);
      }
      // C-note labels under the board (spectrum/educational)
      if (c.labels && k.note % 12 === 0) {
        ctx.fillStyle = "rgba(20,20,24,0.55)";
        ctx.font = `500 ${Math.min(10, k.w * 0.62)}px ${c.fontMono}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "bottom";
        ctx.fillText(noteName(k.note), x0 + k.x + k.w / 2, top + h - 3);
      }
    }

    // a thin top bezel for realism
    if (style === "realistic") {
      ctx.fillStyle = "rgba(0,0,0,0.5)";
      ctx.fillRect(0, top - 3, this.w, 3);
    }

    // black keys
    const bh = h * (style === "strip" ? 0.6 : 0.62);
    for (const k of this.kl.blacks) {
      const tHand = targets.get(k.note);
      const isMatch = matched.has(k.note);
      let fill = "#1c1c20";
      if (tHand) fill = this.keyTint(k.note, tHand, true);
      if (isMatch) fill = "#3fae54";
      if (style === "realistic") {
        const g = ctx.createLinearGradient(0, top, 0, top + bh);
        g.addColorStop(0, shade(fill, 0.25));
        g.addColorStop(0.7, fill);
        g.addColorStop(1, shade(fill, -0.3));
        ctx.fillStyle = g;
      } else {
        ctx.fillStyle = fill;
      }
      roundRect(ctx, x0 + k.x, top, k.w, bh, [0, 0, 2, 2]);
      ctx.fill();
      if (style === "realistic") {
        ctx.fillStyle = "rgba(255,255,255,0.10)";
        ctx.fillRect(x0 + k.x + 1, top + 1, k.w - 2, 2);
      }
    }
  }

  private keyTint(note: number, hand: "L" | "R", black = false): string {
    const c = this.cfg;
    if (c.colorMode === "spectrum") {
      const h = spectrumHue(note);
      return `oklch(${black ? 0.55 : 0.78} 0.15 ${h})`;
    }
    const base = c.colorMode === "accent" ? c.accent : c.handColors[hand] || c.accent;
    return black ? shade(base, -0.15) : base;
  }

  private drawRuler(): void {
    const ctx = this.ctx,
      c = this.cfg;
    ctx.save();
    ctx.font = `500 10px ${c.fontMono}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    // mark each C across the board at the hit line region
    for (const k of this.kl.whites) {
      if (k.note % 12 !== 0) continue;
      ctx.strokeStyle = "rgba(255,255,255,0.06)";
      ctx.beginPath();
      ctx.moveTo(this.boardX + k.x, 0);
      ctx.lineTo(this.boardX + k.x, this.hitY);
      ctx.stroke();
      ctx.fillStyle = "rgba(255,255,255,0.32)";
      ctx.fillText(noteName(k.note), this.boardX + k.x + 3, 12);
    }
    ctx.restore();
  }

  // ── simulated player + scoring ─────────────────────────────────────────
  private matchedNotes(now: number): Set<number> {
    const set = new Set<number>();
    // Live (#168): a "match" is a target note the player is actually holding.
    if (this.live) {
      for (const nt of this.song.notes) {
        if (nt.start <= now && now < nt.end && this.liveHeld.has(nt.note)) {
          set.add(nt.note);
        }
      }
      return set;
    }
    // Demo: notes whose onset was recently "hit" by the sim, still sounding.
    if (!this.cfg.scoring) return set;
    for (const nt of this.song.notes) {
      if (nt.start <= now && now < nt.end && this.hitNotes.has(nt)) set.add(nt.note);
    }
    return set;
  }

  private runScoring(now: number): void {
    const c = this.cfg;
    if (now < this.lastNow) this.judged.clear(); // looped: re-judge
    for (const nt of this.song.notes) {
      if (this.judged.has(nt)) continue;
      if (now >= nt.start) {
        this.judged.add(nt);
        this.total++;
        // deterministic pseudo-result: mostly perfect, some great, rare miss
        const seed = (nt.note * 31 + Math.round(nt.start / 50)) % 100;
        let res: string, pts: number, col: string;
        if (seed < 8) {
          res = "MISS";
          pts = 0;
          col = "#ff5d6c";
          this.combo = 0;
        } else if (seed < 30) {
          res = "GREAT";
          pts = 50 + this.combo;
          col = "#ffd166";
          this.combo++;
          this.hits++;
          this.hitNotes.add(nt);
        } else {
          res = "PERFECT";
          pts = 100 + this.combo * 2;
          col = "#5be7c4";
          this.combo++;
          this.hits++;
          this.hitNotes.add(nt);
        }
        this.bestCombo = Math.max(this.bestCombo, this.combo);
        this.score += pts;
        const lane = this.kl.byNote[nt.note];
        // one central judgment readout (latest wins) + per-lane flash + sparks
        this.judge = { text: res, color: col, born: performance.now() };
        if (lane && res !== "MISS") {
          const fcol = c.colorMode === "spectrum" ? this.noteColor(nt, 0.45) : col;
          this.flashes.push({ cx: lane.cx, w: lane.w, color: fcol, born: performance.now() });
          const n = res === "PERFECT" ? 7 : 4;
          for (let i = 0; i < n; i++) {
            const a = -Math.PI / 2 + (Math.random() - 0.5) * 1.15;
            const sp = 1.1 + Math.random() * 2.4;
            this.particles.push({
              x: lane.cx,
              y: this.hitY,
              vx: Math.cos(a) * sp,
              vy: Math.sin(a) * sp * 1.7,
              color: fcol,
              born: performance.now(),
              life: 460 + Math.random() * 260,
            });
          }
        }
      }
    }
    // expire matched notes that ended
    for (const nt of [...this.hitNotes]) {
      if (now >= nt.end || now < nt.start) this.hitNotes.delete(nt);
    }
  }

  private drawScoreFx(): void {
    const ctx = this.ctx,
      c = this.cfg,
      t = performance.now();
    // rising spark particles
    this.particles = this.particles.filter((p) => t - p.born < p.life);
    ctx.save();
    for (const p of this.particles) {
      const age = (t - p.born) / p.life;
      const dt = (t - p.born) / 16;
      const x = p.x + p.vx * dt;
      const y = p.y + p.vy * dt + 0.05 * dt * dt; // slight gravity
      ctx.globalAlpha = (1 - age) * 0.9;
      ctx.fillStyle = p.color;
      ctx.shadowColor = p.color;
      ctx.shadowBlur = 8;
      const r = (1 - age) * 2.2 + 0.6;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();
    // hit flashes at the line
    this.flashes = this.flashes.filter((f) => t - f.born < 420);
    for (const f of this.flashes) {
      const k = (t - f.born) / 420;
      ctx.save();
      ctx.globalAlpha = (1 - k) * 0.8;
      ctx.fillStyle = f.color;
      ctx.shadowColor = f.color;
      ctx.shadowBlur = 24;
      const r = f.w * (0.6 + k * 1.6);
      ctx.beginPath();
      ctx.ellipse(f.cx, this.hitY, r, r * 0.5, 0, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    }
    // single central judgment readout
    if (this.judge) {
      const k = (t - this.judge.born) / 520;
      if (k < 1) {
        const pop = k < 0.2 ? k / 0.2 : 1; // quick scale-in
        ctx.save();
        ctx.globalAlpha = k > 0.7 ? (1 - k) / 0.3 : 1;
        ctx.fillStyle = this.judge.color;
        ctx.font = `700 ${22 * (0.7 + pop * 0.3)}px ${c.fontDisp}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.shadowColor = this.judge.color;
        ctx.shadowBlur = 18;
        ctx.fillText(this.judge.text, this.centerX, this.hitY - 46);
        ctx.restore();
      }
    }
  }
}
