// record.js — RecordCanvas: the recording counterpart to HighwayCanvas.
// Same shared model (window.RC: 88-key geometry + the Ember Lantern loop),
// but instead of notes FALLING toward the keyboard to be played, notes are
// EMITTED at the keyboard as the player performs and RISE upward, leaving a
// trail of what's been captured. Four visualizations share one engine:
//   • "ribbons"        rising pitch-coloured note ribbons growing out of keys
//   • "staff"          stylized grand-staff notation that fills as you play
//   • "ribbons+staff"  combination — ribbons rise and crystallize into notation
//   • "roll"           DAW-style horizontal piano-roll edit timeline
// Colours follow the Spectrum scheme (hue by pitch class). The sample take
// loops; each loop the capture clears and refills, so the screen stays live.
(function () {
  const { LOWEST, HIGHEST, isBlack, pitchClass, noteName, keyLayout } = window.RC;

  const lerp = (a, b, t) => a + (b - a) * t;
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const hueOf = (n) => (pitchClass(n) * 30 + 8) % 360;
  const spec = (n, l = 0.72, c = 0.16) => `oklch(${l} ${c} ${hueOf(n)})`;

  // Single global rAF ticker drives every live engine. This survives the
  // case where one logical prototype is mounted in two DOM locations at once
  // (e.g. the design-canvas card AND its fullscreen focus overlay) — each has
  // its own canvas, and both get ticked as long as they're connected.
  const RC_TICKER = {
    set: new Set(),
    raf: null,
    frames: 0,
    add(e) { this.set.add(e); this.ensure(); },
    remove(e) { this.set.delete(e); },
    ensure() {
      if (this.raf != null) return;
      const loop = () => { this.frames++; for (const e of this.set) e.tick(); this.raf = requestAnimationFrame(loop); };
      this.raf = requestAnimationFrame(loop);
    },
  };
  window.__ticker = RC_TICKER;

  // letter-name (diatonic) index from C0 — used to lay notes on a staff.
  const WHITE_STEP = { 0: 0, 2: 1, 4: 2, 5: 3, 7: 4, 9: 5, 11: 6 };
  function diatonic(n) {
    const pc = n % 12;
    const oct = Math.floor(n / 12) - 1;
    // black keys borrow the white key just below (sharp spelling)
    const base = isBlack(n) ? (n - 1) % 12 : pc;
    return oct * 7 + WHITE_STEP[base];
  }

  const DEFAULTS = {
    viz: "ribbons",
    bg: "#0f1016",
    kbRatio: 0.17,
    window: 4200,          // ms of capture shown rising above the keys
    labels: true,
    glow: 0.55,
    radius: 4,
    noteGap: 0.2,
    showKeyboard: true,
    recording: true,
    paused: false,
    selectNote: true,      // roll/edit: show one selected note w/ handles
    accent: "#ff4d57",     // record red — playhead / hit-line tint
    fontMono: "'IBM Plex Mono', ui-monospace, monospace",
    fontDisp: "'Space Grotesk', system-ui, sans-serif",
    fontMusic: "'Bravura', 'Apple Symbols', 'Segoe UI Symbol', serif",
  };

  class RecordCanvas {
    constructor(canvas, cfg) {
      this.canvas = canvas;
      this.ctx = canvas.getContext("2d");
      this.cfg = Object.assign({}, DEFAULTS, cfg);
      this.song = window.RC.SONG;
      this.t0 = performance.now();
      this.tNow = 0;
      this.lastNow = 0;
      this.level = 0;
      this.held = new Set();
      this.particles = [];
      this.rings = [];
      this.keyGlow = {};        // note -> {born, vel}
      // pitch span actually used by the take (for roll + ruler)
      let lo = 200, hi = 0;
      for (const nt of this.song.notes) { lo = Math.min(lo, nt.note); hi = Math.max(hi, nt.note); }
      this.pMin = lo - 2; this.pMax = hi + 2;
      // pick a melody note to show "selected" in edit views
      this.sel = this.song.notes.find((n) => n.hand === "R" && n.note >= 84) || this.song.notes[0];
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
      this.boardW = this.w;
      this.kl = keyLayout(this.boardW);
      this.kbH = this.cfg.showKeyboard ? Math.round(this.h * this.cfg.kbRatio) : 0;
      this.hitY = this.h - this.kbH;
      this.centerX = this.boardW / 2;
      // roll geometry
      this.rollX0 = 50; this.rollTop = 30;
      this.rollX1 = this.w - 14; this.rollBot = this.hitY - 12;
      // staff geometry computed per-frame (depends on split)
    }

    start() {
      this._running = true;
      RC_TICKER.add(this);
    }
    stop() { this._running = false; RC_TICKER.remove(this); this._ro.disconnect(); }

    tick() {
      if (!this._running) return;
      if (!this.canvas.isConnected) return;   // skip off-DOM clones
      try { this.frame(); } catch (e) { if (!this._err) { this._err = String(e && e.stack || e); console.error("record frame error", e); } }
    }

    clockNow() {
      if (this.cfg.paused) return this.tNow;            // freeze when paused
      const elapsed = performance.now() - this.t0;
      return elapsed % this.song.LOOP;
    }

    // ── shared updates ──────────────────────────────────────────────────
    updateLive(now) {
      // detect note onsets crossed since last frame -> spark + ring + glow
      const wrapped = now < this.lastNow;
      this.held.clear();
      let lvl = 0;
      for (const nt of this.song.notes) {
        if (nt.start <= now && now < nt.end) { this.held.add(nt.note); lvl += nt.vel; }
        const crossed = wrapped
          ? (nt.start > this.lastNow || nt.start <= now)
          : (nt.start > this.lastNow && nt.start <= now);
        if (crossed && !this.cfg.paused) this.fire(nt);
      }
      this.level = lerp(this.level, clamp(lvl / 180, 0, 1), 0.25);
      this.lastNow = now;
    }

    fire(nt) {
      const lane = this.kl.byNote[nt.note];
      if (!lane) return;
      this.keyGlow[nt.note] = { born: performance.now(), vel: nt.vel };
      const col = spec(nt.note, 0.78);
      const x = lane.cx, y = this.hitY;
      this.rings.push({ x, y, w: lane.w, color: col, born: performance.now() });
      const n = 4 + Math.round((nt.vel / 127) * 5);
      for (let i = 0; i < n; i++) {
        const a = -Math.PI / 2 + (Math.random() - 0.5) * 1.0;
        const sp = 1 + Math.random() * 2.6 * (nt.vel / 100);
        this.particles.push({ x, y, vx: Math.cos(a) * sp, vy: Math.sin(a) * sp * 1.6, color: col, born: performance.now(), life: 460 + Math.random() * 320 });
      }
    }

    frame() {
      const ctx = this.ctx, c = this.cfg;
      const now = this.tNow = this.clockNow();
      this.updateLive(now);
      ctx.clearRect(0, 0, this.w, this.h);
      ctx.fillStyle = c.bg;
      ctx.fillRect(0, 0, this.w, this.h);

      if (c.viz === "roll") {
        this.drawRoll(now);
      } else if (c.viz === "staff") {
        this.drawStaff(now, 18, this.hitY - 18);
        this.drawHitLine(0.5);
      } else if (c.viz === "ribbons+staff") {
        const split = Math.round(this.hitY * 0.52);
        this.drawStaff(now, 14, split - 6, true);
        this.drawBandDivider(split);
        this.drawRibbons(now, split + 8, this.hitY);
        this.drawHitLine(0.7);
      } else { // ribbons
        this.drawRibbons(now, 0, this.hitY);
        this.drawHitLine(0.7);
      }

      if (c.showKeyboard) this.drawKeyboard(now);
      this.drawLiveFx();
    }

    // ── RIBBONS (rising trails) ─────────────────────────────────────────
    drawRibbons(now, top, bottom) {
      const ctx = this.ctx, c = this.cfg;
      const H = bottom - top;
      const ppm = H / c.window;
      // faint lane tint columns
      ctx.fillStyle = "rgba(255,255,255,0.014)";
      for (const k of this.kl.whites) if (k.wi % 2 === 0) ctx.fillRect(k.x, top, k.w, H);

      for (const nt of this.song.notes) {
        if (nt.start > now) continue;                 // not recorded yet
        const lane = this.kl.byNote[nt.note];
        if (!lane) continue;
        const endC = Math.min(nt.end, now);
        let yBot = bottom - (now - endC) * ppm;        // young edge (near keys)
        let yTop = bottom - (now - nt.start) * ppm;    // old edge (rising)
        if (yBot < top || yTop > bottom + 4) continue;
        yTop = Math.max(yTop, top - 2);
        const held = now < nt.end;
        const gap = lane.black ? 0.08 : c.noteGap;
        const halfW = (lane.w * (1 - gap)) / 2;
        const L = lane.cx - halfW;
        const depth = clamp((yBot - top) / H, 0, 1);   // 1 near keys
        const col = spec(nt.note, 0.62 + depth * 0.16);
        const fadeTop = clamp((bottom - yTop) / H, 0, 1);

        ctx.save();
        if (c.glow) { ctx.shadowColor = col; ctx.shadowBlur = (6 + depth * 16) * c.glow; }
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
        ctx.moveTo(L, yBot); ctx.lineTo(L + halfW * 2, yBot);
        ctx.lineWidth = held ? 3 : 1.6;
        ctx.strokeStyle = withAlpha("#ffffff", held ? 0.92 : 0.4);
        if (held) { ctx.shadowColor = col; ctx.shadowBlur = 12; }
        ctx.stroke();
        ctx.restore();

        if (c.labels && yBot - yTop > 15 && !lane.black) {
          ctx.save();
          ctx.fillStyle = withAlpha("#0a0a0c", 0.82);
          ctx.font = `600 ${Math.min(11, lane.w * 0.66)}px ${c.fontMono}`;
          ctx.textAlign = "center"; ctx.textBaseline = "middle";
          ctx.fillText(noteName(nt.note).replace(/-?\d+$/, ""), lane.cx, yBot - 8);
          ctx.restore();
        }
      }
    }

    drawBandDivider(y) {
      const ctx = this.ctx;
      const g = ctx.createLinearGradient(0, y - 10, 0, y + 10);
      g.addColorStop(0, "rgba(255,255,255,0)");
      g.addColorStop(0.5, "rgba(255,255,255,0.07)");
      g.addColorStop(1, "rgba(255,255,255,0)");
      ctx.fillStyle = g; ctx.fillRect(0, y - 10, this.w, 20);
    }

    // ── STAFF (stylized grand staff that fills) ─────────────────────────
    drawStaff(now, top, bottom, compact) {
      const ctx = this.ctx, c = this.cfg;
      const x0 = 64, x1 = this.w - 24;
      const span = x1 - x0;
      const mid = (top + bottom) / 2;
      const lineGap = compact ? 7.5 : 9;
      const step = lineGap / 2;                        // one diatonic step
      // reference: middle C (60) sits one step below treble bottom line
      const trebBottomDia = diatonic(64);              // E4 = treble bottom line
      const bassTopDia = diatonic(57);                 // A3 = bass top line
      const trebMid = mid - lineGap * 3.2;             // treble centre line y
      const bassMid = mid + lineGap * 3.2;
      const yTreb = (dia) => trebMid - (dia - diatonic(71)) * step;  // B4 centre
      const yBass = (dia) => bassMid - (dia - diatonic(50)) * step;  // D3 centre
      const yOf = (n) => (n >= 60 ? yTreb(diatonic(n)) : yBass(diatonic(n)));

      // staff lines
      ctx.strokeStyle = "rgba(255,255,255,0.16)"; ctx.lineWidth = 1;
      for (const cy of [trebMid, bassMid]) {
        for (let i = -2; i <= 2; i++) {
          const y = cy + i * lineGap;
          ctx.beginPath(); ctx.moveTo(x0, y); ctx.lineTo(x1, y); ctx.stroke();
        }
      }
      // brace + left bar
      ctx.strokeStyle = "rgba(255,255,255,0.3)"; ctx.lineWidth = 1.4;
      ctx.beginPath(); ctx.moveTo(x0, trebMid - lineGap * 2); ctx.lineTo(x0, bassMid + lineGap * 2); ctx.stroke();
      // clefs
      ctx.fillStyle = "rgba(255,255,255,0.62)";
      ctx.textAlign = "left"; ctx.textBaseline = "middle";
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
      const { BAR } = this.song; const bars = this.song.BARS;
      const barW = (x1 - notesX0) / bars;
      const xOfTime = (t) => notesX0 + (t / this.song.LOOP) * (x1 - notesX0);
      ctx.strokeStyle = "rgba(255,255,255,0.13)"; ctx.lineWidth = 1;
      for (let b = 0; b <= bars; b++) {
        const x = notesX0 + b * barW;
        ctx.beginPath(); ctx.moveTo(x, trebMid - lineGap * 2); ctx.lineTo(x, trebMid + lineGap * 2); ctx.stroke();
        ctx.beginPath(); ctx.moveTo(x, bassMid - lineGap * 2); ctx.lineTo(x, bassMid + lineGap * 2); ctx.stroke();
      }

      // playhead (write position)
      const px = xOfTime(now);
      ctx.save();
      ctx.strokeStyle = withAlpha(c.accent, 0.8); ctx.lineWidth = 1.6;
      ctx.shadowColor = c.accent; ctx.shadowBlur = 10;
      ctx.beginPath(); ctx.moveTo(px, trebMid - lineGap * 3); ctx.lineTo(px, bassMid + lineGap * 3); ctx.stroke();
      ctx.restore();

      const headRx = lineGap * 0.62, headRy = lineGap * 0.46;
      for (const nt of this.song.notes) {
        if (nt.start > now) continue;
        const x = xOfTime(nt.start);
        const y = yOf(nt.note);
        const cy = nt.note >= 60 ? trebMid : bassMid;
        const fresh = clamp(1 - (now - nt.start) / 280, 0, 1);
        const col = spec(nt.note, 0.74);
        // ledger lines
        ctx.strokeStyle = "rgba(255,255,255,0.22)"; ctx.lineWidth = 1;
        const above = y < cy - lineGap * 2 - 1, below = y > cy + lineGap * 2 + 1;
        if (above || below) {
          let ly = cy + (above ? -lineGap * 3 : lineGap * 3);
          const stepDir = above ? lineGap : -lineGap;
          for (let k = 0; k < 3; k++) {
            const lyy = cy + (above ? -1 : 1) * lineGap * (3 + k);
            if ((above && lyy >= y - 1) || (below && lyy <= y + 1)) {
              ctx.beginPath(); ctx.moveTo(x - headRx - 4, lyy); ctx.lineTo(x + headRx + 4, lyy); ctx.stroke();
            }
          }
        }
        // stem
        const stemUp = y > (nt.note >= 60 ? trebMid : bassMid);
        ctx.strokeStyle = withAlpha(col, 0.85); ctx.lineWidth = 1.6;
        ctx.beginPath();
        ctx.moveTo(x + (stemUp ? headRx - 0.5 : -headRx + 0.5), y);
        ctx.lineTo(x + (stemUp ? headRx - 0.5 : -headRx + 0.5), y + (stemUp ? -lineGap * 3.3 : lineGap * 3.3));
        ctx.stroke();
        // accidental
        if (isBlack(nt.note)) {
          ctx.fillStyle = withAlpha("#ffffff", 0.6);
          ctx.font = `${lineGap * 2}px ${c.fontMusic}`;
          ctx.textAlign = "right"; ctx.textBaseline = "middle";
          ctx.fillText("\u266F", x - headRx - 2, y);
        }
        // head
        ctx.save();
        ctx.translate(x, y); ctx.rotate(-0.32);
        if (fresh > 0) { ctx.shadowColor = col; ctx.shadowBlur = 14 * fresh; }
        ctx.fillStyle = col;
        ctx.beginPath(); ctx.ellipse(0, 0, headRx, headRy, 0, 0, Math.PI * 2); ctx.fill();
        if (fresh > 0.05) {
          ctx.globalAlpha = fresh; ctx.fillStyle = "#fff";
          ctx.beginPath(); ctx.ellipse(0, 0, headRx * 0.5, headRy * 0.5, 0, 0, Math.PI * 2); ctx.fill();
        }
        ctx.restore();
      }
    }

    // ── ROLL (DAW horizontal piano-roll edit timeline) ──────────────────
    drawRoll(now) {
      const ctx = this.ctx, c = this.cfg;
      const X0 = this.rollX0, X1 = this.rollX1, T = this.rollTop, B = this.rollBot;
      const rowH = (B - T) / (this.pMax - this.pMin + 1);
      const yOfP = (n) => T + (this.pMax - n) * rowH;
      const xOf = (t) => X0 + (t / this.song.LOOP) * (X1 - X0);
      const { BEAT, BAR } = this.song;

      // pitch rows — tint black-key rows, label each C
      for (let n = this.pMin; n <= this.pMax; n++) {
        const y = yOfP(n);
        ctx.fillStyle = isBlack(n) ? "rgba(255,255,255,0.022)" : "rgba(255,255,255,0.004)";
        ctx.fillRect(X0, y, X1 - X0, rowH);
        if (n % 12 === 0) {
          ctx.fillStyle = "rgba(255,255,255,0.04)"; ctx.fillRect(X0, y, X1 - X0, rowH);
          ctx.fillStyle = "rgba(255,255,255,0.42)";
          ctx.font = `500 9px ${c.fontMono}`; ctx.textAlign = "right"; ctx.textBaseline = "middle";
          ctx.fillText(noteName(n), X0 - 6, y + rowH / 2);
        }
      }
      // mini vertical keyboard guide
      for (let n = this.pMin; n <= this.pMax; n++) {
        const y = yOfP(n);
        ctx.fillStyle = isBlack(n) ? "#16171d" : "#23242c";
        ctx.fillRect(X0 - 30, y + 0.5, 24, rowH - 1);
        if (this.held.has(n)) { ctx.fillStyle = spec(n, 0.7); ctx.fillRect(X0 - 30, y + 0.5, 24, rowH - 1); }
      }
      ctx.strokeStyle = "rgba(0,0,0,0.4)"; ctx.lineWidth = 1;
      ctx.strokeRect(X0 - 30, T, 24, B - T);

      // grid: beats (faint) + bars (strong) + snap subdivisions
      for (let t = 0; t <= this.song.LOOP; t += BEAT / 2) {
        const x = xOf(t); const isBar = t % BAR === 0; const isBeat = t % BEAT === 0;
        ctx.strokeStyle = isBar ? "rgba(255,255,255,0.16)" : isBeat ? "rgba(255,255,255,0.07)" : "rgba(255,255,255,0.03)";
        ctx.lineWidth = isBar ? 1.4 : 1;
        ctx.beginPath(); ctx.moveTo(x, T); ctx.lineTo(x, B); ctx.stroke();
      }
      // time ruler
      ctx.fillStyle = "rgba(255,255,255,0.5)"; ctx.font = `500 9px ${c.fontMono}`; ctx.textAlign = "left"; ctx.textBaseline = "alphabetic";
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
        if (fresh > 0 || isSel) { ctx.shadowColor = col; ctx.shadowBlur = isSel ? 10 : 10 * fresh; }
        const g = ctx.createLinearGradient(x, 0, xe, 0);
        g.addColorStop(0, withAlpha(col, 1)); g.addColorStop(1, withAlpha(col, 0.78));
        ctx.fillStyle = g;
        roundRect(ctx, x, y, Math.max(3, xe - x), hgt, Math.min(3, hgt / 2));
        ctx.fill();
        // velocity cap (left edge intensity)
        ctx.fillStyle = withAlpha("#fff", 0.18 + (nt.vel / 127) * 0.5);
        ctx.fillRect(x, y, 2.5, hgt);
        ctx.restore();
        if (isSel) {
          ctx.strokeStyle = "#fff"; ctx.lineWidth = 1.4;
          roundRect(ctx, x - 0.5, y - 0.5, Math.max(3, xe - x) + 1, hgt + 1, Math.min(3, hgt / 2)); ctx.stroke();
          // resize handles
          ctx.fillStyle = "#fff";
          for (const hx of [x, xe]) ctx.fillRect(hx - 1.5, y + hgt / 2 - 3, 3, 6);
        }
      }

      // playhead
      const px = xOf(now);
      ctx.save();
      ctx.strokeStyle = c.accent; ctx.lineWidth = 1.6; ctx.shadowColor = c.accent; ctx.shadowBlur = 10;
      ctx.beginPath(); ctx.moveTo(px, T - 4); ctx.lineTo(px, B); ctx.stroke();
      ctx.fillStyle = c.accent;
      ctx.beginPath(); ctx.moveTo(px - 4, T - 4); ctx.lineTo(px + 4, T - 4); ctx.lineTo(px, T + 2); ctx.closePath(); ctx.fill();
      ctx.restore();
    }

    drawHitLine(a) {
      const ctx = this.ctx, y = this.hitY;
      const g = ctx.createLinearGradient(0, y - 18, 0, y);
      g.addColorStop(0, "rgba(255,255,255,0)");
      g.addColorStop(1, withAlpha(this.cfg.accent, 0.08 * a + 0.04));
      ctx.fillStyle = g; ctx.fillRect(0, y - 18, this.w, 18);
      ctx.beginPath(); ctx.moveTo(0, y + 0.5); ctx.lineTo(this.w, y + 0.5);
      ctx.lineWidth = 2; ctx.strokeStyle = withAlpha("#ffffff", 0.18 * a + 0.1); ctx.stroke();
    }

    // ── KEYBOARD (flat, spectrum tints on held / glowing keys) ──────────
    drawKeyboard(now) {
      const ctx = this.ctx, c = this.cfg;
      if (this.kbH <= 0) return;
      const top = this.hitY, h = this.kbH, t = performance.now();
      // top bezel
      ctx.fillStyle = "rgba(0,0,0,0.55)"; ctx.fillRect(0, top - 3, this.w, 3);
      for (const k of this.kl.whites) {
        ctx.fillStyle = "#eceae3"; ctx.fillRect(k.x + 0.5, top, k.w - 1, h);
        const g = this.keyGlow[k.note];
        const lit = this.held.has(k.note);
        const tintA = lit ? 0.9 : g ? clamp(1 - (t - g.born) / 320, 0, 1) * 0.8 : 0;
        if (tintA > 0) {
          ctx.save(); ctx.globalAlpha = tintA;
          ctx.fillStyle = spec(k.note, 0.78); ctx.fillRect(k.x + 0.5, top, k.w - 1, h);
          ctx.restore();
        }
        ctx.strokeStyle = "rgba(0,0,0,0.2)"; ctx.lineWidth = 1; ctx.strokeRect(k.x + 0.5, top, k.w - 1, h);
        ctx.fillStyle = "rgba(0,0,0,0.05)"; ctx.fillRect(k.x + 0.5, top + h - 3, k.w - 1, 3);
        if (c.labels && k.note % 12 === 0) {
          ctx.fillStyle = "rgba(20,20,24,0.5)"; ctx.font = `500 ${Math.min(9, k.w * 0.6)}px ${c.fontMono}`;
          ctx.textAlign = "center"; ctx.textBaseline = "bottom";
          ctx.fillText(noteName(k.note), k.x + k.w / 2, top + h - 3);
        }
      }
      const bh = h * 0.62;
      for (const k of this.kl.blacks) {
        ctx.fillStyle = "#191a1f";
        roundRect(ctx, k.x, top, k.w, bh, [0, 0, 2, 2]); ctx.fill();
        const g = this.keyGlow[k.note];
        const lit = this.held.has(k.note);
        const tintA = lit ? 0.95 : g ? clamp(1 - (t - g.born) / 320, 0, 1) * 0.9 : 0;
        if (tintA > 0) {
          ctx.save(); ctx.globalAlpha = tintA;
          ctx.fillStyle = spec(k.note, 0.6);
          roundRect(ctx, k.x, top, k.w, bh, [0, 0, 2, 2]); ctx.fill();
          ctx.restore();
        }
        ctx.fillStyle = "rgba(255,255,255,0.08)"; ctx.fillRect(k.x + 1, top + 1, k.w - 2, 2);
      }
    }

    drawLiveFx() {
      const ctx = this.ctx, t = performance.now();
      this.particles = this.particles.filter((p) => t - p.born < p.life);
      ctx.save();
      for (const p of this.particles) {
        const age = (t - p.born) / p.life, dt = (t - p.born) / 16;
        const x = p.x + p.vx * dt, y = p.y + p.vy * dt + 0.04 * dt * dt;
        ctx.globalAlpha = (1 - age) * 0.9; ctx.fillStyle = p.color;
        ctx.shadowColor = p.color; ctx.shadowBlur = 8;
        const r = (1 - age) * 2 + 0.5;
        ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();
      }
      ctx.restore();
      this.rings = this.rings.filter((f) => t - f.born < 460);
      for (const f of this.rings) {
        const k = (t - f.born) / 460;
        ctx.save(); ctx.globalAlpha = (1 - k) * 0.7; ctx.strokeStyle = f.color;
        ctx.shadowColor = f.color; ctx.shadowBlur = 16; ctx.lineWidth = 2;
        const r = f.w * (0.4 + k * 1.8);
        ctx.beginPath(); ctx.ellipse(f.x, f.y, r, r * 0.45, 0, Math.PI, Math.PI * 2); ctx.stroke();
        ctx.restore();
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
  // mix hex a toward (possibly oklch) b by t — returns a usable color string.
  function mix(a, b, t) {
    if (b.startsWith("oklch")) return withAlpha(b, clamp(t, 0, 1));
    return a;
  }

  window.RecordCanvas = RecordCanvas;
})();
