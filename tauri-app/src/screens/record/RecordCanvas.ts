// RecordCanvas.ts — the recording counterpart to the note highway, ported from
// design/record_screen/rockcraft-proto/record.js as a TypeScript class. Instead
// of notes FALLING toward the keyboard to be played, notes are EMITTED at the
// keyboard as the player performs and RISE upward, leaving a trail of what has
// been captured. Design E uses viz "ribbons+staff": ribbons rise in the lower
// band and crystallise into grand-staff notation above. Colours follow the
// Spectrum scheme (hue by pitch class). The sample take loops; each loop the
// capture clears and refills, so the screen stays live.
//
// Framework-free by design (per tauri-app/CONVENTIONS.md the canvas engines are
// unaffected by the React→Solid swap). Driven by an internal rAF loop.

import type { RecordConfig, ResolvedConfig, Song, TakeNote } from "./types";
import {
  clamp,
  diatonic,
  isBlack,
  type KeyLayout,
  keyLayout,
  lerp,
  noteName,
  roundRect,
  spec,
  withAlpha,
} from "./utils";

const DEFAULTS: ResolvedConfig = {
  viz: "ribbons",
  bg: "#0f1016",
  kbRatio: 0.17,
  window: 4200, // ms of capture shown rising above the keys
  labels: true,
  glow: 0.55,
  radius: 4,
  noteGap: 0.2,
  showKeyboard: true,
  recording: true,
  paused: false,
  selectNote: true, // show one selected note (edit views)
  accent: "#ff4d57", // record red — playhead / hit-line tint
  fontMono: "'IBM Plex Mono', ui-monospace, monospace",
  fontDisp: "'Space Grotesk', system-ui, sans-serif",
  fontMusic: "'Bravura', 'Apple Symbols', 'Segoe UI Symbol', serif",
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
interface Ring {
  x: number;
  y: number;
  w: number;
  color: string;
  born: number;
}
interface Glow {
  born: number;
  vel: number;
}

export class RecordCanvas {
  readonly cfg: ResolvedConfig;
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private song: Song;

  private t0 = performance.now();
  /** ms elapsed into the current loop (read by the chrome). */
  tNow = 0;
  private lastNow = 0;
  /** 0..1 smoothed meter value (read by the chrome). */
  level = 0;
  /** currently selected note, or null (read by the chrome). */
  sel: TakeNote | null;

  private held = new Set<number>();
  private particles: Particle[] = [];
  private rings: Ring[] = [];
  private keyGlow: Record<number, Glow> = {};
  private pMin: number;
  private pMax: number;

  private ro: ResizeObserver;
  private raf: number | null = null;
  private running = false;
  private err: string | null = null;

  // geometry (set in resize)
  private w = 1;
  private h = 1;
  private boardW = 1;
  private kl!: KeyLayout;
  private kbH = 0;
  private hitY = 0;
  private rollX0 = 50;
  private rollTop = 30;
  private rollX1 = 1;
  private rollBot = 0;

  constructor(canvas: HTMLCanvasElement, cfg: RecordConfig, song: Song) {
    this.canvas = canvas;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2d context unavailable");
    this.ctx = ctx;
    this.cfg = { ...DEFAULTS, ...cfg };
    this.song = song;
    // pitch span actually used by the take (for roll + ruler)
    let lo = 200;
    let hi = 0;
    for (const nt of song.notes) {
      lo = Math.min(lo, nt.note);
      hi = Math.max(hi, nt.note);
    }
    this.pMin = lo - 2;
    this.pMax = hi + 2;
    // pick a melody note to show "selected" in edit views
    this.sel = song.notes.find((n) => n.hand === "R" && n.note >= 84) ?? song.notes[0] ?? null;
    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(canvas);
    this.resize();
  }

  /** ms elapsed into the current loop. */
  get now(): number {
    return this.tNow;
  }

  private resize(): void {
    const r = this.canvas.getBoundingClientRect();
    this.w = Math.max(1, Math.round(r.width));
    this.h = Math.max(1, Math.round(r.height));
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.canvas.width = this.w * dpr;
    this.canvas.height = this.h * dpr;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.boardW = this.w;
    this.kl = keyLayout(this.boardW);
    this.kbH = this.cfg.showKeyboard ? Math.round(this.h * this.cfg.kbRatio) : 0;
    this.hitY = this.h - this.kbH;
    // roll geometry
    this.rollX0 = 50;
    this.rollTop = 30;
    this.rollX1 = this.w - 14;
    this.rollBot = this.hitY - 12;
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    const loop = (): void => {
      if (!this.running) return;
      this.tick();
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  stop(): void {
    this.running = false;
    if (this.raf != null) cancelAnimationFrame(this.raf);
    this.raf = null;
    this.ro.disconnect();
  }

  private tick(): void {
    if (!this.canvas.isConnected) return; // skip off-DOM
    try {
      this.frame();
    } catch (e) {
      if (!this.err) {
        this.err = String((e as Error)?.stack ?? e);
        console.error("record frame error", e);
      }
    }
  }

  private clockNow(): number {
    if (this.cfg.paused) return this.tNow; // freeze when paused
    const elapsed = performance.now() - this.t0;
    return elapsed % this.song.LOOP;
  }

  // ── shared updates ──────────────────────────────────────────────────────
  private updateLive(now: number): void {
    // detect note onsets crossed since last frame -> spark + ring + glow
    const wrapped = now < this.lastNow;
    this.held.clear();
    let lvl = 0;
    for (const nt of this.song.notes) {
      if (nt.start <= now && now < nt.end) {
        this.held.add(nt.note);
        lvl += nt.vel;
      }
      const crossed = wrapped
        ? nt.start > this.lastNow || nt.start <= now
        : nt.start > this.lastNow && nt.start <= now;
      if (crossed && !this.cfg.paused) this.fire(nt);
    }
    this.level = lerp(this.level, clamp(lvl / 180, 0, 1), 0.25);
    this.lastNow = now;
  }

  private fire(nt: TakeNote): void {
    const lane = this.kl.byNote[nt.note];
    if (!lane) return;
    this.keyGlow[nt.note] = { born: performance.now(), vel: nt.vel };
    const col = spec(nt.note, 0.78);
    const x = lane.cx;
    const y = this.hitY;
    this.rings.push({ x, y, w: lane.w, color: col, born: performance.now() });
    const n = 4 + Math.round((nt.vel / 127) * 5);
    for (let i = 0; i < n; i++) {
      const a = -Math.PI / 2 + (Math.random() - 0.5) * 1.0;
      const sp = 1 + Math.random() * 2.6 * (nt.vel / 100);
      this.particles.push({
        x,
        y,
        vx: Math.cos(a) * sp,
        vy: Math.sin(a) * sp * 1.6,
        color: col,
        born: performance.now(),
        life: 460 + Math.random() * 320,
      });
    }
  }

  private frame(): void {
    const ctx = this.ctx;
    const c = this.cfg;
    const now = (this.tNow = this.clockNow());
    this.updateLive(now);
    ctx.clearRect(0, 0, this.w, this.h);
    ctx.fillStyle = c.bg;
    ctx.fillRect(0, 0, this.w, this.h);

    if (c.viz === "roll") {
      this.drawRoll(now);
    } else if (c.viz === "staff") {
      this.drawStaff(now, 18, this.hitY - 18, false);
      this.drawHitLine(0.5);
    } else if (c.viz === "ribbons+staff") {
      const split = Math.round(this.hitY * 0.52);
      this.drawStaff(now, 14, split - 6, true);
      this.drawBandDivider(split);
      this.drawRibbons(now, split + 8, this.hitY);
      this.drawHitLine(0.7);
    } else {
      this.drawRibbons(now, 0, this.hitY);
      this.drawHitLine(0.7);
    }

    if (c.showKeyboard) this.drawKeyboard();
    this.drawLiveFx();
  }

  // ── RIBBONS (rising trails) ───────────────────────────────────────────────
  private drawRibbons(now: number, top: number, bottom: number): void {
    const ctx = this.ctx;
    const c = this.cfg;
    const H = bottom - top;
    const ppm = H / c.window;
    // faint lane tint columns
    ctx.fillStyle = "rgba(255,255,255,0.014)";
    for (const k of this.kl.whites) if ((k.wi ?? 0) % 2 === 0) ctx.fillRect(k.x, top, k.w, H);

    for (const nt of this.song.notes) {
      if (nt.start > now) continue; // not recorded yet
      const lane = this.kl.byNote[nt.note];
      if (!lane) continue;
      const endC = Math.min(nt.end, now);
      const yBot = bottom - (now - endC) * ppm; // young edge (near keys)
      let yTop = bottom - (now - nt.start) * ppm; // old edge (rising)
      if (yBot < top || yTop > bottom + 4) continue;
      yTop = Math.max(yTop, top - 2);
      const held = now < nt.end;
      const gap = lane.black ? 0.08 : c.noteGap;
      const halfW = (lane.w * (1 - gap)) / 2;
      const L = lane.cx - halfW;
      const depth = clamp((yBot - top) / H, 0, 1); // 1 near keys
      const col = spec(nt.note, 0.62 + depth * 0.16);
      const fadeTop = clamp((bottom - yTop) / H, 0, 1);

      ctx.save();
      if (c.glow) {
        ctx.shadowColor = col;
        ctx.shadowBlur = (6 + depth * 16) * c.glow;
      }
      const g = ctx.createLinearGradient(0, yTop, 0, yBot);
      g.addColorStop(0, withAlpha(col, 0.05 + fadeTop * 0.55));
      g.addColorStop(1, withAlpha(col, 0.95));
      ctx.fillStyle = g;
      roundRect(ctx, L, yTop, halfW * 2, Math.max(3, yBot - yTop), c.radius);
      ctx.fill();
      ctx.restore();

      // bright growing tip at the young (lower) edge while held
      ctx.save();
      ctx.beginPath();
      ctx.moveTo(L, yBot);
      ctx.lineTo(L + halfW * 2, yBot);
      ctx.lineWidth = held ? 3 : 1.6;
      ctx.strokeStyle = withAlpha("#ffffff", held ? 0.92 : 0.4);
      if (held) {
        ctx.shadowColor = col;
        ctx.shadowBlur = 12;
      }
      ctx.stroke();
      ctx.restore();

      if (c.labels && yBot - yTop > 15 && !lane.black) {
        ctx.save();
        ctx.fillStyle = withAlpha("#0a0a0c", 0.82);
        ctx.font = `600 ${Math.min(11, lane.w * 0.66)}px ${c.fontMono}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(noteName(nt.note).replace(/-?\d+$/, ""), lane.cx, yBot - 8);
        ctx.restore();
      }
    }
  }

  private drawBandDivider(y: number): void {
    const ctx = this.ctx;
    const g = ctx.createLinearGradient(0, y - 10, 0, y + 10);
    g.addColorStop(0, "rgba(255,255,255,0)");
    g.addColorStop(0.5, "rgba(255,255,255,0.07)");
    g.addColorStop(1, "rgba(255,255,255,0)");
    ctx.fillStyle = g;
    ctx.fillRect(0, y - 10, this.w, 20);
  }

  // ── STAFF (stylized grand staff that fills) ───────────────────────────────
  private drawStaff(now: number, top: number, bottom: number, compact: boolean): void {
    const ctx = this.ctx;
    const c = this.cfg;
    const x0 = 64;
    const x1 = this.w - 24;
    const mid = (top + bottom) / 2;
    const lineGap = compact ? 7.5 : 9;
    const step = lineGap / 2; // one diatonic step
    const trebMid = mid - lineGap * 3.2; // treble centre line y
    const bassMid = mid + lineGap * 3.2;
    const yTreb = (dia: number): number => trebMid - (dia - diatonic(71)) * step; // B4 centre
    const yBass = (dia: number): number => bassMid - (dia - diatonic(50)) * step; // D3 centre
    const yOf = (n: number): number => (n >= 60 ? yTreb(diatonic(n)) : yBass(diatonic(n)));

    // staff lines
    ctx.strokeStyle = "rgba(255,255,255,0.16)";
    ctx.lineWidth = 1;
    for (const cy of [trebMid, bassMid]) {
      for (let i = -2; i <= 2; i++) {
        const y = cy + i * lineGap;
        ctx.beginPath();
        ctx.moveTo(x0, y);
        ctx.lineTo(x1, y);
        ctx.stroke();
      }
    }
    // brace + left bar
    ctx.strokeStyle = "rgba(255,255,255,0.3)";
    ctx.lineWidth = 1.4;
    ctx.beginPath();
    ctx.moveTo(x0, trebMid - lineGap * 2);
    ctx.lineTo(x0, bassMid + lineGap * 2);
    ctx.stroke();
    // clefs
    ctx.fillStyle = "rgba(255,255,255,0.62)";
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.font = `${lineGap * 4.6}px ${c.fontMusic}`;
    ctx.fillText("\u{1D11E}", x0 + 6, trebMid - lineGap * 0.2);
    ctx.font = `${lineGap * 3.4}px ${c.fontMusic}`;
    ctx.fillText("\u{1D122}", x0 + 6, bassMid - lineGap * 0.9);
    // time signature
    ctx.fillStyle = "rgba(255,255,255,0.4)";
    ctx.font = `700 ${lineGap * 1.9}px ${c.fontDisp}`;
    ctx.textAlign = "center";
    ctx.fillText("4", x0 + lineGap * 4.4, trebMid - lineGap);
    ctx.fillText("4", x0 + lineGap * 4.4, trebMid + lineGap);

    // bar lines (4 measures across the staff)
    const notesX0 = x0 + lineGap * 6;
    const bars = this.song.BARS;
    const barW = (x1 - notesX0) / bars;
    const xOfTime = (t: number): number => notesX0 + (t / this.song.LOOP) * (x1 - notesX0);
    ctx.strokeStyle = "rgba(255,255,255,0.13)";
    ctx.lineWidth = 1;
    for (let b = 0; b <= bars; b++) {
      const x = notesX0 + b * barW;
      ctx.beginPath();
      ctx.moveTo(x, trebMid - lineGap * 2);
      ctx.lineTo(x, trebMid + lineGap * 2);
      ctx.stroke();
      ctx.beginPath();
      ctx.moveTo(x, bassMid - lineGap * 2);
      ctx.lineTo(x, bassMid + lineGap * 2);
      ctx.stroke();
    }

    // playhead (write position)
    const px = xOfTime(now);
    ctx.save();
    ctx.strokeStyle = withAlpha(c.accent, 0.8);
    ctx.lineWidth = 1.6;
    ctx.shadowColor = c.accent;
    ctx.shadowBlur = 10;
    ctx.beginPath();
    ctx.moveTo(px, trebMid - lineGap * 3);
    ctx.lineTo(px, bassMid + lineGap * 3);
    ctx.stroke();
    ctx.restore();

    const headRx = lineGap * 0.62;
    const headRy = lineGap * 0.46;
    for (const nt of this.song.notes) {
      if (nt.start > now) continue;
      const x = xOfTime(nt.start);
      const y = yOf(nt.note);
      const cy = nt.note >= 60 ? trebMid : bassMid;
      const fresh = clamp(1 - (now - nt.start) / 280, 0, 1);
      const col = spec(nt.note, 0.74);
      // ledger lines
      ctx.strokeStyle = "rgba(255,255,255,0.22)";
      ctx.lineWidth = 1;
      const above = y < cy - lineGap * 2 - 1;
      const below = y > cy + lineGap * 2 + 1;
      if (above || below) {
        for (let k = 0; k < 3; k++) {
          const lyy = cy + (above ? -1 : 1) * lineGap * (3 + k);
          if ((above && lyy >= y - 1) || (below && lyy <= y + 1)) {
            ctx.beginPath();
            ctx.moveTo(x - headRx - 4, lyy);
            ctx.lineTo(x + headRx + 4, lyy);
            ctx.stroke();
          }
        }
      }
      // stem
      const stemUp = y > (nt.note >= 60 ? trebMid : bassMid);
      ctx.strokeStyle = withAlpha(col, 0.85);
      ctx.lineWidth = 1.6;
      ctx.beginPath();
      ctx.moveTo(x + (stemUp ? headRx - 0.5 : -headRx + 0.5), y);
      ctx.lineTo(
        x + (stemUp ? headRx - 0.5 : -headRx + 0.5),
        y + (stemUp ? -lineGap * 3.3 : lineGap * 3.3),
      );
      ctx.stroke();
      // accidental
      if (isBlack(nt.note)) {
        ctx.fillStyle = withAlpha("#ffffff", 0.6);
        ctx.font = `${lineGap * 2}px ${c.fontMusic}`;
        ctx.textAlign = "right";
        ctx.textBaseline = "middle";
        ctx.fillText("♯", x - headRx - 2, y);
      }
      // head
      ctx.save();
      ctx.translate(x, y);
      ctx.rotate(-0.32);
      if (fresh > 0) {
        ctx.shadowColor = col;
        ctx.shadowBlur = 14 * fresh;
      }
      ctx.fillStyle = col;
      ctx.beginPath();
      ctx.ellipse(0, 0, headRx, headRy, 0, 0, Math.PI * 2);
      ctx.fill();
      if (fresh > 0.05) {
        ctx.globalAlpha = fresh;
        ctx.fillStyle = "#fff";
        ctx.beginPath();
        ctx.ellipse(0, 0, headRx * 0.5, headRy * 0.5, 0, 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.restore();
    }
  }

  // ── ROLL (DAW horizontal piano-roll edit timeline) ────────────────────────
  private drawRoll(now: number): void {
    const ctx = this.ctx;
    const c = this.cfg;
    const X0 = this.rollX0;
    const X1 = this.rollX1;
    const T = this.rollTop;
    const B = this.rollBot;
    const rowH = (B - T) / (this.pMax - this.pMin + 1);
    const yOfP = (n: number): number => T + (this.pMax - n) * rowH;
    const xOf = (t: number): number => X0 + (t / this.song.LOOP) * (X1 - X0);
    const { BEAT, BAR } = this.song;

    // pitch rows — tint black-key rows, label each C
    for (let n = this.pMin; n <= this.pMax; n++) {
      const y = yOfP(n);
      ctx.fillStyle = isBlack(n) ? "rgba(255,255,255,0.022)" : "rgba(255,255,255,0.004)";
      ctx.fillRect(X0, y, X1 - X0, rowH);
      if (n % 12 === 0) {
        ctx.fillStyle = "rgba(255,255,255,0.04)";
        ctx.fillRect(X0, y, X1 - X0, rowH);
        ctx.fillStyle = "rgba(255,255,255,0.42)";
        ctx.font = `500 9px ${c.fontMono}`;
        ctx.textAlign = "right";
        ctx.textBaseline = "middle";
        ctx.fillText(noteName(n), X0 - 6, y + rowH / 2);
      }
    }
    // mini vertical keyboard guide
    for (let n = this.pMin; n <= this.pMax; n++) {
      const y = yOfP(n);
      ctx.fillStyle = isBlack(n) ? "#16171d" : "#23242c";
      ctx.fillRect(X0 - 30, y + 0.5, 24, rowH - 1);
      if (this.held.has(n)) {
        ctx.fillStyle = spec(n, 0.7);
        ctx.fillRect(X0 - 30, y + 0.5, 24, rowH - 1);
      }
    }
    ctx.strokeStyle = "rgba(0,0,0,0.4)";
    ctx.lineWidth = 1;
    ctx.strokeRect(X0 - 30, T, 24, B - T);

    // grid: beats (faint) + bars (strong) + snap subdivisions
    for (let t = 0; t <= this.song.LOOP; t += BEAT / 2) {
      const x = xOf(t);
      const isBar = t % BAR === 0;
      const isBeat = t % BEAT === 0;
      ctx.strokeStyle = isBar
        ? "rgba(255,255,255,0.16)"
        : isBeat
          ? "rgba(255,255,255,0.07)"
          : "rgba(255,255,255,0.03)";
      ctx.lineWidth = isBar ? 1.4 : 1;
      ctx.beginPath();
      ctx.moveTo(x, T);
      ctx.lineTo(x, B);
      ctx.stroke();
    }
    // time ruler
    ctx.fillStyle = "rgba(255,255,255,0.5)";
    ctx.font = `500 9px ${c.fontMono}`;
    ctx.textAlign = "left";
    ctx.textBaseline = "alphabetic";
    for (let b = 0; b < this.song.BARS; b++) ctx.fillText(`${b + 1}`, xOf(b * BAR) + 4, T - 6);

    // recorded notes (only up to playhead)
    for (const nt of this.song.notes) {
      if (nt.start > now) continue;
      const x = xOf(nt.start);
      const xe = xOf(Math.min(nt.end, now));
      const y = yOfP(nt.note) + 1;
      const hgt = rowH - 2;
      const col = spec(nt.note, 0.7);
      const isSel = this.cfg.selectNote && nt === this.sel;
      const fresh = clamp(1 - (now - nt.start) / 240, 0, 1);
      ctx.save();
      if (fresh > 0 || isSel) {
        ctx.shadowColor = col;
        ctx.shadowBlur = isSel ? 10 : 10 * fresh;
      }
      const g = ctx.createLinearGradient(x, 0, xe, 0);
      g.addColorStop(0, withAlpha(col, 1));
      g.addColorStop(1, withAlpha(col, 0.78));
      ctx.fillStyle = g;
      roundRect(ctx, x, y, Math.max(3, xe - x), hgt, Math.min(3, hgt / 2));
      ctx.fill();
      // velocity cap (left edge intensity)
      ctx.fillStyle = withAlpha("#fff", 0.18 + (nt.vel / 127) * 0.5);
      ctx.fillRect(x, y, 2.5, hgt);
      ctx.restore();
      if (isSel) {
        ctx.strokeStyle = "#fff";
        ctx.lineWidth = 1.4;
        roundRect(ctx, x - 0.5, y - 0.5, Math.max(3, xe - x) + 1, hgt + 1, Math.min(3, hgt / 2));
        ctx.stroke();
        // resize handles
        ctx.fillStyle = "#fff";
        for (const hx of [x, xe]) ctx.fillRect(hx - 1.5, y + hgt / 2 - 3, 3, 6);
      }
    }

    // playhead
    const px = xOf(now);
    ctx.save();
    ctx.strokeStyle = c.accent;
    ctx.lineWidth = 1.6;
    ctx.shadowColor = c.accent;
    ctx.shadowBlur = 10;
    ctx.beginPath();
    ctx.moveTo(px, T - 4);
    ctx.lineTo(px, B);
    ctx.stroke();
    ctx.fillStyle = c.accent;
    ctx.beginPath();
    ctx.moveTo(px - 4, T - 4);
    ctx.lineTo(px + 4, T - 4);
    ctx.lineTo(px, T + 2);
    ctx.closePath();
    ctx.fill();
    ctx.restore();
  }

  private drawHitLine(a: number): void {
    const ctx = this.ctx;
    const y = this.hitY;
    const g = ctx.createLinearGradient(0, y - 18, 0, y);
    g.addColorStop(0, "rgba(255,255,255,0)");
    g.addColorStop(1, withAlpha(this.cfg.accent, 0.08 * a + 0.04));
    ctx.fillStyle = g;
    ctx.fillRect(0, y - 18, this.w, 18);
    ctx.beginPath();
    ctx.moveTo(0, y + 0.5);
    ctx.lineTo(this.w, y + 0.5);
    ctx.lineWidth = 2;
    ctx.strokeStyle = withAlpha("#ffffff", 0.18 * a + 0.1);
    ctx.stroke();
  }

  // ── KEYBOARD (flat, spectrum tints on held / glowing keys) ────────────────
  private drawKeyboard(): void {
    const ctx = this.ctx;
    const c = this.cfg;
    if (this.kbH <= 0) return;
    const top = this.hitY;
    const h = this.kbH;
    const t = performance.now();
    // top bezel
    ctx.fillStyle = "rgba(0,0,0,0.55)";
    ctx.fillRect(0, top - 3, this.w, 3);
    for (const k of this.kl.whites) {
      ctx.fillStyle = "#eceae3";
      ctx.fillRect(k.x + 0.5, top, k.w - 1, h);
      const g = this.keyGlow[k.note];
      const lit = this.held.has(k.note);
      const tintA = lit ? 0.9 : g ? clamp(1 - (t - g.born) / 320, 0, 1) * 0.8 : 0;
      if (tintA > 0) {
        ctx.save();
        ctx.globalAlpha = tintA;
        ctx.fillStyle = spec(k.note, 0.78);
        ctx.fillRect(k.x + 0.5, top, k.w - 1, h);
        ctx.restore();
      }
      ctx.strokeStyle = "rgba(0,0,0,0.2)";
      ctx.lineWidth = 1;
      ctx.strokeRect(k.x + 0.5, top, k.w - 1, h);
      ctx.fillStyle = "rgba(0,0,0,0.05)";
      ctx.fillRect(k.x + 0.5, top + h - 3, k.w - 1, 3);
      if (c.labels && k.note % 12 === 0) {
        ctx.fillStyle = "rgba(20,20,24,0.5)";
        ctx.font = `500 ${Math.min(9, k.w * 0.6)}px ${c.fontMono}`;
        ctx.textAlign = "center";
        ctx.textBaseline = "bottom";
        ctx.fillText(noteName(k.note), k.x + k.w / 2, top + h - 3);
      }
    }
    const bh = h * 0.62;
    for (const k of this.kl.blacks) {
      ctx.fillStyle = "#191a1f";
      roundRect(ctx, k.x, top, k.w, bh, [0, 0, 2, 2]);
      ctx.fill();
      const g = this.keyGlow[k.note];
      const lit = this.held.has(k.note);
      const tintA = lit ? 0.95 : g ? clamp(1 - (t - g.born) / 320, 0, 1) * 0.9 : 0;
      if (tintA > 0) {
        ctx.save();
        ctx.globalAlpha = tintA;
        ctx.fillStyle = spec(k.note, 0.6);
        roundRect(ctx, k.x, top, k.w, bh, [0, 0, 2, 2]);
        ctx.fill();
        ctx.restore();
      }
      ctx.fillStyle = "rgba(255,255,255,0.08)";
      ctx.fillRect(k.x + 1, top + 1, k.w - 2, 2);
    }
  }

  private drawLiveFx(): void {
    const ctx = this.ctx;
    const t = performance.now();
    this.particles = this.particles.filter((p) => t - p.born < p.life);
    ctx.save();
    for (const p of this.particles) {
      const age = (t - p.born) / p.life;
      const dt = (t - p.born) / 16;
      const x = p.x + p.vx * dt;
      const y = p.y + p.vy * dt + 0.04 * dt * dt;
      ctx.globalAlpha = (1 - age) * 0.9;
      ctx.fillStyle = p.color;
      ctx.shadowColor = p.color;
      ctx.shadowBlur = 8;
      const r = (1 - age) * 2 + 0.5;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();
    this.rings = this.rings.filter((f) => t - f.born < 460);
    for (const f of this.rings) {
      const k = (t - f.born) / 460;
      ctx.save();
      ctx.globalAlpha = (1 - k) * 0.7;
      ctx.strokeStyle = f.color;
      ctx.shadowColor = f.color;
      ctx.shadowBlur = 16;
      ctx.lineWidth = 2;
      const r = f.w * (0.4 + k * 1.8);
      ctx.beginPath();
      ctx.ellipse(f.x, f.y, r, r * 0.45, 0, Math.PI, Math.PI * 2);
      ctx.stroke();
      ctx.restore();
    }
  }
}
