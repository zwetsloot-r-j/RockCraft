// highway.js — HighwayCanvas: a configurable falling-note highway + 88-key
// board, drawn on a single <canvas>. One class, three looks (driven by config).
// Faithful to highway.rs projection: time t maps to a vertical position where
// `now` is the keyboard hit-line at the bottom and `now + lead` is the top.
(function () {
  const { LOWEST, HIGHEST, isBlack, pitchClass, noteName, keyLayout } = window.RC;

  const lerp = (a, b, t) => a + (b - a) * t;
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

  // 12-hue spectrum by pitch class (Spectrum prototype). Even wheel, fixed L/C.
  const spectrumHue = (n) => (pitchClass(n) * 30 + 8) % 360;

  // Default config; each prototype overrides a subset.
  const DEFAULTS = {
    lead: 2600,            // ms of look-ahead shown top→hit-line
    bg: "#0e0f13",
    laneTint: "rgba(255,255,255,0.018)",
    boardInset: 0,         // px inset on each side of the keyboard
    kbRatio: 0.18,         // keyboard height as fraction of canvas height
    noteGap: 0.16,         // fraction of lane width trimmed (white notes)
    radius: 4,             // note corner radius
    colorMode: "hands",    // 'hands' | 'spectrum' | 'accent'
    handColors: { L: "#5ad1c7", R: "#e6a14b" },
    accent: "#7aa2ff",
    perspective: 0,        // 0 = flat; else top horizontal scale (e.g. 0.5)
    glow: 0,               // 0..1 glow strength on notes
    labels: false,         // note-name labels on notes
    gridlines: "soft",     // 'none' | 'soft' | 'perspective'
    keyboard: "realistic", // 'realistic' | 'flat' | 'strip'
    hitLine: "#ffffff",
    scoring: false,        // run a simulated player + score HUD (arcade)
    pitchRuler: false,     // left-edge octave ruler (spectrum)
    fontMono: "'IBM Plex Mono', ui-monospace, monospace",
    fontDisp: "'Space Grotesk', system-ui, sans-serif",
  };

  class HighwayCanvas {
    constructor(canvas, cfg) {
      this.canvas = canvas;
      this.ctx = canvas.getContext("2d");
      this.cfg = Object.assign({}, DEFAULTS, cfg);
      this.song = window.RC.SONG;
      this.t0 = performance.now();
      this.lastNow = 0;
      // scoring sim state
      this.score = 0; this.combo = 0; this.bestCombo = 0;
      this.hits = 0; this.total = 0;
      this.judged = new Set();
      this.popups = [];     // {x,y,text,color,born}
      this.flashes = [];    // {cx,color,born}
      this.particles = [];  // {x,y,vx,vy,color,born,life}
      this._raf = null;
      this._ro = new ResizeObserver(() => this.resize());
      this._ro.observe(canvas);
      this.resize();
    }

    resize() {
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

    start() {
      const loop = () => {
        try { this.frame(); } catch (e) { if (!this._err) { this._err = String(e && e.stack || e); console.error("frame error", e); } }
        this._raf = requestAnimationFrame(loop);
      };
      this._raf = requestAnimationFrame(loop);
    }
    stop() { if (this._raf) cancelAnimationFrame(this._raf); this._ro.disconnect(); }

    // Horizontal perspective scale at highway y (0 top .. hitY bottom).
    sAt(y) {
      if (!this.cfg.perspective) return 1;
      const d = clamp(y / this.hitY, 0, 1); // 0 top, 1 hit-line
      return lerp(this.cfg.perspective, 1, d);
    }
    xP(x, s) { return this.centerX + (x - this.centerX) * s; }
    // time -> highway y (now at hitY, now+lead at 0)
    yOf(t, now) { return this.hitY * (1 - (t - now) / this.cfg.lead); }

    activeTargets(now) {
      const set = new Map(); // note -> hand
      for (const nt of this.song.notes) {
        if (nt.start <= now && now < nt.end) set.set(nt.note, nt.hand);
      }
      return set;
    }

    noteColor(nt, light = 0) {
      const c = this.cfg;
      if (c.colorMode === "spectrum") {
        const h = spectrumHue(nt.note);
        return `oklch(${0.7 + light * 0.12} 0.16 ${h})`;
      }
      if (c.colorMode === "accent") return c.accent;
      return c.handColors[nt.hand] || c.accent;
    }

    frame() {
      const ctx = this.ctx, c = this.cfg;
      const elapsed = performance.now() - this.t0;
      const now = elapsed % this.song.LOOP;
      if (this.cfg.scoring) this.runScoring(now);

      // bg
      ctx.clearRect(0, 0, this.w, this.h);
      this.drawBackground();
      this.drawGrid(now);
      this.drawNotes(now);
      this.drawHitLine();
      this.drawKeyboard(now);
      if (c.pitchRuler) this.drawRuler();
      if (c.scoring) this.drawScoreFx();
      this.lastNow = now;
    }

    drawBackground() {
      const ctx = this.ctx, c = this.cfg;
      ctx.fillStyle = c.bg;
      ctx.fillRect(0, 0, this.w, this.h);
      // faint lane tint columns (white keys) on the highway
      if (c.laneTint) {
        ctx.fillStyle = c.laneTint;
        for (const k of this.kl.whites) {
          if (k.wi % 2) continue;
          // taper with perspective by sampling top & bottom
          const sT = this.sAt(0), sB = this.sAt(this.hitY);
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

    drawGrid(now) {
      const ctx = this.ctx, c = this.cfg;
      if (c.gridlines === "none") return;
      const { BEAT, BAR } = this.song;
      // draw beat & bar lines within the visible window
      const firstBeat = Math.floor(now / BEAT) * BEAT;
      for (let t = firstBeat; t <= now + c.lead; t += BEAT) {
        const y = this.yOf(t, now);
        if (y < 0 || y > this.hitY) continue;
        const isBar = Math.round(t / BAR) * BAR === Math.round(t) ? (t % BAR < 1 || BAR - (t % BAR) < 1) : false;
        const bar = (Math.round(t) % BAR) === 0;
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

    drawNotes(now) {
      const ctx = this.ctx, c = this.cfg;
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

    drawNote(nt, lane, st, en, now) {
      const ctx = this.ctx, c = this.cfg;
      let yTop = clamp(this.yOf(en, now), -40, this.hitY);
      let yBot = clamp(this.yOf(st, now), -40, this.hitY);
      if (yBot - yTop < 3) yBot = yTop + 3; // min visible height
      const gap = lane.black ? 0.06 : c.noteGap;
      const halfW = (lane.w * (1 - gap)) / 2;
      const L = lane.cx - halfW, R = lane.cx + halfW;
      const sT = this.sAt(yTop), sB = this.sAt(yBot);
      const depth = 1 - clamp(yBot / this.hitY, 0, 1); // 0 far, 1 near hit
      const light = depth * 0.5;
      const col = this.noteColor(nt, light);

      ctx.save();
      // glow
      if (c.glow) {
        ctx.shadowColor = col;
        ctx.shadowBlur = (8 + depth * 22) * c.glow;
      }
      ctx.beginPath();
      if (!c.perspective) {
        roundRect(ctx, this.xP(L, 1), yTop, halfW * 2, yBot - yTop, c.radius);
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
      ctx.globalAlpha = lane.black ? 0.96 : 1;
      ctx.fill();
      ctx.restore();

      // bright leading edge (nearest the hit line)
      ctx.save();
      ctx.beginPath();
      const eL = this.xP(L, sB), eR = this.xP(R, sB);
      ctx.moveTo(eL, yBot); ctx.lineTo(eR, yBot);
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

    drawHitLine() {
      const ctx = this.ctx, c = this.cfg;
      const y = this.hitY;
      const g = ctx.createLinearGradient(0, y - 16, 0, y);
      g.addColorStop(0, "rgba(255,255,255,0)");
      g.addColorStop(1, withAlpha(c.hitLine, 0.10));
      ctx.fillStyle = g;
      ctx.fillRect(0, y - 16, this.w, 16);
      ctx.beginPath();
      ctx.moveTo(0, y + 0.5); ctx.lineTo(this.w, y + 0.5);
      ctx.lineWidth = 2;
      ctx.strokeStyle = withAlpha(c.hitLine, 0.5);
      ctx.stroke();
    }

    drawKeyboard(now) {
      const ctx = this.ctx, c = this.cfg;
      const x0 = this.boardX, top = this.hitY, h = this.kbH;
      const targets = this.activeTargets(now);
      const matched = this.matchedNotes(now);
      const style = c.keyboard;

      // white keys
      for (const k of this.kl.whites) {
        const tHand = targets.get(k.note);
        const isMatch = matched.has(k.note);
        let fill = "#f3f1ea", topF = "#ffffff";
        if (tHand) {
          const col = this.keyTint(k.note, tHand);
          fill = col; topF = withAlpha("#ffffff", 0.6);
        }
        if (isMatch) { fill = "#7ee08a"; topF = "#d8ffdd"; }
        if (style === "realistic") {
          const g = ctx.createLinearGradient(0, top, 0, top + h);
          g.addColorStop(0, topF); g.addColorStop(0.12, fill); g.addColorStop(1, shade(fill, -0.14));
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
          ctx.textAlign = "center"; ctx.textBaseline = "bottom";
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
          g.addColorStop(0, shade(fill, 0.25)); g.addColorStop(0.7, fill); g.addColorStop(1, shade(fill, -0.3));
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

    keyTint(note, hand, black = false) {
      const c = this.cfg;
      if (c.colorMode === "spectrum") {
        const h = spectrumHue(note);
        return `oklch(${black ? 0.55 : 0.78} 0.15 ${h})`;
      }
      const base = c.colorMode === "accent" ? c.accent : (c.handColors[hand] || c.accent);
      return black ? shade(base, -0.15) : base;
    }

    drawRuler() {
      const ctx = this.ctx, c = this.cfg;
      ctx.save();
      ctx.font = `500 10px ${c.fontMono}`;
      ctx.textAlign = "left"; ctx.textBaseline = "middle";
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

    // ── simulated player + scoring (arcade) ──────────────────────────────
    matchedNotes(now) {
      // notes whose onset was recently "hit" by the sim, still sounding
      if (!this.cfg.scoring) {
        // play-along guide: light targets as matches too (clean look)
        return new Set();
      }
      const set = new Set();
      for (const nt of this.song.notes) {
        if (nt.start <= now && now < nt.end && this._hitNotes && this._hitNotes.has(nt)) set.add(nt.note);
      }
      return set;
    }

    runScoring(now) {
      const c = this.cfg;
      if (now < this.lastNow) { this.judged.clear(); } // looped: re-judge
      if (!this._hitNotes) this._hitNotes = new Set();
      for (const nt of this.song.notes) {
        if (this.judged.has(nt)) continue;
        if (now >= nt.start) {
          this.judged.add(nt);
          this.total++;
          // deterministic pseudo-result: mostly perfect, some great, rare miss
          const seed = (nt.note * 31 + Math.round(nt.start / 50)) % 100;
          let res, pts, col;
          if (seed < 8) { res = "MISS"; pts = 0; col = "#ff5d6c"; this.combo = 0; }
          else if (seed < 30) { res = "GREAT"; pts = 50 + this.combo; col = "#ffd166"; this.combo++; this.hits++; this._hitNotes.add(nt); }
          else { res = "PERFECT"; pts = 100 + this.combo * 2; col = "#5be7c4"; this.combo++; this.hits++; this._hitNotes.add(nt); }
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
              this.particles.push({ x: lane.cx, y: this.hitY, vx: Math.cos(a) * sp, vy: Math.sin(a) * sp * 1.7, color: fcol, born: performance.now(), life: 460 + Math.random() * 260 });
            }
          }
        }
      }
      // expire matched notes that ended
      if (this._hitNotes) for (const nt of [...this._hitNotes]) if (now >= nt.end || now < nt.start) this._hitNotes.delete(nt);
    }

    drawScoreFx() {
      const ctx = this.ctx, c = this.cfg, t = performance.now();
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
        ctx.shadowColor = p.color; ctx.shadowBlur = 8;
        const r = (1 - age) * 2.2 + 0.6;
        ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();
      }
      ctx.restore();
      // hit flashes at the line
      this.flashes = this.flashes.filter((f) => t - f.born < 420);
      for (const f of this.flashes) {
        const k = (t - f.born) / 420;
        ctx.save();
        ctx.globalAlpha = (1 - k) * 0.8;
        ctx.fillStyle = f.color;
        ctx.shadowColor = f.color; ctx.shadowBlur = 24;
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
          const pop = k < 0.2 ? k / 0.2 : 1;          // quick scale-in
          ctx.save();
          ctx.globalAlpha = k > 0.7 ? (1 - k) / 0.3 : 1;
          ctx.fillStyle = this.judge.color;
          ctx.font = `700 ${22 * (0.7 + pop * 0.3)}px ${c.fontDisp}`;
          ctx.textAlign = "center"; ctx.textBaseline = "middle";
          ctx.shadowColor = this.judge.color; ctx.shadowBlur = 18;
          ctx.fillText(this.judge.text, this.centerX, this.hitY - 46);
          ctx.restore();
        }
      }
    }
  }

  // helpers ──────────────────────────────────────────────────────────────
  function roundRect(ctx, x, y, w, h, r) {
    if (typeof r === "number") r = [r, r, r, r];
    const [tl, tr, br, bl] = r;
    ctx.beginPath();
    ctx.moveTo(x + tl, y);
    ctx.lineTo(x + w - tr, y); ctx.arcTo(x + w, y, x + w, y + tr, tr);
    ctx.lineTo(x + w, y + h - br); ctx.arcTo(x + w, y + h, x + w - br, y + h, br);
    ctx.lineTo(x + bl, y + h); ctx.arcTo(x, y + h, x, y + h - bl, bl);
    ctx.lineTo(x, y + tl); ctx.arcTo(x, y, x + tl, y, tl);
    ctx.closePath();
  }
  function withAlpha(col, a) {
    if (col.startsWith("oklch")) return col.replace(")", ` / ${a})`);
    if (col.startsWith("#")) {
      const n = col.slice(1);
      const v = n.length === 3 ? n.split("").map((c) => c + c).join("") : n;
      const r = parseInt(v.slice(0, 2), 16), g = parseInt(v.slice(2, 4), 16), b = parseInt(v.slice(4, 6), 16);
      return `rgba(${r},${g},${b},${a})`;
    }
    return col;
  }
  // shade a hex color lighter (+) or darker (-); passthrough for oklch.
  function shade(col, amt) {
    if (!col.startsWith("#")) {
      if (col.startsWith("oklch")) {
        return col.replace(/oklch\(([\d.]+)/, (m, l) => `oklch(${clamp(parseFloat(l) + amt, 0, 1)}`);
      }
      return col;
    }
    const n = col.slice(1);
    const v = n.length === 3 ? n.split("").map((c) => c + c).join("") : n;
    let r = parseInt(v.slice(0, 2), 16), g = parseInt(v.slice(2, 4), 16), b = parseInt(v.slice(4, 6), 16);
    const f = (x) => clamp(Math.round(x + 255 * amt), 0, 255);
    return `rgb(${f(r)},${f(g)},${f(b)})`;
  }

  window.HighwayCanvas = HighwayCanvas;
})();
